//! Typed terminal reports parsed from the lossless syntax layer.
//!
//! A report is a host-visible reply a terminal sends in answer to a query: a cursor position
//! report, a device status report, primary device attributes, DEC private mode reports, an
//! XTVERSION report, and OSC colour reports. Each type parses one complete syntax token — a CSI
//! [`ControlSequence`], a DCS string, or an OSC payload from the [syntax layer](crate::SyntaxToken)
//! — into a typed value, rejecting anything that is not exactly the report shape it recognizes.
//!
//! These parsers are **pure and side-effect-free**: they read a syntax token and return a typed
//! value or `None`. They do not read a terminal, prove which request caused a report, or apply
//! timeout policy. Correlating a report to the query that provoked it is the job of the internal
//! query correlator; matching happens over these same typed parsers.
//!
//! # Canonical paths
//!
//! These types are the single home for terminal report parsing. They are re-exported at the crate
//! root ([`crate::CursorPositionReport`], [`crate::TerminalStatusReport`],
//! [`crate::TerminalStatus`]) for convenience and are also reachable through this module as
//! [`report::`](self) for a stable module path — the ghostty-rs encode oracle uses the module path.
//! Both paths name the same types.
//!
//! An earlier input slice shipped `CursorPositionReport` and `TerminalStatusReport` parsers over a
//! basic `CsiInput` value; that path has been retired, and these `ControlSequence`-based parsers
//! are the only report parsers qwertty ships.
//!
//! [`report::`]: self

use crate::ProtocolPosition;
use crate::caps::Rgb;
use crate::syntax::{ControlSequence, StringSequence};

/// A parsed terminal cursor position report.
///
/// Cursor position reports are sent by a terminal in response to a `CSI 6 n` cursor position query
/// (the Device Status Report cursor form). The shape this type recognizes is `CSI row ; column R`,
/// where row and column are one-based decimal protocol coordinates.
///
/// # Accepted shape
///
/// The report must be a CSI sequence with:
///
/// - final byte `R`;
/// - no private marker bytes and no intermediate bytes;
/// - exactly two `;`-separated decimal parameters, both present;
/// - each parameter greater than zero and no larger than [`u16::MAX`].
///
/// Anything else — a different final byte, private markers or intermediates, a missing or extra
/// field, a non-decimal or zero field, or a value that overflows `u16` — is rejected with `None`.
///
/// # Modified-F3 ambiguity
///
/// This type parses the CPR *shape*; it does not resolve the collision with the modified-F3 key
/// report (`CSI 1 ; modifier R`). That disambiguation is a correlation policy and lives in the
/// internal query correlator, not here: a raw `CSI 1 ; 2 R` is a syntactically valid CPR at row 1,
/// and this parser accepts it. The correlator's cursor-position matcher is the layer that refuses
/// the ambiguous form.
///
/// # Example
///
/// ```
/// use qwertty::report::CursorPositionReport;
/// use qwertty::{ProtocolPosition, SyntaxParser, SyntaxToken};
///
/// let mut parser = SyntaxParser::new();
/// let tokens = parser.feed(b"\x1b[12;34R");
/// let SyntaxToken::Csi(csi) = &tokens[0] else {
///     panic!("expected a CSI token");
/// };
///
/// let report = CursorPositionReport::from_control_sequence(csi).expect("cursor position report");
/// assert_eq!(report.position(), ProtocolPosition::new(12, 34));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CursorPositionReport {
    position: ProtocolPosition,
}

impl CursorPositionReport {
    /// Creates a cursor position report value.
    #[must_use]
    pub const fn new(position: ProtocolPosition) -> Self {
        Self { position }
    }

    /// Parses a cursor position report from a complete CSI control sequence.
    ///
    /// Returns `None` when the sequence is not exactly `CSI row ; column R`: a different final
    /// byte, any private marker or intermediate byte, a missing or extra field, a non-decimal
    /// or zero field, or a coordinate that does not fit in `u16`.
    ///
    /// # Example
    ///
    /// ```
    /// use qwertty::report::CursorPositionReport;
    /// use qwertty::{SyntaxParser, SyntaxToken};
    ///
    /// let mut parser = SyntaxParser::new();
    /// // A device status report, not a cursor report: rejected.
    /// let tokens = parser.feed(b"\x1b[0n");
    /// let SyntaxToken::Csi(csi) = &tokens[0] else {
    ///     panic!("expected a CSI token");
    /// };
    /// assert!(CursorPositionReport::from_control_sequence(csi).is_none());
    /// ```
    #[must_use]
    pub fn from_control_sequence(csi: &ControlSequence) -> Option<Self> {
        let params = csi.params();
        if params.final_byte() != b'R'
            || !params.private_markers().is_empty()
            || !params.intermediates().is_empty()
        {
            return None;
        }

        let mut fields = params.param_bytes().split(|&byte| byte == b';');
        let row = parse_one_based_u16(fields.next()?)?;
        let column = parse_one_based_u16(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }

        Some(Self::new(ProtocolPosition::new(row, column)))
    }

