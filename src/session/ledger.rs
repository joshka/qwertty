//! Session mode ledger.
//!
//! The ledger records every reversible state change a session makes, with the actions that apply
//! and undo it. It is the single source of truth for all lifecycle paths: re-entering applies the
//! entries in enablement order, and orderly leave, drop, and the emergency restore handle all
//! undo the same entries in reverse enablement order.

use crate::DeviceMode;

/// The kitty keyboard full-reset (pop-all) sequence `CSI < u`, used in the emergency blob.
#[cfg(unix)]
pub(crate) const KITTY_KEYBOARD_EMERGENCY_RESET: &[u8] = b"\x1b[<u";

/// One terminal state action a ledger entry can apply or undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StateAction {
    /// Write protocol bytes to the terminal.
    ///
    /// The mouse, focus, and bracketed-paste mode entries are the first production byte-based
    /// entries: their apply action writes the DEC private-mode set (`CSI ? N h`) and their undo
    /// action writes the reset (`CSI ? N l`). Replay writes the apply bytes on `enter`, orderly
    /// leave writes the undo bytes in reverse, and the undo bytes flow into the emergency blob
    /// through [`ModeLedger::protocol_undo_bytes`].
    WriteBytes(Vec<u8>),
    /// Apply a device mode.
    SetMode(DeviceMode),
}

/// The kind of session state a ledger entry tracks.
///
/// Each kind is single-instance: recording a kind again replaces its actions in place, so
/// re-recording a mode does not grow the ledger or reorder cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModeKind {
    /// Raw terminal mode.
    Raw,
    /// Kitty keyboard progressive enhancement flags.
    ///
    /// The apply action pushes `CSI > flags u` (the *granted* flags, recorded after
    /// verify-after-push); the undo action pops the single pushed entry off the terminal's
    /// keyboard-flags stack with `CSI < 1 u`. This is a byte-based ledger entry, so the pop bytes
    /// also flow into the emergency blob (`protocol_undo_bytes`) — design 06: teardown pops the
    /// granted reality, not the requested intent. Design 01's panic-hook reset additionally emits
    /// the stronger `CSI < u` full pop-all as belt-and-braces, but the ordinary ledger undo is the
    /// exact one-entry pop.
    KittyKeyboard,
    /// Mouse reporting mode: a tracking mode (1000/1002/1003) always paired with SGR extended
    /// coordinates (1006).
    ///
    /// The apply action sets the chosen tracking mode and 1006 (`CSI ? N h CSI ? 1006 h`); the undo
    /// resets both (`CSI ? 1006 l CSI ? N l`). Re-recording replaces the whole entry, so switching
    /// tracking modes never leaves a stale one enabled.
    Mouse,
    /// Focus reporting mode (1004): apply sets `CSI ? 1004 h`, undo resets `CSI ? 1004 l`.
    Focus,
    /// Bracketed-paste mode (2004): apply sets `CSI ? 2004 h`, undo resets `CSI ? 2004 l`.
    BracketedPaste,
    /// In-band resize mode (2048): apply sets `CSI ? 2048 h`, undo resets `CSI ? 2048 l`.
    ///
    /// With it on, the terminal reports every size change in band as `CSI 48 ; … t`, decoded to
    /// [`ResizeEvent`](crate::ResizeEvent), so an app can avoid `SIGWINCH` entirely (design 01,
    /// R-IN-8). Like the other byte-based mode entries, its reset flows into the emergency blob.
    InBandResize,
    /// Alternate screen buffer (xterm private mode 1049).
    ///
    /// The apply action is `CSI ? 1049 h` **followed by an explicit `CSI 2 J`** clear (R-OUT-3):
    /// some hosts (mosh) do not clear the alternate buffer on entry the way most terminals do, and
    /// helix emits an explicit clear for exactly this reason, so qwertty follows that evidence
    /// rather than trusting the terminal's own 1049 behavior. The undo action is `CSI ? 1049 l`
    /// alone — leaving never needs to clear, since the terminal is switching back to the primary
    /// buffer it never touched. Like the other byte-based mode entries, the leave bytes flow into
    /// the emergency blob, so a panic teardown returns to the primary screen.
    AlternateScreen,
    /// Cursor visibility (DECTCEM, DEC private mode 25).
    ///
    /// Only *hiding* the cursor is ledger-tracked: the apply action is `CSI ? 25 l` (hide), and the
    /// undo action is `CSI ? 25 h` (show) — the ledger entry exists to guarantee the cursor is
    /// shown again on leave/drop/emergency, not to track visibility generically (FM-L3). A session
    /// that never hides the cursor never records this entry, so it never appears in the undo
    /// sequence or the emergency blob.
    CursorVisibility,
}

