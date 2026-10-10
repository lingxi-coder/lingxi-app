//! `tool-ios-use` (M8-P11b) — the `ios_use` tool.
//!
//! On-device iOS UI automation. The M8 skeleton routes to the shared
//! `ctx.computer_control` (`Arc<dyn ComputerControl>`) seam — an iOS
//! XCUITest/accessibility backend injected via `UniFFI` (P12) supplies the impl.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use lingxi_core::host::computer_control::ComputerError;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::json;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolInputRepairMetadata,
    ToolInputRepairSchema, ToolInputSchemaProvenance, ToolStaticContext, ValidationError,
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

static INPUT_SCHEMA: Lazy<Utf16JsonProjection> = Lazy::new(|| {
    Utf16JsonProjection::from_serde_value(&json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["screenshot", "tap", "type", "key"] },
            "x": { "type": "integer", "minimum": 0 },
            "y": { "type": "integer", "minimum": 0 },
            "text": { "type": "string" }
        },
        "required": ["action"]
    }))
    .expect("iOS tool input schema is valid JSON")
});

fn unsigned_integer_at(input: &Utf16JsonProjection, pointer: &str) -> Option<u64> {
    let number = input.number_at(pointer)?;
    if !number.is_finite()
        || number.fract() != 0.0
        || !(0.0..18_446_744_073_709_551_616.0).contains(&number)
    {
        return None;
    }
    // The exclusive upper bound above guarantees this conversion does not saturate.
    #[allow(clippy::cast_possible_truncation)]
    Some(number as u64)
}

fn map_err(e: &ComputerError) -> ToolError {
    match e {
        ComputerError::PermissionDenied(m) => ToolError::PermissionDenied(m.clone().into()),
        other => ToolError::Internal(other.to_string().into()),
    }
}

#[async_trait]
impl Tool for IosUseTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Utf16JsonProjection {
        &INPUT_SCHEMA
    }
    fn input_repair_metadata(&self) -> ToolInputRepairMetadata {
        ToolInputRepairMetadata::Known {
            schema: ToolInputRepairSchema::Advertised,
            schema_provenance: ToolInputSchemaProvenance::GeneratedBuiltin,
            exempt_top_level_keys: None,
        }
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4096
    }
    fn is_concurrency_safe(&self, _: &Utf16JsonProjection) -> bool {
        false
    }
    fn is_read_only(&self, input: &Utf16JsonProjection) -> bool {
        input.string_at("/action") == Some("screenshot")
    }

    async fn check_permissions(
        &self,
        _: &Utf16JsonProjection,
        _: &ToolUseContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ios_use — iOS automation entitlement gates use".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Utf16JsonProjection, _: &DescriptionOptions) -> String {
        let action = input.string_at("/action").unwrap_or("screenshot");
        format!("iOS action: {action}")
    }

    async fn prompt(&self, _: &PromptOptions) -> Utf16JsonProjection {
        "Drive the iOS UI: screenshot, tap, type, key.".into()
    }

    async fn validate_input(
        &self,
        input: &Utf16JsonProjection,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.string_at("/action") {
            Some("screenshot" | "tap" | "type" | "key") => Ok(()),
            _ => Err(ValidationError(
                "`action` must be screenshot|tap|type|key".into(),
            )),
        }
    }

    async fn call(
        &self,
        input: Utf16JsonProjection,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let cc = self
            .ctx
            .computer_control
            .as_ref()
            .ok_or_else(|| ToolError::Internal("ios automation not available".into()))?;

        let action = input.string_at("/action").unwrap_or("screenshot");
        let data = match action {
            "screenshot" => {
                let s = cc.screenshot().await.map_err(|e| map_err(&e))?;
                json!({ "width": s.width, "height": s.height, "png_bytes_len": s.png_bytes.len() })
            }
            "tap" => {
                let x = unsigned_integer_at(&input, "/x");
                let y = unsigned_integer_at(&input, "/y");
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
                    .string_at("/text")
                    .ok_or_else(|| ToolError::InvalidInput("`type` requires `text`".into()))?;
                cc.type_text(text.to_string())
                    .await
                    .map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
            _ => {
                let key = input
                    .string_at("/text")
                    .ok_or_else(|| ToolError::InvalidInput("`key` requires `text`".into()))?;
                cc.key(key.to_string()).await.map_err(|e| map_err(&e))?;
                json!({ "ok": true })
            }
        };

        let data =
            Utf16JsonProjection::from_serde_value(&data).expect("iOS tool result is valid JSON");
        Ok(ToolCallResult::from_data(data))
    }
}

/// Register the `ios_use` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(IosUseTool::new(ctx)));
}
