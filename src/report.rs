//! Typed terminal reports parsed from the lossless syntax layer.
//!
//! A report is a host-visible reply a terminal sends in answer to a query: a cursor position
//! report, a device status report, and (in later slices) device attributes, mode reports, and
//! colour reports. Each type here parses one complete [`ControlSequence`] — the CSI token from the
//! [syntax layer](crate::SyntaxToken) — into a typed value, rejecting anything that is not exactly
//! the report shape it recognizes.
//!
//! These parsers are **pure and side-effect-free**: they read a syntax token and return a typed
//! value or `None`. They do not read a terminal, prove which request caused a report, or apply
//! timeout policy. Correlating a report to the query that provoked it is the job of the internal
//! query correlator; matching happens over these same typed parsers.
//!
//! # Relationship to the old parsers
//!
//! qwertty's first input slice shipped `CursorPositionReport` and `TerminalStatusReport` that
//! parsed from the old `CsiInput` value. Those types still exist at the crate root during the
//! transition. The types here carry the same names and the same exact acceptance and rejection
//! behavior, but parse from the new [`ControlSequence`] token instead, so they are the reports the
//! correlator and the ghostty-rs encode oracle consume. They are exported as [`report::`](self)
//! and are deliberately **not** re-exported at the crate root yet; the crate-root swap happens when
//! the old `CsiInput` path retires.
//!
//! [`report::`]: self

use crate::ProtocolPosition;
use crate::syntax::ControlSequence;

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
/// This is byte-for-byte the same acceptance the crate-root cursor report applied over the old
/// `CsiInput`, ported to the [`ControlSequence`] token.
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
/// byte, or a different final byte is rejected with `None`. This matches the crate-root status
/// report's acceptance over the old `CsiInput`, ported to the [`ControlSequence`] token.
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
}
