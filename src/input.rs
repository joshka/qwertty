//! Terminal input byte values and basic events.
//!
//! The first input layer preserves raw bytes exactly as the terminal device reports them. It does
//! not parse the full Control Sequence Introducer grammar, paste, mouse, focus, query, or vendor
//! extension protocols yet. It can classify complete UTF-8 text, ASCII control input, and a small
//! documented set of Escape-prefixed keys. `InputDecoder` can also buffer incomplete UTF-8 and
//! documented Escape-prefixed keys across chunks so callers can separate simple input from bytes
//! that still need a later parser.

const ESCAPE: u8 = 0x1b;
const DELETE: u8 = 0x7f;

/// A basic terminal input event.
///
/// `InputEvent` is the first event classification layer above raw [`InputBytes`]. It classifies
/// complete UTF-8 text, ASCII control bytes, and a small documented set of Escape-prefixed keys.
/// Unsupported Escape-prefixed input, incomplete or invalid UTF-8, query responses, paste, mouse
/// input, focus reports, and vendor protocols remain [`InputEvent::Undecoded`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InputEvent {
    /// One complete Unicode scalar value decoded from terminal input.
    Text(char),
    /// ASCII control input.
    Control(ControlInput),
    /// A parsed terminal key sequence.
    Key(KeyInput),
    /// Bytes qwertty has not classified yet.
    Undecoded(InputBytes),
}

/// Stateful decoder for terminal input chunks.
///
/// `InputDecoder` owns the small amount of state needed when a terminal read splits a UTF-8
/// scalar value or one of qwertty's documented Escape-prefixed key sequences across byte chunks.
/// It does not route terminal query responses, resolve Escape timing ambiguity, parse paste,
/// mouse, focus, graphics, clipboard, keyboard enhancement, or vendor protocols.
///
/// # Example
///
/// ```
/// use qwertty::{InputDecoder, InputEvent, KeyInput};
///
/// let mut decoder = InputDecoder::new();
///
/// assert!(decoder.decode([0xc3]).is_empty());
/// assert_eq!(decoder.decode([0xa9]), vec![InputEvent::Text('é')]);
///
/// assert!(decoder.decode(b"\x1b[").is_empty());
/// assert_eq!(decoder.decode(b"A"), vec![InputEvent::Key(KeyInput::Up)]);
/// assert!(decoder.finish().is_empty());
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputDecoder {
    pending: Vec<u8>,
}

impl InputDecoder {
    /// Creates an empty input decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes the next raw terminal input chunk into basic input events.
    ///
    /// Complete UTF-8 text becomes [`InputEvent::Text`]. Single ASCII control bytes become
    /// [`InputEvent::Control`]. The documented arrow-key sequences become [`InputEvent::Key`].
    ///
    /// Incomplete UTF-8 and incomplete documented Escape-prefixed key sequences are buffered until
    /// a later call makes them complete or unsupported. Unsupported Escape-prefixed bytes and
    /// invalid UTF-8 are emitted as [`InputEvent::Undecoded`] without losing the original bytes.
    #[must_use]
    pub fn decode(&mut self, input: impl AsRef<[u8]>) -> Vec<InputEvent> {
        let input = input.as_ref();
        if self.pending.is_empty() {
            let (events, pending) = classify_buffered_events(input);
            self.pending = pending;
            return events;
        }

        let mut bytes = std::mem::take(&mut self.pending);
        bytes.extend_from_slice(input);
        let (events, pending) = classify_buffered_events(&bytes);
        self.pending = pending;
        events
    }

    /// Returns buffered bytes that need more input before qwertty can classify them.
    ///
    /// These bytes are the exact bytes retained from previous [`InputDecoder::decode`] calls.
    /// They remain owned by the decoder until another decode call resolves them or
    /// [`InputDecoder::finish`] returns them as undecoded input.
    #[must_use]
    pub fn pending_bytes(&self) -> &[u8] {
        &self.pending
    }

    /// Finishes decoding and returns any remaining buffered bytes as undecoded input.
    ///
    /// This method does not guess whether a pending Escape byte was a standalone Escape key or the
    /// start of a longer sequence. Timing policy belongs to a later input layer, so buffered bytes
    /// are preserved as [`InputEvent::Undecoded`].
    #[must_use]
    pub fn finish(&mut self) -> Vec<InputEvent> {
        if self.pending.is_empty() {
            return Vec::new();
        }

        vec![InputEvent::Undecoded(InputBytes::new(std::mem::take(
            &mut self.pending,
        )))]
    }
}

