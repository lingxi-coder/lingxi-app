// This module documents wire-level event names + variant identifiers that
// are intentionally kept un-backticked in prose; suppress the lint to
// avoid noisy markdown markers.
#![allow(clippy::doc_markdown)]

//! Public entry point: `run_tui_session(runtime, cancel)`.
//!
//! Lifecycle:
//! 1. Construct `RawGuard` (enables raw mode + alt screen).
//! 2. Spawn the crossterm `EventStream` adapter task.
//! 3. If `runtime.bridge` is `Some(_)` (M6-03+), drain `bridge.rx` into
//!    `streaming::apply_event` on the shared `AppState` and emit
//!    streaming-render telemetry when `state.streaming` transitions.
//! 4. Build a 100ms `tokio::time::interval` ticker for the spinner +
//!    keep-alive renders. A `tokio::sync::Notify` debounces the
//!    bridge-driven redraws to ~30fps.
//! 5. Loop: `tokio::select!` over (events, ticker, notify, cancel).
//! 6. Drop `RawGuard` (restores terminal).
//!
//! M6-03 wires the bridge + apply_event + rate-limit; the iocraft
//! reactive runtime mount is still incomplete (the loop here renders
//! through `app.render()` once per tick rather than driving iocraft's
//! reconciler reactively). The Streaming Gate in T12 manually checks
//! the result on real Anthropic SSE; if the manual gate fails the
//! plan's R3 mitigation directs us into M6-03b (ratatui pivot).

use crate::app::TuiApp;
use crate::error::TuiError;
use crate::events::keymap::{classify, KeyClass};
use crate::events::orchestrator_bridge::TurnEvent;
use crate::state::{AppState, StatusSnapshot};
use crate::streaming::apply_event;
use crate::telemetry::{
    FIRST_RENDER, RESIZE, SESSION_ENDED, SESSION_STARTED, STREAMING_RENDER_ENDED,
    STREAMING_RENDER_STARTED,
};
use crate::terminal::RawGuard;
use crossterm::event::{Event as CtEvent, EventStream};
use futures::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

/// Bridge handle the CLI passes into [`run_tui_session`]. Wires the
/// orchestrator's [`BridgeOutputStream`] receiver into the render loop.
///
/// [`BridgeOutputStream`]: crate::events::orchestrator_bridge::BridgeOutputStream
pub struct TuiBridge {
    /// Receiver drained by the render loop. Each event is passed to
    /// `crate::streaming::apply_event`.
    pub rx: mpsc::UnboundedReceiver<TurnEvent>,
}

/// Opaque runtime handle that the CLI passes in.
///
/// M6-01 only carried a session id. M6-03 widens to optionally carry a
/// [`TuiBridge`] so the streaming + render-loop wiring is end-to-end.
pub struct Runtime {
    /// Session UUID for telemetry correlation.
    pub session_id: lingxi_protocol::SessionId,
    /// Optional bridge for streaming events. `None` falls back to M6-01's
    /// static placeholder behaviour (no events drained).
    pub bridge: Option<TuiBridge>,
    /// Initial status snapshot (model / cwd / cost). Defaults are
    /// acceptable for the placeholder rendering path.
    pub status: StatusSnapshot,
}

impl Runtime {
    /// Construct from a session id. M6-01 shape — no bridge, default status.
    #[must_use]
    pub fn new(session_id: lingxi_protocol::SessionId) -> Self {
        Self {
            session_id,
            bridge: None,
            status: StatusSnapshot::default(),
        }
    }

