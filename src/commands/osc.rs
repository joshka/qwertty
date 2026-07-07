//! OSC (Operating System Command) command helpers.
//!
//! OSC commands ask the terminal to do something outside the character grid: set the window
//! title, draw a hyperlink, write the system clipboard, or mark shell prompt/command boundaries
//! for semantic navigation. The wire shape is `OSC Ps ; Pt ST`, where `Ps` selects the command and
//! `Pt` (when present) carries its text parameters. qwertty always emits the 7-bit `ESC \` form of
//! ST (String Terminator) rather than the legacy BEL (`0x07`) terminator some producers use: `ESC
//! \` is unambiguous, and it is the terminator every `db/osc.toml` fixture in this repository
//! pins (see `escape::osc` internally).
//!
//! Every helper in this module returns one [`Command`]: pure bytes, built without a terminal,
//! session, or decoder. Two families here carry documented security obligations beyond "encode
//! the bytes correctly":
//!
//! - **Titles are sanitized before encoding** (FM-X3). A terminal that echoes a title report back
//!   onto a controlling program's stdin has been the exact shape of several CVEs (`ConEmu`
//!   CVE-2022-46387 and its bypass CVE-2023-39150; Windows Terminal's OSC 9;9 working-directory
//!   injection, CVE-2022-44702). [`set_title`] and [`set_icon_and_title`] strip control bytes and a
//!   blocklist of bidi/invisible formatting characters before emitting, and cap length, so a caller
//!   cannot accidentally forward attacker-controlled bytes into a raw title escape. See
//!   [`sanitize_title`] for the exact rule.
//! - **[`set_clipboard`] (OSC 52) is an exfiltration surface, not merely an output command**
//!   (FM-X4). This module only builds the bytes; it has no policy of its own, cannot prompt a user,
//!   and cannot know whether the caller trusts the data being written or the terminal receiving it.
//!   MITRE ATT&CK T1115 (clipboard data) and kitty's own design discussion (kitty#9428) treat blind
//!   OSC 52 writes as something a host must gate, not something a protocol layer can make safe by
//!   construction. **Any session or application code that calls [`set_clipboard`] and forwards the
//!   bytes to a real terminal must apply its own policy gate first** (user opt-in, size/rate
//!   limits, or an explicit allowlist) — that gate is a session concern out of scope for this
//!   encode-only module.
//!
//! ```
//! use qwertty::CommandBuffer;
//! use qwertty::commands::osc;
//!
//! let mut frame = CommandBuffer::new();
//! frame
//!     .command(osc::set_title("build: ok"))
//!     .command(osc::hyperlink("https://example.com", Some("docs")))
//!     .text("docs")
//!     .command(osc::close_hyperlink());
//!
//! assert_eq!(
//!     frame.as_bytes(),
//!     b"\x1b]2;build: ok\x1b\\\x1b]8;id=docs;https://example.com\x1b\\docs\x1b]8;;\x1b\\"
//! );
//! ```

use crate::{Command, escape};

/// The maximum number of `char`s kept in a sanitized title.
///
/// Window titles are typically rendered in a fixed-width tab or title bar; there is no protocol
/// limit, but an unbounded title is both a poor user experience and a way to smuggle a large
/// payload through a channel meant for a short label. 240 chars comfortably covers every
/// legitimate title this crate's profiles need (shell prompts, build status, file paths) while
/// keeping the cap well short of sizes that stress terminal-side title rendering.
const TITLE_MAX_CHARS: usize = 240;

/// The maximum number of raw bytes accepted by [`set_clipboard`] before the write is dropped.
///
/// OSC 52 payloads are base64-encoded, which already inflates size by roughly a third; some
/// terminals additionally cap the total OSC payload they will buffer. 100,000 raw bytes (roughly
/// 133,000 encoded) is generous for realistic clipboard content (a path, a URL, a paragraph, a
/// small code snippet) while bounding how much a single write can push through what is already a
/// documented exfiltration surface (FM-X4). This follows the size-cap precedent used by other
/// encode-only OSC 52 implementations (codex).
const CLIPBOARD_MAX_BYTES: usize = 100_000;

