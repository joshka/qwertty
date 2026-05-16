//! Parse a cursor position report without routing terminal queries.

use qwertty::commands::cursor;
use qwertty::{CommandBuffer, CsiInput, CursorPositionReport, InputEvent, ProtocolPosition};

fn main() {
    let mut query = CommandBuffer::new();
    query.command(cursor::request_position());
    assert_eq!(query.as_bytes(), b"\x1b[6n");

    let csi = CsiInput::from_bytes(b"\x1b[12;34R").expect("complete CSI input");
    let report = CursorPositionReport::from_csi(&csi).expect("cursor position report");

    assert_eq!(report.position(), ProtocolPosition::new(12, 34));

    let matched =
        CursorPositionReport::match_events(vec![InputEvent::Text('x'), InputEvent::Csi(csi)]);

    assert_eq!(
        matched.report().map(CursorPositionReport::position),
        Some(report.position())
    );
    assert_eq!(matched.remaining_events(), &[InputEvent::Text('x')]);
}
