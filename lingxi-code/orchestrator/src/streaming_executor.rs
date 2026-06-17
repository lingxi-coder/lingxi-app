//! Faithful port of claude-code's `StreamingToolExecutor`
//! (`services/tools/StreamingToolExecutor.ts`). Schedules tool execution as
//! `tool_use` blocks stream in, under concurrency control, buffering results
//! for emission in *received* order. Single-task: all tool futures borrow
//! `&ConversationOrchestrator` and are polled on one `FuturesUnordered`, so no
//! `'static`/spawn is required.

use crate::conversation::ConversationOrchestrator;
use futures::{stream::FuturesUnordered, StreamExt};
use protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use tool_api::ContextModifier;

/// Name of the Bash tool — the ONLY tool whose error cascades to siblings
/// (TS `BASH_TOOL_NAME` guard in `collectResults`).
const BASH_TOOL_NAME: &str = "Bash";

/// Result of one `dispatch_tool_uses_tracked` call routed through the executor:
/// the single result block + the tool's injected messages + context modifiers.
type DispatchOutcome = Result<
    (ContentBlock, Vec<(ConversationMessage, ToolUseId)>, Vec<ContextModifier>),
    crate::error::OrchestratorError,
>;

/// Why a tracked tool is being cancelled (TS `getAbortReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbortReason {
    SiblingError,
    /// PHASE-2: emitted when the user interrupts an in-flight tool; constructed
    /// once the per-tool `CancellationToken` path lands.
    #[allow(dead_code)]
    UserInterrupted,
    StreamingFallback,
}

/// Build the synthetic `tool_result` for a cancelled tool (TS
/// `createSyntheticErrorMessage`). `provider_tool_use_id` is left `None` —
/// the caller copies the tracked tool's `provider_id` in before persisting.
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
// Task 7: concurrency gate + ordered process_queue
// ============================================================================

/// TS `canExecuteTool` (line 129-135): a tool may start if nothing is
/// executing, OR if both the candidate and every executing tool are
/// concurrency-safe.
///
/// `executing_safe_flags` is a slice of the `is_concurrency_safe` flags for
/// every tool currently in the `Executing` state.
#[allow(dead_code)] // wired into the live streaming loop in Task 11
pub(crate) fn can_execute(executing_safe_flags: &[bool], candidate_safe: bool) -> bool {
    executing_safe_flags.is_empty()
        || (candidate_safe && executing_safe_flags.iter().all(|&s| s))
}

// ============================================================================
// StreamingToolExecutor — Task 6: struct + new() + add_tool()
// ============================================================================

/// Manages the lifecycle of all `tool_use` blocks from one streaming assistant
/// turn. Mirrors TS `StreamingToolExecutor` (`StreamingToolExecutor.ts:76-124`
/// for `addTool`).
///
/// All fields are now read by the Task 8 dispatch/abort logic.
pub(crate) struct StreamingToolExecutor<'a> {
    orch: &'a ConversationOrchestrator,
    pub(crate) tools: Vec<TrackedTool>,
    /// Set when any dispatched Bash tool completes with an error; causes queued
    /// siblings to be aborted with `AbortReason::SiblingError` (Task 8).
    has_errored: bool,
    /// Description of the first errored tool, forwarded into sibling abort
    /// messages (Task 8).
    errored_desc: Option<String>,
    /// Set when the turn is discarded (streaming fallback); all queued tools
    /// are cancelled with `AbortReason::StreamingFallback` (Task 8).
    discarded: bool,
    /// In-flight tool futures keyed by their index in `tools`. Polled on the
    /// current task (no spawn); each borrows `&'a orch`.
    inflight: FuturesUnordered<
        std::pin::Pin<Box<dyn std::future::Future<Output = (usize, DispatchOutcome)> + 'a>>,
    >,
}

