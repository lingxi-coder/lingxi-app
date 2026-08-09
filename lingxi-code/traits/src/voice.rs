//! `VoiceRecorder` — microphone capture seam (M8-P10).
//!
//! Implemented natively in Swift/Kotlin via `UniFFI` (P12) and injected into the
//! mobile `Platform`. The `tool-voice` tool (P11) routes through it.

use async_trait::async_trait;
use thiserror::Error;

/// Options for a recording session.
#[derive(Debug, Clone)]
pub struct VoiceRecordingOpts {
    /// Target sample rate in Hz (e.g. `16_000` for speech).
    pub sample_rate_hz: u32,
    /// Container/codec hint (e.g. `"m4a"`, `"wav"`).
    pub format: String,
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct VoiceRecording {
    /// Encoded audio bytes.
    pub audio_bytes: Vec<u8>,
    /// MIME type of `audio_bytes` (e.g. `"audio/m4a"`).
    pub mime_type: String,
}

/// Failure modes for [`VoiceRecorder`] operations.
#[derive(Debug, Clone, Error)]
pub enum VoiceError {
    /// The user denied microphone permission.
    #[error("microphone permission denied")]
    PermissionDenied,
    /// `stop_recording` was called with no active session.
    #[error("not currently recording")]
    NotRecording,
    /// The platform audio session is held by another consumer (e.g. the
    /// FlowMode voice orb) — retry after it releases.
    #[error("audio session busy")]
    Busy,
    /// Any other native failure.
    #[error("voice error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_display_names_the_audio_session() {
        assert_eq!(VoiceError::Busy.to_string(), "audio session busy");
    }
}

/// Native microphone recording.
#[async_trait]
pub trait VoiceRecorder: Send + Sync {
    /// Begin a recording session.
    async fn start_recording(&self, opts: VoiceRecordingOpts) -> Result<(), VoiceError>;
    /// Stop the active session and return the captured audio.
    async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError>;
    /// Whether a recording session is currently active.
    async fn is_recording(&self) -> bool;
}
