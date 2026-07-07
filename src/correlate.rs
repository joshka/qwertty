//! The sans-io query correlator: a pure state machine matching replies to expectations.
//!
//! This is the risk core of qwertty's query story (design 03). A [`Correlator`] holds a small
//! ordered set of typed [`Expectation`] values, is fed decoded [`Event`] values one at a time (or a
//! whole decode batch through [`Correlator::feed_batch`]), and for each event decides:
//!
//! - it **completes** the first pending expectation whose typed matcher fully matches the event's
//!   reply — [`Feed::Completed`] carries the expectation id and the typed [`Reply`]; or
//! - it is ordinary input — [`Feed::Passthrough`] returns the event untouched, in arrival order,
//!   never dropped.
//!
//! There is **no clock, no I/O, and no async here** (design 04). Time and bytes are injected by the
//! driver: a Tokio session, a blocking one-shot loop, or a test. Deadlines and EOF enter only
//! through [`Correlator::resolve`], which the driver calls; the correlator never waits.
//!
//! # The rules (design 03)
//!
//! 1. **Full-discriminator matching.** A matcher matches on the complete identity of a reply, so
//!    two pending expectations can never be completed by the same event. Whether two expectations
//!    can be told apart is the static [`distinguishes`] relation. Registering an expectation that
//!    does **not** distinguish from a pending one — yet is not identical to it — is a type-level
//!    error ([`RegisterError::Ambiguous`]); the caller serializes those queries. An *identical*
//!    expectation instead **coalesces**: it returns the pending id with its waiter count bumped
//!    (rule 3, FM-Q14 — two `background_color()` calls want the one answer the terminal sends
//!    once).
//! 2. **Ambiguity policy per query type.** CPR (`CSI r ; c R`) collides with the modified-F3 key
//!    report (`CSI 1 ; modifier R`). The [`Expectation::CursorPosition`] matcher therefore matches
//!    only the *unambiguous* CPR shape and refuses the `row == 1` two-parameter form that could be
//!    an F3 key report (see [the CPR/F3 rule](#the-cprf3-rule)). Ambiguous cursor queries serialize
//!    with the caller.
//! 3. **Duplicate identical queries coalesce** to one expectation with a waiter count. The single
//!    reply is held until every waiter has taken it with [`Correlator::take_reply`]; the state is
//!    freed only then.
//! 4. **Late replies can never complete a later query.** An expectation is removed at
//!    [`Correlator::resolve`] time; a reply arriving after its expectation is gone is a
//!    [`Feed::Passthrough`] (FM-Q3/Q11). Unsolicited query-shaped input is always passthrough.
//! 5. **Typeahead and interleaved input pass through in arrival order** — the correlator never
//!    reorders passthrough events relative to each other (FM-Q1, FM-Q6).
//! 6. **EOF, timeout, and cancellation are distinct resolutions** ([`Resolution`]), surfaced
//!    distinctly so a driver can report each as its own error (FM-Q8).
//!
//! # The CPR/F3 rule
//!
//! The modified-F3 key report and a cursor position report share the `CSI … R` shape: `CSI 1 ; 2 R`
//! is both a valid row-1 CPR and "F3 with Shift". The [syntax layer](crate::report) parses either
//! as a [`CursorPositionReport`] because both are syntactically CPR. The correlator applies design
//! 03 rule 2: [`Expectation::CursorPosition`] matches **only unambiguous CPR shapes** and rejects
//! the two-parameter form whose first parameter is `1`, because that form could be a modified-F3
//! key report. A real cursor at row 1 with two parameters is the price; the design's stated policy
//! is that an app which needs unambiguous row-1 CPR serializes the query (or enables kitty
//! disambiguation, which removes the collision at the source). Every other CPR shape — any row
//! greater than 1, any column — matches normally. The refused event is not lost: it becomes a
//! [`Feed::Passthrough`] carrying its syntax, so an app can still read the F3 keypress (or the
//! row-1 report) itself.
//!
//! # The fence (batch) rule
//!
//! A capability probe writes a bundle of queries plus a trailing `CSI c` (Primary Device
//! Attributes) request as a **fence**: DA1 is answered last, so its reply means "every earlier
//! reply that was coming has now arrived." The correlator supports this with two pieces:
//!
//! - [`Correlator::feed_batch`] feeds a whole decode batch (one `read()` worth of events) at once.
//!   The fence rule is that the session resolves still-pending probe expectations as no-reply
//!   **only after a full batch has been fed** — a DA1 reply and a slower reply arriving in the same
//!   `read()` must both land before the fence acts (FM-Q7: notcurses missed a CPR sitting behind
//!   DA1 in one buffer). `feed_batch` guarantees every event in the batch is matched before it
//!   returns, so the session sees the DA1 completion and the slower completion together.
//! - [`Expectation::PrimaryDeviceAttributes`] is the fence matcher. It matches the DA1 report
//!   *shape* (`CSI ? … c`, any parameters — FM-C4: tmux widening `?1;2c` to `?1;2;4c` must still
//!   match) and completes like any other expectation. It does **not** auto-resolve other pending
//!   expectations: the correlator has no notion of "these expectations belong to one probe." That
//!   fence semantics — treating a DA1 completion as the signal to resolve the probe's other
//!   expectations as no-reply — lives in the probe layer (M3), which owns the set of ids in a
//!   bundle. The correlator only reports the DA1 completion; the M3 layer decides what it means.
//!
//! One more fence rule is a **session** concern, noted here but implemented in M2-S2: registering
//! an expectation must first drain already-buffered undelivered events through the correlator, so a
//! reply that arrived interleaved with earlier typeahead can complete the query before any new
//! read. The correlator makes that implementable — `feed`/`feed_batch` are the drain primitive —
//! but owning the buffered-event queue is the session's job.
//!
//! # Extending the vocabulary (M3)
//!
//! [`Expectation`] and [`Reply`] are `#[non_exhaustive]`. M3 adds discriminator-carrying variants
//! such as `DecPrivateMode { mode }` (DECRQM answers, distinguished by mode number) and OSC colour
//! reports (distinguished by colour index). Each new variant extends [`distinguishes`] with its
//! discriminator so two DECRQM expectations for different modes distinguish (and so register
//! accepts both), while two for the same mode coalesce. The prototype's cross-completion bug
//! (FM-Q10 — a DECRQM reply completing the wrong mode's query) is exactly what the discriminator in
//! [`distinguishes`] prevents.

