//! Shared Windows handle and pipe primitives for the ConPTY target and its relay child.
//!
//! Both halves of the ConPTY adapter — the host ([`super::conpty`]) and the relay child
//! ([`super::relay_conpty`]) — move framed bytes over a byte pipe and juggle raw handles. The
//! narrow FFI they share lives here so the `unsafe` discipline (one call per block, a `// SAFETY:`
//! each; ADR 0021) is written once. Everything is `#[cfg(windows)]`; nothing here is reachable off
//! Windows, so the non-Windows qdb build never sees an `unsafe` line.
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

/// An owned Windows handle that closes itself on drop, so teardown paths cannot leak or
/// double-close. Holds a raw `HANDLE`, so a value of this type is not `Send` — the ConPTY adapter
/// is driven from a single thread, matching the PTY-hosted adapters.
#[derive(Debug)]
pub(crate) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Wraps a handle returned by an FFI call, rejecting the null / `INVALID_HANDLE_VALUE`
    /// sentinels so callers get an error at the source rather than a late failure.
    pub(crate) fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(handle))
        }
    }

    /// The raw handle, for passing to further FFI. The handle stays owned by `self`.
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a live handle this value uniquely owns (constructed via `new`, which
        // rejects the invalid sentinels), closed exactly once here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// The last-error code for the calling thread.
pub(crate) fn last_error() -> u32 {
    // SAFETY: `GetLastError` only reads the calling thread's last-error slot; always sound.
    unsafe { GetLastError() }
}

/// What a non-blocking peek found on a byte pipe.
pub(crate) enum Ready {
    /// This many bytes are immediately readable (`0` means "quiet, nothing yet").
    Bytes(usize),
    /// The peer closed its end — a genuine EOF, distinct from "quiet".
    Closed,
}

/// Peeks how many bytes are readable right now without blocking, distinguishing a quiet pipe
/// (`Ready::Bytes(0)`) from a closed peer (`Ready::Closed`) — the same silence-is-data /
/// dead-relay-is-EOF split the Unix [`super::relay::RelayTransport`] draws with `EWOULDBLOCK`
/// versus `EOF`.
///
/// # Errors
///
/// Returns an error only on an unexpected `PeekNamedPipe` failure (not a broken pipe, which is a
/// clean `Closed`).
pub(crate) fn peek(handle: HANDLE) -> Result<Ready, String> {
    let mut available: u32 = 0;
    // SAFETY: `handle` is a live pipe handle; every pointer but `available` is null/zero, and
    // `available` is a valid out-param for the `lpTotalBytesAvail` slot.
    let ok = unsafe {
        PeekNamedPipe(
            handle,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            &raw mut available,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let err = last_error();
        if err == ERROR_BROKEN_PIPE {
            return Ok(Ready::Closed);
        }
        return Err(format!("PeekNamedPipe failed (error {err})"));
    }
    Ok(Ready::Bytes(available as usize))
}

/// Reads up to `buf.len()` bytes from the pipe. Returns the number read; `0` means EOF (the peer
/// closed). Callers peek first, so a normal read never blocks on an empty pipe.
///
/// # Errors
///
/// Returns an error on a `ReadFile` failure other than a broken pipe.
pub(crate) fn read_some(handle: HANDLE, buf: &mut [u8]) -> Result<usize, String> {
    let cap = u32::try_from(buf.len()).unwrap_or(u32::MAX);
    let mut read: u32 = 0;
    // SAFETY: `handle` is a live readable pipe handle; `buf` is valid for `cap` bytes; `read` is a
    // valid out-param; the overlapped pointer is null for a synchronous read.
    let ok = unsafe {
        ReadFile(
            handle,
            buf.as_mut_ptr(),
            cap,
            &raw mut read,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let err = last_error();
        if err == ERROR_BROKEN_PIPE {
            return Ok(0);
        }
        return Err(format!("ReadFile failed (error {err})"));
    }
    Ok(read as usize)
}

/// Writes every byte to the pipe, looping over partial writes.
///
/// # Errors
///
/// Returns an error if `WriteFile` fails or reports a zero-byte write (a wedged/closed peer).
pub(crate) fn write_all(handle: HANDLE, mut bytes: &[u8]) -> Result<(), String> {
    while !bytes.is_empty() {
        let chunk = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let mut written: u32 = 0;
        // SAFETY: `handle` is a live writable pipe handle; `bytes` is valid for `chunk` bytes;
        // `written` is a valid out-param; the overlapped pointer is null for a synchronous write.
        let ok = unsafe {
            WriteFile(
                handle,
                bytes.as_ptr(),
                chunk,
                &raw mut written,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!("WriteFile failed (error {})", last_error()));
        }
        let n = written as usize;
        if n == 0 {
            return Err("WriteFile made no progress (side channel closed?)".to_string());
        }
        bytes = &bytes[n..];
    }
    Ok(())
}

/// Encodes a Rust string as a NUL-terminated UTF-16 buffer for the `*W` Windows entry points.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
