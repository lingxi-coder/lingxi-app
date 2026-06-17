//! Faithful port of claude-code's `StreamingToolExecutor`
//! (`services/tools/StreamingToolExecutor.ts`). Schedules tool execution as
//! `tool_use` blocks stream in, under concurrency control, buffering results
//! for emission in *received* order. Single-task: all tool futures borrow
//! `&ConversationOrchestrator` and are polled on one `FuturesUnordered`, so no
//! `'static`/spawn is required.

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
}
