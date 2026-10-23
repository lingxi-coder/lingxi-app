//! A minimal Firebase Realtime Database client over REST and Server-Sent
//! Events, plus an in-memory stand-in with the same semantics.
//!
//! Everything else in the crate talks to the [`Rtdb`] trait, so tests and other
//! crates substitute [`MemoryRtdb`]. No Firebase SDK is involved: the caller
//! supplies ID tokens through a [`TokenProvider`].

mod client;
mod memory;
mod mirror;
pub mod tree;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

pub use client::{RtdbClient, RtdbClientOptions};
pub use memory::MemoryRtdb;
pub use mirror::{ChangeKind, RtdbChange, RtdbChanges, RtdbMirror};

/// Supplies Firebase ID tokens (the `auth` query value).
///
/// Called for every request, so it should cache. `force_refresh` is `true` on the
/// single retry after a 401 and after `auth_revoked`.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    /// A valid ID token, minted afresh when `force_refresh` is set.
    async fn id_token(&self, force_refresh: bool) -> Result<String, TokenError>;
}

/// A token could not be obtained.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("token unavailable: {0}")]
pub struct TokenError(pub String);

/// A [`TokenProvider`] that always returns the same token (tests, the emulator).
#[derive(Clone)]
pub struct StaticToken(pub String);

impl std::fmt::Debug for StaticToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StaticToken(..)")
    }
}

#[async_trait]
impl TokenProvider for StaticToken {
    async fn id_token(&self, _force_refresh: bool) -> Result<String, TokenError> {
        Ok(self.0.clone())
    }
}

/// One change notification, as in the RTDB streaming protocol. `path` is relative
/// to the listened location (`/` is the location itself).
#[derive(Debug, Clone, PartialEq)]
pub struct RtdbEvent {
    /// `Put`: the value at `path` is now `data` (`null` means deleted). `Patch`:
    /// `data` is an object whose keys are child paths below `path` (possibly
    /// multi-segment) and whose values replace those children.
    pub kind: RtdbEventKind,
    /// Path relative to the listened location.
    pub path: String,
    /// The payload.
    pub data: Value,
}

/// `put` or `patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtdbEventKind {
    /// Replace the value at the path.
    Put,
    /// Replace the listed children of the path.
    Patch,
}

/// Why a subscription ended for good: nothing is retried afterwards.
///
/// Two ways of being refused read access are told apart. A stream that cannot be
/// opened is an HTTP 401 `Permission denied`, but a 401 may only mean the token
/// expired, so it is tried once more at once with a fresh token; a second 401 is
/// reported as `Http`. A stream that was open and then lost its read access gets
/// `event: cancel` from the server, which is final as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtdbListenError {
    /// The server cancelled the stream (the security rules no longer allow the read).
    Cancel,
    /// A request failed in a way retrying cannot fix (400, 401 after a fresh token, 403, 404).
    Http {
        /// HTTP status.
        status: u16,
        /// Server message, with secrets redacted.
        message: String,
    },
}

/// What a [`Listener`] yields.
#[derive(Debug, Clone, PartialEq)]
pub enum ListenItem {
    /// A change.
    Event(RtdbEvent),
    /// The subscription ended for good; this is always the last item.
    Error(RtdbListenError),
}

/// A failed RTDB request. `status` is the HTTP status, or 0 when no response arrived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("RTDB error {status}: {message}")]
pub struct RtdbError {
    /// HTTP status, or 0 for a network or token failure.
    pub status: u16,
    /// Message with the token redacted.
    pub message: String,
}

impl RtdbError {
    /// An error with this status and message.
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Control surface of a running subscription.
pub trait ListenControl: Send + Sync {
    /// Stop listening. Idempotent.
    fn close(&self);
    /// Drop the current stream and open a new one at once, resetting the backoff.
    /// The new stream starts with a `put` at `/` carrying the whole subtree.
    fn reconnect(&self);
}

/// A live `listen`: a stream of [`ListenItem`]s. Dropping it closes the subscription.
///
/// The first event is always a `put` at `/` with the current subtree (or `null`).
/// **Every reconnect delivers that full-subtree `put` again**, so consumers must be
/// idempotent; [`RtdbMirror`] turns the repeat into a diff.
pub struct Listener {
    events: mpsc::UnboundedReceiver<ListenItem>,
    control: Arc<dyn ListenControl>,
}

impl Listener {
    /// Assemble a listener from its parts (for [`Rtdb`] implementations).
    pub fn new(
        events: mpsc::UnboundedReceiver<ListenItem>,
        control: Arc<dyn ListenControl>,
    ) -> Self {
        Self { events, control }
    }

    /// The next item; `None` once the subscription is closed and drained.
    pub async fn recv(&mut self) -> Option<ListenItem> {
        self.events.recv().await
    }

    /// Stop listening. Items already queued may still be received.
    pub fn close(&self) {
        self.control.close();
    }

    /// See [`ListenControl::reconnect`].
    pub fn reconnect(&self) {
        self.control.reconnect();
    }

    /// A handle that can close or reconnect after the listener moved into a task.
    pub fn control(&self) -> Arc<dyn ListenControl> {
        Arc::clone(&self.control)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.control.close();
    }
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener").finish_non_exhaustive()
    }
}

/// The slice of the Realtime Database the remote stack uses.
#[async_trait]
pub trait Rtdb: Send + Sync {
    /// Read the value at `path`; `null` when nothing is there.
    async fn get(&self, path: &str) -> Result<Value, RtdbError>;
    /// Replace the value at `path` (`null` deletes it).
    async fn put(&self, path: &str, value: &Value) -> Result<(), RtdbError>;
    /// Replace the listed children of `path`, leaving its other children alone (`null` deletes one).
    async fn patch(&self, path: &str, value: &Map<String, Value>) -> Result<(), RtdbError>;
    /// Delete `path`.
    async fn remove(&self, path: &str) -> Result<(), RtdbError>;
    /// Stream changes below `path`. Must be called inside a Tokio runtime.
    fn listen(&self, path: &str) -> Listener;
}

/// The entries of an RTDB value as `(key, child)` pairs, whether RTDB returned an
/// object or an array. RTDB turns integer-like keys into arrays and renders the
/// gaps as `null`; those holes (and any other `null`) are skipped.
pub fn child_entries(value: &Value) -> Vec<(String, &Value)> {
    match value {
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter(|(_, child)| !child.is_null())
            .map(|(index, child)| (index.to_string(), child))
            .collect(),
        Value::Object(object) => object
            .iter()
            .filter(|(_, child)| !child.is_null())
            .map(|(key, child)| (key.clone(), child))
            .collect(),
        _ => Vec::new(),
    }
}
