//! JSON RPC over a [`Link`], independent of how the bytes travel.
//!
//! - request: `{id, method, params}` with a string `id`;
//! - response: `{id, result}` or `{id, error: {code, message}}`;
//! - notification: `{method, params}` without an `id`.
//!
//! Failure model: an [`RpcError`] returned from a request handler is answered with
//! its code and message; nothing else about the host leaks. Inbound text that is
//! not valid JSON, or not a request, response or notification, closes the peer with
//! `protocol_error`. When the link closes, every pending request fails with `closed`.
//!
//! Liveness: the `ping` notification is answered with `pong` here, and neither
//! reaches the application's notification handler. [`RpcPeer::start_keepalive`]
//! sends pings and gives up on a peer that has been silent too long.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::error::ProtocolError;
use crate::framing::{encode_message, Reassembler};
use crate::transport::{FragmentLink, Link, LinkEvent};
use crate::version::{
    DATA_CHANNEL_DEAD, DATA_CHANNEL_PING_INTERVAL, RPC_DEFAULT_TIMEOUT, TUNNEL_DEAD,
    TUNNEL_PING_INTERVAL,
};

/// The protocol's error codes, plus two that only ever arise locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoteErrorCode {
    /// The device is not approved yet.
    NotApproved,
    /// The request was already answered (first answer wins).
    NotPending,
    /// The host is at capacity.
    Capacity,
    /// The parameters are invalid.
    InvalidParams,
    /// The session does not exist.
    UnknownSession,
    /// The session is not shared remotely.
    NotRemoteEnabled,
    /// The host refuses this request.
    Forbidden,
    /// The protocol major differs.
    UpgradeRequired,
    /// Anything else.
    Internal,
    /// Local: no response arrived within the request's timeout.
    Timeout,
    /// Local: the peer or transport closed before the response arrived.
    Closed,
}

impl RemoteErrorCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotApproved => "not_approved",
            Self::NotPending => "not_pending",
            Self::Capacity => "capacity",
            Self::InvalidParams => "invalid_params",
            Self::UnknownSession => "unknown_session",
            Self::NotRemoteEnabled => "not_remote_enabled",
            Self::Forbidden => "forbidden",
            Self::UpgradeRequired => "upgrade_required",
            Self::Internal => "internal",
            Self::Timeout => "timeout",
            Self::Closed => "closed",
        }
    }

    /// Parse a wire code; unknown codes become `Internal`.
    pub fn from_wire(code: &str) -> Self {
        match code {
            "not_approved" => Self::NotApproved,
            "not_pending" => Self::NotPending,
            "capacity" => Self::Capacity,
            "invalid_params" => Self::InvalidParams,
            "unknown_session" => Self::UnknownSession,
            "not_remote_enabled" => Self::NotRemoteEnabled,
            "forbidden" => Self::Forbidden,
            "upgrade_required" => Self::UpgradeRequired,
            "timeout" => Self::Timeout,
            "closed" => Self::Closed,
            _ => Self::Internal,
        }
    }
}

/// An RPC failure. Return it from a request handler to answer with that `code`
/// and `message`; a failed `request()` carries what the peer sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct RpcError {
    /// The error code.
    pub code: RemoteErrorCode,
    /// A human-readable message.
    pub message: String,
}

impl RpcError {
    /// An error with this code and message.
    pub fn new(code: RemoteErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// An error whose message is its code.
    pub fn code(code: RemoteErrorCode) -> Self {
        Self::new(code, code.as_str())
    }
}

/// Answers an inbound request.
#[async_trait]
pub trait RequestHandler: Send + Sync {
    /// Resolve with the result, or fail with an [`RpcError`].
    async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError>;
}

/// Receives an inbound notification (never `ping` or `pong`).
pub trait NotificationHandler: Send + Sync {
    /// Handle one notification. Must not block.
    fn handle(&self, method: &str, params: Value);
}

impl<F> NotificationHandler for F
where
    F: Fn(&str, Value) + Send + Sync,
{
    fn handle(&self, method: &str, params: Value) {
        self(method, params);
    }
}

/// Which transport's liveness timings [`RpcPeer::start_keepalive`] uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveKind {
    /// Ping every 15 s, dead after 45 s.
    DataChannel,
    /// Ping every 25 s, dead after 75 s.
    Tunnel,
}

