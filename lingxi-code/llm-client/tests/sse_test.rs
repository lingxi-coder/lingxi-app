use llm_client::SseFrameSplitter;

#[test]
fn splits_simple_data_events() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].bytes, b"{\"a\":1}");
    assert_eq!(frames[1].bytes, b"{\"b\":2}");
}

#[test]
fn buffers_events_across_chunk_boundaries() {
    let mut splitter = SseFrameSplitter::default();

    assert!(splitter.push(b"data: {\"a\"").is_empty());
    assert!(splitter.push(b":1}\n").is_empty());
    let frames = splitter.push(b"\ndata: x");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].bytes, b"{\"a\":1}");
    let trailing = splitter.finish().expect("trailing unterminated event");
    assert_eq!(trailing.bytes, b"x");
}

#[test]
fn joins_multiple_data_lines_with_newline() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(b"data: line1\ndata: line2\n\n");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].bytes, b"line1\nline2");
}

#[test]
fn handles_crlf_delimiters() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(b"data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\r\n\r\n");

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].bytes, b"{\"a\":1}");
    assert_eq!(frames[1].bytes, b"{\"b\":2}");
}

#[test]
fn ignores_comments_and_non_data_fields() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(
        b": keep-alive\nevent: content_block_delta\nid: 7\nretry: 100\ndata: payload\n\n",
    );

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].bytes, b"payload");
}

#[test]
fn event_without_data_produces_no_frame() {
    let mut splitter = SseFrameSplitter::default();

    assert!(splitter.push(b"event: ping\n\n").is_empty());
    assert!(splitter.finish().is_none());
}

#[test]
fn passes_done_sentinel_through() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(b"data: [DONE]\n\n");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].bytes, b"[DONE]");
}

#[test]
fn data_without_space_after_colon_is_kept() {
    let mut splitter = SseFrameSplitter::default();

    let frames = splitter.push(b"data:{\"a\":1}\n\n");

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].bytes, b"{\"a\":1}");
}
