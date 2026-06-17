//! Faithful port of claude-code's `StreamingToolExecutor`
//! (`services/tools/StreamingToolExecutor.ts`). Schedules tool execution as
//! `tool_use` blocks stream in, under concurrency control, buffering results
//! for emission in *received* order. Single-task: all tool futures borrow
//! `&ConversationOrchestrator` and are polled on one `FuturesUnordered`, so no
//! `'static`/spawn is required.

use crate::conversation::ConversationOrchestrator;
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use tool_api::ContextModifier;

/// Why a tracked tool is being cancelled (TS `getAbortReason`).
#[allow(dead_code)] // wired into the executor in Task 8
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbortReason {
    SiblingError,
    UserInterrupted,
    StreamingFallback,
}

/// Build the synthetic `tool_result` for a cancelled tool (TS
/// `createSyntheticErrorMessage`). `provider_tool_use_id` is left `None` —
/// the caller copies the tracked tool's `provider_id` in before persisting.
#[allow(dead_code)] // wired into the executor in Task 8
pub(crate) fn synthetic_error_block(
    tool_use_id: ToolUseId,
    reason: AbortReason,
    errored_desc: Option<&str>,
) -> ContentBlock {
    let content = match reason {
        AbortReason::StreamingFallback =>
            "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>".to_string(),
        // PHASE-2: claude-code uses REJECT_MESSAGE + withMemoryCorrectionHint here.
        AbortReason::UserInterrupted =>
            "<tool_use_error>User rejected tool use</tool_use_error>".to_string(),
        AbortReason::SiblingError => match errored_desc {
            Some(desc) => format!("<tool_use_error>Cancelled: parallel tool call {desc} errored</tool_use_error>"),
            None => "<tool_use_error>Cancelled: parallel tool call errored</tool_use_error>".to_string(),
        },
    };
    ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error: true,
        provider_tool_use_id: None,
    }
}

/// Lifecycle of one tracked tool, mirroring TS `ToolStatus`.
#[allow(dead_code)] // variants wired in Tasks 5-11
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolStatus {
    Queued,
    Executing,
    Completed,
    Yielded,
}

/// One `tool_use` block under management. `assistant_id` is the id of the
/// assistant message that requested this call — it becomes the JSONL
/// `parentUuid` of the result (TS `sourceToolAssistantUUID`).
#[allow(dead_code)] // fields wired in Tasks 5-11
pub(crate) struct TrackedTool {
    pub(crate) id: ToolUseId,
    pub(crate) name: String,
    pub(crate) input: serde_json::Value,
    pub(crate) provider_id: Option<String>,
    pub(crate) assistant_id: MessageId,
    pub(crate) status: ToolStatus,
    pub(crate) is_concurrency_safe: bool,
    /// The result block once `Completed` (the unknown-tool case fills it
    /// synchronously at `add_tool` time).
    pub(crate) result: Option<ContentBlock>,
    /// Tool-injected follow-up messages (SKILLEXEC.3) + context modifiers,
    /// threaded through unchanged from `dispatch_tool_uses_tracked`.
    pub(crate) injected: Vec<(ConversationMessage, ToolUseId)>,
    pub(crate) modifiers: Vec<ContextModifier>,
}

// ============================================================================
// StreamingToolExecutor — Task 6: struct + new() + add_tool()
// ============================================================================

/// Manages the lifecycle of all `tool_use` blocks from one streaming assistant
/// turn. Mirrors TS `StreamingToolExecutor` (`StreamingToolExecutor.ts:76-124`
/// for `addTool`).
///
/// Fields annotated "scheduling/abort fields wired in Tasks 7-8" are present
/// now so later tasks can add their logic without struct churn.
#[allow(dead_code)] // scheduling/abort fields wired in Tasks 7-8
pub(crate) struct StreamingToolExecutor<'a> {
    orch: &'a ConversationOrchestrator,
    pub(crate) tools: Vec<TrackedTool>,
    /// Set when any dispatched tool completes with an error; causes siblings
    /// to be aborted with `AbortReason::SiblingError` (Task 8).
    has_errored: bool,
    /// Description of the first errored tool, forwarded into sibling abort
    /// messages (Task 8).
    errored_desc: Option<String>,
    /// Set when the turn is discarded (streaming fallback); all in-flight
    /// tools are cancelled with `AbortReason::StreamingFallback` (Task 8).
    discarded: bool,
}

impl<'a> StreamingToolExecutor<'a> {
    /// Construct a fresh executor borrowing the given orchestrator for the
    /// duration of the streaming turn.
    pub(crate) fn new(orch: &'a ConversationOrchestrator) -> Self {
        Self {
            orch,
            tools: Vec::new(),
            has_errored: false,
            errored_desc: None,
            discarded: false,
        }
    }

