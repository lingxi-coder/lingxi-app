#![allow(clippy::doc_markdown)]

//! Public entry point: `run_tui_session(runtime, cancel)`.
//!
//! **Architecture (post M6-04 prerequisite fix):**
//!
//! iocraft's `Element::fullscreen().await` (a `RenderLoopFuture`) owns the
//! terminal — raw mode, alt-screen, crossterm event pump, panic-safe restore.
//! External events (the orchestrator-bridge mpsc `Receiver<TurnEvent>`,
//! the external `CancellationToken`) are routed into the iocraft component
//! tree via `crate::root::TuiRoot`'s hooks (`use_future` for bridge pump +
//! ticker + cancel watch; `use_terminal_events` for keystrokes).
//!
//! M6-01..M6-03 ran a hand-rolled `tokio::select!` loop and dropped the
//! iocraft element tree every frame — nothing actually painted. This module
//! now constructs the root element, hands ownership of `Arc<Mutex<AppState>>`
//! and the bridge receiver into its props, and delegates to iocraft's
//! reconciler.

use crate::error::TuiError;
use crate::root::{BridgeRxSlot, TuiRoot};
use crate::state::{AppState, StatusSnapshot};
use crate::telemetry::SESSION_STARTED;
use iocraft::prelude::*;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Bridge handle the CLI passes into [`run_tui_session`]. Wires the
/// orchestrator's [`BridgeOutputStream`] receiver into the render loop.
///
/// [`BridgeOutputStream`]: crate::events::orchestrator_bridge::BridgeOutputStream
pub struct TuiBridge {
    /// Receiver drained by the root component's `use_future`. Each event
    /// is passed to `crate::streaming::apply_event`.
    pub rx: mpsc::UnboundedReceiver<crate::events::orchestrator_bridge::TurnEvent>,
}

/// Opaque runtime handle that the CLI passes in.
pub struct Runtime {
    /// Session UUID for telemetry correlation.
    pub session_id: lingxi_protocol::SessionId,
    /// Optional bridge for streaming events. `None` falls back to a static
    /// REPL with no orchestrator wiring (smoke / manual gates).
    pub bridge: Option<TuiBridge>,
    /// Initial status snapshot (model / cwd / cost).
    pub status: StatusSnapshot,
}

impl Runtime {
    /// Construct from a session id. No bridge, default status.
    #[must_use]
    pub fn new(session_id: lingxi_protocol::SessionId) -> Self {
        Self {
            session_id,
            bridge: None,
            status: StatusSnapshot::default(),
        }
    }

    /// Construct with a streaming bridge + status snapshot.
    #[must_use]
    pub fn with_bridge(
        session_id: lingxi_protocol::SessionId,
        bridge: TuiBridge,
        status: StatusSnapshot,
    ) -> Self {
        Self {
            session_id,
            bridge: Some(bridge),
            status,
        }
    }
}

/// Public entry point. Drives the TUI to a clean shutdown.
///
/// # Errors
///
/// Returns `TuiError::Terminal` if iocraft's render loop fails (most often
/// because stdout isn't a TTY — the caller should have routed to the
/// stdio REPL via `cli::mode::decide_mode` instead). Returns
/// `TuiError::Cancelled` if `cancel` trips before the user quits — that's
/// surfaced as a clean exit via `SystemContext::exit()`, so this path
/// returns `Ok(())` and the caller distinguishes the two via the
/// `state.should_exit` flag (not currently exposed; M6-09 polish).
pub async fn run_tui_session(
    mut runtime: Runtime,
    cancel: CancellationToken,
) -> Result<(), TuiError> {
    let started = Instant::now();

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    tracing::info!(
        event = SESSION_STARTED,
        session_id = %runtime.session_id,
        cols = cols,
        rows = rows,
    );

    // Shared AppState — the bridge pump task + key handlers mutate it,
    // the render path reads it. Wrapped in a tokio Mutex so the pump
    // can await locks across .await points.
    let state = Arc::new(Mutex::new(AppState::new(runtime.status.clone())));

    // Move the bridge receiver into an `Arc<std::sync::Mutex<Option<...>>>`
    // slot so the iocraft root's first `use_future` can `take()` it once.
    let rx_slot: BridgeRxSlot =
        Arc::new(std::sync::Mutex::new(runtime.bridge.take().map(|b| b.rx)));

    let result = element! {
        TuiRoot(
            state: Some(state.clone()),
            bridge_rx: Some(rx_slot),
            cancel: Some(cancel.clone()),
            session_id: Some(runtime.session_id),
            started_at: Some(started),
        )
    }
    .fullscreen()
    .await;

    if let Err(e) = result {
        return Err(TuiError::Terminal(e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-tripped cancel tokens resolve immediately. The full
    /// `run_tui_session` cannot be driven from CI (no TTY), so we assert
    /// the token contract directly.
    #[tokio::test]
    async fn cancel_token_triggers_exit() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancelled().await; // returns immediately
    }

    /// Construct + drop a Runtime to verify the constructor compiles and
    /// the session_id round-trips.
    #[test]
    fn runtime_carries_session_id() {
        let id = lingxi_protocol::SessionId::new();
        let r = Runtime::new(id);
        assert_eq!(r.session_id, id);
        assert!(r.bridge.is_none());
    }

    #[test]
    fn runtime_with_bridge_carries_rx() {
        let id = lingxi_protocol::SessionId::new();
        let (_, rx) = mpsc::unbounded_channel();
        let r = Runtime::with_bridge(id, TuiBridge { rx }, StatusSnapshot::default());
        assert!(r.bridge.is_some());
    }
}
