//! Length-prefixed framing for the ConPTY relay side channel.
//!
//! The ConPTY target and its relay child talk over a named pipe that is *separate* from the two
//! pseudo-console VT pipes (host input, host output). Two logical streams share that pipe — the
//! host telling the child "emit these bytes" and the child forwarding "here is what conhost
//! replied" — and although the pipe's two directions already keep them apart, a self-describing
//! frame (a kind tag plus a length prefix) gives each direction message boundaries and lets the
//! wire be validated. It also mirrors [`super::relay`]'s hello handshake: the child sends a
//! [`KIND_HELLO`] frame once it is attached so the host can trust EOF semantics afterward.
//!
//! This module is the *only* part of the ConPTY work that is testable off Windows: it is pure
//! byte manipulation with no FFI, compiled under `cfg(any(test, windows))` so the host test build
//! (`--all-targets`) exercises the unit tests below while the real Windows build links it into the
//! relay and the transport.

/// Frame kind: the relay child announces it is attached and pumping (child → host, once). The
/// host's analogue of [`super::relay`]'s `HELLO` byte — after it, EOF on the side channel is a
/// genuine "relay died" verdict rather than "not connected yet".
pub const KIND_HELLO: u8 = 0x01;

/// Frame kind: the host instructs the relay to emit these bytes to conhost (host → child). The
/// child writes the payload to its real `STD_OUTPUT_HANDLE`, which is what makes conhost's VT
/// engine process a query.
pub const KIND_EMIT: u8 = 0x02;

/// Frame kind: the relay forwards bytes it captured from its stdin (child → host). Under the RR-6
/// hypothesis these are conhost's query replies, delivered to the child's input direction.
pub const KIND_CAPTURED: u8 = 0x03;

/// Header size on the wire: one kind byte followed by a little-endian `u32` payload length.
const HEADER_LEN: usize = 5;

/// Encodes one frame as `[kind][len: u32 LE][payload]`.
///
/// Payloads are query/reply sized (a handful of bytes), so the `u32` length is never a real
/// constraint; an implausibly large payload is capped rather than panicking.
#[must_use]
pub fn encode_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let capped = &payload[..len as usize];
    let mut out = Vec::with_capacity(HEADER_LEN + capped.len());
    out.push(kind);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(capped);
    out
}

/// Incremental decoder: bytes arrive from the pipe in arbitrary chunks, so frames are reassembled
/// across reads. Feed with [`FrameDecoder::push`] and drain with [`FrameDecoder::next_frame`].
#[derive(Debug, Default)]
pub struct FrameDecoder {
    /// Bytes received but not yet consumed into a complete frame.
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends freshly read bytes to the reassembly buffer.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pops the next complete `(kind, payload)` frame, or `None` when a whole frame is not yet
    /// buffered. Call in a loop after each [`FrameDecoder::push`] to drain every ready frame.
    pub fn next_frame(&mut self) -> Option<(u8, Vec<u8>)> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let kind = self.buf[0];
        let len = u32::from_le_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < HEADER_LEN + len {
            return None;
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Some((kind, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_one_frame() {
        let wire = encode_frame(KIND_EMIT, b"\x1b[6n");
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(dec.next_frame(), Some((KIND_EMIT, b"\x1b[6n".to_vec())));
        assert_eq!(dec.next_frame(), None);
    }

    #[test]
    fn hello_frame_has_empty_payload() {
        let wire = encode_frame(KIND_HELLO, &[]);
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(dec.next_frame(), Some((KIND_HELLO, Vec::new())));
    }

    #[test]
    fn reassembles_across_partial_pushes() {
        let wire = encode_frame(KIND_CAPTURED, b"\x1b[10;5R");
        let mut dec = FrameDecoder::new();
        // Feed one byte at a time: no frame surfaces until the last byte lands.
        for (i, byte) in wire.iter().enumerate() {
            dec.push(std::slice::from_ref(byte));
            if i + 1 < wire.len() {
                assert_eq!(dec.next_frame(), None, "frame surfaced early at byte {i}");
            }
        }
        assert_eq!(
            dec.next_frame(),
            Some((KIND_CAPTURED, b"\x1b[10;5R".to_vec()))
        );
    }

    #[test]
    fn drains_multiple_frames_from_one_chunk() {
        let mut wire = encode_frame(KIND_EMIT, b"a");
        wire.extend(encode_frame(KIND_CAPTURED, b"bb"));
        wire.extend(encode_frame(KIND_HELLO, &[]));
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(dec.next_frame(), Some((KIND_EMIT, b"a".to_vec())));
        assert_eq!(dec.next_frame(), Some((KIND_CAPTURED, b"bb".to_vec())));
        assert_eq!(dec.next_frame(), Some((KIND_HELLO, Vec::new())));
        assert_eq!(dec.next_frame(), None);
    }

    #[test]
    fn trailing_partial_frame_stays_buffered() {
        let first = encode_frame(KIND_EMIT, b"x");
        let second = encode_frame(KIND_CAPTURED, b"yy");
        let mut dec = FrameDecoder::new();
        dec.push(&first);
        // Only the header of the second frame so far.
        dec.push(&second[..HEADER_LEN]);
        assert_eq!(dec.next_frame(), Some((KIND_EMIT, b"x".to_vec())));
        assert_eq!(dec.next_frame(), None);
        dec.push(&second[HEADER_LEN..]);
        assert_eq!(dec.next_frame(), Some((KIND_CAPTURED, b"yy".to_vec())));
    }
}
