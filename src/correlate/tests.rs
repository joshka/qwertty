//! Unit tests for the sans-io correlator.
//!
//! These cover the salvage-derived contract set (coalescing, shared-result fan-out,
//! cancel-one-waiter, late-reply-passthrough, unmatched-stays-visible, state-freed-after-read), the
//! FM-derived fixtures (stale CPR, wrong-type, two-replies-one-batch, interleaved keystrokes, the
//! CPR/F3 exclusion), the `distinguishes` matrix, and a seeded property test over random
//! interleavings. The correlator is `pub(crate)`, so its tests live in-module (repo precedent for
//! `pub(crate)` reachability).

use super::*;
use crate::ProtocolPosition;
use crate::event::{Key, KeyEvent, SemanticDecoder};
use crate::report::TerminalStatus;
use crate::syntax::SyntaxParser;

/// Decodes `bytes` to exactly one semantic event, panicking if it is not exactly one.
fn one_event(bytes: &[u8]) -> Event {
    let mut decoder = SemanticDecoder::new();
    let mut events = decoder.feed(bytes);
    events.extend(decoder.finish());
    assert_eq!(events.len(), 1, "expected one event from {bytes:?}");
    events.into_iter().next().expect("one event")
}

/// Decodes `bytes` to a batch of semantic events (any count).
fn events(bytes: &[u8]) -> Vec<Event> {
    let mut decoder = SemanticDecoder::new();
    let mut out = decoder.feed(bytes);
    out.extend(decoder.finish());
    out
}

/// Builds a bare CSI passthrough event directly from bytes, bypassing the semantic key mapping.
///
/// Used where a test wants a specific CSI shape as a passthrough event regardless of whether the
/// semantic layer would map it to a key.
fn csi_event(bytes: &[u8]) -> Event {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(bytes);
    tokens.extend(parser.finish());
    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    Event::Syntax(tokens.into_iter().next().expect("one token"))
}

// --- distinguishes matrix -------------------------------------------------------------------

#[test]
fn distinguishes_is_false_on_identical_pairs() {
    for expectation in [
        Expectation::CursorPosition,
        Expectation::TerminalStatus,
        Expectation::PrimaryDeviceAttributes,
    ] {
        assert!(
            !distinguishes(&expectation, &expectation),
            "{expectation:?} must not distinguish from itself (that is the coalescing case)"
        );
    }
}

#[test]
fn distinguishes_is_true_on_every_different_pair() {
    let variants = [
        Expectation::CursorPosition,
        Expectation::TerminalStatus,
        Expectation::PrimaryDeviceAttributes,
    ];
    for (i, a) in variants.iter().enumerate() {
        for (j, b) in variants.iter().enumerate() {
            if i != j {
                assert!(
                    distinguishes(a, b),
                    "{a:?} and {b:?} have disjoint reply shapes and must distinguish"
                );
            }
        }
    }
}

#[test]
fn distinguishes_is_symmetric() {
    let variants = [
        Expectation::CursorPosition,
        Expectation::TerminalStatus,
        Expectation::PrimaryDeviceAttributes,
    ];
    for a in &variants {
        for b in &variants {
            assert_eq!(
                distinguishes(a, b),
                distinguishes(b, a),
                "distinguishes must be symmetric for {a:?}, {b:?}"
            );
        }
    }
}

#[test]
fn m2_non_distinguishing_pairs_are_all_identical() {
    // The register-reject path (`RegisterError::Ambiguous`) fires only for a non-identical,
    // non-distinguishing pair. For the M2 variants no such pair exists — every non-distinguishing
    // pair is identical — so this invariant documents *why* register never rejects today, and why
    // the reject branch first becomes reachable when M3 adds a discriminator-carrying variant.
    let variants = [
        Expectation::CursorPosition,
        Expectation::TerminalStatus,
        Expectation::PrimaryDeviceAttributes,
    ];
    for a in &variants {
        for b in &variants {
            if !distinguishes(a, b) {
                assert_eq!(a, b, "a non-distinguishing M2 pair must be identical");
            }
        }
    }
}

