//! `tool-speech` — the mobile-exclusive `speech` tool (ASR + TTS).
//!
//! Routes to `ctx.stt` (`Arc<dyn SpeechToText>`) and `ctx.tts`
//! (`Arc<dyn TextToSpeech>`). `None` on desktop; mobile composition roots wire a
//! native Swift / Kotlin impl via `UniFFI`. Sibling of `tool-voice` (raw mic
//! capture) — this is recognition (`transcribe`) and synthesis (`speak`).


use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use traits::stt::{SttError, SttOpts};
use traits::tts::{TtsError, TtsOpts};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "speech";

/// `SpeechTool` — transcribe speech (ASR) or synthesize speech (TTS).
#[derive(Clone)]
pub struct SpeechTool {
    ctx: BuiltinToolContext,
}

impl SpeechTool {
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
            "action": { "type": "string", "enum": ["transcribe", "speak"] },
            "text": { "type": "string", "description": "Text to speak (action=speak)." },
            "voice": { "type": "string", "description": "Voice id (action=speak, optional)." },
            "language": { "type": "string", "description": "BCP-47 hint (action=transcribe, optional)." }
        },
        "required": ["action"]
    })
});

fn map_stt_err(e: &SttError) -> ToolError {
    match e {
        SttError::PermissionDenied => {
            ToolError::PermissionDenied("microphone permission denied".into())
        }
        other => ToolError::Internal(other.to_string()),
    }
}

fn map_tts_err(e: &TtsError) -> ToolError {
    ToolError::Internal(e.to_string())
}

#[async_trait]
impl Tool for SpeechTool {
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
        // `transcribe` only reads the mic; `speak` produces audio (no side effect
        // on the workspace) — both are effectively read-only w.r.t. files.
        let _ = input;
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "speech tool — native OS microphone prompt gates recognition".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        match input.get("action").and_then(Value::as_str) {
            Some("speak") => "Synthesizing speech".into(),
            _ => "Transcribing speech".into(),
        }
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Transcribe spoken audio from the microphone, or synthesize speech from text."
            .into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(Value::as_str) {
            Some("transcribe") => Ok(()),
            Some("speak") => {
                if input
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !t.trim().is_empty())
                {
                    Ok(())
                } else {
                    Err(ValidationError("`speak` requires a non-empty `text`".into()))
                }
            }
            _ => Err(ValidationError(
                "`action` must be `transcribe` or `speak`".into(),
            )),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("transcribe");

        let data = if action == "speak" {
            let tts = self.ctx.tts.as_ref().ok_or_else(|| {
                ToolError::Internal("text-to-speech not available on this platform".into())
            })?;
            let text = input
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let voice = input
                .get("voice")
                .and_then(Value::as_str)
                .map(str::to_string);
            let audio = tts
                .synthesize(TtsOpts { text, voice })
                .await
                .map_err(|e| map_tts_err(&e))?;
            json!({
                "spoken": true,
                "sample_rate_hz": audio.sample_rate_hz,
                "pcm_bytes_len": audio.pcm.len(),
            })
        } else {
            let stt = self.ctx.stt.as_ref().ok_or_else(|| {
                ToolError::Internal("speech recognition not available on this platform".into())
            })?;
            let language = input
                .get("language")
                .and_then(Value::as_str)
                .map(str::to_string);
            let t = stt
                .transcribe(SttOpts { language })
                .await
                .map_err(|e| map_stt_err(&e))?;
            json!({
                "text": t.text,
                "language": t.language,
                "confidence": t.confidence,
            })
        };

        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// Register the `speech` tool against `reg`.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    reg.register_builtin(Arc::new(SpeechTool::new(ctx)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use traits::stt::{SpeechToText, SttTranscript};
    use traits::tts::{TextToSpeech, TtsAudio};

    struct FakeStt;
    #[async_trait]
    impl SpeechToText for FakeStt {
        async fn transcribe(&self, _opts: SttOpts) -> Result<SttTranscript, SttError> {
            Ok(SttTranscript {
                text: "hello world".into(),
                language: Some("en-US".into()),
                confidence: Some(0.9),
            })
        }
    }

    struct FakeTts;
    #[async_trait]
    impl TextToSpeech for FakeTts {
        async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError> {
            assert!(!opts.text.is_empty());
            Ok(TtsAudio {
                pcm: vec![0u8; 320],
                sample_rate_hz: 22_050,
            })
        }
    }

    fn empty_output() -> traits::process::ProcessOutput {
        traits::process::ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    fn ctx_with(
        stt: Option<Arc<dyn SpeechToText>>,
        tts: Option<Arc<dyn TextToSpeech>>,
    ) -> BuiltinToolContext {
        let mut ctx = tool_api::test_support::shell_test_ctx(empty_output());
        ctx.stt = stt;
        ctx.tts = tts;
        ctx
    }

    #[tokio::test]
    async fn transcribe_routes_to_stt() {
        let tool = SpeechTool::new(ctx_with(Some(Arc::new(FakeStt)), None));
        let res = tool
            .call(
                json!({ "action": "transcribe" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["text"], "hello world");
        assert_eq!(res.data["language"], "en-US");
    }

    #[tokio::test]
    async fn speak_routes_to_tts() {
        let tool = SpeechTool::new(ctx_with(None, Some(Arc::new(FakeTts))));
        let res = tool
            .call(
                json!({ "action": "speak", "text": "hi" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["spoken"], true);
        assert_eq!(res.data["sample_rate_hz"], 22_050);
        assert_eq!(res.data["pcm_bytes_len"], 320);
    }

    #[tokio::test]
    async fn missing_backend_errors() {
        let tool = SpeechTool::new(ctx_with(None, None));
        let err = tool
            .call(
                json!({ "action": "transcribe" }),
                tool_api::test_support::fresh_ctx(),
                tool_api::test_support::fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Internal(_)));
    }

    #[tokio::test]
    async fn speak_requires_text() {
        let tool = SpeechTool::new(ctx_with(None, Some(Arc::new(FakeTts))));
        let err = tool
            .validate_input(
                &json!({ "action": "speak" }),
                &tool_api::test_support::fresh_ctx(),
            )
            .await
            .unwrap_err();
        assert!(err.0.contains("text"));
    }
}
