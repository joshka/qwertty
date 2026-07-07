//! Semantic input event decoding tests.
//!
//! These mirror the decoding cases of `tests/input.rs` (text, C0 controls, arrow keys, CSI
//! passthrough, undecoded preservation) adapted to the new [`Event`] vocabulary, prove chunk-split
//! equivalence at the event level, and smoke-test the fixture corpus through [`SemanticDecoder`]
//! asserting nothing is dropped.

use std::fs;
use std::path::{Path, PathBuf};

use qwertty::event::{FocusState, MouseButton, MouseEventKind, ScrollDirection};
use qwertty::{
    Event, Key, KeyEvent, KeyEventKind, Modifiers, SemanticDecoder, SyntaxToken, TextPayload,
};

/// Decodes an input whole (feed then finish), returning every event.
fn decode_whole(input: &[u8]) -> Vec<Event> {
    let mut decoder = SemanticDecoder::new();
    let mut events = decoder.feed(input);
    events.extend(decoder.finish());
    events
}

/// A key event for the character `c` with its text set, as legacy text decodes.
fn char_event(character: char) -> Event {
    Event::Key(KeyEvent::new(Key::Char(character)).with_text(character))
}

/// A key event for a keycode with no text, as controls and arrows decode.
fn key_event(key: Key) -> Event {
    Event::Key(KeyEvent::new(key))
}

/// A lossless syntax passthrough event for a complete CSI sequence.
fn csi_event(bytes: &[u8]) -> Event {
    let mut decoder = SemanticDecoder::new();
    let events = decoder.feed(bytes);
    assert!(decoder.finish().is_empty(), "{bytes:?} left pending bytes");
    assert_eq!(events.len(), 1, "{bytes:?} did not decode to one event");
    events.into_iter().next().expect("one event")
}

#[test]
fn decodes_single_byte_text_and_controls() {
    // Parity with input_bytes_classify_single_byte_text_and_controls: "A \t\r\x03\x7f".
    let events = decode_whole(b"A \t\r\x03\x7f");

    assert_eq!(
        events,
        vec![
            char_event('A'),
            char_event(' '),
            key_event(Key::Tab),
            key_event(Key::Enter),
            key_event(Key::Control(0x03)),
            key_event(Key::Backspace),
        ]
    );
}

#[test]
fn text_key_events_are_presses_with_no_modifiers() {
    let events = decode_whole(b"a");
    let Event::Key(key) = &events[0] else {
        panic!("expected key event, got {:?}", events[0]);
    };

    assert_eq!(key.key(), Key::Char('a'));
    assert_eq!(key.kind(), KeyEventKind::Press);
    assert_eq!(key.modifiers(), Modifiers::empty());
    assert_eq!(key.text().map(TextPayload::as_str), Some("a"));
}

#[test]
fn decodes_basic_arrow_keys() {
    // Parity with input_bytes_classify_basic_arrow_keys: "A\x1b[A".
    let events = decode_whole(b"A\x1b[A");

    assert_eq!(events, vec![char_event('A'), key_event(Key::Up)]);
}

#[test]
fn decodes_all_four_arrow_keys() {
    let events = decode_whole(b"\x1b[A\x1b[B\x1b[C\x1b[D");

    assert_eq!(
        events,
        vec![
            key_event(Key::Up),
            key_event(Key::Down),
            key_event(Key::Right),
            key_event(Key::Left),
        ]
    );
}

#[test]
fn decodes_arrow_keys_with_default_one_parameter() {
    // Terminals send `ESC [ 1 A` when a modifier field is present but defaulted; the syntax layer
    // surfaces the explicit `1`, and the decoder still maps it to the arrow key.
    let events = decode_whole(b"\x1b[1A\x1b[1D");

    assert_eq!(events, vec![key_event(Key::Up), key_event(Key::Left)]);
}

