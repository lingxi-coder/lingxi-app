//! Foreground Decision proxy over the existing authenticated host connection.
use crate::audio_bridge::AudioRequestSink;
use async_trait::async_trait;
use client::protocol::events::ClientEvent;
use decision_api::{DecisionService, Error, Input, InvocationContext, Response};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Response, Error>>>>>;

/// Request half injected into the runtime's ordinary Decision tool.
pub struct DecisionBridge {
    sink: Arc<dyn AudioRequestSink>,
    pending: Pending,
    configured: Arc<AtomicBool>,
}
/// Completion half owned by exactly one authenticated bridge connection.
#[derive(Clone)]
pub struct DecisionResponder {
    pending: Pending,
    configured: Arc<AtomicBool>,
}

/// Audio, media and Decision share the connection's existing event sink.
pub fn new_decision_bridge(
    sink: Arc<dyn AudioRequestSink>,
) -> (Arc<DecisionBridge>, DecisionResponder) {
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let configured = Arc::new(AtomicBool::new(false));
    (
        Arc::new(DecisionBridge {
            sink,
            pending: pending.clone(),
            configured: configured.clone(),
        }),
        DecisionResponder {
            pending,
            configured,
        },
    )
}
impl DecisionResponder {
    /// Advertise only the host configuration flag; never expose backend details.
    pub fn configure(&self, configured: bool) {
        self.configured.store(configured, Ordering::Release);
    }
    /// IDs are random and scoped to this connection; late/duplicate replies are ignored.
    pub fn resolve(&self, request_id: &str, result_json: &str) -> bool {
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);
        let Some(sender) = sender else {
            return false;
        };
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reply {
            result: Option<Response>,
            error: Option<String>,
        }
        let result = if result_json.len() <= 16 * 1024 * 1024 {
            match serde_json::from_str::<Reply>(result_json) {
                Ok(Reply {
                    result: Some(response),
                    error: None,
                }) => Ok(response),
                Ok(Reply {
                    result: None,
                    error: Some(error),
                }) => Err(Error::Backend(error)),
                _ => Err(Error::Invalid("invalid Decision host response".into())),
            }
        } else {
            Err(Error::Invalid(
                "Decision host response exceeds 16 MiB".into(),
            ))
        };
        sender.send(result).is_ok()
    }
    /// Connection teardown terminates all callers; it never replays an evaluation.
    pub fn drain(&self) {
        self.configure(false);
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

struct PendingCall {
    runtime: tokio::runtime::Handle,
    request_id: String,
    pending: Pending,
    sink: Arc<dyn AudioRequestSink>,
}
impl Drop for PendingCall {
    fn drop(&mut self) {
        if self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.request_id)
            .is_some()
        {
            let sink = self.sink.clone();
            let request_id = self.request_id.clone();
            // Also runs when the tool dispatcher drops this future on interrupt.
            self.runtime.spawn(async move {
                sink.emit_request(ClientEvent::DecisionCancel { request_id })
                    .await;
            });
        }
    }
}

#[async_trait]
impl DecisionService for DecisionBridge {
    fn is_configured(&self) -> bool {
        self.configured.load(Ordering::Acquire)
    }
    async fn evaluate(&self, input: Input, context: InvocationContext) -> Result<Response, Error> {
        input.validate()?;
        if !self.is_configured() {
            return Err(Error::Unavailable);
        }
        if context.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let input_json = serde_json::to_string(&input)
            .map_err(|_| Error::Invalid("invalid Decision input".into()))?;
        if input_json.len() > 16 * 1024 * 1024 {
            return Err(Error::Invalid("Decision input exceeds 16 MiB".into()));
        }
        let timeout = context.timeout.min(Duration::from_secs(30));
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.len() >= 8 {
                return Err(Error::Backend("Decision service is busy".into()));
            }
            pending.insert(request_id.clone(), tx);
        }
        let _guard = PendingCall {
            runtime: tokio::runtime::Handle::current(),
            request_id: request_id.clone(),
            pending: self.pending.clone(),
            sink: self.sink.clone(),
        };
        let operation = async {
            if !self
                .sink
                .emit_request(ClientEvent::DecisionRequest {
                    request_id,
                    input_json,
                    timeout_ms: timeout.as_millis() as u64,
                })
                .await
            {
                return Err(Error::Unavailable);
            }
            rx.await.map_err(|_| Error::Unavailable)?
        };
        let response = tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => return Err(Error::Cancelled),
            result = tokio::time::timeout(timeout, operation) => result.map_err(|_| Error::Timeout)??,
        };
        response.validate_for_input(&input)?;
        Ok(response)
    }
}

#[cfg(test)]
#[path = "decision_bridge/tests.rs"]
mod tests;
