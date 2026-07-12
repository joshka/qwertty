//! win32-input-mode (`CSI … _`) semantic decode.
//!
//! This module turns a [`ControlSequence`] whose final byte is `_` into the [`KeyEvent`]
//! vocabulary, the wire format Windows Terminal's `ConPTY` sends when an application enables
//! private mode 9001 (`CSI ? 9001 h`). It is the terminal-to-host mirror of a Win32
//! `KEY_EVENT_RECORD`, one CSI per console key event:
//!
//! ```text
//! CSI Vk ; Sc ; Uc ; Kd ; Cs ; Rc _
//! ```
//!
//! Every field is an optional decimal parameter (an omitted or empty field takes its default), so
//! `CSI _` is a legal degenerate event. The fields are the record's members: `Vk` the virtual-key
//! code, `Sc` the scan code (ignored — a decoder needs the virtual key and character, not the
//! physical position), `Uc` the UTF-16 code unit as a decimal (`0` = none), `Kd` the key-down flag
//! (`1` down, `0` up), `Cs` the `dwControlKeyState` bitfield, and `Rc` the repeat count (omitted
//! defaults to `1`).
//!
//! # Mapping into the frozen vocabulary
//!
//! The [`Event`](crate::Event) vocabulary is frozen (ADR 0019), so win32 concepts map onto the
//! existing [`Key`]/[`Modifiers`]/[`KeyEventKind`] shape rather than growing new variants; a
//! concept with no lossless home is dropped here and the CSI still round-trips byte-for-byte
//! through the syntax layer:
//!
//! - **Kind.** `Kd = 1` is [`KeyEventKind::Press`], `Kd = 0` is [`KeyEventKind::Release`]. Win32
//!   cannot distinguish an auto-repeat from a fresh press, so this decode **never** emits
//!   [`KeyEventKind::Repeat`] (unlike the kitty decode, which has an explicit event-type subfield).
//! - **Modifiers.** The `Cs` bitfield collapses the two positional Alt bits
//!   (`LEFT_ALT`/`RIGHT_ALT`) into [`Modifiers::ALT`] and the two positional Ctrl bits into
//!   [`Modifiers::CTRL`]; `SHIFT` is [`Modifiers::SHIFT`]. `NUMLOCK` and `CAPSLOCK` map to the lock
//!   bits the frozen [`Modifiers`] already carries. `SCROLLLOCK` and `ENHANCED_KEY` have no home in
//!   the frozen set and are dropped, as is the Windows key (which never appears in `Cs` — it
//!   arrives only as standalone `VK_LWIN`/`VK_RWIN` chatter).
//! - **Key identity.** A `Uc` that is a real (non-control) character is preferred, becoming a
//!   [`Key::Char`]; otherwise the virtual key selects a named key (arrows, the navigation cluster,
//!   `F1`–`F24`, `Enter`/`Tab`/`Backspace`/`Escape`) or a base character for the letter and digit
//!   keys (so a control chord like `Ctrl+A`, whose `Uc` is the `SOH` control code, still resolves
//!   to [`Key::Char('a')`](Key::Char)). A modifier or lock virtual key with `Uc = 0` (`VK_SHIFT`,
//!   `VK_CONTROL`, `VK_MENU`, the `VK_L*`/`VK_R*` halves, `VK_LWIN`/`VK_RWIN`,
//!   `VK_CAPITAL`/`VK_NUMLOCK`/`VK_SCROLL`) is modifier chatter and emits nothing, as does any
//!   otherwise-unrecognized virtual key with `Uc = 0`.
//!
//! # Surrogate pairs and the repeat cap
//!
//! A code point above the basic plane arrives as two consecutive events carrying the UTF-16
//! surrogate halves. A high surrogate is held in the decoder — a single `u16`, the only state this
//! module keeps — and the following low surrogate completes the character (using that **second**
//! event's `Kd`/`Cs`, since the pair shares one key). A high surrogate followed by anything other
//! than its low half flushes [`U+FFFD`](char::REPLACEMENT_CHARACTER) and processes the interrupting
//! event normally; a leftover high surrogate at end of input flushes the same way through
//! [`Win32Decoder::flush`]. The pending half is bounded to one unit and resets whenever the owning
//! decoder is recreated.
//!
//! The repeat count is expanded into that many identical events, capped at
//! [`REPEAT_CAP`](self::REPEAT_CAP): a hostile or buggy peer can claim a repeat count up to
//! `2^31`, so the expansion is bounded to keep decode memory constant. A count of `0` or `1` both
//! mean a single event.

