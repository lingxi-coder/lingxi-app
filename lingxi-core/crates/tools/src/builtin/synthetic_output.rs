//! `SyntheticOutputTool` — echo-input stub for parity replay.
//!
//! Wire identifiers locked in spec §7:
//! - Input `{ content: String }` → output `{ content: String }` echoed verbatim.
//! - Maximum content length = `MAX_TOOL_OUTPUT_LENGTH` (30 000 chars);
//!   excess truncated with the M4-01 truncation suffix.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{
    SYNTHETIC_OUTPUT_COMPLETED, SYNTHETIC_OUTPUT_FAILED, SYNTHETIC_OUTPUT_STARTED,
};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::shared::MAX_TOOL_OUTPUT_LENGTH;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const SYNTHETIC_OUTPUT_TOOL_NAME: &str = "SyntheticOutput";
/// Truncation suffix appended when input exceeds `MAX_TOOL_OUTPUT_LENGTH`.
/// Matches M4-01 truncation pattern (see `crates/tools/src/shared.rs`).
pub const SYNTHETIC_TRUNCATION_SUFFIX: &str = "\n... [truncated]";

/// `SyntheticOutputTool` — echo-input stub for parity replay.
pub struct SyntheticOutputTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl SyntheticOutputTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "content": { "type": "string" }
        },
        "required": ["content"]
    })
});

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(SYNTHETIC_OUTPUT_FAILED, md).await;
}

/// Truncate `s` at char boundary so the result + suffix fits in
/// `MAX_TOOL_OUTPUT_LENGTH`. Returns `(truncated_string, was_truncated)`.
fn maybe_truncate(s: &str) -> (String, bool) {
    if s.chars().count() <= MAX_TOOL_OUTPUT_LENGTH {
        return (s.to_string(), false);
    }
    let keep = MAX_TOOL_OUTPUT_LENGTH.saturating_sub(SYNTHETIC_TRUNCATION_SUFFIX.chars().count());
    let prefix: String = s.chars().take(keep).collect();
    (format!("{prefix}{SYNTHETIC_TRUNCATION_SUFFIX}"), true)
}

#[async_trait]
impl Tool for SyntheticOutputTool {
    fn name(&self) -> &str {
        SYNTHETIC_OUTPUT_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
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
                reason: "SyntheticOutput echoes input (replay-only)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Echo-input stub for parity replay. Output capped at MAX_TOOL_OUTPUT_LENGTH.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "SyntheticOutput: echoes its input (used by parity replay).".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ValidationError("SyntheticOutput: missing or non-string content".into())
            })?;
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();
        let content = match input.get("content").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    "missing_content",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "SyntheticOutput: missing or non-string content".into(),
                ));
            }
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "content_len".into(),
            AnalyticsValue::Int(content.chars().count() as i64),
        );
        md.insert(
            "_PROTO_content_preview".into(),
            pii_str(&content[..content.len().min(80)]),
        );
        bus.log_event(SYNTHETIC_OUTPUT_STARTED, md).await;

        let (out_content, truncated) = maybe_truncate(&content);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("truncated".into(), AnalyticsValue::Bool(truncated));
        md.insert(
            "output_len".into(),
            AnalyticsValue::Int(out_content.chars().count() as i64),
        );
        bus.log_event(SYNTHETIC_OUTPUT_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "content": out_content,
                "truncated": truncated,
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
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use lingxi_traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SYNTHETIC_OUTPUT_TOOL_NAME, "SyntheticOutput");
    }

    #[tokio::test]
    async fn echoes_short_input_verbatim() {
        let tool = SyntheticOutputTool::new(shell_test_ctx(dummy_out()));
        let out = tool
            .call(json!({"content": "hello world"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["content"], json!("hello world"));
        assert_eq!(out.data["truncated"], json!(false));
    }

    #[tokio::test]
    async fn truncates_oversized_input() {
        let tool = SyntheticOutputTool::new(shell_test_ctx(dummy_out()));
        let big = "x".repeat(MAX_TOOL_OUTPUT_LENGTH + 100);
        let out = tool
            .call(json!({"content": big}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        let content = out.data["content"].as_str().unwrap();
        assert!(content.ends_with(SYNTHETIC_TRUNCATION_SUFFIX));
        assert!(content.chars().count() <= MAX_TOOL_OUTPUT_LENGTH);
        assert_eq!(out.data["truncated"], json!(true));
    }

    #[tokio::test]
    async fn rejects_missing_content() {
        let tool = SyntheticOutputTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing");
        assert!(format!("{err}").contains("missing or non-string content"));
    }
}
