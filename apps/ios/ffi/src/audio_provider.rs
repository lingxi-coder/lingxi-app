//! Native UniFFI names backed by the canonical shared mobile audio lifecycle.
use crate::callbacks::{IosSecureStorage, IosSecureStorageBridge};
use harness_runtime::mobile::{MobileEngineError, MobileEngineHandle};
use std::sync::Arc;
use IosRealtimeAudioListener as PlatformRealtimeAudioListener;
use IosSecureStorage as PlatformSecureStorage;
use IosSecureStorageBridge as PlatformSecureStorageBridge;

#[path = "../../../../packages/mobile-ffi/src/audio_provider.rs"]
mod common;

/// App-scoped SDK audio host.
#[derive(uniffi::Object)]
pub struct IosAudioProviderHost {
    inner: common::AudioProviderCore,
}

#[uniffi::export]
pub fn build_ios_audio_provider_host(
    profiles_json: String,
    region: String,
    secure_storage: Box<dyn IosSecureStorage>,
) -> Result<Arc<IosAudioProviderHost>, MobileEngineError> {
    Ok(Arc::new(IosAudioProviderHost {
        inner: common::AudioProviderCore::new(profiles_json, region, secure_storage)?,
    }))
}

/// Reuse the current engine's service registry for session-bound audio.
#[uniffi::export]
pub fn build_ios_session_audio_provider_host(
    engine: Arc<MobileEngineHandle>,
) -> Arc<IosAudioProviderHost> {
    Arc::new(IosAudioProviderHost {
        inner: common::AudioProviderCore::for_engine(engine),
    })
}

#[uniffi::export(async_runtime = "tokio")]
impl IosAudioProviderHost {
    pub async fn capabilities(&self, request_json: String) -> String {
        self.inner.capabilities(request_json).await
    }
    pub async fn transcribe(
        &self,
        request_json: String,
        audio: Vec<u8>,
        mime_type: String,
    ) -> String {
        self.inner.transcribe(request_json, audio, mime_type).await
    }
    pub async fn synthesize(&self, request_json: String, text: String) -> String {
        self.inner.synthesize(request_json, text).await
    }
    pub async fn cancel(&self, operation_id: String) {
        self.inner.cancel(operation_id).await
    }
}

/// Canonical native realtime events. The callback must return promptly.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait IosRealtimeAudioListener: Send + Sync {
    async fn on_event(&self, event_json: String);
}

/// Current-engine Agent audio session; retains the engine for its entire lifetime.
#[derive(uniffi::Object)]
pub struct IosRealtimeAudioSession {
    inner: common::RealtimeAudioCore,
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn start_ios_realtime_audio(
    engine: Arc<MobileEngineHandle>,
    provider_host: Arc<IosAudioProviderHost>,
    request_json: String,
    listener: Box<dyn IosRealtimeAudioListener>,
) -> Result<Arc<IosRealtimeAudioSession>, MobileEngineError> {
    let inner =
        common::RealtimeAudioCore::start(engine, &provider_host.inner, request_json, listener)
            .await?;
    Ok(Arc::new(IosRealtimeAudioSession { inner }))
}

#[uniffi::export(async_runtime = "tokio")]
impl IosRealtimeAudioSession {
    pub async fn send_audio(&self, pcm: Vec<u8>) -> Result<(), MobileEngineError> {
        self.inner.send_audio(pcm).await
    }
    pub async fn commit_input(&self) -> Result<(), MobileEngineError> {
        self.inner.commit_input().await
    }
    pub async fn interrupt(
        &self,
        item_id: Option<String>,
        audio_end_ms: Option<u32>,
    ) -> Result<(), MobileEngineError> {
        self.inner.interrupt(item_id, audio_end_ms).await
    }
    pub async fn playback_completed(
        &self,
        item_id: Option<String>,
    ) -> Result<(), MobileEngineError> {
        self.inner.playback_completed(item_id).await
    }
    pub async fn close(&self) -> Result<(), MobileEngineError> {
        self.inner.close().await
    }
    pub async fn abort(&self) {
        self.inner.abort().await;
    }
}