#[test]
fn decodes_mixed_arrow_key_text_and_controls() {
    // Parity with input_bytes_classify_mixed_arrow_key_text_and_controls: "\x1b[Aok\r".
    let events = decode_whole(b"\x1b[Aok\r");

    assert_eq!(
        events,
        vec![
            key_event(Key::Up),
            char_event('o'),
            char_event('k'),
            key_event(Key::Enter),
        ]
    );
}

#[test]
fn passes_unmapped_csi_through_as_syntax() {
    // Parity with input_bytes_classify_complete_csi_input: "A\x1b[Z". CSI Z is not an arrow key, so
    // it passes through losslessly as syntax rather than becoming a fake keypress.
    let events = decode_whole(b"A\x1b[Z");

    assert_eq!(events, vec![char_event('A'), csi_event(b"\x1b[Z")]);
    assert_eq!(
        events[1].syntax_token().map(SyntaxToken::as_bytes),
        Some(&b"\x1b[Z"[..])
    );
}

#[test]
fn passes_modified_arrow_csi_through_as_syntax() {
    // `ESC [ 1 ; 5 A` (Ctrl+Up) is not decoded in this parity slice; it stays lossless syntax until
    // the milestone M4 `CSI u` and modified-arrow decode lands, never a fake unmodified Up.
    let events = decode_whole(b"\x1b[1;5A");

    assert_eq!(events, vec![csi_event(b"\x1b[1;5A")]);
}

#[test]
fn passes_private_status_csi_through_as_syntax() {
    let events = decode_whole(b"\x1b[?25n");

    assert_eq!(events, vec![csi_event(b"\x1b[?25n")]);
}

#[test]
fn passes_non_csi_escape_sequence_through_as_syntax() {
    // Parity with input_bytes_preserve_unsupported_non_csi_escape_input_as_undecoded: "A\x1bZ".
    // The old path called this Undecoded; the syntax layer parses `ESC Z` as a complete escape
    // sequence, so it passes through as lossless syntax with its bytes intact.
    let events = decode_whole(b"A\x1bZ");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0], char_event('A'));
    assert_eq!(
        events[1].syntax_token().map(SyntaxToken::as_bytes),
        Some(&b"\x1bZ"[..])
    );
}

#[test]
fn decodes_standalone_escape_from_finish() {
    // The layer above flushes a standalone Escape; the parser surfaces a bare `SyntaxToken::Esc`,
    // and the decoder maps it to the Escape key. This mirrors the ESC timing boundary: a lone ESC
    // only resolves to Escape once the parser is finished.
    let mut decoder = SemanticDecoder::new();
    assert!(decoder.feed(b"\x1b").is_empty());
    let events = decoder.finish();

    assert_eq!(events, vec![key_event(Key::Escape)]);
}

#[test]
fn decodes_complete_utf8_text() {
    // Parity with input_bytes_classify_complete_utf8_text: "é".
    let events = decode_whole("é".as_bytes());

    assert_eq!(events, vec![char_event('é')]);
    let Event::Key(key) = &events[0] else {
        panic!("expected key event");
    };
    assert_eq!(key.text().map(TextPayload::as_str), Some("é"));
}

#[test]
fn decodes_utf8_without_swallowing_later_controls() {
    // Parity with input_bytes_classify_utf8_without_swallowing_later_controls: "é\r".
    let events = decode_whole("é\r".as_bytes());

    assert_eq!(events, vec![char_event('é'), key_event(Key::Enter)]);
}

#[test]
fn multi_character_text_run_is_one_event_per_char() {
    // Design 02: legacy UTF-8 input decodes one key event per character, never a batched payload.
    let events = decode_whole("héllo".as_bytes());

    assert_eq!(
        events,
        vec![
            char_event('h'),
            char_event('é'),
            char_event('l'),
            char_event('l'),
            char_event('o'),
        ]
    );
}