/// Bidirectional-formatting and invisible-formatting code points stripped from titles (FM-X3).
///
/// This is the "Trojan Source" set: characters that can reorder or hide surrounding text when a
/// title-bar or tab renderer displays them, without being C0/C1 control bytes. Left in place, an
/// attacker-influenced title could visually spoof a different string than the bytes it contains.
/// Each entry is documented individually so a reviewer can check this list against a new CVE
/// without re-deriving it:
///
/// - `U+200B` ZERO WIDTH SPACE, `U+200C` ZERO WIDTH NON-JOINER, `U+200D` ZERO WIDTH JOINER,
///   `U+200E` LEFT-TO-RIGHT MARK, `U+200F` RIGHT-TO-LEFT MARK — invisible spacing/direction marks.
/// - `U+202A`-`U+202E` — the LRE/RLE/PDF/LRO/RLO explicit bidi embedding/override controls
///   (CVE-2021-42574's "Trojan Source" family).
/// - `U+2066`-`U+2069` — the newer LRI/RLI/FSI/PDI bidi isolate controls.
/// - `U+2028` LINE SEPARATOR, `U+2029` PARAGRAPH SEPARATOR — Unicode line breaks that are not C0
///   control bytes but can still inject visual newlines into a single-line title.
/// - `U+FEFF` ZERO WIDTH NO-BREAK SPACE (byte-order mark) — invisible and can be used to split or
///   hide tokens in the displayed title.
const BLOCKED_TITLE_CHARS: &[char] = &[
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{2028}', '\u{2029}', '\u{202A}',
    '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    '\u{FEFF}',
];

/// Returns `true` when `c` must be stripped from a title before encoding.
///
/// This is the union of two classes: ECMA-48 C0/C1 control code points (`< 0x20`, `0x7f`, and
/// `0x80..=0x9f`), which could otherwise smuggle another control sequence or a raw ST/BEL
/// terminator into the title payload and end the OSC early; and [`BLOCKED_TITLE_CHARS`], the
/// bidi/invisible-formatting set (FM-X3).
fn is_blocked_title_char(c: char) -> bool {
    let code = u32::from(c);
    let is_control = code < 0x20 || code == 0x7f || (0x80..=0x9f).contains(&code);
    is_control || BLOCKED_TITLE_CHARS.contains(&c)
}

/// Sanitizes a title string for safe inclusion in an OSC title command (FM-X3).
///
/// Strips every C0/C1 control character and every code point in `BLOCKED_TITLE_CHARS`, then
/// truncates to `TITLE_MAX_CHARS` `char`s. The result contains no bytes that could terminate the
/// OSC early (no ESC, no BEL, no C1 ST), reorder surrounding text, or hide content from a human
/// reading the rendered title.
///
/// This is a pure string transform with no protocol framing; [`set_title`] and
/// [`set_icon_and_title`] call it before building the OSC bytes.
#[must_use]
pub fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| !is_blocked_title_char(*c))
        .take(TITLE_MAX_CHARS)
        .collect()
}

/// Sets the window title.
///
/// This encodes OSC 2, emitted as `OSC 2 ; <sanitized title> ST`. `title` is passed through
/// [`sanitize_title`] first (FM-X3): control bytes and bidi/invisible-formatting characters are
/// stripped, and the result is capped at `TITLE_MAX_CHARS` characters.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::set_title("qwertty"));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]2;qwertty\x1b\\");
/// ```
#[must_use]
pub fn set_title(title: &str) -> Command {
    escape::osc(format!("2;{}", sanitize_title(title)))
}

/// Sets both the icon name and the window title.
///
/// This encodes OSC 0, emitted as `OSC 0 ; <sanitized title> ST`. `title` is sanitized exactly as
/// [`set_title`] sanitizes its argument (FM-X3); OSC 0 differs from OSC 2 only in that terminals
/// that track an icon name separately from the window title update both from the same text.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::set_icon_and_title("qwertty"));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]0;qwertty\x1b\\");
/// ```
#[must_use]
pub fn set_icon_and_title(title: &str) -> Command {
    escape::osc(format!("0;{}", sanitize_title(title)))
}

/// Strips control bytes and ST-injection characters from a hyperlink URI or `id` parameter.
///
/// A raw ESC, BEL, or C1 control byte inside a URI could terminate the OSC 8 sequence early and
/// let the remaining "URI" bytes be interpreted as new terminal input, the same class of injection
/// FM-X3 documents for titles. This strips C0 (`< 0x20`, `0x7f`) and C1 (`0x80..=0x9f`) control
/// code points; it does not otherwise validate or escape the URI (scheme checking, percent-encoding
/// correctness, and similar policy stay a caller's decision).
fn sanitize_uri_component(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            let code = u32::from(*c);
            !(code < 0x20 || code == 0x7f || (0x80..=0x9f).contains(&code))
        })
        .collect()
}

