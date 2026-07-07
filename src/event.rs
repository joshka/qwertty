//! Semantic input events: the typed vocabulary above the syntax layer.
//!
//! This is the second of the two decoder layers described in design 02. Where [`SyntaxParser`]
//! classifies every input byte into a lossless [`SyntaxToken`] by ECMA-48 family,
//! [`SemanticDecoder`] turns those tokens into the typed [`Event`] vocabulary applications consume:
//! [`KeyEvent`] values for keys, and lossless [`Event::Syntax`] passthrough for
//! complete-but-unmapped tokens.
//!
//! ```text
//! bytes -> SyntaxParser -> SyntaxToken -> SemanticDecoder -> Event
//! ```
//!
//! # Scope
//!
//! The vocabulary is **pre-freeze until milestone M4 exit** (design 08: `event::` types change
//! freely before publish and calcify at 0.1). This slice reaches parity with the retired basic
//! input-decoder path only, mapping:
//!
//! - printable UTF-8 text to one [`KeyEvent`] per character, with the decoded character carried in
//!   the event's [`TextPayload`];
//! - the C0 controls the old decoder named to [`KeyEvent`] values with the mapped [`Key`]
//!   ([`Key::Enter`], [`Key::Tab`], [`Key::Backspace`], and [`Key::Control`] for the rest);
//! - the four arrow-key CSI sequences to [`Key::Up`], [`Key::Down`], [`Key::Left`], and
//!   [`Key::Right`];
//! - a standalone Escape (flushed by the layer above and seen here as a bare [`SyntaxToken::Esc`])
//!   to [`Key::Escape`];
//! - every other complete token — CSI qwertty does not decode yet, OSC/DCS/APC/PM/SOS, other escape
//!   sequences, and [`SyntaxToken::Malformed`] — losslessly to [`Event::Syntax`], never a fake
//!   keypress (design 02's forward-compatibility contract).
//!
//! Kitty `CSI u` key decoding (functional keys, modifiers, event kinds, associated text), mouse,
//! focus, paste, and resize events arrive in milestone M4. The [`Event`] enum is
//! `#[non_exhaustive]` so those variants add without churning existing code.
//!
//! # Text asymmetry
//!
//! Legacy UTF-8 input decodes to **one key event per character** (design 02): a text run of `n`
//! characters becomes `n` [`KeyEvent`] values, each carrying a single-character [`TextPayload`].
//! Multi-codepoint text (decomposed accents, jamo runs, ZWJ clusters as one event) arrives only
//! through the kitty `CSI u` associated-text field in milestone M4; the [`TextPayload`] type is
//! multi-codepoint-capable so that path needs no vocabulary change, but this slice never emits more
//! than one character per key.
//!
//! # ESC timing
//!
//! [`SemanticDecoder`] never applies Escape-versus-sequence timing policy — that stays in the layer
//! above (design 02). The decoder maps a bare [`SyntaxToken::Esc`] to [`Key::Escape`] only because
//! seeing one means the layer above already decided the Escape stood alone (it flushed the parser).
//! An `ESC`-prefixed sequence never reaches the decoder as a bare Escape.
//!
//! # Example
//!
//! ```
//! use qwertty::{Event, Key, SemanticDecoder};
//!
//! let mut decoder = SemanticDecoder::new();
//! let events = decoder.feed(b"hi\x1b[A");
//!
//! assert_eq!(events.len(), 3);
//! assert_eq!(events[0].key_event().map(|k| k.key()), Some(Key::Char('h')));
//! assert_eq!(events[1].key_event().map(|k| k.key()), Some(Key::Char('i')));
//! assert_eq!(events[2].key_event().map(|k| k.key()), Some(Key::Up));
//! assert!(decoder.finish().is_empty());
//! ```

mod key;

pub use key::{Key, KeyEvent, KeyEventKind, Modifiers, TextPayload};

use crate::syntax::{EscapeSequence, SyntaxParser, SyntaxToken};

const CR: u8 = 0x0d;
const HT: u8 = 0x09;
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;