#[test]
fn preserves_invalid_utf8_as_malformed_syntax() {
    // Parity with input_bytes_preserve_invalid_utf8_as_undecoded: the old path called invalid UTF-8
    // Undecoded; the syntax layer preserves it as Malformed, and it passes through as syntax. A
    // `0xc3` lead byte followed by `A` (`0x41`) is an invalid two-byte sequence, so the syntax
    // layer preserves both bytes as one Malformed token (matching the old path, which also kept
    // both bytes in one Undecoded value).
    let events = decode_whole(&[0xc3, b'A']);

    assert_eq!(
        events,
        vec![Event::Syntax(SyntaxToken::Malformed(vec![0xc3, b'A']))]
    );
}

#[test]
fn preserves_control_run_losslessly() {
    // NUL and other less-common C0 controls have no named key; they survive as Key::Control.
    let events = decode_whole(&[0x00, 0x01, 0x02]);

    assert_eq!(
        events,
        vec![
            key_event(Key::Control(0x00)),
            key_event(Key::Control(0x01)),
            key_event(Key::Control(0x02)),
        ]
    );
}

#[test]
fn buffers_split_utf8_text() {
    // Parity with input_decoder_buffers_split_utf8_text, adapted to the syntax layer. The old path
    // emitted the character as soon as it completed; the syntax layer buffers a text *run* until a
    // boundary byte or `finish`, because more text could follow in the next chunk. So the completed
    // `é` flushes at `finish`, not on the completing `feed`. Split-equivalence still holds: feeding
    // "é" whole flushes the same way (see `decodes_complete_utf8_text`, which feeds whole +
    // finish).
    let mut decoder = SemanticDecoder::new();

    assert!(decoder.feed(&[0xc3]).is_empty());
    assert!(decoder.feed(&[0xa9]).is_empty());
    assert_eq!(decoder.finish(), vec![char_event('é')]);
}

#[test]
fn buffers_split_arrow_key_input() {
    // Parity with input_decoder_buffers_split_arrow_key_input.
    let mut decoder = SemanticDecoder::new();

    assert!(decoder.feed(b"\x1b").is_empty());
    assert!(decoder.feed(b"[").is_empty());
    assert_eq!(decoder.feed(b"A"), vec![key_event(Key::Up)]);
}

#[test]
fn classifies_mixed_input_after_buffered_key() {
    // Parity with input_decoder_classifies_mixed_input_after_buffered_key.
    let mut decoder = SemanticDecoder::new();

    assert_eq!(decoder.feed(b"A\x1b["), vec![char_event('A')]);
    assert_eq!(
        decoder.feed(b"B\r"),
        vec![key_event(Key::Down), key_event(Key::Enter)]
    );
}

#[test]
fn keeps_arrow_keys_as_key_events_before_csi_passthrough() {
    // Parity with input_decoder_keeps_arrow_keys_as_key_events.
    let events = decode_whole(b"\x1b[A\x1b[?25n");

    assert_eq!(events, vec![key_event(Key::Up), csi_event(b"\x1b[?25n")]);
}

