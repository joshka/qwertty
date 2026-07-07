//! Focus event vocabulary and decode (DEC private mode 1004).
//!
//! When focus reporting is enabled, the terminal sends `CSI I` when the window gains focus and
//! `CSI O` when it loses focus (design 02, R-IN-9). These decode to a [`FocusEvent`] carrying the
//! [`FocusState`]; the enable/disable of mode 1004 lives in the session.

use crate::syntax::ControlSequence;

/// Whether a [`FocusEvent`] reports the terminal gaining or losing focus.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum FocusState {
    /// The terminal window gained focus (`CSI I`).
    Gained,
    /// The terminal window lost focus (`CSI O`).
    Lost,
}

/// A decoded terminal focus event.
///
/// Focus events arrive only after the session enables focus reporting (mode 1004). The struct is
/// `#[non_exhaustive]`.
///
/// # Example
///
/// ```
/// use qwertty::SemanticDecoder;
/// use qwertty::event::FocusState;
///
/// let mut decoder = SemanticDecoder::new();
/// let events = decoder.feed(b"\x1b[I\x1b[O");
///
/// assert_eq!(
///     events[0].focus_event().map(|f| f.state()),
///     Some(FocusState::Gained)
/// );
/// assert_eq!(
///     events[1].focus_event().map(|f| f.state()),
///     Some(FocusState::Lost)
/// );
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct FocusEvent {
    state: FocusState,
}

impl FocusEvent {
    /// Returns whether the terminal gained or lost focus.
    #[must_use]
    pub fn state(&self) -> FocusState {
        self.state
    }
}

/// Decodes a focus report `CSI I` / `CSI O` into a [`FocusEvent`], or `None`.
///
/// Returns `None` for any sequence that is not a bare focus report: private markers, intermediates,
/// or parameters disqualify it, so a lookalike passes through as lossless syntax rather than a fake
/// focus event (design 02).
pub(crate) fn decode(csi: &ControlSequence) -> Option<FocusEvent> {
    let params = csi.params();
    if !params.private_markers().is_empty()
        || !params.intermediates().is_empty()
        || !params.param_bytes().is_empty()
    {
        return None;
    }
    let state = match params.final_byte() {
        b'I' => FocusState::Gained,
        b'O' => FocusState::Lost,
        _ => return None,
    };
    Some(FocusEvent { state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{SyntaxParser, SyntaxToken};

    fn csi(bytes: &[u8]) -> ControlSequence {
        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(bytes);
        tokens.extend(parser.finish());
        match tokens.into_iter().next().expect("one token") {
            SyntaxToken::Csi(csi) => csi,
            other => panic!("expected a CSI token, got {other:?}"),
        }
    }

    #[test]
    fn focus_gained_and_lost() {
        assert_eq!(
            decode(&csi(b"\x1b[I")).map(|f| f.state()),
            Some(FocusState::Gained)
        );
        assert_eq!(
            decode(&csi(b"\x1b[O")).map(|f| f.state()),
            Some(FocusState::Lost)
        );
    }

    #[test]
    fn parameterized_or_marked_forms_are_declined() {
        // `CSI 1 I` is a horizontal-tab control, `CSI ? I` is not a focus report.
        assert!(decode(&csi(b"\x1b[1I")).is_none());
        assert!(decode(&csi(b"\x1b[?I")).is_none());
        assert!(decode(&csi(b"\x1b[H")).is_none());
    }
}