/// Options of [`RpcPeer::start`].
#[derive(Default)]
pub struct RpcOptions {
    /// Default request timeout; 30 s if `None`.
    pub timeout: Option<Duration>,
    /// Answers inbound requests; without one they fail with `internal`.
    pub requests: Option<Arc<dyn RequestHandler>>,
    /// Receives inbound notifications.
    pub notifications: Option<Arc<dyn NotificationHandler>>,
}

/// Why a send failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// The message breaks the framing rules (above 16 MiB).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The transport is closed.
    #[error("transport closed: {0}")]
    Closed(String),
}

type Responder = oneshot::Sender<Result<Value, RpcError>>;

struct Shared {
    link: Arc<dyn FragmentLink>,
    send_lock: tokio::sync::Mutex<()>,
    pending: Mutex<HashMap<String, Responder>>,
    next_id: AtomicU64,
    closed: watch::Sender<Option<String>>,
    last_received: Mutex<Instant>,
    keepalive: Mutex<Option<JoinHandle<()>>>,
    default_timeout: Duration,
    requests: Option<Arc<dyn RequestHandler>>,
    notifications: Option<Arc<dyn NotificationHandler>>,
}

/// One end of an RPC conversation. Both ends can send requests and notifications.
/// Inbound requests are handled concurrently. Cheap to clone.
#[derive(Clone)]
pub struct RpcPeer {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for RpcPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcPeer")
            .field("closed", &self.closed_reason())
            .finish_non_exhaustive()
    }
}

struct PendingGuard<'a> {
    shared: &'a Shared,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.shared
            .pending
            .lock()
            .expect("pending")
            .remove(&self.id);
    }
}

