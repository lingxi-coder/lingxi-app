//! Screens — single top-level views composed from the `components/` module.
//!
//! M6-02 shipped only `repl::ReplScreen`. M7-11 establishes the screen-overlay
//! route-state (`Screen` + `AppState.active_screen`); later sub-plans add
//! `Resume` (M7-12), `Settings` (M7-13), `Memory` (M7-14).

pub mod agents;
pub mod background_tasks;
pub mod connect;
pub mod connect_method;
pub mod connect_picker;
pub mod doctor;
pub mod github_deploy;
pub mod help;
pub mod hooks;
pub mod mcp;
pub mod memory;
pub mod model;
pub mod permissions;
pub mod repl;
pub mod resume;
pub mod scroll;
pub mod settings;
pub mod skills;
pub mod stats;
pub mod theme;
pub mod transcript;

/// Which full-page screen currently overlays the REPL. `None` ⇒ REPL is live.
/// Established by M7-11; M7-12/13/14 add `Resume`/`Settings`/`Memory`.
///
/// (M7-11 review) Each variant CARRIES its own per-screen state inline (the
/// data lives in the variant, not in a parallel `AppState` field). This makes
/// the foundation a clean "add a variant carrying its state + a render arm +
/// (optional) a key arm" for M7-12/13/14, and lets `AppState::close_screen`
/// stay a single generic `active_screen = None` with no per-screen clear.
/// Carrying the (non-`Copy`) `DoctorDiagnostics` drops `Screen: Copy`; the
/// ≤2 live match sites borrow the variant (`match &st.active_screen`).
///
/// (M7-13) `Eq` is dropped (kept `PartialEq`): the `Settings` variant carries a
/// `SettingsData` snapshot whose `StatusSnapshot`/`CostSnapshot` hold `f64`
/// cost fields, which do not implement `Eq`. No code uses `Screen` as a
/// `HashSet`/`HashMap` key, so `Eq` is unused; the existing `assert_eq!` /
/// `matches!` sites need only `PartialEq` + `Debug`.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// The diagnostic screen opened by `/doctor`, carrying its captured
    /// diagnostics.
    Doctor(doctor::DoctorDiagnostics),
    /// (M7-12) The Resume picker, carrying its own rows + selection state.
    /// The FIRST screen with interactive keys (Up/Down select, Enter resume):
    /// `root::handle_screen_key` dispatches per-variant so this arm runs the
    /// pure `resume::handle_resume_key` while Doctor stays read-only.
    Resume(resume::ResumeState),
    /// (M7-13) The Settings screen — a Config/Settings/Status/Usage tab
    /// overlay carrying its tab + read-once data snapshot. Interactive like
    /// Resume: `root::handle_screen_key` runs the pure `settings::apply_settings_key`
    /// (Left/Right/Tab cycle, Esc/`q` close). Surface-only — reads real M3
    /// settings + status/cost, writes ONLY via the `edit_config_file` handoff
    /// (§4 R7 — no inline mutation).
    Settings(settings::SettingsState),
    /// (M7-14) The Memory file editor — pick a LINGXI.md tier, edit it
    /// inline, save through the M3 store. Carries its own selector/edit
    /// state. Interactive like Resume/Settings: `root::handle_screen_key`
    /// runs the pure `memory::handle_memory_key` (↑/↓ select, Enter edit,
    /// Ctrl-S save, Esc back/close). Reads via the M3 loader; writes back
    /// to the SAME `HierarchyEntry.path` atomically (§4 R7 — no new
    /// persistence layer).
    Memory(memory::MemoryScreenState),
    /// (M7-15) The theme picker — claude-code `ThemePicker.tsx`. Carries its
    /// own highlight + restore-on-cancel state. Interactive like
    /// Resume/Settings/Memory: `root::handle_screen_key` runs the pure
    /// `theme::theme_picker_handle_key` (↑/↓ live-preview, Enter commit + persist,
    /// Esc/`q` cancel-restore). Up/Down LIVE-PREVIEW the highlighted theme by
    /// writing `AppState.theme` directly (the whole UI re-renders); Enter
    /// commits via `set_theme` + best-effort persist; Esc restores the prior
    /// setting. Persists through the existing `~/.lingxi/settings.json` `theme`
    /// field (§4 R7 — best-effort, session-only on failure).
    Theme(theme::ThemePickerState),
    /// The `/help` keyboard-shortcuts + slash-command viewer — claude-code
    /// `HelpV2`. A read-only, scrollable screen carrying an embedded
    /// `ScrollState` over the static Shortcuts + Slash-commands body.
    /// Interactive like Skills/Stats (read-only): `root::handle_screen_key`
    /// runs the pure `help::handle_help_key` (scroll keys via the embedded
    /// `ScrollState`; Esc/`q` close). Opened SYNCHRONOUSLY by `/help` (the
    /// content is static — no fs walk or handle call), mirroring the `/tasks`
    /// inline open. The `crates/commands` `HelpHandler` stays the `--no-tui`
    /// text path. PARITY-GAP: claude-code's multi-tab `HelpV2` is flattened to
    /// one scrollable screen here (custom-commands / [ant-only] tabs omitted),
    /// and the displayed chords are static defaults (no `useShortcutDisplay`
    /// user-keybinding seam yet).
    Help(help::HelpState),
    /// (M9-05) The background-tasks dialog — claude-code
    /// `BackgroundTasksDialog.tsx`. Carries its own list↔detail state
    /// (selection + mode + the open task's output tail). Interactive like
    /// Resume/Settings/Memory/Theme: `root::handle_screen_key` runs the pure
    /// `background_tasks::handle_background_tasks_key` (↑/↓ move, Enter open
    /// detail, Esc/`q` close; in detail Esc/`←` returns to the list). Opened
    /// from normal editing by Shift+Down; the task list it browses lives in
    /// `AppState.multiagent.tasks` (driven by the M9-05 `MultiAgent` pump).
    BackgroundTasks(background_tasks::BackgroundTasksState),
    /// (M9-08) The agent-discovery screen — claude-code `AgentsList.tsx` +
    /// `AgentDetail.tsx`. Carries its own list↔detail state (selection + mode
    /// + the agent catalog rows). Interactive like `BackgroundTasks`:
    ///   `root::handle_screen_key` runs the pure `agents::handle_agents_key`
    ///   (↑/↓ move, Enter open detail, Esc/`q` close; in detail Esc/`←`/`q`
    ///   returns to the list). Opened by `/agents`; rows come from
    ///   `OrchestratorHandle::list_agents` (`name/description/tools_allowed`).
    Agents(agents::AgentsScreenState),
    /// (M9-09) The `/skills` registry viewer — claude-code `SkillsMenu.tsx`. A
    /// read-only, scrollable list of discovered skills grouped by source.
    /// Carries its grouped sections + an embedded `ScrollState`. Interactive
    /// like Agents (read-only): `root::handle_screen_key` runs the pure
    /// `skills::handle_skills_key` (scroll keys via the embedded `ScrollState`;
    /// Esc/`q` close). Opened synchronously by `/skills`. The skill catalog is
    /// in-tree (`skill-api`) but not yet reachable from the TUI (the frozen
    /// `OrchestratorHandle` exposes no `list_skills` and `AppState` holds no
    /// `SkillRegistry`), so the open passes an EMPTY catalog — the locked
    /// `No skills found` empty state — until a `list_skills` handle method
    /// lands (out of this batch's scope).
    Skills(skills::SkillsState),
    /// (M9-10) The `/stats` usage-stats screen — claude-code `Stats.tsx`. A
    /// two-tab (`Overview` / `Models`) overlay over the in-tree aggregation of
    /// the `*.jsonl` session transcripts (sparkline tokens-per-day + activity
    /// heatmap). Carries its aggregated `StatsData` + active `StatsTab` + an
    /// embedded `ScrollState`. Interactive like Skills (read-only):
    /// `root::handle_screen_key` runs the pure `stats::handle_stats_key`
    /// (Tab/Shift-Tab switch tab, scroll keys via the embedded `ScrollState`,
    /// Esc/`q` close). Opened by `/stats` via the async `pump_open_stats` (the
    /// fs walk over `<lingxi_home>/projects/` runs OUTSIDE the `AppState` lock).
    /// `StatsData` carries only integer/string fields (no `f64`), so the
    /// `Screen: PartialEq` bound is satisfiable.
    Stats(stats::StatsState),
    /// The `/mcp` server viewer — claude-code `MCPSettings`. A read-only
    /// list↔detail of the configured MCP servers (name · status · transport).
    /// Interactive like Agents: `root::handle_screen_key` runs the pure
    /// `mcp::handle_mcp_key` (↑/↓ move, Enter detail, Esc/`q` close; in detail
    /// Esc/`←`/`q` returns to the list). Opened by `/mcp` via the async
    /// `pump_open_mcp` over the real `OrchestratorHandle::list_mcp_servers`. The
    /// MANAGEMENT actions (connect/reconnect/auth/toggle) of claude-code's full
    /// `/mcp` are deferred (need connection/auth seams).
    Mcp(mcp::McpScreenState),
    /// The `/hooks` viewer — claude-code `HooksConfigMenu`. A read-only
    /// list↔detail of the configured hooks (name · event; matcher + timeout in
    /// detail). Interactive like Agents/Mcp: `root::handle_screen_key` runs the
    /// pure `hooks::handle_hooks_key`. Opened by `/hooks` via the async
    /// `pump_open_hooks` over the real `OrchestratorHandle::list_hooks`. The
    /// CONFIGURATION (add/edit/remove → settings write) of claude-code's full
    /// `/hooks` is deferred (needs a settings-write seam).
    Hooks(hooks::HooksScreenState),
    /// The `/model` picker — claude-code `ModelPicker`. A single-select list of
    /// the available models (current marked `(current)`, pre-highlighted).
    /// Interactive: `root::handle_screen_key` runs the pure `model::handle_model_key`
    /// (↑/↓ move, Enter commit, Esc/`q` cancel). Opened by `/model` via the async
    /// `pump_open_model` over `OrchestratorHandle::list_available_models`; on
    /// commit the runner raises `AppState.pending_switch_model`, and
    /// `root::pump_switch_model` performs the async `switch_model` write + updates
    /// the status-line model. Unlike `theme`, there is NO live preview (the
    /// switch is an async write, not a sync palette swap).
    Model(model::ModelScreenState),
    /// The `/permissions` viewer — claude-code `commands/permissions`. A
    /// read-only list↔detail of the configured permission rules (behavior ·
    /// rule; source in the detail) plus the active permission mode. Interactive
    /// like Hooks/Mcp: `root::handle_screen_key` runs the pure
    /// `permissions::handle_permissions_key`. Opened by `/permissions` via the
    /// async OFF-DISK `pump_open_permissions` (reads the three settings tiers —
    /// the same files the enforcement loader + 3c persistence use). The
    /// interactive MANAGER (add/remove rule, switch mode → settings write) of
    /// claude-code's full `/permissions` is deferred (the 3c persist mechanism
    /// exists; wiring an add/remove UI onto it is a follow-up).
    Permissions(permissions::PermissionsScreenState),
    /// The `/connect <provider>` interactive credential screen (Plan 3c §6.3).
    /// Opened by the picker's `ModelOutcome::Connect` (via `pump_open_connect`)
    /// or a `/connect <provider>` prompt intercept. Two flows over one pure
    /// reducer (`connect::handle_connect_key`): a masked API-key field (Enter
    /// raises `AppState.pending_store_key` → `root::pump_store_provider_key`
    /// persists it through `CredentialManager::set_provider_key`), and the
    /// GitHub Copilot device-flow (the host drives `CopilotLogin`; this screen
    /// renders the code + spinner). Esc cancels either flow.
    Connect(connect::ConnectScreenState),
    /// The bare-`/connect` provider PICKER — an opencode-style grouped,
    /// searchable list of the connectable LLM providers (modeled on the
    /// `/model` picker). Interactive: `root::handle_screen_key` runs the pure
    /// `connect_picker::handle_connect_picker_key` (↑/↓ move, type-to-search,
    /// Enter select, Esc cancel). Opened SYNCHRONOUSLY by a bare `/connect`
    /// (no arg) in the app.rs dispatch intercept (the catalog is static — no
    /// async fetch). On `Select { provider_id }` the runner raises
    /// `AppState.pending_connect` + closes; `root::pump_open_connect` then
    /// opens the EXISTING key-entry `Connect` screen for that provider.
    ConnectPicker(connect_picker::ConnectPickerState),
    /// (GitHub Copilot Enterprise) The deployment-type sub-flow shown when
    /// connecting GitHub Copilot: pick GitHub.com Public vs GitHub Enterprise
    /// (then enter the host). Resolving opens the device-flow `Connect` screen
    /// with the chosen domain (`AppState.copilot_login_domain`).
    GithubDeployment(github_deploy::GithubDeploymentState),
    /// (T2b) The login-METHOD choice shown for multi-method providers (Anthropic:
    /// Pro/Max OAuth vs API key). `handle_screen_key` runs the pure
    /// `connect_method::handle_connect_method_key`; the pick opens the chosen
    /// `ConnectFlow`. Opened by `pump_open_connect` when `provider_methods` > 1.
    ConnectMethod(connect_method::ConnectMethodState),
    /// (RRS-06) The Ctrl+O transcript toggle — claude-code `app:toggleTranscript`.
    /// A read-only, scrollable verbose dump of the FULL message log (every
    /// message, not the live-REPL folded/capped view), captured at open time.
    /// Interactive like Help/Skills/Stats (read-only): `root::handle_screen_key`
    /// runs the pure `transcript::handle_transcript_key` (scroll keys via the
    /// embedded `ScrollState`; Ctrl+O toggles off / Esc closes). Opened by the
    /// Ctrl+O open-binding in `handle_live_key`; this makes the compact-boundary's
    /// `(ctrl+o for history)` hint functional. Reuses the same `render_transcript`
    /// formatter `/export` writes, so the on-screen dump and the exported `.txt`
    /// are byte-identical.
    Transcript(transcript::TranscriptScreenState),
}
