//! Syntax token vocabulary for the total, lossless input decoder.
//!
//! A [`SyntaxToken`] classifies one span of terminal input bytes by its ECMA-48 syntactic family
//! without assigning protocol meaning. The layer is a total function over bytes: every input byte
//! belongs to exactly one emitted token, and concatenating the raw spans of the tokens reproduces
//! the input byte-for-byte (the single exception is a truncated string payload, which records the
//! dropped-byte count instead — see [`StringSequence::truncated`]).

/// A parsed CSI or DCS parameter separator.
///
/// ECMA-48 separates parameters with `;` (semicolon). Some modern protocols, such as SGR true
/// color and the kitty keyboard protocol, use `:` (colon) to group sub-parameters. The distinction
/// is meaningful, so the syntax layer preserves it rather than merging both into one separator
/// (design 02 invariant 4).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ParamSeparator {
    /// A semicolon (`;`, byte `0x3b`) separating two top-level parameters.
    Semicolon,
    /// A colon (`:`, byte `0x3a`) separating two sub-parameters within one parameter.
    Colon,
}

impl ParamSeparator {
    /// Returns the separator byte.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Semicolon => b';',
            Self::Colon => b':',
        }
    }
}

/// One numeric parameter value with the separator that preceded it.
///
/// A missing parameter (for example the empty field in `CSI ; 5 m`) has a `value` of `None`. The
/// `separator` is the byte that appeared immediately before this parameter in the raw parameter
/// bytes; the first parameter has no preceding separator and reports `None`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Param {
    separator: Option<ParamSeparator>,
    value: Option<u32>,
}

impl Param {
    /// Creates a parameter value from its separator and parsed number.
    #[must_use]
    pub const fn new(separator: Option<ParamSeparator>, value: Option<u32>) -> Self {
        Self { separator, value }
    }

    /// Returns the separator that preceded this parameter, if any.
    ///
    /// The first parameter in a sequence has no preceding separator and returns `None`. A `:`
    /// separator marks this parameter as a sub-parameter of the group started by the most recent
    /// `;`-separated (or leading) parameter.
    #[must_use]
    pub const fn separator(self) -> Option<ParamSeparator> {
        self.separator
    }

    /// Returns the parsed numeric value, or `None` for an empty (defaulted) parameter.
    ///
    /// A value is `None` when the parameter field was empty, as in the first field of `CSI ; 5 H`.
    /// Callers apply protocol-specific defaults; the syntax layer does not.
    #[must_use]
    pub const fn value(self) -> Option<u32> {
        self.value
    }
}

/// How a control-string sequence (OSC, DCS, APC, PM, SOS) was terminated.
///
/// String sequences end with String Terminator (ST). ST has a 7-bit spelling (`ESC \`) and an
/// 8-bit C1 spelling (`0x9c`). OSC additionally accepts BEL (`0x07`) as a widely supported
/// terminator. A sequence that never terminated before the input (or [`SyntaxParser::finish`])
/// ended is reported as [`StringTerminator::None`].
///
/// [`SyntaxParser::finish`]: crate::SyntaxParser::finish
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StringTerminator {
    /// Terminated by BEL (`0x07`). Accepted for OSC only.
    Bel,
    /// Terminated by the 7-bit String Terminator `ESC \` (`0x1b 0x5c`).
    EscBackslash,
    /// Terminated by the 8-bit C1 String Terminator (`0x9c`).
    C1,
    /// The sequence was not terminated before input ended.
    None,
}

impl StringTerminator {
    /// Returns the raw terminator bytes, or an empty slice for [`StringTerminator::None`].
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Bel => b"\x07",
            Self::EscBackslash => b"\x1b\\",
            Self::C1 => b"\x9c",
            Self::None => b"",
        }
    }
}

/// Which control-string family introduced a [`StringSequence`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StringKind {
    /// Operating System Command (`ESC ]` or `0x9d`).
    Osc,
    /// Device Control String (`ESC P` or `0x90`).
    Dcs,
    /// Application Program Command (`ESC _` or `0x9f`).
    Apc,
    /// Privacy Message (`ESC ^` or `0x9e`).
    Pm,
    /// Start of String (`ESC X` or `0x98`).
    Sos,
}