use crate::event::key::{Key, KeyEvent, KeyEventKind, Modifiers};
use crate::syntax::{ControlParams, ControlSequence, ParamSeparator};

/// The most identical events a single win32 event's repeat count expands into.
///
/// The repeat count is an untrusted `u16`-to-`u32` field; capping the expansion keeps decode memory
/// bounded by a constant no matter what a peer claims. A real console repeat is almost always `1`.
const REPEAT_CAP: u32 = 32;

// `dwControlKeyState` bit constants (Win32 `wincon.h`). The two Alt and two Ctrl bits are
// positional (left/right); this decode collapses each pair into one frozen modifier.
const RIGHT_ALT_PRESSED: u32 = 0x0001;
const LEFT_ALT_PRESSED: u32 = 0x0002;
const RIGHT_CTRL_PRESSED: u32 = 0x0004;
const LEFT_CTRL_PRESSED: u32 = 0x0008;
const SHIFT_PRESSED: u32 = 0x0010;
const NUMLOCK_ON: u32 = 0x0020;
const CAPSLOCK_ON: u32 = 0x0080;
// SCROLLLOCK_ON (0x0040) and ENHANCED_KEY (0x0100) have no home in the frozen `Modifiers` and are
// dropped; see the module docs.

// UTF-16 surrogate ranges. A code point above U+FFFF arrives as a high half then a low half.
const HIGH_SURROGATE_START: u32 = 0xD800;
const HIGH_SURROGATE_END: u32 = 0xDBFF;
const LOW_SURROGATE_START: u32 = 0xDC00;
const LOW_SURROGATE_END: u32 = 0xDFFF;

/// Streaming decode state for win32-input-mode.
///
/// The only state is the pending high surrogate half of a not-yet-completed astral character, held
/// between two consecutive events. It is bounded to exactly one `u16`, and is owned by the semantic
/// decoder so it resets with it. A fresh decoder holds nothing pending.
#[derive(Clone, Debug, Default)]
pub(crate) struct Win32Decoder {
    /// The high surrogate of an astral character whose low half has not yet arrived, if any.
    pending_high_surrogate: Option<u16>,
}

impl Win32Decoder {
    /// Decodes a win32-input event, appending the key events it produces to `out`.
    ///
    /// Returns `true` when `csi` is a win32-input event (final byte `_`, no private markers or
    /// intermediates, no colon sub-parameters), even when it produces no key event (a modifier-key
    /// chatter event is a well-formed win32 event that simply carries no key). Returns `false` when
    /// the sequence is not win32-input, so the caller passes it through as lossless syntax.
    pub(crate) fn decode_key(&mut self, csi: &ControlSequence, out: &mut Vec<KeyEvent>) -> bool {
        let params = csi.params();
        if params.final_byte() != b'_' {
            return false;
        }
        // A win32-input event never carries a private marker or an intermediate byte, and its
        // fields are `;`-separated with no `:` sub-parameters. Anything else sharing the
        // `_` final is not this protocol and must round-trip as syntax rather than decode.
        if !params.private_markers().is_empty() || !params.intermediates().is_empty() {
            return false;
        }
        let Some(fields) = positional_fields(params) else {
            return false;
        };
        self.decode_fields(&fields, out);
        true
    }

    /// Flushes a leftover high surrogate as [`U+FFFD`](char::REPLACEMENT_CHARACTER) at end of
    /// input.
    ///
    /// The semantic decoder calls this from its finish path so a stream that ends on a dangling
    /// high surrogate does not silently drop it. It is a no-op when nothing is pending.
    pub(crate) fn flush(&mut self, out: &mut Vec<KeyEvent>) {
        if self.pending_high_surrogate.take().is_some() {
            out.push(replacement_event());
        }
    }