/// A terminal key sequence qwertty can classify.
///
/// This is intentionally small. The first Escape parser only recognizes the common arrow-key
/// Control Sequence Introducer encodings `ESC [ A`, `ESC [ B`, `ESC [ C`, and `ESC [ D`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum KeyInput {
    /// Up arrow, `ESC [ A`.
    Up,
    /// Down arrow, `ESC [ B`.
    Down,
    /// Right arrow, `ESC [ C`.
    Right,
    /// Left arrow, `ESC [ D`.
    Left,
}

impl KeyInput {
    /// Returns this key's documented byte sequence.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Up => b"\x1b[A",
            Self::Down => b"\x1b[B",
            Self::Right => b"\x1b[C",
            Self::Left => b"\x1b[D",
        }
    }
}

/// A classified ASCII control input byte.
///
/// This type names common single-byte controls while preserving less common controls as their raw
/// byte value. Escape is only classified here when it appears by itself. Escape-prefixed sequences
/// remain [`InputEvent::Undecoded`] so qwertty does not pretend to parse keys or protocol messages
/// before those slices exist.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum ControlInput {
    /// Null, `NUL`, byte `0x00`.
    Null,
    /// Backspace, `BS`, byte `0x08`.
    Backspace,
    /// Horizontal tab, `HT`, byte `0x09`.
    Tab,
    /// Line feed, `LF`, byte `0x0a`.
    LineFeed,
    /// Carriage return, `CR`, byte `0x0d`.
    CarriageReturn,
    /// Escape, `ESC`, byte `0x1b`.
    Escape,
    /// Delete, `DEL`, byte `0x7f`.
    Delete,
    /// Another C0 control byte.
    Other(u8),
}

impl ControlInput {
    /// Classifies a single ASCII control byte.
    ///
    /// Returns `None` for printable bytes and non-ASCII bytes.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::Null),
            0x08 => Some(Self::Backspace),
            0x09 => Some(Self::Tab),
            0x0a => Some(Self::LineFeed),
            0x0d => Some(Self::CarriageReturn),
            ESCAPE => Some(Self::Escape),
            DELETE => Some(Self::Delete),
            0x01..=0x1f => Some(Self::Other(byte)),
            _ => None,
        }
    }

    /// Returns this control input's raw byte.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Null => 0x00,
            Self::Backspace => 0x08,
            Self::Tab => 0x09,
            Self::LineFeed => 0x0a,
            Self::CarriageReturn => 0x0d,
            Self::Escape => ESCAPE,
            Self::Delete => DELETE,
            Self::Other(byte) => byte,
        }
    }
}

/// Raw bytes read from terminal input.
///
/// `InputBytes` is the first caller-visible input value in qwertty. It keeps terminal bytes
/// available exactly as read while [`InputBytes::events`] provides basic UTF-8 text and control
/// classification.
///
/// # Example
///
/// ```
/// use qwertty::InputBytes;
///
/// let input = InputBytes::new(b"a\x1b[A".to_vec());
///
/// assert_eq!(input.as_bytes(), b"a\x1b[A");
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputBytes {
    bytes: Vec<u8>,
}

impl InputBytes {
    /// Creates an input byte value from raw terminal bytes.
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    /// Returns the raw terminal input bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Classifies these bytes into basic input events.
    ///
    /// Complete UTF-8 text becomes [`InputEvent::Text`]. Single ASCII control bytes become
    /// [`InputEvent::Control`]. The documented arrow-key sequences become [`InputEvent::Key`].
    /// Unsupported Escape-prefixed input, incomplete or invalid UTF-8, query responses, paste,
    /// mouse input, focus reports, and vendor protocols remain undecoded.
    ///
    /// Classification is scoped to this byte value. This method does not buffer incomplete UTF-8
    /// across multiple terminal reads.
    #[must_use]
    pub fn events(&self) -> Vec<InputEvent> {
        classify_events(&self.bytes)
    }

    /// Consumes the value and returns the raw terminal input bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Returns the number of input bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `true` when this value contains no input bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl AsRef<[u8]> for InputBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

fn classify_events(bytes: &[u8]) -> Vec<InputEvent> {
    let mut events = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == ESCAPE && index + 1 < bytes.len() {
            match classify_escape_from(bytes, index, &mut events) {
                Some(next_index) => {
                    index = next_index;
                    continue;
                }
                None => break,
            }
        }

        if let Some(control) = ControlInput::from_byte(byte) {
            events.push(InputEvent::Control(control));
            index += 1;
            continue;
        }

        if byte.is_ascii_graphic() || byte == b' ' {
            events.push(InputEvent::Text(char::from(byte)));
            index += 1;
            continue;
        }

        index = classify_utf8_from(bytes, index, &mut events);
    }
    events
}