/// Structured access to CSI or DCS parameter, intermediate, and final bytes.
///
/// This carries both the raw parameter bytes and the parsed [`Param`] list so callers can choose
/// byte-exact preservation or numeric decoding. Private marker bytes (`0x3c..=0x3f`) that lead the
/// parameter bytes are exposed separately. When more than [`ControlParams::PARAM_LIMIT`] parameters
/// are present, parsing stops and [`ControlParams::params_overflowed`] is set; the raw bytes still
/// hold every parameter, so nothing is lost.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlParams {
    private_markers: Vec<u8>,
    param_bytes: Vec<u8>,
    params: Vec<Param>,
    params_overflowed: bool,
    intermediates: Vec<u8>,
    final_byte: u8,
}

impl ControlParams {
    /// The maximum number of parsed [`Param`] values retained.
    ///
    /// Matches the historical cap used by the first CSI decoder. Parameters beyond this cap are not
    /// parsed into the [`ControlParams::params`] list, but the raw parameter bytes returned by
    /// [`ControlParams::param_bytes`] always contain every parameter, so no bytes are lost.
    pub const PARAM_LIMIT: usize = 32;

    pub(crate) fn new(
        private_markers: Vec<u8>,
        param_bytes: Vec<u8>,
        intermediates: Vec<u8>,
        final_byte: u8,
    ) -> Self {
        let (params, params_overflowed) = parse_params(&param_bytes);
        Self {
            private_markers,
            param_bytes,
            params,
            params_overflowed,
            intermediates,
            final_byte,
        }
    }

    /// Returns the leading private marker bytes (`0x3c..=0x3f`), such as `?` in `CSI ? 25 h`.
    #[must_use]
    pub fn private_markers(&self) -> &[u8] {
        &self.private_markers
    }

    /// Returns the raw parameter bytes, excluding leading private markers.
    ///
    /// These bytes preserve the exact `:` versus `;` separators and every parameter even when the
    /// parsed [`ControlParams::params`] list is capped.
    #[must_use]
    pub fn param_bytes(&self) -> &[u8] {
        &self.param_bytes
    }

    /// Returns the parsed parameters, preserving `:` versus `;` separation.
    ///
    /// The list is capped at [`ControlParams::PARAM_LIMIT`]; check
    /// [`ControlParams::params_overflowed`] to learn whether parameters were dropped from this
    /// list.
    #[must_use]
    pub fn params(&self) -> &[Param] {
        &self.params
    }

    /// Returns `true` when there were more than [`ControlParams::PARAM_LIMIT`] parameters.
    ///
    /// This is a token flag, not silent truncation: the raw [`ControlParams::param_bytes`] still
    /// hold every parameter (design 02 invariant 4).
    #[must_use]
    pub fn params_overflowed(&self) -> bool {
        self.params_overflowed
    }

    /// Returns the intermediate bytes (`0x20..=0x2f`).
    #[must_use]
    pub fn intermediates(&self) -> &[u8] {
        &self.intermediates
    }

    /// Returns the final byte.
    ///
    /// For CSI this is a byte in `0x40..=0x7e`. For DCS this is the final byte that ended the
    /// control string's parameter prefix and began its payload.
    #[must_use]
    pub fn final_byte(&self) -> u8 {
        self.final_byte
    }
}

/// A complete control-string sequence: OSC, DCS, APC, PM, or SOS.
///
/// The value keeps the exact raw bytes (introducer, payload, and terminator) alongside the payload
/// span and the terminator kind. When the payload exceeds the parser's configured byte bound, the
/// stored payload is the retained prefix, [`StringSequence::truncated`] is `true`, and
/// [`StringSequence::dropped_bytes`] records how many payload bytes were counted and dropped
/// (design 02 invariant 3). DCS additionally exposes its parameter prefix through
/// [`StringSequence::control_params`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringSequence {
    kind: StringKind,
    bytes: Vec<u8>,
    payload: Vec<u8>,
    dropped_bytes: usize,
    terminator: StringTerminator,
    control_params: Option<ControlParams>,
}

impl StringSequence {
    pub(crate) fn new(
        kind: StringKind,
        bytes: Vec<u8>,
        payload: Vec<u8>,
        dropped_bytes: usize,
        terminator: StringTerminator,
        control_params: Option<ControlParams>,
    ) -> Self {
        Self {
            kind,
            bytes,
            payload,
            dropped_bytes,
            terminator,
            control_params,
        }
    }