// --- basic matching -------------------------------------------------------------------------

#[test]
fn cursor_reply_completes_cursor_expectation() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");

    let feed = correlator.feed(one_event(b"\x1b[12;34R"));
    let Feed::Completed { id: got, reply } = feed else {
        panic!("expected completion, got {feed:?}");
    };
    assert_eq!(got, id);
    assert_eq!(
        reply,
        Reply::CursorPosition(CursorPositionReport::new(ProtocolPosition::new(12, 34)))
    );

    let taken = correlator.take_reply(id).expect("take reply");
    assert_eq!(taken, reply);
    assert!(
        correlator.is_empty(),
        "slot freed after the only waiter read"
    );
}

#[test]
fn status_reply_completes_status_expectation() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::TerminalStatus)
        .expect("register");

    let feed = correlator.feed(one_event(b"\x1b[0n"));
    assert!(matches!(feed, Feed::Completed { .. }));
    let Some(Reply::TerminalStatus(report)) = correlator.take_reply(id) else {
        panic!("expected terminal status reply");
    };
    assert_eq!(report.status(), TerminalStatus::Ready);
}

#[test]
fn da1_reply_completes_fence_shape_tolerantly() {
    // FM-C4: any parameter shape must complete the fence, including a widened list.
    for bytes in [
        &b"\x1b[?1;2c"[..],
        &b"\x1b[?1;2;4c"[..],
        &b"\x1b[?62;1;6;9c"[..],
        &b"\x1b[?c"[..],
    ] {
        let mut correlator = Correlator::new();
        let id = correlator
            .register(Expectation::PrimaryDeviceAttributes)
            .expect("register");
        let feed = correlator.feed(csi_event(bytes));
        assert!(
            matches!(feed, Feed::Completed { .. }),
            "DA1 shape {bytes:?} must complete the fence"
        );
        let Some(Reply::PrimaryDeviceAttributes(attrs)) = correlator.take_reply(id) else {
            panic!("expected DA1 reply for {bytes:?}");
        };
        // The `?` marker and final `c` are excluded from the preserved params.
        let expected = &bytes[3..bytes.len() - 1];
        assert_eq!(attrs.params(), expected, "DA1 params for {bytes:?}");
    }
}

#[test]
fn da1_matcher_rejects_non_private_da_and_wrong_final() {
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::PrimaryDeviceAttributes)
        .expect("register");
    // No `?` private marker (this is a DA3-ish/plain form): not the fence shape.
    assert!(matches!(
        correlator.feed(csi_event(b"\x1b[1;2c")),
        Feed::Passthrough(_)
    ));
    // Wrong final byte.
    assert!(matches!(
        correlator.feed(csi_event(b"\x1b[?1;2R")),
        Feed::Passthrough(_)
    ));
}

// --- CPR / F3 exclusion (design 03 rule 2) --------------------------------------------------

#[test]
fn cpr_matcher_rejects_ambiguous_modified_f3_form() {
    // `CSI 1;2R` is the modified-F3 collision (row == 1, two params): the CPR matcher must NOT
    // complete a CursorPosition expectation; it passes through so the app can read the key.
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    let event = csi_event(b"\x1b[1;2R");
    let feed = correlator.feed(event.clone());
    assert_eq!(
        feed,
        Feed::Passthrough(event),
        "ambiguous CSI 1;2R must pass through, not complete CPR"
    );
}

#[test]
fn cpr_matcher_accepts_unambiguous_row_one_is_impossible_but_higher_rows_match() {
    // Any row greater than 1 with two params is an unambiguous CPR and matches.
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    let feed = correlator.feed(one_event(b"\x1b[2;1R"));
    assert!(
        matches!(feed, Feed::Completed { .. }),
        "row>1 CPR is unambiguous and must complete"
    );
    let Some(Reply::CursorPosition(report)) = correlator.take_reply(id) else {
        panic!("expected cursor reply");
    };
    assert_eq!(report.position(), ProtocolPosition::new(2, 1));
}