// This slice lands the correlator ahead of its first non-test consumer. The Tokio/blocking sessions
// that drive `register`/`feed`/`resolve` arrive in M2-S2; until then every item here is exercised
// only by this module's unit tests, so the whole `pub(crate)` module is dead code in a plain
// non-test lib build. Allow it module-wide with this note rather than sprinkling per-item allows;
// remove this when the session wires the correlator in.
#![allow(dead_code)]

use crate::event::Event;
use crate::report::{CursorPositionReport, TerminalStatusReport};
use crate::syntax::{ControlSequence, SyntaxToken};

/// A typed expectation: the identity of a reply the correlator is waiting for.
///
/// Each variant is a matcher plus, in later slices, the discriminator that tells its reply apart
/// from another pending expectation's (design 03 rule 1). The three M2 variants carry no
/// discriminator because their replies never overlap — CPR ends in `R`, DSR in `n`, DA1 in `c` — so
/// [`distinguishes`] separates them by shape alone. M3 adds variants whose discriminator is a mode
/// number or colour index; see the [module docs](self#extending-the-vocabulary-m3).
///
/// The enum is `#[non_exhaustive]` so those variants add without churning existing matches.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Expectation {
    /// A cursor position report, answering a `CSI 6 n` query.
    ///
    /// Matches a [`CursorPositionReport`] in an *unambiguous* CPR shape only: it refuses the
    /// two-parameter `CSI 1 ; modifier R` form that could be a modified-F3 key report (design 03
    /// rule 2; see [the CPR/F3 rule](self#the-cprf3-rule)).
    CursorPosition,
    /// A terminal status report, answering a `CSI 5 n` query. Matches `CSI 0 n` or `CSI 3 n`.
    TerminalStatus,
    /// The Primary Device Attributes fence, answering a `CSI c` query.
    ///
    /// Matches the DA1 report *shape* `CSI ? … c` tolerating any parameter count and values
    /// (FM-C4). This is a fence, not a feature oracle: completing it means "replies that were
    /// coming have arrived," not "the terminal supports X." The correlator does not
    /// auto-resolve other expectations on this completion; the probe layer (M3) owns that.
    PrimaryDeviceAttributes,
}

