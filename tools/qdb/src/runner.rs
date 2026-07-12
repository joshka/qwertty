//! The conformance runner loop: policy over dumb targets.
//!
//! The runner owns everything the Target sketch assigns it (design
//! `conformance-target-interface.md`): deadline policy, replay-class enforcement (nothing
//! `modal`/`destructive` without explicit opt-in — the DECSLPP incident rule), reply recording,
//! late-reply attribution, an echoed-reply fabrication guard, and the DA1/XTVERSION identity
//! cross-check. Adapters own only the efficient wait. Capture mode is this same loop with
//! recording on: the [`ProbeReport`] it produces is the exact input the minting layer
//! (`crate::capture`) already turns into sidecars, fixtures, and results.
//!
//! Platform-independent by construction — the loop sees only the [`Target`] trait, so its unit
//! tests run against a scripted fake on every platform, no terminal needed.

use std::time::Duration;

use crate::capture::{Identity, ProbeLine, ProbePlan, ProbeReport, ProbeStatus, VersionSource};
use crate::escape;
use crate::targets::Target;

/// DA1 primary device attributes query — sent for identity regardless of db entries.
const DA1: &[u8] = b"\x1b[c";
/// XTVERSION query (`CSI > q`) — the terminal-name/version string, for identity.
const XTVERSION: &[u8] = b"\x1b[>q";

/// Runner policy knobs. The defaults are the M7-S2 harness's proven values.
#[derive(Clone, Debug)]
pub struct RunnerOptions {
    /// Session geometry, columns.
    pub cols: u16,
    /// Session geometry, rows.
    pub rows: u16,
    /// How long to wait for the first reply byte before declaring a query silent. Terminals
    /// that answer do so in single-digit milliseconds; a generous deadline still keeps a
    /// 30-query run fast while surviving a loaded CI host.
    pub first_byte_deadline: Duration,
    /// After the first reply byte, keep draining until this much quiet passes, so a multi-byte
    /// reply (DA1, OSC color) is captured whole rather than truncated at the first read.
    pub settle_quiet: Duration,
    /// Allow entries with `replay = "modal"` into the run (explicit opt-in).
    pub allow_modal: bool,
    /// Allow entries with `replay = "destructive"` into the run (explicit opt-in; never a
    /// blind default — the DECSLPP incident resized a real xterm).
    pub allow_destructive: bool,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            cols: 120,
            rows: 40,
            first_byte_deadline: Duration::from_millis(400),
            settle_quiet: Duration::from_millis(120),
            allow_modal: false,
            allow_destructive: false,
        }
    }
}

/// The verdict of the DA1/XTVERSION identity cross-check against the adapter's claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityCheck {
    /// The wire name matched the adapter's expectation.
    Verified {
        /// The name the terminal reported on the wire.
        wire_name: String,
    },
    /// The check could not run (no expectation declared, or the terminal answered no
    /// XTVERSION). Recorded, never silently treated as verified.
    Unverifiable {
        /// Why the check could not run.
        reason: String,
    },
    /// The wire name contradicts the adapter's expectation — a recorded error: the results
    /// would be keyed to the wrong terminal.
    Mismatch {
        /// The name the adapter expected on the wire.
        expected: String,
        /// The name the terminal actually reported.
        wire_name: String,
    },
}

/// Everything one runner pass produced.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    /// The probe report — the minting layer's input, schema-identical to the M7-S2 harness.
    pub report: ProbeReport,
    /// The identity cross-check verdict.
    pub identity_check: IdentityCheck,
    /// Reply bytes that arrived before any query was sent (escaped) — identity-probe
    /// stragglers. Recorded rather than dropped; never attributed to a query.
    pub strays: Vec<String>,
    /// How the target's version was resolved (XTVERSION vs the adapter hint), for the results
    /// metadata.
    pub version_source: VersionSource,
    /// A teardown failure, if `end` failed. The captured data is kept either way.
    pub teardown_error: Option<String>,
}

