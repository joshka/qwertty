//! Terminal input byte values and basic events.
//!
//! The first input layer preserves raw bytes exactly as the terminal device reports them. It does
//! not parse Escape, Control Sequence Introducer, UTF-8, paste, mouse, focus, query, or vendor
//! extension protocols yet. It can classify single-byte printable ASCII and ASCII control input so
//! callers can separate simple input from bytes that still need a later parser.

const ESCAPE: u8 = 0x1b;
const DELETE: u8 = 0x7f;

/// A basic terminal input event.
///
/// `InputEvent` is the first event classification layer above raw [`InputBytes`]. It classifies
/// printable single-byte ASCII text and ASCII control bytes. Escape-prefixed input, non-ASCII
/// bytes, UTF-8 sequences, query responses, paste, mouse input, focus reports, and vendor
/// protocols remain [`InputEvent::Undecoded`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InputEvent {
    /// Printable single-byte ASCII text.
    Text(char),
    /// ASCII control input.
    Control(ControlInput),
    /// Bytes qwertty has not classified yet.
    Undecoded(InputBytes),
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
/// undecoded so later parser and query-routing slices can be built on behavior that is easy to
/// test and inspect.
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
    /// Printable single-byte ASCII becomes [`InputEvent::Text`]. Single ASCII control bytes become
    /// [`InputEvent::Control`]. Escape-prefixed input, non-ASCII bytes, UTF-8 sequences, query
    /// responses, paste, mouse input, focus reports, and vendor protocols remain undecoded.
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

fn classify_events(bytes: &[u8]) -> Vec<InputEvent> {
    let mut events = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == ESCAPE && index + 1 < bytes.len() {
            events.push(InputEvent::Undecoded(InputBytes::new(
                bytes[index..].to_vec(),
            )));
            break;
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

        events.push(InputEvent::Undecoded(InputBytes::new(
            bytes[index..].to_vec(),
        )));
        break;
    }
    events
}
