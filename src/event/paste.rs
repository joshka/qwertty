//! Paste event vocabulary, line-ending normalization, and control inspection.
//!
//! A [`PasteEvent`] is one bounded segment of a bracketed paste (design 02's two-mechanism rule).
//! The syntax layer captures the paste opaquely and segments a large one losslessly (see
//! [`PasteSequence`](crate::PasteSequence)); this layer turns each segment into a `PasteEvent`,
//! normalizes its line endings, and exposes [`contains_control`](PasteEvent::contains_control) for
//! paste hygiene (R-SEC-3, FM-X5).
//!
//! # Segmentation
//!
//! A small paste is one `PasteEvent` with [`is_final`](PasteEvent::is_final) `true`. A paste larger
//! than the decoder's byte bound arrives as several events, each carrying up to the bound of
//! payload, the last flagged final — so an application can stream a huge paste without the library
//! buffering it whole, and a keybinding never fires mid-paste (FM-P12). A missing end bracket still
//! yields the payload in bounded segments, the last final but
//! [`terminated`](PasteEvent::terminated) `false`, so it degrades visibly rather than hanging
//! (FM-A8).
//!
//! # Line-ending normalization
//!
//! Terminals differ on the byte a pasted newline uses: some send carriage return (`\r`), some send
//! CRLF (`\r\n`), some send LF (`\n`). Left unnormalized, this loses newlines across terminals
//! (FM-P12: reedline#576, crossterm#780). The paste payload is normalized so every line ending
//! becomes a single LF: a CRLF pair collapses to one `\n`, and a lone `\r` becomes `\n`. The
//! normalization is applied across segment boundaries (a `\r` ending one segment and a `\n` opening
//! the next collapse to one `\n`), so segmenting a paste never changes its normalized text.

use crate::syntax::PasteSequence;

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// One bounded segment of a decoded bracketed paste.
///
/// The [`data`](PasteEvent::data) is the segment's pasted bytes with line endings normalized to LF.
/// The struct is `#[non_exhaustive]`.
///
/// # Example
///
/// ```
/// use qwertty::SemanticDecoder;
///
/// let mut decoder = SemanticDecoder::new();
/// // A pasted "a\r\nb" (CRLF) normalizes to "a\nb"; one final segment.
/// let events = decoder.feed(b"\x1b[200~a\r\nb\x1b[201~");
/// let paste = events[0].paste_event().expect("a paste event");
///
/// assert_eq!(paste.data(), b"a\nb");
/// assert!(paste.is_final());
/// assert!(paste.terminated());
/// assert!(!paste.contains_control());
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PasteEvent {
    data: Vec<u8>,
    is_first: bool,
    is_final: bool,
    terminated: bool,
}

impl PasteEvent {
    /// Returns this segment's pasted bytes with line endings normalized to LF.
    ///
    /// The bytes are the paste payload as data: no escape sequences are interpreted, and control
    /// bytes other than the normalized line endings are preserved (inspect them with
    /// [`contains_control`](Self::contains_control)).
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns this segment's normalized bytes as a UTF-8 string, when they are valid UTF-8.
    ///
    /// Pasted text is usually UTF-8, but a paste can carry arbitrary bytes, so this returns `None`
    /// rather than lossily replacing invalid bytes. Use [`data`](Self::data) for the raw bytes.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }

    /// Returns `true` when this is the first segment of its paste.
    #[must_use]
    pub fn is_first(&self) -> bool {
        self.is_first
    }

    /// Returns `true` when this is the last segment of its paste.
    ///
    /// A single-segment paste is both first and final. After a final segment the next paste (if
    /// any) starts fresh.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    /// Returns `true` when the paste closed with its `ESC [ 201 ~` end bracket.
    ///
    /// Meaningful on the final segment. A paste whose end bracket never arrived reports `false`:
    /// the payload is still delivered, but the application knows it degraded rather than closed
    /// (FM-A8).
    #[must_use]
    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// Returns `true` when the normalized payload contains a control byte.
    ///
    /// This is the paste-hygiene hook (R-SEC-3, FM-X5 pastejacking): pasted content is *data*, but
    /// data carrying control bytes (escape sequences, C0 controls other than the normalized `\n`,
    /// or `DEL`) can drive a naive consumer that echoes paste back to the terminal. A control byte
    /// here lets a policy decide to strip, reject, or warn. The normalized newline (`\n`, `0x0a`)
    /// is **not** counted as a control byte; every other C0 byte (`0x00..=0x1f`) and `DEL`
    /// (`0x7f`) is.
    #[must_use]
    pub fn contains_control(&self) -> bool {
        self.data
            .iter()
            .any(|&byte| (byte <= 0x1f && byte != LF) || byte == 0x7f)
    }
}

