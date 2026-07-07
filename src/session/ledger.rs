//! Session mode ledger.
//!
//! The ledger records every reversible state change a session makes, with the actions that apply
//! and undo it. It is the single source of truth for all lifecycle paths: re-entering applies the
//! entries in enablement order, and orderly leave, drop, and the emergency restore handle all
//! undo the same entries in reverse enablement order.

use crate::DeviceMode;

/// One terminal state action a ledger entry can apply or undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StateAction {
    /// Write protocol bytes to the terminal.
    ///
    /// No production entry constructs this yet: the first byte-based ledger entries arrive with
    /// the alternate-screen and protocol-mode slices. Replay and the emergency blob already
    /// handle it so those slices only add `record` calls.
    #[allow(dead_code)]
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
    pub(crate) fn protocol_undo_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for action in self.undo_actions() {
            if let StateAction::WriteBytes(undo) = action {
                bytes.extend_from_slice(undo);
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
}