    /// Returns which control-string family this sequence belongs to.
    #[must_use]
    pub fn kind(&self) -> StringKind {
        self.kind
    }

    /// Returns the raw bytes retained for this sequence.
    ///
    /// When [`StringSequence::truncated`] is `false`, these bytes are the exact input span. When it
    /// is `true`, the dropped payload tail is absent and [`StringSequence::dropped_bytes`] accounts
    /// for the difference.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the payload bytes, excluding the introducer and terminator.
    ///
    /// For DCS the payload begins after the parameter-prefix final byte returned by
    /// [`StringSequence::control_params`]. When truncated, this is the retained prefix only.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns how the sequence was terminated.
    #[must_use]
    pub fn terminator(&self) -> StringTerminator {
        self.terminator
    }

    /// Returns `true` when the payload exceeded the configured byte bound.
    ///
    /// A truncated sequence delivered its retained prefix through [`StringSequence::payload`] and
    /// counted the dropped tail in [`StringSequence::dropped_bytes`]. This is the only place the
    /// reconstruction invariant is deliberately waived, and the token says so.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.dropped_bytes > 0
    }

    /// Returns the number of payload bytes counted and dropped past the configured bound.
    ///
    /// This is `0` for any sequence that was not truncated.
    #[must_use]
    pub fn dropped_bytes(&self) -> usize {
        self.dropped_bytes
    }

    /// Returns the DCS parameter prefix, or `None` for OSC, APC, PM, and SOS.
    ///
    /// DCS carries a CSI-shaped parameter prefix (`private markers, params, intermediates, final`)
    /// before its payload, so its parameters are exposed the same way as [`ControlSequence`].
    #[must_use]
    pub fn control_params(&self) -> Option<&ControlParams> {
        self.control_params.as_ref()
    }
}

/// A complete CSI (Control Sequence Introducer) sequence.
///
/// The value keeps the exact raw bytes and the structured [`ControlParams`] access. The 7-bit
/// introducer is `ESC [`; the 8-bit C1 introducer is `0x9b` (design 02 invariant 5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlSequence {
    bytes: Vec<u8>,
    params: ControlParams,
}

impl ControlSequence {
    pub(crate) fn new(bytes: Vec<u8>, params: ControlParams) -> Self {
        Self { bytes, params }
    }

    /// Returns the exact raw bytes of the sequence, including the introducer and final byte.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns structured access to the parameter, intermediate, and final bytes.
    #[must_use]
    pub fn params(&self) -> &ControlParams {
        &self.params
    }
}

/// A complete non-CSI, non-string escape sequence, or a bare trailing Escape.
///
/// This covers `ESC` followed by zero or more intermediate bytes (`0x20..=0x2f`) and one final byte
/// (`0x30..=0x7e`), such as `ESC c` (reset) or `ESC ( B` (designate charset). A bare `ESC` reported
/// by [`SyntaxParser::finish`] at end of input is also an [`EscapeSequence`] with no final byte;
/// the syntax layer never guesses whether it was a standalone Escape key or the start of a longer
/// sequence (design 02: the parser never guesses ESC ambiguity).
///
/// [`SyntaxParser::finish`]: crate::SyntaxParser::finish
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscapeSequence {
    bytes: Vec<u8>,
    intermediates: Vec<u8>,
    final_byte: Option<u8>,
}

impl EscapeSequence {
    pub(crate) fn new(bytes: Vec<u8>, intermediates: Vec<u8>, final_byte: Option<u8>) -> Self {
        Self {
            bytes,
            intermediates,
            final_byte,
        }
    }

    /// Returns the exact raw bytes, including the leading `ESC`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the intermediate bytes (`0x20..=0x2f`) between `ESC` and the final byte.
    #[must_use]
    pub fn intermediates(&self) -> &[u8] {
        &self.intermediates
    }

    /// Returns the final byte, or `None` for a bare trailing `ESC` flushed by
    /// [`SyntaxParser::finish`].
    ///
    /// [`SyntaxParser::finish`]: crate::SyntaxParser::finish
    #[must_use]
    pub fn final_byte(&self) -> Option<u8> {
        self.final_byte
    }
}

