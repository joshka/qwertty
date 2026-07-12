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
use crate::report::{DecPrivateModeState, OscColorKind, TerminalStatus};
use crate::syntax::SyntaxParser;

/// A representative set of every expectation variant, using distinct discriminators, for the
/// matrix tests. Two `DecPrivateMode` and two `OscColor` are included with *different*
/// discriminators so the matrix exercises the FM-Q10 same-variant/different-discriminator case.
fn all_variants() -> Vec<Expectation> {
    vec![
        Expectation::CursorPosition,
        Expectation::TerminalStatus,
        Expectation::PrimaryDeviceAttributes,
        Expectation::KittyKeyboardFlags,
        Expectation::XtVersion,
        Expectation::DecPrivateMode { mode: 2026 },
        Expectation::DecPrivateMode { mode: 2027 },
        Expectation::OscColor {
            which: OscColorKind::Foreground,
        },
        Expectation::OscColor {
            which: OscColorKind::Background,
        },
        Expectation::KittyGraphics { image_id: 31 },
        Expectation::KittyGraphics { image_id: 32 },
        Expectation::TextAreaPixels,
        Expectation::CellSize,
    ]
}

/// Builds an `Event::Syntax` from the single APC token `bytes` must encode.
fn apc_event(bytes: &[u8]) -> Event {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(bytes);
    tokens.extend(parser.finish());
    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    match tokens.into_iter().next().expect("one token") {
        token @ SyntaxToken::Apc(_) => Event::Syntax(token),
        other => panic!("expected an APC token, got {other:?}"),
    }
}

/// Builds an `Event::Syntax` from the single OSC token `bytes` must encode.
fn osc_event(bytes: &[u8]) -> Event {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(bytes);
    tokens.extend(parser.finish());
    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    match tokens.into_iter().next().expect("one token") {
        token @ SyntaxToken::Osc(_) => Event::Syntax(token),
        other => panic!("expected an OSC token, got {other:?}"),
    }
}

/// Builds an `Event::Syntax` from the single DCS token `bytes` must encode.
fn dcs_event(bytes: &[u8]) -> Event {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(bytes);
    tokens.extend(parser.finish());
    assert_eq!(tokens.len(), 1, "expected one token from {bytes:?}");
    match tokens.into_iter().next().expect("one token") {
        token @ SyntaxToken::Dcs(_) => Event::Syntax(token),
        other => panic!("expected a DCS token, got {other:?}"),
    }
}

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
    for expectation in all_variants() {
        assert!(
            !distinguishes(expectation, expectation),
            "{expectation:?} must not distinguish from itself (that is the coalescing case)"
        );
    }
}

#[test]
fn distinguishes_is_true_on_every_different_pair() {
    let variants = all_variants();
    for (i, a) in variants.iter().enumerate() {
        for (j, b) in variants.iter().enumerate() {
            if i != j {
                assert!(
                    distinguishes(*a, *b),
                    "{a:?} and {b:?} have disjoint reply shapes/discriminators and must distinguish"
                );
            }
        }
    }
}

#[test]
fn two_different_dec_private_modes_distinguish() {
    // FM-Q10, the production test: two DECRQM expectations for different modes must distinguish, so
    // both can be registered in one bundle and each completes only with its own mode's answer.
    let a = Expectation::DecPrivateMode { mode: 2026 };
    let b = Expectation::DecPrivateMode { mode: 2027 };
    assert!(
        distinguishes(a, b),
        "DecPrivateMode(2026) and DecPrivateMode(2027) must distinguish (FM-Q10)"
    );
    // Same mode coalesces (does not distinguish).
    assert!(!distinguishes(
        a,
        Expectation::DecPrivateMode { mode: 2026 }
    ));
}

#[test]
fn two_different_osc_colors_distinguish() {
    let fg = Expectation::OscColor {
        which: OscColorKind::Foreground,
    };
    let bg = Expectation::OscColor {
        which: OscColorKind::Background,
    };
    assert!(
        distinguishes(fg, bg),
        "OscColor(Foreground) and OscColor(Background) must distinguish"
    );
    assert!(!distinguishes(
        fg,
        Expectation::OscColor {
            which: OscColorKind::Foreground
        }
    ));
}

#[test]
fn distinguishes_is_symmetric() {
    let variants = all_variants();
    for a in &variants {
        for b in &variants {
            assert_eq!(
                distinguishes(*a, *b),
                distinguishes(*b, *a),
                "distinguishes must be symmetric for {a:?}, {b:?}"
            );
        }
    }
}

#[test]
fn non_distinguishing_pairs_are_all_identical() {
    // The register-reject path (`RegisterError::Ambiguous`) fires only for a non-identical,
    // non-distinguishing pair. Because every matcher requires either a disjoint reply shape or its
    // own discriminator, no such pair exists — every non-distinguishing pair is identical, and
    // those coalesce. This documents why register never rejects for the current vocabulary.
    let variants = all_variants();
    for a in &variants {
        for b in &variants {
            if !distinguishes(*a, *b) {
                assert_eq!(a, b, "a non-distinguishing pair must be identical");
            }
        }
    }
}

