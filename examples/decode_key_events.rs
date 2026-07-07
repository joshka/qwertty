//! Decode terminal input into semantic key events with `SemanticDecoder`.
//!
//! This shows the semantic layer above the syntax tokenizer: text becomes one key event per
//! character (with the character carried as associated text), C0 controls and arrow keys become key
//! events with a mapped keycode, and every complete-but-unmapped sequence passes through losslessly
//! as `Event::Syntax` rather than a fake keypress.

use qwertty::{Event, Key, KeyEvent, SemanticDecoder, SyntaxToken, TextPayload};

fn main() {
    // Text, an Enter control, an Up arrow, and a private status CSI qwertty does not decode yet.
    let input = b"hi\r\x1b[A\x1b[?25n";

    let mut decoder = SemanticDecoder::new();
    let mut events = decoder.feed(input);
    events.extend(decoder.finish());

    for event in &events {
        match event {
            Event::Key(key) => match key.key() {
                Key::Char(character) => {
                    println!(
                        "Char {character:?} text={:?}",
                        key.text().map(TextPayload::as_str)
                    );
                }
                other => println!("Key {other:?}"),
            },
            Event::Syntax(token) => {
                println!(
                    "Syntax passthrough: {:?}",
                    String::from_utf8_lossy(token.as_bytes())
                );
            }
            other => println!("Other: {other:?}"),
        }
    }

    // Two characters, one Enter, one Up arrow, one passthrough CSI.
    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].key_event().map(KeyEvent::key),
        Some(Key::Char('h'))
    );
    assert_eq!(events[2].key_event().map(KeyEvent::key), Some(Key::Enter));
    assert_eq!(events[3].key_event().map(KeyEvent::key), Some(Key::Up));
    assert_eq!(
        events[4].syntax_token().map(SyntaxToken::as_bytes),
        Some(&b"\x1b[?25n"[..])
    );
    println!("decoded {} events", events.len());
}
