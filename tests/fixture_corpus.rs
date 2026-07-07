//! Corpus-driven acceptance test over the fixture corpus: host->terminal commands and queries, plus
//! the terminal->host reply fixtures the live-capture harness mints.
//!
//! This walks `fixtures/**/*.seq` at test time (no build script), parses each fixture's header,
//! unescapes its payload with the same rules the normative `fixtures/FORMAT.md` documents, and
//! feeds every fixture through [`qwertty::SyntaxParser`]. For each fixture it asserts:
//!
//! 1. **Reconstruction.** Concatenating the raw bytes of the emitted tokens reproduces the
//!    unescaped input byte-for-byte, and no token dropped payload bytes (these are short command
//!    fixtures that must never truncate at the default payload limit).
//! 2. **Split-equivalence.** Feeding the input one byte at a time yields the same tokens as feeding
//!    it whole.
//! 3. **Well-formedness.** No emitted token is [`SyntaxToken::Malformed`]; every fixture is a
//!    well-formed sequence, whichever direction it travels.
//!
//! The `.seq` file format is: a `#!` header line, then the escaped payload, then a single trailing
//! LF that is a file-format artifact (not sequence payload). See `fixtures/FORMAT.md`.

use std::fs;
use std::path::{Path, PathBuf};

use qwertty::{SyntaxParser, SyntaxToken};

/// Parses two ASCII hex digits into a byte, returning `None` if either is not a hex digit.
fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let hi = (hi as char).to_digit(16)?;
    let lo = (lo as char).to_digit(16)?;
    u8::try_from(hi * 16 + lo).ok()
}

/// Unescapes the fixture escaped-text encoding, mirroring the reference routine in `FORMAT.md`.
///
/// Rules: `\e` -> ESC (`0x1b`), `\xNN` -> the byte with hex value `NN`, `\\` -> a single
/// backslash, and any other byte is copied literally. A trailing backslash with no following
/// byte is copied literally.
fn unescape(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\\' && i + 1 < input.len() {
            match input[i + 1] {
                b'e' => {
                    out.push(0x1b);
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'x' if i + 3 < input.len() && hex_byte(input[i + 2], input[i + 3]).is_some() => {
                    out.push(hex_byte(input[i + 2], input[i + 3]).expect("hex checked in guard"));
                    i += 4;
                }
                _ => {
                    out.push(input[i]);
                    i += 1;
                }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

/// A parsed fixture: its path (for diagnostics) and its unescaped sequence bytes.
struct Fixture {
    name: String,
    bytes: Vec<u8>,
}

/// Splits a `.seq` file into its header line and unescaped payload.
///
/// The header is the first line (up to and including the first LF). The remainder is the escaped
/// payload followed by exactly one trailing LF file terminator, which is stripped before
/// unescaping.
fn parse_fixture(path: &Path, raw: &[u8]) -> Fixture {
    let name = path.display().to_string();
    let newline = raw
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or_else(|| panic!("{name}: fixture has no header line"));
    let header = &raw[..newline];
    // Both directions live in the corpus now: host-to-terminal commands/queries, and the
    // terminal-to-host reply fixtures the live-capture harness mints (`origin=capture:`). Reply
    // bytes are equally valid, well-formed sequences the syntax layer must round-trip, so the
    // corpus assertions below apply to both; only the header's direction may differ.
    assert!(
        header.starts_with(b"#! direction=host-to-terminal origin=")
            || header.starts_with(b"#! direction=terminal-to-host origin="),
        "{name}: unexpected header {:?}",
        String::from_utf8_lossy(header),
    );
    let mut body = &raw[newline + 1..];
    // Strip the single trailing LF file-format artifact (not sequence payload).
    if body.last() == Some(&b'\n') {
        body = &body[..body.len() - 1];
    }
    Fixture {
        name,
        bytes: unescape(body),
    }
}

/// Recursively collects every `*.seq` file under `dir`.
fn collect_seq_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_seq_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "seq") {
            out.push(path);
        }
    }
}

/// Loads every fixture in the corpus, sorted by path for stable iteration.
fn load_corpus() -> Vec<Fixture> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let mut paths = Vec::new();
    collect_seq_files(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no fixtures found under {}",
        root.display(),
    );
    paths
        .iter()
        .map(|p| {
            let raw = fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            parse_fixture(p, &raw)
        })
        .collect()
}

/// Tokenizes `input` whole (feed then finish).
fn tokenize_whole(input: &[u8]) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(input);
    tokens.extend(parser.finish());
    tokens
}

/// Tokenizes `input` one byte at a time (feed each byte, then finish).
fn tokenize_byte_at_a_time(input: &[u8]) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::new();
    let mut tokens = Vec::new();
    for &byte in input {
        tokens.extend(parser.feed(&[byte]));
    }
    tokens.extend(parser.finish());
    tokens
}

/// Concatenates the raw bytes of every token.
fn concat_bytes(tokens: &[SyntaxToken]) -> Vec<u8> {
    let mut out = Vec::new();
    for token in tokens {
        out.extend_from_slice(token.as_bytes());
    }
    out
}

/// Returns the number of payload bytes any token counted and dropped past the payload bound.
///
/// Only string sequences (OSC, DCS, APC, PM, SOS) can drop bytes, and only when truncated.
fn dropped_bytes(tokens: &[SyntaxToken]) -> usize {
    tokens
        .iter()
        .map(|token| match token {
            SyntaxToken::Osc(s)
            | SyntaxToken::Dcs(s)
            | SyntaxToken::Apc(s)
            | SyntaxToken::Pm(s)
            | SyntaxToken::Sos(s) => s.dropped_bytes(),
            _ => 0,
        })
        .sum()
}

/// Reconstruction: token bytes concatenate back to the input, with nothing dropped.
#[test]
fn corpus_reconstructs_losslessly() {
    for fixture in load_corpus() {
        let tokens = tokenize_whole(&fixture.bytes);
        assert_eq!(
            concat_bytes(&tokens),
            fixture.bytes,
            "{}: reconstruction mismatch",
            fixture.name,
        );
        assert_eq!(
            dropped_bytes(&tokens),
            0,
            "{}: a token dropped payload bytes at the default limit",
            fixture.name,
        );
    }
}

/// Split-equivalence: byte-at-a-time tokenization matches whole tokenization.
#[test]
fn corpus_split_equivalent() {
    for fixture in load_corpus() {
        let whole = tokenize_whole(&fixture.bytes);
        let split = tokenize_byte_at_a_time(&fixture.bytes);
        assert_eq!(whole, split, "{}: split-equivalence mismatch", fixture.name);
    }
}

/// Well-formedness: no fixture tokenizes as Malformed.
#[test]
fn corpus_has_no_malformed_tokens() {
    for fixture in load_corpus() {
        let tokens = tokenize_whole(&fixture.bytes);
        for token in &tokens {
            assert!(
                !matches!(token, SyntaxToken::Malformed(_)),
                "{}: produced a Malformed token: {token:?}",
                fixture.name,
            );
        }
    }
}
