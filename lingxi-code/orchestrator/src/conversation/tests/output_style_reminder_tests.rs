use super::*;
use crate::test_support::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    mock_message_response, noop_hook_executor, text_delta, MockApiClient, MockOutputStream,
    MockStreamingApiClient, NoOpPermissionGate, StaticMemoryProvider,
};
use crate::OrchestratorConfig;
use llm_client::ContentBlock as LlmContentBlock;
use protocol::ContentBlock;
use std::sync::Arc;
use tool_api::registry::ToolRegistry;

/// The byte-exact reminder text for the `Explanatory` builtin — 1:1 with TS
/// `wrapInSystemReminder(`${name} output style is active. …`)`
/// (`messages.ts:3097-3099` + `3805-3810`).
const EXPLANATORY_REMINDER: &str = "<system-reminder>\nExplanatory output style is active. \
     Remember to follow the specific guidelines for this style.\n</system-reminder>";
const LEARNING_REMINDER: &str = "<system-reminder>\nLearning output style is active. \
     Remember to follow the specific guidelines for this style.\n</system-reminder>";

/// Config with a non-default builtin output style active.
fn config_with_style(style: &str) -> OrchestratorConfig {
    OrchestratorConfig {
        output_style: Some(style.to_string()),
        ..OrchestratorConfig::default()
    }
}

/// Concatenated text of a message's text blocks (for substring checks).
fn text_of(msg: &ConversationMessage) -> String {
    match msg {
        ConversationMessage::User { content, .. }
        | ConversationMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        ConversationMessage::System { content, .. } => content.clone(),
    }
}

fn is_reminder(msg: &ConversationMessage, expected: &str) -> bool {
    matches!(msg, ConversationMessage::User { .. }) && text_of(msg) == expected
}

/// True when `msg` is the leading `additionalContext` (`# claudeMd` /
/// `# userEmail` / `# currentDate`) meta message prepended each turn
/// (R-P1c/R-P1d). With `StaticMemoryProvider::empty()` and no `user_email`
/// it carries only the always-present `# currentDate` entry.
fn is_additional_context(msg: &ConversationMessage) -> bool {
    matches!(msg, ConversationMessage::User { .. })
        && text_of(msg).starts_with(
            "<system-reminder>\nAs you answer the user's questions, you can use the following context:",
        )
}

// ----- direct unit coverage of the reminder builder -----

#[tokio::test]
async fn builder_emits_byte_exact_explanatory_reminder() {
    let orch = ConversationOrchestrator::new(
        config_with_style("Explanatory"),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );
    let msg = orch
        .output_style_reminder_message()
        .await
        .expect("Explanatory resolves to a reminder");
    assert!(matches!(msg, ConversationMessage::User { .. }));
    assert_eq!(text_of(&msg), EXPLANATORY_REMINDER);
    // Spell out the literal bytes once so a drift in the helper const is caught.
    assert_eq!(
        text_of(&msg),
        "<system-reminder>\nExplanatory output style is active. Remember to follow the specific guidelines for this style.\n</system-reminder>"
    );
}

#[tokio::test]
async fn builder_emits_byte_exact_learning_reminder() {
    let orch = ConversationOrchestrator::new(
        config_with_style("Learning"),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );
    assert_eq!(
        text_of(
            &orch
                .output_style_reminder_message()
                .await
                .expect("Learning resolves")
        ),
        LEARNING_REMINDER
    );
}

#[tokio::test]
async fn builder_returns_none_for_default_and_unknown_styles() {
    for style in [None, Some("default"), Some(""), Some("Nonexistent")] {
        let cfg = OrchestratorConfig {
            output_style: style.map(str::to_string),
            ..OrchestratorConfig::default()
        };
        let orch = ConversationOrchestrator::new(
            cfg,
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            std::env::temp_dir(),
        );
        assert!(
            orch.output_style_reminder_message().await.is_none(),
            "style {style:?} must not produce a reminder"
        );
    }
}

