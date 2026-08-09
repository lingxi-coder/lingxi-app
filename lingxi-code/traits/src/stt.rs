//! `SpeechToText` — speech recognition seam (mobile device capability).
//!
//! Implemented natively in Swift/Kotlin via `UniFFI` and injected into the
//! mobile `Platform`. The `tool-speech` tool routes through it so Rust can
//! request a transcription without knowing the native ASR API. On Android the
//! impl wraps the system `SpeechRecognizer`; on iOS, `SFSpeechRecognizer`.
//!
//! Unlike a file-based STT, the system recognizer opens the live microphone
//! itself, so [`SpeechToText::transcribe`] takes only options (no audio bytes):
//! it opens the mic, listens for an utterance, and returns the final transcript.

use async_trait::async_trait;
use thiserror::Error;

/// Options for a one-shot transcription.
#[derive(Debug, Clone, Default)]
pub struct SttOpts {
    /// BCP-47 language hint (e.g. `"en-US"`, `"zh-CN"`). `None` = device default.
    pub language: Option<String>,
}

/// A finished transcription.
#[derive(Debug, Clone)]
pub struct SttTranscript {
    /// Recognized text (empty when nothing was heard).
    pub text: String,
    /// BCP-47 language actually detected, when the provider reports it.
    pub language: Option<String>,
    /// Confidence in `[0, 1]`, when the provider reports it.
    pub confidence: Option<f32>,
}

/// Failure modes for [`SpeechToText`] operations.
#[derive(Debug, Clone, Error)]
pub enum SttError {
    /// The user denied microphone permission.
    #[error("microphone permission denied")]
    PermissionDenied,
    /// No speech was detected before the listen timeout.
    #[error("no speech detected")]
    NoSpeech,
    /// The device has no speech-recognition service installed/available.
    #[error("speech recognition unavailable")]
    Unavailable,
    /// The platform audio session is held by another consumer (a local
    /// app's recording, FlowMode's voice orb) — the same contention
    /// [`crate::VoiceError::Busy`] reports, and a caller branching on one
    /// should not have to also recognize the other.
    #[error("audio session busy")]
    Busy,
    /// A transient failure (network ASR, recognizer busy) — safe to retry.
    #[error("transient speech error: {0}")]
    Retriable(String),
    /// Any other native failure.
    #[error("speech error: {0}")]
    Other(String),
}

/// Native speech-to-text (live microphone → transcript).
#[async_trait]
pub trait SpeechToText: Send + Sync {
    /// Open the microphone, listen for a single utterance, and return the
    /// final transcript. Implementations MUST honor cancellation.
    async fn transcribe(&self, opts: SttOpts) -> Result<SttTranscript, SttError>;
}