/// The chunk-split corpus: representative parity inputs plus richer syntax, reused for the
/// event-level split-equivalence proof (mirrors the syntax-layer corpus approach).
fn split_corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("text_ascii", b"hello world".to_vec()),
        ("text_utf8", "héllo wörld".as_bytes().to_vec()),
        ("controls", b"a\r\n\t\x03\x7f\x08b".to_vec()),
        ("arrows", b"\x1b[A\x1b[B\x1b[C\x1b[D".to_vec()),
        ("arrow_one_param", b"\x1b[1A\x1b[1D".to_vec()),
        ("mixed", b"\x1b[Aok\rmore\x1b[B".to_vec()),
        ("csi_passthrough", b"\x1b[?25n\x1b[0m\x1b[Z".to_vec()),
        ("modified_arrow_csi", b"\x1b[1;5A".to_vec()),
        // Kitty CSI-u forms: press, release, shifted alternate, multi-codepoint text, functional
        // key, and the flags report — split-equivalence must hold at the event level for all.
        ("kitty_press", b"\x1b[97;1u".to_vec()),
        ("kitty_release", b"\x1b[97;1:3u".to_vec()),
        ("kitty_shifted_alt", b"\x1b[97:65;2u".to_vec()),
        (
            "kitty_zwj_text",
            b"\x1b[128104;1;128104:8205:128105:8205:128103u".to_vec(),
        ),
        ("kitty_functional", b"\x1b[57357u".to_vec()),
        ("kitty_flags_report", b"\x1b[?1u".to_vec()),
        (
            "kitty_mixed_with_text",
            b"hi\x1b[97:65;2;65uok\x1b[57357u".to_vec(),
        ),
        // Mouse, focus, and paste: split-equivalence must hold at the event level for these too.
        ("mouse_press", b"\x1b[<0;10;20M".to_vec()),
        ("mouse_release", b"\x1b[<0;10;20m".to_vec()),
        ("mouse_wheel", b"\x1b[<64;5;5M".to_vec()),
        ("mouse_drag", b"\x1b[<32;3;4M".to_vec()),
        ("focus", b"\x1b[I\x1b[O".to_vec()),
        ("paste_small", b"\x1b[200~hello\x1b[201~".to_vec()),
        ("paste_crlf", b"\x1b[200~a\r\nb\rc\x1b[201~".to_vec()),
        (
            "paste_embedded_esc",
            b"\x1b[200~text\x1b[31mmore\x1b[201~".to_vec(),
        ),
        (
            "mixed_mouse_focus_paste_keys",
            b"hi\x1b[<0;1;1M\x1b[Ipaste:\x1b[200~x\x1b[201~\x1b[O".to_vec(),
        ),
        ("non_csi_escape", b"a\x1bcb".to_vec()),
        ("osc", b"\x1b]0;title\x07after".to_vec()),
        (
            "osc8",
            b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\".to_vec(),
        ),
        ("dcs", b"\x1bP1$r q\x1b\\".to_vec()),
        ("apc", b"\x1b_Gf=100\x1b\\".to_vec()),
        ("malformed", b"\x1b[?bad\x18tail".to_vec()),
        ("invalid_utf8", vec![b'a', 0xc3, b'b']),
        ("c1_csi", b"x\x9b31mz".to_vec()),
        (
            "mixed_families",
            b"hi\x1b[A\x1b]0;t\x07\x1b[?25n\r".to_vec(),
        ),
    ]
}

/// Feeds an input as the given chunk sizes, flushing at the end.
fn decode_chunks(input: &[u8], chunk_size: usize) -> Vec<Event> {
    let mut decoder = SemanticDecoder::new();
    let mut events = Vec::new();
    for chunk in input.chunks(chunk_size.max(1)) {
        events.extend(decoder.feed(chunk));
    }
    events.extend(decoder.finish());
    events
}

#[test]
fn split_equivalence_over_all_one_splits() {
    for (name, input) in split_corpus() {
        let whole = decode_whole(&input);
        for split in 0..=input.len() {
            let mut decoder = SemanticDecoder::new();
            let (head, tail) = input.split_at(split);
            let mut events = decoder.feed(head);
            events.extend(decoder.feed(tail));
            events.extend(decoder.finish());
            assert_eq!(events, whole, "one-split at {split} differs for {name}");
        }
    }
}

#[test]
fn split_equivalence_byte_at_a_time() {
    for (name, input) in split_corpus() {
        let whole = decode_whole(&input);
        let split = decode_chunks(&input, 1);
        assert_eq!(split, whole, "byte-at-a-time differs for {name}");
    }
}

// --- Fixture-corpus smoke test ---------------------------------------------------------------

/// Parses two ASCII hex digits into a byte.
fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let hi = (hi as char).to_digit(16)?;
    let lo = (lo as char).to_digit(16)?;
    u8::try_from(hi * 16 + lo).ok()
}

/// Unescapes the fixture escaped-text encoding (mirrors `fixtures/FORMAT.md`).
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