// ----- batched driver (`run_turn` / `execute_one_turn`) -----

#[tokio::test]
async fn batched_active_style_appends_transient_reminder_not_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_path = dir.path().join("session.jsonl");
    let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
        dir.path().to_path_buf(),
    ));
    let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
        session_path.clone(),
        fs,
    ));

    let resp = mock_message_response(
        vec![LlmContentBlock::Text {
            text: "assistant body".into(),
            cache_control: None,
        }],
        Some("end_turn"),
    );
    let api = Arc::new(MockApiClient::new(vec![resp]));
    let orch = ConversationOrchestrator::new(
        config_with_style("Explanatory"),
        api.clone(),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        dir.path().to_path_buf(),
    )
    .with_jsonl_writer(writer);

    orch.run_turn("user prompt body").await.expect("turn");

    // OUTGOING snapshot: [additionalContext(meta), user(prompt), reminder] —
    // the leading additional-context meta message (R-P1c/d) prepends the
    // user prompt; the output-style reminder trails it (TS position).
    let outgoing = api.captured_msgs().await;
    assert_eq!(outgoing.len(), 1, "exactly one batched API call");
    let sent = &outgoing[0];
    assert_eq!(
        sent.len(),
        3,
        "additionalContext + prompt + reminder; got {sent:?}"
    );
    assert!(
        is_additional_context(&sent[0]),
        "leading meta; got {:?}",
        sent[0]
    );
    assert_eq!(text_of(&sent[1]), "user prompt body");
    assert!(
        is_reminder(&sent[2], EXPLANATORY_REMINDER),
        "trailing message must be the byte-exact reminder; got {:?}",
        sent[2]
    );

    // STORED history: [user(prompt), assistant] — the reminder was NOT pushed.
    let history = orch.session.lock().await.history.clone();
    assert_eq!(history.len(), 2, "user + assistant only; got {history:?}");
    assert!(
        history
            .iter()
            .all(|m| !is_reminder(m, EXPLANATORY_REMINDER)),
        "the reminder must never enter stored history; got {history:?}"
    );
    assert_eq!(text_of(&history[0]), "user prompt body");
    assert_eq!(text_of(&history[1]), "assistant body");

    // JSONL transcript: user + assistant only, reminder text absent.
    let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
    assert!(on_disk.contains("user prompt body"));
    assert!(on_disk.contains("assistant body"));
    assert!(
        !on_disk.contains("output style is active"),
        "the reminder must never be persisted to JSONL; file:\n{on_disk}"
    );
}

