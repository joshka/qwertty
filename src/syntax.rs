//! Total, lossless, bounded, stateful syntax tokenizer for terminal input.
//!
//! This is the first of the two decoder layers described in design 02. It is a *total function
//! over bytes*: it classifies every input byte into a [`SyntaxToken`] by its ECMA-48 syntactic
//! family (text, C0 control, CSI, OSC, DCS, APC, PM, SOS, escape, or malformed) without assigning
//! any protocol meaning. The semantic layer above it turns these tokens into key, mouse, paste,
//! and report events; the syntax layer only decides *what shape* each byte run is.
//!
//! # Contract
//!
//! The layer upholds four invariants, each an acceptance criterion (design 02 invariants 1-6):
//!
//! - **Reconstruction.** Concatenating [`SyntaxToken::as_bytes`] over the emitted tokens reproduces
//!   the input byte-for-byte. Malformed input is preserved as a [`SyntaxToken::Malformed`] token
//!   carrying its bytes, never silently dropped. The one waiver is a string payload past the
//!   configured bound: it is streamed as a prefix plus a recorded dropped-byte count (see
//!   [`SyntaxParser::with_payload_limit`]).
//! - **Split-equivalence.** Any chunking of the same input yields the identical token sequence.
//!   Continuation state lives in the [`SyntaxParser`]; [`SyntaxParser::finish`] flushes pending
//!   bytes without guessing ESC ambiguity.
//! - **Bounded.** String-sequence payloads (OSC/DCS/APC/PM/SOS) buffer up to a configurable bound
//!   (default 64 KiB). The bound is enforced *while bytes accumulate*, not just when a token is
//!   built: an unterminated over-limit string keeps only the bounded prefix in parser memory while
//!   the tail is counted and dropped, and terminator scanning continues over the dropped bytes. The
//!   same byte bound caps every other accumulation path: an over-limit CSI/DCS parameter prefix or
//!   escape-intermediate run stops being sequence syntax and is emitted as
//!   [`SyntaxToken::Malformed`] carrying exactly the retained bytes (nothing dropped — the
//!   remaining bytes reparse as ordinary input), and an unbroken text run is emitted in bounded
//!   [`SyntaxToken::Text`] slices (lossless). Cap and slice positions depend only on the input
//!   bytes, never on chunk boundaries, so split-equivalence is preserved.
//! - **Param fidelity.** CSI and DCS parameters keep both raw bytes and parsed numbers, preserve
//!   `:` versus `;` separators, and flag param-count overflow rather than merging silently.
//!
//! The reconstruction waiver, precisely: only string-sequence *payload* bytes past the bound are
//! ever dropped, and always with the count recorded on the token. If an over-limit string is
//! aborted by CAN/SUB or a non-terminator escape, the truncated string token (with
//! [`StringTerminator::None`]) is emitted first and the aborting bytes then form their own tokens,
//! so the dropped count stays recoverable. All other over-limit paths re-classify bytes instead of
//! dropping them.
//!
//! C1 (8-bit) introducers and terminators are recognized per ECMA-48. Because bytes `0x80..=0x9f`
//! are also UTF-8 continuation bytes, a C1 byte is treated as an introducer only when it appears
//! where a new character would start — never in the middle of an in-progress UTF-8 sequence (see
//! [the C1 rule](#c1-versus-utf-8)).
//!
//! # C1 versus UTF-8
//!
//! The 8-bit control introducers (CSI `0x9b`, OSC `0x9d`, DCS `0x90`, APC `0x9f`, PM `0x9e`, SOS
//! `0x98`) and the String Terminator (`0x9c`) share their byte range with UTF-8 continuation bytes.
//! The tokenizer resolves the tension the standard way: it only recognizes a C1 byte as an
//! introducer or terminator at a position where a new character starts. A `0x9b` immediately after
//! a UTF-8 lead byte is consumed as that character's continuation byte (or, if it does not form
//! valid UTF-8, becomes [`SyntaxToken::Malformed`]); a `0x9b` at a text boundary introduces a CSI.
//!
//! # Example
//!
//! ```
//! use qwertty::{SyntaxParser, SyntaxToken};
//!
//! let mut parser = SyntaxParser::new();
//! let tokens = parser.feed(b"hi\x1b[31m");
//!
//! assert_eq!(tokens[0], SyntaxToken::Text(b"hi".to_vec()));
//! match &tokens[1] {
//!     SyntaxToken::Csi(csi) => {
//!         assert_eq!(csi.as_bytes(), b"\x1b[31m");
//!         assert_eq!(csi.params().final_byte(), b'm');
//!     }
//!     other => panic!("expected CSI, got {other:?}"),
//! }
//! assert!(parser.finish().is_empty());
//! ```