    /// Decodes the six already-defaulted fields into zero or more key events.
    fn decode_fields(&mut self, fields: &Fields, out: &mut Vec<KeyEvent>) {
        let Fields { vk, uc, kd, cs, rc } = *fields;
        let count = repeat_count(rc);
        let kind = if kd == 0 {
            KeyEventKind::Release
        } else {
            KeyEventKind::Press
        };
        let modifiers = decode_modifiers(cs);

        if (HIGH_SURROGATE_START..=HIGH_SURROGATE_END).contains(&uc) {
            // A high surrogate is held for its low half. A previous unpaired high is orphaned and
            // flushes first; only one half is ever pending.
            if self.pending_high_surrogate.take().is_some() {
                out.push(replacement_event());
            }
            self.pending_high_surrogate = Some(u16::try_from(uc).unwrap_or_default());
            return;
        }
        if (LOW_SURROGATE_START..=LOW_SURROGATE_END).contains(&uc) {
            match self.pending_high_surrogate.take() {
                // The pair completes; the pair shares one key, so this second event's kind and
                // modifiers describe it.
                Some(high) => {
                    let character = combine_surrogates(high, uc);
                    push_repeated(out, text_key_event(character, modifiers, kind), count);
                }
                // A low surrogate with no pending high is a broken pair.
                None => push_repeated(
                    out,
                    text_key_event(char::REPLACEMENT_CHARACTER, modifiers, kind),
                    count,
                ),
            }
            return;
        }
        // A non-surrogate event interrupts any pending high surrogate: flush it, then decode this
        // event on its own.
        if self.pending_high_surrogate.take().is_some() {
            out.push(replacement_event());
        }
        if let Some(event) = key_event(vk, uc, modifiers, kind) {
            push_repeated(out, event, count);
        }
    }
}

/// The six win32-input fields, already resolved to their defaults (`Sc` is dropped — unused).
#[derive(Clone, Copy)]
struct Fields {
    vk: u32,
    uc: u32,
    kd: u32,
    cs: u32,
    rc: u32,
}

/// Reads the positional parameters into the six fields with their protocol defaults, or `None` when
/// a `:` sub-parameter shows the sequence is not win32-input grammar.
///
/// Fields default to `0` except the repeat count, which defaults to `1` (an omitted count is one
/// event). Parameters past the sixth are ignored; the syntax layer already bounds how many it
/// parses.
fn positional_fields(params: &ControlParams) -> Option<Fields> {
    // Vk, Sc, Uc, Kd, Cs default to 0; Rc defaults to 1.
    let mut values = [0u32, 0, 0, 0, 0, 1];
    for (index, param) in params.params().iter().enumerate() {
        if param.separator() == Some(ParamSeparator::Colon) {
            return None;
        }
        if let (Some(value), Some(slot)) = (param.value(), values.get_mut(index)) {
            *slot = value;
        }
    }
    Some(Fields {
        vk: values[0],
        uc: values[2],
        kd: values[3],
        cs: values[4],
        rc: values[5],
    })
}

/// Expands a raw repeat count into the number of events to emit: at least one, at most
/// [`REPEAT_CAP`].
fn repeat_count(rc: u32) -> usize {
    rc.clamp(1, REPEAT_CAP) as usize
}

/// Collapses a `dwControlKeyState` bitfield into the frozen [`Modifiers`].
///
/// The two positional Alt bits and two positional Ctrl bits each collapse to one modifier; Shift,
/// Num Lock, and Caps Lock map directly. Scroll Lock and the enhanced-key bit have no frozen home
/// and are dropped.
fn decode_modifiers(cs: u32) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    if cs & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
        modifiers.insert(Modifiers::ALT);
    }
    if cs & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        modifiers.insert(Modifiers::CTRL);
    }
    if cs & SHIFT_PRESSED != 0 {
        modifiers.insert(Modifiers::SHIFT);
    }
    if cs & NUMLOCK_ON != 0 {
        modifiers.insert(Modifiers::NUM_LOCK);
    }
    if cs & CAPSLOCK_ON != 0 {
        modifiers.insert(Modifiers::CAPS_LOCK);
    }
    modifiers
}

