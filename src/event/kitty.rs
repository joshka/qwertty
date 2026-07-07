//! Kitty keyboard protocol (`CSI u`) semantic decode.
//!
//! This module turns a [`ControlSequence`] from the syntax layer into a typed [`KeyEvent`], plus
//! the flags [`report`](decode_flags_report) the correlator consumes. It implements the full kitty
//! `CSI u` grammar the text-granularity spike settled (design 02, OQ-6):
//!
//! ```text
//! CSI unicode-key-code:shifted-key:base-layout-key ; modifiers:event-type ; text-codepoints u
//! ```
//!
//! Only `unicode-key-code` is mandatory. The three fields are `;`-separated; the key and modifier
//! fields carry `:`-separated subfields (alternate keys, event type). The text field is a
//! `:`-separated run of Unicode code points that becomes a multi-codepoint-capable [`TextPayload`]
//! (the OQ-6 payoff: decomposed accents, jamo runs, and ZWJ clusters are one event).
//!
//! The legacy modified-key CSI forms (`CSI 1 ; mods A` for a modified arrow, `CSI n ; mods ~` for
//! the editing/function keys, `CSI 1 ; mods H/F` for Home/End) also decode here so a modifier on an
//! arrow or Home/End is reported, not dropped.
//!
//! # Modifier encoding
//!
//! The kitty modifier field is **1 + the modifier bitset** ("value-1 encoding"): a bare press with
//! no modifiers is `1`, Shift is `2` (`1 + 1`), Ctrl is `5` (`1 + 4`), and so on. Decoding
//! subtracts one before reading the [`Modifiers`] bits; a field of `0` or a missing field is no
//! modifiers. The Caps Lock (`0b0100_0000`) and Num Lock (`0b1000_0000`) bits ride in the same
//! field and decode to [`Modifiers::CAPS_LOCK`] and [`Modifiers::NUM_LOCK`].

use crate::event::key::{Key, KeyEvent, KeyEventKind, Modifiers, TextPayload};
use crate::syntax::{ControlParams, ControlSequence, ParamSeparator};

/// Decodes a kitty `CSI … u` key sequence or a legacy modified-key CSI into a [`KeyEvent`].
///
/// Returns `None` when the sequence is not a kitty key event this layer recognizes, so the caller
/// passes it through as lossless syntax (design 02 forward-compatibility). A `CSI ? flags u` flags
/// report is **not** a key event and returns `None` here; use [`decode_flags_report`] for it.
pub(crate) fn decode_key(csi: &ControlSequence) -> Option<KeyEvent> {
    let params = csi.params();
    // Private markers (`?`, `>`, `<`, `=`) never introduce a key event: `CSI ? flags u` is a flags
    // report, and the push/pop/query control forms are host-to-terminal, never replies.
    if !params.private_markers().is_empty() || !params.intermediates().is_empty() {
        return None;
    }

    match params.final_byte() {
        b'u' => decode_csi_u(params),
        // Legacy CSI functional-key forms. These decode only to carry modifiers; the unmodified
        // arrow forms are still handled by the arrow-key path in the parent module for parity.
        b'A' | b'B' | b'C' | b'D' | b'H' | b'F' | b'P' | b'Q' | b'S' | b'~' => {
            decode_legacy_functional(params)
        }
        _ => None,
    }
}

/// Decodes the flags value from a `CSI ? flags u` keyboard-flags report, or `None`.
///
/// This is the terminal's reply to the `CSI ? u` query: the currently active progressive
/// enhancement flags as a decimal bitset. It is not a key event. The correlator's keyboard-flags
/// expectation matches this shape and takes the decoded value as the *granted* set.
pub(crate) fn decode_flags_report(csi: &ControlSequence) -> Option<u8> {
    let params = csi.params();
    if params.final_byte() != b'u'
        || params.private_markers() != b"?"
        || !params.intermediates().is_empty()
    {
        return None;
    }

    // A bare `CSI ? u` (no parameter) reports no flags active; otherwise the single parameter is
    // the flag bitset. A terminal never sends more than one parameter here.
    match params.param_bytes() {
        b"" => Some(0),
        bytes => parse_decimal_u8(bytes),
    }
}