/// A parsed fixture: its path (for diagnostics) and unescaped sequence bytes.
struct Fixture {
    name: String,
    bytes: Vec<u8>,
}

/// Splits a `.seq` file into its header and unescaped payload.
fn parse_fixture(path: &Path, raw: &[u8]) -> Fixture {
    let name = path.display().to_string();
    let newline = raw
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or_else(|| panic!("{name}: fixture has no header line"));
    let mut body = &raw[newline + 1..];
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
        root.display()
    );
    paths
        .iter()
        .map(|p| {
            let raw = fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            parse_fixture(p, &raw)
        })
        .collect()
}

/// Returns the number of input bytes a key event accounts for.
///
/// A character key accounts for its text payload's UTF-8 byte length; a control-derived key (Enter,
/// Tab, Backspace, or a raw control) for its one C0 byte. Arrow keys are not expected in the
/// host-to-terminal fixture corpus (they are terminal-to-host input), so this asserts if one
/// appears rather than guessing between the `ESC [ X` and `ESC [ 1 X` byte forms.
fn key_byte_len(key: &KeyEvent) -> usize {
    match key.key() {
        Key::Char(_) => key.text().expect("char key carries text").as_str().len(),
        Key::Enter | Key::Tab | Key::Backspace | Key::Escape | Key::Control(_) => 1,
        other => panic!("unexpected key {other:?} in host-to-terminal fixture corpus"),
    }
}

/// Returns the raw bytes an event preserves: a syntax token's bytes, or nothing for a key event.
///
/// Key events do not reconstruct byte-for-byte (a text key drops the encoding boundary and a
/// control key is one byte), so the smoke test only asserts byte preservation for the syntax
/// passthrough events. Every non-key event must preserve its token bytes.
fn syntax_bytes(event: &Event) -> Option<&[u8]> {
    event.syntax_token().map(SyntaxToken::as_bytes)
}

#[test]
fn fixture_corpus_decodes_without_panic_or_drops() {
    for fixture in load_corpus() {
        // Whole-feed and byte-at-a-time must agree (no panic, split-equivalent through the layer).
        let whole = decode_whole(&fixture.bytes);
        let split = decode_chunks(&fixture.bytes, 1);
        assert_eq!(whole, split, "{}: split-equivalence mismatch", fixture.name);

        // Nothing is dropped: every non-key event preserves its token bytes exactly, and the input
        // byte length is fully accounted for. A text key accounts for its UTF-8 byte length, a
        // keyless key (control or arrow) for its one control byte or its arrow sequence, and a
        // syntax event for its preserved token bytes.
        let mut accounted = 0usize;
        for event in &whole {
            match event {
                Event::Key(key) => accounted += key_byte_len(key),
                Event::Syntax(_) => {
                    let bytes = syntax_bytes(event).expect("syntax event has bytes");
                    assert!(
                        !bytes.is_empty(),
                        "{}: empty syntax token bytes",
                        fixture.name
                    );
                    accounted += bytes.len();
                }
                other => panic!("{}: unexpected event variant {other:?}", fixture.name),
            }
        }
        assert_eq!(
            accounted,
            fixture.bytes.len(),
            "{}: decoded events account for {accounted} bytes, not the {} input bytes",
            fixture.name,
            fixture.bytes.len(),
        );
    }
}

// --- kitty CSI-u decode through the full SemanticDecoder ----------------------------------------

/// Decodes an input to exactly one event, asserting nothing is left pending.
fn decode_one(input: &[u8]) -> Event {
    let events = decode_whole(input);
    assert_eq!(events.len(), 1, "{input:?} did not decode to one event");
    events.into_iter().next().expect("one event")
}

/// Returns the key event an input decodes to, panicking if it is not a single key event.
fn decode_key_event(input: &[u8]) -> KeyEvent {
    match decode_one(input) {
        Event::Key(key) => key,
        other => panic!("{input:?} decoded to {other:?}, not a key event"),
    }
}

