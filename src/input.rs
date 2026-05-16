//! Terminal input byte values and basic events.
//!
//! The first input layer preserves raw bytes exactly as the terminal device reports them. It does
//! not parse the full Control Sequence Introducer grammar, paste, mouse, focus, query, or vendor
//! extension protocols yet. It can classify complete UTF-8 text, ASCII control input, and a small
//! documented set of Escape-prefixed keys. `InputDecoder` can also buffer incomplete UTF-8 and
//! Control Sequence Introducer input across chunks so callers can separate simple input from bytes
//! that still need later parser or policy layers.

use crate::ProtocolPosition;

const ESCAPE: u8 = 0x1b;
const DELETE: u8 = 0x7f;

/// A basic terminal input event.
///
/// `InputEvent` is the first event classification layer above raw [`InputBytes`]. It classifies
/// complete UTF-8 text, ASCII control bytes, a small documented set of Escape-prefixed keys, and
/// complete Control Sequence Introducer input. Unsupported Escape-prefixed input, incomplete or
/// invalid UTF-8, query responses, paste, mouse input, focus reports, and vendor protocols remain
/// [`InputEvent::Undecoded`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InputEvent {
    /// One complete Unicode scalar value decoded from terminal input.
    Text(char),
    /// ASCII control input.
    Control(ControlInput),
    /// A parsed terminal key sequence.
    Key(KeyInput),
    /// A complete uninterpreted Control Sequence Introducer input sequence.
    Csi(CsiInput),
    /// Bytes qwertty has not classified yet.
    Undecoded(InputBytes),
}

/// Stateful decoder for terminal input chunks.
///
/// `InputDecoder` owns the small amount of state needed when a terminal read splits a UTF-8
/// scalar value or Control Sequence Introducer input across byte chunks. It does not route
/// terminal query responses, resolve Escape timing ambiguity, parse paste, mouse, focus, graphics,
/// clipboard, keyboard enhancement, or vendor protocols.
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
    /// Other complete Control Sequence Introducer input becomes [`InputEvent::Csi`].
    ///
    /// Incomplete UTF-8 and incomplete Control Sequence Introducer input are buffered until a later
    /// call makes them complete or unsupported. Unsupported Escape-prefixed bytes and invalid UTF-8
    /// are emitted as [`InputEvent::Undecoded`] without losing the original bytes.
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

/// A complete Control Sequence Introducer input sequence.
///
/// `CsiInput` preserves the original bytes and exposes the syntactic pieces qwertty can identify
/// without assigning protocol meaning. The recognized shape is the common 7-bit CSI spelling:
/// `ESC [`, followed by parameter bytes `0x30..=0x3f`, intermediate bytes `0x20..=0x2f`, and one
/// final byte `0x40..=0x7e`.
///
/// The value does not say whether the sequence is a cursor report, device status report, keyboard
/// enhancement response, mouse event, vendor extension, or unsupported protocol. Later parser and
/// query-routing layers own that interpretation.
///
/// # Example
///
/// ```
/// use qwertty::CsiInput;
///
/// let csi = CsiInput::from_bytes(b"\x1b[?25n").expect("complete CSI input");
///
/// assert_eq!(csi.as_bytes(), b"\x1b[?25n");
/// assert_eq!(csi.parameter_bytes(), b"?25");
/// assert_eq!(csi.private_marker_bytes(), b"?");
/// assert_eq!(csi.intermediate_bytes(), b"");
/// assert_eq!(csi.final_byte(), b'n');
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CsiInput {
    bytes: Vec<u8>,
    parameters: Vec<u8>,
    intermediates: Vec<u8>,
    final_byte: u8,
}

impl CsiInput {
    /// Parses a complete 7-bit CSI input sequence.
    ///
    /// Returns `None` when the bytes are not exactly one complete `ESC [` CSI sequence in the
    /// syntactic shape qwertty currently recognizes.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Option<Self> {
        let bytes = bytes.into();
        match parse_csi_from(&bytes, 0) {
            CsiParse::Complete(csi, end) if end == bytes.len() => Some(csi),
            _ => None,
        }
    }

    /// Returns the original CSI bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns all CSI parameter bytes.
    ///
    /// These bytes may include private marker bytes such as `?`. qwertty preserves them here and
    /// also exposes the leading private marker run through [`CsiInput::private_marker_bytes`].
    #[must_use]
    pub fn parameter_bytes(&self) -> &[u8] {
        &self.parameters
    }

    /// Returns leading private marker parameter bytes.
    ///
    /// ECMA-48 reserves parameter bytes `0x3c..=0x3f` for private use. Common terminal protocols
    /// use marker bytes such as `?` before numeric parameters.
    #[must_use]
    pub fn private_marker_bytes(&self) -> &[u8] {
        let marker_len = self
            .parameters
            .iter()
            .take_while(|&&byte| is_private_parameter_byte(byte))
            .count();
        &self.parameters[..marker_len]
    }

    /// Returns CSI intermediate bytes.
    #[must_use]
    pub fn intermediate_bytes(&self) -> &[u8] {
        &self.intermediates
    }

    /// Returns the CSI final byte.
    #[must_use]
    pub fn final_byte(&self) -> u8 {
        self.final_byte
    }

    /// Consumes the value and returns the original CSI bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A parsed terminal cursor position report.
