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
    pub session_id: protocol::SessionId,
    /// Optional bridge for streaming events. `None` falls back to a static
    /// REPL with no orchestrator wiring (smoke / manual gates).
    pub bridge: Option<TuiBridge>,
    /// Initial status snapshot (model / cwd / cost).
    pub status: StatusSnapshot,
    /// (M7-13 review) Orchestrator handle for the async Settings open pump
    /// (`SettingsData::snapshot`). `None` (smoke gates / no-bridge mounts)
    /// leaves Settings unreachable — correct for those mounts.
    pub orchestrator: Option<Arc<dyn traits::OrchestratorHandle>>,
    /// (M9-05) Live multi-agent feed (desktop `PollerFeed` over the real
    /// `TaskRegistryHandle`). Drives the background-task footer + dialog. `None`
    /// (smoke gates / resume picker) leaves the task surface empty.
    pub multiagent_feed: Option<Arc<dyn crate::multiagent::MultiAgentFeed>>,
}

impl Runtime {
    /// Construct from a session id. No bridge, default status.
    #[must_use]
    pub fn new(session_id: protocol::SessionId) -> Self {
        Self {
            session_id,
            bridge: None,
            status: StatusSnapshot::default(),
            orchestrator: None,
            multiagent_feed: None,
        }
    }

    /// Construct with a streaming bridge + status snapshot.
    #[must_use]
    pub fn with_bridge(
        session_id: protocol::SessionId,
        bridge: TuiBridge,
        status: StatusSnapshot,
    ) -> Self {
        Self {
            session_id,
            bridge: Some(bridge),
            status,
            orchestrator: None,
            multiagent_feed: None,
        }
    }

    /// (M7-13 review) Attach the orchestrator handle that drives the async
    /// Settings open pump. Without it the Settings screen is unreachable.
    #[must_use]
    pub fn with_orchestrator(mut self, orchestrator: Arc<dyn traits::OrchestratorHandle>) -> Self {
        self.orchestrator = Some(orchestrator);
        self
    }

    /// (M9-05) Attach the live multi-agent feed (a `PollerFeed` over the real
    /// `TaskRegistryHandle`). The render loop polls it on the ticker and drains
    /// the produced events into `AppState.multiagent`, lighting up the
    /// background-task footer + dialog. Without it the task surface stays empty.
    #[must_use]
    pub fn with_multiagent_feed(
        mut self,
        feed: Arc<dyn crate::multiagent::MultiAgentFeed>,
    ) -> Self {
        self.multiagent_feed = Some(feed);
        self
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
    let mut initial_state = AppState::new(runtime.status.clone());
    // (M7-15) Apply the stored theme preference from ~/.claude/settings.json
    // (best-effort; absent/unreadable → session-default `auto`). Read once at
    // startup, before the first render, so the very first frame uses the saved
    // theme.
    if let Some(setting) = crate::theme_persist::load_theme_setting() {
        initial_state.set_theme(setting);
    }
    let state = Arc::new(Mutex::new(initial_state));

    // Move the bridge receiver into an `Arc<std::sync::Mutex<Option<...>>>`
    // slot so the iocraft root's first `use_future` can `take()` it once.
    let rx_slot: BridgeRxSlot =
        Arc::new(std::sync::Mutex::new(runtime.bridge.take().map(|b| b.rx)));

    // (M9-05) The MultiAgent channel — paired tx/rx for the live task surface.
    // The ticker pushes `pump_once(feed)` events onto `ma_tx`; the second pump
    // drains `ma_rx` into `AppState.multiagent`. Wired only when a feed is
    // present (`multiagent_feed`); otherwise all three props are `None` and the
    // pump/ticker poll stay inert. Mirrors the `rx_slot` take-once discipline.
    let (multiagent_rx, multiagent_tx) = match runtime.multiagent_feed.as_ref() {
        Some(_) => {
            let (tx, rx) = mpsc::unbounded_channel();
            let slot: crate::root::MultiAgentRxSlot = Arc::new(std::sync::Mutex::new(Some(rx)));
            (Some(slot), Some(tx))
        }
        None => (None, None),
    };

    let result = element! {
        TuiRoot(
            state: Some(state.clone()),
            bridge_rx: Some(rx_slot),
            cancel: Some(cancel.clone()),
            session_id: Some(runtime.session_id),
            started_at: Some(started),
            orchestrator: runtime.orchestrator.clone(),
            multiagent_rx: multiagent_rx,
            multiagent_tx: multiagent_tx,
            multiagent_feed: runtime.multiagent_feed.clone(),
        )
    }
    .fullscreen()
    .await;

    if let Err(e) = result {
        return Err(TuiError::Terminal(e));
    }

    Ok(())
}

/// Launch the TUI directly on the Resume screen, seeded with `rows`. Returns
/// the session UUID the user chose (`None` if cancelled). Used by the CLI's
/// `--resume` (no id) TTY branch (M7-12).
///
/// The ONLY differences from a normal [`run_tui_session`]: (a) the initial
/// `AppState` is seeded with `active_screen = Some(Screen::Resume(..))` so the
/// binary opens directly on the picker, and (b) there is no orchestrator
/// bridge to pump (the picker streams no turn). The mount is otherwise the
/// same `TuiRoot::fullscreen().await`; on Enter the screen sets
/// `resume_request` + `should_exit`, the mount unwinds, and we read the
/// recorded UUID back out.
///
/// # Errors
///
/// Returns `TuiError::Terminal` if iocraft's render loop fails (e.g. stdout
/// isn't a TTY — the CLI routes the non-TTY case to the stdio picker instead).
pub async fn run_resume_picker(
    rows: Vec<session::jsonl::loader::SessionMetadata>,
) -> Result<Option<uuid::Uuid>, TuiError> {
    use crate::screens::resume::{ResumeRow, ResumeState};
    use crate::screens::Screen;

    let display: Vec<ResumeRow> = rows.iter().map(ResumeRow::from_meta).collect();
    let mut app = AppState::new(StatusSnapshot::default());
    app.active_screen = Some(Screen::Resume(ResumeState::new(display)));
    // (M7-16) The resume picker seeds `active_screen` directly (not via an
    // `open_*` helper), so emit the screen-opened event here to keep the
    // `None → Some(_)` transition instrumented like every other screen.
    crate::telemetry::screen_opened("resume");
    let state = Arc::new(Mutex::new(app));

    let result = element! {
        TuiRoot(
            state: Some(state.clone()),
            bridge_rx: None,
            cancel: Some(CancellationToken::new()),
            session_id: None,
            started_at: Some(Instant::now()),
            // The resume picker is bridge-less + handle-less: Settings is
            // unreachable here, which is correct (the picker streams no turn).
            orchestrator: None,
        )
    }
    .fullscreen()
    .await;

    if let Err(e) = result {
        return Err(TuiError::Terminal(e));
    }

    let chosen = state.lock().await.resume_request;
    Ok(chosen)
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
        let id = protocol::SessionId::new();
        let r = Runtime::new(id);
        assert_eq!(r.session_id, id);
        assert!(r.bridge.is_none());
    }

    #[test]
    fn runtime_with_bridge_carries_rx() {
        let id = protocol::SessionId::new();
        let (_, rx) = mpsc::unbounded_channel();
        let r = Runtime::with_bridge(id, TuiBridge { rx }, StatusSnapshot::default());
        assert!(r.bridge.is_some());
    }
}