/// Opens a hyperlink: following text is a clickable link to `uri` until [`close_hyperlink`].
///
/// This encodes OSC 8, emitted as `OSC 8 ; <params> ; <sanitized uri> ST`. When `id` is `Some`,
/// `params` is `id=<id>`, letting a terminal treat multiple text spans as one hyperlink (for
/// example, a link wrapped across lines); when `id` is `None`, `params` is empty. Both `uri` and
/// `id` are sanitized with `sanitize_uri_component` (control-byte and ST-injection stripping)
/// before encoding.
///
/// # Examples
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::hyperlink("https://example.com", Some("docs")));
///
/// assert_eq!(
///     frame.as_bytes(),
///     b"\x1b]8;id=docs;https://example.com\x1b\\"
/// );
/// ```
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::hyperlink("https://example.com", None));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]8;;https://example.com\x1b\\");
/// ```
#[must_use]
pub fn hyperlink(uri: &str, id: Option<&str>) -> Command {
    let uri = sanitize_uri_component(uri);
    let params = match id {
        Some(id) => format!("id={}", sanitize_uri_component(id)),
        None => String::new(),
    };
    escape::osc(format!("8;{params};{uri}"))
}

/// Closes the currently open hyperlink so following text is not linked.
///
/// This encodes the empty-URI form of OSC 8, emitted as `OSC 8 ; ; ST`. Use this as the closing
/// pair for [`hyperlink`].
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::close_hyperlink());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]8;;\x1b\\");
/// ```
#[must_use]
pub fn close_hyperlink() -> Command {
    escape::osc("8;;")
}

/// Requests the terminal's default foreground colour (OSC 10 query).
///
/// This encodes `OSC 10 ; ? ST`, emitted as `b"\x1b]10;?\x1b\\"`. Terminals answer with
/// `OSC 10 ; rgb:… ST` (the terminator may be ST or BEL — FM-P9), which qwertty parses into an
/// [`OscColorReport`](crate::report::OscColorReport). This is a foreground query in a capability
/// probe (design 03).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a response,
/// or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::request_foreground_color());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]10;?\x1b\\");
/// ```
#[must_use]
pub fn request_foreground_color() -> Command {
    escape::osc("10;?")
}

/// Requests the terminal's default background colour (OSC 11 query).
///
/// This encodes `OSC 11 ; ? ST`, emitted as `b"\x1b]11;?\x1b\\"`. Terminals answer with
/// `OSC 11 ; rgb:… ST` (the terminator may be ST or BEL — FM-P9), which qwertty parses into an
/// [`OscColorReport`](crate::report::OscColorReport). A background-colour query is how an
/// application detects a light or dark theme (design 06).
///
/// This helper only builds the request bytes. It does not write to a terminal, wait for a response,
/// or filter unrelated input.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::request_background_color());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]11;?\x1b\\");
/// ```
#[must_use]
pub fn request_background_color() -> Command {
    escape::osc("11;?")
}

/// Which clipboard selection an OSC 52 command targets.
///
/// This enum is `#[non_exhaustive]`: OSC 52 defines further single-letter selection targets (`s`
/// selection, `q` cut buffers 0-7) that qwertty does not expose yet.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ClipboardSelection {
    /// The system clipboard (OSC 52 target `c`).
    Clipboard,
    /// The X11 primary selection (OSC 52 target `p`).
    Primary,
}

impl ClipboardSelection {
    /// Returns the single-letter OSC 52 target byte for this selection (`c` or `p`).
    const fn target(self) -> char {
        match self {
            Self::Clipboard => 'c',
            Self::Primary => 'p',
        }
    }
}