    /// Returns the reported one-based terminal protocol position.
    #[must_use]
    pub const fn position(self) -> ProtocolPosition {
        self.position
    }

    /// Returns the reported one-based row.
    #[must_use]
    pub const fn row(self) -> u16 {
        self.position.row()
    }

    /// Returns the reported one-based column.
    #[must_use]
    pub const fn column(self) -> u16 {
        self.position.column()
    }
}

/// Reported terminal status.
///
/// These values are sent by a terminal in response to a `CSI 5 n` Device Status Report status
/// query.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum TerminalStatus {
    /// Terminal is ready, reported as `CSI 0 n`.
    Ready,
    /// Terminal reports a malfunction, reported as `CSI 3 n`.
    Malfunction,
}

impl TerminalStatus {
    /// Returns the report parameter bytes for this status.
    #[must_use]
    pub const fn parameter_bytes(self) -> &'static [u8] {
        match self {
            Self::Ready => b"0",
            Self::Malfunction => b"3",
        }
    }
}

/// A parsed terminal status report.
///
/// Terminal status reports are sent by a terminal in response to a `CSI 5 n` Device Status Report
/// status query. The shapes this type recognizes are `CSI 0 n` for ready and `CSI 3 n` for
/// malfunction.
///
/// # Accepted shape
///
/// The report must be a CSI sequence with final byte `n`, no private markers or intermediate bytes,
/// and a single parameter of exactly `0` (ready) or `3` (malfunction). Any other parameter, a
/// private marker (`CSI ? 0 n` is a DEC private status form, not this report), an intermediate
/// byte, or a different final byte is rejected with `None`.
///
/// # Example
///
/// ```
/// use qwertty::report::{TerminalStatus, TerminalStatusReport};
/// use qwertty::{SyntaxParser, SyntaxToken};
///
/// let mut parser = SyntaxParser::new();
/// let tokens = parser.feed(b"\x1b[0n");
/// let SyntaxToken::Csi(csi) = &tokens[0] else {
///     panic!("expected a CSI token");
/// };
///
/// let report = TerminalStatusReport::from_control_sequence(csi).expect("terminal status report");
/// assert_eq!(report.status(), TerminalStatus::Ready);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TerminalStatusReport {
    status: TerminalStatus,
}

impl TerminalStatusReport {
    /// Creates a terminal status report value.
    #[must_use]
    pub const fn new(status: TerminalStatus) -> Self {
        Self { status }
    }

    /// Parses a terminal status report from a complete CSI control sequence.
    ///
    /// Returns `None` when the sequence is not exactly `CSI 0 n` or `CSI 3 n`: any private marker
    /// or intermediate byte, a different final byte, or any other status parameter.
    ///
    /// # Example
    ///
    /// ```
    /// use qwertty::report::{TerminalStatus, TerminalStatusReport};
    /// use qwertty::{SyntaxParser, SyntaxToken};
    ///
    /// let mut parser = SyntaxParser::new();
    /// let tokens = parser.feed(b"\x1b[3n");
    /// let SyntaxToken::Csi(csi) = &tokens[0] else {
    ///     panic!("expected a CSI token");
    /// };
    ///
    /// let report = TerminalStatusReport::from_control_sequence(csi).expect("terminal status report");
    /// assert_eq!(report.status(), TerminalStatus::Malfunction);
    /// ```
    #[must_use]
    pub fn from_control_sequence(csi: &ControlSequence) -> Option<Self> {
        let params = csi.params();
        if params.final_byte() != b'n'
            || !params.private_markers().is_empty()
            || !params.intermediates().is_empty()
        {
            return None;
        }

        let status = match params.param_bytes() {
            b"0" => TerminalStatus::Ready,
            b"3" => TerminalStatus::Malfunction,
            _ => return None,
        };

        Some(Self::new(status))
    }

    /// Returns the reported terminal status.
    #[must_use]
    pub const fn status(self) -> TerminalStatus {
        self.status
    }
}