// --- FM-Q10: DECRQM discriminator matching --------------------------------------------------

#[test]
fn decrqm_reply_completes_only_the_matching_mode() {
    // Two concurrent DECRQM expectations for modes 2026 and 2027. A `?2026;1$y` reply completes
    // ONLY the 2026 expectation; the 2027 expectation stays pending. This is the exact
    // cross-completion the prototype got wrong (FM-Q10).
    let mut correlator = Correlator::new();
    let m2026 = correlator
        .register(Expectation::DecPrivateMode { mode: 2026 })
        .expect("register 2026");
    let m2027 = correlator
        .register(Expectation::DecPrivateMode { mode: 2027 })
        .expect("register 2027");

    let feed = correlator.feed(csi_event(b"\x1b[?2026;1$y"));
    let Feed::Completed { id, reply } = feed else {
        panic!("expected the 2026 expectation to complete, got {feed:?}");
    };
    assert_eq!(id, m2026, "only the 2026 expectation completes");
    let Reply::DecPrivateMode(report) = reply else {
        panic!("expected a DecPrivateMode reply");
    };
    assert_eq!(report.mode(), 2026);
    assert_eq!(report.state(), DecPrivateModeState::Set);
    assert!(
        correlator.contains(m2027),
        "the 2027 expectation is untouched by the 2026 reply"
    );

    // The 2027 answer then completes only 2027.
    let feed = correlator.feed(csi_event(b"\x1b[?2027;2$y"));
    assert!(matches!(feed, Feed::Completed { id, .. } if id == m2027));
}

#[test]
fn decrqm_reply_for_unregistered_mode_passes_through() {
    // A DECRQM answer for a mode no expectation is waiting on is ordinary input.
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::DecPrivateMode { mode: 2026 })
        .expect("register 2026");
    let event = csi_event(b"\x1b[?2048;1$y");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

// --- kitty graphics (APC-framed) and XTWINOPS geometry matching -----------------------------

#[test]
fn kitty_graphics_reply_completes_only_the_matching_image_id() {
    // Two concurrent graphics expectations for image ids 31 and 32: the FM-Q10 discriminator rule
    // applied to the APC response's echoed id.
    let mut correlator = Correlator::new();
    let id31 = correlator
        .register(Expectation::KittyGraphics { image_id: 31 })
        .expect("register image 31");
    let id32 = correlator
        .register(Expectation::KittyGraphics { image_id: 32 })
        .expect("register image 32");

    let feed = correlator.feed(apc_event(b"\x1b_Gi=31;OK\x1b\\"));
    let Feed::Completed { id, reply } = feed else {
        panic!("expected the image-31 expectation to complete, got {feed:?}");
    };
    assert_eq!(id, id31, "only the image-31 expectation completes");
    let Reply::KittyGraphics(report) = reply else {
        panic!("expected a KittyGraphics reply");
    };
    assert_eq!(report.image_id(), Some(31));
    assert!(report.is_ok());
    assert!(
        correlator.contains(id32),
        "the image-32 expectation is untouched by the image-31 response"
    );

    // An error response still completes its own expectation: an answer is an answer.
    let feed = correlator.feed(apc_event(b"\x1b_Gi=32;EBADPNG:bad data\x1b\\"));
    let Feed::Completed { id, reply } = feed else {
        panic!("expected the image-32 expectation to complete, got {feed:?}");
    };
    assert_eq!(id, id32);
    let Reply::KittyGraphics(report) = reply else {
        panic!("expected a KittyGraphics reply");
    };
    assert!(!report.is_ok());
    assert_eq!(report.message(), "EBADPNG:bad data");
}

#[test]
fn kitty_graphics_reply_for_unregistered_id_passes_through() {
    // A graphics response echoing an id nothing is waiting on is ordinary input (rule 4).
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::KittyGraphics { image_id: 31 })
        .expect("register image 31");
    let event = apc_event(b"\x1b_Gi=99;OK\x1b\\");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

#[test]
fn non_graphics_apc_passes_through_with_graphics_pending() {
    // An APC that is not a graphics response (no leading G) never completes the expectation.
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::KittyGraphics { image_id: 31 })
        .expect("register image 31");
    let event = apc_event(b"\x1b_zsome-other-apc\x1b\\");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

#[test]
fn geometry_replies_complete_only_their_own_op_code() {
    // Text-area (op 4) and cell-size (op 6) share the `t` final; the leading op code is the
    // discriminator. Register both, answer in reverse order.
    let mut correlator = Correlator::new();
    let text_area = correlator
        .register(Expectation::TextAreaPixels)
        .expect("register text-area");
    let cell = correlator
        .register(Expectation::CellSize)
        .expect("register cell-size");

    let feed = correlator.feed(csi_event(b"\x1b[6;25;14t"));
    let Feed::Completed { id, reply } = feed else {
        panic!("expected the cell-size expectation to complete, got {feed:?}");
    };
    assert_eq!(id, cell, "op 6 completes only the cell-size expectation");
    let Reply::CellSize(report) = reply else {
        panic!("expected a CellSize reply");
    };
    assert_eq!(report.pixel_size(), Some(crate::PixelSize::new(14, 25)));

    let feed = correlator.feed(csi_event(b"\x1b[4;1000;1680t"));
    let Feed::Completed { id, reply } = feed else {
        panic!("expected the text-area expectation to complete, got {feed:?}");
    };
    assert_eq!(id, text_area);
    let Reply::TextAreaPixels(report) = reply else {
        panic!("expected a TextAreaPixels reply");
    };
    assert_eq!(report.pixel_size(), Some(crate::PixelSize::new(1680, 1000)));
}