#[test]
fn kitty_press_release_repeat_decode_through_decoder() {
    assert_eq!(decode_key_event(b"\x1b[97;1u").kind(), KeyEventKind::Press);
    assert_eq!(
        decode_key_event(b"\x1b[97;1:2u").kind(),
        KeyEventKind::Repeat
    );
    assert_eq!(
        decode_key_event(b"\x1b[97;1:3u").kind(),
        KeyEventKind::Release
    );
}

#[test]
fn kitty_shifted_and_base_layout_alternates_decode_through_decoder() {
    let event = decode_key_event(b"\x1b[97:65:97;2;65u");
    assert_eq!(event.key(), Key::Char('a'));
    assert_eq!(event.shifted_key(), Some('A'));
    assert_eq!(event.base_layout_key(), Some('a'));
    assert_eq!(event.modifiers(), Modifiers::SHIFT);
    assert_eq!(event.text(), Some(&TextPayload::from_char('A')));
}

#[test]
fn kitty_multi_codepoint_text_is_one_event_through_decoder() {
    // The OQ-6 payoff: a decomposed accent and a ZWJ cluster each arrive as ONE event carrying the
    // whole multi-codepoint TextPayload, with key/modifier association intact.
    let combining = decode_key_event(b"\x1b[101;1;101:769u");
    assert_eq!(combining.key(), Key::Char('e'));
    assert_eq!(combining.text(), Some(&TextPayload::from_text("e\u{0301}")));

    let zwj = decode_key_event(b"\x1b[128104;1;128104:8205:128105:8205:128103u");
    assert_eq!(
        zwj.text(),
        Some(&TextPayload::from_text(
            "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"
        ))
    );
}

#[test]
fn kitty_functional_keys_decode_through_decoder() {
    assert_eq!(decode_key_event(b"\x1b[57356u").key(), Key::Home);
    assert_eq!(decode_key_event(b"\x1b[57357u").key(), Key::End);
    assert_eq!(decode_key_event(b"\x1b[3~").key(), Key::Delete);
    assert_eq!(decode_key_event(b"\x1b[57364u").key(), Key::Function(1));
}

#[test]
fn kitty_modifier_arithmetic_edge_cases_through_decoder() {
    // Value-1 encoding, Ctrl, and the Caps/Num lock bits.
    assert_eq!(decode_key_event(b"\x1b[97;5u").modifiers(), Modifiers::CTRL);
    assert_eq!(
        decode_key_event(b"\x1b[97;65u").modifiers(),
        Modifiers::CAPS_LOCK
    );
    assert_eq!(
        decode_key_event(b"\x1b[97;129u").modifiers(),
        Modifiers::NUM_LOCK
    );
}

#[test]
fn kitty_legacy_modified_arrow_decodes_modifiers() {
    let event = decode_key_event(b"\x1b[1;5A");
    assert_eq!(event.key(), Key::Up);
    assert_eq!(event.modifiers(), Modifiers::CTRL);
}

#[test]
fn kitty_flags_report_and_control_forms_pass_through_as_syntax() {
    // Neither a `CSI ? flags u` report nor the push/pop/query host-to-terminal control forms are
    // key events: they degrade to lossless syntax passthrough, never fake keypresses.
    assert_eq!(decode_one(b"\x1b[?1u"), csi_event(b"\x1b[?1u"));
    assert_eq!(decode_one(b"\x1b[>1u"), csi_event(b"\x1b[>1u"));
    assert_eq!(decode_one(b"\x1b[<u"), csi_event(b"\x1b[<u"));
    assert_eq!(decode_one(b"\x1b[<1u"), csi_event(b"\x1b[<1u"));
}

// --- settled trailing text (drain-boundary flush support) ---------------------------------------

