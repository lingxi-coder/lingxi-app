use crate::callbacks::{IosSecureStorage, IosSecureStorageBridge};
use harness_runtime::mobile::MobileEngineError;
use std::{collections::BTreeMap, sync::Arc};

/// App-scoped SDK audio host; constructing it never creates an Agent engine.
#[derive(uniffi::Object)]
pub struct IosAudioProviderHost {
    pub(super) inner: audio_provider::AudioProviderHost,
}

#[uniffi::export]
pub fn build_ios_audio_provider_host(
    profiles_json: String,
    region: String,
    secure_storage: Box<dyn IosSecureStorage>,
) -> Result<Arc<IosAudioProviderHost>, MobileEngineError> {
    let storage = Arc::new(IosSecureStorageBridge {
        inner: secure_storage,
    });
    let inner = audio_provider::AudioProviderHost::new(
        &profiles_json,
        &region,
        Some(storage),
        BTreeMap::new(),
    )
    .map_err(MobileEngineError::Internal)?;
    Ok(Arc::new(IosAudioProviderHost { inner }))
}

/// Reuse the exact current engine's service registry for session-bound audio.
#[uniffi::export]
pub fn build_ios_session_audio_provider_host(
    engine: Arc<harness_runtime::mobile::MobileEngineHandle>,
) -> Arc<IosAudioProviderHost> {
    let (services, credentials_ids, _credentials) = engine.audio_provider_services();
    Arc::new(IosAudioProviderHost {
        inner: audio_provider::AudioProviderHost::from_provider_services(services, credentials_ids),
    })
}

#[uniffi::export(async_runtime = "tokio")]
impl IosAudioProviderHost {
    pub async fn capabilities(&self, request_json: String) -> String {
        self.inner.capabilities(&request_json).await
    }
    pub async fn transcribe(
        &self,
        request_json: String,
        audio: Vec<u8>,
        mime_type: String,
    ) -> String {
        self.inner
            .transcribe(&request_json, audio, &mime_type)
            .await
    }
    pub async fn synthesize(&self, request_json: String, text: String) -> String {
        self.inner.synthesize(&request_json, &text).await
    }
    pub async fn cancel(&self, operation_id: String) {
        self.inner.cancel(&operation_id);
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
    inner: Arc<audio_provider::realtime::NativeRealtimeSession>,
    _engine: Arc<harness_runtime::mobile::MobileEngineHandle>,
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn start_ios_realtime_audio(
    engine: Arc<harness_runtime::mobile::MobileEngineHandle>,
    provider_host: Arc<IosAudioProviderHost>,
    request_json: String,
    listener: Box<dyn IosRealtimeAudioListener>,
) -> Result<Arc<IosRealtimeAudioSession>, MobileEngineError> {
    let runner = engine.clone();
    let (session, mut events) = audio_provider::realtime::NativeRealtimeSession::start_with_runner(
        engine.audio_orchestrator(),
        &provider_host.inner,
        &request_json,
        move |run| async move {
            runner
                .run_realtime_agent_owned(
                    run.prepared,
                    run.control,
                    run.events,
                    run.inputs,
                    run.output,
                    run.limits,
                    run.cancel,
                )
                .await
        },
    )
    .await
    .map_err(|error| MobileEngineError::Internal(error.to_string()))?;
    let weak = Arc::downgrade(&session);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if tokio::time::timeout(std::time::Duration::from_secs(5), listener.on_event(event))
                .await
                .is_err()
            {
                if let Some(session) = weak.upgrade() {
                    session.abort().await;
                }
                break;
            }
        }
    });
    Ok(Arc::new(IosRealtimeAudioSession {
        inner: session,
        _engine: engine,
    }))
}

#[uniffi::export(async_runtime = "tokio")]
impl IosRealtimeAudioSession {
    pub async fn send_audio(&self, pcm: Vec<u8>) -> Result<(), MobileEngineError> {
        self.inner.send_audio(pcm).map_err(realtime_error)
    }
    pub async fn commit_input(&self) -> Result<(), MobileEngineError> {
        self.inner.commit_input().map_err(realtime_error)
    }
    pub async fn interrupt(
        &self,
        item_id: Option<String>,
        audio_end_ms: Option<u32>,
    ) -> Result<(), MobileEngineError> {
        self.inner
            .interrupt(item_id, audio_end_ms)
            .map_err(realtime_error)
    }
    pub async fn playback_completed(
        &self,
        item_id: Option<String>,
    ) -> Result<(), MobileEngineError> {
        self.inner
            .playback_completed(item_id)
            .map_err(realtime_error)
    }
    pub async fn close(&self) -> Result<(), MobileEngineError> {
        self.inner.close().await.map_err(realtime_error)
    }
    pub async fn abort(&self) {
        self.inner.abort().await;
    }
}
fn realtime_error(error: serde_json::Value) -> MobileEngineError {
    MobileEngineError::Internal(error.to_string())
}