/// Decodes a `CSI … u` key sequence from its already-parsed params.
fn decode_csi_u(params: &ControlParams) -> Option<KeyEvent> {
    let mut groups = param_groups(params);

    // Field 1 (mandatory): unicode-key-code : shifted-key : base-layout-key. The unicode-key-code
    // itself must be present; the alternates may be empty subfields (kept as `None`).
    let key_group = groups.next()?;
    let unicode_key = char::from_u32((*key_group.first()?)?)?;
    let shifted_key = key_group.get(1).copied().flatten().and_then(char::from_u32);
    let base_layout_key = key_group.get(2).copied().flatten().and_then(char::from_u32);

    // Field 2 (optional): modifiers : event-type.
    let modifier_group = groups.next().unwrap_or_default();
    let modifiers = decode_modifiers(modifier_group.first().copied().flatten());
    let kind = decode_event_type(modifier_group.get(1).copied().flatten());

    // Field 3 (optional): text-as-code-points, colon-separated.
    let text_group = groups.next().unwrap_or_default();
    let text = decode_text(&text_group);

    let mut event = KeyEvent::new(functional_key(unicode_key))
        .with_modifiers(modifiers)
        .with_kind(kind);
    if let Some(shifted) = shifted_key {
        event = event.with_shifted_key(shifted);
    }
    if let Some(base) = base_layout_key {
        event = event.with_base_layout_key(base);
    }
    if let Some(text) = text {
        event = event.with_text_payload(text);
    }
    Some(event)
}

/// Decodes a legacy modified-key CSI form (`CSI 1 ; mods A`, `CSI n ; mods ~`, `CSI 1 ; mods H`).
///
/// The letter-final forms (`A`-`D`, `H`, `F`, `P`, `Q`, `S`) are a modified key **only** when their
/// first parameter is `1` (the CSI-1 modified-key convention): a different numeric first parameter
/// means the sequence is an ECMA-48 cursor-movement or scroll control (e.g. `CSI 2 F` is Cursor
/// Previous Line, `CSI 3 A` is Cursor Up 3), which is not a key and must pass through as syntax.
/// The `~` form instead selects the key by its first-parameter number. In both, the second
/// `;`-separated group carries the value-1-encoded modifier and event type.
fn decode_legacy_functional(params: &ControlParams) -> Option<KeyEvent> {
    let mut groups = param_groups(params);
    let first = groups.next().unwrap_or_default();
    // The first parameter, defaulting to `1` when the field is empty (CSI default).
    let selector = first.first().copied().flatten().unwrap_or(1);

    let modifier_group = groups.next().unwrap_or_default();
    let has_modifier_group = !modifier_group.is_empty();
    let modifiers = decode_modifiers(modifier_group.first().copied().flatten());
    let kind = decode_event_type(modifier_group.get(1).copied().flatten());

    let final_byte = params.final_byte();

    if final_byte == b'~' {
        let key = legacy_tilde_key(selector)?;
        return Some(KeyEvent::new(key).with_modifiers(modifiers).with_kind(kind));
    }

    // The letter-final modified-key forms require the CSI-1 selector; anything else is a cursor
    // control, not a key.
    if selector != 1 {
        return None;
    }

    // A bare arrow with no modifier group is the parity arrow path's job (handled by the parent
    // module); declining it here keeps one sequence from producing two events. A modified arrow, or
    // a Home/End/F form, decodes here.
    if matches!(final_byte, b'A' | b'B' | b'C' | b'D') && !has_modifier_group {
        return None;
    }

    let key = match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        // Home/End have both a `CSI 1 ; mods H/F` form and, via SS3-in-CSI, `CSI H/F`.
        b'H' => Key::Home,
        b'F' => Key::End,
        // Legacy F1-F4 (`SS3 P/Q/S` and the CSI variants some terminals send).
        b'P' => Key::Function(1),
        b'Q' => Key::Function(2),
        b'S' => Key::Function(4),
        _ => return None,
    };

    Some(KeyEvent::new(key).with_modifiers(modifiers).with_kind(kind))
}