///
/// Cursor position reports are commonly sent by a terminal in response to a `CSI 6 n` cursor
/// position query. The report shape qwertty recognizes is `CSI row ; column R`, where row and
/// column are one-based decimal protocol coordinates.
///
/// This value does not route query responses or prove which request caused the report. It only
/// interprets one complete [`CsiInput`] value.
///
/// # Example
///
/// ```
/// use qwertty::{CsiInput, CursorPositionReport, ProtocolPosition};
///
/// let csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
/// let report = CursorPositionReport::from_csi(&csi).expect("cursor position report");
///
/// assert_eq!(report.position(), ProtocolPosition::new(12, 34));
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CursorPositionReport {
    position: ProtocolPosition,
}

impl CursorPositionReport {
    /// Creates a cursor position report value.
    #[must_use]
    pub const fn new(position: ProtocolPosition) -> Self {
        Self { position }
    }

    /// Parses a cursor position report from a complete CSI input value.
    ///
    /// Returns `None` when the CSI value is not exactly `CSI row ; column R`, when either field is
    /// missing, when either field is not decimal, when either coordinate is zero, or when either
    /// coordinate does not fit in `u16`.
    #[must_use]
    pub fn from_csi(csi: &CsiInput) -> Option<Self> {
        if csi.final_byte() != b'R' || !csi.intermediate_bytes().is_empty() {
            return None;
        }

        let mut fields = csi.parameter_bytes().split(|&byte| byte == b';');
        let row = parse_one_based_u16(fields.next()?)?;
        let column = parse_one_based_u16(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }

        Some(Self::new(ProtocolPosition::new(row, column)))
    }

    /// Returns the reported one-based terminal protocol position.
    #[must_use]
    pub const fn position(self) -> ProtocolPosition {
        self.position
    }

    /// Returns the reported one-based row.
    #[must_use]
    pub const fn row(self) -> u16 {
        self.position.row()
    }

    /// Returns the reported one-based column.
    #[must_use]
    pub const fn column(self) -> u16 {
        self.position.column()
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
    /// Other complete Control Sequence Introducer input becomes [`InputEvent::Csi`]. Unsupported
    /// Escape-prefixed input, incomplete or invalid UTF-8, query responses, paste, mouse input,
    /// focus reports, and vendor protocols remain undecoded.
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

    match classify_escape_sequence_from(bytes, index, events) {
        EscapeStep::Consumed(next_index) => BufferedStep::Consumed(next_index),
        EscapeStep::Incomplete => BufferedStep::Pending,
        EscapeStep::Undecoded => {
            events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
            BufferedStep::Stopped
        }
    }
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

    match classify_escape_sequence_from(bytes, index, events) {
        EscapeStep::Consumed(next_index) => Some(next_index),
        EscapeStep::Incomplete | EscapeStep::Undecoded => {
            events.push(InputEvent::Undecoded(InputBytes::new(remaining.to_vec())));
            None
        }
    }
}

enum EscapeStep {
    Consumed(usize),
    Incomplete,
    Undecoded,
}

fn classify_escape_sequence_from(
    bytes: &[u8],
    index: usize,
    events: &mut Vec<InputEvent>,
) -> EscapeStep {
    if let Some(key) = key_input_from_bytes(&bytes[index..]) {
        events.push(InputEvent::Key(key));
        return EscapeStep::Consumed(index + 3);
    }

    match parse_csi_from(bytes, index) {
        CsiParse::Complete(csi, next_index) => {
            events.push(InputEvent::Csi(csi));
            EscapeStep::Consumed(next_index)
        }
        CsiParse::Incomplete => EscapeStep::Incomplete,
        CsiParse::Invalid => EscapeStep::Undecoded,
    }
}

fn key_input_from_bytes(bytes: &[u8]) -> Option<KeyInput> {
    match bytes.get(..3)? {
        [ESCAPE, b'[', b'A'] => Some(KeyInput::Up),
        [ESCAPE, b'[', b'B'] => Some(KeyInput::Down),
        [ESCAPE, b'[', b'C'] => Some(KeyInput::Right),
        [ESCAPE, b'[', b'D'] => Some(KeyInput::Left),
        _ => None,
    }
}

enum CsiParse {
    Complete(CsiInput, usize),
    Incomplete,
    Invalid,
}

fn parse_csi_from(bytes: &[u8], index: usize) -> CsiParse {
    let start = index;
    if bytes.get(start..start + 2) != Some(b"\x1b[") {
        return CsiParse::Invalid;
    }

    let mut index = start + 2;
    let parameter_start = index;
    while bytes
        .get(index)
        .is_some_and(|&byte| is_csi_parameter_byte(byte))
    {
        index += 1;
    }

    let intermediate_start = index;
    while bytes
        .get(index)
        .is_some_and(|&byte| is_csi_intermediate_byte(byte))
    {
        index += 1;
    }

    let Some(&final_byte) = bytes.get(index) else {
        return CsiParse::Incomplete;
    };

    if !is_csi_final_byte(final_byte) {
        return CsiParse::Invalid;
    }

    let end = index + 1;
    CsiParse::Complete(
        CsiInput {
            bytes: bytes[start..end].to_vec(),
            parameters: bytes[parameter_start..intermediate_start].to_vec(),
            intermediates: bytes[intermediate_start..index].to_vec(),
            final_byte,
        },
        end,
    )
}

fn is_csi_parameter_byte(byte: u8) -> bool {
    matches!(byte, 0x30..=0x3f)
}

fn is_private_parameter_byte(byte: u8) -> bool {
    matches!(byte, 0x3c..=0x3f)
}

fn is_csi_intermediate_byte(byte: u8) -> bool {
    matches!(byte, 0x20..=0x2f)
}

fn is_csi_final_byte(byte: u8) -> bool {
    matches!(byte, 0x40..=0x7e)
}

fn parse_one_based_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() {
        return None;
    }

    let mut value: u16 = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }

        let digit = u16::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }

    (value != 0).then_some(value)
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
