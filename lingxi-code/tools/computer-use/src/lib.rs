//! `tool-computer-use` (M8-P11b) — the `computer` tool.
//!
//! Screen capture + mouse/keyboard automation, routed to `ctx.computer_control`
//! (`Arc<dyn ComputerControl>`). `None` unless a backend is wired — a desktop
//! automation impl, or a mobile UniFFI impl. Pure-Rust dispatch.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use traits::computer_control::ComputerError;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "computer";

/// `ComputerTool` — screenshot + mouse/keyboard automation.
#[derive(Clone)]
pub struct ComputerTool {
    ctx: BuiltinToolContext,
}

impl ComputerTool {
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
            "action": {
                "type": "string",
                "enum": [
                    "screenshot", "display_size", "mouse_move", "left_click",
                    "right_click", "double_click", "type", "key", "scroll"
                ]
            },
            "x": { "type": "integer", "minimum": 0 },
            "y": { "type": "integer", "minimum": 0 },
            "dx": { "type": "integer" },
            "dy": { "type": "integer" },
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

fn xy(input: &Value) -> Result<(u32, u32), ToolError> {
    let x = input.get("x").and_then(Value::as_u64);
    let y = input.get("y").and_then(Value::as_u64);
    match (x, y) {
        (Some(x), Some(y)) => Ok((x as u32, y as u32)),
        _ => Err(ToolError::InvalidInput(
            "this action requires `x` and `y`".into(),
        )),
    }
}

#[async_trait]
impl Tool for ComputerTool {
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
            Some("screenshot" | "display_size")
        )
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "computer tool — OS screen-recording/accessibility prompt gates use".into(),
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
        format!("Computer action: {action}")
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Capture the screen and drive the mouse/keyboard.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(Value::as_str) {
            Some(_) => Ok(()),
            None => Err(ValidationError("`action` is required".into())),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let cc = self.ctx.computer_control.as_ref().ok_or_else(|| {
            ToolError::Internal("computer-control not available on this platform".into())
        })?;

        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("screenshot");
        let data = match action {
            "screenshot" => {
                let s = cc.screenshot().await.map_err(|e| map_err(&e))?;
                json!({ "width": s.width, "height": s.height, "png_bytes_len": s.png_bytes.len() })
            }
            "display_size" => {
                let (w, h) = cc.display_size().await.map_err(|e| map_err(&e))?;
                json!({ "width": w, "height": h })
            }
            "mouse_move" => {
                let (x, y) = xy(&input)?;
                cc.mouse_move(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "left_click" => {
                let (x, y) = xy(&input)?;
                cc.left_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "right_click" => {
                let (x, y) = xy(&input)?;
                cc.right_click(x, y).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "double_click" => {
                let (x, y) = xy(&input)?;
                cc.double_click(x, y).await.map_err(|e| map_err(&e))?;
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
            "key" => {
                let key = input.get("text").and_then(Value::as_str).ok_or_else(|| {
                    ToolError::InvalidInput("`key` requires `text` (the key name)".into())
                })?;
                cc.key(key.to_string()).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            "scroll" => {
                let (x, y) = xy(&input)?;
                let dx = input.get("dx").and_then(Value::as_i64).unwrap_or(0) as i32;
                let dy = input.get("dy").and_then(Value::as_i64).unwrap_or(0) as i32;
                cc.scroll(x, y, dx, dy).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            other => return Err(ToolError::InvalidInput(format!("unknown action: {other}"))),
        };

        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// Register the `computer` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(ComputerTool::new(ctx)));
}