/// Maps a legacy `CSI n ~` editing/function number to its [`Key`].
fn legacy_tilde_key(number: u32) -> Option<Key> {
    Some(match number {
        2 => Key::Insert,
        3 => Key::Delete,
        5 => Key::PageUp,
        6 => Key::PageDown,
        7 => Key::Home,
        8 => Key::End,
        // The `CSI n ~` function-key block: 11-15 -> F1-F5, 17-21 -> F6-F10, 23-24 -> F11-F12.
        11 => Key::Function(1),
        12 => Key::Function(2),
        13 => Key::Function(3),
        14 => Key::Function(4),
        15 => Key::Function(5),
        17 => Key::Function(6),
        18 => Key::Function(7),
        19 => Key::Function(8),
        20 => Key::Function(9),
        21 => Key::Function(10),
        23 => Key::Function(11),
        24 => Key::Function(12),
        _ => return None,
    })
}

/// Maps a kitty `CSI u` unicode-key-code to a named functional [`Key`], or [`Key::Char`].
///
/// The kitty protocol reuses the C0 code points 9/13/27/127 for Tab/Enter/Escape/Backspace and a
/// Unicode Private Use Area block (`57344..`) for the functional keys. Any code point without a
/// named mapping is a character key.
fn functional_key(code: char) -> Key {
    match u32::from(code) {
        9 => Key::Tab,
        13 => Key::Enter,
        27 => Key::Escape,
        127 => Key::Backspace,
        57348 => Key::Insert,
        57349 => Key::Delete,
        57350 => Key::Left,
        57351 => Key::Right,
        57352 => Key::Up,
        57353 => Key::Down,
        57354 => Key::PageUp,
        57355 => Key::PageDown,
        57356 => Key::Home,
        57357 => Key::End,
        // The functional-key PUA block places F1 at 57364 (kitty's `KP_...`/`F...` table). F1-F35
        // are contiguous from 57364.
        code @ 57364..=57398 => {
            // The range width is 35, so the F-number is 1..=35, always a valid u8.
            let number = u8::try_from(code - 57364 + 1).unwrap_or(u8::MAX);
            Key::Function(number)
        }
        _ => Key::Char(code),
    }
}

/// Decodes the value-1-encoded kitty modifier field into [`Modifiers`].
///
/// The field is `1 + bitset`; `None`, `Some(0)`, or `Some(1)` all mean no modifiers. Only the low
/// eight bits are meaningful (Shift, Alt, Ctrl, Super, Hyper, Meta, Caps Lock, Num Lock, in kitty
/// bit order — which [`Modifiers`] already mirrors).
fn decode_modifiers(field: Option<u32>) -> Modifiers {
    let raw = field.unwrap_or(0);
    // A field of 0 (some terminals send it) and the value-1 base both mean "no modifiers".
    let bits = raw.saturating_sub(1);
    // Only the low eight bits are defined modifiers; a wider field keeps its low byte.
    let byte = u8::try_from(bits & 0xff).unwrap_or(0);
    Modifiers::from_kitty_bits(byte)
}

/// Decodes the kitty event-type subfield: 1 (or absent) press, 2 repeat, 3 release.
fn decode_event_type(field: Option<u32>) -> KeyEventKind {
    match field {
        Some(2) => KeyEventKind::Repeat,
        Some(3) => KeyEventKind::Release,
        _ => KeyEventKind::Press,
    }
}