impl Expectation {
    /// Attempts to match one event against this expectation, returning the typed [`Reply`] on a
    /// full match.
    ///
    /// Only [`Event::Syntax`] carrying a [`SyntaxToken::Csi`] can match any current expectation; a
    /// key event or any other syntax token never does. Matching is *full-discriminator*: the whole
    /// reply identity must match, so no two pending expectations are ever completed by one event.
    fn match_event(self, event: &Event) -> Option<Reply> {
        let csi = control_sequence(event)?;
        match self {
            Self::CursorPosition => match_cursor_position(csi).map(Reply::CursorPosition),
            Self::TerminalStatus => {
                TerminalStatusReport::from_control_sequence(csi).map(Reply::TerminalStatus)
            }
            Self::PrimaryDeviceAttributes => {
                match_primary_device_attributes(csi).map(Reply::PrimaryDeviceAttributes)
            }
        }
    }
}

/// Returns `true` when no single event could complete both expectations — the static overlap
/// relation of design 03 rule 1.
///
/// Two expectations **distinguish** when their reply identities are disjoint: there is no event
/// that both matchers accept. Registering a new expectation that does not distinguish from a
/// pending one is a type-level error *unless the two are identical*, in which case they coalesce
/// (design 03 rule 3). The relation is reflexively `false` on equal expectations (an expectation
/// never distinguishes from itself — that is the coalescing case), symmetric, and enumerable per
/// pair.
///
/// For the M2 variants the reply shapes are disjoint by final byte — CPR ends in `R`, DSR in `n`,
/// DA1 in `c` — so every pair of *different* variants distinguishes. The only non-distinguishing
/// pairs are the identical ones. M3's discriminator-carrying variants (DECRQM by mode, OSC colour
/// by index) refine this: same variant, different discriminator distinguishes; same discriminator
/// does not.
///
/// # Example
///
/// This module is `pub(crate)`, so the example is illustrative rather than a run doctest:
///
/// ```ignore
/// use crate::correlate::{Expectation, distinguishes};
///
/// // Different reply shapes always distinguish.
/// assert!(distinguishes(
///     &Expectation::CursorPosition,
///     &Expectation::TerminalStatus,
/// ));
/// // An expectation never distinguishes from an identical one; that is the coalescing case.
/// assert!(!distinguishes(
///     &Expectation::CursorPosition,
///     &Expectation::CursorPosition,
/// ));
/// ```
///
/// The executed form of this example lives in the module's unit tests (the `distinguishes` matrix).
// The `&Expectation` signature is intentional and part of the design 03 contract: `distinguishes`
// is a *relation over expectations*, and future discriminator-carrying variants (M3) will make
// `Expectation` larger than a `Copy`-by-value threshold, so the by-reference signature is the
// stable one. Suppress the trivially-copy lint that fires only while the M2 variants are still
// fieldless.
#[allow(clippy::trivially_copy_pass_by_ref)]
#[must_use]
pub fn distinguishes(a: &Expectation, b: &Expectation) -> bool {
    // For the M2 variants, distinguishing is exactly "not the same variant": the three reply shapes
    // are disjoint by final byte, and no variant carries a discriminator yet, so identical variants
    // are the only non-distinguishing pair. When M3 adds a discriminator-carrying variant, this
    // relation gains an arm that compares the discriminators of two same-variant expectations
    // (e.g. two `DecPrivateMode` distinguish iff their mode numbers differ).
    a != b
}