/// Runs the conformance loop: start the target, probe identity, execute the plan's queries
/// under the replay-class gate, and end the session.
///
/// # Errors
///
/// Returns an error if the target cannot start, the transport dies mid-run, or the plan
/// contains a replay class the options do not allow (an internal-consistency guard — the plan
/// builder applies the same gate).
pub fn run(
    target: &mut dyn Target,
    plan: &ProbePlan,
    timestamp: &str,
    opts: &RunnerOptions,
) -> Result<RunOutcome, String> {
    for spec in &plan.specs {
        let allowed = match spec.replay {
            crate::capture::ReplayClass::Safe => true,
            crate::capture::ReplayClass::Modal => opts.allow_modal,
            crate::capture::ReplayClass::Destructive => opts.allow_destructive,
        };
        if !allowed {
            return Err(format!(
                "plan contains {} entry {} without the matching opt-in flag",
                spec.replay.as_str(),
                spec.query_id
            ));
        }
    }

    target.start(opts.cols, opts.rows)?;
    let identity = target.identity();

    // Identity first, so a terminal that answers nothing else still gets a DA1/XTVERSION
    // record.
    target.feed(DA1)?;
    let da1 = drain_reply(target, opts)?;
    target.feed(XTVERSION)?;
    let xtversion = drain_reply(target, opts)?;
    let (version, version_source) = resolve_version(&identity.version_hint, &xtversion);
    let identity_check = cross_check(identity.expected_wire_name.as_deref(), &xtversion);

    let mut lines: Vec<ProbeLine> = Vec::new();
    let mut strays = Vec::new();
    for spec in &plan.specs {
        // Attribute residue before feeding: a reply that arrived after its query's deadline
        // belongs to the *previous* line, and must never pollute this query's recording.
        attribute_residue(&target.drain_output(None)?, &mut lines, &mut strays);

        target.feed(&spec.query_bytes)?;
        let reply = drain_reply(target, opts)?;
        let status = if reply.is_empty() {
            ProbeStatus::Timeout
        } else {
            ProbeStatus::Answered
        };
        // Fabrication guard: a "reply" that is byte-identical to the query is almost certainly
        // the terminal (or an interposed shell) echoing input — the exact failure mode the
        // quarantine exists for. Recorded as suspect; the minting layer refuses it a fixture.
        let echo_suspect = !reply.is_empty() && reply == spec.query_bytes;
        lines.push(ProbeLine {
            query_id: spec.query_id.clone(),
            reply_id: spec.reply_id.clone(),
            reply_escaped: escape::escape(&reply),
            reply_len: reply.len(),
            status,
            late_reply_escaped: None,
            echo_suspect,
        });
    }
    // A reply racing the shutdown still gets attributed.
    attribute_residue(&target.drain_output(None)?, &mut lines, &mut strays);

    let teardown_error = target.end().err();

    Ok(RunOutcome {
        report: ProbeReport {
            identity: Identity {
                target: identity.name,
                da1_escaped: escape::escape(&da1),
                xtversion_escaped: escape::escape(&xtversion),
                version,
            },
            timestamp: timestamp.to_string(),
            lines,
        },
        identity_check,
        strays,
        version_source,
        teardown_error,
    })
}

/// Drains one reply under the runner's deadline policy: wait up to `first_byte_deadline` for
/// the first byte, then keep draining until `settle_quiet` passes with no further bytes, so
/// multi-byte replies arrive whole. Empty means silent — data, not an error.
fn drain_reply(target: &mut dyn Target, opts: &RunnerOptions) -> Result<Vec<u8>, String> {
    let mut reply = target.drain_output(Some(opts.first_byte_deadline))?;
    if reply.is_empty() {
        return Ok(reply);
    }
    loop {
        let more = target.drain_output(Some(opts.settle_quiet))?;
        if more.is_empty() {
            return Ok(reply);
        }
        reply.extend_from_slice(&more);
    }
}

/// Attributes residue bytes to the last recorded line as a late reply, or to the stray list
/// when no query has run yet. Late bytes are appended when a line already carries some.
fn attribute_residue(residue: &[u8], lines: &mut [ProbeLine], strays: &mut Vec<String>) {
    if residue.is_empty() {
        return;
    }
    let escaped = escape::escape(residue);
    match lines.last_mut() {
        Some(line) => match &mut line.late_reply_escaped {
            Some(existing) => existing.push_str(&escaped),
            none => *none = Some(escaped),
        },
        None => strays.push(escaped),
    }
}

/// Picks the terminal's identity version string and reports how it was obtained. The XTVERSION
/// reply is authoritative — it is the emulator naming *itself* (betamax hosts ghostty, so its
/// XTVERSION is `libghostty`, not the betamax tool version) — so it wins over the adapter hint
/// (`tmux -V` / `betamax --version`), which is only a fallback when the terminal answers no
/// XTVERSION. An empty hint with no XTVERSION yields `("", None)`.
fn resolve_version(hint: &str, xtversion: &[u8]) -> (String, VersionSource) {
    if let Some(name) = xtversion_name(xtversion) {
        return (name, VersionSource::XtVersion);
    }
    let hint = hint.trim().to_string();
    if hint.is_empty() {
        (hint, VersionSource::None)
    } else {
        (hint, VersionSource::Hint)
    }
}

