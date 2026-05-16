//! Decode terminal input that arrives in split byte chunks.

use qwertty::{CsiInput, InputDecoder, InputEvent, KeyInput};

fn main() {
    let mut decoder = InputDecoder::new();

    assert!(decoder.decode([0xc3]).is_empty());
    assert_eq!(decoder.decode([0xa9]), vec![InputEvent::Text('é')]);

    assert!(decoder.decode(b"\x1b[").is_empty());
    assert_eq!(decoder.decode(b"A"), vec![InputEvent::Key(KeyInput::Up)]);

    assert!(decoder.decode(b"\x1b[?25").is_empty());
    assert_eq!(
        decoder.decode(b"n"),
        vec![InputEvent::Csi(CsiInput::from_bytes(b"\x1b[?25n").unwrap())]
    );

    assert!(decoder.finish().is_empty());
}