/// A typed reply payload delivered by a [`Feed::Completed`] and read back with
/// [`Correlator::take_reply`].
///
/// Each variant carries the parsed report that completed its expectation, so a waiter takes a typed
/// value, not raw bytes. The enum is `#[non_exhaustive]`; M3 adds a variant per new expectation
/// kind.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Reply {
    /// A cursor position report completed an [`Expectation::CursorPosition`].
    CursorPosition(CursorPositionReport),
    /// A terminal status report completed an [`Expectation::TerminalStatus`].
    TerminalStatus(TerminalStatusReport),
    /// A Primary Device Attributes report completed an [`Expectation::PrimaryDeviceAttributes`].
    ///
    /// The fence carries the raw DA1 parameter bytes (everything between `CSI ?` and `c`) so a
    /// probe layer can inspect them if it wants; the correlator itself treats DA1 only as a
    /// fence.
    PrimaryDeviceAttributes(DeviceAttributes),
}

/// The parameters of a Primary Device Attributes (DA1) fence reply.
///
/// DA1 is `CSI ? p1 ; p2 ; … c`; different terminals report different attribute lists, and some
/// (tmux) widen the list over time (FM-C4). The fence matcher tolerates any parameter shape, so
/// this value simply preserves the raw parameter bytes between the `?` marker and the final `c`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceAttributes {
    params: Vec<u8>,
}

impl DeviceAttributes {
    /// Returns the raw DA1 parameter bytes, excluding the `?` private marker and the final `c`.
    ///
    /// For `CSI ? 1 ; 2 c` this is `b"1;2"`. An empty slice is possible for a bare `CSI ? c`.
    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params
    }
}

/// The outcome of feeding one event to the [`Correlator`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Feed {
    /// The event completed a pending expectation.
    ///
    /// The reply is stored on the correlator for the expectation's waiters to take with
    /// [`Correlator::take_reply`]; `id` names that expectation. When more than one waiter coalesced
    /// onto the expectation, every waiter gets the same stored reply.
    Completed {
        /// The expectation this event completed.
        id: ExpectationId,
        /// The typed reply payload.
        reply: Reply,
    },
    /// The event matched no pending expectation and passes through untouched, in arrival order.
    Passthrough(Event),
}

/// An opaque handle to a registered expectation.
///
/// Ids are unique for the life of a [`Correlator`] and are never reused, so a late reply can never
/// be confused with a new expectation that happened to reuse a slot. A coalesced duplicate
/// registration returns the *same* id as the expectation it joined.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExpectationId(u64);

/// Why registering an expectation was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RegisterError {
    /// The new expectation does not [`distinguishes`] from a pending one, and is not identical to
    /// it, so a single reply could complete both (design 03 rule 1). The caller must serialize the
    /// queries: wait for the pending one to resolve before registering this one. Carries the id of
    /// the conflicting pending expectation.
    Ambiguous {
        /// The pending expectation this registration conflicts with.
        conflicting: ExpectationId,
    },
}

/// How a driver resolved a still-pending expectation (design 03 rule 6).
///
/// These are distinct so a driver reports each as its own error. All three are synchronous: there
/// is no async hole between "gave up" and "cleaned up," which is what makes the Tokio driver
/// cancel-safe (design 03 §proof plan).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Resolution {
    /// The driver's deadline elapsed before a reply arrived.
    Timeout,
    /// The terminal closed (end of input) before a reply arrived.
    Eof,
    /// The waiting future was dropped (cancelled) before a reply arrived.
    Cancelled,
}

/// What [`Correlator::resolve`] did to the named expectation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Resolved {
    /// One waiter gave up but others remain; the expectation stays pending for them. Carries the
    /// remaining waiter count.
    WaiterRemoved {
        /// Waiters still waiting on this expectation after this one gave up.
        remaining: u32,
    },
    /// The last waiter gave up and the expectation was removed; no reply had completed it.
    Removed,
    /// The expectation had already been completed by a reply and its result is still held for
    /// unread waiters; this resolution decremented the unread count instead of removing anything.
    /// Carries the remaining unread count.
    AlreadyCompleted {
        /// Waiters that have not yet taken the completed reply.
        unread: u32,
    },
    /// No expectation with this id is pending (already fully resolved or unknown id). A no-op.
    Unknown,
}