impl RpcPeer {
    /// Start the peer on a link: spawns the task that reads inbound fragments.
    /// Must be called inside a Tokio runtime.
    pub fn start(link: Link, options: RpcOptions) -> Self {
        let (closed, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            link: link.link,
            send_lock: tokio::sync::Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            closed,
            last_received: Mutex::new(Instant::now()),
            keepalive: Mutex::new(None),
            default_timeout: options.timeout.unwrap_or(RPC_DEFAULT_TIMEOUT),
            requests: options.requests,
            notifications: options.notifications,
        });
        tokio::spawn(read_loop(Arc::clone(&shared), link.events));
        Self { shared }
    }

    /// `true` once the peer or its transport has closed.
    pub fn is_closed(&self) -> bool {
        self.closed_reason().is_some()
    }

    /// Why the peer closed, if it has.
    pub fn closed_reason(&self) -> Option<String> {
        self.shared.closed.borrow().clone()
    }

    /// Wait until the peer closes; returns the reason.
    pub async fn closed(&self) -> String {
        let mut receiver = self.shared.closed.subscribe();
        loop {
            if let Some(reason) = receiver.borrow_and_update().clone() {
                return reason;
            }
            if receiver.changed().await.is_err() {
                return "closed".into();
            }
        }
    }

    /// Send a request and wait for its result. Fails with the peer's code, `timeout`
    /// after `timeout` (default 30 s), or `closed`.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, RpcError> {
        let shared = &self.shared;
        if let Some(reason) = self.closed_reason() {
            return Err(RpcError::new(RemoteErrorCode::Closed, reason));
        }
        let id = (shared.next_id.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        let mut message = Map::new();
        message.insert("id".into(), Value::String(id.clone()));
        message.insert("method".into(), Value::String(method.to_owned()));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        let text = Value::Object(message).to_string();
        let (responder, receiver) = oneshot::channel();
        shared
            .pending
            .lock()
            .expect("pending")
            .insert(id.clone(), responder);
        let _guard = PendingGuard {
            shared,
            id: id.clone(),
        };
        let exchange = async {
            if let Err(error) = send_text(shared, &text).await {
                return Err(match error {
                    // A request the framing refuses (above 16 MiB) is the caller's fault.
                    SendError::Protocol(e) if !self.is_closed() => {
                        RpcError::new(RemoteErrorCode::InvalidParams, e.0)
                    }
                    _ => RpcError::new(
                        RemoteErrorCode::Closed,
                        self.closed_reason().unwrap_or_else(|| "closed".into()),
                    ),
                });
            }
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err(RpcError::new(
                    RemoteErrorCode::Closed,
                    self.closed_reason().unwrap_or_else(|| "closed".into()),
                )),
            }
        };
        match tokio::time::timeout(timeout.unwrap_or(shared.default_timeout), exchange).await {
            Ok(result) => result,
            Err(_) => Err(RpcError::new(
                RemoteErrorCode::Timeout,
                format!("{method} timed out"),
            )),
        }
    }

    /// Send a notification. Succeeds without sending if the peer is closed, since a
    /// notification promises no delivery anyway. Fails only for a message the framing
    /// refuses (above 16 MiB), which the caller must handle rather than lose.
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), ProtocolError> {
        if self.is_closed() {
            return Ok(());
        }
        let mut message = Map::new();
        message.insert("method".into(), Value::String(method.to_owned()));
        if let Some(params) = params {
            message.insert("params".into(), params);
        }
        match send_text(&self.shared, &Value::Object(message).to_string()).await {
            Err(SendError::Protocol(error)) if !self.is_closed() => Err(error),
            _ => Ok(()),
        }
    }

    /// Keep the link alive and notice when it is dead: send a `ping` notification
    /// every 15 s (data channel) or 25 s (tunnel), and close the peer with
    /// `keepalive_timeout` once nothing, not just a `pong`, has arrived for 45 s or
    /// 75 s. Silence is counted from this call. Idempotent; stops with the peer.
    ///
    /// Call it on both ends: a long message holds back the pings queued behind it,
    /// and the other end's pings are what show the link is alive meanwhile. Every
    /// inbound fragment counts as activity, so a long download is not silence.
    pub fn start_keepalive(&self, kind: KeepaliveKind) {
        if self.is_closed() {
            return;
        }
        let mut slot = self.shared.keepalive.lock().expect("keepalive");
        if slot.is_some() {
            return;
        }
        let (interval, dead) = match kind {
            KeepaliveKind::DataChannel => (DATA_CHANNEL_PING_INTERVAL, DATA_CHANNEL_DEAD),
            KeepaliveKind::Tunnel => (TUNNEL_PING_INTERVAL, TUNNEL_DEAD),
        };
        *self.shared.last_received.lock().expect("last_received") = Instant::now();
        let peer = self.clone();
        *slot = Some(tokio::spawn(async move {
            let mut wait = interval;
            loop {
                tokio::time::sleep(wait).await;
                if peer.is_closed() {
                    return;
                }
                let silent = peer
                    .shared
                    .last_received
                    .lock()
                    .expect("last_received")
                    .elapsed();
                if silent >= dead {
                    peer.close("keepalive_timeout");
                    return;
                }
                let pinger = peer.clone();
                tokio::spawn(async move {
                    let _ = pinger.notify("ping", None).await;
                });
                // Pings keep their cadence; only the last wait is cut short, so the
                // verdict falls exactly at the dead time.
                wait = interval.min(dead - silent);
            }
        }));
    }

    /// Close the peer and its link. Idempotent.
    pub fn close(&self, reason: &str) {
        self.shared.link.close(reason);
        end(&self.shared, reason);
    }
}

async fn send_text(shared: &Shared, text: &str) -> Result<(), SendError> {
    let fragments = encode_message(text, shared.link.max_fragment_bytes())?;
    // Concurrent sends are serialized, so messages never interleave.
    let _turn = shared.send_lock.lock().await;
    // A timed-out request or an aborted caller may drop this future while the
    // link is backpressured. Once a split message may be on the wire, the next
    // sender cannot safely reuse the peer's half-filled reassembler.
    let mut partial = IncompleteSend {
        shared,
        armed: fragments.len() > 1,
    };
    for fragment in fragments {
        let closed = shared.closed.borrow().clone();
        if let Some(reason) = closed {
            return Err(SendError::Closed(reason));
        }
        shared
            .link
            .send_fragment(fragment)
            .await
            .map_err(|e| SendError::Closed(e.0))?;
    }
    partial.armed = false;
    Ok(())
}

struct IncompleteSend<'a> {
    shared: &'a Shared,
    armed: bool,
}

impl Drop for IncompleteSend<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.shared.link.close("send_incomplete");
            end(self.shared, "send_incomplete");
        }
    }
}

fn end(shared: &Shared, reason: &str) {
    let first = shared.closed.send_if_modified(|current| {
        if current.is_some() {
            return false;
        }
        *current = Some(reason.to_owned());
        true
    });
    if !first {
        return;
    }
    if let Some(handle) = shared.keepalive.lock().expect("keepalive").take() {
        handle.abort();
    }
    let pending: Vec<Responder> = shared
        .pending
        .lock()
        .expect("pending")
        .drain()
        .map(|(_, responder)| responder)
        .collect();
    for responder in pending {
        let _ = responder.send(Err(RpcError::new(RemoteErrorCode::Closed, reason)));
    }
}