    /// Register one `tool_use` block received from the stream.
    ///
    /// - If the tool is **not in the registry**, a `TrackedTool` already
    ///   `Completed` is pushed with an unknown-tool error block (short-circuit,
    ///   mirrors TS `addTool` lines 76-85).
    /// - If the tool **is known**, classify `is_concurrency_safe` via the tool's
    ///   own method and push a `Queued` entry (mirrors TS lines 86-124).
    pub(crate) fn add_tool(
        &mut self,
        id: ToolUseId,
        name: String,
        input: serde_json::Value,
        provider_id: Option<String>,
        assistant_id: MessageId,
    ) {
        match self.orch.tools.find_by_name(&name) {
            None => {
                let mut block = synthetic_unknown_tool(id, &name);
                if let ContentBlock::ToolResult { provider_tool_use_id, .. } = &mut block {
                    *provider_tool_use_id = provider_id.clone();
                }
                self.tools.push(TrackedTool {
                    id,
                    name,
                    input,
                    provider_id,
                    assistant_id,
                    status: ToolStatus::Completed,
                    is_concurrency_safe: true,
                    result: Some(block),
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
            Some(tool) => {
                // NOTE divergence: claude-code parses input against the schema
                // first and marks unparseable input `isConcurrencySafe=false`.
                // LingXi's is_concurrency_safe takes raw &Value and returns the
                // tool's conservative default on malformed input — acceptable
                // for Phase 1.
                let safe = tool.is_concurrency_safe(&input);
                self.tools.push(TrackedTool {
                    id,
                    name,
                    input,
                    provider_id,
                    assistant_id,
                    status: ToolStatus::Queued,
                    is_concurrency_safe: safe,
                    result: None,
                    injected: Vec::new(),
                    modifiers: Vec::new(),
                });
            }
        }
    }
}

/// Build the synthetic `tool_result` for an unknown tool (TS `addTool` line
/// 78-84). `provider_tool_use_id` is left `None` — the caller copies the
/// tracked tool's `provider_id` in before persisting.
fn synthetic_unknown_tool(id: ToolUseId, name: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id,
        content: format!(
            "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
        ),
        is_error: true,
        provider_tool_use_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_enum_roundtrips() {
        assert_eq!(ToolStatus::Queued, ToolStatus::Queued);
        assert_ne!(ToolStatus::Queued, ToolStatus::Yielded);
    }

    #[test]
    fn sibling_error_synthetic_with_description() {
        let block = synthetic_error_block(ToolUseId::new(), AbortReason::SiblingError, Some("Bash(rm -rf /tmp/x)"));
        let ContentBlock::ToolResult { content, is_error, .. } = block else { panic!() };
        assert!(is_error);
        assert_eq!(content, "<tool_use_error>Cancelled: parallel tool call Bash(rm -rf /tmp/x) errored</tool_use_error>");
    }

    #[test]
    fn sibling_error_synthetic_without_description() {
        let block = synthetic_error_block(ToolUseId::new(), AbortReason::SiblingError, None);
        let ContentBlock::ToolResult { content, .. } = block else { panic!() };
        assert_eq!(content, "<tool_use_error>Cancelled: parallel tool call errored</tool_use_error>");
    }

    #[test]
    fn streaming_fallback_synthetic() {
        let block = synthetic_error_block(ToolUseId::new(), AbortReason::StreamingFallback, None);
        let ContentBlock::ToolResult { content, .. } = block else { panic!() };
        assert_eq!(content, "<tool_use_error>Error: Streaming fallback - tool execution discarded</tool_use_error>");
    }

    // ============================================================================
    // Task 6: StreamingToolExecutor::add_tool tests
    // ============================================================================

    use crate::conversation::ConversationOrchestrator;
    use crate::test_support::{noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider};
    use crate::OrchestratorConfig;
    use async_trait::async_trait;
    use protocol::MessageId;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    /// Build an orchestrator with an EMPTY tool registry. Used to exercise the
    /// unknown-tool short-circuit path.
    fn orch_empty() -> ConversationOrchestrator {
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(ToolRegistry::new()),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    /// A minimal concurrency-safe tool for the "known tool" path test.
    struct SafeTool;

    #[async_trait]
    impl Tool for SafeTool {
        fn name(&self) -> &str { "SafeTool" }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
        fn max_result_size_chars(&self) -> usize { 1024 * 1024 }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other { reason: "test".into() },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "safe-tool".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String { String::new() }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "content": "ok" }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    /// Build an orchestrator whose registry contains a single `SafeTool`.
    fn orch_with_safe_tool() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(SafeTool) as Arc<dyn Tool>);
        ConversationOrchestrator::new(
            OrchestratorConfig::default(),
            Arc::new(MockApiClient::new(vec![])),
            Arc::new(registry),
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        )
    }

    #[tokio::test]
    async fn add_unknown_tool_completes_immediately_with_wrapper() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "Nope".into(), json!({}), None, MessageId::new());
        let t = &exec.tools[0];
        assert_eq!(t.status, ToolStatus::Completed);
        assert!(t.is_concurrency_safe);
        let ContentBlock::ToolResult { content, is_error, .. } = t.result.as_ref().unwrap() else {
            panic!("expected ToolResult block")
        };
        assert!(*is_error);
        assert_eq!(
            content,
            "<tool_use_error>Error: No such tool available: Nope</tool_use_error>"
        );
    }

    #[tokio::test]
    async fn add_known_concurrency_safe_tool_is_queued() {
        let orch = orch_with_safe_tool();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, MessageId::new());
        let t = &exec.tools[0];
        assert_eq!(t.status, ToolStatus::Queued);
        assert!(t.is_concurrency_safe);
        assert!(t.result.is_none());
    }

    #[tokio::test]
    async fn add_unknown_tool_sets_provider_id_on_result() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(
            ToolUseId::new(),
            "Ghost".into(),
            json!({}),
            Some("prov-abc-123".into()),
            MessageId::new(),
        );
        let t = &exec.tools[0];
        let ContentBlock::ToolResult { provider_tool_use_id, .. } = t.result.as_ref().unwrap()
        else {
            panic!()
        };
        assert_eq!(provider_tool_use_id.as_deref(), Some("prov-abc-123"));
    }
}