/// The reported state of a DEC private mode, as carried by a DECRPM report.
///
/// This is the second parameter of `CSI ? mode ; value $ y`. The five states are the xterm/DECRPM
/// value set; `0` (not recognized) is the case a probe reads as "no answer for this feature"
/// (design 06: unknown, not unsupported).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum DecPrivateModeState {
    /// The terminal does not recognize the mode (value `0`).
    NotRecognized,
    /// The mode is currently set/enabled (value `1`).
    Set,
    /// The mode is currently reset/disabled (value `2`).
    Reset,
    /// The mode is permanently set and cannot be changed (value `3`).
    PermanentlySet,
    /// The mode is permanently reset and cannot be changed (value `4`).
    PermanentlyReset,
}

impl DecPrivateModeState {
    /// Maps a DECRPM value (`0`–`4`) to a state, or `None` for any other value.
    #[must_use]
    const fn from_value(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::NotRecognized,
            1 => Self::Set,
            2 => Self::Reset,
            3 => Self::PermanentlySet,
            4 => Self::PermanentlyReset,
            _ => return None,
        })
    }

    /// Returns `true` when the mode is enabled now (set or permanently set).
    ///
    /// This is the yes half of the tri-state a capability probe reads from a DECRPM answer: a mode
    /// reported [`Set`](Self::Set) or [`PermanentlySet`](Self::PermanentlySet) is enabled. A
    /// [`NotRecognized`](Self::NotRecognized) answer is neither enabled nor disabled — it is the
    /// terminal saying it does not know the mode — so it returns `None` from
    /// [`is_enabled`](Self::is_enabled) rather than `false`.
    #[must_use]
    pub const fn is_enabled(self) -> Option<bool> {
        match self {
            Self::Set | Self::PermanentlySet => Some(true),
            Self::Reset | Self::PermanentlyReset => Some(false),
            Self::NotRecognized => None,
        }
    }
}

/// A parsed DEC private mode report (DECRPM).
///
/// A terminal sends this in answer to a DEC private-mode DECRQM query `CSI ? mode $ p`. The shape
/// this type recognizes is `CSI ? mode ; value $ y`: a `?` private marker, two `;`-separated
/// decimal parameters, a `$` intermediate, and the final byte `y`.
///
/// # The discriminator is the mode number (FM-Q10)
///
/// The report carries **both** the queried mode number and the reported state. Two concurrent
/// DECRQM queries (for example mode 2026 and mode 2027) send two reports that differ only in the
/// mode field; a correlator that ignored the mode could complete the wrong query. This type keeps
/// the mode so a matcher can require it, which is exactly the fix for the prototype's
/// cross-completion bug (the internal correlator's `DecPrivateMode { mode }` expectation carries
/// the same discriminator).
///
/// # Accepted shape
///
/// - final byte `y`;
/// - a `?` private marker and no other marker bytes;
/// - a single `$` intermediate byte;
/// - exactly two `;`-separated decimal parameters, both present and fitting `u16`;
/// - the second parameter (the state) is `0`–`4`.
///
/// Anything else — a different final byte, a missing `?` or `$`, a wrong parameter count, an empty
/// field, an overflowing number, or an out-of-range state — is rejected with `None`.
///
/// # Example
///
/// ```
/// use qwertty::report::{DecPrivateModeReport, DecPrivateModeState};
/// use qwertty::{SyntaxParser, SyntaxToken};
///
/// let mut parser = SyntaxParser::new();
/// let tokens = parser.feed(b"\x1b[?2026;1$y");
/// let SyntaxToken::Csi(csi) = &tokens[0] else {
///     panic!("expected a CSI token");
/// };
///
/// let report = DecPrivateModeReport::from_control_sequence(csi).expect("DECRPM report");
/// assert_eq!(report.mode(), 2026);
/// assert_eq!(report.state(), DecPrivateModeState::Set);
/// assert_eq!(report.is_enabled(), Some(true));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DecPrivateModeReport {
    mode: u16,
    state: DecPrivateModeState,
}

impl DecPrivateModeReport {
    /// Creates a DEC private mode report value.
    #[must_use]
    pub const fn new(mode: u16, state: DecPrivateModeState) -> Self {
        Self { mode, state }
    }

