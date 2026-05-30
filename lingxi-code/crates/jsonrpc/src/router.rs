//! Outbound request routing — assigns numeric IDs, tracks pending oneshots,
//! enforces per-call timeout (default 60s), and removes pending entries when
//! the caller's future is dropped.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

use crate::messages::{Id, Notification, Request, Response, ResponseError};

/// Per-call timeout default — matches claude-code MCP / vscode-jsonrpc default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Outbound-routing errors.
#[derive(Debug, Error)]
pub enum RouterError {
    /// The remote peer returned a JSON-RPC error response.
    #[error("remote error: code={}, message={}", .0.code, .0.message)]
    Remote(ResponseError),
    /// The request timed out (the configured per-call deadline elapsed).
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    /// The writer half (broker) was dropped before the response arrived.
    #[error("router writer closed before response")]
    WriterClosed,
    /// Serializing the params into JSON failed.
    #[error("serialize params: {0}")]
    Serialize(serde_json::Error),
    /// Deserializing the result into the caller's type failed.
    #[error("deserialize result: {0}")]
    Deserialize(serde_json::Error),
}

type PendingMap = Arc<DashMap<Id, oneshot::Sender<Result<Value, ResponseError>>>>;

/// Outbound JSON-RPC router. Holds an atomic counter for outbound IDs and a
/// `DashMap` of pending oneshots keyed by Id.
#[derive(Clone)]
pub struct Router {
    next_id: Arc<AtomicI64>,
    pub(crate) pending: PendingMap,
    outbound: mpsc::UnboundedSender<OutboundMessage>,
    default_timeout: Duration,
}

/// What the writer task drains from the outbound queue.
#[derive(Debug)]
pub enum OutboundMessage {
    /// Outbound request awaiting a response (tracked in the pending map).
    Request(Request),
    /// Outbound notification (fire-and-forget, no pending entry).
    Notification(Notification),
}

impl Router {
    /// Construct a router. The `outbound` sender is owned by the writer task
    /// (typically the `Broker`); when that task drops the receiver, pending
    /// calls resolve with `RouterError::WriterClosed`.
    #[must_use]
    pub fn new(outbound: mpsc::UnboundedSender<OutboundMessage>) -> Self {
        Self {
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(DashMap::new()),
            outbound,
            default_timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the default per-call timeout.
    #[must_use]
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Drain side of the router used by the broker reader task: dispatch a
    /// received `Response` to the pending oneshot. Unknown IDs are dropped
    /// with a `tracing::warn!`.
    pub fn dispatch_response(&self, resp: Response) {
        let Some(id) = resp.id else {
            tracing::warn!("dropping response with null id");
            return;
        };
        let Some((_, sender)) = self.pending.remove(&id) else {
            tracing::warn!(?id, "dropping response for unknown id");
            return;
        };
        let outcome = match (resp.result, resp.error) {
            (Some(v), None) => Ok(v),
            (None, Some(e)) => Err(e),
            // Edge case: both or neither — surface as InternalError.
            _ => Err(ResponseError {
                code: crate::messages::INTERNAL_ERROR,
                message: "response had neither result nor error (or both)".into(),
                data: None,
            }),
        };
        let _ = sender.send(outcome);
    }

    /// Send an outbound notification (fire-and-forget).
    pub fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), RouterError> {
        let params = serde_json::to_value(params).map_err(RouterError::Serialize)?;
        let n = Notification::new(method, Some(params));
        self.outbound
            .send(OutboundMessage::Notification(n))
            .map_err(|_| RouterError::WriterClosed)?;
        Ok(())
    }

    /// Send an outbound request and await the typed response with the default timeout.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, RouterError> {
        self.call_with_timeout(method, params, self.default_timeout)
            .await
    }

    /// Send an outbound request and await the typed response with an explicit timeout.
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<R, RouterError> {
        let id = Id::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let params_value = serde_json::to_value(params).map_err(RouterError::Serialize)?;
        let req = Request::new(method, Some(params_value), id.clone());
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id.clone(), tx);

        // Drop guard: if the future is cancelled (dropped), remove the entry
        // so a late response from the peer doesn't accumulate.
        let drop_guard = DropGuard {
            pending: self.pending.clone(),
            id: id.clone(),
        };

        self.outbound
            .send(OutboundMessage::Request(req))
            .map_err(|_| {
                // Writer closed — eagerly clean up the pending slot.
                self.pending.remove(&id);
                RouterError::WriterClosed
            })?;

        let outcome = tokio::time::timeout(timeout, rx).await;
        // Successful completion — defuse the guard so we don't double-remove.
        std::mem::forget(drop_guard);

        let value = match outcome {
            Ok(Ok(Ok(v))) => v,
            Ok(Ok(Err(remote))) => return Err(RouterError::Remote(remote)),
            Ok(Err(_recv_err)) => return Err(RouterError::WriterClosed),
            Err(_elapsed) => {
                self.pending.remove(&id);
                return Err(RouterError::Timeout(timeout));
            }
        };

        serde_json::from_value(value).map_err(RouterError::Deserialize)
    }
}