mod token;

pub use token::{
    ControlParams, ControlSequence, EscapeSequence, Param, ParamSeparator, StringKind,
    StringSequence, StringTerminator, SyntaxToken,
};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const DELETE: u8 = 0x7f;

const C1_ST: u8 = 0x9c;

/// Default string-payload byte bound (64 KiB), chosen to clear known-large legitimate payloads.
pub const DEFAULT_PAYLOAD_LIMIT: usize = 64 * 1024;

/// Streaming, stateful tokenizer that turns terminal input bytes into [`SyntaxToken`] values.
///
/// Feed input chunks with [`SyntaxParser::feed`]; the parser retains any partial sequence across
/// calls so a sequence split across reads produces the same tokens as feeding it whole. Call
/// [`SyntaxParser::finish`] at end of input to flush pending partial state as honest tokens.
///
/// Parser memory is bounded: no input, terminated or not, grows internal buffering past the
/// configured payload limit plus a small constant (see [`SyntaxParser::with_payload_limit`]).
///
/// # Example
///
/// A CSI sequence split across three chunks decodes identically to feeding it whole:
///
/// ```
/// use qwertty::{SyntaxParser, SyntaxToken};
///
/// let mut split = SyntaxParser::new();
/// let mut tokens = split.feed(b"\x1b[");
/// tokens.extend(split.feed(b"31"));
/// tokens.extend(split.feed(b"m"));
/// tokens.extend(split.finish());
///
/// assert_eq!(tokens.len(), 1);
/// assert_eq!(tokens[0].as_bytes(), b"\x1b[31m");
/// ```
#[derive(Clone, Debug)]
pub struct SyntaxParser {
    state: State,
    payload_limit: usize,
}

impl Default for SyntaxParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxParser {
    /// Creates a parser with the [`DEFAULT_PAYLOAD_LIMIT`] string-payload bound (64 KiB).
    #[must_use]
    pub fn new() -> Self {
        Self::with_payload_limit(DEFAULT_PAYLOAD_LIMIT)
    }

    /// Creates a parser with a custom byte bound for sequence accumulation.
    ///
    /// The bound caps how many payload bytes an OSC, DCS, APC, PM, or SOS sequence buffers, and it
    /// is enforced while the payload accumulates: payload bytes past the bound are counted and
    /// dropped immediately, so parser memory stays bounded even for an unterminated sequence. The
    /// resulting token reports [`StringSequence::truncated`] with the dropped count. A bound of
    /// `0` truncates every non-empty payload.
    ///
    /// The same bound caps CSI/DCS parameter prefixes, escape-intermediate runs, and the size of
    /// individual [`SyntaxToken::Text`] slices; those paths re-classify over-limit bytes instead
    /// of dropping them, so they stay reconstruction-exact.
    #[must_use]
    pub fn with_payload_limit(payload_limit: usize) -> Self {
        Self {
            state: State::Ground,
            payload_limit,
        }
    }

    /// Returns the configured string-payload byte bound.
    #[must_use]
    pub fn payload_limit(&self) -> usize {
        self.payload_limit
    }

