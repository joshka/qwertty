//! Internal helpers for constructing escape-sequence command bytes.
//!
//! Public command helpers should describe the bytes they emit, but they should not each hand-roll
//! the common `ESC [` Control Sequence Introducer prefix. Keeping that construction here gives the
//! command modules a small shared vocabulary while the public API remains the byte-oriented
//! [`Command`] envelope.

use crate::Command;

/// ASCII ESC, used to start 7-bit terminal control sequences.
pub(crate) const ESC: u8 = 0x1b;

/// Builds a 7-bit CSI command.
///
/// `parameters` are copied between `ESC [` and `final_byte`. For example,
/// `csi("3;5", 'H')` emits `b"\x1b[3;5H"`.
pub(crate) fn csi(parameters: impl AsRef<str>, final_byte: char) -> Command {
    let parameters = parameters.as_ref();
    let mut bytes = Vec::with_capacity(2 + parameters.len() + final_byte.len_utf8());
    bytes.push(ESC);
    bytes.push(b'[');
    bytes.extend_from_slice(parameters.as_bytes());
    bytes.extend_from_slice(final_byte.encode_utf8(&mut [0; 4]).as_bytes());
    Command::raw(bytes)
}

/// Builds a two-byte escape command.
///
/// For example, `escape(b'7')` emits `b"\x1b7"`.
pub(crate) fn escape(final_byte: u8) -> Command {
    Command::raw([ESC, final_byte])
}

/// Builds a 7-bit OSC (Operating System Command) command, ST-terminated.
///
/// `payload` is copied between `ESC ]` and the String Terminator. qwertty always emits the 7-bit
/// `ESC \` spelling of ST (rather than the BEL, `0x07`, form some OSC producers use, or the 8-bit
/// C1 `0x9c` form): `ESC \` is unambiguous, round-trips through 7-bit-clean channels, and is the
/// terminator every db/osc.toml fixture pins. For example, `osc("0;title")` emits
/// `b"\x1b]0;title\x1b\\"`.
pub(crate) fn osc(payload: impl AsRef<str>) -> Command {
    let payload = payload.as_ref();
    let mut bytes = Vec::with_capacity(2 + payload.len() + 2);
    bytes.push(ESC);
    bytes.push(b']');
    bytes.extend_from_slice(payload.as_bytes());
    bytes.push(ESC);
    bytes.push(b'\\');
    Command::raw(bytes)
}

/// Builds a 7-bit APC (Application Program Command) command, ST-terminated.
///
/// `payload` is copied between the `ESC _` APC introducer and the same 7-bit `ESC \` String
/// Terminator [`osc`] uses. The kitty graphics protocol is carried in APC sequences of the shape
/// `ESC _ G <control-keys> ; <base64-payload> ESC \`, so the whole `G...`-prefixed body is passed
/// as `payload`. For example, `apc("Ga=p,i=7;")` emits `b"\x1b_Ga=p,i=7;\x1b\\"`.
pub(crate) fn apc(payload: impl AsRef<str>) -> Command {
    let payload = payload.as_ref();
    let mut bytes = Vec::with_capacity(2 + payload.len() + 2);
    bytes.push(ESC);
    bytes.push(b'_');
    bytes.extend_from_slice(payload.as_bytes());
    bytes.push(ESC);
    bytes.push(b'\\');
    Command::raw(bytes)
}