// --- coalescing + shared result fan-out (salvage) -------------------------------------------

#[test]
fn identical_registration_coalesces_and_bumps_waiters() {
    let mut correlator = Correlator::new();
    let first = correlator
        .register(Expectation::CursorPosition)
        .expect("first");
    let second = correlator
        .register(Expectation::CursorPosition)
        .expect("second (coalesced)");
    assert_eq!(first, second, "identical registration returns the same id");
    assert_eq!(correlator.waiters(first), Some(2));
    assert_eq!(correlator.len(), 1, "only one slot for two waiters");
}

#[test]
fn shared_result_fans_out_to_every_waiter() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("first");
    correlator
        .register(Expectation::CursorPosition)
        .expect("second");
    correlator
        .register(Expectation::CursorPosition)
        .expect("third");
    assert_eq!(correlator.waiters(id), Some(3));

    // One reply completes the coalesced expectation.
    assert!(matches!(
        correlator.feed(one_event(b"\x1b[7;8R")),
        Feed::Completed { .. }
    ));

    // Each of the three waiters takes the same reply; the slot survives until the last read.
    let expected = Reply::CursorPosition(CursorPositionReport::new(ProtocolPosition::new(7, 8)));
    assert_eq!(correlator.take_reply(id), Some(expected.clone()));
    assert!(correlator.contains(id), "held for remaining waiters");
    assert_eq!(correlator.take_reply(id), Some(expected.clone()));
    assert!(correlator.contains(id), "held for the last waiter");
    assert_eq!(correlator.take_reply(id), Some(expected));
    assert!(
        !correlator.contains(id),
        "state freed only after all waiters read"
    );
    // A further take is a no-op.
    assert_eq!(correlator.take_reply(id), None);
}

// --- cancel-one-waiter keeps others (salvage) -----------------------------------------------

#[test]
fn cancel_one_waiter_keeps_the_others() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("first");
    correlator
        .register(Expectation::CursorPosition)
        .expect("second");
    assert_eq!(correlator.waiters(id), Some(2));

    // One waiter cancels; the expectation stays pending for the other.
    let resolved = correlator.resolve(id, Resolution::Cancelled);
    assert_eq!(resolved, Resolved::WaiterRemoved { remaining: 1 });
    assert_eq!(correlator.waiters(id), Some(1));

    // The reply still completes it for the remaining waiter.
    assert!(matches!(
        correlator.feed(one_event(b"\x1b[3;4R")),
        Feed::Completed { .. }
    ));
    assert!(correlator.take_reply(id).is_some());
    assert!(correlator.is_empty());
}

#[test]
fn last_waiter_resolution_removes_the_expectation() {
    for resolution in [Resolution::Timeout, Resolution::Eof, Resolution::Cancelled] {
        let mut correlator = Correlator::new();
        let id = correlator
            .register(Expectation::TerminalStatus)
            .expect("register");
        assert_eq!(correlator.resolve(id, resolution), Resolved::Removed);
        assert!(
            !correlator.contains(id),
            "removed after last waiter for {resolution:?}"
        );
    }
}

// --- late reply never completes a resolved query (rule 4, salvage) --------------------------

#[test]
fn timed_out_query_never_consumes_a_late_reply() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    assert_eq!(
        correlator.resolve(id, Resolution::Timeout),
        Resolved::Removed
    );

    // The late reply arrives after the expectation is gone: passthrough, not completion.
    let event = one_event(b"\x1b[12;34R");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

#[test]
fn cancelled_query_never_consumes_a_late_reply() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::TerminalStatus)
        .expect("register");
    assert_eq!(
        correlator.resolve(id, Resolution::Cancelled),
        Resolved::Removed
    );
    let event = one_event(b"\x1b[0n");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

// --- completed-but-unread interaction with resolve ------------------------------------------