    /// Returns the bytes retained from earlier [`SyntaxParser::feed`] calls awaiting completion.
    ///
    /// For a small partial (an escape or CSI prefix, an incomplete UTF-8 run, a text slice in
    /// progress) these are the exact buffered bytes. For an in-progress string sequence past the
    /// payload bound, this returns only the *retained* prefix: dropped payload bytes are counted
    /// on the eventual token but not stored, and an escape byte held as a possible `ESC \`
    /// terminator start is tracked separately until the next byte resolves it.
    #[must_use]
    pub fn pending_bytes(&self) -> &[u8] {
        match &self.state {
            State::Ground => &[],
            State::Partial(pending) => pending,
            State::InString(accum) => &accum.retained,
        }
    }

    /// Feeds one input chunk and returns the tokens that became complete.
    ///
    /// Bytes that begin but do not complete a sequence are buffered (bounded) for the next call.
    /// The returned vector is empty when the whole chunk only extends a pending sequence.
    #[must_use]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        let mut buffer = match std::mem::replace(&mut self.state, State::Ground) {
            State::Ground => bytes.to_vec(),
            State::Partial(mut pending) => {
                pending.extend_from_slice(bytes);
                pending
            }
            State::InString(mut accum) => {
                match consume_string(&mut accum, bytes, 0, self.payload_limit, &mut tokens) {
                    StringOutcome::Exhausted => {
                        self.state = State::InString(accum);
                        return tokens;
                    }
                    StringOutcome::Done(next) => bytes[next..].to_vec(),
                    StringOutcome::DoneWithHeldEsc(next) => held_esc_buffer(&bytes[next..]),
                }
            }
        };

        let mut index = 0;
        while index < buffer.len() {
            match step(&buffer, index, self.payload_limit) {
                Step::Emit(token, next) => {
                    tokens.push(token);
                    index = next;
                }
                Step::NeedMore => {
                    self.state = State::Partial(buffer[index..].to_vec());
                    return tokens;
                }
                Step::EnterString(mut accum, next) => {
                    match consume_string(&mut accum, &buffer, next, self.payload_limit, &mut tokens)
                    {
                        StringOutcome::Exhausted => {
                            self.state = State::InString(accum);
                            return tokens;
                        }
                        StringOutcome::Done(next) => index = next,
                        StringOutcome::DoneWithHeldEsc(next) => {
                            buffer = held_esc_buffer(&buffer[next..]);
                            index = 0;
                        }
                    }
                }
            }
        }

        tokens
    }

    /// Flushes any pending partial sequence and returns it as an honest token.
    ///
    /// A bare trailing `ESC` becomes an [`SyntaxToken::Esc`] with no final byte. An unterminated
    /// string sequence becomes its string token with [`StringTerminator::None`], bounded and
    /// truncation-flagged like a terminated one. A pending text run flushes as
    /// [`SyntaxToken::Text`] when it is complete UTF-8, or [`SyntaxToken::Malformed`] when it ends
    /// mid-character. Any other incomplete sequence (a CSI missing its final byte, a DCS still in
    /// its parameter prefix) becomes [`SyntaxToken::Malformed`] carrying its exact bytes. This
    /// never guesses ESC ambiguity; timing policy belongs to a layer above (design 02).
    #[must_use]
    pub fn finish(&mut self) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        match std::mem::replace(&mut self.state, State::Ground) {
            State::Ground => {}
            State::Partial(pending) => tokens.push(finish_pending(&pending)),
            State::InString(mut accum) => {
                if accum.esc_held {
                    // The held ESC turned out to be the last byte of an unterminated string, so
                    // it is a payload byte after all, subject to the same bound.
                    push_payload_byte(&mut accum, ESC, self.payload_limit);
                }
                emit_string_token(&mut accum, StringTerminator::None, &mut tokens);
            }
        }
        tokens
    }
}