/// Builds the key event for one non-surrogate win32 event, or `None` when it carries no key.
///
/// A real character in `Uc` wins and becomes a [`Key::Char`]; otherwise the virtual key names the
/// key. A virtual key with no mapping and no character (modifier/lock chatter, or an unrecognized
/// key) yields `None`, so nothing is emitted.
fn key_event(vk: u32, uc: u32, modifiers: Modifiers, kind: KeyEventKind) -> Option<KeyEvent> {
    if let Some(character) = char::from_u32(uc)
        && is_text_char(character)
    {
        return Some(text_key_event(character, modifiers, kind));
    }
    let key = named_key(vk)?;
    Some(KeyEvent::new(key).with_modifiers(modifiers).with_kind(kind))
}

/// Maps a virtual-key code to a [`Key`], or `None` for modifier/lock chatter and unknown keys.
///
/// The named keys are the ones with no character of their own (navigation and function keys, and
/// the C0-named keys whose `Uc` is a control code). The letter and digit keys map to their
/// unmodified base character so a control chord — where `Uc` holds a control code, not the letter —
/// still names the character it was pressed with.
fn named_key(vk: u32) -> Option<Key> {
    Some(match vk {
        0x08 => Key::Backspace, // VK_BACK
        0x09 => Key::Tab,       // VK_TAB
        0x0D => Key::Enter,     // VK_RETURN
        0x1B => Key::Escape,    // VK_ESCAPE
        0x21 => Key::PageUp,    // VK_PRIOR
        0x22 => Key::PageDown,  // VK_NEXT
        0x23 => Key::End,       // VK_END
        0x24 => Key::Home,      // VK_HOME
        0x25 => Key::Left,      // VK_LEFT
        0x26 => Key::Up,        // VK_UP
        0x27 => Key::Right,     // VK_RIGHT
        0x28 => Key::Down,      // VK_DOWN
        0x2D => Key::Insert,    // VK_INSERT
        0x2E => Key::Delete,    // VK_DELETE
        // VK_0..VK_9 equal ASCII '0'..'9'; the code point is always a valid scalar value.
        0x30..=0x39 => Key::Char(char::from_u32(vk).unwrap_or(char::REPLACEMENT_CHARACTER)),
        // VK_A..VK_Z equal ASCII 'A'..'Z'; the unmodified key produces the lowercase letter.
        0x41..=0x5A => Key::Char(char::from_u32(vk + 0x20).unwrap_or(char::REPLACEMENT_CHARACTER)),
        // VK_F1 (0x70) .. VK_F24 (0x87) are contiguous, so the F-number is 1..=24, always a valid
        // u8.
        0x70..=0x87 => Key::Function(u8::try_from(vk - 0x70 + 1).unwrap_or(u8::MAX)),
        // Modifier and lock keys (VK_SHIFT/CONTROL/MENU, the L*/R* halves, VK_LWIN/RWIN,
        // VK_CAPITAL/NUMLOCK/SCROLL) and every other virtual key carry no key of their own here.
        _ => return None,
    })
}

/// Builds a character key event, carrying the character as text on a press (a release has no text).
fn text_key_event(character: char, modifiers: Modifiers, kind: KeyEventKind) -> KeyEvent {
    let event = KeyEvent::new(Key::Char(character))
        .with_modifiers(modifiers)
        .with_kind(kind);
    if kind == KeyEventKind::Press {
        event.with_text(character)
    } else {
        event
    }
}

/// The event a flushed, unpaired high surrogate becomes: a bare
/// [`U+FFFD`](char::REPLACEMENT_CHARACTER) press. The orphaned half carried no reusable kind or
/// modifiers, so none are attached.
fn replacement_event() -> KeyEvent {
    KeyEvent::new(Key::Char(char::REPLACEMENT_CHARACTER)).with_text(char::REPLACEMENT_CHARACTER)
}