/// Normalizes a paste segment's line endings, threading a trailing-CR carry across segments.
///
/// `pending_cr` is `true` when the previous segment ended with a carriage return that has not yet
/// been resolved (it might have been the CR of a CRLF split across the segment boundary). Returns
/// the normalized bytes for this segment and whether *this* segment ends with an unresolved
/// trailing CR to carry into the next.
///
/// The rule: a `\r` becomes `\n`; a `\r\n` pair becomes one `\n` (the `\n` is dropped because the
/// `\r` already produced the newline). A `\r` at the very end of a segment is held (returned as a
/// carry) so that if the next segment opens with `\n` the pair still collapses to one newline.
fn normalize(payload: &[u8], mut pending_cr: bool) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(payload.len() + usize::from(pending_cr));
    for &byte in payload {
        if pending_cr {
            // The previous byte was a CR we already turned into a newline. If this byte is the LF
            // of a CRLF pair, drop it; otherwise it is ordinary content.
            pending_cr = false;
            if byte == LF {
                continue;
            }
        }
        match byte {
            CR => {
                out.push(LF);
                pending_cr = true;
            }
            other => out.push(other),
        }
    }
    (out, pending_cr)
}

/// Turns a syntax paste segment into a [`PasteEvent`], normalizing line endings with the carried CR
/// state. Returns the event and the updated trailing-CR carry for the next segment.
pub(crate) fn decode(paste: &PasteSequence, pending_cr: bool) -> (PasteEvent, bool) {
    let (data, carry) = normalize(paste.payload(), pending_cr);
    // A trailing CR carried past the final segment resolved to a newline already emitted; the carry
    // only matters between segments of the same paste, so the final segment clears it for the next
    // paste.
    let carry = if paste.is_final() { false } else { carry };
    let event = PasteEvent {
        data,
        is_first: paste.is_first(),
        is_final: paste.is_final(),
        terminated: paste.terminated(),
    };
    (event, carry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxParser, SyntaxToken};

    /// Decodes `bytes` into paste events through the syntax layer, threading the CR carry.
    fn paste_events(bytes: &[u8], limit: usize) -> Vec<PasteEvent> {
        let mut parser = SyntaxParser::with_payload_limit(limit);
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        let mut pending_cr = false;
        let mut events = Vec::new();
        for token in tokens {
            if let SyntaxToken::Paste(paste) = token {
                let (event, carry) = decode(&paste, pending_cr);
                pending_cr = carry;
                events.push(event);
            }
        }
        events
    }

    fn single(bytes: &[u8]) -> PasteEvent {
        let mut events = paste_events(bytes, 1 << 20);
        assert_eq!(events.len(), 1, "expected one paste event from {bytes:?}");
        events.pop().expect("one event")
    }

    #[test]
    fn plain_paste_is_final_terminated() {
        let event = single(b"\x1b[200~hello\x1b[201~");
        assert_eq!(event.data(), b"hello");
        assert!(event.is_first() && event.is_final() && event.terminated());
        assert!(!event.contains_control());
    }

    #[test]
    fn crlf_collapses_to_lf() {
        assert_eq!(single(b"\x1b[200~a\r\nb\x1b[201~").data(), b"a\nb");
    }

    #[test]
    fn lone_cr_becomes_lf() {
        assert_eq!(single(b"\x1b[200~a\rb\x1b[201~").data(), b"a\nb");
    }

    #[test]
    fn lone_lf_is_kept() {
        assert_eq!(single(b"\x1b[200~a\nb\x1b[201~").data(), b"a\nb");
    }

    #[test]
    fn mixed_line_endings_all_normalize() {
        // "\r\n" -> "\n", "\r" -> "\n", "\n" kept.
        assert_eq!(
            single(b"\x1b[200~a\r\nb\rc\nd\x1b[201~").data(),
            b"a\nb\nc\nd"
        );
    }

    #[test]
    fn trailing_cr_becomes_single_lf() {
        assert_eq!(single(b"\x1b[200~line\r\x1b[201~").data(), b"line\n");
    }

    #[test]
    fn contains_control_flags_embedded_escape() {
        // A pasted ESC (pastejacking vector) is preserved as data and flagged.
        let event = single(b"\x1b[200~ok\x1b[31mred\x1b[201~");
        assert!(event.contains_control());
        // The ESC and CSI bytes are preserved verbatim as paste data, not interpreted.
        assert_eq!(event.data(), b"ok\x1b[31mred");
    }

    #[test]
    fn contains_control_ignores_normalized_newline() {
        let event = single(b"\x1b[200~line one\r\nline two\x1b[201~");
        assert!(
            !event.contains_control(),
            "a normalized newline is not a control byte"
        );
    }

    #[test]
    fn unterminated_paste_is_final_but_not_terminated() {
        let event = single(b"\x1b[200~no end");
        assert_eq!(event.data(), b"no end");
        assert!(event.is_final());
        assert!(!event.terminated());
    }

    #[test]
    fn crlf_split_across_segments_collapses_to_one_newline() {
        // With a tiny bound the paste segments; if a `\r` ends one segment and a `\n` begins the
        // next, the carry must collapse them to a single `\n` — segmenting never changes the text.
        // Payload "ab\r\ncd" with limit 3: segment boundary can fall between the CR and LF.
        let events = paste_events(b"\x1b[200~ab\r\ncd\x1b[201~", 3);
        assert!(events.len() > 1);
        let mut joined = Vec::new();
        for event in &events {
            joined.extend_from_slice(event.data());
        }
        assert_eq!(joined, b"ab\ncd");
    }
}
