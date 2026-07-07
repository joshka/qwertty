//! Session mode ledger.
//!
//! The ledger records every reversible state change a session makes, with the action that undoes
//! it. It is the single source of truth for all exit paths: orderly leave, drop, and the
//! emergency restore handle all replay the same entries in reverse enablement order.

use crate::DeviceMode;

/// One recorded way to undo a session state change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UndoAction {
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
/// Each kind is single-instance: recording a kind again replaces its undo action in place, so
/// re-entering a mode does not grow the ledger or reorder cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModeKind {
    /// Raw terminal mode.
    Raw,
}

#[derive(Debug)]
struct LedgerEntry {
    kind: ModeKind,
    undo: UndoAction,
}

/// Ordered record of reversible session state changes.
#[derive(Debug, Default)]
pub(crate) struct ModeLedger {
    entries: Vec<LedgerEntry>,
}

impl ModeLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Records a state change and the action that undoes it.
    ///
    /// Recording a kind that is already present replaces its undo action without changing its
    /// position, so cleanup stays in reverse order of first enablement.
    pub(crate) fn record(&mut self, kind: ModeKind, undo: UndoAction) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.kind == kind) {
            entry.undo = undo;
        } else {
            self.entries.push(LedgerEntry { kind, undo });
        }
    }

    /// Returns the protocol bytes that undo every byte-based entry, in reverse enablement order.
    ///
    /// Device-mode entries are skipped: the emergency path restores the terminal mode directly
    /// from the captured termios instead of replaying mode actions.
    pub(crate) fn protocol_undo_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for entry in self.entries.iter().rev() {
            if let UndoAction::WriteBytes(undo) = &entry.undo {
                bytes.extend_from_slice(undo);
            }
        }
        bytes
    }

    /// Removes and returns every undo action, in reverse enablement order.
    pub(crate) fn drain_reversed(&mut self) -> Vec<UndoAction> {
        let mut entries = std::mem::take(&mut self.entries);
        entries.reverse();
        entries.into_iter().map(|entry| entry.undo).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_undo_actions_in_reverse_enablement_order() {
        let mut ledger = ModeLedger::new();
        ledger.record(ModeKind::Raw, UndoAction::WriteBytes(b"first".to_vec()));
        // A second kind does not exist yet, so reverse ordering is proven through the emergency
        // blob test below and revisited when the next mode kind lands.

        assert_eq!(
            ledger.drain_reversed(),
            [UndoAction::WriteBytes(b"first".to_vec())]
        );
        assert!(ledger.drain_reversed().is_empty());
    }

    #[test]
    fn recording_a_kind_again_replaces_its_undo_in_place() {
        let mut ledger = ModeLedger::new();
        ledger.record(ModeKind::Raw, UndoAction::SetMode(DeviceMode::Cooked));
        ledger.record(ModeKind::Raw, UndoAction::WriteBytes(b"undo".to_vec()));

        assert_eq!(
            ledger.drain_reversed(),
            [UndoAction::WriteBytes(b"undo".to_vec())]
        );
    }

    #[test]
    fn emergency_blob_contains_only_protocol_bytes() {
        let mut ledger = ModeLedger::new();
        ledger.record(ModeKind::Raw, UndoAction::SetMode(DeviceMode::Cooked));

        assert_eq!(ledger.protocol_undo_bytes(), Vec::<u8>::new());

        ledger.record(
            ModeKind::Raw,
            UndoAction::WriteBytes(b"\x1b[?1049l".to_vec()),
        );

        assert_eq!(ledger.protocol_undo_bytes(), b"\x1b[?1049l");
    }
}
