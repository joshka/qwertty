//! Unit tests for the syntax tokenizer.
//!
//! The exhaustive reconstruction, split-equivalence, and bounds suites live in `tests/syntax.rs`.
//! These unit tests pin the token shapes and structured accessors for representative inputs.

use super::{
    ControlParams, ParamSeparator, StringKind, StringTerminator, SyntaxParser, SyntaxToken,
};

fn tokens(bytes: &[u8]) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::new();
    let mut all = parser.feed(bytes);
    all.extend(parser.finish());
    all
}

#[test]
fn text_run_is_maximal_and_utf8() {
    assert_eq!(tokens(b"hello"), vec![SyntaxToken::Text(b"hello".to_vec())]);
    // Multibyte UTF-8 stays inside one text token.
    assert_eq!(
        tokens("héllo".as_bytes()),
        vec![SyntaxToken::Text("héllo".as_bytes().to_vec())]
    );
}

#[test]
fn control_bytes_are_single_tokens() {
    assert_eq!(
        tokens(b"a\r\x03b"),
        vec![
            SyntaxToken::Text(b"a".to_vec()),
            SyntaxToken::Control(b'\r'),
            SyntaxToken::Control(0x03),
            SyntaxToken::Text(b"b".to_vec()),
        ]
    );
}

#[test]
fn esc_is_never_a_control_token() {
    // A lone trailing ESC becomes an Esc token with no final byte, not a Control.
    let toks = tokens(b"\x1b");
    match &toks[0] {
        SyntaxToken::Esc(escape) => {
            assert_eq!(escape.as_bytes(), b"\x1b");
            assert_eq!(escape.final_byte(), None);
        }
        other => panic!("expected Esc, got {other:?}"),
    }
}

#[test]
fn csi_exposes_params_intermediates_and_final() {
    let toks = tokens(b"\x1b[?25;5h");
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi, got {:?}", toks[0]);
    };
    assert_eq!(csi.as_bytes(), b"\x1b[?25;5h");
    let params = csi.params();
    assert_eq!(params.private_markers(), b"?");
    assert_eq!(params.param_bytes(), b"25;5");
    assert_eq!(params.final_byte(), b'h');
    assert_eq!(params.params().len(), 2);
    assert_eq!(params.params()[0].value(), Some(25));
    assert_eq!(
        params.params()[1].separator(),
        Some(ParamSeparator::Semicolon)
    );
    assert!(!params.params_overflowed());
}

#[test]
fn colon_and_semicolon_separators_are_distinguished() {
    let toks = tokens(b"\x1b[38:2:1:2:3m");
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi");
    };
    let params = csi.params().params();
    assert_eq!(params[0].separator(), None);
    assert_eq!(params[1].separator(), Some(ParamSeparator::Colon));
    assert_eq!(params[2].separator(), Some(ParamSeparator::Colon));
    // The colon SGR example `4:3` keeps its colon.
    let toks = tokens(b"\x1b[4:3m");
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi");
    };
    assert_eq!(
        csi.params().params()[1].separator(),
        Some(ParamSeparator::Colon)
    );
}

#[test]
fn empty_params_are_defaulted_values() {
    let toks = tokens(b"\x1b[;5H");
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi");
    };
    let params = csi.params().params();
    assert_eq!(params[0].value(), None);
    assert_eq!(params[1].value(), Some(5));
}

#[test]
fn param_overflow_is_flagged_not_merged() {
    let mut input = Vec::from(*b"\x1b[");
    for i in 0..40 {
        if i > 0 {
            input.push(b';');
        }
        input.push(b'1');
    }
    input.push(b'm');
    let toks = tokens(&input);
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi");
    };
    assert!(csi.params().params_overflowed());
    assert_eq!(csi.params().params().len(), ControlParams::PARAM_LIMIT);
    // Raw bytes still hold every parameter (40 params joined by 39 semicolons).
    let semicolons = csi.params().param_bytes().split(|&b| b == b';').count() - 1;
    assert_eq!(semicolons, 39);
}

#[test]
fn osc_with_bel_terminator() {
    let toks = tokens(b"\x1b]52;c;SGVsbG8=\x07");
    let SyntaxToken::Osc(osc) = &toks[0] else {
        panic!("expected Osc, got {:?}", toks[0]);
    };
    assert_eq!(osc.kind(), StringKind::Osc);
    assert_eq!(osc.payload(), b"52;c;SGVsbG8=");
    assert_eq!(osc.terminator(), StringTerminator::Bel);
    assert!(!osc.truncated());
}

#[test]
fn osc_with_c1_st_terminator() {
    let toks = tokens(b"\x1b]0;title\x9c");
    let SyntaxToken::Osc(osc) = &toks[0] else {
        panic!("expected Osc, got {:?}", toks[0]);
    };
    assert_eq!(osc.payload(), b"0;title");
    assert_eq!(osc.terminator(), StringTerminator::C1);
}

#[test]
fn dcs_exposes_param_prefix_and_payload() {
    let toks = tokens(b"\x1bP1$r q\x1b\\");
    let SyntaxToken::Dcs(dcs) = &toks[0] else {
        panic!("expected Dcs, got {:?}", toks[0]);
    };
    let control = dcs.control_params().expect("DCS carries a param prefix");
    assert_eq!(control.param_bytes(), b"1");
    assert_eq!(control.intermediates(), b"$");
    assert_eq!(control.final_byte(), b'r');
    assert_eq!(dcs.payload(), b" q");
    assert_eq!(dcs.terminator(), StringTerminator::EscBackslash);
}