#[test]
fn resolving_a_completed_expectation_decrements_unread_without_reopening() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("first");
    correlator
        .register(Expectation::CursorPosition)
        .expect("second");

    // Complete it; both waiters now have an unread held result.
    assert!(matches!(
        correlator.feed(one_event(b"\x1b[5;6R")),
        Feed::Completed { .. }
    ));
    assert!(correlator.is_completed(id));

    // One waiter cancels before reading: it decrements the unread count, does not reopen matching.
    assert_eq!(
        correlator.resolve(id, Resolution::Cancelled),
        Resolved::AlreadyCompleted { unread: 1 }
    );
    assert!(correlator.is_completed(id), "still held for the reader");

    // A same-shaped second reply does not re-complete a held expectation: it passes through.
    let event = one_event(b"\x1b[9;9R");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));

    // The remaining waiter reads and frees the slot.
    assert!(correlator.take_reply(id).is_some());
    assert!(correlator.is_empty());
}

#[test]
fn resolving_last_unread_completed_waiter_frees_state() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    assert!(matches!(
        correlator.feed(one_event(b"\x1b[5;6R")),
        Feed::Completed { .. }
    ));
    assert_eq!(
        correlator.resolve(id, Resolution::Timeout),
        Resolved::AlreadyCompleted { unread: 0 }
    );
    assert!(!correlator.contains(id));
}

#[test]
fn resolving_unknown_id_is_a_noop() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    correlator.resolve(id, Resolution::Timeout);
    assert_eq!(
        correlator.resolve(id, Resolution::Timeout),
        Resolved::Unknown
    );
}

// --- unmatched / wrong-type reports stay visible (rule 4/5, salvage, FM-Q10) ----------------

#[test]
fn wrong_type_report_with_pending_other_type_passes_through() {
    // FM-Q10 shape: a status reply arrives while a cursor query is pending. It must pass through,
    // not complete the cursor expectation, and the cursor expectation stays pending.
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    let event = one_event(b"\x1b[0n");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
    assert!(correlator.contains(id), "cursor expectation still pending");

    // The right reply still completes it afterward.
    assert!(matches!(
        correlator.feed(one_event(b"\x1b[1;2R").clone()),
        Feed::Passthrough(_) | Feed::Completed { .. }
    ));
}

#[test]
fn unmatched_query_shaped_csi_passes_through_with_pending_expectation() {
    // An unrelated query-shaped CSI (`CSI ? 25 n`) is never swallowed by a pending cursor query.
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::CursorPosition)
        .expect("register");
    let event = csi_event(b"\x1b[?25n");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

#[test]
fn stale_unsolicited_cpr_with_nothing_pending_passes_through() {
    // FM-Q3: an unsolicited CPR while nothing is pending is ordinary input.
    let mut correlator = Correlator::new();
    let event = one_event(b"\x1b[12;34R");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
    assert!(correlator.is_empty());
}

// --- interleaved keystrokes pass through in order (rule 5, FM-Q1/Q6) ------------------------

#[test]
fn interleaved_keystrokes_during_pending_query_pass_through_in_order() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::CursorPosition)
        .expect("register");

    // Typeahead "ab", then the reply, then more typeahead "c" — all in one batch.
    let batch = events(b"ab\x1b[12;34Rc");
    let feeds = correlator.feed_batch(batch);

    // The reply is the only completion; the three key events pass through in arrival order.
    let passthrough_keys: Vec<Key> = feeds
        .iter()
        .filter_map(|feed| match feed {
            Feed::Passthrough(event) => event.key_event().map(KeyEvent::key),
            Feed::Completed { .. } => None,
        })
        .collect();
    assert_eq!(
        passthrough_keys,
        vec![Key::Char('a'), Key::Char('b'), Key::Char('c')],
        "keystrokes preserved in arrival order around the reply"
    );
    let completions = feeds
        .iter()
        .filter(|feed| matches!(feed, Feed::Completed { .. }))
        .count();
    assert_eq!(completions, 1, "exactly one completion in the batch");
    assert!(correlator.take_reply(id).is_some());
}