/// Writes `data` to a clipboard selection.
///
/// This encodes OSC 52, emitted as `OSC 52 ; <target> ; <base64(data)> ST`, where `target` is `c`
/// for [`ClipboardSelection::Clipboard`] or `p` for [`ClipboardSelection::Primary`], and the
/// payload is base64-encoded with [`encode_base64`].
///
/// `data` longer than `CLIPBOARD_MAX_BYTES` (100,000 raw bytes) is dropped: this returns a
/// no-op command (an OSC 52 write with an empty payload is treated as a clipboard clear by some
/// terminals, so instead this **encodes nothing at all** — an empty [`Command`] — rather than
/// emit a truncated, silently-different payload the caller did not ask for).
///
/// # Security (FM-X4)
///
/// **This command is an exfiltration surface, not merely a formatting choice.** OSC 52 blind
/// writes let *any* text output — including text the host program is merely displaying, not
/// authoring — reach the local clipboard, which is why terminals themselves increasingly prompt
/// or drop these writes (kitty#9428) and MITRE catalogs clipboard writes as ATT&CK T1115. This
/// function only builds bytes: it has no policy, cannot prompt, and does not know whether the
/// caller's data or destination terminal is trusted. **A session or application must apply its
/// own policy gate (opt-in, allowlist, size/rate limiting) before writing this command's bytes to
/// a real terminal.**
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc::{self, ClipboardSelection};
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::set_clipboard(ClipboardSelection::Clipboard, b"Hello"));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]52;c;SGVsbG8=\x1b\\");
/// ```
#[must_use]
pub fn set_clipboard(selection: ClipboardSelection, data: &[u8]) -> Command {
    if data.len() > CLIPBOARD_MAX_BYTES {
        return Command::raw(Vec::new());
    }
    let encoded = encode_base64(data);
    escape::osc(format!("52;{};{encoded}", selection.target()))
}

/// The standard base64 alphabet (RFC 4648 section 4), used by [`encode_base64`].
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data` as standard base64 (RFC 4648), with `=` padding.
///
/// qwertty hand-rolls this rather than take a dependency on it (design 08's dependency-policy
/// affirmation, ADR 0016): OSC 52 is the only base64 consumer in this crate, and the standard
/// alphabet with padding is a small, stable, easily-tested transform.
///
/// Three raw bytes become four output characters; a final group of one or two bytes is padded
/// with `=` to keep the output length a multiple of four, per RFC 4648.
#[must_use]
pub fn encode_base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();

        let n0 = b0 >> 2;
        let n1 = ((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4);
        let n2 = ((b1.unwrap_or(0) & 0x0f) << 2) | (b2.unwrap_or(0) >> 6);
        let n3 = b2.unwrap_or(0) & 0x3f;

        out.push(BASE64_ALPHABET[n0 as usize] as char);
        out.push(BASE64_ALPHABET[n1 as usize] as char);
        out.push(if b1.is_some() {
            BASE64_ALPHABET[n2 as usize] as char
        } else {
            '='
        });
        out.push(if b2.is_some() {
            BASE64_ALPHABET[n3 as usize] as char
        } else {
            '='
        });
    }

    out
}

/// Marks the start of a shell prompt.
///
/// This encodes OSC 133 with the `FinalTerm` `A` mark, emitted as `OSC 133 ; A ST`
/// (`db/osc.toml`'s `finalterm.shell.prompt_start`). Semantic-prompt marks let a terminal or
/// multiplexer navigate by prompt/command boundary instead of by raw line.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::prompt_start());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]133;A\x1b\\");
/// ```
#[must_use]
pub fn prompt_start() -> Command {
    escape::osc("133;A")
}

/// Marks the end of a shell prompt (equivalently: the start of user command input).
///
/// This encodes OSC 133 with the `FinalTerm` `B` mark, emitted as `OSC 133 ; B ST`
/// (`db/osc.toml`'s `finalterm.shell.prompt_end`). This is the same mark as
/// [`command_start`]: `FinalTerm`'s `B` sits between "prompt text ended" and "the command the user
/// types begins", so both names describe the same boundary from either side. Use whichever name
/// reads better at the call site.
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::prompt_end());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]133;B\x1b\\");
/// ```
#[must_use]
pub fn prompt_end() -> Command {
    escape::osc("133;B")
}

/// Marks the end of a shell prompt (equivalently: the start of user command input).
///
/// This is an alias for [`prompt_end`]; see that function for the encoded bytes and the `FinalTerm`
/// `B` mark it shares with it.
#[must_use]
pub fn command_start() -> Command {
    prompt_end()
}

/// Marks the point where the typed command begins executing and its output starts.
///
/// This encodes OSC 133 with the `FinalTerm` `C` mark, emitted as `OSC 133 ; C ST`
/// (`db/osc.toml`'s `finalterm.shell.command_start`).
///
/// # Example
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::command_executed());
///
/// assert_eq!(frame.as_bytes(), b"\x1b]133;C\x1b\\");
/// ```
#[must_use]
pub fn command_executed() -> Command {
    escape::osc("133;C")
}