    /// Parses a DEC private mode report from a complete CSI control sequence.
    ///
    /// Returns `None` when the sequence is not exactly `CSI ? mode ; value $ y` with a state value
    /// in `0`–`4`. See the [type docs](Self#accepted-shape) for the full acceptance.
    ///
    /// # Example
    ///
    /// ```
    /// use qwertty::report::DecPrivateModeReport;
    /// use qwertty::{SyntaxParser, SyntaxToken};
    ///
    /// let mut parser = SyntaxParser::new();
    /// // The non-private ANSI-mode form (no `?`) is a different report: rejected.
    /// let tokens = parser.feed(b"\x1b[4;2$y");
    /// let SyntaxToken::Csi(csi) = &tokens[0] else {
    ///     panic!("expected a CSI token");
    /// };
    /// assert!(DecPrivateModeReport::from_control_sequence(csi).is_none());
    /// ```
    #[must_use]
    pub fn from_control_sequence(csi: &ControlSequence) -> Option<Self> {
        let params = csi.params();
        if params.final_byte() != b'y'
            || params.private_markers() != b"?"
            || params.intermediates() != b"$"
        {
            return None;
        }

        let mut fields = params.param_bytes().split(|&byte| byte == b';');
        let mode = parse_u16(fields.next()?)?;
        let value = parse_u16(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }

        let state = DecPrivateModeState::from_value(value)?;
        Some(Self::new(mode, state))
    }

    /// Returns the queried private-mode number (the discriminator, FM-Q10).
    #[must_use]
    pub const fn mode(self) -> u16 {
        self.mode
    }

    /// Returns the reported mode state.
    #[must_use]
    pub const fn state(self) -> DecPrivateModeState {
        self.state
    }

    /// Returns whether the mode is enabled now, or `None` when the terminal does not recognize it.
    ///
    /// This forwards [`DecPrivateModeState::is_enabled`]: `Some(true)` for set/permanently-set,
    /// `Some(false)` for reset/permanently-reset, and `None` for not-recognized (unknown, design
    /// 06).
    #[must_use]
    pub const fn is_enabled(self) -> Option<bool> {
        self.state.is_enabled()
    }
}

/// A parsed XTVERSION report: the terminal's name and version string.
///
/// A terminal sends this in answer to the XTVERSION query `CSI > q`. The reply is a DCS string:
/// `DCS > | text ST`. The syntax layer parses the `> |` prefix as the DCS parameter prefix (a `>`
/// private marker and a `|` final byte), so the version text is the DCS *payload* — the terminal's
/// self-reported identification (for example `xterm(390)` or `ghostty 1.0.0`).
///
/// The version text is preserved verbatim as a UTF-8 string; this type does not attempt to split it
/// into a program name and a semantic version (that identity parsing is a later slice, design 06).
///
/// # Accepted shape
///
/// The report must be a DCS whose parameter prefix is exactly the `>` private marker and the `|`
/// final byte (no other parameters or intermediates). Its payload is the version text, taken
/// verbatim; an empty version (`DCS > | ST`) is accepted and yields an empty string. A DCS with a
/// different parameter prefix, or a non-DCS token, is rejected with `None`. Invalid UTF-8 in the
/// version text is rejected.
///
/// # Example
///
/// ```
/// use qwertty::report::XtVersionReport;
/// use qwertty::{SyntaxParser, SyntaxToken};
///
/// let mut parser = SyntaxParser::new();
/// let tokens = parser.feed(b"\x1bP>|ghostty 1.0.0\x1b\\");
/// let SyntaxToken::Dcs(dcs) = &tokens[0] else {
///     panic!("expected a DCS token");
/// };
///
/// let report = XtVersionReport::from_string_sequence(dcs).expect("XTVERSION report");
/// assert_eq!(report.version(), "ghostty 1.0.0");
/// ```
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct XtVersionReport {
    version: String,
}

impl XtVersionReport {
    /// Creates an XTVERSION report value from an already-extracted version string.
    #[must_use]
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
        }
    }

    /// Parses an XTVERSION report from a complete DCS string sequence.
    ///
    /// Returns `None` when the DCS payload does not start with the `>|` XTVERSION marker or the
    /// version text is not valid UTF-8.
    ///
    /// # Example
    ///
    /// ```
    /// use qwertty::report::XtVersionReport;
    /// use qwertty::{SyntaxParser, SyntaxToken};
    ///
    /// let mut parser = SyntaxParser::new();
    /// // A DCS payload without the `>|` marker is not an XTVERSION report.
    /// let tokens = parser.feed(b"\x1bP1$r0m\x1b\\");
    /// let SyntaxToken::Dcs(dcs) = &tokens[0] else {
    ///     panic!("expected a DCS token");
    /// };
    /// assert!(XtVersionReport::from_string_sequence(dcs).is_none());
    /// ```
    #[must_use]
    pub fn from_string_sequence(dcs: &StringSequence) -> Option<Self> {
        // The syntax layer parses `ESC P > | …` as a DCS whose parameter prefix is the private
        // marker `>` and final byte `|`; the version text is the payload after it. The XTVERSION
        // reply shape is therefore "a DCS with a `>` marker and `|` final and no other params."
        let params = dcs.control_params()?;
        if params.private_markers() != b">"
            || params.final_byte() != b'|'
            || !params.param_bytes().is_empty()
            || !params.intermediates().is_empty()
        {
            return None;
        }
        let version = std::str::from_utf8(dcs.payload()).ok()?;
        Some(Self::new(version))
    }

    /// Returns the terminal's self-reported version string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Which default terminal colour an OSC colour report describes.
