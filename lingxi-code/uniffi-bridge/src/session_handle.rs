//! Per-session façade handle.
//!
//! `SessionHandle` is what callers receive from
//! [`crate::EngineHandle::create_session`] / `resume_session`. Internally it
//! holds a tokio `Mutex` around the session-level mutable state so the future
//! run loop can mutate it without `&mut self` on the handle (which `UniFFI`
//! forbids).
//!
//! Read methods (`id`, `message_count`) use `try_lock` to stay non-blocking
//! from the FFI — a contended lock returns a default value rather than
//! parking the host thread.

use protocol::SessionId;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Opaque per-session handle.
///
/// Cheap to `Clone` — internally an `Arc<Mutex<...>>` so the same logical
/// session can be referenced from multiple host callers safely.
#[derive(Clone)]
pub struct SessionHandle {
    inner: Arc<Mutex<SessionInner>>,
}

struct SessionInner {
    session_id: SessionId,
    #[allow(dead_code)] // surfaced via getters in Plan 17; recorded for parity.
    model: String,
    message_count: u64,
}

impl SessionHandle {
    /// Create a fresh session bound to `model`. Generates a new
    /// [`SessionId`].
    #[must_use]
    pub fn new(model: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner {
                session_id: SessionId::new(),
                model,
                message_count: 0,
            })),
        }
    }

    /// Return the session id as a UniFFI-friendly string.
    ///
    /// Non-blocking: if the inner mutex is held by another task, returns an
    /// empty string. Bindings should treat the empty string as "no answer
    /// available right now" and retry later.
    #[must_use]
    pub fn id(&self) -> String {
        self.inner
            .try_lock()
            .map(|i| i.session_id.to_string())
            .unwrap_or_default()
    }

    /// Number of messages exchanged in this session so far.
    ///
    /// Non-blocking; see [`Self::id`] for the contention behaviour.
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.inner.try_lock().map(|i| i.message_count).unwrap_or(0)
    }
}