/// One bounded segment of a bracketed-paste span: `ESC [ 200 ~ … ESC [ 201 ~`.
///
/// Bracketed paste is captured **at the syntax layer** as opaque payload rather than being left to
/// the semantic layer to reassemble from between-bracket tokens. The reason is byte fidelity: a
/// paste payload is *data*, not syntax. Reassembling it from ordinary tokens would run every
/// embedded byte through control-string tokenization, and an embedded over-long OSC/DCS would hit
/// the string payload bound and drop bytes the paste must keep (design 02's truncation waiver is
/// for syntax payloads, never for paste data). Treating everything between the brackets as opaque
/// bytes keeps the payload lossless regardless of content — see [the paste capture
/// rules](crate::SyntaxParser#bracketed-paste).
///
/// The end bracket is recognized only in its exact `ESC [ 201 ~` (or 8-bit `0x9b 201 ~`) form; any
/// other bytes, including embedded escape sequences that merely *look* like control data, are
/// payload.
///
/// # Lossless segmentation, not truncation
///
/// A paste is delivered losslessly regardless of size (R-IN-7). To keep parser memory bounded while
/// staying lossless, a paste larger than the configured byte bound is emitted as **several
/// segments** rather than one buffered blob or a dropped tail: each segment carries up to the bound
/// of payload, and [`is_final`](Self::is_final) marks the last one. The first segment carries the
/// `ESC [ 200 ~` start bracket ([`is_first`](Self::is_first)); the final segment carries the
/// `ESC [ 201 ~` end bracket when the paste closed normally. This is the two-mechanism rule (design
/// 02): a *terminated* paste is lossless (all its segments arrive, the last flagged final); an
/// *unterminated* paste flushed by [`SyntaxParser::finish`](crate::SyntaxParser::finish), or one
/// whose terminator never comes, still yields its bytes in bounded segments — the last flagged
/// final but carrying no end bracket — so it degrades visibly instead of hanging (FM-A8/FM-P12). No
/// paste byte is ever dropped; the bound caps *per-segment memory*, and the missing-terminator cost
/// is one segment of memory, not an unbounded buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasteSequence {
    bytes: Vec<u8>,
    payload: Vec<u8>,
    is_first: bool,
    is_final: bool,
    terminated: bool,
}

impl PasteSequence {
    pub(crate) fn new(
        bytes: Vec<u8>,
        payload: Vec<u8>,
        is_first: bool,
        is_final: bool,
        terminated: bool,
    ) -> Self {
        Self {
            bytes,
            payload,
            is_first,
            is_final,
            terminated,
        }
    }

    /// Returns the raw bytes retained for this paste segment.
    ///
    /// These are the exact input span this segment covers: the `ESC [ 200 ~` start bracket on the
    /// first segment, the payload bytes it carries, and the `ESC [ 201 ~` end bracket on a final
    /// segment of a terminated paste. Concatenating [`as_bytes`](Self::as_bytes) over every segment
    /// of a paste reproduces its input bytes exactly (design 02 invariant 1); paste has no
    /// truncation waiver.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns this segment's slice of the paste payload (the bytes between the brackets).
    ///
    /// These are the raw pasted bytes with no normalization. Line-ending normalization and control
    /// inspection happen in the semantic layer's paste event, not here.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns `true` when this is the first segment of its paste (the one carrying `ESC [ 200 ~`).
    #[must_use]
    pub fn is_first(&self) -> bool {
        self.is_first
    }

    /// Returns `true` when this is the last segment of its paste.
    ///
    /// A single-segment paste is both [`is_first`](Self::is_first) and `is_final`. The final
    /// segment carries the `ESC [ 201 ~` end bracket exactly when
    /// [`terminated`](Self::terminated) is `true`.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    /// Returns `true` when the paste closed with its `ESC [ 201 ~` end bracket.
    ///
    /// Only meaningful on the final segment. A paste flushed unterminated (input ended, or the
    /// terminator never arrived) reports `false`: the payload is still delivered, but the closing
    /// bracket is absent, so consumers know the paste degraded rather than closed (FM-A8).
    #[must_use]
    pub fn terminated(&self) -> bool {
        self.terminated
    }
}