///
/// This is the discriminator that tells an OSC 10 foreground report apart from an OSC 11 background
/// report — the two reports share the `rgb:…` payload shape and differ only in the OSC selector, so
/// the colour index is what a correlator matches on (design 03 rule 1, the OSC analogue of the
/// DECRQM mode discriminator).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum OscColorKind {
    /// The default foreground colour, OSC selector `10`.
    Foreground,
    /// The default background colour, OSC selector `11`.
    Background,
}

impl OscColorKind {
    /// Returns the OSC selector number (`10` or `11`) for this colour.
    #[must_use]
    pub const fn selector(self) -> u16 {
        match self {
            Self::Foreground => 10,
            Self::Background => 11,
        }
    }

    /// Maps an OSC selector number to a colour kind, or `None` for any other selector.
    #[must_use]
    const fn from_selector(selector: &[u8]) -> Option<Self> {
        match selector {
            b"10" => Some(Self::Foreground),
            b"11" => Some(Self::Background),
            _ => None,
        }
    }
}

/// A parsed OSC colour report: a terminal's default foreground or background colour.
///
/// A terminal sends this in answer to an OSC colour query `OSC 10 ; ? ST` (foreground) or
/// `OSC 11 ; ? ST` (background). The reply payload is `10;rgb:RRRR/GGGG/BBBB` or
/// `11;rgb:RRRR/GGGG/BBBB`, terminated by either ST (`ESC \`) or BEL — this type parses the OSC
/// *payload string*, so the terminator has already been stripped by the syntax layer (FM-P9: both
/// terminators are accepted because they never reach this parser).
///
/// # Accepted colour forms (X11 `rgb:`)
///
/// The colour is the X11 `rgb:R/G/B` form, where each channel is 1–4 hex digits. Terminals vary in
/// width: `rgb:ff/ff/ff` (8-bit), `rgb:ffff/ffff/ffff` (16-bit), and the `f/f/f` and `fff/fff/fff`
/// forms all appear. Every width is scaled to an 8-bit-per-channel [`Rgb`] by taking the most
/// significant byte of the channel (so `ffff` and `ff` both yield `0xff`, and `0` yields `0x00`).
/// Each channel must have the same number of digits is **not** required; each is scaled on its own.
///
/// # Accepted shape
///
/// - the payload begins with `10;` or `11;` (the OSC selector and its separator);
/// - the remainder is `rgb:` followed by three `/`-separated hex channels, each 1–4 hex digits.
///
/// Anything else — a missing or unknown selector, a non-`rgb:` colour spelling, the wrong channel
/// count, an empty or over-long channel, or a non-hex digit — is rejected with `None`.
///
/// # Example
///
/// ```
/// use qwertty::caps::Rgb;
/// use qwertty::report::{OscColorKind, OscColorReport};
///
/// // OSC payloads reach this parser as the bytes between the OSC introducer and its terminator.
/// let report = OscColorReport::from_osc_payload(b"11;rgb:1a1a/2b2b/3c3c").expect("colour report");
/// assert_eq!(report.kind(), OscColorKind::Background);
/// assert_eq!(report.rgb(), Rgb::new(0x1a, 0x2b, 0x3c));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OscColorReport {
    kind: OscColorKind,
    rgb: Rgb,
}

impl OscColorReport {
    /// Creates an OSC colour report value.
    #[must_use]
    pub const fn new(kind: OscColorKind, rgb: Rgb) -> Self {
        Self { kind, rgb }
    }

    /// Parses an OSC colour report from an OSC payload string (`10;rgb:…` or `11;rgb:…`).
    ///
    /// The `payload` is the bytes between the OSC introducer and its terminator — the syntax layer
    /// has already stripped the `ESC ]` and the ST/BEL terminator, so this parser is
    /// terminator-form agnostic (FM-P9). Returns `None` when the payload is not a well-formed
    /// foreground/background `rgb:` report.
    ///
    /// # Example
    ///
    /// ```
    /// use qwertty::report::OscColorReport;
    ///
    /// // A cursor-colour report (OSC 12) is not a foreground/background colour: rejected.
    /// assert!(OscColorReport::from_osc_payload(b"12;rgb:ff/ff/ff").is_none());
    /// // A malformed channel count is rejected.
    /// assert!(OscColorReport::from_osc_payload(b"10;rgb:ff/ff").is_none());
    /// ```
    #[must_use]
    pub fn from_osc_payload(payload: &[u8]) -> Option<Self> {
        let (selector, rest) = split_once(payload, b';')?;
        let kind = OscColorKind::from_selector(selector)?;
        let rgb = parse_x11_rgb(rest)?;
        Some(Self::new(kind, rgb))
    }