/// Parser state between [`SyntaxParser::feed`] calls.
#[derive(Clone, Debug)]
enum State {
    /// No partial input.
    Ground,
    /// A small partial awaiting more bytes: an escape or CSI/DCS prefix, an incomplete UTF-8 run,
    /// or a text slice in progress. Every path into this state is capped near the payload limit,
    /// and the buffered bytes are re-scanned with the next chunk appended.
    Partial(Vec<u8>),
    /// Inside a string sequence's payload, scanning incrementally for its terminator. Payload
    /// accumulation is bounded; chunk bytes are consumed directly and never re-scanned.
    InString(StringAccum),
}

/// Accumulator for an in-progress string sequence (OSC/DCS/APC/PM/SOS).
#[derive(Clone, Debug)]
struct StringAccum {
    kind: StringKind,
    /// Retained raw bytes: introducer, DCS parameter prefix, and payload up to the bound.
    retained: Vec<u8>,
    /// Length of the introducer (`1` for a C1 byte, `2` for the 7-bit `ESC`-prefixed form).
    introducer_len: usize,
    /// Offset in `retained` where the payload begins.
    payload_start: usize,
    /// Payload bytes counted and dropped past the bound.
    dropped: usize,
    /// An ESC was seen and may start an `ESC \` terminator; the next byte resolves it.
    esc_held: bool,
    /// The DCS parameter prefix, parsed before the payload began.
    control_params: Option<ControlParams>,
}

/// Result of consuming chunk bytes while inside a string sequence.
enum StringOutcome {
    /// The chunk ended with the string still open.
    Exhausted,
    /// The string closed (terminated or aborted); continue parsing at this index.
    Done(usize),
    /// The string closed, and an ESC held from a *previous* chunk must be re-parsed before the
    /// bytes at this index.
    DoneWithHeldEsc(usize),
}

/// Builds a re-parse buffer for a held ESC from a previous chunk followed by the remaining bytes.
fn held_esc_buffer(rest: &[u8]) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(1 + rest.len());
    buffer.push(ESC);
    buffer.extend_from_slice(rest);
    buffer
}

/// Consumes bytes for an in-progress string sequence, enforcing the payload bound as bytes arrive.
///
/// Terminator detection works over bytes that are not being stored: payload bytes past the bound
/// are counted into the accumulator's dropped total and discarded, while BEL, C1 ST, `ESC \`, and
/// CAN/SUB are still recognized. Emits the completed (or, for over-limit aborts, the truncated
/// unterminated) token into `tokens`.
fn consume_string(
    accum: &mut StringAccum,
    bytes: &[u8],
    start: usize,
    limit: usize,
    tokens: &mut Vec<SyntaxToken>,
) -> StringOutcome {
    let mut esc_held = accum.esc_held;
    // Position of the held ESC when it lives in this buffer; `None` when held from a prior chunk.
    let mut esc_pos: Option<usize> = None;
    accum.esc_held = false;

    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if esc_held {
            match byte {
                b'\\' => {
                    emit_string_token(accum, StringTerminator::EscBackslash, tokens);
                    return StringOutcome::Done(index + 1);
                }
                CAN | SUB => {
                    // ECMA-48: CAN and SUB abort a control string in progress. With nothing
                    // dropped the whole aborted span is one exact-bytes malformed token; with
                    // drops the truncated string token accounts for them and the aborting bytes
                    // re-parse on their own.
                    if accum.dropped == 0 {
                        let mut malformed = std::mem::take(&mut accum.retained);
                        malformed.push(ESC);
                        malformed.push(byte);
                        tokens.push(SyntaxToken::Malformed(malformed));
                        return StringOutcome::Done(index + 1);
                    }
                    emit_string_token(accum, StringTerminator::None, tokens);
                    return match esc_pos {
                        Some(position) => StringOutcome::Done(position),
                        None => StringOutcome::DoneWithHeldEsc(index),
                    };
                }
                _ => {
                    // A lone ESC inside a string that is not ST aborts it; the ESC and this byte
                    // re-parse as ordinary input.
                    if accum.dropped == 0 {
                        tokens.push(SyntaxToken::Malformed(std::mem::take(&mut accum.retained)));
                    } else {
                        emit_string_token(accum, StringTerminator::None, tokens);
                    }
                    return match esc_pos {
                        Some(position) => StringOutcome::Done(position),
                        None => StringOutcome::DoneWithHeldEsc(index),
                    };
                }
            }
        }

        match byte {
            BEL if matches!(accum.kind, StringKind::Osc) => {
                emit_string_token(accum, StringTerminator::Bel, tokens);
                return StringOutcome::Done(index + 1);
            }
            C1_ST => {
                emit_string_token(accum, StringTerminator::C1, tokens);
                return StringOutcome::Done(index + 1);
            }
            ESC => {
                esc_held = true;
                esc_pos = Some(index);
            }
            CAN | SUB => {
                if accum.dropped == 0 {
                    let mut malformed = std::mem::take(&mut accum.retained);
                    malformed.push(byte);
                    tokens.push(SyntaxToken::Malformed(malformed));
                    return StringOutcome::Done(index + 1);
                }
                emit_string_token(accum, StringTerminator::None, tokens);
                return StringOutcome::Done(index);
            }
            _ => push_payload_byte(accum, byte, limit),
        }
        index += 1;
    }

    accum.esc_held = esc_held;
    StringOutcome::Exhausted
}

