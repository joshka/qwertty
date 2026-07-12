//! Terminal-aware string width measurement.
//!
//! [`width_of`] answers the question every terminal layout needs — how many columns will this
//! string occupy? — using the hybrid mechanism the width design settled on (design 09-width): a
//! static `unicode-width` baseline, corrected for the small set of grapheme clusters where real
//! terminals disagree with that table (ZWJ sequences, skin-tone modifiers, regional-indicator
//! flags, VS16 emoji) by a per-terminal deviation table measured from live conformance and keyed on
//! the terminal's identity and observed mode-2027 (grapheme-clustering) state.
//!
//! The deviation table ([`crate::width_table`]) is generated from the `db/width/*.toml` conformance
//! data, so the numbers are measured, never asserted. A terminal whose identity is unknown, or a
//! cluster no terminal was measured to render oddly, falls back to the `unicode-width` baseline —
//! there is never an invented per-terminal claim.
//!
//! Mode 2027 is **observed, not enabled**: `width_of` reads the terminal's grapheme-clustering
//! state from [`Capabilities`] and picks the matching measured advance; it never changes the
//! terminal's mode.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use crate::caps::{Capabilities, Multiplexer, TerminalProgram};
use crate::width_table::{PROFILES, Profile};

/// Returns the number of terminal columns `s` occupies on the terminal described by `caps`.
///
/// The result is the sum of each grapheme cluster's width. A cluster is measured against `caps`'s
/// per-terminal deviation table when the terminal is one conformance has profiled and the cluster
/// is one it renders differently from the `unicode-width` baseline (keyed on the observed mode-2027
/// state); otherwise it is the static `unicode-width` sum. An unknown terminal uses the baseline
/// throughout.
///
/// This never performs terminal I/O and never changes terminal state — the 2027 state is read from
/// `caps`, not set.
///
/// # Examples
///
/// ```
/// use qwertty::{Capabilities, width_of};
///
/// // With no terminal identity, the static unicode-width baseline is used.
/// let caps = Capabilities::default();
/// assert_eq!(width_of("hello", &caps), 5);
/// assert_eq!(width_of("中文", &caps), 4); // two wide CJK cells each
/// ```
#[must_use]
pub fn width_of(s: &str, caps: &Capabilities) -> usize {
    let profile = profile_for(caps);
    // 2027 is honoured only when the terminal positively reported it active; unknown or off both
    // mean "use the default-state advance".
    let mode_2027 = caps.grapheme_clustering.value() == Some(&true);
    s.graphemes(true)
        .map(|cluster| cluster_width(cluster, profile, mode_2027))
        .sum()
}

/// The measured width of one grapheme cluster: the deviation-table advance when this terminal is
/// known to render the cluster off-baseline, else the `unicode-width` sum.
fn cluster_width(cluster: &str, profile: Option<&Profile>, mode_2027: bool) -> usize {
    if let Some(profile) = profile {
        if let Some(dev) = profile.deviations.iter().find(|d| d.text == cluster) {
            return if mode_2027 {
                dev.advance_2027.unwrap_or(dev.advance)
            } else {
                dev.advance
            };
        }
    }
    baseline_width(cluster)
}

/// The static `unicode-width` sum for a cluster (control chars and zero-width scalars contribute
/// 0).
fn baseline_width(cluster: &str) -> usize {
    cluster.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Selects the deviation profile for `caps`: tmux owns the rendered width whenever it is in the
/// stack (it redraws through its own width math, FM-C3), so it takes precedence over the terminal
/// underneath; otherwise the profile is the terminal program's, when one was profiled.
fn profile_for(caps: &Capabilities) -> Option<&'static Profile> {
    let under_tmux = caps.identity.program == Some(TerminalProgram::Tmux)
        || caps.identity.mux_stack.contains(&Multiplexer::Tmux);
    if under_tmux {
        return find_profile(&TerminalProgram::Tmux);
    }
    caps.identity.program.as_ref().and_then(find_profile)
}

/// Finds the profile for a program, matching by variant (the `Unknown(_)` payload never matches a
/// profiled terminal).
fn find_profile(program: &TerminalProgram) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| &p.program == program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{Finding, TerminalIdentity};

    fn caps_for(program: Option<TerminalProgram>, grapheme_2027: Option<bool>) -> Capabilities {
        Capabilities {
            grapheme_clustering: match grapheme_2027 {
                Some(v) => Finding::probed(Some(v), "DECRQM 2027"),
                None => Finding::unknown(),
            },
            identity: TerminalIdentity {
                program,
                ..TerminalIdentity::default()
            },
            ..Capabilities::default()
        }
    }

    const ZWJ_FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";

    #[test]
    fn baseline_used_for_unknown_terminal() {
        let caps = Capabilities::default();
        assert_eq!(width_of("hi", &caps), 2);
        assert_eq!(width_of("中", &caps), 2);
        // The ZWJ family sums to the unicode-width baseline (8) with no terminal profile.
        assert_eq!(width_of(ZWJ_FAMILY, &caps), 8);
        // Combining marks and zero-width contribute 0 over the base char.
        assert_eq!(width_of("e\u{0301}", &caps), 1);
        assert_eq!(width_of("a\u{200B}b", &caps), 2);
    }

    #[test]
    fn known_terminal_applies_measured_deviation() {
        // tmux collapses the ZWJ family to 2 columns; the profile overrides the baseline.
        let tmux = caps_for(Some(TerminalProgram::Tmux), None);
        assert_eq!(width_of(ZWJ_FAMILY, &tmux), 2);
        // A non-deviating cluster still uses the baseline on tmux.
        assert_eq!(width_of("中", &tmux), 2);
    }

    #[test]
    fn observed_2027_selects_the_2027_advance() {
        // ghostty renders the ZWJ family at 8 with 2027 off, 2 with 2027 on.
        let off = caps_for(Some(TerminalProgram::Ghostty), Some(false));
        let on = caps_for(Some(TerminalProgram::Ghostty), Some(true));
        assert_eq!(width_of(ZWJ_FAMILY, &off), 8);
        assert_eq!(width_of(ZWJ_FAMILY, &on), 2);
        // Unknown 2027 state is treated as off (never assume the mode is active).
        let unknown = caps_for(Some(TerminalProgram::Ghostty), None);
        assert_eq!(width_of(ZWJ_FAMILY, &unknown), 8);
    }

    #[test]
    fn tmux_in_stack_takes_precedence_over_the_inner_terminal() {
        // Under tmux, tmux's width math applies even when the inner program resolved to ghostty.
        let mut caps = caps_for(Some(TerminalProgram::Ghostty), Some(true));
        caps.identity.mux_stack = vec![Multiplexer::Tmux];
        // tmux's advance for the ZWJ family is 2, and tmux has no 2027 profile, so the on-state
        // does not pull ghostty's number.
        assert_eq!(width_of(ZWJ_FAMILY, &caps), 2);
    }
}
