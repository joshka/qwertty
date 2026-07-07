//! Invariant acceptance suites for the syntax tokenizer (design 02 invariants 1-6).
//!
//! Three properties are proven over a shared corpus of the 12-case escape-layer spike inputs plus
//! a battery of adversarial inputs:
//!
//! 1. Reconstruction: concatenating token bytes reproduces the input, except truncated string
//!    payloads, which instead account for the difference through their dropped-byte count.
//! 2. Split-equivalence: every 1-split, byte-at-a-time, and a few 3-chunk splits yield the same
//!    tokens as feeding the input whole.
//! 3. Bounds: a small payload limit truncates long payloads with the correct dropped count, parsing
//!    resumes cleanly, and no input panics.

use qwertty::{SyntaxParser, SyntaxToken};

/// Feeds an input whole and flushes, returning the complete token sequence.
fn tokenize_whole(input: &[u8], limit: usize) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::with_payload_limit(limit);
    let mut tokens = parser.feed(input);
    tokens.extend(parser.finish());
    tokens
}

/// Feeds an input as the given chunk sizes and flushes, returning the complete token sequence.
fn tokenize_chunks(input: &[u8], chunks: &[&[u8]], limit: usize) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::with_payload_limit(limit);
    let mut tokens = Vec::new();
    for chunk in chunks {
        tokens.extend(parser.feed(chunk));
    }
    let _ = input;
    tokens.extend(parser.finish());
    tokens
}

/// The 12-case escape-layer spike corpus, exact bytes.
fn spike_corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut large_osc = Vec::from(*b"\x1b]1337;File=");
    large_osc.extend(std::iter::repeat_n(b'a', 8192));
    large_osc.extend_from_slice(b"\x1b\\");

    vec![
        ("partial_utf8", b"\xf0\x90\x8c\xbc".to_vec()),
        ("partial_csi", b"\x1b[31m".to_vec()),
        ("colon_sgr", b"\x1b[4:3m".to_vec()),
        (
            "bracketed_paste",
            b"\x1b[200~hello\nworld\x1b[201~".to_vec(),
        ),
        ("focus", b"\x1b[I\x1b[O".to_vec()),
        ("sgr_mouse", b"\x1b[<0;10;20M".to_vec()),
        ("kitty_keyboard", b"\x1b[27;5;65u".to_vec()),
        (
            "osc8_hyperlink",
            b"\x1b]8;;https://example.com\x1b\\label\x1b]8;;\x1b\\".to_vec(),
        ),
        ("osc52_clipboard", b"\x1b]52;c;SGVsbG8=\x07".to_vec()),
        ("large_osc", large_osc),
        ("dcs", b"\x1bP1$r q\x1b\\".to_vec()),
        ("malformed", b"\x1b[?bad\x1b]unterminated".to_vec()),
    ]
}

/// Adversarial inputs beyond the spike corpus, exercising every family boundary.
fn adversarial_corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut forty_param = Vec::from(*b"\x1b[");
    for i in 0..40 {
        if i > 0 {
            forty_param.push(b';');
        }
        forty_param.push(b'1');
    }
    forty_param.push(b'm');

    vec![
        ("lone_c1_csi", b"\x9b31m".to_vec()),
        ("lone_c1_osc", b"\x9d0;t\x9c".to_vec()),
        ("lone_c1_st_stray", b"a\x9cb".to_vec()),
        ("lone_c1_dcs", b"\x90q\x1b\\".to_vec()),
        ("lone_c1_apc", b"\x9fpayload\x1b\\".to_vec()),
        ("lone_c1_pm", b"\x9emsg\x9c".to_vec()),
        ("lone_c1_sos", b"\x98str\x9c".to_vec()),
        ("esc_at_eof", b"abc\x1b".to_vec()),
        ("can_aborted_csi", b"\x1b[31\x18rest".to_vec()),
        ("sub_aborted_osc", b"\x1b]0;ti\x1atail".to_vec()),
        ("unterminated_osc_then_text", b"\x1b]0;never-ends".to_vec()),
        ("interleaved_utf8_esc", "café\x1b[1mné".as_bytes().to_vec()),
        ("c1_st_terminated_osc", b"\x1b]0;title\x9crest".to_vec()),
        ("empty_params_csi", b"\x1b[;;H".to_vec()),
        ("forty_param_csi", forty_param),
        ("subparam_heavy_sgr", b"\x1b[38:2:1:2:3:4m".to_vec()),
        ("apc_sequence", b"\x1b_Gf=100\x1b\\".to_vec()),
        ("pm_sequence", b"\x1b^privacy\x1b\\".to_vec()),
        ("sos_sequence", b"\x1bXstart\x1b\\".to_vec()),
        ("plain_escape_reset", b"\x1bc".to_vec()),
        ("charset_escape", b"\x1b(B".to_vec()),
        ("bare_esc_then_can", b"\x1b\x18".to_vec()),
        ("control_run", b"a\x00\x01\x7fb".to_vec()),
        ("all_bytes", (0u8..=255).collect()),
    ]
}