/// Stores one payload byte up to the bound, counting it as dropped past the bound.
///
/// The bound caps the *retained bytes past the introducer* (the DCS parameter prefix plus the kept
/// payload) at `limit`, not the payload alone: a DCS parameter prefix (itself capped at `limit` by
/// `scan_control_prefix`) counts against the same budget, so an over-long prefix shrinks the
/// retained payload rather than letting prefix and payload each reach `limit` and double parser
/// memory. Total parser memory for a string sequence therefore stays at `introducer_len + limit`.
/// Dropped bytes are still counted exactly, so reconstruction accounting is unaffected, and the cap
/// position depends only on the retained length, so split-equivalence holds.
fn push_payload_byte(accum: &mut StringAccum, byte: u8, limit: usize) {
    if accum.retained.len() - accum.introducer_len < limit {
        accum.retained.push(byte);
    } else {
        accum.dropped += 1;
    }
}

/// Builds the string token from the accumulator and pushes it.
fn emit_string_token(
    accum: &mut StringAccum,
    terminator: StringTerminator,
    tokens: &mut Vec<SyntaxToken>,
) {
    let payload = accum.retained[accum.payload_start..].to_vec();
    let mut raw = std::mem::take(&mut accum.retained);
    raw.extend_from_slice(terminator.as_bytes());
    tokens.push(string_token(
        accum.kind,
        StringSequence::new(
            accum.kind,
            raw,
            payload,
            accum.dropped,
            terminator,
            accum.control_params.take(),
        ),
    ));
}

fn string_token(kind: StringKind, sequence: StringSequence) -> SyntaxToken {
    match kind {
        StringKind::Osc => SyntaxToken::Osc(sequence),
        StringKind::Dcs => SyntaxToken::Dcs(sequence),
        StringKind::Apc => SyntaxToken::Apc(sequence),
        StringKind::Pm => SyntaxToken::Pm(sequence),
        StringKind::Sos => SyntaxToken::Sos(sequence),
    }
}

/// One parsing step at a token boundary.
enum Step {
    /// A complete token; continue at the index.
    Emit(SyntaxToken, usize),
    /// The bytes from the current index onward need more input.
    NeedMore,
    /// A string sequence's introducer (and DCS prefix) completed; consume its payload from the
    /// index onward through the bounded string accumulator.
    EnterString(StringAccum, usize),
}