async fn read_loop(shared: Arc<Shared>, mut events: mpsc::UnboundedReceiver<LinkEvent>) {
    let mut reassembler = Reassembler::new();
    let mut closed = shared.closed.subscribe();
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = closed.changed() => None,
        };
        match event {
            None => {
                end(&shared, "closed");
                return;
            }
            Some(LinkEvent::Closed(reason)) => {
                end(&shared, &reason);
                return;
            }
            Some(LinkEvent::Fragment(fragment)) => {
                if shared.closed.borrow().is_some() {
                    continue;
                }
                *shared.last_received.lock().expect("last_received") = Instant::now();
                match reassembler.push(&fragment) {
                    Ok(None) => {}
                    Ok(Some(text)) => receive(&shared, &text),
                    Err(_) => {
                        shared.link.close("protocol_error");
                        end(&shared, "protocol_error");
                        return;
                    }
                }
            }
        }
    }
}

fn protocol_error(shared: &Shared) {
    shared.link.close("protocol_error");
    end(shared, "protocol_error");
}

fn receive(shared: &Arc<Shared>, text: &str) {
    let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(text) else {
        return protocol_error(shared);
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let id = message.remove("id");
    match (method.filter(|m| !m.is_empty()), id) {
        (Some(method), None) => {
            if method == "ping" {
                let shared = Arc::clone(shared);
                tokio::spawn(async move {
                    let _ = notify_raw(&shared, "pong").await;
                });
            } else if method != "pong" {
                if let Some(handler) = &shared.notifications {
                    handler.handle(&method, message.remove("params").unwrap_or(Value::Null));
                }
            }
        }
        (Some(method), Some(Value::String(id))) => {
            let shared = Arc::clone(shared);
            let params = message.remove("params").unwrap_or(Value::Null);
            tokio::spawn(async move { answer(shared, id, method, params).await });
        }
        (Some(_), Some(_)) => protocol_error(shared),
        (None, Some(Value::String(id)))
            if message.contains_key("result") || message.contains_key("error") =>
        {
            settle(shared, &id, &message);
        }
        _ => protocol_error(shared),
    }
}

async fn notify_raw(shared: &Shared, method: &str) -> Result<(), SendError> {
    send_text(shared, &json!({ "method": method }).to_string()).await
}

fn settle(shared: &Shared, id: &str, message: &Map<String, Value>) {
    let Some(responder) = shared.pending.lock().expect("pending").remove(id) else {
        return; // answered too late: already timed out
    };
    let outcome = match message.get("error") {
        None | Some(Value::Null) => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        Some(Value::Object(error)) => match (
            error.get("code").and_then(Value::as_str),
            error.get("message").and_then(Value::as_str),
        ) {
            (Some(code), Some(text)) => Err(RpcError::new(RemoteErrorCode::from_wire(code), text)),
            _ => Err(RpcError::new(
                RemoteErrorCode::Internal,
                "malformed error response",
            )),
        },
        Some(_) => Err(RpcError::new(
            RemoteErrorCode::Internal,
            "malformed error response",
        )),
    };
    let _ = responder.send(outcome);
}

async fn answer(shared: Arc<Shared>, id: String, method: String, params: Value) {
    let outcome = match &shared.requests {
        None => Err(RpcError::new(
            RemoteErrorCode::Internal,
            "no request handler",
        )),
        Some(handler) => handler.handle(&method, params).await,
    };
    let text = match outcome {
        Ok(result) => json!({ "id": id, "result": result }).to_string(),
        Err(error) => {
            json!({ "id": id, "error": { "code": error.code.as_str(), "message": error.message } })
                .to_string()
        }
    };
    if shared.closed.borrow().is_some() {
        return;
    }
    match send_text(&shared, &text).await {
        Err(SendError::Protocol(_)) if shared.closed.borrow().is_none() => {
            // A result the framing refuses (above 16 MiB) would leave the peer waiting
            // for its timeout, so tell it instead.
            let _ = send_text(
                &shared,
                &json!({ "id": id, "error": { "code": "internal", "message": "response too large" } })
                    .to_string(),
            )
            .await;
        }
        // Usually the transport closed while the handler ran; the peer will see that itself.
        _ => {}
    }
}
