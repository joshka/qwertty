//! Shared encode-only command types.
//!
//! This module contains the small byte-oriented foundation used by the public facade. Semantic
//! helpers live in [`crate::commands`]; this layer only stores encoded bytes, protocol-space
//! coordinates, and ordered output buffers.

/// A one-based terminal protocol position.
///
/// Terminal cursor-positioning protocols use one-based coordinates: row 1, column 1 is the
/// top-left cell in the active terminal coordinate system. Layout code that stores zero-based
/// coordinates should convert at the boundary before building commands.
///
/// The type does not validate against the current terminal size. A future terminal session layer is
/// responsible for deciding whether a position is inside a live terminal.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProtocolPosition {
    row: u16,
    column: u16,
}

impl ProtocolPosition {
    /// The top-left terminal cell.
    pub const ORIGIN: Self = Self::new(1, 1);

    /// Creates a one-based terminal protocol position.
    #[must_use]
    pub const fn new(row: u16, column: u16) -> Self {
        Self { row, column }
    }

    /// Returns the one-based row.
    #[must_use]
    pub const fn row(self) -> u16 {
        self.row
    }

    /// Returns the one-based column.
    #[must_use]
    pub const fn column(self) -> u16 {
        self.column
    }
}

/// Host-to-terminal command bytes.
///
/// `Command` is the encode-only byte envelope shared by command helpers, buffers, examples, and
/// future terminal sessions. It does not write to a terminal, check policy, flush output, or track
/// terminal state.
///
/// # Examples
///
/// ```
/// use qwertty::Command;
///
/// let command = Command::raw(b"\x1b[2J");
/// let mut bytes = Vec::new();
/// command.encode(&mut bytes);
///
/// assert_eq!(bytes, b"\x1b[2J");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// Raw command bytes.
    Raw(Vec<u8>),
}

impl Command {
    /// Creates a raw command from already encoded terminal bytes.
    #[must_use]
    pub fn raw(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Raw(bytes.into())
    }

    /// Appends this command's bytes to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Raw(bytes) => out.extend_from_slice(bytes),
        }
    }
}

impl AsRef<Command> for Command {
    fn as_ref(&self) -> &Command {
        self
    }
}

/// A growable byte buffer for encoded terminal output.
///
/// `CommandBuffer` is useful for snapshot tests, renderer adapters, examples, and later terminal
/// session code. It stores the encoded bytes exactly as they would be written.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandBuffer {
    bytes: Vec<u8>,
}

impl CommandBuffer {
    /// Creates an empty command buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Creates an empty command buffer with space for at least `capacity` bytes.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    /// Queues a terminal command.
    ///
    /// Commands and text keep the order in which they are queued.
    pub fn command(&mut self, command: impl AsRef<Command>) -> &mut Self {
        command.as_ref().encode(&mut self.bytes);
        self
    }

    /// Queues raw bytes.
    ///
    /// Use this for renderer output that is already encoded. Prefer [`CommandBuffer::text`] when
    /// the data is ordinary UTF-8 text.
    pub fn bytes(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        self.bytes.extend_from_slice(bytes.as_ref());
        self
    }

    /// Queues UTF-8 render text.
    ///
    /// This method does not escape control characters. Renderers that accept user-controlled text
    /// should perform their own escaping policy before writing to a terminal stream.
    pub fn text(&mut self, text: impl AsRef<str>) -> &mut Self {
        self.bytes(text.as_ref())
    }

    /// Returns the queued bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the buffer and returns the queued bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Returns the number of queued bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `true` when no bytes are queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Clears the queued bytes while keeping the allocation for reuse.
    pub fn clear(&mut self) {
        self.bytes.clear();
    }
}
