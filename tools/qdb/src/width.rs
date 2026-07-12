//! The width probe: the permanent, runner-integrated form of the C1 width spike (design
//! `09-width.md`). It measures how far a terminal's rendered advance for a grapheme cluster
//! deviates from the static unicode-width baseline, keyed on terminal identity and observed
//! mode-2027 (grapheme-clustering) state, and writes the db/-owned deviation table
//! `db/width/<target>.toml` that the library's `width_of` (C2) embeds.
//!
//! Method: home the cursor, print the cluster, query CPR, read the column — the advance. Repeat
//! under mode 2027 off and, where the terminal *honours* it (observed via DECRQM), on. The probe
//! **observes** 2027; it never leaves it enabled (the maintainer chose observe-only). Silence is
//! data: a cluster with no CPR reply is recorded `no_cpr`, never guessed.
//!
//! The live I/O (feed/drain against a `Target`) is thin; the corpus, CPR/XTVERSION parsing, and
//! TOML rendering are pure and unit-tested without a terminal.

use std::fmt::Write as _;
use std::time::Duration;

use crate::targets::Target;

/// One corpus cluster: a stable id, the bytes to print, and the compiled unicode-width baseline
/// (hand-recorded from the Unicode 15 tables so the row is transparent about what it deviates
/// from).
pub struct Cluster {
    /// Stable id for the db row.
    pub id: &'static str,
    /// The cluster's scalar values.
    pub text: &'static str,
    /// The static unicode-width sum for `text`.
    pub uw: usize,
}

/// The classic width-disagreement corpus (C1 spec's set): ASCII, CJK, emoji (single, ZWJ, skin
/// tone, flag), combining marks, VS15/VS16, and zero-width. The `uw` column is the static baseline.
pub const CORPUS: &[Cluster] = &[
    Cluster {
        id: "ascii-a",
        text: "A",
        uw: 1,
    },
    Cluster {
        id: "cjk-zhong",
        text: "中",
        uw: 2,
    },
    Cluster {
        id: "hangul-han",
        text: "한",
        uw: 2,
    },
    Cluster {
        id: "emoji-grinning",
        text: "😀",
        uw: 2,
    },
    Cluster {
        id: "emoji-zwj-family",
        text: "👨‍👩‍👧‍👦",
        uw: 8,
    },
    Cluster {
        id: "emoji-skin-tone",
        text: "👍🏽",
        uw: 4,
    },
    Cluster {
        id: "regional-flag-us",
        text: "🇺🇸",
        uw: 2,
    },
    Cluster {
        id: "combining-e-acute",
        text: "e\u{0301}",
        uw: 1,
    },
    Cluster {
        id: "heart-vs16",
        text: "❤\u{FE0F}",
        uw: 2,
    },
    Cluster {
        id: "heart-vs15",
        text: "❤\u{FE0E}",
        uw: 1,
    },
    Cluster {
        id: "heart-bare",
        text: "❤",
        uw: 1,
    },
    Cluster {
        id: "zero-width-space",
        text: "\u{200B}",
        uw: 0,
    },
];

/// Deadlines for the CPR/DECRQM drains — the runner's proven values.
const FIRST_BYTE: Duration = Duration::from_millis(400);
const SETTLE: Duration = Duration::from_millis(120);

/// One cluster's measurement: the baseline plus the advance under 2027 off and (optionally) on.
/// `None` advance means no CPR came back — recorded, never guessed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Measurement {
    /// The cluster id.
    pub id: &'static str,
    /// The cluster's scalar values.
    pub text: &'static str,
    /// The static unicode-width baseline.
    pub uw: usize,
    /// Measured advance with 2027 off/unset.
    pub advance: Option<usize>,
    /// Measured advance with 2027 on — `Some` only when the terminal honours 2027.
    pub advance_2027: Option<Option<usize>>,
}

/// Run metadata plus the per-cluster measurements for one target.
#[derive(Clone, Debug)]
pub struct WidthReport {
    /// Target slug.
    pub target: String,
    /// Terminal version string (XTVERSION or the adapter hint).
    pub version: String,
    /// How `version` was obtained.
    pub version_source: String,
    /// Adapter kind.
    pub adapter: String,
    /// Run timestamp.
    pub captured: String,
    /// Runner build.
    pub runner: String,
    /// Session columns/rows.
    pub cols: u16,
    /// Session rows.
    pub rows: u16,
    /// Whether the terminal recognises mode 2027 (DECRQM).
    pub supports_2027: bool,
    /// The per-cluster measurements.
    pub measurements: Vec<Measurement>,
}