#[test]
fn has_settled_text_true_for_complete_trailing_text() {
    // A complete UTF-8 text run parked at the end of a feed is settled: a drained-buffer reader
    // should be able to flush it. This is what lets the Tokio session deliver the last character
    // typed before a pause instead of holding it until the next keystroke.
    let mut decoder = SemanticDecoder::new();
    let events = decoder.feed(b"\xc3\xa9");
    assert!(
        events.is_empty(),
        "trailing text is buffered, not yet emitted"
    );
    assert!(
        decoder.has_settled_text(),
        "complete trailing UTF-8 is settled text"
    );
    assert_eq!(decoder.finish(), [char_event('é')]);
}

#[test]
fn has_settled_text_true_for_ascii_run() {
    let mut decoder = SemanticDecoder::new();
    assert!(decoder.feed(b"hi").is_empty());
    assert!(decoder.has_settled_text());
}

#[test]
fn has_settled_text_false_for_partial_utf8() {
    // A run parked mid-character is NOT settled: the continuation bytes may still arrive.
    let mut decoder = SemanticDecoder::new();
    assert!(
        decoder.feed(b"\xc3").is_empty(),
        "lone lead byte is buffered"
    );
    assert!(
        !decoder.has_settled_text(),
        "a mid-character UTF-8 run must keep waiting for its continuation"
    );
}

#[test]
fn has_settled_text_false_for_partial_escape() {
    // A partial escape/CSI prefix is NOT settled text: flushing it early would guess an ambiguous
    // ESC or a truncated sequence.
    let mut decoder = SemanticDecoder::new();
    assert!(decoder.feed(b"\x1b[").is_empty(), "CSI prefix is buffered");
    assert!(
        !decoder.has_settled_text(),
        "a partial control sequence is not settled text"
    );
}

#[test]
fn has_settled_text_false_when_nothing_pending() {
    let mut decoder = SemanticDecoder::new();
    // A complete sequence leaves nothing pending.
    assert_eq!(decoder.feed(b"\x1b[A").len(), 1);
    assert!(
        !decoder.has_settled_text(),
        "no pending run after a complete token"
    );
}

// --- mouse, focus, paste through the full SemanticDecoder --------------------------------------

#[test]
fn sgr_mouse_press_decodes_through_decoder() {
    let mouse = *decode_one(b"\x1b[<0;10;20M")
        .mouse_event()
        .expect("a mouse event");
    assert_eq!(mouse.kind(), MouseEventKind::Press);
    assert_eq!(mouse.button(), MouseButton::Left);
    assert_eq!(mouse.column(), 10);
    assert_eq!(mouse.row(), 20);
}

#[test]
fn sgr_mouse_scroll_never_coalesces() {
    // FM-V6: three wheel ticks fed together decode to three distinct events, never merged.
    let events = decode_whole(b"\x1b[<64;5;5M\x1b[<64;5;5M\x1b[<64;5;5M");
    assert_eq!(events.len(), 3, "each wheel tick is its own event");
    for event in &events {
        let mouse = event.mouse_event().expect("a mouse event");
        assert_eq!(mouse.kind(), MouseEventKind::Scroll(ScrollDirection::Up));
    }
}

#[test]
fn sgr_mouse_release_and_drag_and_modifiers() {
    assert_eq!(
        decode_one(b"\x1b[<0;1;1m").mouse_event().unwrap().kind(),
        MouseEventKind::Release
    );
    assert_eq!(
        decode_one(b"\x1b[<32;1;1M").mouse_event().unwrap().kind(),
        MouseEventKind::Moved
    );
    // Ctrl+wheel (16 + 64 = 80): a modified scroll, one event, modifiers preserved.
    let ctrl_wheel = *decode_one(b"\x1b[<80;5;5M").mouse_event().unwrap();
    assert_eq!(
        ctrl_wheel.kind(),
        MouseEventKind::Scroll(ScrollDirection::Up)
    );
    assert_eq!(ctrl_wheel.modifiers(), Modifiers::CTRL);
}

