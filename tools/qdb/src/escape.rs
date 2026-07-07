//! Fixture escape/unescape, mirroring `fixtures/FORMAT.md` exactly.
//!
//! The capture harness reads query bytes out of an entry's existing `.seq` fixture (unescape) and
//! writes captured reply bytes back into a minted fixture (escape). Both directions share this one
//! implementation so the round-trip is the format doc's, not a second opinion.

/// Unescapes a fixture payload — the bytes after the header line, trailing `LF` already stripped.
///
/// This is the reference routine from `FORMAT.md`, byte for byte: `\e` is `ESC`, `\xNN` is one
/// hex byte, `\\` is a literal backslash, and any other byte is copied literally.
#[must_use]
pub fn unescape(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\\' && i + 1 < input.len() {
            match input[i + 1] {
                b'e' => {
                    out.push(0x1b);
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'x' if i + 3 < input.len() => {
                    let hi = (input[i + 2] as char).to_digit(16);
                    let lo = (input[i + 3] as char).to_digit(16);
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        out.push(u8::try_from(hi * 16 + lo).unwrap_or(0));
                        i += 4;
                    } else {
                        out.push(input[i]);
                        i += 1;
                    }
                }
                _ => {
                    out.push(input[i]);
                    i += 1;
                }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

/// Escapes raw bytes into the fixture payload encoding.
///
/// The inverse of [`unescape`], choosing the same escapes the harvested fixtures use: `ESC`
/// renders as `\e`, a literal backslash as `\\`, every other printable ASCII byte stays literal,
/// and anything else (controls, high bytes) renders as `\xNN` with lowercase hex. Round-trips:
/// `unescape(escape(b)) == b` for all inputs.
#[must_use]
pub fn escape(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input {
        match b {
            0x1b => out.push_str("\\e"),
            b'\\' => out.push_str("\\\\"),
            // Printable ASCII except backslash: keep literal, matching the harvested fixtures.
            0x20..=0x7e => out.push(b as char),
            other => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\x{other:02x}");
            }
        }
    }
    out
}

/// Reads the escaped payload out of a `.seq` file body: drop the header line (through the first
/// `LF`), strip exactly one trailing `LF`, and return the remaining escaped text unchanged.
///
/// Returns `None` if there is no header line (no `LF` at all).
#[must_use]
pub fn payload_after_header(file_bytes: &[u8]) -> Option<&[u8]> {
    let nl = file_bytes.iter().position(|&b| b == b'\n')?;
    let mut rest = &file_bytes[nl + 1..];
    if rest.last() == Some(&b'\n') {
        rest = &rest[..rest.len() - 1];
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_matches_format_doc() {
        assert_eq!(unescape(b"\\e[c"), vec![0x1b, b'[', b'c']);
        assert_eq!(unescape(b"\\e]11;?\\e\\\\"), b"\x1b]11;?\x1b\\");
        assert_eq!(unescape(b"\\x0a"), vec![0x0a]);
        // A doubled backslash decodes first, so `\\x20` is the four literal bytes, not a hex
        // escape.
        assert_eq!(unescape(b"\\\\x20"), b"\\x20");
    }

    #[test]
    fn escape_round_trips_every_byte() {
        let all: Vec<u8> = (0u8..=255).collect();
        assert_eq!(unescape(escape(&all).as_bytes()), all);
    }

    #[test]
    fn escape_prefers_named_and_literal_forms() {
        assert_eq!(escape(b"\x1b[c"), "\\e[c");
        assert_eq!(escape(b"\\"), "\\\\");
        assert_eq!(escape(&[0x1b, b']', b'1', b'1', 0x07]), "\\e]11\\x07");
        assert_eq!(escape(b"hello"), "hello");
    }

    #[test]
    fn payload_strips_header_and_one_trailing_lf() {
        let file = b"#! direction=host-to-terminal origin=x\n\\e[c\n";
        assert_eq!(payload_after_header(file), Some(&b"\\e[c"[..]));
        // A payload genuinely ending in an encoded LF keeps its `\x0a`; only the file LF is
        // stripped.
        let file2 = b"#! h\n\\x0a\n";
        assert_eq!(payload_after_header(file2), Some(&b"\\x0a"[..]));
        assert_eq!(payload_after_header(b"no newline"), None);
    }
}
