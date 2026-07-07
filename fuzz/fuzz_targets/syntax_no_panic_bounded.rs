//! Fuzzes the bounded no-panic invariant (design 02 invariant 3) over arbitrary bytes.
//!
//! Parser memory must stay bounded regardless of input or chunking: after every `feed`, the bytes
//! the parser is holding for continuation must not exceed the configured payload limit plus a small
//! constant (the introducer and a held ESC), even for an unterminated over-limit string streamed in
//! many chunks. This target derives a small limit and a chunking recipe from the input, feeds the
//! body in those chunks while asserting the bound after each, then finishes. Any panic (including
//! an assertion or an unbounded allocation caught by the RSS limit) is a real bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use qwertty::SyntaxParser;

/// The slack above `limit` allowed in `pending_bytes`: the retained sequence introducer plus a held
/// ESC that may start an `ESC \` terminator. Matches the deterministic bound in `tests/syntax.rs`.
const SLACK: usize = 16;

fuzz_target!(|data: &[u8]| {
    // First byte derives a small limit (0..=63) so truncation and overflow paths are hit often;
    // small limits are where the bound is hardest to hold.
    let Some((&limit_byte, body)) = data.split_first() else {
        return;
    };
    let limit = usize::from(limit_byte % 64);

    // Second byte seeds a fixed chunk size (1..=8) so a run streams across many feeds, exercising
    // the incremental bound rather than a single whole-buffer pass.
    let chunk_size = body.first().map_or(1, |&b| usize::from(b % 8) + 1);

    let mut parser = SyntaxParser::with_payload_limit(limit);
    for chunk in body.chunks(chunk_size) {
        let _ = parser.feed(chunk);
        assert!(
            parser.pending_bytes().len() <= limit + SLACK,
            "pending grew past the bound: {} > {} + {}",
            parser.pending_bytes().len(),
            limit,
            SLACK,
        );
    }
    let _ = parser.finish();
});