// --- two replies in one batch both land (FM-Q7) ---------------------------------------------

#[test]
fn two_replies_in_one_batch_both_complete() {
    // FM-Q7: a DA1 fence and a slower CPR arriving in the same read() must both land. Order in the
    // buffer here is CPR then DA1 (the slower reply sits behind the fence in the same buffer).
    let mut correlator = Correlator::new();
    let cursor = correlator
        .register(Expectation::CursorPosition)
        .expect("cursor");
    let fence = correlator
        .register(Expectation::PrimaryDeviceAttributes)
        .expect("fence");

    let batch = events(b"\x1b[12;34R\x1b[?1;2c");
    let feeds = correlator.feed_batch(batch);

    let completed: Vec<ExpectationId> = feeds
        .iter()
        .filter_map(|feed| match feed {
            Feed::Completed { id, .. } => Some(*id),
            Feed::Passthrough(_) => None,
        })
        .collect();
    assert_eq!(
        completed,
        vec![cursor, fence],
        "both replies in the batch complete their expectations, in arrival order"
    );
    assert!(correlator.take_reply(cursor).is_some());
    assert!(correlator.take_reply(fence).is_some());
    assert!(correlator.is_empty());
}

#[test]
fn feed_batch_does_not_autoresolve_other_expectations_on_da1() {
    // The correlator must NOT treat a DA1 completion as a signal to resolve the other pending
    // expectation (that is the M3 probe layer's job). Here only DA1 replies; the cursor query is
    // untouched and stays pending.
    let mut correlator = Correlator::new();
    let cursor = correlator
        .register(Expectation::CursorPosition)
        .expect("cursor");
    let fence = correlator
        .register(Expectation::PrimaryDeviceAttributes)
        .expect("fence");

    let feeds = correlator.feed_batch(events(b"\x1b[?1;2c"));
    assert_eq!(feeds.len(), 1);
    assert!(matches!(feeds[0], Feed::Completed { .. }));
    assert!(
        correlator.contains(cursor),
        "cursor query untouched by the DA1 completion"
    );
    assert!(correlator.is_completed(fence));
}

// --- property test: random interleavings (seeded xorshift, no new deps) ---------------------

/// A tiny deterministic xorshift64 PRNG so the property test is reproducible with no dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        // Avoid the zero fixed point.
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: u32) -> u32 {
        u32::try_from(self.next_u64() % u64::from(bound)).expect("bound fits in u32")
    }
}

/// One reply shape and the expectation it can complete, used to plant matching replies.
#[derive(Clone, Copy)]
struct ReplyKind {
    expectation: Expectation,
    bytes: &'static [u8],
}

const REPLY_KINDS: [ReplyKind; 3] = [
    ReplyKind {
        expectation: Expectation::CursorPosition,
        bytes: b"\x1b[12;34R",
    },
    ReplyKind {
        expectation: Expectation::TerminalStatus,
        bytes: b"\x1b[0n",
    },
    ReplyKind {
        expectation: Expectation::PrimaryDeviceAttributes,
        bytes: b"\x1b[?1;2c",
    },
];

/// Non-reply "noise" events that must always pass through.
const NOISE: [&[u8]; 3] = [b"a", b"\x1b[A", b"\x1b[?25n"];

