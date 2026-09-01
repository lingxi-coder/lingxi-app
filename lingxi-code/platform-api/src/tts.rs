//! `TextToSpeech` — speech synthesis seam (mobile device capability).
//!
//! Implemented natively in Swift/Kotlin via `UniFFI` and injected into the
//! mobile `Platform`. The `tool-speech` tool routes through it so Rust can
//! request synthesized audio without knowing the native TTS API. On Android the
//! impl wraps the system `TextToSpeech` engine; on iOS, `AVSpeechSynthesizer`.

use async_trait::async_trait;
use thiserror::Error;

/// Options for a synthesis request.
#[derive(Debug, Clone)]
pub struct TtsOpts {
    /// Text to speak.
    pub text: String,
    /// Provider-specific voice id (`None` = the system default voice).
    pub voice: Option<String>,
}

/// Synthesized audio: 16-bit signed little-endian PCM, mono.
#[derive(Debug, Clone)]
pub struct TtsAudio {
    /// Raw PCM16 frames.
    pub pcm: Vec<u8>,
    /// Sample rate of `pcm` in Hz (e.g. `22_050`, `24_000`).
    pub sample_rate_hz: u32,
}

/// Failure modes for [`TextToSpeech`] operations.
#[derive(Debug, Clone, Error)]
pub enum TtsError {
    /// The device has no usable TTS engine.
    #[error("text-to-speech unavailable")]
    Unavailable,
    /// Synthesis failed for the given text/voice.
    #[error("synthesis failed: {0}")]
    SynthesisFailed(String),
    /// Any other native failure.
    #[error("text-to-speech error: {0}")]
    Other(String),
}

/// Native text-to-speech (text → PCM16 audio).
#[async_trait]
pub trait TextToSpeech: Send + Sync {
    /// Synthesize `opts.text` to PCM16 audio.
    async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError>;
}