// The whole executor surface is unused until Task 11 wires it into the live
// streaming loop; suppress dead-code here rather than per-method.
#[allow(dead_code)]
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
            inflight: FuturesUnordered::new(),
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
                let block = synthetic_unknown_tool(id, &name, provider_id.clone());
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

    /// TS `processQueue` (line 140-151): walk the queue IN ORDER; start each
    /// queued tool whose concurrency conditions are met; STOP at the first
    /// queued non-concurrency-safe tool that cannot start yet (preserves
    /// exclusive-tool ordering). After each start the executing set changes, so
    /// we re-evaluate from scratch.
    // Index loop + per-pass rebuild are forced by the borrow checker: `start_tool`
    // takes `&mut self`, so we can't hold an iterator borrow over `self.tools`
    // across a start. N is small (tools per turn), so the rebuild is negligible.
    #[allow(clippy::needless_range_loop)]
    pub(crate) fn process_queue(&mut self) {
        loop {
            let executing_flags: Vec<bool> = self
                .tools
                .iter()
                .filter(|t| t.status == ToolStatus::Executing)
                .map(|t| t.is_concurrency_safe)
                .collect();

            let mut started_any = false;
            for i in 0..self.tools.len() {
                if self.tools[i].status != ToolStatus::Queued {
                    continue;
                }
                let safe = self.tools[i].is_concurrency_safe;
                if can_execute(&executing_flags, safe) {
                    self.start_tool(i);
                    started_any = true;
                    break; // re-evaluate the executing set after each start
                } else if !safe {
                    // An exclusive (non-safe) tool can't start yet → barrier.
                    return;
                }
                // A safe tool that can't start (unsafe tool executing) —
                // keep scanning; TS continues the loop in this case.
            }
            if !started_any {
                return;
            }
        }
    }

    /// Dispatch tool `i` as an in-flight future on `self.inflight`. The future
    /// borrows `&'a orch` and is polled on the current task (no spawn). It
    /// routes through the existing per-tool pipeline
    /// [`crate::turn_loop::dispatch_tool_uses_tracked`] (one tool at a time) so
    /// the hook→permission→registry ordering stays byte-locked.
    fn start_tool(&mut self, i: usize) {
        self.tools[i].status = ToolStatus::Executing;
        let id = self.tools[i].id;
        let name = self.tools[i].name.clone();
        let input = self.tools[i].input.clone();
        let provider_id = self.tools[i].provider_id.clone();
        let orch = self.orch;
        let fut = async move {
            let single = vec![(id, name, input, provider_id)];
            let outcome: DispatchOutcome =
                match crate::turn_loop::dispatch_tool_uses_tracked(orch, &single).await {
                    // `single` has one element, so `pop()` == the only result.
                    Ok((mut blocks, _prevent, injected, modifiers)) => blocks
                        .pop()
                        .map(|b| (b, injected, modifiers))
                        .ok_or_else(|| {
                            crate::error::OrchestratorError::StreamingProtocol(format!(
                                "dispatch returned empty for tool index {i}"
                            ))
                        }),
                    Err(e) => Err(e),
                };
            (i, outcome)
        };
        self.inflight.push(Box::pin(fut));
    }

    /// Await one in-flight tool future, record its result, and trigger the Bash
    /// sibling-error cascade. Returns the completed tool index, or `None` if no
    /// futures are in flight. (TS `executeTool`/`collectResults` completion path.)
    async fn drain_one(&mut self) -> Option<usize> {
        let (i, outcome) = self.inflight.next().await?;
        match outcome {
            Ok((mut block, injected, modifiers)) => {
                let is_err = matches!(&block, ContentBlock::ToolResult { is_error, .. } if *is_error);
                // Only a Bash error cascades to siblings (TS: BASH_TOOL_NAME guard).
                if is_err && self.tools[i].name == BASH_TOOL_NAME {
                    self.has_errored = true;
                    self.errored_desc = Some(tool_description(&self.tools[i]));
                }
                // Copy the provider id onto the result for egress replay.
                set_provider_id(&mut block, self.tools[i].provider_id.clone());
                self.tools[i].result = Some(block);
                self.tools[i].injected = injected;
                self.tools[i].modifiers = modifiers;
                self.tools[i].status = ToolStatus::Completed;
            }
            Err(e) => {
                // A hard orchestrator error → surface as an errored result
                // (analogous to claude-code's outer plumbing catch,
                // toolExecution.ts:480).
                self.tools[i].status = ToolStatus::Completed;
                self.tools[i].result = Some(ContentBlock::ToolResult {
                    tool_use_id: self.tools[i].id,
                    content: format!("<tool_use_error>Error: {e}</tool_use_error>"),
                    is_error: true,
                    provider_tool_use_id: self.tools[i].provider_id.clone(),
                });
            }
        }
        Some(i)
    }

    /// Convert still-`Queued` tools to a synthetic-cancel result once
    /// `has_errored`/`discarded` is set (TS `getAbortReason` on next poll).
    /// PHASE-2: an already-`Executing` sibling is NOT interrupted here (it runs
    /// to completion and keeps its real result); only `Queued` siblings are
    /// cancelled. claude-code interrupts in-flight siblings via the sibling
    /// controller — that arrives with the per-tool `CancellationToken` in
    /// Phase 2.
    fn apply_abort_to_pending(&mut self) {
        let reason = if self.discarded {
            Some(AbortReason::StreamingFallback)
        } else if self.has_errored {
            Some(AbortReason::SiblingError)
        } else {
            None
        };
        let Some(reason) = reason else { return };
        let desc = self.errored_desc.clone();
        for t in &mut self.tools {
            if matches!(t.status, ToolStatus::Queued) && t.result.is_none() {
                let mut block = synthetic_error_block(t.id, reason, desc.as_deref());
                set_provider_id(&mut block, t.provider_id.clone());
                t.result = Some(block);
                t.status = ToolStatus::Completed;
            }
        }
    }

    /// Mark the turn discarded (streaming fallback). Pending tools get a
    /// `StreamingFallback` synthetic result on the next `apply_abort_to_pending`.
    fn discard(&mut self) {
        self.discarded = true;
    }

    #[cfg(test)]
    pub(crate) async fn run_to_completion(
        &mut self,
    ) -> Result<Vec<ContentBlock>, crate::error::OrchestratorError> {
        loop {
            self.apply_abort_to_pending();
            self.process_queue();
            if self.inflight.is_empty() {
                break;
            }
            self.drain_one().await;
        }
        Ok(self
            .tools
            .iter()
            .map(|t| t.result.clone().expect("completed"))
            .collect())
    }
}