    /// Returns which default colour this report describes (foreground or background).
    #[must_use]
    pub const fn kind(self) -> OscColorKind {
        self.kind
    }

    /// Returns the reported colour, scaled to 8 bits per channel.
    #[must_use]
    pub const fn rgb(self) -> Rgb {
        self.rgb
    }
}

/// Splits `bytes` at the first occurrence of `sep`, returning the parts around it, or `None` when
/// `sep` is absent.
fn split_once(bytes: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|&byte| byte == sep)?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

/// Parses an X11 `rgb:R/G/B` colour into an 8-bit-per-channel [`Rgb`], or `None`.
///
/// Each channel is 1–4 hex digits and is scaled to 8 bits by taking its most significant byte, so
/// `ffff`, `ff`, and `f`-widths all map onto the `0..=255` range consistently (`ff -> 0xff`,
/// `0 -> 0x00`). Rejects a payload that is not `rgb:`-prefixed, the wrong channel count, or a
/// non-hex/empty/over-long channel.
fn parse_x11_rgb(bytes: &[u8]) -> Option<Rgb> {
    let body = bytes.strip_prefix(b"rgb:")?;
    let mut channels = body.split(|&byte| byte == b'/');
    let red = parse_hex_channel(channels.next()?)?;
    let green = parse_hex_channel(channels.next()?)?;
    let blue = parse_hex_channel(channels.next()?)?;
    if channels.next().is_some() {
        return None;
    }
    Some(Rgb::new(red, green, blue))
}

/// Parses one 1–4-digit hex channel and scales it to an 8-bit value.
///
/// X11's `rgb:` form lets a terminal report each channel at any width from 1 to 4 hex digits, so
/// qwertty normalizes by left-aligning the parsed value to 16 bits and taking the high byte: `ff`
/// and `ffff` both map to `0xff`, and `0` and `00` both map to `0x00`. Returns `None` for an empty
/// channel, more than four digits, or a non-hex digit.
fn parse_hex_channel(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut value: u32 = 0;
    for &byte in bytes {
        let digit = (byte as char).to_digit(16)?;
        value = value * 16 + digit;
    }
    // Left-align the parsed value within a 16-bit channel by its digit width, then take the high
    // byte: a 4-digit `1a1a` keeps its high byte `0x1a`; a 2-digit `ff` shifts to `0xff00` ->
    // `0xff`; a 1-digit `f` shifts to `0xf000` -> `0xf0`.
    let width_bits = u32::try_from(bytes.len()).ok()? * 4;
    let aligned = value << (16 - width_bits);
    u8::try_from(aligned >> 8).ok()
}

/// Parses a non-empty run of ASCII decimal digits into a `u16`, allowing zero.
///
/// Returns `None` for an empty field, a non-digit byte, or a value that overflows `u16`. Unlike
/// [`parse_one_based_u16`], zero is accepted: a DECRPM mode number and its state value both range
/// over `0`-inclusive.
fn parse_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u16 = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        let digit = u16::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