/// Marks the end of a command's execution, optionally reporting its exit code.
///
/// This encodes OSC 133 with the `FinalTerm` `D` mark, emitted as `OSC 133 ; D ST` when `exit_code`
/// is `None`, or `OSC 133 ; D ; <exit-code> ST` when it is `Some` (`db/osc.toml`'s
/// `finalterm.shell.command_finished`).
///
/// # Examples
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::command_finished(Some(1)));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]133;D;1\x1b\\");
/// ```
///
/// ```
/// use qwertty::CommandBuffer;
/// use qwertty::commands::osc;
///
/// let mut frame = CommandBuffer::new();
/// frame.command(osc::command_finished(None));
///
/// assert_eq!(frame.as_bytes(), b"\x1b]133;D\x1b\\");
/// ```
#[must_use]
pub fn command_finished(exit_code: Option<i32>) -> Command {
    match exit_code {
        Some(code) => escape::osc(format!("133;D;{code}")),
        None => escape::osc("133;D"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCKED_TITLE_CHARS, CLIPBOARD_MAX_BYTES, ClipboardSelection, TITLE_MAX_CHARS,
        close_hyperlink, command_executed, command_finished, command_start, encode_base64,
        hyperlink, prompt_end, prompt_start, sanitize_title, set_clipboard, set_icon_and_title,
        set_title,
    };
    use crate::{Command, SyntaxParser, SyntaxToken};

    /// Encodes `command` and asserts the bytes match exactly.
    fn assert_bytes(command: &Command, expected: &[u8]) {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);
        assert_eq!(bytes, expected);
    }

    /// Asserts that `command`'s bytes parse back through `SyntaxParser` as exactly one OSC token,
    /// and returns its payload (the bytes between `OSC` and the terminator).
    fn assert_round_trips_as_osc(command: &Command) -> Vec<u8> {
        let mut bytes = Vec::new();
        command.encode(&mut bytes);

        let mut parser = SyntaxParser::new();
        let mut tokens = parser.feed(&bytes);
        tokens.extend(parser.finish());

        assert_eq!(tokens.len(), 1, "expected exactly one token from {bytes:?}");
        let SyntaxToken::Osc(osc) = &tokens[0] else {
            panic!("expected an Osc token from {bytes:?}, got {:?}", tokens[0]);
        };
        assert_eq!(osc.as_bytes(), bytes.as_slice());
        osc.payload().to_vec()
    }

    #[test]
    fn set_title_bytes_and_round_trip() {
        let command = set_title("qwertty");
        assert_bytes(&command, b"\x1b]2;qwertty\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"2;qwertty");
    }

    #[test]
    fn set_icon_and_title_bytes_and_round_trip() {
        let command = set_icon_and_title("qwertty");
        assert_bytes(&command, b"\x1b]0;qwertty\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"0;qwertty");
    }

    #[test]
    fn sanitize_title_strips_c0_and_c1_control_bytes() {
        // Embedded ESC and newline must not survive: either could inject a new sequence or a
        // literal terminal newline into a single-line title (FM-X3).
        let sanitized = sanitize_title("hello\x1b[31mworld\nagain\x7f.\u{9f}");
        assert_eq!(sanitized, "hello[31mworldagain.");
    }

    #[test]
    fn sanitize_title_strips_bidi_and_invisible_formatting_chars() {
        for &blocked in BLOCKED_TITLE_CHARS {
            let title = format!("safe{blocked}text");
            assert_eq!(
                sanitize_title(&title),
                "safetext",
                "char {blocked:?} was not stripped"
            );
        }
    }

    #[test]
    fn set_title_emits_exact_sanitized_bytes_for_injection_attempt() {
        // A title carrying an embedded ESC, a bidi override, and a raw ST-shaped tail must
        // encode as a single well-formed OSC whose payload holds only the sanitized text — never
        // bytes that could early-terminate the sequence or visually reorder it.
        let malicious = "log\x1b]0;evil\x1b\\\u{202E}reversed";
        let command = set_title(malicious);
        assert_bytes(&command, b"\x1b]2;log]0;evil\\reversed\x1b\\");
        assert_eq!(
            assert_round_trips_as_osc(&command),
            b"2;log]0;evil\\reversed"
        );
    }

    #[test]
    fn sanitize_title_caps_length() {
        let long = "x".repeat(TITLE_MAX_CHARS + 100);
        let sanitized = sanitize_title(&long);
        assert_eq!(sanitized.chars().count(), TITLE_MAX_CHARS);
    }

    #[test]
    fn hyperlink_with_id_bytes_and_round_trip() {
        let command = hyperlink("https://example.com", Some("docs"));
        assert_bytes(&command, b"\x1b]8;id=docs;https://example.com\x1b\\");
        assert_eq!(
            assert_round_trips_as_osc(&command),
            b"8;id=docs;https://example.com"
        );
    }

    #[test]
    fn hyperlink_without_id_bytes_and_round_trip() {
        let command = hyperlink("https://example.com", None);
        assert_bytes(&command, b"\x1b]8;;https://example.com\x1b\\");
        assert_eq!(
            assert_round_trips_as_osc(&command),
            b"8;;https://example.com"
        );
    }

    #[test]
    fn hyperlink_strips_control_bytes_from_uri_and_id() {
        // The ESC and BEL control bytes are stripped; the literal backslash is ordinary text and
        // is kept, so it cannot be mistaken for a dropped ST.
        let command = hyperlink("https://example.com/\x1b\\injected", Some("a\x07b"));
        assert_bytes(
            &command,
            b"\x1b]8;id=ab;https://example.com/\\injected\x1b\\",
        );
    }

    #[test]
    fn close_hyperlink_bytes_and_round_trip() {
        let command = close_hyperlink();
        assert_bytes(&command, b"\x1b]8;;\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"8;;");
    }

    #[test]
    fn set_clipboard_bytes_and_round_trip() {
        let command = set_clipboard(ClipboardSelection::Clipboard, b"Hello");
        assert_bytes(&command, b"\x1b]52;c;SGVsbG8=\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"52;c;SGVsbG8=");
    }

    #[test]
    fn set_clipboard_primary_selection_uses_p_target() {
        let command = set_clipboard(ClipboardSelection::Primary, b"Hello");
        assert_bytes(&command, b"\x1b]52;p;SGVsbG8=\x1b\\");
    }

    #[test]
    fn set_clipboard_empty_data_encodes_empty_payload() {
        let command = set_clipboard(ClipboardSelection::Clipboard, b"");
        assert_bytes(&command, b"\x1b]52;c;\x1b\\");
    }

    #[test]
    fn set_clipboard_over_size_cap_is_dropped() {
        let data = vec![b'a'; CLIPBOARD_MAX_BYTES + 1];
        let command = set_clipboard(ClipboardSelection::Clipboard, &data);
        assert_bytes(&command, b"");
    }

    #[test]
    fn set_clipboard_at_size_cap_is_encoded() {
        let data = vec![b'a'; CLIPBOARD_MAX_BYTES];
        let command = set_clipboard(ClipboardSelection::Clipboard, &data);
        let mut bytes = Vec::new();
        command.encode(&mut bytes);
        assert!(!bytes.is_empty());
        assert!(bytes.starts_with(b"\x1b]52;c;"));
        assert!(bytes.ends_with(b"\x1b\\"));
    }

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 section 10 test vectors.
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_hello_vector() {
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn base64_binary_data_round_trips_via_known_vector() {
        // 0x00..0x0f is a fixed, easily-verified vector for non-ASCII/binary input.
        let data: Vec<u8> = (0u8..16).collect();
        assert_eq!(encode_base64(&data), "AAECAwQFBgcICQoLDA0ODw==");
    }

    #[test]
    fn prompt_start_bytes_and_round_trip() {
        let command = prompt_start();
        assert_bytes(&command, b"\x1b]133;A\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"133;A");
    }

    #[test]
    fn prompt_end_and_command_start_are_the_same_mark() {
        assert_eq!(prompt_end(), command_start());
        assert_bytes(&prompt_end(), b"\x1b]133;B\x1b\\");
    }

    #[test]
    fn command_executed_bytes_and_round_trip() {
        let command = command_executed();
        assert_bytes(&command, b"\x1b]133;C\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"133;C");
    }

    #[test]
    fn command_finished_with_exit_code_bytes() {
        let command = command_finished(Some(1));
        assert_bytes(&command, b"\x1b]133;D;1\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"133;D;1");
    }

    #[test]
    fn command_finished_without_exit_code_bytes() {
        let command = command_finished(None);
        assert_bytes(&command, b"\x1b]133;D\x1b\\");
        assert_eq!(assert_round_trips_as_osc(&command), b"133;D");
    }

    #[test]
    fn command_finished_negative_exit_code_bytes() {
        let command = command_finished(Some(-1));
        assert_bytes(&command, b"\x1b]133;D;-1\x1b\\");
    }
}