/// Drains one reply: wait for the first byte, then settle, so a multi-byte report arrives whole.
fn drain(target: &mut dyn Target) -> Result<Vec<u8>, String> {
    let mut reply = target.drain_output(Some(FIRST_BYTE))?;
    if reply.is_empty() {
        return Ok(reply);
    }
    loop {
        let more = target.drain_output(Some(SETTLE))?;
        if more.is_empty() {
            return Ok(reply);
        }
        reply.extend_from_slice(&more);
    }
}

/// Extracts the self-reported name from an XTVERSION reply `ESC P > | <name> ESC \`.
#[must_use]
pub fn parse_xtversion(reply: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(reply);
    let start = s.find(">|")?;
    let after = &s[start + 2..];
    let end = after.find('\u{1b}').unwrap_or(after.len());
    let name = after[..end].trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Parses the column out of a CPR reply `ESC [ <row> ; <col> R`. Returns `None` if absent.
#[must_use]
pub fn cpr_column(reply: &[u8]) -> Option<usize> {
    let s = String::from_utf8_lossy(reply);
    let start = s.find("\u{1b}[")?;
    let rest = &s[start + 2..];
    let end = rest.find('R')?;
    rest[..end].split(';').nth(1)?.trim().parse().ok()
}

/// Detects whether a DECRQM `CSI ? 2027 ; Ps $ y` reply means 2027 is recognised (`Ps` 1 or 2).
#[must_use]
pub fn decrqm_recognised(reply: &[u8]) -> bool {
    let s = String::from_utf8_lossy(reply);
    s.contains("2027;1$y") || s.contains("2027;2$y")
}

/// Measures the rendered advance of `text`: home, print, CPR, `col - 1`. `None` if no CPR.
fn measure_advance(target: &mut dyn Target, text: &str) -> Result<Option<usize>, String> {
    target.feed(b"\x1b[H")?;
    let _ = drain(target); // discard any stray
    target.feed(text.as_bytes())?;
    target.feed(b"\x1b[6n")?;
    let reply = drain(target)?;
    Ok(cpr_column(&reply).map(|col| col.saturating_sub(1)))
}

/// Runs the whole corpus against an already-started target under the current mode state.
fn measure_corpus(target: &mut dyn Target) -> Result<Vec<Option<usize>>, String> {
    CORPUS
        .iter()
        .map(|c| measure_advance(target, c.text))
        .collect()
}

/// Drives one target through the width probe and returns its [`WidthReport`]. Observes 2027 (never
/// leaves it enabled).
///
/// # Errors
///
/// Returns an error if the target cannot start or the transport dies.
pub fn probe(
    target: &mut dyn Target,
    slug: &str,
    adapter: &str,
    hint: &str,
    timestamp: &str,
    cols: u16,
    rows: u16,
) -> Result<WidthReport, String> {
    target.start(cols, rows)?;

    target.feed(b"\x1b[>q")?;
    let xtversion = drain(target)?;
    let (version, version_source) = match parse_xtversion(&xtversion) {
        Some(name) => (name, "xtversion".to_string()),
        None if !hint.trim().is_empty() => (hint.trim().to_string(), "hint".to_string()),
        None => (String::new(), "none".to_string()),
    };

    target.feed(b"\x1b[?2027$p")?;
    let supports_2027 = decrqm_recognised(&drain(target)?);

    // Default state (2027 off / unset).
    target.feed(b"\x1b[?2027l")?;
    let _ = drain(target);
    let off = measure_corpus(target)?;

    // 2027 on, only where recognised; restore off afterward (observe-only).
    let on = if supports_2027 {
        target.feed(b"\x1b[?2027h")?;
        let _ = drain(target);
        let rows = measure_corpus(target)?;
        target.feed(b"\x1b[?2027l")?;
        let _ = drain(target);
        Some(rows)
    } else {
        None
    };

    let _ = target.end();

    let measurements = CORPUS
        .iter()
        .enumerate()
        .map(|(i, c)| Measurement {
            id: c.id,
            text: c.text,
            uw: c.uw,
            advance: off[i],
            advance_2027: on.as_ref().map(|o| o[i]),
        })
        .collect();

    Ok(WidthReport {
        target: slug.to_string(),
        version,
        version_source,
        adapter: adapter.to_string(),
        captured: timestamp.to_string(),
        runner: format!("qdb {}", env!("CARGO_PKG_VERSION")),
        cols,
        rows,
        supports_2027,
        measurements,
    })
}

/// Encodes `s` as a TOML basic string. Rust's `{:?}` would emit `\u{XXXX}` escapes that TOML
/// rejects; TOML wants literal Unicode with only `"`, `\`, and control chars escaped (control chars
/// as `\uXXXX`, four hex). Emoji, ZWJ, variation selectors, and combining marks stay literal.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Renders a [`WidthReport`] as the `db/width/<target>.toml` deviation table.
#[must_use]
pub fn render_table(report: &WidthReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Width deviation table for {} — generated by qdb width-probe, do not hand-edit.",
        report.target
    );
    let _ = writeln!(
        out,
        "# Measured rendered advance vs the static unicode-width baseline (design 09-width)."
    );
    let _ = writeln!(out, "target = {:?}", report.target);
    let _ = writeln!(out, "version = {:?}", report.version);
    let _ = writeln!(out, "version_source = {:?}", report.version_source);
    let _ = writeln!(out, "adapter = {:?}", report.adapter);
    let _ = writeln!(out, "captured = {:?}", report.captured);
    let _ = writeln!(out, "runner = {:?}", report.runner);
    let _ = writeln!(
        out,
        "geometry = {{ cols = {}, rows = {} }}",
        report.cols, report.rows
    );
    let _ = writeln!(out, "supports_2027 = {}", report.supports_2027);
    for m in &report.measurements {
        let _ = writeln!(out);
        let _ = writeln!(out, "[[cluster]]");
        let _ = writeln!(out, "id = {:?}", m.id);
        let _ = writeln!(out, "text = {}", toml_string(m.text));
        let _ = writeln!(out, "unicode_width = {}", m.uw);
        match m.advance {
            Some(a) => {
                let _ = writeln!(out, "advance = {a}");
            }
            None => {
                let _ = writeln!(out, "no_cpr = true");
            }
        }
        if let Some(on) = m.advance_2027 {
            match on {
                Some(a) => {
                    let _ = writeln!(out, "advance_2027 = {a}");
                }
                None => {
                    let _ = writeln!(out, "no_cpr_2027 = true");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpr_column_parses_the_column() {
        assert_eq!(cpr_column(b"\x1b[1;3R"), Some(3));
        assert_eq!(cpr_column(b"junk\x1b[24;120Rmore"), Some(120));
        assert_eq!(cpr_column(b"no cpr here"), None);
        assert_eq!(cpr_column(b""), None);
    }

    #[test]
    fn xtversion_extracts_the_name() {
        assert_eq!(
            parse_xtversion(b"\x1bP>|tmux 3.7b\x1b\\").as_deref(),
            Some("tmux 3.7b")
        );
        assert_eq!(parse_xtversion(b"\x1bP>|\x1b\\"), None); // empty name
        assert_eq!(parse_xtversion(b""), None);
    }

    #[test]
    fn decrqm_recognises_set_and_reset() {
        assert!(decrqm_recognised(b"\x1b[?2027;1$y"));
        assert!(decrqm_recognised(b"\x1b[?2027;2$y"));
        assert!(!decrqm_recognised(b"\x1b[?2027;0$y")); // not recognised
        assert!(!decrqm_recognised(b"")); // silence
    }

    fn report(supports_2027: bool, measurements: Vec<Measurement>) -> WidthReport {
        WidthReport {
            target: "tmux".to_string(),
            version: "tmux 3.7b".to_string(),
            version_source: "xtversion".to_string(),
            adapter: "pty-headless".to_string(),
            captured: "2026-07-12T00:00:00Z".to_string(),
            runner: "qdb 0.0.0".to_string(),
            cols: 120,
            rows: 40,
            supports_2027,
            measurements,
        }
    }

    #[test]
    fn render_table_emits_metadata_and_rows() {
        let r = report(
            false,
            vec![
                Measurement {
                    id: "ascii-a",
                    text: "A",
                    uw: 1,
                    advance: Some(1),
                    advance_2027: None,
                },
                Measurement {
                    id: "emoji-zwj-family",
                    text: "👨‍👩‍👧‍👦",
                    uw: 8,
                    advance: None, // no CPR — recorded, not guessed
                    advance_2027: None,
                },
            ],
        );
        let toml = render_table(&r);
        assert!(toml.contains("target = \"tmux\""));
        assert!(toml.contains("supports_2027 = false"));
        assert!(toml.contains("id = \"ascii-a\""));
        assert!(toml.contains("advance = 1"));
        assert!(toml.contains("no_cpr = true")); // the silent cluster
        // Parseable and shaped as expected.
        let parsed: toml::Value = toml::from_str(&toml).unwrap();
        assert_eq!(parsed["cluster"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn render_table_includes_2027_column_when_supported() {
        let r = report(
            true,
            vec![Measurement {
                id: "emoji-zwj-family",
                text: "👨‍👩‍👧‍👦",
                uw: 8,
                advance: Some(8),
                advance_2027: Some(Some(2)),
            }],
        );
        let toml = render_table(&r);
        assert!(toml.contains("supports_2027 = true"));
        assert!(toml.contains("advance = 8"));
        assert!(toml.contains("advance_2027 = 2"));
    }
}
