//! Raw terminal input bytes.
//!
//! This is the raw read value at the bottom of qwertty's input stack: one operating-system read,
//! kept exactly as the terminal device reported it. It assigns no meaning to the bytes. Decoding
//! them into syntax tokens and typed events is the job of the [syntax](crate::SyntaxParser) and
//! [semantic](crate::SemanticDecoder) layers above it; this value only carries the bytes losslessly
//! from a session read to the caller.

/// Raw bytes read from terminal input.
///
/// `InputBytes` is the raw read value a session returns from one read of the terminal device. It
/// keeps terminal bytes available exactly as read; it does not decode UTF-8, classify controls, or
/// parse escape sequences. Feed the bytes through [`SyntaxParser`](crate::SyntaxParser) or
/// [`SemanticDecoder`](crate::SemanticDecoder) to decode them.
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

impl AsRef<[u8]> for InputBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}
