//! Shared support for the qwertty syntax-layer fuzz targets.
//!
//! The three fuzz targets generalize the three deterministic invariant suites in `tests/syntax.rs`
//! (reconstruction, split-equivalence, bounded no-panic) over arbitrary libFuzzer inputs. This
//! module holds the accounting logic those suites and the targets share, so the reconstruction rule
//! is written once and stays strict in both places.

use qwertty::SyntaxToken;

/// Returns the dropped-byte count for a truncated string token, or `None`.
///
/// Only OSC/DCS/APC/PM/SOS tokens can report drops, and only when their payload exceeded the
/// configured bound. Every other token is byte-exact and returns `None`.
#[must_use]
pub fn dropped_bytes(token: &SyntaxToken) -> Option<usize> {
    let (SyntaxToken::Osc(string)
    | SyntaxToken::Dcs(string)
    | SyntaxToken::Apc(string)
    | SyntaxToken::Pm(string)
    | SyntaxToken::Sos(string)) = token
    else {
        return None;
    };
    string.truncated().then_some(string.dropped_bytes())
}

/// Total length the tokens account for: retained raw bytes plus every dropped payload byte.
///
/// This is the reconstruction invariant in its length form. For a run with no drops it equals the
/// concatenated raw-byte length; the reconstruction waiver (a truncated string payload) contributes
/// its retained prefix through `as_bytes` and its dropped tail through `dropped_bytes`, so the
/// total always equals the original input length.
#[must_use]
pub fn accounted_len(tokens: &[SyntaxToken]) -> usize {
    tokens
        .iter()
        .map(|token| token.as_bytes().len() + dropped_bytes(token).unwrap_or(0))
        .sum()
}

/// Returns `true` when any token in the sequence reports dropped payload bytes.
#[must_use]
pub fn any_dropped(tokens: &[SyntaxToken]) -> bool {
    tokens.iter().any(|token| dropped_bytes(token).is_some())
}

/// Concatenates the raw bytes of every token in order.
#[must_use]
pub fn concat_bytes(tokens: &[SyntaxToken]) -> Vec<u8> {
    tokens
        .iter()
        .flat_map(SyntaxToken::as_bytes)
        .copied()
        .collect()
}
