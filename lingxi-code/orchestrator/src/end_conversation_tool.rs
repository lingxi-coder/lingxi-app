//! The `EndConversation` tool (new in claude-code 2.1.206) — lets the model end
//! the current conversation in extreme cases of sustained abuse (or a requested
//! demonstration). Gated behind a model floor + the `tengu_umber_kestrel` GB
//! flag; DEFAULT OFF, so the composition root only registers it when enabled.
//!
//! Lives in `orchestrator` (which owns `tool-api`) alongside the byte-locked
//! strings in [`crate::prompt::end_conversation`]. This is CHUNK A — the tool
//! DEFINITION (metadata + schema + a `call` that reports the end result). The
//! two-call confirmation flow (`lastAssistantTurnCalledEndConversation`) and the
//! actual conversation termination live in the turn loop (CHUNK B), which reads
//! the tool call and decides whether to end (2nd consecutive call) or return the
//! re-read reminder (1st call).

use async_trait::async_trait;
use serde_json::{json, Value};
use tool_api::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolProgressSender,
    ToolStaticContext, ToolUseContext,
};

use crate::prompt::end_conversation as ec;

/// The `EndConversation` tool. `enabled` mirrors claude-code
/// `isEndConversationToolEnabled` (model floor `NWn` && GB `tengu_umber_kestrel`)
/// — the composition root computes it; DEFAULT `false` keeps the tool
/// unregistered/inert so the prompt + tool set stay byte-identical.
pub struct EndConversationTool {
    schema: Value,
    enabled: bool,
}

impl EndConversationTool {
    /// Build the tool. `enabled` should be the resolved
    /// `isEndConversationToolEnabled` signal (default OFF).
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            schema: ec::input_schema(),
            enabled,
        }
    }
}

impl Default for EndConversationTool {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl Tool for EndConversationTool {
    fn name(&self) -> &str {
        ec::END_CONVERSATION_TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        // `isEndConversationToolEnabled` (model-floor && umber_kestrel GB).
        // DEFAULT OFF → tool absent → byte-identical prompt + tool set.
        self.enabled
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // claude-code `isReadOnly(){return!0}`.
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // claude-code `isConcurrencySafe(){return!1}`.
        false
    }

    fn max_result_size_chars(&self) -> usize {
        ec::END_CONVERSATION_MAX_RESULT_SIZE_CHARS
    }

    async fn check_permissions(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> permission::PermissionResult {
        // claude-code `checkPermissions(){return{behavior:"allow",updatedInput}}`.
        permission::PermissionResult::Allow {
            reason: permission::PermissionDecisionReason::Other {
                reason: "end conversation".to_string(),
            },
            updated_input: Some(input.clone()),
            update_destination: None,
            metadata: permission::result::PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        // claude-code `description(){return G2r}` — the full text.
        ec::render_prompt(ec::END_CONVERSATION_TOOL_NAME)
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        // claude-code `prompt(){return G2r}` — same full text as description().
        ec::render_prompt(ec::END_CONVERSATION_TOOL_NAME)
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // CHUNK A: report the end result per the `v0y` output schema
        // ({ended, message}). The turn loop's two-call gate (CHUNK B) decides
        // whether this call actually terminates the conversation (2nd
        // consecutive call) or is converted to the re-read reminder (1st call).
        Ok(ToolCallResult {
            data: json!({
                "ended": true,
                "message": ec::END_CONVERSATION_ENDED_MESSAGE,
            }),
            model_content: None,
            new_messages: Vec::new(),
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_and_shaped() {
        let t = EndConversationTool::default();
        assert_eq!(t.name(), "EndConversation");
        assert!(!t.is_enabled(&ToolStaticContext::default()));
        assert!(t.should_defer());
        assert!(t.is_read_only(&json!({})));
        assert!(!t.is_concurrency_safe(&json!({})));
        assert_eq!(t.max_result_size_chars(), 10_000);
        assert_eq!(t.input_schema()["additionalProperties"], false);
    }

    #[tokio::test]
    async fn description_and_prompt_are_the_full_g2r() {
        let t = EndConversationTool::new(true);
        let d = t
            .description(&json!({}), &DescriptionOptions { is_non_interactive_session: false })
            .await;
        assert!(t.is_enabled(&ToolStaticContext::default()));
        assert!(d.starts_with("End the current conversation. Use only for sustained user abuse"));
        assert!(d.ends_with("and then use the EndConversation tool to do so."));
        assert_eq!(d, t.prompt(&PromptOptions::default()).await);
    }

    #[tokio::test]
    async fn call_reports_ended_true_with_end_message() {
        let t = EndConversationTool::new(true);
        let (tx, _rx) = tool_api::progress::progress_channel();
        let r = t
            .call(json!({}), ToolUseContext::model_seed("x".into()), tx)
            .await
            .expect("ok");
        assert_eq!(r.data["ended"], true);
        assert_eq!(
            r.data["message"],
            "Claude ended the conversation. To continue, please start a new session."
        );
    }
}