#[test]
fn c1_csi_introducer_is_recognized() {
    let toks = tokens(b"\x9b31m");
    let SyntaxToken::Csi(csi) = &toks[0] else {
        panic!("expected Csi from C1 introducer, got {:?}", toks[0]);
    };
    assert_eq!(csi.as_bytes(), b"\x9b31m");
    assert_eq!(csi.params().final_byte(), b'm');
}

#[test]
fn c1_byte_inside_utf8_is_a_continuation_not_an_introducer() {
    // U+009B is encoded as 0xc2 0x9b. The 0x9b must be consumed as a continuation byte.
    let toks = tokens(&[0xc2, 0x9b, b'a']);
    assert_eq!(toks, vec![SyntaxToken::Text(vec![0xc2, 0x9b, b'a'])]);
}

#[test]
fn invalid_utf8_is_malformed_not_text() {
    let toks = tokens(&[b'a', 0xff, b'b']);
    assert_eq!(
        toks,
        vec![
            SyntaxToken::Text(b"a".to_vec()),
            SyntaxToken::Malformed(vec![0xff]),
            SyntaxToken::Text(b"b".to_vec()),
        ]
    );
}

#[test]
fn can_aborts_csi_as_malformed() {
    let toks = tokens(b"\x1b[31\x18");
    assert_eq!(toks, vec![SyntaxToken::Malformed(b"\x1b[31\x18".to_vec())]);
}

#[test]
fn plain_escape_sequence() {
    let toks = tokens(b"\x1bc");
    match &toks[0] {
        SyntaxToken::Esc(escape) => {
            assert_eq!(escape.as_bytes(), b"\x1bc");
            assert_eq!(escape.final_byte(), Some(b'c'));
        }
        other => panic!("expected Esc, got {other:?}"),
    }
}

#[test]
fn bounded_osc_truncates_and_counts_dropped() {
    let mut parser = SyntaxParser::with_payload_limit(4);
    let mut toks = parser.feed(b"\x1b]0;abcdefgh\x07");
    toks.extend(parser.finish());
    let SyntaxToken::Osc(osc) = &toks[0] else {
        panic!("expected Osc, got {:?}", toks[0]);
    };
    assert!(osc.truncated());
    assert_eq!(osc.payload(), b"0;ab");
    assert_eq!(osc.dropped_bytes(), b"cdefgh".len());
    // Parsing resumes cleanly after the terminator.
    assert_eq!(toks.len(), 1);
}

#[test]
fn no_panic_on_malformed_corpus_case() {
    let _ = tokens(b"\x1b[?bad\x1b]unterminated");
}

#[test]
fn bracketed_paste_is_one_opaque_token() {
    let toks = tokens(b"\x1b[200~hello\x1b[201~");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"hello");
    assert!(paste.is_first());
    assert!(paste.is_final());
    assert!(paste.terminated());
    assert_eq!(paste.as_bytes(), b"\x1b[200~hello\x1b[201~");
}

#[test]
fn paste_payload_keeps_embedded_sequences_as_data() {
    // The payload is opaque: an embedded SGR and an embedded OSC are kept verbatim, not tokenized.
    // This is the whole reason paste capture lives at the syntax layer.
    let toks = tokens(b"\x1b[200~\x1b[31mred\x1b]0;t\x07\x1b[201~");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"\x1b[31mred\x1b]0;t\x07");
}

#[test]
fn paste_false_end_bracket_is_payload() {
    // `ESC [ 201 x ~` is not the end bracket (extra `x`), so it stays payload; the real end bracket
    // that follows closes the paste. A lone ESC before the real end bracket is payload too.
    let toks = tokens(b"\x1b[200~a\x1b[201x~b\x1b\x1b[201~");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"a\x1b[201x~b\x1b");
    assert!(paste.terminated());
}

#[test]
fn paste_recognizes_8bit_end_bracket() {
    let toks = tokens(b"\x1b[200~hi\x9b201~");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"hi");
    assert!(paste.terminated());
}

#[test]
fn unterminated_paste_flushes_as_final_unterminated_segment() {
    // FM-A8: no hang. `finish` flushes the held bytes as a final segment marked unterminated.
    let toks = tokens(b"\x1b[200~no end here");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"no end here");
    assert!(paste.is_final());
    assert!(!paste.terminated(), "no end bracket was seen");
}

#[test]
fn large_paste_segments_losslessly_at_the_bound() {
    // R-IN-7: a terminated paste larger than the bound is segmented, not truncated. Every byte is
    // preserved across the segments, the first carries the start bracket, the last is terminated.
    let mut parser = SyntaxParser::with_payload_limit(4);
    let mut toks = parser.feed(b"\x1b[200~abcdefghij\x1b[201~");
    toks.extend(parser.finish());

    assert!(toks.len() > 1, "a paste past the bound segments");
    let mut payload = Vec::new();
    for (i, token) in toks.iter().enumerate() {
        let SyntaxToken::Paste(paste) = token else {
            panic!("expected Paste, got {token:?}");
        };
        assert_eq!(paste.is_first(), i == 0);
        assert_eq!(paste.is_final(), i == toks.len() - 1);
        payload.extend_from_slice(paste.payload());
    }
    assert_eq!(payload, b"abcdefghij", "no paste byte is dropped");
    let SyntaxToken::Paste(last) = toks.last().expect("segments") else {
        unreachable!()
    };
    assert!(last.terminated());
}

#[test]
fn empty_paste_is_a_single_terminated_segment() {
    let toks = tokens(b"\x1b[200~\x1b[201~");
    assert_eq!(toks.len(), 1);
    let SyntaxToken::Paste(paste) = &toks[0] else {
        panic!("expected Paste, got {:?}", toks[0]);
    };
    assert_eq!(paste.payload(), b"");
    assert!(paste.is_first() && paste.is_final() && paste.terminated());
}
