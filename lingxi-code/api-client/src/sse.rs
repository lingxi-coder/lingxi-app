//! SSE (Server-Sent Events) parser.
//!
//! Conforms to <https://html.spec.whatwg.org/multipage/server-sent-events.html>.
//! Buffers raw chunks and emits complete `SseEvent` values.

use protocol::SseEvent;

/// Parse one or more complete events out of a chunk. The chunk MUST end with
/// `\n\n` to terminate the last event; partial events are dropped.
///
/// For real streaming, wrap this in a buffered consumer (see `SseStreamReader`
/// below — added in a later task when we wire to `HttpTransport::stream_sse`).
#[must_use]
pub fn parse_sse_chunks(raw: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for block in raw.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let mut event_type: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();
        let mut id: Option<String> = None;

        for line in block.lines() {
            if line.starts_with(':') {
                continue; // comment / keepalive
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("id:") {
                id = Some(rest.trim().to_string());
            }
        }

        if !data_lines.is_empty() {
            events.push(SseEvent {
                event_type,
                data: data_lines.join("\n"),
                id,
            });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_event() {
        let raw = "event: content_block_delta\ndata: {\"delta\":{\"text\":\"hi\"}}\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type.as_deref(), Some("content_block_delta"));
        assert!(events[0].data.contains("hi"));
    }

    #[test]
    fn parse_multiple_events() {
        let raw = "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn ignore_keepalive_lines() {
        let raw = ": this is a comment\nevent: a\ndata: 1\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn multi_line_data_concatenates() {
        let raw = "event: x\ndata: line1\ndata: line2\n\n";
        let events = parse_sse_chunks(raw);
        assert_eq!(events[0].data, "line1\nline2");
    }
}