    /// Construct with a streaming bridge + status snapshot. (M6-03)
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
/// Returns `TuiError::Terminal` if raw-mode toggling fails (most often
/// because stdout isn't a TTY — the caller should have routed to the
/// stdio REPL via `cli::mode::decide_mode` instead). Returns
/// `TuiError::Cancelled` if `cancel` trips before the user quits.
#[allow(clippy::too_many_lines)]
pub async fn run_tui_session(
    mut runtime: Runtime,
    cancel: CancellationToken,
) -> Result<(), TuiError> {
    let guard = RawGuard::enter()?;
    let started = Instant::now();

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    tracing::info!(
        event = SESSION_STARTED,
        session_id = %runtime.session_id,
        cols = cols,
        rows = rows,
    );

    let mut app = TuiApp::new();
    let mut first_render_emitted = false;
    let mut prev_streaming = false;
    let ended_via;

    // Shared AppState for the render path. Drained by both the bridge
    // pumper and the keyboard handlers (only the latter actually mutates
    // it in M6-03; permission/key dispatch lands in later sub-plans).
    let app_state: Arc<Mutex<AppState>> =
        Arc::new(Mutex::new(AppState::new(runtime.status.clone())));

    // Render-debounce notify. The bridge pumper calls `notify.notify_one`
    // after each `apply_event`; the render loop awaits `notify.notified`
    // and sleeps 33ms after each redraw to cap at ~30fps.
    let notify = Arc::new(Notify::new());

    // If a bridge is present, spawn a task that drains it into AppState.
    let bridge_task: Option<tokio::task::JoinHandle<()>> = if let Some(b) = runtime.bridge.take() {
        let state = app_state.clone();
        let notify = notify.clone();
        let mut rx = b.rx;
        Some(tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let mut st = state.lock().await;
                apply_event(&mut st, ev, &notify);
            }
        }))
    } else {
        None
    };

    let mut event_stream = EventStream::new();
    // The orchestrator-bridge channel from M6-01 is now obsolete; we keep
    // an unused sender so the receiver doesn't immediately close.
    let (_orch_tx, mut orch_rx) = mpsc::channel::<crate::events::TuiEvent>(64);
    let mut ticker = interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                ended_via = "cancelled";
                break;
            }

            // Bridge-driven redraw: drain accumulated state, emit
            // streaming transition telemetry, then sleep 33ms to enforce
            // the 30fps cap.
            () = notify.notified() => {
                let cur_streaming = app_state.lock().await.streaming.is_some();
                if cur_streaming && !prev_streaming {
                    tracing::info!(
                        event = STREAMING_RENDER_STARTED,
                        session_id = %runtime.session_id,
                    );
                } else if !cur_streaming && prev_streaming {
                    tracing::info!(
                        event = STREAMING_RENDER_ENDED,
                        session_id = %runtime.session_id,
                    );
                }
                prev_streaming = cur_streaming;
                // The actual iocraft reactive re-render hookup is M6-04
                // work — for M6-03 we render `TuiApp::render()` (placeholder)
                // so the loop touches the same code path the M6-04 mount
                // will replace.
                let _ = app.render();
                tokio::time::sleep(Duration::from_millis(33)).await;
            }

            maybe_ev = event_stream.next() => {
                match maybe_ev {
                    Some(Ok(CtEvent::Key(k))) => {
                        if classify(&k) == KeyClass::Quit {
                            app.request_quit();
                        }
                    }
                    Some(Ok(CtEvent::Resize(c, r))) => {
                        tracing::info!(
                            event = RESIZE,
                            session_id = %runtime.session_id,
                            cols = c,
                            rows = r,
                        );
                    }
                    Some(Ok(_)) => { /* M6-04+: mouse, paste, focus events */ }
                    Some(Err(e)) => {
                        return Err(TuiError::Terminal(e));
                    }
                    None => {
                        // crossterm stream closed (stdin EOF) — exit cleanly.
                        ended_via = "stream_eof";
                        break;
                    }
                }
            }

            maybe_orch = orch_rx.recv() => {
                if let Some(ev) = maybe_orch {
                    tracing::debug!(?ev, "orchestrator event (legacy channel, dropped)");
                }
            }

            _ = ticker.tick() => {
                if !first_render_emitted {
                    let _el = app.render();
                    tracing::info!(
                        event = FIRST_RENDER,
                        session_id = %runtime.session_id,
                        latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    );
                    first_render_emitted = true;
                }
            }
        }

        if app.should_quit {
            ended_via = "quit_key";
            break;
        }
    }

    if let Some(h) = bridge_task {
        h.abort();
    }
    drop(orch_rx); // explicit cleanup
    tracing::info!(
        event = SESSION_ENDED,
        session_id = %runtime.session_id,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ended_via = ended_via,
    );

    guard.exit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-tripped cancel tokens resolve immediately. The full
    /// `run_tui_session` cannot be driven from CI (no TTY), so we assert
    /// the token contract directly; the real-terminal exercise lives in
    /// Task 14 Step 3 (manual smoke).
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
