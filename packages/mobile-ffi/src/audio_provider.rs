//! Canonical mobile audio host and realtime-session lifecycle.
use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use super::{PlatformRealtimeAudioListener, PlatformSecureStorage, PlatformSecureStorageBridge};
use harness_runtime::mobile::{MobileEngineError, MobileEngineHandle};

pub(super) struct AudioProviderCore {
    inner: audio_provider::AudioProviderHost,
}

impl AudioProviderCore {
    pub(super) fn new(
        profiles_json: String,
        region: String,
        secure_storage: Box<dyn PlatformSecureStorage>,
    ) -> Result<Self, MobileEngineError> {
        let storage = Arc::new(PlatformSecureStorageBridge {
            inner: secure_storage,
        });
        let inner = audio_provider::AudioProviderHost::new(
            &profiles_json,
            &region,
            Some(storage),
            BTreeMap::new(),
        )
        .map_err(MobileEngineError::Internal)?;
        Ok(Self { inner })
    }

    pub(super) fn for_engine(engine: Arc<MobileEngineHandle>) -> Self {
        let (services, credential_ids, _credentials) = engine.audio_provider_services();
        Self {
            inner: audio_provider::AudioProviderHost::from_provider_services(
                services,
                credential_ids,
            ),
        }
    }

    pub(super) async fn capabilities(&self, request_json: String) -> String {
        self.inner.capabilities(&request_json).await
    }
    pub(super) async fn transcribe(
        &self,
        request_json: String,
        audio: Vec<u8>,
        mime_type: String,
    ) -> String {
        self.inner
            .transcribe(&request_json, audio, &mime_type)
            .await
    }
    pub(super) async fn synthesize(&self, request_json: String, text: String) -> String {
        self.inner.synthesize(&request_json, &text).await
    }
    pub(super) async fn cancel(&self, operation_id: String) {
        self.inner.cancel(&operation_id);
    }
}

/// Own the exact engine that created the session until the native object is dropped.
pub(super) struct RealtimeAudioCore {
    inner: Arc<audio_provider::realtime::NativeRealtimeSession>,
    _engine: Arc<MobileEngineHandle>,
}

impl RealtimeAudioCore {
    pub(super) async fn start(
        engine: Arc<MobileEngineHandle>,
        provider_host: &AudioProviderCore,
        request_json: String,
        listener: Box<dyn PlatformRealtimeAudioListener>,
    ) -> Result<Self, MobileEngineError> {
        let runner = engine.clone();
        let (session, events) = audio_provider::realtime::NativeRealtimeSession::start_with_runner(
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
            forward_events(events, listener, move || async move {
                if let Some(session) = weak.upgrade() {
                    session.abort().await;
                }
            })
            .await;
        });
        Ok(Self {
            inner: session,
            _engine: engine,
        })
    }

    pub(super) async fn send_audio(&self, pcm: Vec<u8>) -> Result<(), MobileEngineError> {
        self.inner.send_audio(pcm).map_err(realtime_error)
    }
    pub(super) async fn commit_input(&self) -> Result<(), MobileEngineError> {
        self.inner.commit_input().map_err(realtime_error)
    }
    pub(super) async fn interrupt(
        &self,
        item_id: Option<String>,
        audio_end_ms: Option<u32>,
    ) -> Result<(), MobileEngineError> {
        self.inner
            .interrupt(item_id, audio_end_ms)
            .map_err(realtime_error)
    }
    pub(super) async fn playback_completed(
        &self,
        item_id: Option<String>,
    ) -> Result<(), MobileEngineError> {
        self.inner
            .playback_completed(item_id)
            .map_err(realtime_error)
    }
    pub(super) async fn close(&self) -> Result<(), MobileEngineError> {
        self.inner.close().await.map_err(realtime_error)
    }
    pub(super) async fn abort(&self) {
        self.inner.abort().await;
    }
}

fn realtime_error(error: serde_json::Value) -> MobileEngineError {
    MobileEngineError::Internal(error.to_string())
}

// Serial delivery preserves event order. A hung foreign callback must abort the
// native session to release its capture, transport and engine-owned agent work.
async fn forward_events<F, Fut>(
    mut events: tokio::sync::mpsc::Receiver<String>,
    listener: Box<dyn PlatformRealtimeAudioListener>,
    on_timeout: F,
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    while let Some(event) = events.recv().await {
        if tokio::time::timeout(Duration::from_secs(5), listener.on_event(event))
            .await
            .is_err()
        {
            on_timeout().await;
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    struct Recorder {
        seen: Arc<Mutex<Vec<String>>>,
        stall_on: Option<&'static str>,
    }

    #[async_trait::async_trait]
    impl PlatformRealtimeAudioListener for Recorder {
        async fn on_event(&self, event: String) {
            self.seen.lock().unwrap().push(event.clone());
            if self.stall_on == Some(event.as_str()) {
                std::future::pending::<()>().await;
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_callback_aborts_once_and_stops_delivering_events() {
        let (tx, rx) = tokio::sync::mpsc::channel(3);
        for event in ["first", "stalled", "late"] {
            tx.send(event.into()).await.unwrap();
        }
        drop(tx);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        forward_events(
            rx,
            Box::new(Recorder {
                seen: seen.clone(),
                stall_on: Some("stalled"),
            }),
            move || async move {
                abort_flag.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(*seen.lock().unwrap(), ["first", "stalled"]);
        assert!(aborted.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn completed_event_stream_drains_in_order_without_aborting() {
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tx.send("first".into()).await.unwrap();
        tx.send("second".into()).await.unwrap();
        drop(tx);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        forward_events(
            rx,
            Box::new(Recorder {
                seen: seen.clone(),
                stall_on: None,
            }),
            move || async move {
                abort_flag.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert_eq!(*seen.lock().unwrap(), ["first", "second"]);
        assert!(!aborted.load(Ordering::SeqCst));
    }
}
