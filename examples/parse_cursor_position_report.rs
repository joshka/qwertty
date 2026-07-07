//! Parse a cursor position report from the lossless syntax layer without routing terminal queries.
//!
//! This shows the pure parsing half of the query story: build the request bytes, tokenize a reply
//! through the [`SyntaxParser`], and parse the CSI token into a typed [`CursorPositionReport`].
//! Matching a report to the query that provoked it is the correlator's job and is owned by the
//! Tokio session; this example only parses one report shape.

use qwertty::commands::cursor;
use qwertty::report::CursorPositionReport;
use qwertty::{CommandBuffer, ProtocolPosition, SyntaxParser, SyntaxToken};

fn main() {
    // The cursor-position query the request helper emits.
    let mut query = CommandBuffer::new();
    query.command(cursor::request_position());
    assert_eq!(query.as_bytes(), b"\x1b[6n");

    // A terminal answers a cursor-position query with `CSI row ; column R`. Tokenize the reply.
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(b"\x1b[12;34R");
    tokens.extend(parser.finish());

    let SyntaxToken::Csi(csi) = &tokens[0] else {
        panic!("expected a CSI token");
    };

    // Parse the CSI token into a typed cursor position report.
    let report = CursorPositionReport::from_control_sequence(csi).expect("cursor position report");
    assert_eq!(report.position(), ProtocolPosition::new(12, 34));
    assert_eq!(report.row(), 12);
    assert_eq!(report.column(), 34);

    // The parser rejects anything that is not exactly the report shape. An unrelated CSI token —
    // here a private device status report — is not a cursor position report.
    let mut parser = SyntaxParser::new();
    let mut tokens = parser.feed(b"\x1b[?25n");
    tokens.extend(parser.finish());
    let SyntaxToken::Csi(unrelated) = &tokens[0] else {
        panic!("expected a CSI token");
    };
    assert_eq!(CursorPositionReport::from_control_sequence(unrelated), None);

    println!("parsed cursor position report at {:?}", report.position());
}