fn full_corpus() -> Vec<(&'static str, Vec<u8>)> {
    let mut corpus = spike_corpus();
    corpus.extend(adversarial_corpus());
    corpus
}

/// Reconstructs the input bytes accounting for truncated string payloads.
///
/// For a truncated string token, the retained raw bytes are missing `dropped_bytes` payload bytes;
/// this helper reinserts that many placeholder bytes so the length and structure line up with the
/// original input. The reconstruction invariant is byte-exact for a limit large enough that nothing
/// truncates, and length-exact (with the token accounting for the gap) otherwise.
fn reconstruct(tokens: &[SyntaxToken], filler: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for token in tokens {
        out.extend_from_slice(token.as_bytes());
        if let Some(dropped) = dropped_bytes(token) {
            // Reinsert the dropped payload bytes just before the terminator to restore length.
            let terminator_len = token.as_bytes().len() - string_prefix_and_kept_payload_len(token);
            let insert_at = out.len() - terminator_len;
            let fill: Vec<u8> = filler.iter().copied().cycle().take(dropped).collect();
            out.splice(insert_at..insert_at, fill);
        }
    }
    out
}

/// Returns the dropped-byte count for a truncated string token, or `None`.
fn dropped_bytes(token: &SyntaxToken) -> Option<usize> {
    let (SyntaxToken::Osc(string)
    | SyntaxToken::Dcs(string)
    | SyntaxToken::Apc(string)
    | SyntaxToken::Pm(string)
    | SyntaxToken::Sos(string)) = token
    else {
        return None;
    };
    string.truncated().then(|| string.dropped_bytes())
}

/// Length of a string token's raw bytes minus its terminator (prefix + kept payload).
fn string_prefix_and_kept_payload_len(token: &SyntaxToken) -> usize {
    let (SyntaxToken::Osc(string)
    | SyntaxToken::Dcs(string)
    | SyntaxToken::Apc(string)
    | SyntaxToken::Pm(string)
    | SyntaxToken::Sos(string)) = token
    else {
        return token.as_bytes().len();
    };
    token.as_bytes().len() - string.terminator().as_bytes().len()
}

#[test]
fn reconstruction_is_byte_exact_without_truncation() {
    // A limit larger than every corpus payload means no truncation, so reconstruction is exact.
    let limit = 1 << 20;
    for (name, input) in full_corpus() {
        let tokens = tokenize_whole(&input, limit);
        for token in &tokens {
            assert!(
                dropped_bytes(token).is_none(),
                "{name}: unexpected truncation with a large limit"
            );
        }
        let rebuilt: Vec<u8> = tokens.iter().flat_map(|t| t.as_bytes().to_vec()).collect();
        assert_eq!(rebuilt, input, "reconstruction failed for {name}");
    }
}

