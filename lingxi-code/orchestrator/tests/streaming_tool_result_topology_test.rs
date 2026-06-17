//! Sub-task 11b CAPSTONE topology test: wiring the `StreamingToolExecutor` into
//! the live streaming turn flips tool-result persistence to claude-code's
//! per-result, assistant-parented shape (TS `StreamingToolExecutor` +
//! `query.ts:826-862` + `sessionStorage` `sourceToolAssistantUUID → parentUuid`).
//!
//! For a streaming assistant turn that requests ≥2 tools, this asserts:
//!   1. Each `tool_result` is its OWN `user` message in `session.history`
//!      (N parented messages — NOT one batched user message carrying all N
//!      result blocks).
//!   2. Each result's JSONL `parentUuid` == the assistant line's uuid (every
//!      result parents to the assistant that requested it).
//!   3. The N results appear in RECEIVED order (the in-stream tool order).

use async_trait::async_trait;
use orchestrator::test_support::{
    content_block_start_tool_use, content_block_stop, input_json_delta, message_delta_stop,
    message_start, message_stop, MockApiClient, MockOutputStream, MockStreamingApiClient,
    NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use platform_posix::fs::PosixFileSystem;
use protocol::{ContentBlock, ConversationMessage, ToolUseId};
use serde_json::json;
use session::jsonl::reader::JsonlReader;
use session::jsonl::writer::JsonlWriter;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tool_api::progress::ToolProgressSender;
use tool_api::registry::ToolRegistry;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use traits::FileSystem;

/// A trivial concurrency-safe tool whose result echoes its own name, so the
/// per-result blocks can be matched back to received (in-stream) order.
struct EchoTool {
    name: &'static str,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        self.name
    }
    fn input_schema(&self) -> &serde_json::Value {
        static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
            once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
        &SCHEMA
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024 * 1024
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "test".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
        self.name.into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        String::new()
    }
    async fn call(
        &self,
        _input: serde_json::Value,
        _ctx: tool_api::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        Ok(ToolCallResult {
            data: json!({ "content": format!("result-of-{}", self.name) }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

fn registry_with_three() -> Arc<ToolRegistry> {
    let mut r = ToolRegistry::new();
    r.register_builtin(Arc::new(EchoTool { name: "Alpha" }));
    r.register_builtin(Arc::new(EchoTool { name: "Bravo" }));
    r.register_builtin(Arc::new(EchoTool { name: "Charlie" }));
    Arc::new(r)
}

#[tokio::test]
async fn streaming_tool_results_are_per_result_assistant_parented() {
    let dir = tempdir().expect("tempdir");
    let session_path = dir.path().join("session.jsonl");
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(dir.path().to_path_buf()));
    let writer = Arc::new(JsonlWriter::new(session_path.clone(), fs.clone()));

    // Three tool_use blocks in one streaming assistant turn (received order
    // Alpha → Bravo → Charlie), then a second turn that ends.
    let id_a = ToolUseId::new();
    let id_b = ToolUseId::new();
    let id_c = ToolUseId::new();
    let turn1 = scripted![
        message_start("m1", "claude-opus-4-7"),
        content_block_start_tool_use(0, id_a, "Alpha"),
        input_json_delta(0, "{}"),
        content_block_stop(0),
        content_block_start_tool_use(1, id_b, "Bravo"),
        input_json_delta(1, "{}"),
        content_block_stop(1),
        content_block_start_tool_use(2, id_c, "Charlie"),
        input_json_delta(2, "{}"),
        content_block_stop(2),
        message_delta_stop("tool_use"),
        message_stop(),
    ];
    let turn2 = scripted![
        message_start("m2", "claude-opus-4-7"),
        message_delta_stop("end_turn"),
        message_stop(),
    ];

    let api = Arc::new(MockStreamingApiClient::with_turns(vec![turn1, turn2]));
    let batched = Arc::new(MockApiClient::new(Vec::new()));

    let orch = ConversationOrchestrator::new_with_streaming(
        OrchestratorConfig::default(),
        batched,
        api,
        registry_with_three(),
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        PathBuf::from("/tmp"),
    )
    .with_jsonl_writer(writer);

    let _ = orch.run_turn_streaming("call three tools").await.expect("ok");

    // ── (1) + (3): N per-result user messages in session.history, in received order
    let session = orch.session();
    let s = session.lock().await;

    // Collect each User message that carries exactly one ToolResult block —
    // these are the per-result tool_result user messages.
    let tool_result_msgs: Vec<&Vec<ContentBlock>> = s
        .history
        .iter()
        .filter_map(|m| match m {
            ConversationMessage::User { content, .. }
                if content.len() == 1
                    && matches!(content[0], ContentBlock::ToolResult { .. }) =>
            {
                Some(content)
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        tool_result_msgs.len(),
        3,
        "expected 3 separate tool_result user messages (per-result topology), \
         got {} — history: {:?}",
        tool_result_msgs.len(),
        s.history
    );

    // No single batched user message should carry more than one tool_result.
    assert!(
        !s.history.iter().any(|m| matches!(m,
            ConversationMessage::User { content, .. }
                if content.iter().filter(|b| matches!(b, ContentBlock::ToolResult { .. })).count() > 1)),
        "no user message may batch multiple tool_result blocks (per-result topology): {:?}",
        s.history
    );

    // Received order: Alpha, Bravo, Charlie (by tool_use_id).
    let result_ids: Vec<ToolUseId> = tool_result_msgs
        .iter()
        .map(|content| match &content[0] {
            ContentBlock::ToolResult { tool_use_id, .. } => *tool_use_id,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        result_ids,
        vec![id_a, id_b, id_c],
        "tool_result user messages must appear in RECEIVED (in-stream) order"
    );
    drop(s);

    // ── (2): each result's JSONL parentUuid == the assistant line's uuid
    let reader = JsonlReader::new(session_path, fs);
    let msgs = reader.read_all().await.expect("read_all");

    // Locate the assistant line that carries the tool_use blocks (turn 1).
    let assistant_line = msgs
        .iter()
        .find(|m| m.message_type == "assistant")
        .expect("assistant line present");
    let assistant_uuid = assistant_line.uuid.clone();

    // Every tool_result user line (user lines whose single content block is a
    // tool_result) must parent to that assistant uuid.
    let tool_result_lines: Vec<_> = msgs
        .iter()
        .filter(|m| {
            m.message_type == "user"
                && serde_json::to_string(&m.message)
                    .map(|s| s.contains("tool_result"))
                    .unwrap_or(false)
        })
        .collect();

    assert_eq!(
        tool_result_lines.len(),
        3,
        "expected 3 tool_result JSONL user lines, got {}",
        tool_result_lines.len()
    );
    for line in &tool_result_lines {
        assert_eq!(
            line.parent_uuid.as_deref(),
            Some(assistant_uuid.as_str()),
            "each tool_result line must parent to the requesting assistant uuid \
             (TS sourceToolAssistantUUID)"
        );
    }
}
