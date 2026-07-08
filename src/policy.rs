//! Session security policy for side-effecting and exfiltrating terminal features.
//!
//! Some terminal features do more than paint the grid: they reach the system clipboard, pull data
//! back from the terminal, transfer files, raise desktop notifications, or wrap sequences for a
//! multiplexer to pass through. Those are the operations an attacker who controls a program's
//! *output* wants to reach — a log line that quietly writes the clipboard (FM-X4) is an
//! exfiltration primitive, not a formatting choice. A [`Policy`] is a plain value the session
//! carries to gate exactly these operations (R-SEC-1); gated methods consult it before emitting
//! and return a teachable [`PolicyDenied`](crate::Error::PolicyDenied) error naming the gate when
//! it says no.
//!
//! # Presets
//!
//! Every preset is an ordinary struct an app can also build field-by-field. The ladder widens from
//! safe-by-default to fully trusted:
//!
//! - [`Policy::restricted`] is the default. Clipboard **write** is on — not because writes are
//!   harmless, but because terminals themselves gate the sensitive direction: a
//!   paste-from-clipboard is what prompts or is dropped terminal-side (FM-X4, kitty#9428), so a
//!   write here still can't silently reach a user without their terminal's own consent. Everything
//!   that *reads* or *exfiltrates* — clipboard read, file transfer — stays off, as do notifications
//!   and mux passthrough.
//! - [`Policy::interactive`] widens `restricted` for a locally-trusted interactive app: it adds
//!   notifications and mux passthrough. Clipboard **read** and file transfer stay off — an
//!   interactive app being trusted to draw and notify is not the same as being trusted to read the
//!   user's clipboard or move files.
//! - [`Policy::trusted`] turns every gate on. Use it only when the program and the data it emits
//!   are both trusted, since it opens the reads (clipboard read, file transfer) that exfiltrate.

/// A session security policy gating side-effecting and exfiltrating terminal features (R-SEC-1).
///
/// Each field is a single gate an app opts into. All fields are public so a caller can build a
/// policy by hand; the [presets](#implementations) cover the common ladder. The session consults
/// the policy through [`allows`](Policy::allows) before emitting a gated command and returns
/// [`PolicyDenied`](crate::Error::PolicyDenied) when a gate is off.
///
/// See the [module documentation](self) for why each preset sits where it does on the ladder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each field is an independent, app-set opt-in gate; a flat flag struct an app builds \
              by hand is the intended API (R-SEC-1), not a state machine"
)]
pub struct Policy {
    /// Allow writing the system clipboard (OSC 52 write, FM-X4).
    ///
    /// On even in [`restricted`](Policy::restricted): the sensitive direction (paste back out) is
    /// gated by the terminal itself, so a write here cannot silently reach the user.
    pub clipboard_write: bool,
    /// Allow reading the system clipboard (OSC 52 read, FM-X4).
    ///
    /// Off until [`trusted`](Policy::trusted): a clipboard read pulls the user's clipboard into
    /// the program's input, an exfiltration surface a merely-interactive app has no need for.
    pub clipboard_read: bool,
    /// Allow raising desktop notifications (OSC 9 / OSC 777).
    ///
    /// A notification is an annoyance-and-spoof surface — attacker-controlled output can raise a
    /// misleading system toast — so it stays off in [`restricted`](Policy::restricted) and opens
    /// at [`interactive`](Policy::interactive).
    pub notifications: bool,
    /// Allow file-transfer sequences (kitty/iTerm2 file transfer, FM-X4).
    ///
    /// Off until [`trusted`](Policy::trusted): transferring files to or from the terminal is a
    /// direct exfiltration and delivery channel.
    pub file_transfer: bool,
    /// Allow wrapping sequences for a multiplexer to pass through (mux passthrough, FM-M1).
    ///
    /// Passthrough tunnels sequences past a multiplexer that would otherwise swallow them; because
    /// the wrapping differs per mux layer and can smuggle sequences through it, it stays off in
    /// [`restricted`](Policy::restricted) and opens at [`interactive`](Policy::interactive).
    pub mux_passthrough: bool,
}

impl Policy {
    /// The safe default: clipboard write on, everything else off.
    ///
    /// Clipboard write is on because the terminal gates the sensitive paste-back direction itself
    /// (FM-X4); the reads and exfiltration surfaces (clipboard read, file transfer), notifications,
    /// and mux passthrough are all off. See the [module documentation](self) for the full
    /// rationale.
    #[must_use]
    pub const fn restricted() -> Self {
        Self {
            clipboard_write: true,
            clipboard_read: false,
            notifications: false,
            file_transfer: false,
            mux_passthrough: false,
        }
    }

    /// A locally-trusted interactive profile: [`restricted`](Self::restricted) plus notifications
    /// and mux passthrough.
    ///
    /// Clipboard read and file transfer stay off — an app trusted to draw and notify is not thereby
    /// trusted to read the clipboard or move files. See the [module documentation](self).
    #[must_use]
    pub const fn interactive() -> Self {
        Self {
            notifications: true,
            mux_passthrough: true,
            ..Self::restricted()
        }
    }

    /// A fully trusted profile: every gate on.
    ///
    /// This opens the reads that exfiltrate (clipboard read, file transfer); use it only when both
    /// the program and the data it emits are trusted. See the [module documentation](self).
    #[must_use]
    pub const fn trusted() -> Self {
        Self {
            clipboard_write: true,
            clipboard_read: true,
            notifications: true,
            file_transfer: true,
            mux_passthrough: true,
        }
    }