#[test]
fn reconstruction_accounts_for_truncated_payloads() {
    // A tiny limit truncates the large payloads; the dropped count must restore the length.
    let limit = 8;
    for (name, input) in full_corpus() {
        let tokens = tokenize_whole(&input, limit);
        // Use a filler byte that appears in the truncated payloads ('a' for large_osc).
        let rebuilt = reconstruct(&tokens, b"a");
        assert_eq!(rebuilt.len(), input.len(), "length mismatch for {name}");
    }
}

#[test]
fn split_equivalence_over_all_one_splits() {
    let limit = 1 << 20;
    for (name, input) in full_corpus() {
        let whole = tokenize_whole(&input, limit);
        for split in 0..=input.len() {
            let (head, tail) = input.split_at(split);
            let chunked = tokenize_chunks(&input, &[head, tail], limit);
            assert_eq!(chunked, whole, "one-split at {split} differs for {name}");
        }
    }
}

#[test]
fn split_equivalence_byte_at_a_time() {
    let limit = 1 << 20;
    for (name, input) in full_corpus() {
        let whole = tokenize_whole(&input, limit);
        let single: Vec<&[u8]> = input.chunks(1).collect();
        let chunked = tokenize_chunks(&input, &single, limit);
        assert_eq!(chunked, whole, "byte-at-a-time differs for {name}");
    }
}

#[test]
fn split_equivalence_three_chunk_splits() {
    let limit = 1 << 20;
    for (name, input) in full_corpus() {
        if input.len() < 3 {
            continue;
        }
        let whole = tokenize_whole(&input, limit);
        // A deterministic spread of 3-chunk split positions.
        let len = input.len();
        for (a, b) in [
            (1, 2),
            (len / 3, 2 * len / 3),
            (1, len - 1),
            (len / 2, len - 1),
        ] {
            let a = a.min(len);
            let b = b.clamp(a, len);
            let chunks = [&input[..a], &input[a..b], &input[b..]];
            let chunked = tokenize_chunks(&input, &chunks, limit);
            assert_eq!(chunked, whole, "3-chunk split ({a},{b}) differs for {name}");
        }
    }
}

#[test]
fn bounds_truncate_long_osc_with_correct_dropped_count() {
    let limit = 8;
    let mut input = Vec::from(*b"\x1b]0;");
    input.extend(std::iter::repeat_n(b'z', 100));
    input.extend_from_slice(b"\x07");
    // Trailing text after the terminator proves parsing resumes cleanly.
    input.extend_from_slice(b"after");

    let tokens = tokenize_whole(&input, limit);
    let SyntaxToken::Osc(osc) = &tokens[0] else {
        panic!("expected Osc, got {:?}", tokens[0]);
    };
    assert!(osc.truncated());
    // Payload is "0;" + 100 z = 102 bytes; kept prefix is `limit`, dropped is the rest.
    assert_eq!(osc.payload().len(), limit);
    assert_eq!(osc.dropped_bytes(), 102 - limit);
    // Parsing resumes: the next token is the trailing text.
    assert_eq!(tokens[1], SyntaxToken::Text(b"after".to_vec()));
}

#[test]
fn bounds_truncate_unterminated_osc_at_finish() {
    let limit = 8;
    let mut input = Vec::from(*b"\x1b]0;");
    input.extend(std::iter::repeat_n(b'z', 100));
    // No terminator: flushed by finish(), still bounded.

    let tokens = tokenize_whole(&input, limit);
    assert_eq!(tokens.len(), 1);
    let SyntaxToken::Osc(osc) = &tokens[0] else {
        panic!("expected Osc, got {:?}", tokens[0]);
    };
    assert!(osc.truncated());
    assert_eq!(osc.payload().len(), limit);
    assert_eq!(osc.dropped_bytes(), 102 - limit);
    assert_eq!(osc.terminator(), qwertty::StringTerminator::None);
}