/// Decodes the colon-separated text-as-code-points field into a [`TextPayload`], or `None`.
///
/// Each subfield is one Unicode code point; the payload is their concatenation, so a single
/// codepoint, a decomposed accent, a jamo run, and a ZWJ cluster each become one payload. An empty
/// field (a release event carries none) yields `None`. Control code points are dropped per the spec
/// ("the associated text must not contain control codes"); if that leaves nothing, the result is
/// `None`.
fn decode_text(codepoints: &[Option<u32>]) -> Option<TextPayload> {
    if codepoints.is_empty() {
        return None;
    }
    let mut text = String::new();
    for &code in codepoints.iter().flatten() {
        if let Some(character) = char::from_u32(code)
            && !is_control_codepoint(code)
        {
            text.push(character);
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(TextPayload::from_text(&text))
    }
}

/// Returns whether a code point is a control code the spec forbids in associated text.
///
/// The spec: "control codes are code points below U+0020 and codepoints in the C0 and C1 blocks"
/// (C0 is `..0x20`, C1 is `0x80..=0x9f`, and DEL `0x7f` is a C0-adjacent control).
fn is_control_codepoint(code: u32) -> bool {
    code < 0x20 || code == 0x7f || (0x80..=0x9f).contains(&code)
}

/// Splits the parsed params into `;`-separated groups, each a vector of the group's `:`-separated
/// subfield values, preserving an **empty** subfield as `None` (e.g. the empty shifted-key in
/// `CSI 229::97 …`).
///
/// The syntax layer already parsed the params into [`Param`] values that carry their preceding
/// separator, so a new group starts at each [`ParamSeparator::Semicolon`] (and at the first
/// param), while a [`ParamSeparator::Colon`] extends the current group. A `Param` whose value is
/// `None` is an empty field, kept as `None` so an empty subfield never decodes to a spurious NUL
/// alternate key.
///
/// [`Param`]: crate::syntax::Param
fn param_groups(params: &ControlParams) -> impl Iterator<Item = Vec<Option<u32>>> {
    let mut groups: Vec<Vec<Option<u32>>> = Vec::new();
    for param in params.params() {
        let value = param.value();
        match param.separator() {
            // The first param, or one after a `;`, begins a new group.
            None | Some(ParamSeparator::Semicolon) => groups.push(vec![value]),
            // A `:` extends the current group; if somehow first, start one.
            Some(ParamSeparator::Colon) => match groups.last_mut() {
                Some(group) => group.push(value),
                None => groups.push(vec![value]),
            },
        }
    }
    groups.into_iter()
}

/// Parses a run of ASCII decimal digits into a `u8`, or `None` on overflow or a non-digit.
fn parse_decimal_u8(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &byte in bytes {
        let digit = (byte as char).to_digit(10)?;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    u8::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// Decodes `bytes` as a kitty key event, panicking if it is not recognized as one.
    fn key(bytes: &[u8]) -> KeyEvent {
        decode_key(&csi(bytes)).unwrap_or_else(|| panic!("{bytes:?} did not decode to a key event"))
    }

    #[test]
    fn ascii_press_carries_char_and_no_modifiers() {
        // `ESC[97;1u` -> 'a' press, value-1 modifier field means no modifiers.
        let event = key(b"\x1b[97;1u");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.modifiers(), Modifiers::empty());
        assert_eq!(event.kind(), KeyEventKind::Press);
        // No text field present.
        assert_eq!(event.text(), None);
        assert_eq!(event.shifted_key(), None);
        assert_eq!(event.base_layout_key(), None);
    }

    #[test]
    fn press_with_omitted_modifier_field_defaults_to_press_no_modifiers() {
        // Only the mandatory unicode-key-code, no `;` group at all.
        let event = key(b"\x1b[97u");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.modifiers(), Modifiers::empty());
        assert_eq!(event.kind(), KeyEventKind::Press);
    }

    #[test]
    fn release_event_type_decodes() {
        // `ESC[97;1:3u` -> 'a' release, no text.
        let event = key(b"\x1b[97;1:3u");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.kind(), KeyEventKind::Release);
        assert_eq!(event.text(), None);
    }

    #[test]
    fn repeat_event_type_decodes() {
        let event = key(b"\x1b[97;1:2u");
        assert_eq!(event.kind(), KeyEventKind::Repeat);
    }

    #[test]
    fn shifted_alternate_key_decodes() {
        // Spec's own example: `ESC[97:65;2u` -> shift+a, shifted-key alternate 'A'.
        let event = key(b"\x1b[97:65;2u");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.modifiers(), Modifiers::SHIFT);
        assert_eq!(event.shifted_key(), Some('A'));
        assert_eq!(event.base_layout_key(), None);
    }

    #[test]
    fn full_alternates_and_text_all_present() {
        // `ESC[97:65:97;2;65u` -> shifted-key 'A', base-layout-key 'a', text "A".
        let event = key(b"\x1b[97:65:97;2;65u");
        assert_eq!(event.key(), Key::Char('a'));
        assert_eq!(event.shifted_key(), Some('A'));
        assert_eq!(event.base_layout_key(), Some('a'));
        assert_eq!(event.modifiers(), Modifiers::SHIFT);
        assert_eq!(event.text().map(TextPayload::as_str), Some("A"));
    }

    #[test]
    fn empty_shifted_subfield_is_not_a_nul_alternate() {
        // `ESC[229::97;9u` -> empty shifted-key, base-layout-key 'a'. Field 9 = 1 + 8, and kitty
        // bit 8 is Super (the spike fixture's note mislabels it "alt"; the wire byte decodes to the
        // Super bit per the kitty modifier table, which `Modifiers` mirrors).
        let event = key(b"\x1b[229::97;9u");
        assert_eq!(event.key(), Key::Char('\u{00e5}'));
        assert_eq!(event.shifted_key(), None, "empty subfield must not be NUL");
        assert_eq!(event.base_layout_key(), Some('a'));
        assert_eq!(event.modifiers(), Modifiers::SUPER);
    }

    #[test]
    fn precomposed_single_codepoint_text() {
        // `ESC[233;1;233u` -> 'é' key with text "é" (U+00E9).
        let event = key(b"\x1b[233;1;233u");
        assert_eq!(event.key(), Key::Char('\u{00e9}'));
        assert_eq!(event.text().map(TextPayload::as_str), Some("\u{00e9}"));
    }

    #[test]
    fn decomposed_two_codepoint_text_is_one_event() {
        // `ESC[101;1;101:769u` -> 'e' + combining acute, two codepoints in one event.
        let event = key(b"\x1b[101;1;101:769u");
        assert_eq!(event.key(), Key::Char('e'));
        assert_eq!(event.text().map(TextPayload::as_str), Some("e\u{0301}"));
    }

    #[test]
    fn zwj_family_multi_codepoint_text_is_one_event() {
        // `ESC[128104;1;128104:8205:128105:8205:128103u` -> 5-codepoint ZWJ cluster, one event.
        let event = key(b"\x1b[128104;1;128104:8205:128105:8205:128103u");
        assert_eq!(event.key(), Key::Char('\u{1f468}'));
        assert_eq!(
            event.text().map(TextPayload::as_str),
            Some("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"),
        );
    }

    #[test]
    fn emoji_uses_codepoint_as_key() {
        let event = key(b"\x1b[128512;1;128512u");
        assert_eq!(event.key(), Key::Char('\u{1f600}'));
        assert_eq!(event.text().map(TextPayload::as_str), Some("\u{1f600}"));
    }

    #[test]
    fn modifier_with_explicit_text_keeps_association() {
        // `ESC[97::97;9;229u` -> modifier field 9 (Super per kitty bits), base-layout 'a', text
        // "å". The point is association: modifiers stay on the same event as the text (design 02's
        // OQ-6 payoff over candidate A/B, which drop modifiers the moment text appears).
        let event = key(b"\x1b[97::97;9;229u");
        assert_eq!(event.modifiers(), Modifiers::SUPER);
        assert_eq!(event.base_layout_key(), Some('a'));
        assert_eq!(event.text().map(TextPayload::as_str), Some("\u{00e5}"));
    }

    #[test]
    fn value_one_modifier_arithmetic() {
        // Ctrl is bit 0b100 -> field 5 (1 + 4). Shift+Ctrl -> field 6 (1 + 1 + 4).
        assert_eq!(key(b"\x1b[97;5u").modifiers(), Modifiers::CTRL);
        assert_eq!(
            key(b"\x1b[97;6u").modifiers(),
            Modifiers::SHIFT.union(Modifiers::CTRL),
        );
    }

    #[test]
    fn caps_and_num_lock_bits_decode() {
        // Caps Lock is bit 0b0100_0000 -> field 65 (1 + 64); Num Lock -> field 129 (1 + 128).
        assert_eq!(key(b"\x1b[97;65u").modifiers(), Modifiers::CAPS_LOCK);
        assert_eq!(key(b"\x1b[97;129u").modifiers(), Modifiers::NUM_LOCK);
    }

    #[test]
    fn zero_modifier_field_means_no_modifiers() {
        // Some terminals send a literal 0 rather than the value-1 base.
        assert_eq!(key(b"\x1b[97;0u").modifiers(), Modifiers::empty());
    }

    #[test]
    fn named_functional_codepoints_decode() {
        assert_eq!(key(b"\x1b[9u").key(), Key::Tab);
        assert_eq!(key(b"\x1b[13u").key(), Key::Enter);
        assert_eq!(key(b"\x1b[27u").key(), Key::Escape);
        assert_eq!(key(b"\x1b[127u").key(), Key::Backspace);
        assert_eq!(key(b"\x1b[57350u").key(), Key::Left);
        assert_eq!(key(b"\x1b[57357u").key(), Key::End);
        assert_eq!(key(b"\x1b[57364u").key(), Key::Function(1));
        assert_eq!(key(b"\x1b[57398u").key(), Key::Function(35));
    }

    #[test]
    fn legacy_modified_arrow_carries_modifiers() {
        // `ESC[1;5A` -> Ctrl+Up.
        let event = key(b"\x1b[1;5A");
        assert_eq!(event.key(), Key::Up);
        assert_eq!(event.modifiers(), Modifiers::CTRL);
    }

    #[test]
    fn legacy_tilde_forms_decode() {
        assert_eq!(key(b"\x1b[3~").key(), Key::Delete);
        assert_eq!(key(b"\x1b[5~").key(), Key::PageUp);
        assert_eq!(key(b"\x1b[1;2H").key(), Key::Home);
        assert_eq!(key(b"\x1b[15~").key(), Key::Function(5));
        assert_eq!(key(b"\x1b[24;5~").modifiers(), Modifiers::CTRL);
        assert_eq!(key(b"\x1b[24;5~").key(), Key::Function(12));
    }

    #[test]
    fn bare_arrow_is_not_claimed_by_the_legacy_path() {
        // The parity arrow path owns `ESC[A` and `ESC[1A`; the legacy decode declines them so one
        // sequence never produces two events.
        assert!(decode_key(&csi(b"\x1b[A")).is_none());
        assert!(decode_key(&csi(b"\x1b[1A")).is_none());
    }

    #[test]
    fn flags_report_and_query_shapes() {
        // A flags report is not a key event.
        assert!(decode_key(&csi(b"\x1b[?1u")).is_none());
        // The report decodes its flag bitset; a bare `?u` is zero flags.
        assert_eq!(decode_flags_report(&csi(b"\x1b[?1u")), Some(1));
        assert_eq!(decode_flags_report(&csi(b"\x1b[?31u")), Some(31));
        assert_eq!(decode_flags_report(&csi(b"\x1b[?u")), Some(0));
        // A plain key `CSI u` (no `?`) is not a flags report.
        assert_eq!(decode_flags_report(&csi(b"\x1b[97u")), None);
    }

    #[test]
    fn push_pop_query_control_forms_are_not_key_events() {
        // `CSI > flags u`, `CSI < u`, `CSI < 1 u` are host-to-terminal control forms, never keys.
        assert!(decode_key(&csi(b"\x1b[>1u")).is_none());
        assert!(decode_key(&csi(b"\x1b[<u")).is_none());
        assert!(decode_key(&csi(b"\x1b[<1u")).is_none());
    }

    #[test]
    fn non_key_final_byte_is_declined() {
        assert!(decode_key(&csi(b"\x1b[12;34R")).is_none());
        assert!(decode_key(&csi(b"\x1b[0n")).is_none());
    }
}