/// Classifies the byte at `index`, which is always a token boundary.
fn step(bytes: &[u8], index: usize, cap: usize) -> Step {
    let byte = bytes[index];
    match byte {
        ESC => parse_escape(bytes, index, cap),
        0x9b => parse_csi(bytes, index, 1, cap),
        0x90 => parse_string(bytes, index, StringKind::Dcs, 1, cap),
        0x9d => parse_string(bytes, index, StringKind::Osc, 1, cap),
        0x9f => parse_string(bytes, index, StringKind::Apc, 1, cap),
        0x9e => parse_string(bytes, index, StringKind::Pm, 1, cap),
        0x98 => parse_string(bytes, index, StringKind::Sos, 1, cap),
        CAN | SUB => Step::Emit(SyntaxToken::Malformed(vec![byte]), index + 1),
        0x00..=0x1f | DELETE => Step::Emit(SyntaxToken::Control(byte), index + 1),
        // A lone C1 String Terminator at a character boundary is a stray terminator with no
        // string in progress: preserved as malformed, never silently dropped.
        C1_ST => Step::Emit(SyntaxToken::Malformed(vec![byte]), index + 1),
        _ => parse_text(bytes, index, cap),
    }
}

/// Classifies `ESC` at `index` by the byte after it.
fn parse_escape(bytes: &[u8], index: usize, cap: usize) -> Step {
    let Some(&second) = bytes.get(index + 1) else {
        return Step::NeedMore;
    };
    match second {
        b'[' => parse_csi(bytes, index, 2, cap),
        b'P' => parse_string(bytes, index, StringKind::Dcs, 2, cap),
        b']' => parse_string(bytes, index, StringKind::Osc, 2, cap),
        b'_' => parse_string(bytes, index, StringKind::Apc, 2, cap),
        b'^' => parse_string(bytes, index, StringKind::Pm, 2, cap),
        b'X' => parse_string(bytes, index, StringKind::Sos, 2, cap),
        CAN | SUB => Step::Emit(SyntaxToken::Malformed(vec![ESC, second]), index + 2),
        _ => parse_plain_escape(bytes, index, cap),
    }
}

/// Parses a complete CSI sequence starting at `start`, given the introducer length.
fn parse_csi(bytes: &[u8], start: usize, prefix_len: usize, cap: usize) -> Step {
    match scan_control_prefix(bytes, start + prefix_len, cap) {
        ControlPrefix::Complete {
            private_markers,
            param_bytes,
            intermediates,
            final_byte,
            end,
        } => {
            let params =
                ControlParams::new(private_markers, param_bytes, intermediates, final_byte);
            let token = SyntaxToken::Csi(ControlSequence::new(bytes[start..end].to_vec(), params));
            Step::Emit(token, end)
        }
        ControlPrefix::NeedMore => Step::NeedMore,
        ControlPrefix::Aborted(end) | ControlPrefix::Overflow(end) => {
            Step::Emit(SyntaxToken::Malformed(bytes[start..end].to_vec()), end)
        }
    }
}

/// Parses a string-sequence introducer (and, for DCS, its parameter prefix) into an accumulator.
fn parse_string(
    bytes: &[u8],
    start: usize,
    kind: StringKind,
    prefix_len: usize,
    cap: usize,
) -> Step {
    let mut cursor = start + prefix_len;

    // DCS carries a CSI-shaped parameter prefix before its payload.
    let control_params = if matches!(kind, StringKind::Dcs) {
        match scan_control_prefix(bytes, cursor, cap) {
            ControlPrefix::Complete {
                private_markers,
                param_bytes,
                intermediates,
                final_byte,
                end,
            } => {
                cursor = end;
                Some(ControlParams::new(
                    private_markers,
                    param_bytes,
                    intermediates,
                    final_byte,
                ))
            }
            ControlPrefix::NeedMore => return Step::NeedMore,
            ControlPrefix::Aborted(end) | ControlPrefix::Overflow(end) => {
                return Step::Emit(SyntaxToken::Malformed(bytes[start..end].to_vec()), end);
            }
        }
    } else {
        None
    };

    let accum = StringAccum {
        kind,
        retained: bytes[start..cursor].to_vec(),
        introducer_len: prefix_len,
        payload_start: cursor - start,
        dropped: 0,
        esc_held: false,
        control_params,
    };
    Step::EnterString(accum, cursor)
}