#[test]
fn bounds_hold_parser_memory_for_unterminated_osc_stream() {
    // A 1 MiB unterminated OSC payload fed in 4 KiB chunks must never grow parser memory past
    // the default 64 KiB bound plus a small constant (the introducer).
    let limit = qwertty::DEFAULT_PAYLOAD_LIMIT;
    let mut parser = SyntaxParser::new();
    assert!(parser.feed(b"\x1b]").is_empty());

    let chunk = vec![b'A'; 4096];
    for _ in 0..256 {
        assert!(parser.feed(&chunk).is_empty());
        assert!(
            parser.pending_bytes().len() <= limit + 16,
            "pending grew past the bound: {}",
            parser.pending_bytes().len()
        );
    }

    let tokens = parser.finish();
    assert_eq!(tokens.len(), 1);
    let SyntaxToken::Osc(osc) = &tokens[0] else {
        panic!("expected Osc, got {:?}", tokens[0]);
    };
    assert!(osc.truncated());
    assert_eq!(osc.payload().len(), limit);
    assert_eq!(osc.dropped_bytes(), 256 * 4096 - limit);
    assert_eq!(osc.terminator(), qwertty::StringTerminator::None);
}

#[test]
fn bounds_hold_when_terminator_straddles_chunks_after_overflow() {
    // Same stream, but the `ESC \` terminator arrives split across two chunks after the bound
    // was exceeded: the token still carries the terminator and the exact dropped count.
    let limit = qwertty::DEFAULT_PAYLOAD_LIMIT;
    let mut parser = SyntaxParser::new();
    assert!(parser.feed(b"\x1b]").is_empty());

    let chunk = vec![b'A'; 4096];
    for _ in 0..256 {
        assert!(parser.feed(&chunk).is_empty());
    }

    // The ESC lands alone at a chunk boundary; memory stays bounded while it is held.
    assert!(parser.feed(b"\x1b").is_empty());
    assert!(parser.pending_bytes().len() <= limit + 16);

    let tokens = parser.feed(b"\\");
    assert_eq!(tokens.len(), 1);
    let SyntaxToken::Osc(osc) = &tokens[0] else {
        panic!("expected Osc, got {:?}", tokens[0]);
    };
    assert_eq!(osc.terminator(), qwertty::StringTerminator::EscBackslash);
    assert_eq!(osc.dropped_bytes(), 256 * 4096 - limit);

    // Reconstruction accounting: retained raw bytes plus the dropped count restore the input
    // length (introducer + payload + terminator).
    let accounted = osc.as_bytes().len() + osc.dropped_bytes();
    assert_eq!(accounted, 2 + 256 * 4096 + 2);
    assert!(parser.finish().is_empty());
}

#[test]
fn bounds_cap_pathological_csi_parameters() {
    // `ESC [` followed by an endless parameter run must not grow parser memory without limit.
    // At the cap the sequence stops being CSI syntax: the retained prefix is emitted as
    // Malformed (nothing dropped) and the remaining bytes reparse as ordinary text.
    let limit = 64;
    let mut parser = SyntaxParser::with_payload_limit(limit);
    let mut input = Vec::from(*b"\x1b[");
    let mut tokens = parser.feed(b"\x1b[");

    let chunk = [b'7'; 16];
    for _ in 0..16 {
        input.extend_from_slice(&chunk);
        tokens.extend(parser.feed(&chunk));
        assert!(
            parser.pending_bytes().len() <= limit + 16,
            "pending grew past the bound: {}",
            parser.pending_bytes().len()
        );
    }
    tokens.extend(parser.finish());

    assert_eq!(
        tokens[0],
        SyntaxToken::Malformed(input[..2 + limit].to_vec())
    );
    let rebuilt: Vec<u8> = tokens.iter().flat_map(|t| t.as_bytes().to_vec()).collect();
    assert_eq!(rebuilt, input, "CSI cap must stay reconstruction-exact");
}

#[test]
fn no_input_panics() {
    // Every corpus input at several limits, whole and split, must not panic.
    for limit in [0usize, 1, 8, 64, 1 << 20] {
        for (_name, input) in full_corpus() {
            let _ = tokenize_whole(&input, limit);
            for split in 0..=input.len() {
                let (head, tail) = input.split_at(split);
                let _ = tokenize_chunks(&input, &[head, tail], limit);
            }
        }
    }
}