#[test]
fn other_xtwinops_t_reports_pass_through_geometry_expectations() {
    // The cells report (op 8) matches neither geometry expectation.
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::TextAreaPixels)
        .expect("register text-area");
    correlator
        .register(Expectation::CellSize)
        .expect("register cell-size");
    let event = csi_event(b"\x1b[8;40;120t");
    assert_eq!(correlator.feed(event.clone()), Feed::Passthrough(event));
}

#[test]
fn zero_geometry_reply_still_completes_with_unknown_pixel_size() {
    // FM-Z5: a zero answer is an answer (Probed evidence) whose value is unknown — the completion
    // must happen so the probe records "answered zeros", not a timeout.
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::TextAreaPixels)
        .expect("register text-area");
    let feed = correlator.feed(csi_event(b"\x1b[4;0;0t"));
    assert!(matches!(feed, Feed::Completed { id: done, .. } if done == id));
    let Some(Reply::TextAreaPixels(report)) = correlator.take_reply(id) else {
        panic!("expected a TextAreaPixels reply");
    };
    assert_eq!(report.pixel_size(), None, "zeros never become a geometry");
}

// --- XTVERSION (DCS-framed) and OSC colour (OSC-framed) matching ----------------------------

#[test]
fn xtversion_dcs_reply_completes_xtversion_expectation() {
    let mut correlator = Correlator::new();
    let id = correlator
        .register(Expectation::XtVersion)
        .expect("register xtversion");
    let feed = correlator.feed(dcs_event(b"\x1bP>|ghostty 1.0.0\x1b\\"));
    assert!(matches!(feed, Feed::Completed { .. }));
    let Some(Reply::XtVersion(report)) = correlator.take_reply(id) else {
        panic!("expected an XtVersion reply");
    };
    assert_eq!(report.version(), "ghostty 1.0.0");
}

#[test]
fn osc_color_reply_completes_only_the_matching_color() {
    // An OSC 11 background reply completes only the background expectation, not the foreground one.
    let mut correlator = Correlator::new();
    let fg = correlator
        .register(Expectation::OscColor {
            which: OscColorKind::Foreground,
        })
        .expect("register fg");
    let bg = correlator
        .register(Expectation::OscColor {
            which: OscColorKind::Background,
        })
        .expect("register bg");

    // A background report (OSC 11), ST-terminated.
    let feed = correlator.feed(osc_event(b"\x1b]11;rgb:1a1a/2b2b/3c3c\x1b\\"));
    let Feed::Completed { id, .. } = feed else {
        panic!("expected the background expectation to complete, got {feed:?}");
    };
    assert_eq!(id, bg, "only the background colour expectation completes");
    assert!(correlator.contains(fg), "foreground stays pending");

    // A foreground report (OSC 10), BEL-terminated (FM-P9: both terminators accepted).
    let feed = correlator.feed(osc_event(b"\x1b]10;rgb:ffff/ffff/ffff\x07"));
    assert!(matches!(feed, Feed::Completed { id, .. } if id == fg));
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
fn kitty_flags_reply_completes_kitty_expectation() {
    // The `CSI ? flags u` report completes the verify-after-push expectation, carrying the granted
    // flag bitset. A bare `CSI ? u` reports zero flags.
    for (bytes, expected) in [
        (&b"\x1b[?1u"[..], 1u8),
        (&b"\x1b[?31u"[..], 31u8),
        (&b"\x1b[?u"[..], 0u8),
    ] {
        let mut correlator = Correlator::new();
        let id = correlator
            .register(Expectation::KittyKeyboardFlags)
            .expect("register");
        let feed = correlator.feed(csi_event(bytes));
        assert!(
            matches!(feed, Feed::Completed { .. }),
            "flags report {bytes:?} must complete the expectation"
        );
        let Some(Reply::KittyKeyboardFlags(bits)) = correlator.take_reply(id) else {
            panic!("expected kitty flags reply for {bytes:?}");
        };
        assert_eq!(bits, expected, "granted flags for {bytes:?}");
    }
}

#[test]
fn kitty_flags_matcher_rejects_key_csi_u_and_control_forms() {
    let mut correlator = Correlator::new();
    correlator
        .register(Expectation::KittyKeyboardFlags)
        .expect("register");
    // A plain key `CSI u` (no `?`) is not a flags report.
    assert!(matches!(
        correlator.feed(csi_event(b"\x1b[97u")),
        Feed::Passthrough(_)
    ));
    // The push/pop control forms (`>`/`<` markers) are not the report shape either.
    assert!(matches!(
        correlator.feed(csi_event(b"\x1b[>1u")),
        Feed::Passthrough(_)
    ));
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