/// A pending or completed expectation in the correlator's ordered set.
#[derive(Clone, Debug)]
struct Slot {
    id: ExpectationId,
    expectation: Expectation,
    /// Waiters coalesced onto this expectation that have **not** yet taken a result.
    ///
    /// While pending, this is the number of live waiters. After completion it is the number of
    /// waiters that still need to [`Correlator::take_reply`]; the slot is freed when it reaches
    /// zero. A [`Resolution`] on a live waiter decrements it whether or not the reply has arrived.
    outstanding: u32,
    /// The reply once the expectation is completed; `None` while still pending.
    reply: Option<Reply>,
}

/// The sans-io query correlator (design 03).
///
/// Owns an ordered set of typed expectations and matches fed events to them. Holds no clock, no
/// I/O, and no async state — see the [module docs](self). Construct with [`Correlator::new`],
/// register expectations, feed events, and resolve on driver deadlines/EOF/cancellation.
///
/// # Example
///
/// This module is `pub(crate)`, so the example is illustrative rather than a run doctest; the
/// executed form lives in the module's unit tests:
///
/// ```ignore
/// use crate::correlate::{Correlator, Expectation, Feed, Reply};
/// use crate::SemanticDecoder;
///
/// let mut correlator = Correlator::new();
/// let id = correlator
///     .register(Expectation::CursorPosition)
///     .expect("first cursor expectation");
///
/// // A reply arrives as a decoded event and completes the expectation.
/// let mut decoder = SemanticDecoder::new();
/// let events = decoder.feed(b"\x1b[12;34R");
/// let feed = correlator.feed(events.into_iter().next().unwrap());
///
/// assert!(matches!(feed, Feed::Completed { .. }));
/// let Some(Reply::CursorPosition(report)) = correlator.take_reply(id) else {
///     panic!("expected a cursor position reply");
/// };
/// assert_eq!(report.row(), 12);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Correlator {
    slots: Vec<Slot>,
    next_id: u64,
}