fn classify_buffered_events(bytes: &[u8]) -> (Vec<InputEvent>, Vec<u8>) {
    let mut events = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == ESCAPE {
            match classify_buffered_escape_from(bytes, index, &mut events) {
                BufferedStep::Consumed(next_index) => {
                    index = next_index;
                    continue;
                }
                BufferedStep::Pending => return (events, bytes[index..].to_vec()),
                BufferedStep::Stopped => return (events, Vec::new()),
            }
        }

        if let Some(control) = ControlInput::from_byte(byte) {
            events.push(InputEvent::Control(control));
            index += 1;
            continue;
        }

        if byte.is_ascii_graphic() || byte == b' ' {
            events.push(InputEvent::Text(char::from(byte)));
            index += 1;
            continue;
        }

        match classify_buffered_utf8_from(bytes, index, &mut events) {
            BufferedStep::Consumed(next_index) => index = next_index,
            BufferedStep::Pending => return (events, bytes[index..].to_vec()),
            BufferedStep::Stopped => return (events, Vec::new()),
        }
    }
    (events, Vec::new())
}

enum BufferedStep {
    Consumed(usize),
    Pending,
    Stopped,
}

fn classify_buffered_escape_from(
    bytes: &[u8],
    index: usize,
    events: &mut Vec<InputEvent>,
) -> BufferedStep {
    let remaining = &bytes[index..];
    if is_supported_escape_prefix(remaining) {
        return BufferedStep::Pending;
    }

    if remaining.len() < 3 {
        events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
        return BufferedStep::Stopped;
    }

    let key = match remaining[..3] {
        [ESCAPE, b'[', b'A'] => KeyInput::Up,
        [ESCAPE, b'[', b'B'] => KeyInput::Down,
        [ESCAPE, b'[', b'C'] => KeyInput::Right,
        [ESCAPE, b'[', b'D'] => KeyInput::Left,
        _ => {
            events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
            return BufferedStep::Stopped;
        }
    };

    events.push(InputEvent::Key(key));
    BufferedStep::Consumed(index + 3)
}

fn is_supported_escape_prefix(bytes: &[u8]) -> bool {
    matches!(bytes, [ESCAPE] | [ESCAPE, b'['])
}

fn classify_buffered_utf8_from(
    bytes: &[u8],
    index: usize,
    events: &mut Vec<InputEvent>,
) -> BufferedStep {
    let Some(width) = utf8_width(bytes[index]) else {
        let invalid_end = index + 1;
        events.push(InputEvent::Undecoded(InputBytes::new(
            bytes[index..invalid_end].to_vec(),
        )));
        return BufferedStep::Consumed(invalid_end);
    };

    let end = index + width;
    if end > bytes.len() {
        return BufferedStep::Pending;
    }

    match std::str::from_utf8(&bytes[index..end]) {
        Ok(text) => {
            let character = text
                .chars()
                .next()
                .expect("non-empty UTF-8 sequence should decode one character");
            events.push(InputEvent::Text(character));
        }
        Err(_) => {
            events.push(InputEvent::Undecoded(InputBytes::new(
                bytes[index..end].to_vec(),
            )));
        }
    }
    BufferedStep::Consumed(end)
}

fn classify_escape_from(bytes: &[u8], index: usize, events: &mut Vec<InputEvent>) -> Option<usize> {
    let remaining = &bytes[index..];
    if remaining.len() < 3 {
        events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
        return None;
    }

    let key = match remaining[..3] {
        [ESCAPE, b'[', b'A'] => KeyInput::Up,
        [ESCAPE, b'[', b'B'] => KeyInput::Down,
        [ESCAPE, b'[', b'C'] => KeyInput::Right,
        [ESCAPE, b'[', b'D'] => KeyInput::Left,
        _ => {
            events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
            return None;
        }
    };

    events.push(InputEvent::Key(key));
    Some(index + 3)
}

fn classify_utf8_from(bytes: &[u8], index: usize, events: &mut Vec<InputEvent>) -> usize {
    let Some(width) = utf8_width(bytes[index]) else {
        let invalid_end = index + 1;
        events.push(InputEvent::Undecoded(InputBytes::new(
            bytes[index..invalid_end].to_vec(),
        )));
        return invalid_end;
    };

    let end = index + width;
    if end > bytes.len() {
        events.push(InputEvent::Undecoded(InputBytes::new(
            bytes[index..].to_vec(),
        )));
        return bytes.len();
    }

    match std::str::from_utf8(&bytes[index..end]) {
        Ok(text) => {
            let character = text
                .chars()
                .next()
                .expect("non-empty UTF-8 sequence should decode one character");
            events.push(InputEvent::Text(character));
        }
        Err(_) => {
            events.push(InputEvent::Undecoded(InputBytes::new(
                bytes[index..end].to_vec(),
            )));
        }
    }
    end
}

fn utf8_width(byte: u8) -> Option<usize> {
    match byte {
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}
