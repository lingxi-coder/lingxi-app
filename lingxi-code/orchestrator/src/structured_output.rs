//! The `StructuredOutput` tool for `--json-schema` structured output.
//!
//! Faithful to claude-code's structured-output path: in print mode the model is
//! FORCED (via `tool_choice`) to call a `StructuredOutput` tool whose
//! `input_schema` is the user-supplied JSON schema. Calling it is the model's
//! way of returning the final structured result — so the tool simply CAPTURES
//! its arguments into a shared slot for the print path to validate (against the
//! same schema) and emit, retrying the turn on validation failure.
//!
//! Lives in `orchestrator` (which owns `tool-api`) so both the desktop
//! composition root (`engine_desktop::build`, which registers the tool + forces
//! `tool_choice`) and the CLI print path (which reads the captured value) can
//! reach it. The schema validator + retry budget live in `apps/cli`
//! (`structured_output`), which reads this slot.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tool_api::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolProgressSender,
    ToolStaticContext, ToolUseContext,
};

/// Canonical name of the synthetic structured-output tool.
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "StructuredOutput";

/// Shared slot the [`StructuredOutputTool`] writes the model's structured result
/// into. The print path reads it after the turn to validate + emit.
pub type StructuredOutputSlot = Arc<Mutex<Option<Value>>>;

/// The forced `StructuredOutput` tool. Its `input_schema` IS the user's schema;
/// `call` captures the model's arguments into the shared slot and returns a
/// trivial acknowledgement.
pub struct StructuredOutputTool {
    /// The user-supplied JSON schema, returned verbatim as `input_schema`.
    schema: Value,
    /// Where `call` deposits the model's structured arguments.
    captured: StructuredOutputSlot,
}

impl StructuredOutputTool {
    /// Build the tool for `schema`, capturing the model's call into `slot`.
    #[must_use]
    pub fn new(schema: Value, slot: StructuredOutputSlot) -> Self {
        Self {
            schema,
            captured: slot,
        }
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("return the final response as structured JSON")
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        // claude sets `maxResultSizeChars:1e5` on the StructuredOutput tool (the
        // shared default). The result is a trivial ack ("{\"ok\":true}") so the
        // cap is never approached, but match the binary value for parity.
        100_000
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // Captures into an in-memory slot; no workspace side effect.
        true
    }

    fn result_ends_turn(&self, _result: &ToolCallResult) -> bool {
        // Claude Code's StructuredOutput result carries `endsTurn: true`.
        true
    }

    async fn check_permissions(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
    ) -> permission::PermissionResult {
        // Synthetic output tool — always allowed (the model is forced to call it).
        permission::PermissionResult::Allow {
            reason: permission::PermissionDecisionReason::Other {
                reason: "structured output".to_string(),
            },
            updated_input: None,
            update_destination: None,
            metadata: permission::result::PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        "Return the final structured output".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Call this tool exactly once with the final result. The arguments MUST \
         conform to the provided JSON schema."
            .to_string()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // Capture the model's structured result for the print path to validate.
        // Lock is held only for the store (no await across it).
        if let Ok(mut slot) = self.captured.lock() {
            *slot = Some(input);
        }
        Ok(ToolCallResult {
            data: Value::String("Structured output provided successfully".to_string()),
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
    use serde_json::json;

    #[test]
    fn tool_exposes_the_user_schema_as_input_schema() {
        let schema = json!({ "type": "object", "required": ["x"] });
        let slot: StructuredOutputSlot = Arc::new(Mutex::new(None));
        let tool = StructuredOutputTool::new(schema.clone(), slot);
        assert_eq!(tool.name(), "StructuredOutput");
        assert_eq!(tool.input_schema(), &schema);
    }

    #[tokio::test]
    async fn call_captures_the_models_arguments_into_the_slot() {
        let slot: StructuredOutputSlot = Arc::new(Mutex::new(None));
        let tool = StructuredOutputTool::new(json!({"type": "object"}), slot.clone());
        let (tx, _rx) = tool_api::progress::progress_channel();
        let result = tool
            .call(
                json!({"answer": 42}),
                ToolUseContext::model_seed("test".into()),
                tx,
            )
            .await
            .expect("call ok");
        assert_eq!(
            result.data,
            Value::String("Structured output provided successfully".to_string())
        );
        assert!(
            tool.result_ends_turn(&result),
            "StructuredOutput mirrors the oracle's endsTurn:true result"
        );
        assert_eq!(
            slot.lock().unwrap().as_ref(),
            Some(&json!({"answer": 42})),
            "the model's arguments must be captured for validation"
        );
    }
}