#[derive(Debug)]
struct LedgerEntry {
    kind: ModeKind,
    apply: StateAction,
    undo: StateAction,
}

/// Ordered record of reversible session state changes.
///
/// Entries persist across lifecycle cycles: undoing does not drain the ledger, so a later
/// re-enter can apply the same entries again.
#[derive(Debug, Default)]
pub(crate) struct ModeLedger {
    entries: Vec<LedgerEntry>,
}

impl ModeLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Records a state change with the actions that apply and undo it.
    ///
    /// Recording a kind that is already present replaces its actions without changing its
    /// position, so cleanup stays in reverse order of first enablement.
    pub(crate) fn record(&mut self, kind: ModeKind, apply: StateAction, undo: StateAction) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.kind == kind) {
            entry.apply = apply;
            entry.undo = undo;
        } else {
            self.entries.push(LedgerEntry { kind, apply, undo });
        }
    }

    /// Returns the apply actions in enablement order.
    pub(crate) fn apply_actions(&self) -> impl Iterator<Item = &StateAction> {
        self.entries.iter().map(|entry| &entry.apply)
    }

    /// Returns the undo actions in reverse enablement order.
    pub(crate) fn undo_actions(&self) -> impl Iterator<Item = &StateAction> {
        self.entries.iter().rev().map(|entry| &entry.undo)
    }

    /// Returns the protocol bytes that undo every byte-based entry, in reverse enablement order.
    ///
    /// Device-mode entries are skipped: the emergency path restores the terminal mode directly
    /// from the captured termios instead of replaying mode actions.
    ///
    /// The kitty keyboard entry is the one place the emergency form is **stronger than the ordinary
    /// pop** (design 01, FM-L2, codex practice): where the orderly `leave` undo pops the single
    /// pushed entry (`CSI < 1 u`), the emergency blob emits the full pop-all reset `CSI < u` so a
    /// panic teardown clears the terminal's whole keyboard-flags stack regardless of how deep it
    /// grew, rather than trusting the stack depth mid-panic.
    #[cfg(unix)]
    pub(crate) fn protocol_undo_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in self.entries.iter().rev() {
            match (entry.kind, &entry.undo) {
                (ModeKind::KittyKeyboard, StateAction::WriteBytes(_)) => {
                    bytes.extend_from_slice(KITTY_KEYBOARD_EMERGENCY_RESET);
                }
                (_, StateAction::WriteBytes(undo)) => bytes.extend_from_slice(undo),
                (_, StateAction::SetMode(_)) => {}
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_entry() -> (StateAction, StateAction) {
        (
            StateAction::SetMode(DeviceMode::Raw),
            StateAction::SetMode(DeviceMode::Cooked),
        )
    }

    #[test]
    fn undo_actions_run_in_reverse_enablement_order_and_persist() {
        let mut ledger = ModeLedger::new();
        let (apply, undo) = raw_entry();
        ledger.record(ModeKind::Raw, apply.clone(), undo.clone());

        assert_eq!(ledger.undo_actions().collect::<Vec<_>>(), [&undo]);
        assert_eq!(
            ledger.undo_actions().collect::<Vec<_>>(),
            [&undo],
            "undo must not drain the ledger; re-enter needs the entries"
        );
        assert_eq!(ledger.apply_actions().collect::<Vec<_>>(), [&apply]);
    }

    #[test]
    fn recording_a_kind_again_replaces_its_actions_in_place() {
        let mut ledger = ModeLedger::new();
        let (apply, undo) = raw_entry();
        ledger.record(ModeKind::Raw, apply, undo);
        ledger.record(
            ModeKind::Raw,
            StateAction::WriteBytes(b"apply".to_vec()),
            StateAction::WriteBytes(b"undo".to_vec()),
        );

        assert_eq!(
            ledger.undo_actions().collect::<Vec<_>>(),
            [&StateAction::WriteBytes(b"undo".to_vec())]
        );
    }

    #[cfg(unix)]
    #[test]
    fn emergency_blob_contains_only_protocol_bytes() {
        let mut ledger = ModeLedger::new();
        let (apply, undo) = raw_entry();
        ledger.record(ModeKind::Raw, apply, undo);

        assert_eq!(ledger.protocol_undo_bytes(), Vec::<u8>::new());

        ledger.record(
            ModeKind::Raw,
            StateAction::WriteBytes(b"\x1b[?1049h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?1049l".to_vec()),
        );

        assert_eq!(ledger.protocol_undo_bytes(), b"\x1b[?1049l");
    }

    #[cfg(unix)]
    #[test]
    fn kitty_keyboard_emergency_blob_is_the_full_pop_all_reset() {
        let mut ledger = ModeLedger::new();
        let (apply, undo) = raw_entry();
        ledger.record(ModeKind::Raw, apply, undo);
        // The ordinary undo pops the single pushed entry with `CSI < 1 u`...
        ledger.record(
            ModeKind::KittyKeyboard,
            StateAction::WriteBytes(b"\x1b[>1u".to_vec()),
            StateAction::WriteBytes(b"\x1b[<1u".to_vec()),
        );

        assert_eq!(
            ledger.undo_actions().collect::<Vec<_>>(),
            [
                &StateAction::WriteBytes(b"\x1b[<1u".to_vec()),
                &StateAction::SetMode(DeviceMode::Cooked),
            ],
            "orderly undo pops one entry",
        );
        // ...but the emergency blob is the stronger full-reset `CSI < u` (design 01, FM-L2).
        assert_eq!(
            ledger.protocol_undo_bytes(),
            b"\x1b[<u",
            "emergency blob is the pop-all reset, not the one-entry pop",
        );
    }

    #[cfg(unix)]
    #[test]
    fn mode_undo_bytes_run_in_reverse_and_feed_the_emergency_blob() {
        // Raw first, then mouse, focus, paste — the production byte-based entries.
        let mut ledger = ModeLedger::new();
        let (apply, undo) = raw_entry();
        ledger.record(ModeKind::Raw, apply, undo);
        ledger.record(
            ModeKind::Mouse,
            StateAction::WriteBytes(b"\x1b[?1000h\x1b[?1006h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?1006l\x1b[?1000l".to_vec()),
        );
        ledger.record(
            ModeKind::Focus,
            StateAction::WriteBytes(b"\x1b[?1004h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?1004l".to_vec()),
        );
        ledger.record(
            ModeKind::BracketedPaste,
            StateAction::WriteBytes(b"\x1b[?2004h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?2004l".to_vec()),
        );

        // Undo runs in reverse enablement order: paste, focus, mouse, then raw (cooked).
        assert_eq!(
            ledger.undo_actions().collect::<Vec<_>>(),
            [
                &StateAction::WriteBytes(b"\x1b[?2004l".to_vec()),
                &StateAction::WriteBytes(b"\x1b[?1004l".to_vec()),
                &StateAction::WriteBytes(b"\x1b[?1006l\x1b[?1000l".to_vec()),
                &StateAction::SetMode(DeviceMode::Cooked),
            ],
        );

        // The emergency blob concatenates the byte-based undos in the same reverse order (the raw
        // SetMode entry is skipped — the emergency path restores the mode from captured termios).
        assert_eq!(
            ledger.protocol_undo_bytes(),
            b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1000l",
        );
    }

    #[cfg(unix)]
    #[test]
    fn re_recording_mouse_replaces_the_tracking_mode_in_place() {
        // Switching from button-event (1002) to any-event (1003) mouse must not leave 1002 enabled;
        // the single-instance ModeKind replaces the whole entry, keeping cleanup exact.
        let mut ledger = ModeLedger::new();
        ledger.record(
            ModeKind::Mouse,
            StateAction::WriteBytes(b"\x1b[?1002h\x1b[?1006h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?1006l\x1b[?1002l".to_vec()),
        );
        ledger.record(
            ModeKind::Mouse,
            StateAction::WriteBytes(b"\x1b[?1003h\x1b[?1006h".to_vec()),
            StateAction::WriteBytes(b"\x1b[?1006l\x1b[?1003l".to_vec()),
        );

        assert_eq!(
            ledger.protocol_undo_bytes(),
            b"\x1b[?1006l\x1b[?1003l",
            "only the latest tracking mode is undone",
        );
    }
}
