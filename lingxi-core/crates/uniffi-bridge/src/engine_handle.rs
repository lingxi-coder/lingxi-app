//! Top-level façade handle.
//!
//! `EngineHandle` is the single entry point host bindings see; it owns the
//! collection of live [`SessionHandle`]s and exposes the lifecycle operations
//! (`create_session`, `resume_session`, `send_user_message`). The handle is
//! deliberately opaque — host code never touches engine internals; everything
//! crosses the FFI as DTOs or by-id references.
//!
//! M1.22 keeps the bodies stub-shaped (no real run loop) so the type surface
//! and ownership story can be locked in ahead of Plan 17's runtime wiring.

use crate::session_handle::SessionHandle;
use std::sync::Mutex;
use thiserror::Error;

/// Errors surfaced to the host bindings.
///
/// Mirrors the `UniFFI` `[Error]` enum that `lingxi_core.udl` will declare in
/// M2. Kept intentionally narrow — concrete subsystem errors are mapped into
/// these by the façade rather than leaking across the boundary.
#[derive(Debug, Clone, Error)]
pub enum EngineError {
    /// Requested session is not registered with this engine.
    #[error("session not found")]
    NotFound,
    /// The engine is in a state that does not allow the requested operation
    /// (for example sending a message after termination).
    #[error("invalid state")]
    InvalidState,
    /// Catch-all for engine-internal failures; the embedded message is safe
    /// to surface to logs.
    #[error("internal: {0}")]
    Internal(String),
}

/// Opaque façade over the engine.
///
/// Holds the live session table. Operations are synchronous from the host's
/// perspective; any tokio work happens inside the façade.
pub struct EngineHandle {
    inner: Mutex<EngineInner>,
}

struct EngineInner {
    sessions: Vec<SessionHandle>,
}

impl Default for EngineHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::unwrap_used)] // std Mutex poison is treated as a fatal bug here.
impl EngineHandle {
    /// Construct a fresh engine with no live sessions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                sessions: Vec::new(),
            }),
        }
    }

    /// Create a new session bound to `model` and register it on the engine.
    pub fn create_session(&self, model: String) -> SessionHandle {
        let h = SessionHandle::new(model);
        self.inner.lock().unwrap().sessions.push(h.clone());
        h
    }

    /// Resume a previously-persisted session by id.
    ///
    /// M1.22 returns a brand-new handle pinned to the default model — Plan 17
    /// wires this through `lingxi-session::SessionResumer`.
    #[allow(unused_variables, clippy::needless_pass_by_value)]
    pub fn resume_session(&self, session_id: String) -> SessionHandle {
        SessionHandle::new("claude-opus-4-6".into())
    }

    /// Drive one user-typed message through the engine. M1.22 returns a stub
    /// ack so the host plumbing can be exercised end-to-end; Plan 17 swaps
    /// in the real run loop.
    ///
    /// The handle and text are taken by value because the future `UniFFI`
    /// bindings pass owned types across the FFI boundary (`UniFFI` cannot
    /// model borrowed references).
    #[allow(clippy::needless_pass_by_value)]
    pub fn send_user_message(
        &self,
        _handle: SessionHandle,
        text: String,
    ) -> Result<String, EngineError> {
        Ok(format!("ack: {text}"))
    }
}