/// Removes the pending entry on Drop unless explicitly defused.
struct DropGuard {
    pending: PendingMap,
    id: Id,
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.pending.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{Response, JSONRPC_VERSION};
    use serde_json::json;
    use std::time::Duration;

    fn router_with_writer() -> (Router, mpsc::UnboundedReceiver<OutboundMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Router::new(tx), rx)
    }

    #[tokio::test]
    async fn call_request_response_roundtrip() {
        let (router, mut rx) = router_with_writer();
        let router_clone = router.clone();

        // Spawn a fake responder.
        tokio::spawn(async move {
            let msg = rx.recv().await.unwrap();
            let OutboundMessage::Request(req) = msg else {
                panic!("expected request");
            };
            assert_eq!(req.method, "ping");
            assert_eq!(req.jsonrpc, JSONRPC_VERSION);
            // Echo back a response with the same id.
            let resp = Response::success(req.id, json!({"pong": true}));
            router_clone.dispatch_response(resp);
        });

        let out: serde_json::Value = router.call("ping", json!({})).await.unwrap();
        assert_eq!(out, json!({"pong": true}));
    }

    #[tokio::test]
    async fn call_propagates_remote_error() {
        let (router, mut rx) = router_with_writer();
        let router_clone = router.clone();
        tokio::spawn(async move {
            let OutboundMessage::Request(req) = rx.recv().await.unwrap() else {
                unreachable!("expected request");
            };
            let resp = Response::error(
                Some(req.id),
                ResponseError {
                    code: crate::messages::METHOD_NOT_FOUND,
                    message: "nope".into(),
                    data: None,
                },
            );
            router_clone.dispatch_response(resp);
        });

        let err = router
            .call::<_, serde_json::Value>("nope", json!({}))
            .await
            .unwrap_err();
        match err {
            RouterError::Remote(e) => {
                assert_eq!(e.code, -32601);
                assert_eq!(e.message, "nope");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn call_times_out_when_no_response() {
        tokio::time::pause();
        let (router, _rx) = router_with_writer();
        let fut = router.call_with_timeout::<_, serde_json::Value>(
            "hangs",
            json!({}),
            Duration::from_millis(50),
        );
        tokio::time::advance(Duration::from_millis(60)).await;
        let err = fut.await.unwrap_err();
        assert!(matches!(err, RouterError::Timeout(_)));
    }

    #[tokio::test]
    async fn writer_closed_yields_clear_error() {
        let (router, rx) = router_with_writer();
        drop(rx); // Simulate broker shutdown.
        let err = router
            .call::<_, serde_json::Value>("ping", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, RouterError::WriterClosed));
    }

    #[tokio::test]
    async fn notify_does_not_create_pending_entry() {
        let (router, mut rx) = router_with_writer();
        router.notify("hello", json!({"x": 1})).unwrap();
        let msg = rx.recv().await.unwrap();
        let OutboundMessage::Notification(n) = msg else {
            panic!("expected notification");
        };
        assert_eq!(n.method, "hello");
        assert_eq!(n.params, Some(json!({"x": 1})));
    }

    #[tokio::test]
    async fn default_timeout_constant_is_60s() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn dropping_call_future_removes_pending_entry() {
        let (router, _rx) = router_with_writer();
        // Spawn a `call` and immediately abort the task.
        let r2 = router.clone();
        let h = tokio::spawn(async move {
            let _: serde_json::Value = r2
                .call_with_timeout("hangs", json!({}), Duration::from_secs(5))
                .await
                .unwrap();
        });
        // Give the writer a moment to put the entry into `pending`.
        tokio::task::yield_now().await;
        assert_eq!(router.pending.len(), 1);

        h.abort();
        // Wait for the abort to actually run drop.
        let _ = h.await;
        // After the future is dropped, the pending slot must be cleaned up.
        assert_eq!(router.pending.len(), 0);
    }

    #[tokio::test]
    async fn one_hundred_concurrent_calls_resolve_to_correct_responses() {
        let (router, mut rx) = router_with_writer();
        let router_clone = router.clone();

        // Spawn a responder that echoes back `{"echo": <id>}` per request.
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let OutboundMessage::Request(req) = msg else {
                    continue;
                };
                let id_value = match &req.id {
                    Id::Number(n) => serde_json::json!(n),
                    Id::String(s) => serde_json::json!(s),
                };
                let resp = Response::success(req.id, json!({"echo": id_value}));
                router_clone.dispatch_response(resp);
            }
        });

        let mut handles = Vec::with_capacity(100);
        for i in 0..100 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let out: serde_json::Value =
                    r.call("echo", json!({"i": i})).await.expect("call ok");
                out
            }));
        }

        let results = futures::future::try_join_all(handles).await.unwrap();
        assert_eq!(results.len(), 100);
        // Each `echo` field must be a positive integer that matches the
        // numeric id we assigned. They MUST be distinct.
        let mut echoed_ids: Vec<i64> = results
            .into_iter()
            .map(|v| v["echo"].as_i64().expect("number"))
            .collect();
        echoed_ids.sort_unstable();
        // Ids start at 1 (Router::new sets AtomicI64::new(1)) and are monotonic.
        assert_eq!(echoed_ids, (1..=100).collect::<Vec<_>>());
    }
}
