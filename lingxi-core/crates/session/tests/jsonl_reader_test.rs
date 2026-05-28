//! JsonlReader full + lite parity.

use lingxi_platform_posix::fs::PosixFileSystem;
use lingxi_session::jsonl::reader::{JsonlReader, SessionMetadata};
use lingxi_session::jsonl::schema::JsonlMessage;
use lingxi_session::jsonl::writer::JsonlWriter;
use lingxi_traits::FileSystem;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

fn user_msg(n: u8) -> JsonlMessage {
    JsonlMessage {
        message_type: "user".into(),
        uuid: format!("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa{:02x}", n),
        parent_uuid: None,
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        timestamp: format!("2026-05-25T14:30:0{}.000Z", n),
        cwd: "/tmp/proj".into(),
        version: "0.6.0".into(),
        message: json!({"role":"user","content":format!("msg {}", n)}),
        is_sidechain: false,
        user_type: Some("external".into()),
        git_branch: None,
        extra: Default::default(),
    }
}

#[tokio::test]
async fn read_all_round_trips_writer_output() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(dir.path().to_path_buf()));
    let writer = JsonlWriter::new(path.clone(), fs.clone());

    for n in 0..3 {
        writer.append(&user_msg(n)).await.expect("append");
    }

    let reader = JsonlReader::new(path, fs);
    let got = reader.read_all().await.expect("read_all");
    assert_eq!(got.len(), 3);
    for (n, msg) in got.iter().enumerate() {
        assert_eq!(msg.timestamp, format!("2026-05-25T14:30:0{}.000Z", n));
    }
}

#[tokio::test]
async fn read_lite_extracts_metadata_from_first_line_only() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(dir.path().to_path_buf()));
    let writer = JsonlWriter::new(path.clone(), fs.clone());

    // First line is small + has all three lite fields.
    writer.append(&user_msg(0)).await.expect("append 0");
    // Pad with many subsequent lines so the file exceeds 64 KiB —
    // the lite read MUST NOT need to parse past the head buffer.
    let big_content = "x".repeat(2_000);
    for n in 1..40 {
        let mut m = user_msg(n);
        m.message = json!({"role":"user","content":big_content.clone()});
        writer.append(&m).await.expect("append n");
    }

    let file_size = std::fs::metadata(&path).expect("stat").len();
    assert!(
        file_size > 65_536,
        "test setup must produce a >64KiB file (got {file_size})"
    );

    let reader = JsonlReader::new(path, fs);
    let meta: SessionMetadata = reader.read_lite().await.expect("lite");
    assert_eq!(meta.session_id, "11111111-2222-3333-4444-555555555555");
    assert_eq!(meta.cwd, "/tmp/proj");
    assert_eq!(meta.first_type, "user");
}

#[tokio::test]
async fn read_lite_handles_escaped_chars_in_first_line() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(dir.path().to_path_buf()));
    let writer = JsonlWriter::new(path.clone(), fs.clone());

    // cwd contains a quote + backslash (forced into the field by manual
    // construction; the writer escapes them in JSON).
    let mut m = user_msg(0);
    m.cwd = "/path/with \"quote\" and \\back".into();
    writer.append(&m).await.expect("append");

    let reader = JsonlReader::new(path, fs);
    let meta = reader.read_lite().await.expect("lite");
    assert_eq!(meta.cwd, "/path/with \"quote\" and \\back");
}