/// Short `Name(arg…)` description for sibling-cancel messages
/// (TS `getToolDescription`, StreamingToolExecutor.ts:243-252).
fn tool_description(t: &TrackedTool) -> String {
    let summary = t
        .input
        .get("command")
        .or_else(|| t.input.get("file_path"))
        .or_else(|| t.input.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if summary.is_empty() {
        t.name.clone()
    } else {
        let truncated = if summary.chars().count() > 40 {
            format!("{}\u{2026}", summary.chars().take(40).collect::<String>())
        } else {
            summary.to_owned()
        };
        format!("{}({})", t.name, truncated)
    }
}

/// Copy a provider-issued tool-call id onto a `ToolResult` block's
/// `provider_tool_use_id` for egress replay; no-op for non-`ToolResult` blocks.
fn set_provider_id(block: &mut ContentBlock, provider_id: Option<String>) {
    if let ContentBlock::ToolResult { provider_tool_use_id, .. } = block {
        *provider_tool_use_id = provider_id;
    }
}

// ============================================================================
// Task 9: ordered result drain (TS getCompletedResults / hasUnfinishedTools)
// ============================================================================

/// One drained result ready for the live loop to persist, carrying everything
/// needed to build + assistant-parent the user message (TS getCompletedResults
/// yields one message per result; `assistant_id` is TS `sourceToolAssistantUUID`).
#[allow(dead_code)] // wired in Task 11
pub(crate) struct DrainedResult {
    pub(crate) block: ContentBlock,
    pub(crate) assistant_id: MessageId,
    pub(crate) injected: Vec<(ConversationMessage, ToolUseId)>,
    pub(crate) modifiers: Vec<ContextModifier>,
}

#[allow(dead_code)] // wired in Task 11
impl<'a> StreamingToolExecutor<'a> {
    /// TS `getCompletedResults`: walk tools in order, yield each newly-`Completed`
    /// tool's result (marking it `Yielded`), and STOP at an `Executing`
    /// non-concurrency-safe tool (don't emit past an unfinished exclusive
    /// barrier). Returns results in RECEIVED order.
    pub(crate) fn take_newly_completed(&mut self) -> Vec<DrainedResult> {
        let mut out = Vec::new();
        for t in &mut self.tools {
            match t.status {
                ToolStatus::Completed => {
                    t.status = ToolStatus::Yielded;
                    out.push(DrainedResult {
                        block: t.result.clone().expect("completed tool has result"),
                        assistant_id: t.assistant_id,
                        injected: std::mem::take(&mut t.injected),
                        modifiers: std::mem::take(&mut t.modifiers),
                    });
                }
                ToolStatus::Yielded => continue,
                ToolStatus::Executing if !t.is_concurrency_safe => break,
                _ => {}
            }
        }
        out
    }

    /// TS `hasUnfinishedTools`: any tool not yet `Yielded`.
    pub(crate) fn has_unfinished(&self) -> bool {
        self.tools.iter().any(|t| t.status != ToolStatus::Yielded)
    }
}

/// Build the synthetic `tool_result` for an unknown tool (claude-code `addTool`
/// line 78-84 / `toolExecution.ts:401`). Shared by the streaming executor and
/// the batched dispatch (`turn_loop`) so this parity-critical string lives in
/// exactly one place.
pub(crate) fn synthetic_unknown_tool(
    id: ToolUseId,
    name: &str,
    provider_id: Option<String>,
) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: id,
        content: format!(
            "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
        ),
        is_error: true,
        provider_tool_use_id: provider_id,
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

    // ============================================================================
    // Task 7: can_execute + process_queue tests
    // ============================================================================

    use super::can_execute;

    #[test]
    fn can_execute_respects_concurrency_safety() {
        assert!(can_execute(&[], true));
        assert!(can_execute(&[], false));         // nothing running → ok
        assert!(can_execute(&[true, true], true)); // all safe + candidate safe → ok
        assert!(!can_execute(&[true], false));    // candidate unsafe, something running → no
        assert!(!can_execute(&[false], true));    // an unsafe tool running → no
        assert!(!can_execute(&[true, true], false)); // many safe running, unsafe candidate → no
    }

    /// A minimal concurrency-UNSAFE tool for ordering/barrier tests.
    struct UnsafeTool;

    #[async_trait]
    impl Tool for UnsafeTool {
        fn name(&self) -> &str { "UnsafeTool" }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
        fn max_result_size_chars(&self) -> usize { 1024 * 1024 }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
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
            "unsafe-tool".into()
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

    /// Build an orchestrator with both SafeTool and UnsafeTool.
    fn orch_with_both_tools() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(SafeTool) as Arc<dyn Tool>);
        registry.register_builtin(Arc::new(UnsafeTool) as Arc<dyn Tool>);
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

    /// [safe, unsafe, safe]: first safe tool starts; the unsafe is a barrier;
    /// the third safe tool must NOT start out of order.
    #[tokio::test]
    async fn process_queue_starts_safe_tool_and_barriers_on_unsafe() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        // First safe starts; unsafe is a barrier (can't run while safe executes);
        // the third safe sits behind the barrier and must NOT start out of order.
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
    }

    /// [safe, safe]: both concurrent-safe tools start.
    #[tokio::test]
    async fn process_queue_starts_all_safe_tools_concurrently() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
    }

    /// [unsafe, safe]: the unsafe tool starts first (nothing executing), then
    /// the following safe tool stays Queued (exclusive holds the barrier).
    #[tokio::test]
    async fn process_queue_unsafe_first_blocks_following_safe() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
    }

    /// [safe, safe, unsafe, safe]: both leading safe tools start; the unsafe is
    /// a barrier; the trailing safe stays Queued behind it.
    #[tokio::test]
    async fn process_queue_two_safe_then_unsafe_barrier() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
        assert_eq!(exec.tools[3].status, ToolStatus::Queued);
    }

    /// A SAFE queued tool blocked by an already-Executing UNSAFE tool does NOT
    /// barrier (only an unsafe queued tool barriers) — process_queue scans past
    /// it and starts nothing. Exercises the "keep scanning" continuation branch
    /// that requires pre-existing Executing state.
    #[tokio::test]
    async fn process_queue_safe_blocked_by_executing_unsafe_keeps_scanning() {
        let orch = orch_with_both_tools();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "UnsafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        // Simulate the unsafe tool already running (as if a prior process_queue
        // started it and it has not completed yet).
        exec.tools[0].status = ToolStatus::Executing;
        exec.process_queue();
        // Both safe tools are blocked by the executing unsafe tool; neither
        // starts, and the safe ones do NOT barrier (scan continues past them).
        assert_eq!(exec.tools[0].status, ToolStatus::Executing);
        assert_eq!(exec.tools[1].status, ToolStatus::Queued);
        assert_eq!(exec.tools[2].status, ToolStatus::Queued);
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

    // ============================================================================
    // Task 8: real dispatch + Bash sibling-error cascade
    // ============================================================================

    /// A tool named "Bash" that is concurrency-UNSAFE (so it barriers a queued
    /// sibling) and always errors from `call` — the dispatch turns the `Err`
    /// into an `is_error: true` ToolResult block.
    struct BashErrorTool;

    #[async_trait]
    impl Tool for BashErrorTool {
        fn name(&self) -> &str { "Bash" }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
        fn max_result_size_chars(&self) -> usize { 1024 * 1024 }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
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
            "bash".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String { String::new() }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Err(ToolError::Internal("boom".into()))
        }
    }

    /// A NON-Bash, concurrency-UNSAFE tool that always errors — used to prove a
    /// non-Bash error does NOT cascade to a queued sibling.
    struct ReaderErrorTool;

    #[async_trait]
    impl Tool for ReaderErrorTool {
        fn name(&self) -> &str { "Reader" }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
        fn max_result_size_chars(&self) -> usize { 1024 * 1024 }
        // Unsafe so it barriers the queued sibling and runs FIRST (alone),
        // making the ordering deterministic for the non-cascade assertion.
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
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
            "reader".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String { String::new() }
        async fn call(
            &self,
            _input: serde_json::Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Err(ToolError::Internal("boom".into()))
        }
    }

    fn orch_with_bash_error_and_safe() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(BashErrorTool) as Arc<dyn Tool>);
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

    fn orch_with_reader_error_and_safe() -> ConversationOrchestrator {
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ReaderErrorTool) as Arc<dyn Tool>);
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
    async fn bash_error_cancels_a_queued_sibling() {
        let orch = orch_with_bash_error_and_safe();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "Bash".into(), json!({"command":"false"}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        let results = exec.run_to_completion().await.unwrap();
        // tools[0] = Bash's real error; tools[1] = SafeTool got the synthetic
        // sibling-cancel (Bash, being unsafe, barriers the queued sibling so it
        // is still Queued when the error cascade fires).
        let ContentBlock::ToolResult { is_error, .. } = &results[0] else { panic!() };
        assert!(*is_error, "Bash result should be an error");
        let ContentBlock::ToolResult { content, is_error, .. } = &results[1] else { panic!() };
        assert!(*is_error);
        assert!(
            content.contains("Cancelled: parallel tool call"),
            "got: {content}"
        );
    }

    #[tokio::test]
    async fn non_bash_error_does_not_cancel_a_queued_sibling() {
        let orch = orch_with_reader_error_and_safe();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        // Reader is unsafe → barriers → runs first and alone → errors. Because
        // it is NOT Bash, has_errored stays false and the queued SafeTool then
        // runs and gets its REAL (non-synthetic) result.
        exec.add_tool(ToolUseId::new(), "Reader".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        let results = exec.run_to_completion().await.unwrap();
        assert!(!exec.has_errored, "non-Bash error must NOT set has_errored");
        let ContentBlock::ToolResult { content: r0, is_error: e0, .. } = &results[0] else { panic!() };
        assert!(*e0, "Reader result should be an error");
        assert!(r0.contains("boom"), "Reader keeps its real error: {r0}");
        // SafeTool ran for real — not a sibling-cancel.
        let ContentBlock::ToolResult { content: r1, is_error: e1, .. } = &results[1] else { panic!() };
        assert!(!*e1, "SafeTool should succeed, got error: {r1}");
        assert!(
            !r1.contains("Cancelled: parallel tool call"),
            "SafeTool must NOT be sibling-cancelled: {r1}"
        );
    }

    #[test]
    fn tool_description_priority_and_truncation() {
        // command field, > 40 chars → truncate to 40 + ellipsis.
        let long = "x".repeat(45);
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Bash".into(),
            input: json!({ "command": long }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: false,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        let desc = tool_description(&t);
        assert_eq!(desc, format!("Bash({}\u{2026})", "x".repeat(40)));

        // file_path fallback (no command), short → no truncation.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Read".into(),
            input: json!({ "file_path": "/tmp/a.txt" }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "Read(/tmp/a.txt)");

        // pattern fallback.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "Grep".into(),
            input: json!({ "pattern": "foo" }),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "Grep(foo)");

        // empty input → bare name.
        let t = TrackedTool {
            id: ToolUseId::new(),
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: MessageId::new(),
            status: ToolStatus::Queued,
            is_concurrency_safe: true,
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        };
        assert_eq!(tool_description(&t), "SafeTool");
    }

    // ============================================================================
    // Task 9: take_newly_completed + has_unfinished tests
    // ============================================================================

    /// Test 1: Two SafeTools driven to completion → take_newly_completed returns
    /// both in received order; a second call returns empty; statuses become Yielded.
    #[tokio::test]
    async fn take_newly_completed_returns_results_in_received_order() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        exec.process_queue();
        while !exec.inflight.is_empty() {
            exec.drain_one().await;
        }
        // Both should be Completed now.
        assert_eq!(exec.tools[0].status, ToolStatus::Completed);
        assert_eq!(exec.tools[1].status, ToolStatus::Completed);

        let results = exec.take_newly_completed();
        assert_eq!(results.len(), 2, "expected both tools drained");
        // Statuses should now be Yielded.
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);
        assert_eq!(exec.tools[1].status, ToolStatus::Yielded);

        // Second call returns empty (all already Yielded).
        let results2 = exec.take_newly_completed();
        assert!(results2.is_empty(), "second call must return empty");
    }

    /// Test 2: Unknown tool (already Completed at add_tool time) is immediately
    /// yielded by take_newly_completed without calling process_queue.
    #[tokio::test]
    async fn take_newly_completed_yields_unknown_tool_immediately() {
        let orch = orch_empty();
        let mut exec = StreamingToolExecutor::new(&orch);
        exec.add_tool(ToolUseId::new(), "NoSuchTool".into(), json!({}), None, MessageId::new());
        // No process_queue, no drain_one — it's already Completed.
        assert_eq!(exec.tools[0].status, ToolStatus::Completed);

        let results = exec.take_newly_completed();
        assert_eq!(results.len(), 1, "unknown tool result should be drained");
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);

        let ContentBlock::ToolResult { is_error, .. } = &results[0].block else {
            panic!("expected ToolResult block")
        };
        assert!(*is_error, "unknown-tool block must be an error");
    }

    /// Test 3: Exclusive-barrier stop.
    /// tool[0] = Completed (safe), tool[1] = Executing+unsafe, tool[2] = Completed (safe).
    /// take_newly_completed must yield ONLY tool[0] and stop at tool[1].
    #[tokio::test]
    async fn take_newly_completed_stops_at_executing_exclusive_tool() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);

        // We build a contrived state by directly constructing TrackedTools.
        let id0 = ToolUseId::new();
        let id1 = ToolUseId::new();
        let id2 = ToolUseId::new();
        let result_block = ContentBlock::ToolResult {
            tool_use_id: id0,
            content: "done".into(),
            is_error: false,
            provider_tool_use_id: None,
        };
        let result_block2 = ContentBlock::ToolResult {
            tool_use_id: id2,
            content: "also done".into(),
            is_error: false,
            provider_tool_use_id: None,
        };

        exec.tools.push(TrackedTool {
            id: id0,
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Completed,
            is_concurrency_safe: true,
            result: Some(result_block),
            injected: Vec::new(),
            modifiers: Vec::new(),
        });
        exec.tools.push(TrackedTool {
            id: id1,
            name: "UnsafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Executing,
            is_concurrency_safe: false,  // exclusive barrier
            result: None,
            injected: Vec::new(),
            modifiers: Vec::new(),
        });
        exec.tools.push(TrackedTool {
            id: id2,
            name: "SafeTool".into(),
            input: json!({}),
            provider_id: None,
            assistant_id: a,
            status: ToolStatus::Completed,
            is_concurrency_safe: true,
            result: Some(result_block2),
            injected: Vec::new(),
            modifiers: Vec::new(),
        });

        let results = exec.take_newly_completed();
        // Only tool[0] should be emitted; tool[1] is the barrier; tool[2] is skipped.
        assert_eq!(results.len(), 1, "only the pre-barrier completed tool should be drained");
        assert_eq!(exec.tools[0].status, ToolStatus::Yielded);
        // tool[1] still Executing (we don't touch it).
        assert_eq!(exec.tools[1].status, ToolStatus::Executing);
        // tool[2] still Completed (was NOT emitted past the barrier).
        assert_eq!(exec.tools[2].status, ToolStatus::Completed);
    }

    /// Test 4: has_unfinished returns true when there are Queued/Executing tools,
    /// and false once all tools are Yielded.
    #[tokio::test]
    async fn has_unfinished_tracks_non_yielded_tools() {
        let orch = orch_with_safe_tool();
        let a = MessageId::new();
        let mut exec = StreamingToolExecutor::new(&orch);
        // No tools at all → nothing unfinished.
        assert!(!exec.has_unfinished(), "empty executor must have no unfinished tools");

        exec.add_tool(ToolUseId::new(), "SafeTool".into(), json!({}), None, a);
        assert!(exec.has_unfinished(), "Queued tool means unfinished");

        exec.process_queue();
        assert!(exec.has_unfinished(), "Executing tool still unfinished");

        while !exec.inflight.is_empty() {
            exec.drain_one().await;
        }
        assert!(exec.has_unfinished(), "Completed but not yet Yielded still unfinished");

        exec.take_newly_completed();
        assert!(!exec.has_unfinished(), "all Yielded → no unfinished tools");
    }
}