    /// Returns whether this policy allows the operation behind `gate`.
    ///
    /// This maps each [`PolicyGate`] to its corresponding field. Gated session methods call it
    /// before emitting and return [`PolicyDenied`](crate::Error::PolicyDenied) when it is `false`.
    #[must_use]
    pub const fn allows(&self, gate: PolicyGate) -> bool {
        match gate {
            PolicyGate::ClipboardWrite => self.clipboard_write,
            PolicyGate::ClipboardRead => self.clipboard_read,
            PolicyGate::Notification => self.notifications,
            PolicyGate::FileTransfer => self.file_transfer,
            PolicyGate::MuxPassthrough => self.mux_passthrough,
        }
    }
}

impl Default for Policy {
    /// Returns [`Policy::restricted`], the safe default.
    fn default() -> Self {
        Self::restricted()
    }
}

/// A single side-effecting or exfiltrating operation a [`Policy`] gates.
///
/// This enum is `#[non_exhaustive]`: further gated operations (window title policy,
/// answer-influencing overrides) join it as the session grows more gated features. Each variant's
/// documentation cites the attack class it defends against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PolicyGate {
    /// Writing the system clipboard (OSC 52 write).
    ///
    /// Clipboard writes are an exfiltration surface (FM-X4): any emitted output, including text the
    /// program is merely displaying, can reach the clipboard. MITRE ATT&CK catalogs this as T1115.
    ClipboardWrite,
    /// Reading the system clipboard (OSC 52 read).
    ///
    /// A clipboard read pulls the user's clipboard contents into the program's input stream, a
    /// direct exfiltration of data the user never chose to hand over (FM-X4).
    ClipboardRead,
    /// Raising a desktop notification (OSC 9 / OSC 777).
    ///
    /// Notifications are an annoyance-and-spoof surface: attacker-influenced output can raise a
    /// misleading or noisy system toast the user attributes to a trusted app.
    Notification,
    /// Transferring a file to or from the terminal (kitty/iTerm2 file transfer).
    ///
    /// File transfer is both an exfiltration channel and a delivery channel for attacker-chosen
    /// bytes (FM-X4), so it opens only in the trusted profile.
    FileTransfer,
    /// Wrapping a sequence for a multiplexer to pass through (mux passthrough).
    ///
    /// Passthrough tunnels sequences past a multiplexer that would otherwise swallow them (FM-M1);
    /// the wrapping is mux-layer-specific and can smuggle sequences through, so it is gated.
    MuxPassthrough,
}

impl core::fmt::Display for PolicyGate {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self {
            Self::ClipboardWrite => "clipboard write",
            Self::ClipboardRead => "clipboard read",
            Self::Notification => "notification",
            Self::FileTransfer => "file transfer",
            Self::MuxPassthrough => "mux passthrough",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::{Policy, PolicyGate};

    /// Every gate, so preset tests can assert the full mapping exhaustively.
    const ALL_GATES: [PolicyGate; 5] = [
        PolicyGate::ClipboardWrite,
        PolicyGate::ClipboardRead,
        PolicyGate::Notification,
        PolicyGate::FileTransfer,
        PolicyGate::MuxPassthrough,
    ];

    #[test]
    fn restricted_bits() {
        let policy = Policy::restricted();
        assert!(policy.clipboard_write);
        assert!(!policy.clipboard_read);
        assert!(!policy.notifications);
        assert!(!policy.file_transfer);
        assert!(!policy.mux_passthrough);
    }

    #[test]
    fn interactive_bits() {
        let policy = Policy::interactive();
        assert!(policy.clipboard_write);
        assert!(!policy.clipboard_read);
        assert!(policy.notifications);
        assert!(!policy.file_transfer);
        assert!(policy.mux_passthrough);
    }

    #[test]
    fn trusted_bits() {
        let policy = Policy::trusted();
        assert!(policy.clipboard_write);
        assert!(policy.clipboard_read);
        assert!(policy.notifications);
        assert!(policy.file_transfer);
        assert!(policy.mux_passthrough);
    }

    #[test]
    fn default_is_restricted() {
        assert_eq!(Policy::default(), Policy::restricted());
    }

    #[test]
    fn allows_maps_every_gate_for_restricted() {
        let policy = Policy::restricted();
        for gate in ALL_GATES {
            let expected = matches!(gate, PolicyGate::ClipboardWrite);
            assert_eq!(policy.allows(gate), expected, "gate {gate:?}");
        }
    }

    #[test]
    fn allows_maps_every_gate_for_interactive() {
        let policy = Policy::interactive();
        for gate in ALL_GATES {
            let expected = matches!(
                gate,
                PolicyGate::ClipboardWrite | PolicyGate::Notification | PolicyGate::MuxPassthrough
            );
            assert_eq!(policy.allows(gate), expected, "gate {gate:?}");
        }
    }

    #[test]
    fn allows_maps_every_gate_for_trusted() {
        let policy = Policy::trusted();
        for gate in ALL_GATES {
            assert!(policy.allows(gate), "gate {gate:?}");
        }
    }

    #[test]
    fn gate_display_names_the_gate() {
        for gate in ALL_GATES {
            assert!(!gate.to_string().is_empty(), "gate {gate:?} display empty");
        }
        assert_eq!(PolicyGate::ClipboardWrite.to_string(), "clipboard write");
        assert_eq!(PolicyGate::ClipboardRead.to_string(), "clipboard read");
    }
}