/// One token in the total, lossless syntax layer.
///
/// Every input byte belongs to exactly one token. Concatenating [`SyntaxToken::as_bytes`] over the
/// emitted tokens reproduces the input byte-for-byte, except that a [`SyntaxToken::Osc`] or other
/// string token with [`StringSequence::truncated`] set records its dropped-byte count instead of
/// keeping the dropped payload tail (design 02 invariants 1 and 3).
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SyntaxToken {
    /// A maximal run of printable UTF-8 text, including multibyte characters.
    ///
    /// The bytes are guaranteed to be valid UTF-8. Invalid UTF-8 bytes are never text; they become
    /// [`SyntaxToken::Malformed`].
    Text(Vec<u8>),
    /// A single C0 control byte (`0x00..=0x1f` or `0x7f`) that is not a sequence introducer.
    ///
    /// `ESC` (`0x1b`) is never a `Control` token because it introduces escape and control
    /// sequences; it appears inside [`SyntaxToken::Csi`], [`SyntaxToken::Esc`], the string tokens,
    /// or (bare, at end of input) as an [`SyntaxToken::Esc`].
    Control(u8),
    /// A complete CSI sequence (7-bit `ESC [` or 8-bit `0x9b`).
    Csi(ControlSequence),
    /// A complete OSC (Operating System Command) sequence.
    Osc(StringSequence),
    /// A complete DCS (Device Control String) sequence.
    Dcs(StringSequence),
    /// A complete APC (Application Program Command) sequence.
    Apc(StringSequence),
    /// A complete PM (Privacy Message) sequence.
    Pm(StringSequence),
    /// A complete SOS (Start of String) sequence.
    Sos(StringSequence),
    /// A bracketed-paste span (`ESC [ 200 ~ … ESC [ 201 ~`), captured opaquely.
    ///
    /// The bytes between the brackets are the paste payload, kept verbatim without being tokenized
    /// as syntax (see [`PasteSequence`] for why capture lives at this layer). The semantic layer
    /// turns this into a paste event with line-ending normalization; the syntax layer only draws
    /// the opaque, bounded span.
    Paste(PasteSequence),
    /// A complete non-CSI, non-string escape sequence, or a bare trailing `ESC`.
    Esc(EscapeSequence),
    /// A byte run that cannot be valid syntax, carrying its exact bytes.
    ///
    /// This covers invalid UTF-8, sequences aborted by CAN (`0x18`) or SUB (`0x1a`) per ECMA-48,
    /// garbage after `ESC`, and incomplete sequences flushed by [`SyntaxParser::finish`]. Malformed
    /// bytes are never silently dropped (design 02 invariants 1 and 6).
    ///
    /// [`SyntaxParser::finish`]: crate::SyntaxParser::finish
    Malformed(Vec<u8>),
}

impl SyntaxToken {
    /// Returns the raw bytes retained for this token.
    ///
    /// For every token except a truncated string sequence, this is the exact input span.
    /// Concatenating this over an emitted token sequence reconstructs the input (design 02
    /// invariant 1).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(bytes) | Self::Malformed(bytes) => bytes,
            Self::Control(byte) => std::slice::from_ref(byte),
            Self::Csi(csi) => csi.as_bytes(),
            Self::Osc(string)
            | Self::Dcs(string)
            | Self::Apc(string)
            | Self::Pm(string)
            | Self::Sos(string) => string.as_bytes(),
            Self::Paste(paste) => paste.as_bytes(),
            Self::Esc(escape) => escape.as_bytes(),
        }
    }
}

fn parse_params(param_bytes: &[u8]) -> (Vec<Param>, bool) {
    if param_bytes.is_empty() {
        return (Vec::new(), false);
    }

    let mut params = Vec::new();
    let mut separator = None;
    let mut value: Option<u32> = None;
    let mut overflowed = false;

    let push = |separator: Option<ParamSeparator>, value: Option<u32>, params: &mut Vec<Param>| {
        if params.len() >= ControlParams::PARAM_LIMIT {
            return true;
        }
        params.push(Param::new(separator, value));
        false
    };

    for &byte in param_bytes {
        match byte {
            b';' | b':' => {
                overflowed |= push(separator, value, &mut params);
                separator = Some(if byte == b';' {
                    ParamSeparator::Semicolon
                } else {
                    ParamSeparator::Colon
                });
                value = None;
            }
            b'0'..=b'9' => {
                let digit = u32::from(byte - b'0');
                value = Some(value.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            }
            _ => {
                // Private markers are stripped before this point; any other byte cannot appear in
                // parameter bytes because the tokenizer only collects `0x30..=0x3f` here.
            }
        }
    }

    overflowed |= push(separator, value, &mut params);
    (params, overflowed)
}