/// Parses a non-empty run of ASCII decimal digits into a one-based `u16`.
///
/// Returns `None` for an empty field, a non-digit byte, a value of zero, or a value that overflows
/// `u16`. This is the exact acceptance the old cursor report used, so the ported parser preserves
/// its documented edge cases (leading zeros are accepted as decimal; `0` and empty are rejected).
fn parse_one_based_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }

    let mut value: u16 = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }

        let digit = u16::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }

    (value != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxParser, SyntaxToken};

    /// Parses `bytes` through the syntax layer and returns the single CSI token it must contain.
    fn csi(bytes: &[u8]) -> ControlSequence {
        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
        match tokens.into_iter().next().expect("one token") {
            SyntaxToken::Csi(csi) => csi,
            other => panic!("expected a CSI token, got {other:?}"),
        }
    }

    #[test]
    fn cursor_report_parses_row_and_column() {
        let report = CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;34R"))
            .expect("cursor report");
        assert_eq!(report.row(), 12);
        assert_eq!(report.column(), 34);
        assert_eq!(report.position(), ProtocolPosition::new(12, 34));
    }

    #[test]
    fn cursor_report_accepts_origin() {
        let report =
            CursorPositionReport::from_control_sequence(&csi(b"\x1b[1;1R")).expect("cursor report");
        assert_eq!(report.position(), ProtocolPosition::new(1, 1));
    }

    #[test]
    fn cursor_report_accepts_leading_zeros() {
        // Ported edge case: leading zeros parse as ordinary decimal.
        let report = CursorPositionReport::from_control_sequence(&csi(b"\x1b[01;09R"))
            .expect("cursor report");
        assert_eq!(report.position(), ProtocolPosition::new(1, 9));
    }

    #[test]
    fn cursor_report_rejects_wrong_final_byte() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;34H")).is_none());
    }

    #[test]
    fn cursor_report_rejects_private_marker() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[?12;34R")).is_none());
    }

    #[test]
    fn cursor_report_rejects_intermediate() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;34 R")).is_none());
    }

    #[test]
    fn cursor_report_rejects_missing_field() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12R")).is_none());
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;R")).is_none());
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[;34R")).is_none());
    }

    #[test]
    fn cursor_report_rejects_extra_field() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;34;5R")).is_none());
    }

    #[test]
    fn cursor_report_rejects_zero_field() {
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[0;34R")).is_none());
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[12;0R")).is_none());
    }

    #[test]
    fn cursor_report_rejects_overflow() {
        // 65536 does not fit in u16.
        assert!(CursorPositionReport::from_control_sequence(&csi(b"\x1b[65536;1R")).is_none());
        // u16::MAX is accepted.
        let report = CursorPositionReport::from_control_sequence(&csi(b"\x1b[65535;1R"))
            .expect("cursor report at u16::MAX");
        assert_eq!(report.row(), u16::MAX);
    }

    #[test]
    fn status_report_parses_ready_and_malfunction() {
        assert_eq!(
            TerminalStatusReport::from_control_sequence(&csi(b"\x1b[0n"))
                .expect("ready")
                .status(),
            TerminalStatus::Ready
        );
        assert_eq!(
            TerminalStatusReport::from_control_sequence(&csi(b"\x1b[3n"))
                .expect("malfunction")
                .status(),
            TerminalStatus::Malfunction
        );
    }

    #[test]
    fn status_report_rejects_other_params() {
        assert!(TerminalStatusReport::from_control_sequence(&csi(b"\x1b[5n")).is_none());
        assert!(TerminalStatusReport::from_control_sequence(&csi(b"\x1b[00n")).is_none());
        assert!(TerminalStatusReport::from_control_sequence(&csi(b"\x1b[n")).is_none());
    }

    #[test]
    fn status_report_rejects_private_marker() {
        assert!(TerminalStatusReport::from_control_sequence(&csi(b"\x1b[?0n")).is_none());
    }

    #[test]
    fn status_report_rejects_wrong_final_byte() {
        assert!(TerminalStatusReport::from_control_sequence(&csi(b"\x1b[0R")).is_none());
    }

    #[test]
    fn status_parameter_bytes_round_trip() {
        assert_eq!(TerminalStatus::Ready.parameter_bytes(), b"0");
        assert_eq!(TerminalStatus::Malfunction.parameter_bytes(), b"3");
    }

    /// Parses `bytes` through the syntax layer and returns the single DCS token it must contain.
    fn dcs(bytes: &[u8]) -> StringSequence {
        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
        match tokens.into_iter().next().expect("one token") {
            SyntaxToken::Dcs(dcs) => dcs,
            other => panic!("expected a DCS token, got {other:?}"),
        }
    }

    // --- DECRPM (DEC private mode report) -----------------------------------------------------

    #[test]
    fn decrpm_report_parses_mode_and_all_states() {
        for (bytes, mode, state, enabled) in [
            (
                &b"\x1b[?2026;0$y"[..],
                2026,
                DecPrivateModeState::NotRecognized,
                None,
            ),
            (
                &b"\x1b[?2026;1$y"[..],
                2026,
                DecPrivateModeState::Set,
                Some(true),
            ),
            (
                &b"\x1b[?2027;2$y"[..],
                2027,
                DecPrivateModeState::Reset,
                Some(false),
            ),
            (
                &b"\x1b[?2048;3$y"[..],
                2048,
                DecPrivateModeState::PermanentlySet,
                Some(true),
            ),
            (
                &b"\x1b[?2004;4$y"[..],
                2004,
                DecPrivateModeState::PermanentlyReset,
                Some(false),
            ),
        ] {
            let report =
                DecPrivateModeReport::from_control_sequence(&csi(bytes)).expect("DECRPM report");
            assert_eq!(report.mode(), mode, "mode for {bytes:?}");
            assert_eq!(report.state(), state, "state for {bytes:?}");
            assert_eq!(report.is_enabled(), enabled, "is_enabled for {bytes:?}");
        }
    }

    #[test]
    fn decrpm_report_rejects_out_of_range_state() {
        // Value 5 is not a defined DECRPM state.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026;5$y")).is_none());
    }

    #[test]
    fn decrpm_report_rejects_non_private_and_wrong_shape() {
        // The non-private ANSI-mode form (no `?`) is a different report.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[4;2$y")).is_none());
        // Missing the `$` intermediate.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026;1y")).is_none());
        // Wrong final byte.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026;1$p")).is_none());
        // Only one parameter.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026$y")).is_none());
        // Three parameters.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026;1;2$y")).is_none());
        // Empty state field.
        assert!(DecPrivateModeReport::from_control_sequence(&csi(b"\x1b[?2026;$y")).is_none());
    }

    // --- XTVERSION -----------------------------------------------------------------------------

    #[test]
    fn xtversion_report_parses_version_text() {
        let report = XtVersionReport::from_string_sequence(&dcs(b"\x1bP>|ghostty 1.0.0\x1b\\"))
            .expect("XTVERSION report");
        assert_eq!(report.version(), "ghostty 1.0.0");
    }

    #[test]
    fn xtversion_report_accepts_empty_version() {
        let report =
            XtVersionReport::from_string_sequence(&dcs(b"\x1bP>|\x1b\\")).expect("empty version");
        assert_eq!(report.version(), "");
    }

    #[test]
    fn xtversion_report_rejects_missing_marker() {
        // A DCS payload without the `>|` XTVERSION marker (here a DECRPM-in-DCS) is not a version.
        assert!(XtVersionReport::from_string_sequence(&dcs(b"\x1bP1$r0m\x1b\\")).is_none());
    }

    // --- OSC colour report ---------------------------------------------------------------------

    #[test]
    fn osc_color_report_parses_foreground_and_background() {
        let fg = OscColorReport::from_osc_payload(b"10;rgb:1111/2222/3333").expect("fg report");
        assert_eq!(fg.kind(), OscColorKind::Foreground);
        assert_eq!(fg.rgb(), Rgb::new(0x11, 0x22, 0x33));

        let bg = OscColorReport::from_osc_payload(b"11;rgb:aaaa/bbbb/cccc").expect("bg report");
        assert_eq!(bg.kind(), OscColorKind::Background);
        assert_eq!(bg.rgb(), Rgb::new(0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn osc_color_report_accepts_channel_widths() {
        // FM-P9-adjacent: X11 rgb: accepts 1/2/4-hex-digit-per-channel forms. Every width scales to
        // 8 bits by its most significant byte.
        // 2-digit form: `ff` -> 0xff.
        assert_eq!(
            OscColorReport::from_osc_payload(b"11;rgb:ff/00/80")
                .expect("2-digit")
                .rgb(),
            Rgb::new(0xff, 0x00, 0x80)
        );
        // 4-digit form: `ffff` -> 0xff, `0000` -> 0x00.
        assert_eq!(
            OscColorReport::from_osc_payload(b"10;rgb:ffff/0000/8000")
                .expect("4-digit")
                .rgb(),
            Rgb::new(0xff, 0x00, 0x80)
        );
        // 1-digit form: `f` -> 0xf0.
        assert_eq!(
            OscColorReport::from_osc_payload(b"11;rgb:f/0/8")
                .expect("1-digit")
                .rgb(),
            Rgb::new(0xf0, 0x00, 0x80)
        );
    }

    #[test]
    fn osc_color_report_rejects_malformed() {
        // Unknown selector (OSC 12 is cursor colour, not fg/bg).
        assert!(OscColorReport::from_osc_payload(b"12;rgb:ff/ff/ff").is_none());
        // Non-`rgb:` colour spelling.
        assert!(OscColorReport::from_osc_payload(b"10;#ffffff").is_none());
        // Wrong channel count.
        assert!(OscColorReport::from_osc_payload(b"10;rgb:ff/ff").is_none());
        assert!(OscColorReport::from_osc_payload(b"10;rgb:ff/ff/ff/ff").is_none());
        // Empty channel.
        assert!(OscColorReport::from_osc_payload(b"10;rgb://").is_none());
        // Over-long channel (5 hex digits).
        assert!(OscColorReport::from_osc_payload(b"10;rgb:fffff/0/0").is_none());
        // Non-hex digit.
        assert!(OscColorReport::from_osc_payload(b"10;rgb:gg/00/00").is_none());
        // No selector separator.
        assert!(OscColorReport::from_osc_payload(b"10rgb:ff/ff/ff").is_none());
    }
}
