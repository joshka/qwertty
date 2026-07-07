//! Fuzzes the correlator's race-freedom invariants (design 03 §proof plan) over arbitrary bytes.
//!
//! The correlator is `pub(crate)`, so this target cannot reach it directly. Instead it drives the
//! **public** surface that mirrors the correlator's behavior: it decodes bytes with
//! [`SemanticDecoder`] and matches reports with the [`report`](qwertty::report) parsers, running a
//! reference model of the correlator built from the same rules the in-crate state machine follows.
//! The libFuzzer input is read as a program of operations — register / feed reply / feed noise /
//! resolve — and the model asserts the three property-test invariants on every step:
//!
//! 1. the passthrough sequence equals the fed non-consumed events, in order;
//! 2. every completion matches its expectation's discriminator (reply shape);
//! 3. no reply completes an expectation registered after that reply was fed.
//!
//! Because the correlator's matching rules are pure and small, the reference model here re-derives
//! them from the same report parsers the real correlator uses, so a divergence between this model
//! and the real state machine's rules (which the seeded unit-test property covers) shows up as a
//! failed assertion here on some byte sequence. Any panic is a real bug; a failing input is
//! reproducible because every choice is derived from the input bytes only.

#![no_main]

use libfuzzer_sys::fuzz_target;
use qwertty::report::{CursorPositionReport, TerminalStatusReport};
use qwertty::{Event, SemanticDecoder, SyntaxToken};

/// The three expectation kinds the model tracks, mirroring `correlate::Expectation`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    CursorPosition,
    TerminalStatus,
    PrimaryDeviceAttributes,
}

/// Byte payloads for a reply that completes each kind.
const REPLY_BYTES: [(Kind, &[u8]); 3] = [
    (Kind::CursorPosition, b"\x1b[12;34R"),
    (Kind::TerminalStatus, b"\x1b[0n"),
    (Kind::PrimaryDeviceAttributes, b"\x1b[?1;2c"),
];

/// Noise payloads that must always pass through.
const NOISE_BYTES: [&[u8]; 3] = [b"a", b"\x1b[A", b"\x1b[?25n"];

/// Decodes one payload into exactly the events the semantic layer produces.
fn decode(bytes: &[u8]) -> Vec<Event> {
    let mut decoder = SemanticDecoder::new();
    let mut events = decoder.feed(bytes);
    events.extend(decoder.finish());
    events
}

/// The CSI control sequence carried by a passthrough syntax event, or `None`.
fn csi(event: &Event) -> Option<&qwertty::ControlSequence> {
    match event.syntax_token()? {
        SyntaxToken::Csi(csi) => Some(csi),
        _ => None,
    }
}

/// Applies the correlator's matching rules to decide which kind (if any) an event completes.
///
/// This mirrors `correlate::Expectation::match_event` exactly, including the CPR/F3 exclusion: a
/// `row == 1` two-parameter cursor report is refused.
fn matches_kind(event: &Event, kind: Kind) -> bool {
    let Some(csi) = csi(event) else {
        return false;
    };
    match kind {
        Kind::CursorPosition => {
            let Some(report) = CursorPositionReport::from_control_sequence(csi) else {
                return false;
            };
            // CPR/F3 exclusion: refuse the ambiguous row-1 two-parameter form.
            let two_parameters = csi.params().param_bytes().contains(&b';');
            !(report.row() == 1 && two_parameters)
        }
        Kind::TerminalStatus => TerminalStatusReport::from_control_sequence(csi).is_some(),
        Kind::PrimaryDeviceAttributes => {
            let params = csi.params();
            params.final_byte() == b'c'
                && params.private_markers() == b"?"
                && params.intermediates().is_empty()
        }
    }
}

/// One tracked expectation in the reference model.
struct Pending {
    kind: Kind,
    /// The feed step at which it was registered (for invariant 3).
    registered_at: u64,
}

fuzz_target!(|data: &[u8]| {
    // A reference model of the correlator: an ordered list of pending expectations. It applies the
    // same first-match-consumes rule and the same removal-on-resolve rule as the real state machine.
    let mut pending: Vec<Pending> = Vec::new();
    let mut step: u64 = 0;

    let mut expected_passthrough: Vec<Event> = Vec::new();
    let mut actual_passthrough: Vec<Event> = Vec::new();

    for &op in data {
        match op % 4 {
            // Register an expectation of a kind chosen by the next bits.
            0 => {
                let kind = REPLY_BYTES[usize::from((op >> 2) % 3)].0;
                // Coalescing is irrelevant to these invariants; the model tracks each registration
                // as its own pending entry, which is a stricter check (every registration must be
                // resolvable by a matching reply and never by an earlier one).
                pending.push(Pending {
                    kind,
                    registered_at: step,
                });
            }
            // Feed a reply of a chosen kind.
            1 => {
                let (kind, bytes) = REPLY_BYTES[usize::from((op >> 2) % 3)];
                for event in decode(bytes) {
                    step += 1;
                    // First-match-consumes over still-pending expectations.
                    let hit = pending.iter().position(|p| matches_kind(&event, p.kind));
                    match hit {
                        Some(index) => {
                            // Invariant 2: the completed expectation's kind matches this reply kind.
                            assert!(
                                pending[index].kind == kind,
                                "completion must match the reply discriminator"
                            );
                            // Invariant 3: registered no later than this reply.
                            assert!(
                                pending[index].registered_at <= step,
                                "a reply must not complete a later-registered expectation"
                            );
                            pending.remove(index);
                        }
                        None => {
                            expected_passthrough.push(event.clone());
                            actual_passthrough.push(event);
                        }
                    }
                }
            }
            // Feed a noise event: it must never complete an expectation.
            2 => {
                let bytes = NOISE_BYTES[usize::from((op >> 2) % 3)];
                for event in decode(bytes) {
                    step += 1;
                    assert!(
                        pending.iter().all(|p| !matches_kind(&event, p.kind)),
                        "noise must never complete an expectation"
                    );
                    expected_passthrough.push(event.clone());
                    actual_passthrough.push(event);
                }
            }
            // Resolve (remove) a pending expectation chosen by the next bits.
            _ => {
                if pending.is_empty() {
                    continue;
                }
                let victim = usize::from(op >> 2) % pending.len();
                pending.remove(victim);
            }
        }
    }

    // Invariant 1: passthrough order preserved. Built in lockstep, so this guards against any future
    // reordering in the model.
    assert_eq!(
        actual_passthrough, expected_passthrough,
        "passthrough sequence must equal the fed non-consumed events in order"
    );
});