#[test]
fn focus_reports_decode_through_decoder() {
    let events = decode_whole(b"\x1b[I\x1b[O");
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].focus_event().map(qwertty::FocusEvent::state),
        Some(FocusState::Gained)
    );
    assert_eq!(
        events[1].focus_event().map(qwertty::FocusEvent::state),
        Some(FocusState::Lost)
    );
}

#[test]
fn small_paste_is_one_event_with_normalized_newlines() {
    let paste = decode_one(b"\x1b[200~line one\r\nline two\r\x1b[201~");
    let paste = paste.paste_event().expect("a paste event");
    assert_eq!(paste.data(), b"line one\nline two\n");
    assert!(paste.is_first() && paste.is_final() && paste.terminated());
    assert!(!paste.contains_control());
}

#[test]
fn paste_payload_keeps_embedded_sequences_as_data() {
    // The embedded ESC/CSI/OSC bytes are DATA inside the paste, delivered byte-exact and never
    // interpreted as syntax. This is the layering guarantee: paste capture at the syntax layer.
    let paste = decode_one(b"\x1b[200~\x1b[31mred\x1b]0;t\x07\x1b[201~");
    let paste = paste.paste_event().expect("a paste event");
    assert_eq!(paste.data(), b"\x1b[31mred\x1b]0;t\x07");
    assert!(
        paste.contains_control(),
        "embedded escapes are surfaced for paste hygiene (R-SEC-3)"
    );
}

#[test]
fn large_paste_arrives_in_final_flagged_segments_losslessly() {
    // A paste larger than the bound arrives as several segments; concatenating their data restores
    // the whole (normalized) paste, and exactly the last segment is final and terminated.
    let mut decoder = SemanticDecoder::with_payload_limit(4);
    let payload: Vec<u8> = (b'a'..=b'z').collect();
    let mut input = Vec::from(*b"\x1b[200~");
    input.extend_from_slice(&payload);
    input.extend_from_slice(b"\x1b[201~");

    let events = {
        let mut events = decoder.feed(&input);
        events.extend(decoder.finish());
        events
    };
    let pastes: Vec<_> = events
        .iter()
        .map(|e| e.paste_event().expect("all paste events"))
        .collect();
    assert!(pastes.len() > 1, "a large paste segments");

    let mut joined = Vec::new();
    for (i, paste) in pastes.iter().enumerate() {
        assert_eq!(paste.is_first(), i == 0);
        assert_eq!(paste.is_final(), i == pastes.len() - 1);
        joined.extend_from_slice(paste.data());
    }
    assert_eq!(joined, payload, "no paste byte is lost across segments");
    assert!(pastes.last().unwrap().terminated());
}

#[test]
fn unterminated_paste_degrades_without_hanging() {
    // FM-A8: a missing end bracket flushes at finish as a final, unterminated paste event — the
    // payload is delivered, and nothing hangs.
    let mut decoder = SemanticDecoder::new();
    assert!(
        decoder.feed(b"\x1b[200~partial paste").is_empty(),
        "an open paste stays buffered until an end bracket or finish"
    );
    let events = decoder.finish();
    assert_eq!(events.len(), 1);
    let paste = events[0].paste_event().expect("a paste event");
    assert_eq!(paste.data(), b"partial paste");
    assert!(paste.is_final());
    assert!(!paste.terminated());
}

#[test]
fn crlf_split_across_paste_segments_normalizes_to_one_newline() {
    // With a small bound, a CRLF can straddle a segment boundary; the decoder's carry collapses it
    // to a single newline, so segmenting never changes the normalized text.
    let mut decoder = SemanticDecoder::with_payload_limit(3);
    let events = {
        let mut events = decoder.feed(b"\x1b[200~ab\r\ncd\x1b[201~");
        events.extend(decoder.finish());
        events
    };
    let joined: Vec<u8> = events
        .iter()
        .flat_map(|e| e.paste_event().expect("paste").data().to_vec())
        .collect();
    assert_eq!(joined, b"ab\ncd");
}
