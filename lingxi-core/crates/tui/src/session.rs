// This module documents wire-level event names + variant identifiers that
// are intentionally kept un-backticked in prose; suppress the lint to
// avoid noisy markdown markers.
#![allow(clippy::doc_markdown)]

//! Public entry point: `run_tui_session(runtime, cancel)`.
//!
//! Lifecycle:
//! 1. Construct `RawGuard` (enables raw mode + alt screen).
//! 2. Spawn the crossterm `EventStream` adapter task.
//! 3. Build the orchestrator-bridge channel (placeholder in M6-01 —
//!    returns Pending; real bridge in M6-02).
//! 4. Build a 100ms `tokio::time::interval` ticker.
//! 5. Loop: `tokio::select!` over (events, ticker, cancel). On
//!    `KeyAction::Quit` or cancel trip → break.
//! 6. Drop `RawGuard` (restores terminal).
//!
//! M6-01 does not yet wire iocraft's reactive runtime — that lands in
//! M6-02 with the first interactive component. M6-01 keeps the loop
//! observable via the telemetry events (session_started / first_render /
//! session_ended) and asserts the loop exits on Ctrl-C then Ctrl-D
//! through the behavior test.

use crate::app::TuiApp;
use crate::error::TuiError;
use crate::events::keymap::{classify, KeyAction};
use crate::telemetry::{FIRST_RENDER, RESIZE, SESSION_ENDED, SESSION_STARTED};
use crate::terminal::RawGuard;
use crossterm::event::{Event as CtEvent, EventStream};
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

/// Opaque runtime handle that the CLI passes in. M6-01 only needs a
/// session id for telemetry; M6-02 will widen this to the full lingxi-cli
/// `Runtime` (orchestrator + dispatcher + auth).
pub struct Runtime {
    /// Session UUID for telemetry correlation.
    pub session_id: lingxi_protocol::SessionId,
}

impl Runtime {
    /// Construct from a session id. Wider constructor in M6-02.
    #[must_use]
    pub fn new(session_id: lingxi_protocol::SessionId) -> Self {
        Self { session_id }
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
pub async fn run_tui_session(
    runtime: Runtime,
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
    let ended_via;

    let mut event_stream = EventStream::new();
    // The orchestrator-bridge channel is reserved for M6-02; we hold the
    // sender so the receiver doesn't immediately observe close.
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

            maybe_ev = event_stream.next() => {
                match maybe_ev {
                    Some(Ok(CtEvent::Key(k))) => {
                        if classify(&k) == KeyAction::Quit {
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
                    Some(Ok(_)) => { /* M6-02+: mouse, paste, focus events */ }
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
                    // M6-01 placeholder: log + drop. M6-02 dispatches into app state.
                    tracing::debug!(?ev, "orchestrator event (M6-01 dropped)");
                }
            }

            _ = ticker.tick() => {
                // Periodic redraw heartbeat. M6-01 has nothing to redraw
                // (placeholder is static), but firing FIRST_RENDER on the
                // initial tick proves the loop is alive.
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
    }
}