#[test]
fn property_random_interleavings_preserve_invariants() {
    // Assertions (design 03 §proof plan part 1):
    //  1. passthrough sequence == the fed non-consumed events, in order;
    //  2. every completion's reply matches its expectation's discriminator;
    //  3. no reply ever completes an expectation registered *after* that reply was fed.
    for seed in 0..200u64 {
        let mut rng = XorShift64::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1));
        let mut correlator = Correlator::new();

        // Track, per expectation kind, the ids currently pending and, for each, the feed-step at
        // which it was registered. A reply fed at step S may only complete an expectation whose
        // registration step is <= S (assertion 3).
        let mut pending: Vec<(ReplyKind, ExpectationId, u64)> = Vec::new();
        let mut step: u64 = 0;

        // Expected passthrough events, in order (assertion 1).
        let mut expected_passthrough: Vec<Event> = Vec::new();
        let mut actual_passthrough: Vec<Event> = Vec::new();

        let operations = 40 + rng.below(40);
        for _ in 0..operations {
            match rng.below(4) {
                // Register an expectation of a random kind.
                0 => {
                    let kind = REPLY_KINDS[rng.below(3) as usize];
                    if let Ok(id) = correlator.register(kind.expectation) {
                        pending.push((kind, id, step));
                    }
                }
                // Feed a reply of a random kind.
                1 => {
                    let kind = REPLY_KINDS[rng.below(3) as usize];
                    let event = one_event(kind.bytes);
                    step += 1;
                    let feed = correlator.feed(event.clone());
                    match feed {
                        Feed::Completed { id, reply } => {
                            // Assertion 2: the completed expectation matches this reply kind.
                            let entry = pending
                                .iter()
                                .position(|(_, pid, _)| *pid == id)
                                .expect("completed id must be pending");
                            let (matched_kind, _, reg_step) = pending[entry];
                            assert_eq!(
                                matched_kind.expectation, kind.expectation,
                                "completion must match the reply's discriminator"
                            );
                            assert!(
                                reply_matches_kind(&reply, kind.expectation),
                                "reply payload must match the expectation kind"
                            );
                            // Assertion 3: the expectation was registered before this reply.
                            assert!(
                                reg_step <= step,
                                "a reply must not complete a later-registered expectation"
                            );
                            // Drain the (single-waiter) reply and drop the pending entry.
                            correlator.take_reply(id).expect("take completed reply");
                            pending.remove(entry);
                        }
                        Feed::Passthrough(event) => {
                            expected_passthrough.push(event.clone());
                            actual_passthrough.push(event);
                        }
                    }
                }
                // Feed a noise event: it must always pass through.
                2 => {
                    let bytes = NOISE[rng.below(3) as usize];
                    let event = one_event(bytes);
                    step += 1;
                    let feed = correlator.feed(event.clone());
                    match feed {
                        Feed::Passthrough(passed) => {
                            assert_eq!(passed, event, "noise must pass through unchanged");
                            expected_passthrough.push(event.clone());
                            actual_passthrough.push(event);
                        }
                        Feed::Completed { .. } => {
                            panic!("noise event {bytes:?} must never complete an expectation")
                        }
                    }
                }
                // Resolve a random pending expectation.
                _ => {
                    if pending.is_empty() {
                        continue;
                    }
                    let victim =
                        rng.below(u32::try_from(pending.len()).expect("len fits")) as usize;
                    let (_, id, _) = pending[victim];
                    let resolution = match rng.below(3) {
                        0 => Resolution::Timeout,
                        1 => Resolution::Eof,
                        _ => Resolution::Cancelled,
                    };
                    correlator.resolve(id, resolution);
                    pending.remove(victim);
                }
            }
        }

        // Assertion 1: the two passthrough sequences are identical and in order. (They are built in
        // lockstep here; the assertion guards against a future refactor reordering them.)
        assert_eq!(
            actual_passthrough, expected_passthrough,
            "passthrough sequence must equal the fed non-consumed events in order (seed {seed})"
        );
    }
}

/// Returns `true` when a reply payload belongs to the given expectation kind.
fn reply_matches_kind(reply: &Reply, expectation: Expectation) -> bool {
    matches!(
        (reply, expectation),
        (Reply::CursorPosition(_), Expectation::CursorPosition)
            | (Reply::TerminalStatus(_), Expectation::TerminalStatus)
            | (
                Reply::PrimaryDeviceAttributes(_),
                Expectation::PrimaryDeviceAttributes
            )
    )
}