/// A decoded semantic input event.
///
/// `Event` is the typed vocabulary above the lossless syntax layer. This slice produces
/// [`Event::Key`] for the keys the parity scope covers and [`Event::Syntax`] for every other
/// complete token, preserving its bytes for a later layer or the application.
///
/// The enum is `#[non_exhaustive]`. Paste, mouse, focus, and resize variants arrive in milestone
/// M4; the vocabulary is pre-freeze until then (design 08).
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// A decoded key event.
    Key(KeyEvent),
    /// A complete syntax token qwertty does not map to a typed event yet.
    ///
    /// This is the forward-compatibility passthrough: CSI sequences beyond the arrow keys,
    /// OSC/DCS/APC/PM/SOS control strings, non-arrow escape sequences, and
    /// [`SyntaxToken::Malformed`] runs all arrive here with their bytes intact, so new protocols
    /// degrade to visible, lossless syntax rather than fake keypresses (design 02).
    Syntax(SyntaxToken),
}

impl Event {
    /// Returns the [`KeyEvent`] when this is an [`Event::Key`], or `None` otherwise.
    #[must_use]
    pub fn key_event(&self) -> Option<&KeyEvent> {
        match self {
            Self::Key(key) => Some(key),
            Self::Syntax(_) => None,
        }
    }

    /// Returns the [`SyntaxToken`] when this is an [`Event::Syntax`], or `None` otherwise.
    #[must_use]
    pub fn syntax_token(&self) -> Option<&SyntaxToken> {
        match self {
            Self::Syntax(token) => Some(token),
            Self::Key(_) => None,
        }
    }
}

/// Streaming decoder from terminal input bytes to semantic [`Event`] values.
///
/// `SemanticDecoder` owns a [`SyntaxParser`] and maps its tokens to the [`Event`] vocabulary. Feed
/// input chunks with [`SemanticDecoder::feed`]; the owned parser retains partial sequences across
/// calls, so a sequence split across reads decodes identically to feeding it whole. Call
/// [`SemanticDecoder::finish`] at end of input to flush pending parser state.
///
/// # Example
///
/// A CSI arrow key split across chunks decodes to one key event:
///
/// ```
/// use qwertty::{Key, SemanticDecoder};
///
/// let mut decoder = SemanticDecoder::new();
/// assert!(decoder.feed(b"\x1b[").is_empty());
///
/// let events = decoder.feed(b"A");
/// assert_eq!(events.len(), 1);
/// assert_eq!(events[0].key_event().map(|k| k.key()), Some(Key::Up));
/// assert!(decoder.finish().is_empty());
/// ```
#[derive(Clone, Debug)]
pub struct SemanticDecoder {
    parser: SyntaxParser,
}

impl Default for SemanticDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticDecoder {
    /// Creates a decoder over a [`SyntaxParser`] with the default payload bound.
    #[must_use]
    pub fn new() -> Self {
        Self {
            parser: SyntaxParser::new(),
        }
    }

    /// Creates a decoder over a [`SyntaxParser`] with a custom string-payload byte bound.
    ///
    /// The bound is passed straight through to [`SyntaxParser::with_payload_limit`]; it caps how
    /// many bytes an over-long control-string payload buffers before the token is truncated. It
    /// does not affect key decoding.
    #[must_use]
    pub fn with_payload_limit(payload_limit: usize) -> Self {
        Self {
            parser: SyntaxParser::with_payload_limit(payload_limit),
        }
    }

    /// Feeds one input chunk and returns the semantic events that became complete.
    ///
    /// Bytes that begin but do not complete a sequence are buffered by the owned parser for the
    /// next call, so the returned vector is empty when the whole chunk only extends a pending
    /// sequence.
    #[must_use]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let tokens = self.parser.feed(bytes);
        map_tokens(tokens)
    }

    /// Flushes any pending partial sequence and returns its semantic events.
    ///
    /// A bare trailing `ESC` flushed by the parser becomes a [`Key::Escape`] key event. Every other
    /// flushed token maps the same way it would mid-stream. This never applies Escape timing
    /// policy; seeing a bare Escape here means the layer above already flushed the parser
    /// (design 02).
    #[must_use]
    pub fn finish(&mut self) -> Vec<Event> {
        let tokens = self.parser.finish();
        map_tokens(tokens)
    }

    /// Returns whether the decoder is holding a **settled** trailing text run.
    ///
    /// The syntax layer buffers a trailing text run at a read boundary because the next read might
    /// continue it (keeping split-equivalence). When a reader has drained the operating-system
    /// input buffer, that pending run is instead *settled* input the caller should receive now
    /// — but only when it is complete: a run parked mid-character (an incomplete UTF-8 lead
    /// byte) or a partial escape/control sequence must keep waiting for the bytes that finish
    /// it.
    ///
    /// This returns `true` exactly when [`pending`](SemanticDecoder::finish) holds a run that
    /// begins with a text byte and is complete valid UTF-8, so a driver can
    /// [`finish`](Self::finish) it at a drained-buffer boundary without prematurely flushing a
    /// genuinely partial sequence. See the Tokio session's read loop for the drain-boundary
    /// flush this enables.
    #[must_use]
    pub fn has_settled_text(&self) -> bool {
        let pending = self.parser.pending_bytes();
        !pending.is_empty()
            && pending[0] >= 0x20
            && pending[0] != DEL
            && std::str::from_utf8(pending).is_ok()
    }
}