impl Correlator {
    /// Creates an empty correlator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            next_id: 0,
        }
    }

    /// Registers an expectation, returning its id.
    ///
    /// Three outcomes, per design 03 rules 1 and 3:
    ///
    /// - **Coalesce.** An expectation *identical* to a pending one bumps that expectation's waiter
    ///   count and returns its existing [`ExpectationId`]. The single reply that arrives is shared
    ///   by all waiters (each takes it with [`Correlator::take_reply`]).
    /// - **Reject.** An expectation that does not [`distinguishes`] from a pending one, and is not
    ///   identical to it, returns [`RegisterError::Ambiguous`]. The caller must serialize.
    /// - **Register.** Otherwise a fresh id is minted and the expectation is appended to the
    ///   ordered set.
    ///
    /// A completed-but-unread expectation still counts as pending for conflict and coalescing
    /// purposes until its last waiter has taken the reply, so a duplicate registered while a result
    /// is held joins that result rather than waiting for a fresh reply.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError::Ambiguous`] when a non-identical, non-distinguishing expectation is
    /// already pending.
    pub fn register(&mut self, expectation: Expectation) -> Result<ExpectationId, RegisterError> {
        // First pass: an identical pending expectation coalesces (rule 3). A non-identical
        // non-distinguishing one is a conflict (rule 1). Scanning the small ordered set is O(N) at
        // N<=10, deliberately not a HashMap (design 03 alternatives).
        for slot in &mut self.slots {
            if slot.expectation == expectation {
                slot.outstanding += 1;
                return Ok(slot.id);
            }
            if !distinguishes(&slot.expectation, &expectation) {
                return Err(RegisterError::Ambiguous {
                    conflicting: slot.id,
                });
            }
        }

        let id = ExpectationId(self.next_id);
        self.next_id += 1;
        self.slots.push(Slot {
            id,
            expectation,
            outstanding: 1,
            reply: None,
        });
        Ok(id)
    }

    /// Feeds one event, completing the first matching pending expectation or passing it through.
    ///
    /// Scans pending expectations in registration order; the first whose matcher fully accepts the
    /// event is completed and the typed [`Reply`] is stored for its waiters. A completed-but-unread
    /// expectation is skipped for matching (its reply already arrived), so a second reply of the
    /// same shape passes through rather than overwriting the held result. No match returns
    /// [`Feed::Passthrough`] with the event intact.
    #[must_use]
    pub fn feed(&mut self, event: Event) -> Feed {
        for slot in &mut self.slots {
            // Only expectations still awaiting a reply can match; a held (completed) result is not
            // re-completed by a second same-shaped reply — that reply passes through (rule 4).
            if slot.reply.is_some() {
                continue;
            }
            if let Some(reply) = slot.expectation.match_event(&event) {
                slot.reply = Some(reply.clone());
                return Feed::Completed { id: slot.id, reply };
            }
        }
        Feed::Passthrough(event)
    }

    /// Feeds a whole decode batch and returns one [`Feed`] per event, in order.
    ///
    /// This is the fence primitive (design 03 §probe bundle): the session feeds one `read()` worth
    /// of events through here so that every reply in the batch — a DA1 fence *and* a slower reply
    /// arriving in the same buffer — is matched before the batch returns. The fence rule the
    /// session relies on is that it must not resolve still-pending probe expectations as
    /// no-reply until after a full batch has been fed; `feed_batch` guarantees that "full
    /// batch" is atomic from the session's point of view (FM-Q7).
    ///
    /// The correlator itself does **not** treat a [`Expectation::PrimaryDeviceAttributes`]
    /// completion as a signal to resolve other expectations; that fence *semantics* is the probe
    /// layer's job (M3). This method only guarantees ordering and completeness of matching over the
    /// batch.
    #[must_use]
    pub fn feed_batch(&mut self, events: impl IntoIterator<Item = Event>) -> Vec<Feed> {
        events.into_iter().map(|event| self.feed(event)).collect()
    }

    /// Takes the completed reply for one waiter of an expectation.
    ///
    /// Returns the stored [`Reply`] and decrements the expectation's outstanding-waiter count. When
    /// the last waiter takes the reply, the slot is freed. Returns `None` when the id is unknown or
    /// the expectation has not been completed yet (no reply to take).
    ///
    /// This is the shared-result fan-out primitive: coalesced waiters each call `take_reply` and
    /// each receives the same reply; the correlator holds the result until all of them have (design
    /// 03 rule 3, salvage "state freed after all waiters read").
    #[must_use]
    pub fn take_reply(&mut self, id: ExpectationId) -> Option<Reply> {
        let index = self.slots.iter().position(|slot| slot.id == id)?;
        let reply = self.slots[index].reply.clone()?;
        let slot = &mut self.slots[index];
        slot.outstanding = slot.outstanding.saturating_sub(1);
        if slot.outstanding == 0 {
            self.slots.remove(index);
        }
        Some(reply)
    }

    /// Resolves one waiter of an expectation with a driver-injected [`Resolution`].
    ///
    /// A [`Resolution`] means one waiting future gave up (timeout, EOF, or cancellation). It
    /// decrements the expectation's outstanding count:
    ///
    /// - if the expectation is still pending and other waiters remain, it stays pending for them
    ///   ([`Resolved::WaiterRemoved`]);
    /// - if the last pending waiter gives up, the expectation is removed ([`Resolved::Removed`]);
    ///   after this a matching reply is passthrough (rule 4 — a late reply never completes a query
    ///   that was already given up on);
    /// - if the expectation was already completed, its held reply stays available for the waiters
    ///   that have not read it; this resolution just decrements the unread count
    ///   ([`Resolved::AlreadyCompleted`]);
    /// - an unknown id is a no-op ([`Resolved::Unknown`]).
    ///
    /// The [`Resolution`] value is preserved in the return only through its effect; the *kind*
    /// (timeout vs EOF vs cancelled) is the driver's to surface as an error. The correlator treats
    /// all three the same way here — they all mean "this waiter is done waiting" — which is what
    /// makes cancellation, timeout, and EOF a single synchronous cleanup path (design 03).
    pub fn resolve(&mut self, id: ExpectationId, _resolution: Resolution) -> Resolved {
        let Some(index) = self.slots.iter().position(|slot| slot.id == id) else {
            return Resolved::Unknown;
        };

        // A completed-but-unread expectation keeps its held reply for readers; a resolution here
        // means one waiter stopped waiting without reading, so decrement the unread count and free
        // the slot only when it hits zero. It is never re-opened for matching.
        if self.slots[index].reply.is_some() {
            let slot = &mut self.slots[index];
            slot.outstanding = slot.outstanding.saturating_sub(1);
            let unread = slot.outstanding;
            if unread == 0 {
                self.slots.remove(index);
            }
            return Resolved::AlreadyCompleted { unread };
        }

        let slot = &mut self.slots[index];
        slot.outstanding = slot.outstanding.saturating_sub(1);
        if slot.outstanding == 0 {
            self.slots.remove(index);
            Resolved::Removed
        } else {
            Resolved::WaiterRemoved {
                remaining: slot.outstanding,
            }
        }
    }

    /// Returns the number of expectations currently tracked (pending or completed-but-unread).
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Returns `true` when no expectations are tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Returns `true` when an expectation with this id is still tracked (pending or held).
    #[must_use]
    pub fn contains(&self, id: ExpectationId) -> bool {
        self.slots.iter().any(|slot| slot.id == id)
    }

    /// Returns the outstanding-waiter count for an expectation, or `None` when it is not tracked.
    ///
    /// While pending this is the live waiter count; after completion it is the number of waiters
    /// that have not yet taken the reply.
    #[must_use]
    pub fn waiters(&self, id: ExpectationId) -> Option<u32> {
        self.slots
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| slot.outstanding)
    }

    /// Returns `true` when an expectation has been completed and its reply is still held.
    #[must_use]
    pub fn is_completed(&self, id: ExpectationId) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.id == id && slot.reply.is_some())
    }
}

