//! Feed OSC-8 hyperlink and CSI corpus lines through the syntax tokenizer and print the tokens.
//!
//! This shows the total, lossless syntax layer classifying input by ECMA-48 family without
//! assigning protocol meaning: printable text, complete CSI sequences, and complete OSC string
//! sequences with their payloads and terminators.

use qwertty::{StringKind, SyntaxParser, SyntaxToken};

fn main() {
    // An OSC-8 hyperlink wrapping a label, followed by an SGR color CSI and some text.
    let input = b"\x1b]8;;https://example.com\x1b\\label\x1b]8;;\x1b\\\x1b[31mred";

    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(input);
    tokens.extend(parser.finish());

    for token in &tokens {
        match token {
            SyntaxToken::Text(bytes) => {
                println!("Text: {:?}", String::from_utf8_lossy(bytes));
            }
            SyntaxToken::Control(byte) => println!("Control: {byte:#04x}"),
            SyntaxToken::Csi(csi) => {
                println!(
                    "Csi: raw={:?} params={:?} final={:?}",
                    String::from_utf8_lossy(csi.as_bytes()),
                    String::from_utf8_lossy(csi.params().param_bytes()),
                    char::from(csi.params().final_byte()),
                );
            }
            SyntaxToken::Osc(osc) => {
                let terminator = osc.terminator();
                println!(
                    "Osc: payload={:?} terminator={terminator:?} truncated={}",
                    String::from_utf8_lossy(osc.payload()),
                    osc.truncated(),
                );
                assert_eq!(osc.kind(), StringKind::Osc);
            }
            other => println!("Other: {other:?}"),
        }
    }

    // The layer is lossless: concatenating token bytes reproduces the input exactly.
    let rebuilt: Vec<u8> = tokens.iter().flat_map(|t| t.as_bytes().to_vec()).collect();
    assert_eq!(rebuilt, input);
    println!("reconstructed {} bytes exactly", rebuilt.len());
}
