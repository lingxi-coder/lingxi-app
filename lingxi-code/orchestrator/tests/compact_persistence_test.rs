//! P1-05 (parity 2.1.207) — post-compact JSONL persistence + cold-resume
//! reconstruction.
//!
//! claude 2.1.207 persists the FULL compaction transition: the
//! `subtype:"compact_boundary"` system line with `parentUuid: null` (chain
//! reset; real parent in `logicalParentUuid`) + camelCase `compactMetadata`,
//! the summary user line(s) flagged `isCompactSummary` /
//! `isVisibleInTranscriptOnly`, and the loader re-splices the preserved
//! verbatim tail from `compactMetadata.preservedMessages` at cold load. A cold
//! `--resume` therefore reconstructs exactly the post-compact in-memory state.
//!
//! This suite drives a REAL compaction through `run_turn` with a `JsonlWriter`
//! attached, then cold-reloads the file through the loader chain walk and
//! asserts hot-state parity plus the on-disk line shapes.

use llm_client::ContentBlock as LlmContentBlock;

use compaction::CompactionOrchestrator;
use orchestrator::test_support::{
    mock_message_response, noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{state_from_messages, ConversationOrchestrator, OrchestratorConfig};
use platform_posix::fs::PosixFileSystem;
use protocol::ConversationMessage;
use serde_json::Value;
use session::jsonl::loader::build_conversation_chain;
use session::jsonl::reader::JsonlReader;
use session::jsonl::writer::JsonlWriter;
use session::jsonl::JsonlMessage;
use std::sync::Arc;
use tempfile::tempdir;
use traits::FileSystem;

/// The (kind, text) shape used to compare hot vs cold history — message ids
/// differ across a persist/reload cycle (assistant turns are persisted
/// per-block with fresh line uuids), so identity is content-based.
fn shape(history: &[ConversationMessage]) -> Vec<(&'static str, String)> {
    history
        .iter()
        .map(|m| {
            let kind = match m {
                ConversationMessage::User { .. } => "user",
                ConversationMessage::Assistant { .. } => "assistant",
                ConversationMessage::System { .. } => "system",
            };
            (kind, m.text_content())
        })
        .collect()
}

/// Inner `message.content` text of a JSONL line — accepts both the plain
/// string form and the content-block array the persist path writes.
fn inner_text(line: &JsonlMessage) -> String {
    match line.message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

#[tokio::test]
async fn cold_resume_reconstructs_post_compact_state() {
    let dir = tempdir().expect("tempdir");
    let session_path = dir.path().join("session.jsonl");
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(dir.path().to_path_buf()));
    let writer = Arc::new(JsonlWriter::new(session_path.clone(), fs.clone()));

    // Four scripted end_turn replies: three small turns (under the autocompact
    // threshold), then one whose big prompt trips the proactive pre-call
    // compaction.
    let responses: Vec<_> = ["ok one", "ok two", "ok three", "final reply"]
        .iter()
        .map(|t| {
            mock_message_response(
                vec![LlmContentBlock::Text {
                    text: (*t).to_string(),
                    cache_control: None,
                }],
                Some("end_turn"),
            )
        })
        .collect();
    let api = Arc::new(MockApiClient::new(responses));
    let output = Arc::new(MockOutputStream::new());
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api.clone(),
        Arc::new(tool_api::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output.clone(),
        Arc::new(StaticMemoryProvider::empty()),
        dir.path().to_path_buf(),
    )
    .with_jsonl_writer(writer)
    .with_compaction(Arc::new(CompactionOrchestrator::new(200)));

    // Three small persisted turns...
    orch.run_turn("small one").await.expect("turn 1");
    orch.run_turn("small two").await.expect("turn 2");
    orch.run_turn("small three").await.expect("turn 3");
    // ...then a big prompt: the proactive trigger compacts BEFORE the model
    // call, so the boundary + summary land on disk mid-turn.
    let big_prompt = format!("analyze this: {}", "x".repeat(8000));
    orch.run_turn(&big_prompt).await.expect("turn 4");

    // Hot post-compact state.
    let hot_history = {
        let session = orch.session();
        let s = session.lock().await;
        s.history.clone()
    };
    let boundary_pos = hot_history
        .iter()
        .position(compaction::is_compact_boundary)
        .expect("hot history carries the compact boundary marker");
    assert_eq!(
        boundary_pos, 0,
        "boundary marker leads the compacted history"
    );

    // ---- On-disk shape (claude 2.1.207) ---------------------------------- //
    let reader = JsonlReader::new(session_path.clone(), fs.clone());
    let lines: Vec<JsonlMessage> = reader.read_all().await.expect("read_all");

    let boundary_idx = lines
        .iter()
        .position(|l| {
            l.message_type == "system"
                && l.extra.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
        })
        .expect("a compact_boundary system line must be persisted");
    let boundary = &lines[boundary_idx];
    assert!(boundary_idx > 0, "boundary follows the pre-compact lines");
    // Chain reset: parentUuid null; the real parent (the last pre-compact
    // on-disk line) rides in logicalParentUuid.
    assert_eq!(boundary.parent_uuid, None, "boundary line resets the chain");
    assert_eq!(
        boundary.logical_parent_uuid.as_deref(),
        Some(lines[boundary_idx - 1].uuid.as_str()),
        "logicalParentUuid = the last pre-compact line's uuid"
    );
    assert_eq!(
        boundary.extra.get("content").and_then(Value::as_str),
        Some("Conversation compacted")
    );
    assert_eq!(
        boundary.extra.get("level").and_then(Value::as_str),
        Some("info")
    );
    let cm = boundary
        .extra
        .get("compactMetadata")
        .expect("compactMetadata persisted");
    assert_eq!(cm.get("trigger").and_then(Value::as_str), Some("auto"));

    // The summary user line chains off the boundary and carries the flags.
    let summary = &lines[boundary_idx + 1];
    assert_eq!(summary.message_type, "user");
    assert_eq!(
        summary.parent_uuid.as_deref(),
        Some(boundary.uuid.as_str()),
        "summary chains off the boundary"
    );
    assert_eq!(
        summary.extra.get("isCompactSummary"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        summary.extra.get("isVisibleInTranscriptOnly"),
        Some(&Value::Bool(true))
    );

    // Suffix-preserving compaction kept the in-flight big prompt verbatim: the
    // boundary's preservedMessages lists it, anchored on the summary, and the
    // NEXT persisted line (the turn's assistant reply) physically parents off
    // the tail's last on-disk line — claude's exact chain shape.
    let pm = cm
        .get("preservedMessages")
        .expect("preserved tail metadata persisted");
    assert_eq!(
        pm.get("anchorUuid").and_then(Value::as_str),
        Some(summary.uuid.as_str()),
        "tail anchors on the summary line"
    );
    let preserved_uuids: Vec<&str> = pm
        .get("uuids")
        .and_then(Value::as_array)
        .expect("uuids array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let big_prompt_line = lines
        .iter()
        .find(|l| l.message_type == "user" && inner_text(l).starts_with("analyze this:"))
        .expect("big prompt line persisted pre-compact");
    // The tail is round-grouped, so it may carry more than the prompt; the
    // in-flight big prompt is always its LAST member (the newest message).
    assert_eq!(
        preserved_uuids.last().copied(),
        Some(big_prompt_line.uuid.as_str()),
        "preserved tail ends with the in-flight prompt"
    );
    let reply_line = lines.last().expect("assistant reply is the last line");
    assert_eq!(reply_line.message_type, "assistant");
    assert_eq!(
        reply_line.parent_uuid.as_deref(),
        Some(big_prompt_line.uuid.as_str()),
        "post-compact line chains off the preserved tail's last on-disk line"
    );

    // ---- Cold reload == hot state ---------------------------------------- //
    let loaded = reader.read_routed().await.expect("read_routed");
    let (chain, _tip_sid) = build_conversation_chain(&loaded, "cold-load");
    let session_uuid = uuid::Uuid::new_v4();
    let cold_state = state_from_messages(session_uuid, &chain);

    // The boundary system line is skipped on replay (reconstructed at runtime);
    // everything else must match the hot post-compact history exactly — the
    // summary, the re-spliced preserved tail, and the post-compact reply. No
    // summarized pre-compact message may re-enter.
    let hot_minus_marker: Vec<ConversationMessage> = hot_history
        .iter()
        .filter(|m| !compaction::is_compact_boundary(m))
        .cloned()
        .collect();
    assert_eq!(
        shape(&cold_state.history),
        shape(&hot_minus_marker),
        "cold-resume history must equal the hot post-compact history"
    );
    for (_, text) in shape(&cold_state.history) {
        assert!(
            !text.contains("small one") || text.contains("Summary"),
            "summarized pre-compact prompt must not replay verbatim: {text}"
        );
    }
    // The summarized prefix's user prompts are gone from cold history as
    // standalone messages.
    assert!(
        !cold_state
            .history
            .iter()
            .any(|m| matches!(m, ConversationMessage::User { .. })
                && m.text_content() == "small one"),
        "pre-compact user prompt replayed into cold history"
    );
}

/// `isCompactSummary` user lines replay into resumed history as NORMAL user
/// messages (not meta, not skipped) — the cold-resume twin of the hot
/// in-memory summary message.
#[test]
fn compact_summary_line_replays_as_user_history() {
    let mut extra = serde_json::Map::new();
    extra.insert("isVisibleInTranscriptOnly".to_string(), Value::Bool(true));
    extra.insert("isCompactSummary".to_string(), Value::Bool(true));
    let summary = JsonlMessage {
        message_type: "user".to_string(),
        uuid: "9a1b2c3d-4e5f-6789-abcd-ef0123456789".to_string(),
        parent_uuid: None,
        session_id: "11111111-2222-3333-4444-555555555555".to_string(),
        timestamp: "2026-07-13T10:00:00.000Z".to_string(),
        cwd: "/tmp".to_string(),
        version: "0.6.0".to_string(),
        message: serde_json::json!({"role":"user","content":"Summary:\nS"}),
        is_sidechain: false,
        user_type: Some("external".to_string()),
        git_branch: None,
        entrypoint: None,
        slug: None,
        prompt_id: None,
        logical_parent_uuid: None,
        extra,
    };
    let state = state_from_messages(uuid::Uuid::new_v4(), &[summary]);
    assert_eq!(state.history.len(), 1, "summary line replays into history");
    match &state.history[0] {
        ConversationMessage::User { is_meta, .. } => {
            assert!(!is_meta, "summary replays as a NORMAL user message");
        }
        other => panic!("expected a user message, got {other:?}"),
    }
    assert_eq!(state.history[0].text_content(), "Summary:\nS");
}
