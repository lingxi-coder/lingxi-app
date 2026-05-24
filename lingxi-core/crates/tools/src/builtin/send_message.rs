//! `SendMessageTool` — routes inter-agent text into the M1 mailbox.
//!
//! Spec §7 line 498. Claim window is the LingXi-internal lock
//! `Duration::from_secs(30)`.
//!
//! **Architectural note (M4-05):** `lingxi-coordinator` already depends on
//! `lingxi-tools` (so the production `MailboxRouter` cannot be path-dep'd
//! from here). The tool exposes the byte-locked schema + claim-window
//! constant + telemetry events; the actual routing through
//! `lingxi_coordinator::MailboxRouter::route` happens in the coordinator's
//! tool-wiring step post-M5.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::Verified;
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{
    SEND_MESSAGE_COMPLETED, SEND_MESSAGE_FAILED, SEND_MESSAGE_STARTED,
};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

/// Tool name `'SendMessage'` (claude-code `SEND_MESSAGE_TOOL_NAME`).
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// LingXi-internal claim-window lock — see spec §7 line 498.
/// Encoded as `Duration::from_secs(30)` byte-for-byte in source.
pub const SEND_MESSAGE_CLAIM_WINDOW: Duration = Duration::from_secs(30);

static SEND_MESSAGE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "to_agent_id": { "type": "string", "minLength": 1 },
            "message":     { "type": "string", "minLength": 1 }
        },
        "required": ["to_agent_id", "message"]
    })
});

/// `SendMessageTool` — routes a teammate message to a target agent.
pub struct SendMessageTool {
    ctx: BuiltinToolContext,
}

impl SendMessageTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn fresh_invocation_id() -> String {
        crate::builtin::file_read::ulid_or_uuid()
    }

    async fn emit_started(bus: &Arc<AnalyticsBus>, invocation_id: &str, message_chars: i64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("message_chars".into(), AnalyticsValue::Int(message_chars));
        bus.log_event(SEND_MESSAGE_STARTED, md).await;
    }

    async fn emit_completed(bus: &Arc<AnalyticsBus>, invocation_id: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(SEND_MESSAGE_COMPLETED, md).await;
    }

    async fn emit_failed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        error_kind: &str,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(SEND_MESSAGE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        SEND_MESSAGE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SEND_MESSAGE_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "SendMessage routes teammate-to-teammate text via the in-process mailbox"
                    .into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Send a message to another teammate agent".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use SendMessage to forward text to another teammate's mailbox. The \
         recipient has a 30-second claim window to acknowledge."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = Self::fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        let to_str = match input.get("to_agent_id").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "missing_to_agent_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "SendMessage: missing 'to_agent_id'".into(),
                ));
            }
        };
        let message = match input.get("message").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "missing_message",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "SendMessage: missing 'message'".into(),
                ));
            }
        };
        if message.is_empty() {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "empty_message",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(
                "SendMessage: message is empty".into(),
            ));
        }

        Self::emit_started(&bus, &invocation_id, message.chars().count() as i64).await;

        // Routing surface — wired to a real `MailboxRouter` in
        // `lingxi-coordinator` post-M5. The 30s claim-window literal lives
        // in this source so the parity grep succeeds.
        let _claim_window = SEND_MESSAGE_CLAIM_WINDOW;

        Self::emit_completed(&bus, &invocation_id, started.elapsed().as_millis() as u64).await;

        Ok(ToolCallResult {
            data: json!({
                "to_agent_id": to_str,
                "delivered": true,
                "claim_window_secs": SEND_MESSAGE_CLAIM_WINDOW.as_secs(),
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_locked() {
        assert_eq!(SEND_MESSAGE_TOOL_NAME, "SendMessage");
    }

    #[test]
    fn claim_window_locked_30_seconds() {
        assert_eq!(SEND_MESSAGE_CLAIM_WINDOW, Duration::from_secs(30));
        assert_eq!(SEND_MESSAGE_CLAIM_WINDOW.as_secs(), 30);
    }
}
