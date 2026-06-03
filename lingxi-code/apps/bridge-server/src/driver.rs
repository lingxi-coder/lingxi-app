//! S1 — the production [`TurnDriver`] over a real [`ConversationOrchestrator`].
//!
//! This is the engine entry the [`crate::server::BridgeConnection`] calls when an
//! inbound [`client_protocol::commands::ClientCommand::SendPrompt`] arrives. It is
//! the production counterpart of the test-only driver that lived inside the F2-06
//! e2e suite (`tests/e2e_permission_test.rs`): a thin wrapper that turns one
//! `run_turn(prompt)` call into one streaming turn on the orchestrator.
//!
//! ## How the events reach the client
//!
//! The driver does NOT hold the [`client_adapter::AdapterOutputStream`] directly —
//! the orchestrator already owns it as its [`traits::OutputStream`] (wired at
//! construction, by `engine_desktop::build` in production or by the test harness).
//! Because that output stream lowers every callback into a
//! [`client_protocol::events::ClientEvent`] and forwards it through the
//! connection-scoped [`client_adapter::ClientEventSink`]
//! ([`crate::server::BridgeConnection::event_sink`]), simply driving the streaming
//! turn is enough: `TextDelta`, `ToolUseStarted`/`Result`, the per-turn
//! `CostUpdate`, and the terminal `TurnEnded` all flow out as `Frame::Event`s as a
//! side effect. The driver's only job is to START the turn (on a spawned task, per
//! the [`TurnDriver`] contract) and to surface a turn-level FAILURE as a
//! [`ClientEvent::Error`] so the client is never left waiting silently.
//!
//! ## Cancellation
//!
//! Each turn gets a fresh [`CancellationToken`]; the driver uses the orchestrator's
//! cancelable streaming entry ([`ConversationOrchestrator::run_turn_streaming_with_cancel`])
//! so a future per-turn cancel command (an `AbortTurn`-style control) has a hook to
//! fire it. Today nothing cancels mid-driver, so the token is never tripped and the
//! turn runs to completion — behavior identical to the plain streaming entry.

use std::sync::Arc;

use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::{ClientEvent, ErrorKindDto};
use orchestrator::{ConversationOrchestrator, OrchestratorError};
use tokio_util::sync::CancellationToken;

use crate::server::TurnDriver;

/// A production [`TurnDriver`] backed by a real [`ConversationOrchestrator`].
///
/// Wraps the orchestrator (whose [`client_adapter::AdapterOutputStream`] is already
/// wired to the connection's event sink) plus a clone of that same
/// [`ClientEventSink`] — held ONLY so a turn-level error (an `Err` out of the
/// streaming entry) can be surfaced as a [`ClientEvent::Error`]. Successful events
/// flow through the orchestrator's own output stream, not through this handle.
pub struct OrchestratorTurnDriver {
    orchestrator: Arc<ConversationOrchestrator>,
    /// The connection's event sink, shared with the orchestrator's output stream.
    /// Used solely to emit a terminal [`ClientEvent::Error`] on turn failure;
    /// `None` to drop errors silently (e.g. tests that only assert success).
    error_sink: Option<Arc<dyn ClientEventSink>>,
}

impl OrchestratorTurnDriver {
    /// Construct a driver that drops turn-level errors silently.
    ///
    /// Use this when the caller does not need failures surfaced as
    /// [`ClientEvent::Error`] (e.g. a test wired to a mock that never errors).
    /// The production server uses [`Self::with_error_sink`] so an error reaches
    /// the client.
    #[must_use]
    pub fn new(orchestrator: Arc<ConversationOrchestrator>) -> Self {
        Self {
            orchestrator,
            error_sink: None,
        }
    }

    /// Construct a driver that surfaces a turn-level failure as a
    /// [`ClientEvent::Error`] on `error_sink`.
    ///
    /// `error_sink` must be the SAME connection-scoped
    /// [`crate::server::BridgeConnection::event_sink`] the orchestrator's
    /// [`client_adapter::AdapterOutputStream`] was built from, so the error frame
    /// rides the one outbound channel in order behind any events the failed turn
    /// already streamed.
    #[must_use]
    pub fn with_error_sink(
        orchestrator: Arc<ConversationOrchestrator>,
        error_sink: Arc<dyn ClientEventSink>,
    ) -> Self {
        Self {
            orchestrator,
            error_sink: Some(error_sink),
        }
    }

    /// Map an [`OrchestratorError`] to a wire [`ClientEvent::Error`].
    ///
    /// The coarse [`ErrorKindDto`] mirrors the streaming output stream's own
    /// classification: API/transport failures are `Transport`, a stream-protocol
    /// violation is `Protocol`, a `max_turns` budget hit is `MaxTurns`, and any
    /// other internal failure is `Internal`.
    fn error_event(err: &OrchestratorError) -> ClientEvent {
        let kind = match err {
            OrchestratorError::ApiCall(_) | OrchestratorError::Streaming(_) => {
                ErrorKindDto::Transport
            }
            OrchestratorError::StreamingProtocol(_)
            | OrchestratorError::StreamEndedWithoutStop => ErrorKindDto::Protocol,
            OrchestratorError::MaxTurnsReached { .. } => ErrorKindDto::MaxTurns,
            _ => ErrorKindDto::Internal,
        };
        ClientEvent::Error {
            kind,
            message: err.to_string(),
        }
    }
}

#[async_trait]
impl TurnDriver for OrchestratorTurnDriver {
    async fn run_turn(&self, prompt: String) {
        // Each turn gets its own cancel token. Nothing trips it today; it is the
        // seam a future per-turn cancel command fires.
        let cancel = CancellationToken::new();
        match self
            .orchestrator
            .run_turn_streaming_with_cancel(&prompt, cancel)
            .await
        {
            // Success / cancellation / max-turns all already produced their
            // terminal events through the orchestrator's output stream
            // (`TurnEnded`, etc.) — nothing more to push here.
            Ok(_) => {}
            // A hard failure never reached `emit_end_turn`, so surface it
            // explicitly as a terminal `Error` event (when an error sink is wired)
            // rather than letting the client hang.
            Err(err) => {
                if let Some(sink) = &self.error_sink {
                    sink.emit(Self::error_event(&err)).await;
                } else {
                    tracing::debug!(error = %err, "bridge-server: turn failed (no error sink)");
                }
            }
        }
    }
}