/// Result of scanning a CSI or DCS parameter prefix from `start`.
enum ControlPrefix {
    Complete {
        private_markers: Vec<u8>,
        param_bytes: Vec<u8>,
        intermediates: Vec<u8>,
        final_byte: u8,
        end: usize,
    },
    NeedMore,
    Aborted(usize),
    /// The prefix exceeded the byte cap; carries the position after the retained prefix bytes.
    Overflow(usize),
}

fn scan_control_prefix(bytes: &[u8], start: usize, cap: usize) -> ControlPrefix {
    let mut index = start;

    let private_start = index;
    while bytes.get(index).is_some_and(|&b| is_private_marker(b)) {
        index += 1;
    }
    let private_markers = bytes[private_start..index].to_vec();

    let param_start = index;
    while bytes.get(index).is_some_and(|&b| is_param_byte(b)) {
        index += 1;
    }
    let param_bytes = bytes[param_start..index].to_vec();

    let intermediate_start = index;
    while bytes.get(index).is_some_and(|&b| is_intermediate_byte(b)) {
        index += 1;
    }
    let intermediates = bytes[intermediate_start..index].to_vec();

    // The cap is a position-deterministic bound so chunking cannot change where it triggers.
    if index - start > cap {
        return ControlPrefix::Overflow(start + cap);
    }

    match bytes.get(index) {
        None => ControlPrefix::NeedMore,
        Some(&byte) if is_final_byte(byte) => ControlPrefix::Complete {
            private_markers,
            param_bytes,
            intermediates,
            final_byte: byte,
            end: index + 1,
        },
        // CAN/SUB abort the sequence per ECMA-48; any other byte here is garbage in the prefix.
        Some(&byte) if byte == CAN || byte == SUB => ControlPrefix::Aborted(index + 1),
        Some(_) => ControlPrefix::Aborted(index),
    }
}

/// Parses a run of printable UTF-8 text starting at `index`, in slices bounded by `cap`.
///
/// A run interrupted by the end of the buffer is buffered ([`Step::NeedMore`]) so the next feed
/// continues it, keeping text runs split-equivalent. A run that reaches `cap` bytes emits a
/// bounded slice at a position determined only by the run's content, so slicing is also
/// split-equivalent. Invalid UTF-8 becomes [`SyntaxToken::Malformed`].
fn parse_text(bytes: &[u8], index: usize, cap: usize) -> Step {
    let mut cursor = index;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if is_text_boundary(byte) {
            // A boundary byte ends the run for sure, so the accumulated text is final.
            return Step::Emit(SyntaxToken::Text(bytes[index..cursor].to_vec()), cursor);
        }

        let Some(width) = utf8_width(byte) else {
            // An invalid lead byte. Emit any accumulated text first, then the malformed byte.
            if cursor > index {
                return Step::Emit(SyntaxToken::Text(bytes[index..cursor].to_vec()), cursor);
            }
            return Step::Emit(SyntaxToken::Malformed(vec![byte]), cursor + 1);
        };

        let char_end = cursor + width;
        if char_end > bytes.len() {
            // Incomplete UTF-8 at the end of the buffer: buffer the run so the next feed can
            // continue it.
            return Step::NeedMore;
        }

        if std::str::from_utf8(&bytes[cursor..char_end]).is_err() {
            if cursor > index {
                return Step::Emit(SyntaxToken::Text(bytes[index..cursor].to_vec()), cursor);
            }
            return Step::Emit(
                SyntaxToken::Malformed(bytes[cursor..char_end].to_vec()),
                char_end,
            );
        }

        cursor = char_end;
        if cursor - index >= cap {
            // Bounded slice of an unbroken run: cut after the first character that reaches the
            // cap. The position depends only on the run bytes, never on chunk boundaries.
            return Step::Emit(SyntaxToken::Text(bytes[index..cursor].to_vec()), cursor);
        }
    }

    // The run reached the end of the buffer without a boundary. More text may follow in the next
    // feed, so buffer the run (bounded by the slice cap) rather than emitting a chunk-local text
    // token. `finish` flushes any trailing run.
    Step::NeedMore
}

