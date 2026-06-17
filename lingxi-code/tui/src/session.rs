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
    /// (MULTIMODAL.1) A clone of the bridge SENDER, threaded into the root
    /// component so its live-key turn-spawn pump (`crate::root::pump_turn`) can
    /// emit `TurnStarted`/`TurnEnded` on the same channel the orchestrator
    /// streams text/tool events onto. `None` (smoke gates / resume picker)
    /// leaves the live loop unable to spawn a turn — correct for those mounts.
    pub turn_tx: Option<mpsc::UnboundedSender<crate::events::orchestrator_bridge::TurnEvent>>,
    /// (ARGS.3) Shared command registry, used once at init to populate the
    /// progressive argument-hint map. `None` (smoke gates / no-CLI mounts) leaves
    /// the hint map empty — correct (no builtin declares argNames).
    pub command_registry: Option<Arc<tokio::sync::RwLock<command_api::CommandRegistry>>>,
    /// Prior conversation, mapped to scrollback rows, that a RESUMED session
    /// seeds into `AppState.messages` BEFORE the first render — the Rust analog
    /// of claude-code's REPL `initialMessages` prop (`main.tsx` →
    /// `loadConversationForResume` → `initialMessages` →
    /// `useState(initialMessages ?? [])`). Built via
    /// [`crate::replay::rebuild_messages`] from the persisted transcript. EMPTY
    /// for a FRESH session, so a non-resumed start is byte-identical to today
    /// (no replay). Set by the CLI's resume branch via
    /// [`Runtime::with_resumed_messages`].
    pub resumed_messages: Vec<crate::state::RenderedMessage>,
    /// Shared subscription slot from the composition root (None in print mode /
    /// tests). Threaded into [`AppState::subscription`] at mount.
    subscription: Option<traits::subscription::SharedSubscription>,
    /// (A6 batch-6 Task 2) Parsed `statusLine` setting (User+Local merged,
    /// Local-over-User), or `None` when unset / not a `command` config. Threaded
    /// into [`AppState::status_line_config`] at mount so the debounced statusline
    /// pump (root.rs) can run the configured command. `None` (smoke gates /
    /// resume picker / no settings) leaves the built-in status row in place.
    status_line_config: Option<crate::components::status_line_command::StatusLineConfig>,
    /// (Plan 3c §8) Per-provider availability map computed engine-side at
    /// `build()` (`DesktopRuntime.provider_availability`); threaded into the App
    /// (`AppState::set_provider_availability`) at mount so the `/model` picker can
    /// badge unconfigured providers. Empty (the default) keeps every row
    /// available — byte-identical to the historical behavior.
    provider_availability: std::collections::BTreeMap<String, bool>,
    /// (Plan 3c I1/I2) Authoritative `request_model -> (profile_name,
    /// provider_label)` map computed engine-side at `build()`
    /// (`DesktopRuntime.model_providers`); threaded into the App
    /// (`AppState::set_model_providers`) at mount so the `/model` picker can
    /// resolve a bare USER-provider model id to its own group + availability gate.
    /// Empty (the default) keeps the historical Built-in fallback.
    model_providers: std::collections::BTreeMap<String, (String, String)>,
    /// (Plan 3c C1) Shared engine credential store (`DesktopRuntime.credentials`)
    /// threaded into the App (`AppState::set_provider_key_store`) at mount so the
    /// `/connect` screen's `pump_store_provider_key` persists a collected key.
    /// `None` (the default) leaves the pump a no-op (smoke gates / resume picker).
    provider_key_store: Option<Arc<secret::CredentialManager>>,
    /// (TUI-PERM) Receiver for `TuiPermissionGate` exchanges, handed in by the
    /// CLI (`build_runtime_for_tui`). `None` (smoke gates / resume picker) keeps
    /// the permission pump inert. Moved into a take-once slot at mount.
    pub permission_rx:
        Option<tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>>,
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
            turn_tx: None,
            command_registry: None,
            resumed_messages: Vec::new(),
            subscription: None,
            status_line_config: None,
            provider_availability: std::collections::BTreeMap::new(),
            model_providers: std::collections::BTreeMap::new(),
            provider_key_store: None,
            permission_rx: None,
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
            turn_tx: None,
            command_registry: None,
            resumed_messages: Vec::new(),
            subscription: None,
            status_line_config: None,
            provider_availability: std::collections::BTreeMap::new(),
            model_providers: std::collections::BTreeMap::new(),
            provider_key_store: None,
            permission_rx: None,
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

    /// (MULTIMODAL.1) Attach the bridge sender clone so the root component's
    /// live-key turn-spawn pump (`crate::root::pump_turn`) can drive streaming
    /// turns. Without it the live loop echoes the user line but spawns no turn.
    #[must_use]
    pub fn with_turn_tx(
        mut self,
        turn_tx: mpsc::UnboundedSender<crate::events::orchestrator_bridge::TurnEvent>,
    ) -> Self {
        self.turn_tx = Some(turn_tx);
        self
    }

    /// (TUI-PERM) Attach the `TuiPermissionGate` receiver so the root's
    /// permission pump can drain it. Without it the interactive prompt never
    /// appears (the engine still auto-allows / denies per its gate selection).
    #[must_use]
    pub fn with_permission_rx(
        mut self,
        permission_rx: tokio::sync::mpsc::Receiver<crate::permission_bridge::PermissionExchange>,
    ) -> Self {
        self.permission_rx = Some(permission_rx);
        self
    }

    /// (ARGS.3) Attach the shared command registry. Read once at init to
    /// populate the progressive argument-hint map (`set_command_argument_names`),
    /// so custom markdown commands that declare `argNames` show the inline ghost
    /// hint. Without it the map stays empty — correct, since no built-in declares
    /// `argNames` (the hint never renders for them either way).
    #[must_use]
    pub fn with_command_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<command_api::CommandRegistry>>,
    ) -> Self {
        self.command_registry = Some(registry);
        self
    }

    /// (B4 Task 5) Attach the composition root's shared subscription slot
    /// (`DesktopRuntime.subscription`, filled by the background profile+roles
    /// fetch). Threaded into [`AppState::subscription`] at mount so the
    /// rate-limit composer can read the live snapshot. `None` (print mode /
    /// smoke gates / resume picker) leaves the snapshot absent — the composer
    /// falls back to its subscription-less arms.
    #[must_use]
    pub fn with_subscription(mut self, sub: traits::subscription::SharedSubscription) -> Self {
        self.subscription = Some(sub);
        self
    }

    /// (A6 batch-6 Task 2) Attach the parsed `statusLine` config (User+Local
    /// merged at the CLI seam). Threaded into [`AppState::status_line_config`]
    /// at mount so the debounced statusline pump (root.rs) runs the configured
    /// command. `None` leaves the built-in status row in place — byte-identical
    /// to today.
    #[must_use]
    pub fn with_status_line_config(
        mut self,
        config: Option<crate::components::status_line_command::StatusLineConfig>,
    ) -> Self {
        self.status_line_config = config;
        self
    }

    /// (Plan 3c §8) Attach the engine-computed per-provider availability map
    /// (`DesktopRuntime.provider_availability`). Threaded into the App at init
    /// (`AppState::set_provider_availability`) so the `/model` picker can badge
    /// unconfigured providers. The default (empty) keeps every row available.
    #[must_use]
    pub fn with_provider_availability(
        mut self,
        provider_availability: std::collections::BTreeMap<String, bool>,
    ) -> Self {
        self.provider_availability = provider_availability;
        self
    }

    /// (Plan 3c I1/I2) Attach the engine-computed `request_model -> (profile_name,
    /// provider_label)` map (`DesktopRuntime.model_providers`). Threaded into the
    /// App at init (`AppState::set_model_providers`) so the `/model` picker can
    /// resolve a bare USER-provider model id to its own group + availability gate.
    /// The default (empty) keeps the historical Built-in fallback.
    #[must_use]
    pub fn with_model_providers(
        mut self,
        model_providers: std::collections::BTreeMap<String, (String, String)>,
    ) -> Self {
        self.model_providers = model_providers;
        self
    }

    /// (Plan 3c C1) Attach the shared engine credential store
    /// (`DesktopRuntime.credentials`). Threaded onto the App at init
    /// (`AppState::set_provider_key_store`) so `root::pump_store_provider_key`
    /// persists a key the `/connect` screen collected. Without it the store stays
    /// `None` and the pump is a no-op (smoke gates / resume picker).
    #[must_use]
    pub fn with_provider_key_store(
        mut self,
        provider_key_store: Arc<secret::CredentialManager>,
    ) -> Self {
        self.provider_key_store = Some(provider_key_store);
        self
    }

    /// Seed the prior conversation a RESUMED session should replay into the
    /// TUI scrollback before the first frame. The CLI's resume branch loads the
    /// persisted transcript (`session::SessionStorage::load` /
    /// `session::jsonl::load_session`) and maps it via
    /// [`crate::replay::rebuild_messages`], then threads the resulting rows
    /// through here. A FRESH session never calls this — its `resumed_messages`
    /// stays empty and the first render is byte-identical to today (no replay).
    #[must_use]
    pub fn with_resumed_messages(mut self, messages: Vec<crate::state::RenderedMessage>) -> Self {
        self.resumed_messages = messages;
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
    // (ARGS.3) Populate the progressive argument-hint map from the live command
    // registry (when the CLI threaded one in). `run_tui_session` is async, so we
    // take the read lock here. Custom markdown commands that declare `argNames`
    // will surface the inline ghost hint; built-ins declare none, so this is a
    // no-op for them. `None` (smoke gates / resume picker) leaves the map empty.
    if let Some(reg) = runtime.command_registry.as_ref() {
        let guard = reg.read().await;
        initial_state.set_command_argument_names(&guard);
    }
    // (B4 Task 5) Thread the composition root's shared subscription slot into
    // the state so the rate-limit composer can read the live snapshot via
    // `AppState::subscription_snapshot`. `None` (smoke gates / resume picker /
    // print mode) leaves the snapshot absent — same route as `command_registry`.
    initial_state.subscription = runtime.subscription.take();
    // (A6 batch-6 Task 2) Thread the merged `statusLine` config onto the state
    // so the debounced statusline pump (root.rs) runs the configured command.
    // `None` (smoke gates / resume picker / no settings) leaves the built-in
    // status row in place — same route as `subscription`.
    initial_state.status_line_config = runtime.status_line_config.take();
    // (Plan 3c §8 / I1/I2) Thread the engine-computed provider maps onto the
    // state so the `/model` picker can badge unconfigured providers and resolve a
    // bare USER-provider model id to its own group + availability gate. Empty (the
    // default, before the engine populates them) keeps every row available +
    // grouped under its static label — byte-identical to the historical behavior.
    initial_state.set_provider_availability(std::mem::take(&mut runtime.provider_availability));
    initial_state.set_model_providers(std::mem::take(&mut runtime.model_providers));
    // (Plan 3c C1) Thread the shared engine credential store onto the state so
    // the `/connect` screen's `pump_store_provider_key` can persist a collected
    // key via `CredentialManager::set_provider_key`. `None` (smoke gates / resume
    // picker) leaves the pump a no-op — byte-identical to the pre-seam behavior.
    initial_state.set_provider_key_store(runtime.provider_key_store.take());
    // (M7-15) Apply the stored theme preference from ~/.claude/settings.json
    // (best-effort; absent/unreadable → session-default `auto`). Read once at
    // startup, before the first render, so the very first frame uses the saved
    // theme.
    if let Some(setting) = crate::theme_persist::load_theme_setting() {
        initial_state.set_theme(setting);
    }
    // (GAP D) Install the runtime keybindings keymap (merged default + user
    // `~/.claude/keybindings.json`), which the live PRIMARY dispatch consults
    // before the hardcoded `map_iocraft_key` table. The customization gate is
    // OFF by default — same as the `/keybindings` command's
    // `isKeybindingCustomizationEnabled` (the `tengu_keybinding_customization_release`
    // GrowthBook flag, which external users never have) — so `load_keybindings`
    // returns the canonical defaults and this is byte-identical to the hardcoded
    // chords. When a build flips the gate on, the user's overrides flow through
    // here unchanged. `Keymap::defaults()` already seeded `AppState::new`, so a
    // gate-off load is a behavioral no-op; we still call it so the single load
    // path is exercised. `false` = the customization gate off (external default).
    initial_state.set_keymap(command_core::keybindings::Keymap::load(
        false,
        &command_core::keybindings::keybindings_path(),
        cfg!(target_os = "macos"),
    ));
    // Transcript replay on resume: seed the prior conversation into scrollback
    // BEFORE the first render so a resumed session shows its existing history
    // on the very first frame (claude-code REPL `initialMessages`). EMPTY for a
    // fresh session, so this is a strict no-op there and the first frame is
    // byte-identical to today.
    if !runtime.resumed_messages.is_empty() {
        initial_state.seed_resumed_messages(std::mem::take(&mut runtime.resumed_messages));
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

    // (TUI-PERM) Move the permission receiver into a take-once slot for the
    // root's permission pump, mirroring `rx_slot`.
    let permission_rx_slot: Option<crate::root::PermissionRxSlot> = runtime
        .permission_rx
        .take()
        .map(|rx| Arc::new(std::sync::Mutex::new(Some(rx))));

    let result = element! {
        TuiRoot(
            state: Some(state.clone()),
            bridge_rx: Some(rx_slot),
            permission_rx: permission_rx_slot,
            cancel: Some(cancel.clone()),
            session_id: Some(runtime.session_id),
            started_at: Some(started),
            orchestrator: runtime.orchestrator.clone(),
            multiagent_rx: multiagent_rx,
            multiagent_tx: multiagent_tx,
            multiagent_feed: runtime.multiagent_feed.clone(),
            turn_tx: runtime.turn_tx.clone(),
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

    /// (A6 batch-6 Task 2) `status_line_config` defaults to `None`, and the
    /// `with_status_line_config` builder threads a parsed config through, which
    /// `run_tui_session` lifts onto `AppState.status_line_config` at mount
    /// (mirroring the `with_subscription` end-to-end path).
    #[test]
    fn with_status_line_config_defaults_none_and_threads_through() {
        use crate::components::status_line_command::StatusLineConfig;
        // Default builder leaves it absent.
        let runtime = Runtime::new(protocol::SessionId::new());
        assert!(runtime.status_line_config.is_none());

        // The builder threads a parsed config through the runtime.
        let cfg = StatusLineConfig::from_settings_value(
            &serde_json::json!({"type": "command", "command": "echo hi"}),
        )
        .expect("command config parses");
        let mut runtime = Runtime::new(protocol::SessionId::new())
            .with_status_line_config(Some(cfg.clone()));
        assert_eq!(runtime.status_line_config.as_ref(), Some(&cfg));

        // Replicate `run_tui_session`'s init-application step: lift the config
        // off the runtime onto the AppState (same route as `subscription`).
        let mut state = AppState::new(StatusSnapshot::default());
        state.status_line_config = runtime.status_line_config.take();
        assert_eq!(state.status_line_config.as_ref(), Some(&cfg));
    }

    /// (ARGS.3) End-to-end live-data-path proof without a PTY: a registry with a
    /// markdown command declaring `argNames`, threaded through the real
    /// `with_command_registry` builder, drives the same init-application step
    /// `run_tui_session` runs (read the registry, call
    /// `set_command_argument_names`) and surfaces the inline progressive hint.
    #[tokio::test]
    async fn with_command_registry_populates_argument_hint() {
        use command_api::model::{
            CommandFrontmatter, CommandSource, SlashCommand, SlashCommandKind,
        };
        use command_api::CommandRegistry;

        // Build a registry holding a custom markdown command with argNames.
        let mut reg = CommandRegistry::new();
        reg.register_command(SlashCommand {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            source: CommandSource::Project,
            kind: SlashCommandKind::Markdown {
                file_path: std::path::PathBuf::from("/x/deploy.md"),
                frontmatter: CommandFrontmatter::default(),
                prompt_template: String::new(),
            },
            argument_names: vec!["env".to_string(), "region".to_string()],
            ..SlashCommand::default()
        });
        let registry = Arc::new(tokio::sync::RwLock::new(reg));

        // Thread it through the real builder the CLI uses.
        let runtime =
            Runtime::new(protocol::SessionId::new()).with_command_registry(registry.clone());
        assert!(runtime.command_registry.is_some());

        // Replicate `run_tui_session`'s init-application step exactly: read the
        // registry off the runtime and populate the hint map.
        let mut state = AppState::new(StatusSnapshot::default());
        let reg_handle = runtime
            .command_registry
            .as_ref()
            .expect("registry threaded in");
        let guard = reg_handle.read().await;
        state.set_command_argument_names(&guard);
        drop(guard);

        // The live data path now renders the inline progressive hint.
        state.prompt_text = "/deploy ".to_string();
        assert_eq!(
            state.prompt_argument_hint(),
            Some("[env] [region]".to_string())
        );
        // One arg typed consumes the first declared name.
        state.prompt_text = "/deploy prod ".to_string();
        assert_eq!(state.prompt_argument_hint(), Some("[region]".to_string()));
    }
}