/// Extracts the self-reported name from an XTVERSION reply: `ESC P > | <text> ESC \`.
fn xtversion_name(xtversion: &[u8]) -> Option<String> {
    let start = find_subslice(xtversion, b">|")?;
    let after = &xtversion[start + 2..];
    let end = after.iter().position(|&b| b == 0x1b).unwrap_or(after.len());
    let name = String::from_utf8_lossy(&after[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Cross-checks the adapter's expected wire name against the XTVERSION reply. The expected
/// name matching case-insensitively anywhere in the wire string verifies (tmux reports
/// `tmux 3.7b`; betamax expects `ghostty` and the wire says `libghostty`).
fn cross_check(expected: Option<&str>, xtversion: &[u8]) -> IdentityCheck {
    let Some(expected) = expected else {
        return IdentityCheck::Unverifiable {
            reason: "adapter declares no expected wire name".to_string(),
        };
    };
    let Some(wire_name) = xtversion_name(xtversion) else {
        return IdentityCheck::Unverifiable {
            reason: "target answered no XTVERSION".to_string(),
        };
    };
    if wire_name.to_lowercase().contains(&expected.to_lowercase()) {
        IdentityCheck::Verified { wire_name }
    } else {
        IdentityCheck::Mismatch {
            expected: expected.to_string(),
            wire_name,
        }
    }
}

/// Finds the first index of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{ProbeSource, ProbeSpec, ReplayClass};
    use crate::targets::{AdapterKind, StateProbe, StateReading, TargetIdentity};

    /// What the fake target does when its next drain window opens for a fed query.
    #[derive(Clone, Debug)]
    enum Behavior {
        /// Reply with these bytes inside the first-byte deadline.
        Reply(Vec<u8>),
        /// Never reply.
        Silence,
        /// Reply only after the deadline drain has given up — the bytes surface in a later
        /// drain call (the pre-feed residue drain of the next query, or the final drain).
        Late(Vec<u8>),
        /// Echo the query bytes back verbatim (the fabrication failure mode).
        Echo,
    }

    /// A scripted in-memory target: deterministic, no clocks, no terminal. Behaviors are
    /// keyed by feed order; DA1 and XTVERSION get dedicated replies.
    struct FakeTarget {
        identity: TargetIdentity,
        da1_reply: Vec<u8>,
        xtversion_reply: Vec<u8>,
        behaviors: Vec<Behavior>,
        /// Bytes ready for the next drain call(s).
        pending: Vec<u8>,
        /// Bytes that surface one drain call later (the "late" mechanism).
        late_pending: Vec<u8>,
        fed: usize,
        started: bool,
        ended: bool,
    }

    impl FakeTarget {
        fn new(behaviors: Vec<Behavior>) -> Self {
            Self {
                identity: TargetIdentity {
                    name: "fake".to_string(),
                    version_hint: "fake 0.1".to_string(),
                    adapter: AdapterKind::InProcess,
                    expected_wire_name: Some("faketerm".to_string()),
                },
                da1_reply: b"\x1b[?1;2c".to_vec(),
                xtversion_reply: b"\x1bP>|faketerm 9.9\x1b\\".to_vec(),
                behaviors,
                pending: Vec::new(),
                late_pending: Vec::new(),
                fed: 0,
                started: false,
                ended: false,
            }
        }
    }

    impl Target for FakeTarget {
        fn identity(&self) -> TargetIdentity {
            self.identity.clone()
        }

        fn start(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
            self.started = true;
            Ok(())
        }

        fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
            assert!(self.started, "feed before start");
            let idx = self.fed;
            self.fed += 1;
            // Feeds 0 and 1 are the runner's identity probes.
            if idx == 0 {
                self.pending.extend_from_slice(&self.da1_reply);
                return Ok(());
            }
            if idx == 1 {
                self.pending.extend_from_slice(&self.xtversion_reply);
                return Ok(());
            }
            match self.behaviors.get(idx - 2) {
                Some(Behavior::Reply(r)) => self.pending.extend_from_slice(r),
                Some(Behavior::Silence) | None => {}
                Some(Behavior::Late(r)) => self.late_pending.extend_from_slice(r),
                Some(Behavior::Echo) => self.pending.extend_from_slice(bytes),
            }
            Ok(())
        }

        fn drain_output(&mut self, _deadline: Option<Duration>) -> Result<Vec<u8>, String> {
            if self.pending.is_empty() {
                // The deadline drain came up empty: any late bytes "arrive" now, surfacing in
                // the next drain call.
                self.pending = std::mem::take(&mut self.late_pending);
                return Ok(Vec::new());
            }
            Ok(std::mem::take(&mut self.pending))
        }

        fn read_state(&mut self, _probe: StateProbe) -> Result<Option<StateReading>, String> {
            Ok(None)
        }

        fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), String> {
            Ok(())
        }

        fn end(&mut self) -> Result<(), String> {
            self.ended = true;
            Ok(())
        }
    }

    fn spec(id: &str, bytes: &[u8], replay: ReplayClass) -> ProbeSpec {
        ProbeSpec {
            query_id: id.to_string(),
            reply_id: format!("{id}_report"),
            query_bytes: bytes.to_vec(),
            family_dir: "ecma48".to_string(),
            source: ProbeSource::FixtureBytes,
            replay,
        }
    }

    fn plan(specs: Vec<ProbeSpec>) -> ProbePlan {
        ProbePlan {
            specs,
            unprobeable: Vec::new(),
            skipped: Vec::new(),
        }
    }

    fn run_fake(behaviors: Vec<Behavior>, specs: Vec<ProbeSpec>) -> RunOutcome {
        let mut target = FakeTarget::new(behaviors);
        let outcome = run(
            &mut target,
            &plan(specs),
            "2026-07-11T00:00:00Z",
            &RunnerOptions::default(),
        )
        .unwrap();
        assert!(target.ended, "runner must end the session");
        outcome
    }

    #[test]
    fn reply_is_recorded_answered() {
        let outcome = run_fake(
            vec![Behavior::Reply(b"\x1b[24;1R".to_vec())],
            vec![spec(
                "csi.dsr.cursor_position",
                b"\x1b[6n",
                ReplayClass::Safe,
            )],
        );
        let line = &outcome.report.lines[0];
        assert_eq!(line.status, ProbeStatus::Answered);
        assert_eq!(line.reply_escaped, "\\e[24;1R");
        assert_eq!(line.reply_len, 7);
        assert!(!line.echo_suspect);
        assert!(line.late_reply_escaped.is_none());
    }

    #[test]
    fn silence_is_recorded_timeout() {
        let outcome = run_fake(
            vec![Behavior::Silence],
            vec![spec(
                "osc.11.background_query",
                b"\x1b]11;?\x07",
                ReplayClass::Safe,
            )],
        );
        let line = &outcome.report.lines[0];
        assert_eq!(line.status, ProbeStatus::Timeout);
        assert_eq!(line.reply_len, 0);
    }

    #[test]
    fn garbage_is_recorded_verbatim() {
        // The runner makes no syntax claims: garbage bytes are recorded exactly as they came,
        // escaped per FORMAT.md.
        let outcome = run_fake(
            vec![Behavior::Reply(vec![0xff, 0x00, b'A'])],
            vec![spec("csi.da.primary", b"\x1b[c", ReplayClass::Safe)],
        );
        let line = &outcome.report.lines[0];
        assert_eq!(line.status, ProbeStatus::Answered);
        assert_eq!(line.reply_escaped, "\\xff\\x00A");
        assert!(!line.echo_suspect);
    }

    #[test]
    fn late_reply_attributes_to_its_own_query_not_the_next() {
        let outcome = run_fake(
            vec![
                Behavior::Late(b"\x1b[?2026;2$y".to_vec()),
                Behavior::Reply(b"\x1b[24;1R".to_vec()),
            ],
            vec![
                spec("csi.decrqm.sync", b"\x1b[?2026$p", ReplayClass::Safe),
                spec("csi.dsr.cursor_position", b"\x1b[6n", ReplayClass::Safe),
            ],
        );
        let first = &outcome.report.lines[0];
        // The deadline passed with nothing: the query stays a timeout — but the late bytes are
        // kept as data on the line they belong to.
        assert_eq!(first.status, ProbeStatus::Timeout);
        assert_eq!(first.late_reply_escaped.as_deref(), Some("\\e[?2026;2$y"));
        // And the next query's recording is clean.
        let second = &outcome.report.lines[1];
        assert_eq!(second.reply_escaped, "\\e[24;1R");
        assert!(second.late_reply_escaped.is_none());
    }

    #[test]
    fn echoed_query_is_flagged_suspect() {
        let outcome = run_fake(
            vec![Behavior::Echo],
            vec![spec("csi.da.primary", b"\x1b[c", ReplayClass::Safe)],
        );
        let line = &outcome.report.lines[0];
        // Bytes did arrive — answered is the honest wire status — but the echo flag marks the
        // reply as suspect fabrication input, and the minting layer refuses it a fixture.
        assert_eq!(line.status, ProbeStatus::Answered);
        assert!(line.echo_suspect);
    }

    #[test]
    fn identity_cross_check_verifies_and_mismatches() {
        let outcome = run_fake(vec![], vec![]);
        assert_eq!(
            outcome.identity_check,
            IdentityCheck::Verified {
                wire_name: "faketerm 9.9".to_string()
            }
        );

        let mut target = FakeTarget::new(vec![]);
        target.identity.expected_wire_name = Some("otherterm".to_string());
        let outcome = run(
            &mut target,
            &plan(vec![]),
            "2026-07-11T00:00:00Z",
            &RunnerOptions::default(),
        )
        .unwrap();
        assert_eq!(
            outcome.identity_check,
            IdentityCheck::Mismatch {
                expected: "otherterm".to_string(),
                wire_name: "faketerm 9.9".to_string()
            }
        );

        let mut target = FakeTarget::new(vec![]);
        target.xtversion_reply.clear();
        let outcome = run(
            &mut target,
            &plan(vec![]),
            "2026-07-11T00:00:00Z",
            &RunnerOptions::default(),
        )
        .unwrap();
        assert_eq!(
            outcome.identity_check,
            IdentityCheck::Unverifiable {
                reason: "target answered no XTVERSION".to_string()
            }
        );
    }

    #[test]
    fn version_prefers_xtversion_over_hint() {
        // betamax hosts ghostty: its XTVERSION says `libghostty`, which must win over the
        // `betamax --version` hint so the origin header names the emulator, not the harness.
        let reply = b"\x1bP>|libghostty\x1b\\";
        assert_eq!(
            resolve_version("betamax 0.1.15", reply),
            ("libghostty".to_string(), VersionSource::XtVersion)
        );
        assert_eq!(
            resolve_version("tmux 3.7b", b""),
            ("tmux 3.7b".to_string(), VersionSource::Hint)
        );
        assert_eq!(
            resolve_version("", b""),
            (String::new(), VersionSource::None)
        );
        assert_eq!(
            resolve_version("  ", b"garbage"),
            (String::new(), VersionSource::None)
        );
    }

    #[test]
    fn modal_and_destructive_require_their_flags() {
        let modal = vec![spec(
            "dec.mode.origin.query",
            b"\x1b[?6$p",
            ReplayClass::Modal,
        )];
        let mut target = FakeTarget::new(vec![Behavior::Silence]);
        let err = run(
            &mut target,
            &plan(modal.clone()),
            "2026-07-11T00:00:00Z",
            &RunnerOptions::default(),
        )
        .unwrap_err();
        assert!(err.contains("modal"), "unexpected error: {err}");

        // With the opt-in, the same plan runs.
        let mut target = FakeTarget::new(vec![Behavior::Silence]);
        let opts = RunnerOptions {
            allow_modal: true,
            ..RunnerOptions::default()
        };
        let outcome = run(&mut target, &plan(modal), "2026-07-11T00:00:00Z", &opts).unwrap();
        assert_eq!(outcome.report.lines.len(), 1);

        let destructive = vec![spec("dec.decslpp", b"\x1b[?24t", ReplayClass::Destructive)];
        let mut target = FakeTarget::new(vec![Behavior::Silence]);
        let err = run(
            &mut target,
            &plan(destructive),
            "2026-07-11T00:00:00Z",
            &RunnerOptions {
                allow_modal: true,
                ..RunnerOptions::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("destructive"), "unexpected error: {err}");
    }

    #[test]
    fn residue_before_any_query_is_a_stray_never_an_attribution() {
        // Bytes arriving before any query (identity stragglers) land in `strays`; once lines
        // exist they attach to the last line, appending when it already carries late bytes.
        let mut strays = Vec::new();
        attribute_residue(b"early", &mut [], &mut strays);
        assert_eq!(strays, vec!["early".to_string()]);

        let mut lines = vec![ProbeLine {
            query_id: "q".to_string(),
            reply_id: "r".to_string(),
            reply_escaped: String::new(),
            reply_len: 0,
            status: ProbeStatus::Timeout,
            late_reply_escaped: None,
            echo_suspect: false,
        }];
        attribute_residue(b"\x1b[1$y", &mut lines, &mut strays);
        attribute_residue(b"more", &mut lines, &mut strays);
        assert_eq!(lines[0].late_reply_escaped.as_deref(), Some("\\e[1$ymore"));
        assert_eq!(strays.len(), 1, "attributed residue must not also stray");
    }
}
