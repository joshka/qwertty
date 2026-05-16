//! Terminal input byte values.
//!
//! The first input layer preserves raw bytes exactly as the terminal device reports them. It does
//! not parse Escape, Control Sequence Introducer, UTF-8, paste, mouse, focus, query, or vendor
//! extension protocols yet.

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