/// Parses `ESC` followed by intermediates and one final byte, or reports [`Step::NeedMore`].
fn parse_plain_escape(bytes: &[u8], start: usize, cap: usize) -> Step {
    let mut index = start + 1;
    let intermediate_start = index;
    while bytes.get(index).is_some_and(|&b| is_intermediate_byte(b)) {
        index += 1;
    }

    // Position-deterministic cap, mirroring the CSI prefix bound.
    if index - intermediate_start > cap {
        let end = intermediate_start + cap;
        return Step::Emit(SyntaxToken::Malformed(bytes[start..end].to_vec()), end);
    }
    let intermediates = bytes[intermediate_start..index].to_vec();

    match bytes.get(index) {
        None => Step::NeedMore,
        Some(&byte) if is_escape_final_byte(byte) => {
            let escape =
                EscapeSequence::new(bytes[start..=index].to_vec(), intermediates, Some(byte));
            Step::Emit(SyntaxToken::Esc(escape), index + 1)
        }
        Some(&byte) if byte == CAN || byte == SUB => Step::Emit(
            SyntaxToken::Malformed(bytes[start..=index].to_vec()),
            index + 1,
        ),
        Some(_) => Step::Emit(SyntaxToken::Malformed(bytes[start..index].to_vec()), index),
    }
}

/// Flushes a pending small partial at end of input as its honest token.
///
/// String sequences never reach here (they live in the string accumulator), except a DCS still in
/// its parameter prefix, which flushes as [`SyntaxToken::Malformed`] rather than guessing where
/// its payload would have begun.
fn finish_pending(pending: &[u8]) -> SyntaxToken {
    // A pending buffer that starts with a text byte is a text run parked at a chunk boundary. If
    // it is entirely valid UTF-8 it flushes as text; if it ends mid-character the trailing bytes
    // are malformed, so the honest single token is Malformed carrying the whole run.
    if is_text_start(pending[0]) {
        return match std::str::from_utf8(pending) {
            Ok(_) => SyntaxToken::Text(pending.to_vec()),
            Err(_) => SyntaxToken::Malformed(pending.to_vec()),
        };
    }

    // A bare ESC (with only intermediates) is the one non-guessed case: a well-formed but
    // unterminated escape. Everything else pending is genuinely incomplete syntax.
    if pending[0] == ESC && pending[1..].iter().all(|&b| is_intermediate_byte(b)) {
        return SyntaxToken::Esc(EscapeSequence::new(
            pending.to_vec(),
            pending[1..].to_vec(),
            None,
        ));
    }

    SyntaxToken::Malformed(pending.to_vec())
}

/// Returns `true` for a byte that ends a text run: a C0 control, DEL, ESC, or a C1 introducer.
fn is_text_boundary(byte: u8) -> bool {
    matches!(byte, 0x00..=0x1f | DELETE) || is_c1_introducer(byte)
}

/// Returns `true` for a byte that can start a text run (the inverse of a boundary or stray ST).
fn is_text_start(byte: u8) -> bool {
    !is_text_boundary(byte) && byte != C1_ST
}

fn is_c1_introducer(byte: u8) -> bool {
    matches!(byte, 0x90 | 0x98 | 0x9b | 0x9c | 0x9d | 0x9e | 0x9f)
}

fn is_private_marker(byte: u8) -> bool {
    matches!(byte, 0x3c..=0x3f)
}

fn is_param_byte(byte: u8) -> bool {
    matches!(byte, 0x30..=0x3b)
}

fn is_intermediate_byte(byte: u8) -> bool {
    matches!(byte, 0x20..=0x2f)
}

fn is_final_byte(byte: u8) -> bool {
    matches!(byte, 0x40..=0x7e)
}

fn is_escape_final_byte(byte: u8) -> bool {
    matches!(byte, 0x30..=0x7e)
}

fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