/// Combines a validated UTF-16 surrogate pair into its `char`.
///
/// The high half is the held code unit and `low` is the current event's `Uc`, already range-checked
/// by the caller, so the code point is a valid scalar value; the [`char::REPLACEMENT_CHARACTER`]
/// fallback is unreachable defensive code.
fn combine_surrogates(high: u16, low: u32) -> char {
    let code =
        0x1_0000 + ((u32::from(high) - HIGH_SURROGATE_START) << 10) + (low - LOW_SURROGATE_START);
    char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Returns whether a character is real text rather than a control code.
///
/// Control codes (C0 below `0x20`, `DEL`, and the C1 block `0x80..=0x9f`) are not preferred as the
/// key identity: a `Uc` holding one means the virtual key should name the key instead.
fn is_text_char(character: char) -> bool {
    let code = u32::from(character);
    code >= 0x20 && code != 0x7f && !(0x80..=0x9f).contains(&code)
}

/// Appends `event` to `out` exactly `count` times (`count >= 1`).
fn push_repeated(out: &mut Vec<KeyEvent>, event: KeyEvent, count: usize) {
    for _ in 1..count {
        out.push(event.clone());
    }
    out.push(event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::key::TextPayload;
    use crate::syntax::{SyntaxParser, SyntaxToken};

    /// Parses `bytes` into the single CSI control sequence they encode.
    fn csi(bytes: &[u8]) -> ControlSequence {
        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
        match tokens.into_iter().next().expect("one token") {
            SyntaxToken::Csi(csi) => csi,
            other => panic!("expected a CSI token, got {other:?}"),
        }
    }

    /// Decodes `bytes` as a win32 event through a fresh decoder, asserting it is recognized as one.
    fn decode(bytes: &[u8]) -> Vec<KeyEvent> {
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(
            decoder.decode_key(&csi(bytes), &mut out),
            "{bytes:?} was not recognized as a win32 event",
        );
        out
    }

    /// Decodes `bytes` and asserts it produces exactly one key event, returning it.
    fn decode_one(bytes: &[u8]) -> KeyEvent {
        let mut events = decode(bytes);
        assert_eq!(
            events.len(),
            1,
            "{bytes:?} did not produce exactly one event"
        );
        events.pop().expect("one event")
    }

    #[test]
    fn plain_character_press_carries_char_and_text() {
        // 'a' down: Vk=65, Sc=30, Uc=97, Kd=1 (Cs/Rc defaulted).
        let event = decode_one(b"\x1b[65;30;97;1_");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.kind(), KeyEventKind::Press);
        assert_eq!(event.modifiers(), Modifiers::empty());
        assert_eq!(event.text().map(TextPayload::as_str), Some("a"));
    }

    #[test]
    fn key_up_with_omitted_keydown_is_a_release_without_text() {
        // 'a' up: Kd omitted defaults to 0, so this is a release; a release carries no text.
        let event = decode_one(b"\x1b[65;30;97_");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.kind(), KeyEventKind::Release);
        assert_eq!(event.text(), None);
    }

    #[test]
    fn shift_character_uses_the_shifted_unicode_char() {
        // Shift+a produces 'A' in Uc with the SHIFT bit set.
        let event = decode_one(b"\x1b[65;30;65;1;16_");
        assert_eq!(event.key(), Key::Char('A'));
        assert_eq!(event.modifiers(), Modifiers::SHIFT);
        assert_eq!(event.text().map(TextPayload::as_str), Some("A"));
    }

    #[test]
    fn control_chord_resolves_the_letter_from_the_virtual_key() {
        // Ctrl+A: Uc is the SOH control code, Cs is LEFT_CTRL (0x08, positional). The letter comes
        // from the virtual key, the modifier from the collapsed Ctrl bits, and no text is carried.
        let event = decode_one(b"\x1b[65;30;1;1;8_");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.modifiers(), Modifiers::CTRL);
        assert_eq!(event.text(), None);
    }

    #[test]
    fn right_control_bit_also_collapses_to_ctrl() {
        // RIGHT_CTRL (0x04) is the other positional Ctrl bit and collapses the same way.
        let event = decode_one(b"\x1b[65;30;1;1;4_");
        assert_eq!(event.modifiers(), Modifiers::CTRL);
    }

    #[test]
    fn arrow_key_decodes_and_drops_the_enhanced_key_bit() {
        // Left arrow: VK_LEFT (0x25=37), Uc=0, Cs=ENHANCED_KEY (0x100=256). The arrow decodes; the
        // enhanced-key bit has no frozen home and leaves no modifier.
        let event = decode_one(b"\x1b[37;75;0;1;256_");
        assert_eq!(event.key(), Key::Left);
        assert_eq!(event.modifiers(), Modifiers::empty());
        assert_eq!(event.kind(), KeyEventKind::Press);
    }

    #[test]
    fn navigation_and_named_c0_keys_decode_from_virtual_key() {
        assert_eq!(decode_one(b"\x1b[8;0;8;1_").key(), Key::Backspace);
        assert_eq!(decode_one(b"\x1b[9;0;9;1_").key(), Key::Tab);
        assert_eq!(decode_one(b"\x1b[13;0;13;1_").key(), Key::Enter);
        assert_eq!(decode_one(b"\x1b[27;0;27;1_").key(), Key::Escape);
        assert_eq!(decode_one(b"\x1b[36;0;0;1_").key(), Key::Home);
        assert_eq!(decode_one(b"\x1b[35;0;0;1_").key(), Key::End);
        assert_eq!(decode_one(b"\x1b[33;0;0;1_").key(), Key::PageUp);
        assert_eq!(decode_one(b"\x1b[34;0;0;1_").key(), Key::PageDown);
        assert_eq!(decode_one(b"\x1b[45;0;0;1_").key(), Key::Insert);
        assert_eq!(decode_one(b"\x1b[46;0;0;1_").key(), Key::Delete);
    }

    #[test]
    fn function_keys_span_f1_through_f24() {
        assert_eq!(decode_one(b"\x1b[112;0;0;1_").key(), Key::Function(1));
        assert_eq!(decode_one(b"\x1b[124;0;0;1_").key(), Key::Function(13));
        assert_eq!(decode_one(b"\x1b[135;0;0;1_").key(), Key::Function(24));
    }

    #[test]
    fn modifier_key_chatter_emits_nothing_but_is_consumed() {
        // VK_CONTROL (0x11) down with Uc=0 is modifier chatter: a well-formed win32 event that
        // carries no key. It is recognized (consumed) yet produces no event.
        assert!(decode(b"\x1b[17;29;0;1;8_").is_empty());
        // VK_SHIFT, VK_MENU (Alt), and VK_LWIN behave the same.
        assert!(decode(b"\x1b[16;42;0;1;16_").is_empty());
        assert!(decode(b"\x1b[18;56;0;1;10_").is_empty());
        assert!(decode(b"\x1b[91;0;0;1_").is_empty());
    }

    #[test]
    fn unknown_virtual_key_with_no_char_emits_nothing() {
        // An OEM/punctuation key qwertty does not name, with Uc=0, produces no event but does not
        // error (a fidelity gap, not a failure).
        assert!(decode(b"\x1b[186;0;0;1_").is_empty());
    }

    #[test]
    fn repeat_count_expands_to_that_many_events() {
        let events = decode(b"\x1b[65;30;97;1;0;3_");
        assert_eq!(events.len(), 3);
        for event in &events {
            assert_eq!(event.key(), Key::Char('a'));
            assert_eq!(event.kind(), KeyEventKind::Press);
        }
    }

    #[test]
    fn repeat_count_is_capped() {
        // A hostile repeat count is bounded to REPEAT_CAP events.
        let events = decode(b"\x1b[65;30;97;1;0;1000000_");
        assert_eq!(events.len(), REPEAT_CAP as usize);
    }

    #[test]
    fn zero_repeat_count_still_emits_one_event() {
        let events = decode(b"\x1b[65;30;97;1;0;0_");
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn surrogate_pair_reassembles_across_two_events() {
        // U+1F600 arrives as high (0xD83D=55357) then low (0xDE00=56832); the pair is one char.
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert!(out.is_empty(), "the high surrogate is held, not emitted");
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;56832;1_"), &mut out));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key(), Key::Char('\u{1f600}'));
        assert_eq!(out[0].text().map(TextPayload::as_str), Some("\u{1f600}"));
        assert_eq!(out[0].kind(), KeyEventKind::Press);
    }

    #[test]
    fn low_surrogate_uses_the_second_events_modifiers_and_kind() {
        // The completed character takes the low-surrogate event's Kd/Cs: here a release with SHIFT.
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;56832;0;16_"), &mut out));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind(), KeyEventKind::Release);
        assert_eq!(out[0].modifiers(), Modifiers::SHIFT);
        assert_eq!(out[0].text(), None, "a release carries no text");
    }

    #[test]
    fn orphaned_high_surrogate_is_flushed_by_a_non_low_event() {
        // A high surrogate followed by a plain 'a' flushes U+FFFD, then decodes 'a'.
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert!(decoder.decode_key(&csi(b"\x1b[65;30;97;1_"), &mut out));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].key(), Key::Char(char::REPLACEMENT_CHARACTER));
        assert_eq!(out[1].key(), Key::Char('a'));
    }

    #[test]
    fn a_second_high_surrogate_flushes_the_first() {
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert_eq!(out.len(), 1, "the first high half is flushed as U+FFFD");
        assert_eq!(out[0].key(), Key::Char(char::REPLACEMENT_CHARACTER));
    }

    #[test]
    fn orphaned_low_surrogate_becomes_replacement() {
        let event = decode_one(b"\x1b[0;0;56832;1_");
        assert_eq!(event.key(), Key::Char(char::REPLACEMENT_CHARACTER));
    }

    #[test]
    fn flush_emits_a_dangling_high_surrogate() {
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(decoder.decode_key(&csi(b"\x1b[0;0;55357;1_"), &mut out));
        assert!(out.is_empty());
        decoder.flush(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key(), Key::Char(char::REPLACEMENT_CHARACTER));
        // Flushing again is a no-op; the pending half is gone.
        out.clear();
        decoder.flush(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn num_lock_and_caps_lock_map_but_scroll_lock_does_not() {
        // NUMLOCK (0x20) and CAPSLOCK (0x80) have frozen homes; SCROLLLOCK (0x40) does not.
        assert_eq!(
            decode_one(b"\x1b[65;30;97;1;32_").modifiers(),
            Modifiers::NUM_LOCK
        );
        assert_eq!(
            decode_one(b"\x1b[65;30;97;1;128_").modifiers(),
            Modifiers::CAPS_LOCK,
        );
        assert_eq!(
            decode_one(b"\x1b[65;30;97;1;64_").modifiers(),
            Modifiers::empty(),
        );
    }

    #[test]
    fn degenerate_and_all_empty_events_are_consumed_without_a_key() {
        // `CSI _` and `CSI ;;;;;_` are legal degenerate events: all fields default, no key.
        assert!(decode(b"\x1b[_").is_empty());
        assert!(decode(b"\x1b[;;;;;_").is_empty());
    }

    #[test]
    fn non_underscore_final_is_declined() {
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(!decoder.decode_key(&csi(b"\x1b[65;30;97u"), &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn colon_subparameter_is_declined_for_passthrough() {
        // A `:` sub-parameter is not win32-input grammar; declining lets it pass through as syntax.
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(!decoder.decode_key(&csi(b"\x1b[65:30;97;1_"), &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn private_marker_is_declined() {
        // `CSI ? 9001 h` is the enable sequence, not a key event; a `_`-final with a private marker
        // is likewise not this protocol.
        let mut decoder = Win32Decoder::default();
        let mut out = Vec::new();
        assert!(!decoder.decode_key(&csi(b"\x1b[?9001_"), &mut out));
        assert!(out.is_empty());
    }
}