/// Maps a batch of syntax tokens to semantic events.
fn map_tokens(tokens: Vec<SyntaxToken>) -> Vec<Event> {
    let mut events = Vec::with_capacity(tokens.len());
    for token in tokens {
        map_token(token, &mut events);
    }
    events
}

/// Maps one syntax token to zero or more semantic events, appending them to `events`.
fn map_token(token: SyntaxToken, events: &mut Vec<Event>) {
    match token {
        // A text run decodes to one key event per character, each carrying that character as text.
        SyntaxToken::Text(bytes) => push_text_events(&bytes, events),
        // A C0 control maps to its named key, or a lossless catch-all key.
        SyntaxToken::Control(byte) => events.push(Event::Key(control_key_event(byte))),
        // Arrow-key CSI decodes to a key event; every other CSI passes through as syntax.
        SyntaxToken::Csi(csi) => match arrow_key(&csi) {
            Some(key) => events.push(Event::Key(KeyEvent::new(key))),
            None => events.push(Event::Syntax(SyntaxToken::Csi(csi))),
        },
        // A bare trailing Escape (no final byte) is the standalone Escape key; a complete escape
        // sequence passes through as syntax.
        SyntaxToken::Esc(escape) => events.push(escape_event(escape)),
        // Every remaining complete token is lossless syntax passthrough.
        other => events.push(Event::Syntax(other)),
    }
}

/// Pushes one [`Key::Char`] key event per character in a valid-UTF-8 text run.
fn push_text_events(bytes: &[u8], events: &mut Vec<Event>) {
    // The syntax layer guarantees `SyntaxToken::Text` is valid UTF-8.
    let text = std::str::from_utf8(bytes).expect("SyntaxToken::Text is valid UTF-8");
    for character in text.chars() {
        events.push(Event::Key(
            KeyEvent::new(Key::Char(character)).with_text(character),
        ));
    }
}

/// Builds the key event for a single C0 control byte.
///
/// `CR` (`0x0d`) is Enter, `HT` (`0x09`) is Tab, and `DEL` (`0x7f`) and `BS` (`0x08`) are both
/// Backspace (see [`Key::Backspace`]). Every other control byte is preserved as [`Key::Control`].
/// `ESC` (`0x1b`) never reaches here as a control byte: the syntax layer only emits a lone Escape
/// as [`SyntaxToken::Esc`].
fn control_key_event(byte: u8) -> KeyEvent {
    let key = match byte {
        CR => Key::Enter,
        HT => Key::Tab,
        DEL | BS => Key::Backspace,
        other => Key::Control(other),
    };
    KeyEvent::new(key)
}

/// Recognizes the four arrow-key CSI sequences, returning their [`Key`], or `None`.
///
/// The recognized shapes are `ESC [ A/B/C/D` with no private markers, no intermediate bytes, and
/// either no parameters or the single default parameter `1` (as terminals send `ESC [ 1 A` when a
/// modifier field is present but empty). This matches the old decoder's arrow-key set while
/// tolerating the explicit-`1` spelling the syntax layer can now surface.
fn arrow_key(csi: &crate::syntax::ControlSequence) -> Option<Key> {
    let params = csi.params();
    if !params.private_markers().is_empty() || !params.intermediates().is_empty() {
        return None;
    }
    if !matches!(params.param_bytes(), b"" | b"1") {
        return None;
    }
    match params.final_byte() {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        _ => None,
    }
}

/// Maps an escape token to either the standalone Escape key or lossless syntax passthrough.
fn escape_event(escape: EscapeSequence) -> Event {
    if escape.final_byte().is_none() {
        // A bare `ESC` with no final byte is only ever produced by `SyntaxParser::finish` when the
        // layer above flushed a standalone Escape.
        Event::Key(KeyEvent::new(Key::Escape))
    } else {
        Event::Syntax(SyntaxToken::Esc(escape))
    }
}