/// Returns the CSI control sequence carried by a passthrough syntax event, or `None`.
///
/// Only a [`Event::Syntax`] holding a [`SyntaxToken::Csi`] can be a report; everything else (keys,
/// other syntax families) is never a match candidate.
fn control_sequence(event: &Event) -> Option<&ControlSequence> {
    match event.syntax_token()? {
        SyntaxToken::Csi(csi) => Some(csi),
        _ => None,
    }
}

/// Matches an unambiguous cursor position report, applying the modified-F3 exclusion (rule 2).
///
/// Parses the CPR shape, then refuses the two-parameter `row == 1` form (`CSI 1 ; modifier R`) that
/// could instead be a modified-F3 key report. Every other CPR shape matches. See
/// [the CPR/F3 rule](self#the-cprf3-rule).
fn match_cursor_position(csi: &ControlSequence) -> Option<CursorPositionReport> {
    let report = CursorPositionReport::from_control_sequence(csi)?;

    // The modified-F3 collision: `CSI 1 ; modifier R`. A row-1 report with a second parameter
    // present is ambiguous with F3-plus-modifier, so the CPR matcher declines it (the app can read
    // it as a key or a report through passthrough, or serialize the query). Count the raw `;`
    // separators to detect the two-parameter form; the parser already guaranteed exactly two
    // fields, so a single `;` at row 1 is the ambiguous case.
    let two_parameters = csi.params().param_bytes().contains(&b';');
    if report.row() == 1 && two_parameters {
        return None;
    }

    Some(report)
}

/// Matches the DA1 fence report shape `CSI ? … c`, tolerating any parameters (FM-C4).
///
/// The shape rule is: a `?` private marker, no intermediate bytes, final byte `c`, and any
/// parameter bytes (including none). Terminals report different and sometimes widening attribute
/// lists, so the matcher never inspects the parameter values — matching the shape is enough for a
/// fence.
fn match_primary_device_attributes(csi: &ControlSequence) -> Option<DeviceAttributes> {
    let params = csi.params();
    if params.final_byte() != b'c'
        || params.private_markers() != b"?"
        || !params.intermediates().is_empty()
    {
        return None;
    }
    Some(DeviceAttributes {
        params: params.param_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests;
