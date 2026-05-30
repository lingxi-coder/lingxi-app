//! `tool-voice` (M8-P11) — the mobile-exclusive `voice` tool.
//!
//! Routes to `ctx.voice` (`Arc<dyn VoiceRecorder>`). `None` on desktop; mobile
//! composition roots wire a native Swift / Kotlin impl via UniFFI (P12).

#![forbid(unsafe_code)]

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use traits::voice::{VoiceError, VoiceRecordingOpts};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "voice";

/// `VoiceTool` — start/stop a microphone recording.
#[derive(Clone)]
pub struct VoiceTool {
    ctx: BuiltinToolContext,
}

impl VoiceTool {
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
            "action": { "type": "string", "enum": ["start_recording", "stop_recording", "is_recording"] },
            "sample_rate_hz": { "type": "integer", "minimum": 8000, "maximum": 48000 },
            "format": { "type": "string" }
        },
        "required": ["action"]
    })
});

fn map_voice_err(e: &VoiceError) -> ToolError {
    match e {
        VoiceError::PermissionDenied => {
            ToolError::PermissionDenied("microphone permission denied".into())
        }
        other => ToolError::Internal(other.to_string()),
    }
}

#[async_trait]
impl Tool for VoiceTool {
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
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "voice tool — native OS microphone prompt gates recording".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        match input.get("action").and_then(Value::as_str) {
            Some("stop_recording") => "Stopping the voice recording".into(),
            Some("is_recording") => "Checking recording state".into(),
            _ => "Starting a voice recording".into(),
        }
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Record audio from the device microphone.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(Value::as_str) {
            Some("start_recording" | "stop_recording" | "is_recording") => Ok(()),
            _ => Err(ValidationError(
                "`action` must be `start_recording`, `stop_recording`, or `is_recording`".into(),
            )),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let voice = self.ctx.voice.as_ref().ok_or_else(|| {
            ToolError::Internal("microphone not available on this platform".into())
        })?;

        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("start_recording");
        let data = match action {
            "stop_recording" => {
                let rec = voice
                    .stop_recording()
                    .await
                    .map_err(|e| map_voice_err(&e))?;
                json!({ "recording": false, "audio_bytes_len": rec.audio_bytes.len(), "mime_type": rec.mime_type })
            }
            "is_recording" => json!({ "recording": voice.is_recording().await }),
            _ => {
                let sample_rate_hz = input
                    .get("sample_rate_hz")
                    .and_then(Value::as_u64)
                    .unwrap_or(16_000) as u32;
                let format = input
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("m4a")
                    .to_string();
                voice
                    .start_recording(VoiceRecordingOpts {
                        sample_rate_hz,
                        format,
                    })
                    .await
                    .map_err(|e| map_voice_err(&e))?;
                json!({ "recording": true })
            }
        };

        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// Register the `voice` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(VoiceTool::new(ctx)));
}
