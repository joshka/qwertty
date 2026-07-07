//! Fuzzes the reconstruction invariant (design 02 invariant 1) over arbitrary bytes.
//!
//! Tokenizing an input whole at the default bound and concatenating the tokens' raw bytes must
//! reproduce the input byte-for-byte wherever no token drops payload bytes. Where a token does drop
//! bytes (the one documented reconstruction waiver), the accounted length — retained bytes plus the
//! recorded dropped count — must still equal the input length. This is exactly the rule the
//! deterministic suite in `tests/syntax.rs` proves over a fixed corpus, generalized to every input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use qwertty::SyntaxParser;
use qwertty_fuzz::{accounted_len, any_dropped, concat_bytes};

fuzz_target!(|data: &[u8]| {
    // Default bound: large enough that only genuinely huge payloads truncate, so most inputs
    // exercise the byte-exact branch and a few exercise the length-exact waiver.
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(data);
    tokens.extend(parser.finish());

    // Length accounting always holds, drops or not.
    assert_eq!(
        accounted_len(&tokens),
        data.len(),
        "accounted length must equal input length",
    );

    // With no drops, reconstruction is byte-exact.
    if !any_dropped(&tokens) {
        assert_eq!(
            concat_bytes(&tokens),
            data,
            "token bytes must reconstruct the input exactly when nothing is dropped",
        );
    }
});
