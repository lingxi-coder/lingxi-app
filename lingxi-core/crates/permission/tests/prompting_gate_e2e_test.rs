//! End-to-end `InteractivePromptingGate::prompt_user` tests using
//! `tokio::io::duplex` to script stdin and capture stderr.
//!
//! Pattern: spawn a writer task that pushes the user's response onto the
//! stdin duplex and closes it, plus a reader task that drains the stderr
//! duplex to a `Vec<u8>` for assertion. The gate itself runs on the test's
//! main task — the spawned writer + reader avoid deadlock on the bounded
//! duplex buffers.

use std::sync::Arc;

use lingxi_permission::{
    InteractivePromptingGate, PermissionRequest, PromptDefault, PromptingGate,
};
use serde_json::json;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

fn make_request(tool_name: &str, default: PromptDefault) -> PermissionRequest {
    PermissionRequest {
        tool_name: tool_name.to_string(),
        tool_input: json!({}),
        default_decision: default,
    }
}

#[tokio::test]
async fn typing_y_returns_allow() {
    let (stdin_writer, stdin_reader) = duplex(1024);
    let (stderr_writer, mut stderr_reader) = duplex(1024);

    let gate = InteractivePromptingGate::new(
        Arc::new(Mutex::new(stdin_reader)),
        Arc::new(Mutex::new(stderr_writer)),
    );

    // Script the user's response BEFORE awaiting prompt_user, since duplex
    // is bounded and the gate's write to stderr would deadlock if we block
    // the test task here. Use a background task.
    let writer_handle = tokio::spawn(async move {
        let mut w = stdin_writer;
        w.write_all(b"y\n").await.unwrap();
        // Close so read_line returns even if more bytes were expected.
        drop(w);
    });

    // Drain stderr in parallel so the duplex buffer doesn't fill up and
    // stall the gate's write.
    let reader_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr_reader.read_to_end(&mut buf).await.unwrap();
        buf
    });

    let decision = gate
        .prompt_user(&make_request("Bash", PromptDefault::DenyByDefault))
        .await
        .expect("prompt_user should succeed on `y`");

    assert!(decision.allow, "y should map to allow=true");
    assert!(!decision.persist, "M5-05 always sets persist=false");

    writer_handle.await.unwrap();
    let printed = reader_handle.await.unwrap();
    assert_eq!(
        printed.as_slice(),
        b"Claude needs your permission to use Bash\n[y/N] "
    );
}