#[tokio::test]
async fn batched_default_style_sends_no_reminder() {
    let resp = mock_message_response(
        vec![LlmContentBlock::Text {
            text: "body".into(),
            cache_control: None,
        }],
        Some("end_turn"),
    );
    let api = Arc::new(MockApiClient::new(vec![resp]));
    let orch = ConversationOrchestrator::new(
        OrchestratorConfig::default(), // output_style: None
        api.clone(),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    orch.run_turn("just the prompt").await.expect("turn");

    let outgoing = api.captured_msgs().await;
    assert_eq!(outgoing.len(), 1);
    // No output-style reminder; the only prepended message is the leading
    // additional-context meta (always present via `# currentDate`).
    assert_eq!(
        outgoing[0].len(),
        2,
        "additionalContext + prompt; got {:?}",
        outgoing[0]
    );
    assert!(
        is_additional_context(&outgoing[0][0]),
        "leading meta; got {:?}",
        outgoing[0][0]
    );
    assert_eq!(text_of(&outgoing[0][1]), "just the prompt");
    assert!(
        !outgoing[0]
            .iter()
            .any(|m| is_reminder(m, EXPLANATORY_REMINDER) || is_reminder(m, LEARNING_REMINDER)),
        "no output-style reminder on the default path; got {:?}",
        outgoing[0]
    );
}

// ----- streaming driver (`run_turn_streaming`) -----

#[tokio::test]
async fn streaming_active_style_appends_transient_reminder_not_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_path = dir.path().join("session.jsonl");
    let fs: Arc<dyn traits::FileSystem> = Arc::new(platform_posix::fs::PosixFileSystem::new(
        dir.path().to_path_buf(),
    ));
    let writer = Arc::new(session::jsonl::writer::JsonlWriter::new(
        session_path.clone(),
        fs,
    ));

    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "streamed body"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ]]));
    let orch = ConversationOrchestrator::new_with_streaming(
        config_with_style("Learning"),
        Arc::new(MockApiClient::new(vec![])),
        streaming.clone(),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        dir.path().to_path_buf(),
    )
    .with_jsonl_writer(writer);

    orch.run_turn_streaming("streaming prompt")
        .await
        .expect("streaming turn");

    // OUTGOING snapshot to the stream: [additionalContext(meta), user(prompt),
    // output-style reminder, total_tokens reminder]. The total-tokens block
    // comes LAST, matching the oracle's fan-out order
    // (`…critical_system_reminder, silent_turn_reminder,
    // total_tokens_reminder`), and it is present because that reminder
    // defaults ON as it does upstream.
    let calls = streaming.captured_calls().await;
    assert_eq!(calls.len(), 1, "exactly one streaming call");
    let sent = &calls[0].messages;
    assert_eq!(
        sent.len(),
        4,
        "additionalContext + prompt + style reminder + total_tokens; got {sent:?}"
    );
    assert!(
        is_additional_context(&sent[0]),
        "leading meta; got {:?}",
        sent[0]
    );
    assert_eq!(text_of(&sent[1]), "streaming prompt");
    assert!(
        is_reminder(&sent[2], LEARNING_REMINDER),
        "the style reminder must be the byte-exact Learning reminder; got {:?}",
        sent[2]
    );
    assert!(
        text_of(&sent[3]).contains("<total_tokens>"),
        "total-tokens reminder trails the batch; got {:?}",
        sent[3]
    );

    // STORED history: reminder absent.
    let history = orch.session.lock().await.history.clone();
    assert!(
        history.iter().all(|m| !is_reminder(m, LEARNING_REMINDER)),
        "the reminder must never enter stored history; got {history:?}"
    );
    assert_eq!(text_of(&history[0]), "streaming prompt");

    // JSONL transcript: reminder text absent.
    let on_disk = std::fs::read_to_string(&session_path).expect("read jsonl");
    assert!(on_disk.contains("streaming prompt"));
    assert!(
        !on_disk.contains("output style is active"),
        "the reminder must never be persisted to JSONL; file:\n{on_disk}"
    );
}

#[tokio::test]
async fn streaming_default_style_sends_no_reminder() {
    let streaming = Arc::new(MockStreamingApiClient::with_turns(vec![vec![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_text(0),
        text_delta(0, "body"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ]]));
    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(), // output_style: None
        Arc::new(MockApiClient::new(vec![])),
        streaming.clone(),
        Arc::new(ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    );

    orch.run_turn_streaming("only prompt")
        .await
        .expect("streaming turn");

    let calls = streaming.captured_calls().await;
    assert_eq!(calls.len(), 1);
    // No output-style reminder. The leading additional-context meta
    // (always present via `# currentDate`) prepends the prompt, and the
    // `total_tokens_reminder` trails it — that reminder defaults ON, as it
    // does in a stock Claude Code session, so it is part of every outgoing
    // list now. See `crate::prompt::total_tokens`.
    assert_eq!(
        calls[0].messages.len(),
        3,
        "additionalContext + prompt + total_tokens_reminder; got {:?}",
        calls[0].messages
    );
    assert!(
        is_additional_context(&calls[0].messages[0]),
        "leading meta; got {:?}",
        calls[0].messages[0]
    );
    assert_eq!(text_of(&calls[0].messages[1]), "only prompt");
    assert!(
        text_of(&calls[0].messages[2]).contains("<total_tokens>"),
        "trailing total-tokens reminder; got {:?}",
        calls[0].messages[2]
    );
}
