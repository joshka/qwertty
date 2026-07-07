//! Fuzzes the split-equivalence invariant (design 02 invariant 2) over arbitrary bytes.
//!
//! The parser holds continuation state, so any chunking of the same input must yield the identical
//! token sequence as feeding it whole. This target reads a small header off the front of the input
//! as a chunking recipe, then tokenizes the remaining bytes both whole and in those chunks (each
//! finished with `finish()`), and asserts the two token vectors are equal. It occasionally derives
//! a small payload limit from the header so truncation paths are chunked too — split-equivalence
//! must hold even when a payload straddles a chunk boundary past the bound.

#![no_main]

use libfuzzer_sys::fuzz_target;
use qwertty::{SyntaxParser, SyntaxToken};

/// Splits `body` at deterministic offsets derived from `recipe` bytes.
///
/// Each recipe byte advances the cursor by `(byte % len) + 1` positions (clamped to the end),
/// producing an arbitrary but input-determined chunking. Chunk boundaries depend only on the input,
/// never on libFuzzer internals, so a failing case is reproducible.
fn chunks<'a>(body: &'a [u8], recipe: &[u8]) -> Vec<&'a [u8]> {
    if body.is_empty() {
        return vec![body];
    }
    let mut result = Vec::new();
    let mut cursor = 0;
    for &byte in recipe {
        if cursor >= body.len() {
            break;
        }
        let step = (usize::from(byte) % body.len()) + 1;
        let end = (cursor + step).min(body.len());
        result.push(&body[cursor..end]);
        cursor = end;
    }
    if cursor < body.len() {
        result.push(&body[cursor..]);
    }
    result
}

fn tokenize_whole(body: &[u8], limit: usize) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::with_payload_limit(limit);
    let mut tokens = parser.feed(body);
    tokens.extend(parser.finish());
    tokens
}

fn tokenize_chunks(pieces: &[&[u8]], limit: usize) -> Vec<SyntaxToken> {
    let mut parser = SyntaxParser::with_payload_limit(limit);
    let mut tokens = Vec::new();
    for piece in pieces {
        tokens.extend(parser.feed(piece));
    }
    tokens.extend(parser.finish());
    tokens
}

fuzz_target!(|data: &[u8]| {
    // First byte: how many recipe bytes follow. The rest of the input is the body to tokenize.
    let Some((&recipe_len, rest)) = data.split_first() else {
        return;
    };
    let recipe_len = usize::from(recipe_len).min(rest.len());
    let (recipe, body) = rest.split_at(recipe_len);

    // Occasionally derive a small limit (1..=64) from the recipe to stress truncation under
    // chunking; otherwise use the default large bound where nothing truncates.
    let limit = match recipe.first() {
        Some(&byte) if byte % 4 == 0 => usize::from(byte % 64) + 1,
        _ => qwertty::DEFAULT_PAYLOAD_LIMIT,
    };

    let whole = tokenize_whole(body, limit);
    let chunked = tokenize_chunks(&chunks(body, recipe), limit);

    assert_eq!(
        chunked, whole,
        "chunked tokenization must match whole tokenization (limit {limit})",
    );
});
