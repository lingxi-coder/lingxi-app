//! `tool-mobile` — the device-capability builtin tools.
//!
//! Folds the former one-crate-per-tool split (`tool-share` / `tool-camera` /
//! `tool-voice` / `tool-notification` / `tool-clipboard` / `tool-speech`) into a
//! single crate of submodules. Every tool here is pure Rust that routes to an
//! `Arc<dyn …Control>` capability carried in [`tool_api::BuiltinToolContext`].
//!
//! ## Two composition roots, two entry points
//!
//! - `engine-mobile` calls [`register_all`] — all six tools. Their capabilities
//!   are the Swift / Kotlin implementations injected via `UniFFI`.
//! - `engine-desktop` calls [`register_audio`] — ONLY `voice` + `speech`. Their
//!   capabilities are `bridge_server::audio_bridge::AudioBridge`, which proxies
//!   each trait call to the connected Electron client over the wire. The desktop
//!   has no counterpart for camera / share / notification / clipboard, so those
//!   four stay mobile-only.
//!
//! The crate NAME is therefore no longer accurate — `voice` and `speech` are not
//! mobile-exclusive any more. Deliberately NOT renamed: a rename churns every
//! call site and path for no behavioural gain, and this note is the cheaper fix.

#![forbid(unsafe_code)]

pub mod camera;
pub mod clipboard;
pub mod notification;
pub mod share;
pub mod speech;
pub mod voice;

pub use camera::CameraTool;
pub use clipboard::ClipboardTool;
pub use notification::NotificationTool;
pub use share::ShareTool;
pub use speech::SpeechTool;
pub use voice::VoiceTool;

/// Register every device-capability builtin tool into `reg`.
///
/// Replaces the former per-crate `tool_<name>::register_all` calls the
/// composition root made; the ordering matches the previous wiring.
///
/// Registration is UNCONDITIONAL here, unlike [`register_audio`]: on mobile the
/// six capabilities are one platform object's methods, a device that is missing
/// one still has the tool's *concept*, and the mobile tool surface is pinned by
/// its own snapshots. Changing that is a mobile decision, not a side effect of
/// giving the desktop two of these tools.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    camera::register_all(reg, ctx.clone());
    voice::register_all(reg, ctx.clone());
    speech::register_all(reg, ctx.clone());
    notification::register_all(reg, ctx.clone());
    clipboard::register_all(reg, ctx.clone());
    share::register_all(reg, ctx);
}

/// Register ONLY the two audio tools (`voice`, `speech`), and only where the
/// capability behind each is actually present in `ctx`.
///
/// This is the desktop composition root's entry point. It is narrower than
/// [`register_all`] in two deliberate ways:
///
/// 1. **Two tools, not six.** The desktop's audio comes from the Electron
///    client's microphone and speaker; it has no device camera, share sheet,
///    push-notification service or mobile clipboard behind the other four, and
///    registering them would widen the desktop tool surface with tools that
///    could only ever fail.
/// 2. **Gated on the capability.** A tool the model can call but that always
///    errors is worse than an absent tool: it burns a turn and teaches the model
///    something false about what this app can do. So a context with no audio
///    wiring registers nothing here, and the desktop tool list is byte-identical
///    to what it was before audio existed.
///
/// `stt` and `tts` are both routed by the single `speech` tool, so either one
/// alone is enough to register it — that tool reports "not available on this
/// platform" per action, and half a speech capability is still a real one.
pub fn register_audio(reg: &mut tool_api::ToolRegistry, ctx: &tool_api::BuiltinToolContext) {
    if ctx.voice.is_some() {
        voice::register_all(reg, ctx.clone());
    }
    if ctx.stt.is_some() || ctx.tts.is_some() {
        speech::register_all(reg, ctx.clone());
    }
}

#[cfg(test)]
mod register_audio_tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tool_api::{BuiltinToolContext, ToolRegistry};
    use platform_api::process::ProcessOutput;
    use platform_api::stt::{SpeechToText, SttError, SttOpts, SttTranscript};
    use platform_api::tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
    use platform_api::voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};

    /// One object implementing all three audio traits — the shape the desktop
    /// injects (`bridge_server::audio_bridge::AudioBridge`). Never called: these
    /// tests are about what gets REGISTERED, not what a capability does.
    struct StubAudio;

    #[async_trait]
    impl SpeechToText for StubAudio {
        async fn transcribe(&self, _opts: SttOpts) -> Result<SttTranscript, SttError> {
            Err(SttError::Unavailable)
        }
    }

    #[async_trait]
    impl TextToSpeech for StubAudio {
        async fn synthesize(&self, _opts: TtsOpts) -> Result<TtsAudio, TtsError> {
            Err(TtsError::Unavailable)
        }
    }

    #[async_trait]
    impl VoiceRecorder for StubAudio {
        async fn start_recording(&self, _opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
            Err(VoiceError::Busy)
        }
        async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
            Err(VoiceError::NotRecording)
        }
        async fn is_recording(&self) -> bool {
            false
        }
    }

    fn bare_ctx() -> BuiltinToolContext {
        tool_api::test_support::shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }

    fn audio_names(ctx: &BuiltinToolContext) -> Vec<String> {
        let mut reg = ToolRegistry::new();
        super::register_audio(&mut reg, ctx);
        reg.all_names()
    }

    #[test]
    fn register_audio_registers_nothing_without_a_capability() {
        assert!(
            audio_names(&bare_ctx()).is_empty(),
            "with no capability wired, a tool that could only ever fail must not \
             be advertised at all"
        );
    }

    #[test]
    fn register_audio_registers_both_tools_for_a_full_capability() {
        let audio = Arc::new(StubAudio);
        let ctx = BuiltinToolContext {
            voice: Some(audio.clone()),
            stt: Some(audio.clone()),
            tts: Some(audio),
            ..bare_ctx()
        };
        let names = audio_names(&ctx);
        assert_eq!(
            names,
            vec!["voice".to_string(), "speech".to_string()],
            "exactly the two audio tools, in the same order `register_all` uses"
        );
    }

    #[test]
    fn register_audio_registers_only_what_is_wired() {
        let recorder_only = BuiltinToolContext {
            voice: Some(Arc::new(StubAudio) as Arc<dyn VoiceRecorder>),
            ..bare_ctx()
        };
        assert_eq!(audio_names(&recorder_only), vec!["voice".to_string()]);

        let synthesizer_only = BuiltinToolContext {
            tts: Some(Arc::new(StubAudio) as Arc<dyn TextToSpeech>),
            ..bare_ctx()
        };
        assert_eq!(
            audio_names(&synthesizer_only),
            vec!["speech".to_string()],
            "the speech tool carries BOTH `transcribe` and `speak`; a synthesizer \
             alone is enough to make it worth having"
        );
    }

    /// The mobile entry point is deliberately NOT gated: changing that is a
    /// mobile decision, not a side effect of giving the desktop two of these
    /// tools. This pins the six-tool surface against a well-meaning edit that
    /// routes `register_all` through `register_audio`.
    #[test]
    fn register_all_still_registers_all_six_tools_ungated() {
        let mut reg = ToolRegistry::new();
        super::register_all(&mut reg, bare_ctx());
        assert_eq!(
            reg.all_names(),
            vec![
                "camera".to_string(),
                "voice".to_string(),
                "speech".to_string(),
                "notification".to_string(),
                "clipboard".to_string(),
                "share".to_string(),
            ],
            "mobile registers all six regardless of which capabilities the \
             platform actually supplied"
        );
    }
}
