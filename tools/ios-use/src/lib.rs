//! `tool-ios-use` (M8-P11b) — the `ios_use` tool.
//!
//! On-device iOS UI automation. The M8 skeleton routes to the shared
//! `ctx.computer_control` (`Arc<dyn ComputerControl>`) seam — an iOS
//! XCUITest/accessibility backend injected via `UniFFI` (P12) supplies the impl.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use lingxi_core::host::computer_control::ComputerError;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "ios_use";

/// `IosUseTool` — tap / type / screenshot on an iOS device.
#[derive(Clone)]
pub struct IosUseTool {
    ctx: BuiltinToolContext,
}

impl IosUseTool {
    /// Construct from the builtin tool context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["screenshot", "tap", "type", "key"] },
            "x": { "type": "integer", "minimum": 0 },
            "y": { "type": "integer", "minimum": 0 },
            "text": { "type": "string" }
        },
        "required": ["action"]
    })
});

fn map_err(e: &ComputerError) -> ToolError {
    match e {
        ComputerError::PermissionDenied(m) => ToolError::PermissionDenied(m.clone()),
        other => ToolError::Internal(other.to_string()),
    }
}

#[async_trait]
impl Tool for IosUseTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4096
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, input: &Value) -> bool {
        matches!(
            input.get("action").and_then(Value::as_str),
            Some("screenshot")
        )
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ios_use — iOS automation entitlement gates use".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("screenshot");
        format!("iOS action: {action}")
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Drive the iOS UI: screenshot, tap, type, key.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(Value::as_str) {
            Some("screenshot" | "tap" | "type" | "key") => Ok(()),
            _ => Err(ValidationError(
                "`action` must be screenshot|tap|type|key".into(),
            )),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let cc = self
            .ctx
            .computer_control
            .as_ref()
            .ok_or_else(|| ToolError::Internal("ios automation not available".into()))?;

        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("screenshot");
        let data = match action {
            "screenshot" => {
                let s = cc.screenshot().await.map_err(|e| map_err(&e))?;
                json!({ "width": s.width, "height": s.height, "png_bytes_len": s.png_bytes.len() })
            }
            "tap" => {
                let x = input.get("x").and_then(Value::as_u64);
                let y = input.get("y").and_then(Value::as_u64);
                #[allow(clippy::cast_possible_truncation)] // coordinate space never exceeds u32
                let (x, y) = match (x, y) {
                    (Some(x), Some(y)) => (x as u32, y as u32),
                    _ => return Err(ToolError::InvalidInput("`tap` requires `x` and `y`".into())),
                };
                cc.left_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "type" => {
                let text = input
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("`type` requires `text`".into()))?;
                cc.type_text(text.to_string())
                    .await
                    .map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            _ => {
                let key = input
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("`key` requires `text`".into()))?;
                cc.key(key.to_string()).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
        };

        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

/// Register the `ios_use` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(IosUseTool::new(ctx)));
}
