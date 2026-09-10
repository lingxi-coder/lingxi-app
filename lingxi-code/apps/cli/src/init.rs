//! Build the full orchestrator pipeline from an [`Argv`].
//!
//! **F2-01**: the ~270-line runtime wiring was lifted into the desktop
//! composition root — [`engine_desktop::build`] now assembles the orchestrator
//! pipeline from a deterministic [`engine_desktop::DesktopConfig`]. This module
//! keeps the *env/argv-reading* half: it resolves every `DesktopConfig` field
//! from `Argv` + `std::env` (the bridge-server fills the same config without
//! ever touching the process environment), then delegates the actual assembly.
//!
//! The wiring that now lives in `engine-desktop` (`apps/engine-desktop/src/lib.rs`):
//!
//! 1. `platform-posix` provides `HttpTransport` + `Clock` +
//!    `SecureStorage`.
//! 2. The llm-client transport bridge is built from `cfg.api_base`
//!    (default `https://api.anthropic.com`) + `cfg.api_key`.
//! 3. `llm_client::oauth::anthropic::ClaudeAiOAuthClient` wraps the credential manager so
//!    `/login` + `/logout` have a real handle.
//! 4. `orchestrator::ConversationOrchestrator` is constructed with
//!    `test_support` fillers for the hook/memory slots and — because the CLI
//!    sets `use_noop_permission_gate: true` — the always-allow
//!    `NoOpPermissionGate`.
//! 5. `command-api::RegistrySlashDispatcher` wraps the `CommandRegistry`
//!    populated through the desktop composition root.

use crate::argv::Argv;
use async_trait::async_trait;
use client_protocol::permission::PermissionRequest as PermissionRequestDto;
use command_api::RegistrySlashDispatcher;
use engine_desktop::{build, DesktopConfig};
use orchestrator::ConversationOrchestrator;
use platform_api::{AuthHandle, OrchestratorHandle, OutputStream};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Bundle of everything `run_cli` needs to drive a conversation.
pub struct Runtime {
    /// The fully-constructed orchestrator.
    pub orchestrator: Arc<ConversationOrchestrator>,
    /// Durable session coordinator retained across CLI runtime projection and
    /// remounts. Every host has one; the no-transcript mode gets a disposable
    /// ledger under a temporary home rather than no ledger.
    pub session_state: Arc<engine_desktop::session_state::SessionStateCoordinator>,
    /// Common Fusion terminal recorder retained by the projected runtime.
    pub fusion_recorder: Arc<dyn platform_api::FusionRunRecorder>,
    /// Per-session recorder factory used to drain mounted outboxes during CLI
    /// shutdown or an in-process session remount.
    pub fusion_recorder_factory:
        Arc<engine_desktop::fusion_recorder::DesktopFusionRecorderFactory>,
    /// Ordered producer/cost/session/outbox drain retained across the desktop
    /// runtime projection.
    pub session_lifecycle: Arc<engine_desktop::DesktopSessionLifecycle>,
    /// Slash-command dispatcher seeded with the 94 builtins + 18 wired core
    /// handlers (M5-09/M5-10/M5-11).
    pub dispatcher: RegistrySlashDispatcher,
    /// Auth handle for `/login` and `/logout`.
    pub auth: Arc<dyn AuthHandle>,
    /// (M9-05) The desktop task registry shared with the tool context. The TUI
    /// mount wraps it in a `PollerFeed` so the background-task footer + dialog
    /// read live state.
    pub task_registry: Arc<tasks::registry::TaskRegistry>,
    /// Event-driven workflow lifecycle/progress feed from the desktop
    /// composition root. Non-interactive hosts may drop it.
    pub workflow_events:
        Option<tokio::sync::mpsc::UnboundedReceiver<engine_desktop::DesktopWorkflowEvent>>,
    /// Live settings watcher firing `ConfigChange` hooks when settings files
    /// mutate on disk. Held here purely to keep the watcher alive for the
    /// session: dropping the `Runtime` (process teardown) aborts the watch
    /// tasks (RAII). If this field were dropped at `build_runtime` exit, the
    /// watcher would stop immediately after boot — so it must live on `Runtime`.
    pub settings_watcher: engine_desktop::settings_watch::SettingsWatcherHandle,
    /// Live file-changed watcher firing `FileChanged` hooks when a path resolved
    /// from a `FileChanged` hook's `matcher` mutates on disk. Held here purely to
    /// keep the watcher alive for the session: dropping the `Runtime` (process
    /// teardown) aborts the watch tasks (RAII). If dropped at `build_runtime`
    /// exit, the watcher would stop immediately after boot — so it lives on
    /// `Runtime`, exactly like `settings_watcher`.
    pub file_changed_watcher: engine_desktop::file_changed_watch::FileChangedWatcherHandle,
    /// (B4 Task 5) Shared subscription slot, projected straight from
    /// [`engine_desktop::DesktopRuntime::subscription`] (seeded at build,
    /// refined by the background profile+roles fetch). The TUI mount threads a
    /// clone into `tui::session::Runtime::with_subscription` so the rate-limit
    /// composer reads the live snapshot.
    pub subscription: platform_api::subscription::SharedSubscription,
    /// (`/sandbox`) Shared bash-sandbox toggle cell, projected straight from
    /// [`engine_desktop::DesktopRuntime::sandbox_toggle`] (the SAME
    /// `Arc<AtomicBool>` the bash tool reads). The TUI mount threads a clone
    /// into the widget so `/sandbox` flips sandboxing for the live session.
    pub sandbox_toggle: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// (`/sandbox` description) auto-allow / fallback flags projected from
    /// [`engine_desktop::DesktopRuntime`], rendered in the dynamic `/sandbox`
    /// popup description.
    pub sandbox_desc_auto_allow: bool,
    /// See [`Self::sandbox_desc_auto_allow`].
    pub sandbox_desc_fallback: bool,
    /// (`/sandbox` description) dependency-check status projected from
    /// [`engine_desktop::DesktopRuntime`]; `false` → the warning glyph.
    pub sandbox_desc_deps_ok: bool,
    /// (`/rewind`) Shared file-history checkpoint store, projected from
    /// [`engine_desktop::DesktopRuntime::file_history`]. The TUI mount builds the
    /// `/rewind` picker rows from it, and the restore path calls its
    /// `rewind_files` on a code rewind.
    pub file_history: std::sync::Arc<session::FileHistory>,
    /// (`/reload-plugins`) Retained plugin subsystem, projected from
    /// [`engine_desktop::DesktopRuntime::plugin_runtime`]. The TUI mount threads
    /// it into the `on_reload_plugins` effect so the interactive command applies
    /// pending enable/disable changes to the live session. `None` when plugins
    /// are disabled.
    pub plugin_runtime: Option<std::sync::Arc<engine_desktop::PluginRuntime>>,
    /// (Plan 3c §8) Per-provider availability map, projected straight from
    /// [`engine_desktop::DesktopRuntime::provider_availability`] (computed at
    /// `build()` from the LIVE multi-provider config). The TUI mount threads it
    /// into `tui::session::Runtime::with_provider_availability` so the `/model`
    /// picker can badge unconfigured providers. Empty keeps every row available.
    pub provider_availability: std::collections::BTreeMap<String, bool>,
    /// (T2a) Per-provider login-method tag map, projected straight from
    /// [`engine_desktop::DesktopRuntime::provider_auth_methods`] (derived at
    /// `build()` from the real catalog auth strategy: `"api_key"` /
    /// `"copilot_device"` / `"oauth"`). The TUI mount threads it into
    /// `tui::session::Runtime::with_provider_auth_methods` — it IS the data-driven
    /// `/connect` picker's provider SET, so an empty map yields an empty picker.
    pub provider_auth_methods: std::collections::BTreeMap<String, String>,
    /// (Plan 3c I1/I2) Authoritative `request_model -> (profile_name,
    /// provider_label)` map, projected from
    /// [`engine_desktop::DesktopRuntime::model_providers`]. The TUI mount threads
    /// it into `tui::session::Runtime::with_model_providers` so the `/model`
    /// picker can resolve a bare USER-provider model id to its own group +
    /// availability gate. Empty keeps the historical Built-in fallback.
    pub model_providers: std::collections::BTreeMap<String, (String, String)>,
    /// Provenance of the model selected by the desktop composition root for
    /// the initial/default `/model` row.
    pub model_provenance: platform_api::ModelProvenance,
    /// (Plan 3c C1) Shared engine credential store, projected straight from
    /// [`engine_desktop::DesktopRuntime::credentials`]. The TUI mount threads a
    /// clone into `tui::session::Runtime::with_provider_key_store` so the
    /// `/connect` screen's `pump_store_provider_key` persists a collected
    /// provider key via `CredentialManager::set_provider_key`.
    pub provider_key_store: std::sync::Arc<secret::CredentialManager>,
    /// Shared HTTP transport for TUI-owned `/web` test-search requests.
    pub http: std::sync::Arc<dyn platform_api::HttpTransport>,
    /// Shared analytics bus projected from the desktop runtime.
    pub analytics_bus: std::sync::Arc<telemetry::AnalyticsBus>,
    /// Structured-output capture slot, projected from
    /// [`engine_desktop::DesktopRuntime::structured_output_slot`]. `Some` only
    /// under `--json-schema`; the print path reads it after each turn to validate
    /// the model's `StructuredOutput` result against the schema and retry.
    pub structured_output_slot: Option<orchestrator::structured_output::StructuredOutputSlot>,
    /// (`!` bash mode) The sandboxed Bash runner, projected straight from
    /// [`engine_desktop::DesktopRuntime::bash_runner`]. Built over the SAME
    /// `BuiltinToolContext`/`BashTool` the model uses. The TUI mount threads a
    /// clone into `tui::session::Runtime::with_bash_runner` so a typed `!command`
    /// runs sandboxed and renders inline with no LLM turn.
    pub bash_runner: std::sync::Arc<dyn tui_core::bash_runner::BashRunner>,
    /// (#3 shell-expansion) The shared prompt shell-expansion provider, projected
    /// straight from [`engine_desktop::DesktopRuntime::shell_expansion`]. The TUI
    /// mount (`run_ratatui` → `run_app`) threads a clone into the `ChatWidget` so
    /// a typed `/commit` / `/commit-push-pr` / `/security-review` expands its
    /// embedded `!`git …`` bodies through the real host runner + policy-backed
    /// gate before submit — mirroring the dispatcher's expansion for non-TUI
    /// hosts. Built over the SAME `BuiltinToolContext` the model's Bash tool uses.
    pub shell_expansion: std::sync::Arc<dyn command_api::ShellExpansionProvider>,
    /// (`/connect` Copilot device-flow) GitHub-Copilot OAuth device-flow driver,
    /// projected straight from [`engine_desktop::DesktopRuntime::connect_copilot`].
    /// The TUI mount threads a clone into
    /// `tui::session::Runtime::with_copilot_connect_driver` so picking GitHub
    /// Copilot in `/connect` runs the real web sign-in (browser open + device
    /// poll + token store) instead of an inert key field.
    pub connect_copilot: std::sync::Arc<dyn command_core::CopilotConnectDriver>,
    /// (T2b) Unified OAuth sign-in driver, projected from
    /// [`engine_desktop::DesktopRuntime::oauth_connect_driver`]. The TUI mount
    /// threads a clone into `tui::session::Runtime::with_oauth_connect_driver` so
    /// picking an OAuth provider (Anthropic Pro/Max, OpenAI ChatGPT) in `/connect`
    /// runs the real browser sign-in instead of the inert `Unavailable` screen.
    pub oauth_connect_driver: std::sync::Arc<dyn command_core::OAuthConnectDriver>,
    /// (P1-08 runtime `/add-dir`) The SAME live `Arc<SessionCwd>` the file tools
    /// gate on, projected from [`engine_desktop::DesktopRuntime::session_cwd`].
    /// The `/add-dir` off-loop effect calls `add_trusted_dir(...)` on it so a
    /// directory added mid-session is immediately accessible to the file tools.
    pub session_cwd: std::sync::Arc<tool_api::SessionCwd>,
    /// (P1-08 runtime `/add-dir`) The live MCP registry, projected from
    /// [`engine_desktop::DesktopRuntime::mcp_registry`]. The `/add-dir` effect
    /// calls `add_root(...)` + `notify_roots_list_changed_all()` on it so every
    /// connected server's `roots/list` reflects the new working directory.
    pub mcp_registry: std::sync::Arc<mcp::McpRegistry>,
    /// Provider-neutral local IDE lifecycle handle projected from the desktop
    /// composition root. It owns lockfile discovery and local auth state.
    pub ide_handle: std::sync::Arc<dyn platform_api::IdeHandle>,
    /// The ENFORCING permission gate (`engine_desktop::DesktopRuntime::
    /// enforcing_permission_gate`), threaded to the TUI so Shift+Tab drives live
    /// permission-mode cycling via `set_permission_mode`.
    pub enforcing_permission_gate: Option<std::sync::Arc<dyn permission::gate::PermissionGate>>,
}

/// Build-result for the TUI startup path. (M6-03)
///
/// Returns the standard [`Runtime`] plus the bridge receiver the TUI
/// drains for streaming events. The orchestrator inside `runtime` is
/// constructed with a [`tui_core::orchestrator_bridge::BridgeOutputStream`] as its `output`,
/// so every `emit_text` / `emit_tool_call` / `emit_end_turn` lands on
/// `bridge_rx` as a [`tui_core::orchestrator_bridge::TurnEvent`].
pub struct TuiBuild {
    /// Standard runtime bundle.
    pub runtime: Runtime,
    /// The resolved boot permission mode (claude-code `initialPermissionModeFromCLI`)
    /// and whether bypass is an available Shift+Tab cycle target — seed the TUI's
    /// below-composer permission-mode indicator.
    pub initial_permission_mode: permission::PermissionMode,
    pub bypass_available: bool,
    /// Bridge receiver — the TUI render loop drains this into
    /// `tui::streaming::apply_event`.
    pub bridge_rx: tokio::sync::mpsc::UnboundedReceiver<tui_core::orchestrator_bridge::TurnEvent>,
    /// (MULTIMODAL.1) A clone of the bridge SENDER, handed to the TUI so its
    /// live-key turn-spawn pump (`tui::root::pump_turn`) can emit `TurnStarted`
    /// / `TurnEnded` on the SAME channel the orchestrator's `BridgeOutputStream`
    /// streams text/tool events onto. Both ends land on `bridge_rx`, so the
    /// spawned streaming turn's spinner + completion render through the one
    /// bridge pump. `UnboundedSender` is `Clone`, so cloning it here does not
    /// disturb the `BridgeOutputStream` that owns the original.
    pub turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
    /// Event-driven workflow lifecycle/progress feed. The TUI merges these
    /// updates into `turn_tx`; this is deliberately separate from the
    /// orchestrator output receiver so workflow pushes cannot be starved by a
    /// slow transcript renderer.
    pub workflow_events:
        Option<tokio::sync::mpsc::UnboundedReceiver<engine_desktop::DesktopWorkflowEvent>>,
    /// (TUI-PERM) Receiver for the injected `TuiPermissionGate`'s exchanges.
    /// Threaded into `session::Runtime::with_permission_rx` so the TUI's
    /// permission pump drives the interactive dialog.
    pub permission_rx: tokio::sync::mpsc::Receiver<tui_core::permission_bridge::PermissionExchange>,
    /// Receiver for interactive `AskUserQuestion` exchanges. The TUI drains
    /// this into the dedicated bottom-pane questionnaire view instead of
    /// routing those tools through the generic permission prompt.
    pub ask_user_question_rx:
        tokio::sync::mpsc::Receiver<tui_core::ask_user_question_bridge::AskUserQuestionExchange>,
    /// Receiver for interactive `computer` tool `request_access` exchanges.
    /// The TUI drains this into the dedicated approval bottom-pane view.
    pub computer_access_rx:
        tokio::sync::mpsc::Receiver<tui_core::computer_access_bridge::ComputerAccessExchange>,
    /// (/permissions) The gate's shared session-scoped allow-rule list. The
    /// `/permissions` editor pushes an ADDED allow rule here (in addition to
    /// the disk persist) so it takes effect THIS session — the same in-memory
    /// bucket the "always allow" dialog appends to
    /// (`TuiPermissionGate::session_allow_rules`). Deny/ask rules have no live
    /// bucket, so they are effective-next-load.
    pub session_allow_rules: std::sync::Arc<tokio::sync::Mutex<Vec<permission::PermissionRule>>>,
    /// (/permissions) The resolved settings-file roots for the interactive
    /// editor's disk writes (`permissions.{allow,ask,deny}` in
    /// user/project/local settings) and its snapshot preload — the SAME paths
    /// the gate's `AllowAlways` persist uses.
    pub permission_paths: permission::PermissionPaths,
    /// (companyAnnouncements) The merged `settings.companyAnnouncements` array
    /// (`None` unless configured). The composition root selects one (memoized)
    /// and renders it in the startup banner — CC's `LVs`/`oip()`.
    pub company_announcements: Option<Vec<String>>,
    /// Merged `settings.emojiCompletionEnabled`; absent resolves to `true` at
    /// the composition root and is applied to the mounted composer.
    pub emoji_completion_enabled: bool,
    /// Merged `settings.viewMode`; only `"focus"` currently alters the TUI
    /// boot state, but unknown values are preserved so the composition root can
    /// keep the settings reader aligned with the schema.
    pub startup_view_mode: Option<String>,
    /// Parsed `--settings` layer retained for TUI-owned command surfaces.
    pub flag_settings: Option<lingxi_core::settings::SettingsJson>,
    /// Exact user/project/local source gates for status-line provenance.
    pub status_line_source_scope: (bool, bool, bool),
}

/// Errors surfaced while building a [`Runtime`].
///
/// F2-01: the underlying assembly moved to [`engine_desktop::build`]; this enum
/// is a thin projection of [`engine_desktop::BuildError`] kept for source
/// compatibility with the CLI's existing call sites.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// API base URL resolution / construction failed.
    #[error("api base resolution failed: {0}")]
    ApiBase(String),
    /// Orchestrator construction failed (currently infallible).
    #[error("orchestrator construction failed: {0}")]
    Orchestrator(String),
    /// Durable session claim, hydration, or migration failed before startup.
    #[error("durable session state failed: {0}")]
    DurableSession(String),
    /// Secure-storage backend initialization failed.
    #[error("secure storage init failed: {0}")]
    SecureStorage(String),
    /// `sandbox.enabled` + `sandbox.failIfUnavailable` are both set but the
    /// sandbox cannot run on this host — faithful to claude-code's
    /// `isSandboxRequired()` startup refusal (sandbox-adapter.ts:479).
    #[error("sandbox required but unavailable: {0}")]
    SandboxUnavailable(String),
    /// (worktree-tmux-launch plan, Task 3) `-w`/`--worktree` was passed but
    /// the requested worktree could not be created — a hard boot failure
    /// since the user explicitly asked for an isolated worktree.
    #[error("--worktree launch failed: {0}")]
    WorktreeLaunch(String),
    /// (worktree-tmux-launch plan, Task 4) `--tmux` was passed without
    /// `-w`/`--worktree` — a hard boot failure (the flag's own doc: "requires
    /// --worktree").
    #[error("--tmux requires --worktree")]
    TmuxRequiresWorktree,
    /// (worktree-tmux mode pre-flight) bare `--tmux` (native) on Windows.
    #[error("--tmux is not supported on Windows")]
    TmuxNotSupportedOnWindows,
    /// (worktree-tmux mode pre-flight) bare `--tmux` (native) but tmux is not
    /// installed; payload is the platform-specific install hint.
    #[error("tmux is not installed.\n{0}")]
    TmuxNotInstalled(String),
    /// Custom Anthropic beta headers require an API-key-backed first-party
    /// Anthropic session; OAuth and alternate providers reject them.
    #[error("--betas requires an Anthropic API key and a first-party Anthropic model")]
    InvalidCustomBetas,
}

impl From<engine_desktop::BuildError> for InitError {
    fn from(e: engine_desktop::BuildError) -> Self {
        match e {
            engine_desktop::BuildError::ApiBase(m) => Self::ApiBase(m),
            engine_desktop::BuildError::Orchestrator(m) => Self::Orchestrator(m),
            engine_desktop::BuildError::DurableSession(m) => Self::DurableSession(m),
            engine_desktop::BuildError::SecureStorage(m) => Self::SecureStorage(m),
            engine_desktop::BuildError::SandboxUnavailable(m) => Self::SandboxUnavailable(m),
            engine_desktop::BuildError::WorktreeLaunch(m) => Self::WorktreeLaunch(m),
            engine_desktop::BuildError::TmuxRequiresWorktree => Self::TmuxRequiresWorktree,
            engine_desktop::BuildError::TmuxNotSupportedOnWindows => {
                Self::TmuxNotSupportedOnWindows
            }
            engine_desktop::BuildError::TmuxNotInstalled(m) => Self::TmuxNotInstalled(m),
            engine_desktop::BuildError::InvalidCustomBetas => Self::InvalidCustomBetas,
        }
    }
}

/// A no-op [`PermissionRequestSink`] for the CLI path.
///
/// The CLI builds with `use_noop_permission_gate: true`, so
/// [`engine_desktop::build`] binds the always-allow `NoOpPermissionGate` and
/// NEVER constructs an `AdapterPermissionGate` — the sink is therefore never
/// invoked. It exists only to satisfy `build`'s signature (the bridge-server
/// passes a real WS-backed sink instead).
pub(crate) struct NoopPermissionRequestSink;

#[async_trait]
impl client_adapter::PermissionRequestSink for NoopPermissionRequestSink {
    async fn emit_request(&self, _request: PermissionRequestDto) {
        // Unreachable on the CLI path (NoOpPermissionGate never emits).
        debug_assert!(false, "CLI uses NoOpPermissionGate — no request is emitted");
    }
}

/// Resolve the API base URL: honours `LINGXI_API_BASE_URL` (used by tests
/// to inject a mock), otherwise the canonical Anthropic endpoint.
#[must_use]
pub fn resolve_api_base() -> String {
    std::env::var("LINGXI_API_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

/// Which file setting-sources to load, from claude-code `--setting-sources
/// <user,project,local>`. `None` (flag absent) ⟶ ALL sources (the default:
/// both layers). A comma-separated list gates the user / project layers; the
/// `env` + `defaults` layers always apply.
///
/// NOTE: lingxi has no separate "local" (`settings.local.json`) layer; claude's
/// `local` source is mapped onto the project layer here (so `--setting-sources
/// local` still loads the project `.lingxi/settings.json`).
#[must_use]
pub(crate) fn setting_source_flags(setting_sources: Option<&str>) -> (bool, bool) {
    match setting_sources {
        None => (true, true),
        Some(s) => {
            let listed: Vec<String> = s
                .split(',')
                .map(|x| x.trim().to_ascii_lowercase())
                .collect();
            let include_user = listed.iter().any(|x| x == "user");
            // `local` has no distinct lingxi layer → fold onto `project`.
            let include_project = listed.iter().any(|x| x == "project" || x == "local");
            (include_user, include_project)
        }
    }
}

fn status_line_source_flags(setting_sources: Option<&str>) -> (bool, bool, bool) {
    let Some(setting_sources) = setting_sources else {
        return (true, true, true);
    };
    let listed = setting_sources
        .split(',')
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    (
        listed.iter().any(|source| source == "user"),
        listed.iter().any(|source| source == "project"),
        listed.iter().any(|source| source == "local"),
    )
}

/// Parse `--mcp-config <configs...>` entries into MCP server configs. Each
/// entry is either an existing JSON file path (read from disk) or an inline
/// JSON string; both accept the `{ "mcpServers": {...} }` envelope or a bare
/// server map (claude-code's `parsed.mcpServers || parsed`). Malformed entries
/// are reported to stderr and skipped (non-fatal). Returns `[]` when the flag
/// is absent.
#[must_use]
pub(crate) fn parse_cli_mcp_servers(entries: Option<&Vec<String>>) -> Vec<mcp::McpServerConfig> {
    let mut out = Vec::new();
    let Some(entries) = entries else { return out };
    for entry in entries {
        let content = if std::path::Path::new(entry).is_file() {
            match std::fs::read_to_string(entry) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("lingxi-cli: cannot read --mcp-config file {entry}: {e}");
                    continue;
                }
            }
        } else {
            entry.clone()
        };
        if content.trim().is_empty() {
            continue;
        }
        // CLI-provided servers carry Dynamic scope, matching the oracle's
        // `--mcp-config` handler, which stamps `{...ws, scope:"dynamic"}` on
        // every entry: they are never `.mcp.json` project-approval-gated
        // (`mcp::server_gate` only gates `ConfigScope::Project`). Precedence
        // over discovered servers is enforced by the name-merge in
        // `engine_desktop::build`, not the scope.
        match mcp::json_config::parse_mcp_json_string(&content, mcp::ConfigScope::Dynamic) {
            Ok(cfgs) => out.extend(cfgs.into_iter().map(|mut cfg| {
                cfg.metadata.cli_owned = true;
                cfg
            })),
            Err(e) => eprintln!("lingxi-cli: invalid --mcp-config entry: {e}"),
        }
    }
    out
}

/// Load merged settings once per `(cwd, include_user, include_project)` and
/// reuse the snapshot for every boot helper below.
fn load_scoped_settings(
    include_user: bool,
    include_project: bool,
) -> Option<lingxi_core::settings::EffectiveSettings> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    type Key = (std::path::PathBuf, bool, bool, u64);
    static CACHE: std::sync::Mutex<Option<(Key, lingxi_core::settings::EffectiveSettings)>> =
        std::sync::Mutex::new(None);
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let revision = settings_revision(&project_dir, include_user, include_project, &env);
    let key = (project_dir.clone(), include_user, include_project, revision);
    {
        let guard = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((cached_key, settings)) = guard.as_ref() {
            if *cached_key == key {
                return Some(settings.clone());
            }
        }
    }
    let inputs = lingxi_core::settings::LoadInputs {
        env: &env,
        project_dir: &project_dir,
        defaults: lingxi_core::settings::schema::SettingsJson::default(),
    };
    let loaded =
        lingxi_core::settings::Settings::load_scoped(inputs, include_user, include_project).ok()?;
    *CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((key, loaded.clone()));
    Some(loaded)
}

fn settings_revision(
    project_dir: &std::path::Path,
    include_user: bool,
    include_project: bool,
    env: &std::collections::BTreeMap<String, String>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    include_user.hash(&mut hasher);
    include_project.hash(&mut hasher);
    if include_user {
        settings_file_revision(
            lingxi_core::settings::loader::user_settings_path().as_deref(),
            &mut hasher,
        );
    }
    if include_project {
        settings_file_revision(
            Some(lingxi_core::settings::loader::project_settings_path(project_dir).as_path()),
            &mut hasher,
        );
        settings_file_revision(
            Some(lingxi_core::settings::loader::local_settings_path(project_dir).as_path()),
            &mut hasher,
        );
    }
    for (key, value) in env {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    hasher.finish()
}

fn settings_file_revision(path: Option<&std::path::Path>, hasher: &mut DefaultHasher) {
    let Some(path) = path else {
        0u8.hash(hasher);
        return;
    };
    path.hash(hasher);
    match std::fs::metadata(path) {
        Ok(metadata) => {
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
        Err(_) => 0u8.hash(hasher),
    }
}

/// Load the merged settings `providers` object, honoring `--setting-sources`.
///
/// Resolves the project dir from the *current* working directory — the process
/// has already `chdir`'d into any `--cwd` before `build_runtime` runs, so this
/// reads the same dir as the hook loader. Returns `None` on any load failure or
/// when no `providers` block is set; callers then fall back to built-in
/// profiles only.
fn load_provider_profiles(
    include_user: bool,
    include_project: bool,
) -> Option<std::collections::BTreeMap<String, serde_json::Value>> {
    load_scoped_settings(include_user, include_project).and_then(|eff| eff.settings.providers)
}

/// Load the merged `settings.axScreenReader` (project + user + env layers,
/// gated by `--setting-sources`) — the lowest-precedence source for the
/// [`crate::ax_screen_reader`] gate (below the env var and `--ax-screen-reader`
/// flag). `None` when unset / on any load failure, so the gate falls back to
/// its "off" default.
pub(crate) fn load_settings_ax_screen_reader(argv: &Argv) -> Option<bool> {
    if argv.restricted_enabled() {
        return parse_flag_settings(argv.settings.as_deref())
            .and_then(|settings| settings.ax_screen_reader);
    }
    let (include_user, include_project) = setting_source_flags(argv.setting_sources.as_deref());
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.ax_screen_reader)
}

/// Load the merged `settings.alwaysThinkingEnabled` (project + user + env
/// layers) — claude-code `qIe()`'s `if(e.alwaysThinkingEnabled===!1)return!1`.
/// Folded into the boot session `ThinkingConfig` by
/// [`llm_client::model::thinking::session_thinking_from_env`] (a `MAX_THINKING_TOKENS`
/// env var / `--max-thinking-tokens` flag budget pre-empts it). `None` when
/// unset / on any load failure → the resolver keeps thinking on (adaptive).
fn load_always_thinking_enabled(include_user: bool, include_project: bool) -> Option<bool> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.always_thinking_enabled)
}

/// Load the persisted `settings.model` (the `/model` picker writes it via
/// `tui_core::recent_models::record_default_model`, wired in `mode.rs`'s
/// `on_switch_model`). Used as the default model when
/// `--model` is absent, so the picker choice survives a restart. `None` when
/// unset / on any load failure → the caller keeps the built-in default.
fn load_settings_model(include_user: bool, include_project: bool) -> Option<String> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.model)
        .filter(|m| !m.trim().is_empty())
}

/// Load the merged `settings.plansDirectory` (project + user + env layers) —
/// the custom plan-file directory (206 `iT`). `None` when unset/blank; the
/// orchestrator resolves + containment-checks it against the project root.
fn load_settings_plans_directory(include_user: bool, include_project: bool) -> Option<String> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.plans_directory)
        .filter(|d| !d.trim().is_empty())
}

/// Load merged `settings.apiKeyHelper` (project + user + env layers), blank
/// filtered. The live credential provider invokes the helper only when no
/// higher-priority Anthropic credential was configured.
fn load_settings_api_key_helper(include_user: bool, include_project: bool) -> Option<String> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.api_key_helper)
        .filter(|d| !d.trim().is_empty())
}

/// Load the merged `settings.companyAnnouncements` (project + user + env
/// layers) — the startup announcement strings CC shows via `LVs`/`oip()`.
/// `None` when unset; the TUI composition root selects one (memoized) and
/// renders it in the startup banner.
fn load_settings_company_announcements(
    include_user: bool,
    include_project: bool,
) -> Option<Vec<String>> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.company_announcements)
}

/// Load the merged `settings.emojiCompletionEnabled`, honoring the same
/// `--setting-sources` scope as the rest of the interactive UI settings.
fn load_settings_emoji_completion_enabled(
    include_user: bool,
    include_project: bool,
) -> Option<bool> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.emoji_completion_enabled)
}

/// Load the merged `settings.viewMode`, honoring the same `--setting-sources`
/// scope as the rest of the interactive UI settings.
fn load_settings_view_mode(include_user: bool, include_project: bool) -> Option<String> {
    load_scoped_settings(include_user, include_project).and_then(|eff| eff.settings.view_mode)
}

/// Load the merged `settings.claudeMdExcludes` (project + user + env layers) —
/// glob patterns / absolute paths of `LINGXI.md` files to exclude from the
/// system prompt (claude-code `isLingxiMdExcluded`). Empty when unset.
pub(crate) fn load_lingxi_md_excludes(include_user: bool, include_project: bool) -> Vec<String> {
    load_scoped_settings(include_user, include_project)
        .and_then(|eff| eff.settings.lingxi_md_excludes)
        .unwrap_or_default()
}

/// Load the merged settings `routing` object (project + user + env layers).
///
/// Mirrors [`load_provider_profiles`] but reads the `routing` field. Returns
/// `None` on any load failure or when no `routing` block is set; callers then
/// fall back to the default (empty) routing config.
fn load_routing(include_user: bool, include_project: bool) -> Option<serde_json::Value> {
    load_scoped_settings(include_user, include_project).and_then(|eff| eff.settings.routing)
}

/// Resolve a deterministic [`DesktopConfig`] from `Argv` + `std::env`.
///
/// **F2-01**: this is the *only* half of the old `build_runtime` that remains
/// in `apps/cli` — every env / argv / `dirs` read that the engine-tier
/// `engine_desktop::build` must NOT perform (so the bridge-server can build an
/// identical runtime without touching the process environment). Each field
/// mirrors the concrete read the pre-lift `build_runtime` made:
///
/// - `api_base` ← `resolve_api_base()` (env `LINGXI_API_BASE_URL`).
/// - `api_key` ← env `ANTHROPIC_API_KEY` (empty string is valid).
/// - `cwd` ← `std::env::current_dir()` (the process has already `chdir`'d into
///   any `--cwd`).
/// - `lingxi_home` ← config-home (`$LINGXI_CONFIG_DIR` else `~/.claude`, via
///   `run::lingxi_home_dir`) — the hook / agents / settings loader root.
/// - `default_model` ← `Argv::model`, else the desktop default.
/// - `fallback_model` ← `Argv::fallback_model`, but ONLY in `--print` mode
///   (claude-code restricts `--fallback-model` to non-interactive runs); the
///   interactive TUI path resolves it to `None`.
/// - `provider_profiles` ← settings `providers` block (`load_provider_profiles`).
/// - `routing` ← settings `routing` block (`load_routing`).
/// - `mcp_paths` ← `[<cwd>/.mcp.json, ~/.lingxi.json]` (project `.mcp.json`
///   preferred over the user/global `mcpServers` inside `~/.lingxi.json`, matching
///   claude-code's user/project MCP scopes).
/// - `use_noop_permission_gate` ← `true` (the CLI always binds the always-allow
///   `NoOpPermissionGate`; a transport binds `AdapterPermissionGate`).
/// - `permission_mode` ← the CLI-resolved session mode threaded in by `run_cli`
///   (`initialPermissionModeFromCLI`), replacing the previously hardwired
///   `Default`.
#[must_use]
pub(crate) fn resolve_desktop_config(
    argv: &Argv,
    permission_mode: permission::PermissionMode,
) -> DesktopConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let lingxi_home = crate::run::lingxi_home_dir();
    let restricted = argv.restricted_enabled();
    if let Some(settings) = argv.settings.as_deref() {
        let _ = mcp::enterprise_policy::install_flag_settings_env(flag_settings_env(settings));
    }
    let mut project_mcp_path = cwd.join(".mcp.json");
    // User/global-scope MCP servers live INSIDE `~/.lingxi.json` (top-level
    // `mcpServers`), exactly like claude-code — NOT a standalone file under the
    // OS config dir. The loader reads only that key
    // (`mcp::parse_global_config_mcp_servers`).
    let mut global_mcp_path = migrations::global_config::global_config_path()
        .unwrap_or_else(|| std::path::PathBuf::from("/dev/null"));

    // (M3 cc2.1.198) `--safe-mode` / `--bare` customization gates. Mirrors the
    // binary's `Ql()`/`xd()` predicates: flag OR truthy env (`run_cli` exports
    // the env for children; a pre-set env also activates the mode, e.g. a
    // subagent inheriting `CLAUDE_CODE_SAFE_MODE`).
    let gates = engine_desktop::CustomizationGates {
        safe_mode: argv.safe_mode
            || platform_api::env::is_env_truthy(std::env::var("LINGXI_SAFE_MODE").ok().as_deref()),
        bare: argv.bare
            || platform_api::env::is_env_truthy(std::env::var("LINGXI_SIMPLE").ok().as_deref()),
    };

    // `--strict-mcp-config` (claude-code main.tsx:1586): "Only use MCP servers
    // from --mcp-config, ignoring all other MCP configurations." Null the
    // discovered project/global `.mcp.json` paths so `build()` loads NO ambient
    // servers; the `--mcp-config` servers (parsed into `cli_mcp_servers` below
    // and merged in `build()`) become the only source.
    //
    // (M3 cc2.1.198) `--safe-mode` nulls the SAME discovered paths (binary `fQ`:
    // `if(Hc("mcpAutoDiscovered"))return{servers:L2(),…}` — only flag-supplied
    // servers survive, exactly the strict-mcp-config shape). `--bare` does NOT
    // (`V5d.mcpAutoDiscovered:!1`; its help never lists MCP among the skips).
    if argv.strict_mcp_config || gates.disables_mcp_discovery() {
        // Use a fresh path that cannot exist instead of `/dev/null`: the MCP
        // loader treats an existing empty file as malformed JSON and would
        // otherwise emit three spurious warnings in every strict child.
        let nonexistent =
            std::env::temp_dir().join(format!(".lingxi-no-mcp-config-{}", uuid::Uuid::new_v4()));
        project_mcp_path = nonexistent.clone();
        global_mcp_path = nonexistent;
    }

    // `--mcp-config <configs...>` (claude-code: "Load MCP servers from JSON files
    // or strings"): parse each entry — an existing file path is read; anything
    // else is treated as an inline JSON string — into server configs that
    // `build()` merges OVER the discovered servers (CLI wins on name collision).
    let cli_mcp_servers = parse_cli_mcp_servers(argv.mcp_config.as_ref());

    // `--setting-sources <user,project,local>`: gate which file setting layers
    // the provider/routing/claudeMdExcludes loaders read (env + defaults always
    // apply). `None` ⟶ all sources (default behavior).
    let (incl_user, incl_project) = if restricted {
        (false, false)
    } else {
        setting_source_flags(argv.setting_sources.as_deref())
    };
    let flag_settings = parse_flag_settings(argv.settings.as_deref());

    let mut default_model = DesktopConfig::default().default_model;
    // Persisted `model` from settings.json (written by the `/model` picker) so the
    // last choice survives a restart. `--model` still wins below.
    //
    // Precedence mirrors claude-code's D4 main-model resolution
    // (`e = r !== void 0 ? r : process.env.ANTHROPIC_MODEL ?? Mi()?.model`):
    // `--model` > `ANTHROPIC_MODEL` env > settings-derived model. The
    // settings-derived value already folds the `LINGXI_MODEL` settings-env layer
    // (kept — deliberate lingxi design); the literal `ANTHROPIC_MODEL` name is
    // added here because this repo keeps `ANTHROPIC_*` env names verbatim
    // (ANTHROPIC_API_KEY, ANTHROPIC_DEFAULT_*_MODEL) and CC honors it directly.
    if let Some(persisted) = load_settings_model(incl_user, incl_project) {
        default_model = persisted;
    }
    if let Some(flag_model) = flag_settings
        .as_ref()
        .and_then(|settings| settings.model.clone())
        .filter(|model| !model.trim().is_empty())
    {
        default_model = flag_model;
    }
    let mut default_model_env_pinned = false;
    if let Some(env_model) = std::env::var("ANTHROPIC_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
    {
        default_model = env_model;
        default_model_env_pinned = true;
    }
    if let Some(m) = &argv.model {
        default_model.clone_from(m);
    }
    // Opus-fallback: `--fallback-model` parses unconditionally (`argv.rs`) but
    // claude-code only HONORS it in `--print`/non-interactive mode
    // (`main.tsx:1000` "only works with --print"). Mirror that SOFT restriction
    // here — the interactive TUI path leaves it `None`, so the turn_loop's
    // 529-overload interception stays a no-op for interactive sessions.
    let fallback_model = if argv.print {
        argv.fallback_model.clone()
    } else {
        None
    };
    // `--max-turns` / `--max-budget-usd` are documented "only works with --print"
    // (`main.tsx`). Mirror that SOFT restriction: the interactive TUI/REPL path
    // leaves both unset, so the orchestrator's turn / cost caps stay inert outside
    // `--print` (the flags still PARSE regardless — honoring is the consumer's job).
    let max_turns = if argv.print { argv.max_turns } else { None };
    let max_budget_usd = if argv.print {
        argv.max_budget_usd
    } else {
        None
    };
    // `--json-schema` is structured-output, "only works with --print". Parse the
    // schema string to a JSON value (print-gated). An unparseable schema is
    // dropped → structured output simply does not activate (the turn runs normally).
    let json_schema = if argv.print {
        argv.json_schema
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    } else {
        None
    };
    // `--no-stream` is always honoured in the baseline pipeline (only the
    // batched constructor is wired). Read the flag to silence the unused-field
    // warning and preserve the pre-lift behavior.
    let _ = argv.no_stream;

    DesktopConfig {
        // Real CLI session: the machine's keychain and env ARE legitimate
        // credential sources here.
        isolated_credential_storage: false,
        credential_storage_policy: platform_api::CredentialStoragePolicy::NativePreferred,
        injected_plugin_secrets: std::collections::BTreeMap::new(),
        api_base: resolve_api_base(),
        api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        api_key_helper: if restricted {
            flag_settings
                .as_ref()
                .and_then(|settings| settings.api_key_helper.clone())
                .filter(|helper| !helper.trim().is_empty())
        } else {
            load_settings_api_key_helper(incl_user, incl_project)
        },
        // (M13) Host-launcher OAuth forcing is claude-code's `KWr()`
        // (@228931361) — a pure env predicate that `zb()` (@228933355)
        // evaluates at credential-resolution time, so the ENGINE reads it
        // there and every entrypoint agrees (incl. `mcp serve` /
        // `auto-mode-setup`, which never build a runtime through
        // `build_runtime_from_config`). Nothing on the CLI side declares it: a
        // managed `forceLoginMethod` policy is NOT that predicate and never
        // participates in credential precedence.
        managed_oauth_only: false,
        // (M13) FD-inherited key presence — claude-code's managed/remote
        // launcher contract. Env presence only; LingXi never reads the FD.
        anthropic_key_fd_present: std::env::var("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR")
            .is_ok_and(|v| !v.trim().is_empty()),
        cwd,
        lingxi_home,
        default_model,
        // Only a `--model` flag is an EXPLICIT choice; the persisted
        // `settings.model` and the built-in default remain eligible for the
        // engine's boot-time connected-provider fallback. (An `ANTHROPIC_MODEL`
        // env pin is exempted from the fallback via `default_model_env_pinned`
        // below — NOT by widening this flag-only field, which the `--agent`
        // model-override gate keys on as claude-code's `userSpecifiedModel`.)
        default_model_explicit: argv.model.is_some(),
        // Prior `/model` picks (settings `recentModels`, most-recent-first) —
        // the fallback's first-preference pass. Best-effort read; empty on any
        // failure. `recentModels` lives in the USER settings.json, so the read
        // honors the same `--setting-sources` gate as the sibling loaders.
        recent_models: if incl_user {
            tui_core::recent_models::load_recent_models()
                .into_iter()
                .map(|r| engine_desktop::RecentModelRef {
                    provider: r.provider_id,
                    model: r.request_model,
                })
                .collect()
        } else {
            Vec::new()
        },
        fallback_model,
        custom_betas: argv.betas.clone().unwrap_or_default(),
        flag_settings: flag_settings.clone(),
        provider_profiles: if restricted {
            flag_settings
                .as_ref()
                .and_then(|settings| settings.providers.clone())
        } else {
            load_provider_profiles(incl_user, incl_project)
        },
        routing: if restricted {
            flag_settings
                .as_ref()
                .and_then(|settings| settings.routing.clone())
        } else {
            load_routing(incl_user, incl_project)
        },
        mcp_paths: vec![project_mcp_path, global_mcp_path],
        use_noop_permission_gate: true,
        // HEADLESS deny-on-ask (claude-code `--print` parity): in `-p`/`--print`
        // mode there is no interactive prompt, so an unresolved `Ask` (a mutating
        // tool with no matching allow rule) is DENIED rather than silently allowed.
        // The interactive TUI (`build_runtime_for_tui`) now PROMPTS for an
        // unresolved ask via the injected `TuiPermissionGate`, so it leaves this
        // `false`. The `--no-tui` stdio REPL still routes through `build_runtime`
        // (no injected gate), so an unresolved ask there resolves via the
        // `NoOpPermissionGate` (allow) — a known limitation of the v0.6.0
        // fallback REPL, not the primary interactive surface.
        deny_unresolved_ask: argv.print
            || (restricted && !matches!(crate::mode::decide_mode(argv), crate::mode::Mode::Tui)),
        // Claude Code 2.1.245 `process.stdout.isTTY??!1`. Independent of
        // `--print`: `-p` in a terminal is both print and TTY.
        is_tty: {
            use std::io::IsTerminal;
            std::io::stdout().is_terminal()
        },
        // CLI `--max-turns` / `--max-budget-usd` → orchestrator caps in `build()`
        // (print-gated above; interactive sessions leave both unset).
        max_turns,
        // CLI `--plan-mode-instructions` (print-mode only): custom plan-mode
        // workflow body threaded to `OrchestratorConfig::plan_mode_instructions`.
        // `None` in interactive mode ⟶ the default 5-phase plan reminder.
        plan_mode_instructions: if argv.print {
            argv.plan_mode_instructions.clone()
        } else {
            None
        },
        // `settings.json` `plansDirectory` (206 `iT`): custom plan-file directory
        // threaded to `OrchestratorConfig::plans_directory`, resolved against the
        // project root with a within-root containment check by the orchestrator.
        plans_directory: if restricted {
            flag_settings
                .as_ref()
                .and_then(|settings| settings.plans_directory.clone())
                .filter(|dir| !dir.trim().is_empty())
        } else {
            load_settings_plans_directory(incl_user, incl_project)
        },
        max_budget_usd,
        // `--json-schema` structured output (print-gated above): `build()` forces
        // the StructuredOutput tool + surfaces a capture slot when this is `Some`.
        json_schema,
        // Interactive gate injected ONLY by `build_runtime_for_tui` (the TUI
        // path); this shared headless/REPL config has no interactive prompt.
        injected_permission_gate: None,
        // M10: the CLI does not start a coordinator session (threading this
        // from session metadata is a follow-up; the default is byte-identical
        // to the pre-M10 build).
        session_started_as_coordinator: false,
        // Production memory: load the real `<cwd>/LINGXI.md` +
        // `~/.lingxi/LINGXI.md` hierarchy into the system prompt (claude-code
        // parity), which also makes the session-start
        // `fire_instructions_loaded()` fire over those files. Tests inject a
        // controlled provider (or `None`); only this real-host path reads the FS.
        //
        // (M3 cc2.1.198) `--safe-mode` disables the hierarchy outright; `--bare`
        // disables it unless `--add-dir` supplies dirs (binary `eue()` passes
        // `{explicitlyRequested:cI().length>0}` where `cI()` is the add-dir
        // list). Safe mode ALSO exports `LINGXI_DISABLE_LINGXI_MDS=1` in
        // `run_cli` (the orchestrator-level kill-switch); this `None` makes the
        // gate hold without the env mutation too (env-free tests, bridge hosts).
        memory_provider: if gates
            .disables_claude_md(argv.add_dir.as_ref().is_some_and(|d| !d.is_empty()))
        {
            None
        } else {
            Some(orchestrator::prompt::real_provider_with_excludes(
                if restricted {
                    flag_settings
                        .as_ref()
                        .and_then(|settings| settings.lingxi_md_excludes.clone())
                        .unwrap_or_default()
                } else {
                    load_lingxi_md_excludes(incl_user, incl_project)
                },
            ))
        },
        // CLI-resolved session permission mode (`initialPermissionModeFromCLI`),
        // threaded in by `run_cli`.
        permission_mode,
        // Preserve the raw CLI request so the composition root can fold a
        // selected main-thread agent's frontmatter `permissionMode` with the
        // oracle precedence: explicit CLI override > agent frontmatter >
        // settings `defaultMode`.
        permission_mode_cli: argv.permission_mode.clone(),
        permission_mode_cli_explicit: argv.permission_mode.is_some()
            || argv.dangerously_skip_permissions,
        allow_dangerously_skip_permissions: argv.allow_dangerously_skip_permissions,
        // Plan 3c: the tui supplies the masked-key prompt via the credential
        // store + `pump_store_provider_key`, not this engine port — so the
        // engine `/connect` text-command path uses the headless no-op default.
        connect_prompt: None,
        // CLI `--system-prompt` / `--system-prompt-file` (print-mode only):
        // override the assembled system prompt for the session. `None` in
        // interactive mode so the LINGXI.md hierarchy prompt is used unchanged.
        system_prompt_override: if argv.print {
            argv.resolve_system_prompt()
        } else {
            None
        },
        // CLI `--append-system-prompt` / `--append-system-prompt-file`
        // (print-mode only): text appended to the assembled system prompt.
        // `None` in interactive mode.
        append_system_prompt: if argv.print {
            argv.resolve_append_system_prompt()
        } else {
            None
        },
        // CLI `--session-id <uuid>`: the host validated UUID-ness + the cross-flag
        // rules in `run_cli` BEFORE this runs, so by here `argv.session_id` is
        // either `None` or a valid UUID string. `build()` parses it into the
        // boot-canonical MAIN session id (else mints a fresh one).
        session_id_override: argv.session_id.clone(),
        session_writer_lease: None,
        // Populated by the resume/fork TUI constructor after it has inspected
        // the transcript's versioned `forkedFrom` metadata.
        parent_session_id: None,
        // CLI `--disable-slash-commands`: empties the command/skill registry.
        disable_slash_commands: argv.disable_slash_commands,
        // CLI `--add-dir <directories...>`: extra tool-access directories,
        // unioned into the permission working-dir set in `build()`.
        add_dir: argv.add_dir.clone().unwrap_or_default(),
        // CLI `--mcp-config <configs...>` servers (parsed above), merged over the
        // discovered servers in `build()`.
        cli_mcp_servers,
        // `--strict-mcp-config` proper: the discovered paths were already nulled
        // above; the flag ALSO gates the agent-frontmatter MCP merge in `build()`
        // (claude `FWt`: frontmatter servers skipped under strict mode unless the
        // agent came from `--agents`).
        strict_mcp_config: argv.strict_mcp_config,
        restricted,
        restricted_tools: argv.tools.clone(),
        // CLI `--exclude-dynamic-system-prompt-sections`: move per-machine env
        // sections out of the cacheable system prompt into the first user message.
        exclude_dynamic_system_prompt_sections: argv.exclude_dynamic_system_prompt_sections,
        // `--setting-sources` scope: also gate the engine-side hook + permission
        // tier loaders in `build()` (not just the provider/routing/claudeMdExcludes
        // loaders above), so `--setting-sources project` does NOT load user-level
        // hooks or permission rules. `(true, true)` when the flag is absent.
        setting_source_scope: (incl_user, incl_project),
        // (M3 cc2.1.198) `--safe-mode` / `--bare` gates, consumed at each
        // registration site in `build()` (settings hooks, plugins + plugin LSP,
        // skill/custom-command dirs, custom agents).
        customization_gates: gates,
        // (M3 cc2.1.198) `--no-session-persistence`: print-gated like the other
        // print-only flags (the binary hard-errors on non-print use; `run_cli`
        // already enforced that, this guard keeps direct/test callers faithful).
        // `false` ⟶ `build()` wires no session `JsonlWriter`.
        session_persistence: !(argv.print && argv.no_session_persistence),
        // (M4 cc2.1.198) `--agents <json>`: raw payload; `build()` parses with
        // the strict flag-record schema and merges (flagSettings precedence);
        // safe-mode ignore (warn) also lives in `build()` so bridge hosts get
        // the same gate.
        cli_agents_json: argv.agents.clone(),
        // (M4 cc2.1.198) `--agent <agent>`: resolved against the final catalog
        // in `build()` (`dts` lookup + miss warning); main-thread application
        // is a pending seam there.
        cli_agent: argv.agent.clone(),
        // (M4 cc2.1.198) `--plugin-dir <path>` (repeatable): session-only
        // plugins loaded via the inline-plugin path in `build()`.
        cli_plugin_dirs: argv.plugin_dir.clone(),
        // (M4 cc2.1.198) `--effort <level>`: the argParser-normalized level
        // (`u4i` port; an invalid value already warned on stderr in `run_cli`
        // and normalizes to `None` here) → main-loop `output_config.effort`.
        initial_effort: argv.normalized_effort().0,
        // `ANTHROPIC_MODEL` env pin (resolved above) — exempts the model from the
        // boot connected-provider fallback without counting as `userSpecifiedModel`.
        default_model_env_pinned,
        // Boot SESSION thinking config (claude-code `qIe()` + the `wn` arm):
        // `MAX_THINKING_TOKENS` env > `--max-thinking-tokens` flag >
        // `--thinking` > the `alwaysThinkingEnabled` setting. A fixed budget
        // pre-empts adaptive; `0`/`disabled` turns thinking off.
        session_thinking: llm_client::model::thinking::session_thinking_from_cli(
            argv.thinking.as_deref(),
            argv.max_thinking_tokens,
            if restricted {
                flag_settings
                    .as_ref()
                    .and_then(|settings| settings.always_thinking_enabled)
            } else {
                load_always_thinking_enabled(incl_user, incl_project)
            },
        ),
        // (worktree-tmux-launch plan, Task 3) `-w`/`--worktree [name]`:
        // `argv.worktree` is `None` when the flag is absent (inert boot),
        // `Some("")` for a bare `-w` (`build()` mints a random slug), or
        // `Some(name)` for an explicit name — threaded verbatim.
        worktree_launch: argv.worktree.clone(),
        // (worktree-tmux-launch plan, Task 4) `--tmux[=mode]`: `argv.tmux` is
        // `None` when the flag is absent (inert — no worktree tmux session),
        // `Some("")` for a bare `--tmux` (native mode sentinel), or
        // `Some(mode)` for `--tmux=classic` — threaded verbatim. `build()`
        // hard-errors if this is `Some` while `worktree_launch` is `None`
        // (the CLI's own `--tmux` doc: "requires --worktree").
        tmux_launch: argv.tmux.clone(),
        // (G05 2.1.212 `/fork` `vAd`) BgSessionForker seam: the composition-root
        // impl copies the live conversation into a NEW background session (mints a
        // uuid, SNAPSHOTs the transcript, dispatches a `Launch::Resume` daemon)
        // while the interactive session stays live. Anchored to the SAME
        // config-home + daemon runtime dir the `--bg` dispatch uses so the snapshot
        // write and the resume dispatch agree. Injected here into the orchestrator
        // (via `build()`), preserving the crate-layering rule (orchestrator never
        // depends on apps/cli).
        bg_session_forker: Some(std::sync::Arc::new(
            crate::bg_session_forker::CliBgSessionForker::new(
                crate::run::lingxi_home_dir(),
                crate::run::daemon_runtime_dir(),
                crate::background_launch::BackgroundLaunchOptions::from_argv(argv),
                permission_mode,
            ),
        )),
        ask_user_question_tx: None,
        computer_access_tx: None,
        session_agent_observer: None,
        // No device audio on the CLI/TUI path: audio is proxied to a connected
        // client, and this host has none. The `voice`/`speech` tools are
        // therefore not registered here at all.
        audio: None,
    }
    // NOTE: claude-code's `--add-dir` is "Additional directories to allow TOOL
    // ACCESS to" (NOT LINGXI.md search — an earlier comment here misread it). It
    // is now wired above into `DesktopConfig.add_dir`, which `engine_desktop::
    // build` unions into the permission policy's working-dir set (parity with a
    // settings `permissions.additionalDirectories` entry).
}

/// Read the `env` object from `--settings` (inline JSON or file path) for the
/// MCP deny-policy fallback tier. Only string values participate; terminal
/// color controls are intentionally excluded, matching managed/user parsing.
fn flag_settings_env(settings: &str) -> std::collections::BTreeMap<String, String> {
    let trimmed = settings.trim();
    let text = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        match std::fs::read_to_string(trimmed) {
            Ok(text) => text,
            Err(_) => return std::collections::BTreeMap::new(),
        }
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("env")
                .and_then(serde_json::Value::as_object)
                .cloned()
        })
        .map(|env| {
            env.into_iter()
                .filter(|(key, _)| key != "NO_COLOR" && key != "FORCE_COLOR")
                .filter_map(|(key, value)| Some((key, value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the CLI `--settings` layer once for the desktop engine's canonical
/// settings composition. Accepts the same inline-object-or-file shape as the
/// existing flagSettings consumers. Invalid input remains absent here; the CLI
/// validation/error surface continues to be owned by argument initialization.
pub(crate) fn parse_flag_settings(
    settings: Option<&str>,
) -> Option<lingxi_core::settings::SettingsJson> {
    let raw = settings?.trim();
    let text = if raw.starts_with('{') {
        raw.to_string()
    } else {
        std::fs::read_to_string(raw).ok()?
    };
    serde_json::from_str(&text).ok()
}

/// Build the full runtime from parsed argv + the chosen output stream.
///
/// `output` is the sink the orchestrator will push turn events to (plain
/// stdout or NDJSON, projected from `crate::output::OutputSink` through
/// `crate::output_adapter::SinkAdapter`). M5-12 Task 9 wires this end-to-end
/// so `--json` produces NDJSON `text` / `tool_call` / `turn_end` lines.
///
/// **F2-01**: the actual assembly is delegated to [`engine_desktop::build`].
/// This function only resolves a [`DesktopConfig`] from `Argv`/env
/// ([`resolve_desktop_config`]) and projects the returned
/// [`engine_desktop::DesktopRuntime`] into the CLI's [`Runtime`]. The CLI sets
/// `use_noop_permission_gate: true`, so the supplied [`NoopPermissionRequestSink`]
/// is never invoked (the no-op gate never emits a request) and the returned
/// `permission_gate` handle is always `None`.
pub async fn build_runtime(
    argv: &Argv,
    output: Arc<dyn OutputStream>,
    permission_mode: permission::PermissionMode,
) -> Result<Runtime, InitError> {
    let cfg = resolve_desktop_config(argv, permission_mode);
    let runtime = build_runtime_from_config(cfg, output).await?;
    auto_connect_ide_if_requested(argv, &runtime).await;
    Ok(runtime)
}

/// Shared engine assembly: build the runtime from an already-resolved
/// [`DesktopConfig`] + output sink. Lets the TUI path inject a permission gate
/// derived from the SAME `cfg` without resolving config twice.
pub async fn build_runtime_from_config(
    mut cfg: DesktopConfig,
    output: Arc<dyn OutputStream>,
) -> Result<Runtime, InitError> {
    crate::startup_trace::mark("runtime_build_start");
    // The CLI owns the process live-session registration, so consume one
    // construction-only writer claim before the engine opens its coordinator.
    // This keeps the claim shared by cost, ordinary transcript, and Fusion
    // publication instead of letting each layer reacquire the OS lock.
    if cfg.session_persistence && cfg.session_writer_lease.is_none() {
        let session_id = cfg
            .session_id_override
            .as_deref()
            .and_then(protocol::SessionId::parse_prefixed)
            .unwrap_or_else(protocol::SessionId::new);
        cfg.session_id_override = Some(session_id.as_uuid().to_string());
        let lease = platform_api::live_sessions::LiveSessionDir::at_live(
            cfg.lingxi_home.join("sessions"),
        )
            .claim_session_id(&session_id.to_string(), std::process::id())
            .map_err(|error| InitError::Orchestrator(format!(
                "session writer claim failed: {error}"
            )))?
            .into_shared();
        cfg.session_writer_lease = Some(lease);
    }
    let permission_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(NoopPermissionRequestSink);
    let rt = build(cfg, output, permission_sink).await?;
    crate::startup_trace::mark("runtime_build_end");
    // Boot-time connected-provider fallback notice: one stderr line, emitted at
    // the shared choke point every mode's runtime flows through — before the
    // interactive TUI enters the alt-screen, and off stdout so `--print`/
    // stream-json output stays parseable.
    if let Some(n) = &rt.default_model_fallback {
        eprintln!(
            "Note: default model {} is unavailable (its provider is not connected). \
             Using {} instead — run /model to change it, or /connect to reconnect the provider.",
            n.from, n.to
        );
    }
    Ok(Runtime {
        orchestrator: rt.orchestrator,
        session_state: rt.session_state,
        fusion_recorder: rt.fusion_recorder,
        fusion_recorder_factory: rt.fusion_recorder_factory,
        session_lifecycle: rt.session_lifecycle,
        dispatcher: rt.dispatcher,
        auth: rt.auth,
        enforcing_permission_gate: rt.enforcing_permission_gate,
        task_registry: rt.task_registry,
        workflow_events: rt.workflow_events,
        settings_watcher: rt.settings_watcher,
        file_changed_watcher: rt.file_changed_watcher,
        subscription: rt.subscription,
        sandbox_toggle: rt.sandbox_toggle,
        sandbox_desc_auto_allow: rt.sandbox_desc_auto_allow,
        sandbox_desc_fallback: rt.sandbox_desc_fallback,
        sandbox_desc_deps_ok: rt.sandbox_desc_deps_ok,
        file_history: rt.file_history,
        plugin_runtime: rt.plugin_runtime,
        provider_availability: rt.provider_availability,
        provider_auth_methods: rt.provider_auth_methods,
        model_providers: rt.model_providers,
        model_provenance: rt.model_provenance,
        provider_key_store: rt.credentials,
        http: rt.http,
        analytics_bus: rt.analytics_bus,
        structured_output_slot: rt.structured_output_slot,
        bash_runner: rt.bash_runner,
        shell_expansion: rt.shell_expansion,
        connect_copilot: rt.connect_copilot,
        oauth_connect_driver: rt.oauth_connect_driver,
        // P1-08 runtime `/add-dir` live-effect handles.
        session_cwd: rt.session_cwd,
        mcp_registry: rt.mcp_registry,
        ide_handle: rt.ide_handle,
    })
}

/// Apply the boot-time `--ide` policy to an already-built runtime.
///
/// Some callers must inject a permission gate before composing the runtime and
/// therefore cannot use [`build_runtime`]. Keeping this policy in one helper
/// ensures those paths still auto-connect exactly one valid local endpoint,
/// while zero/multiple endpoints remain a no-op.
pub(crate) async fn auto_connect_ide_if_requested(argv: &Argv, runtime: &Runtime) {
    if !argv.ide {
        return;
    }
    match runtime.orchestrator.ide_auto_connect_if_single().await {
        Ok(true) | Ok(false) => {}
        Err(error) => eprintln!("Warning: could not connect to local IDE: {error}"),
    }
}

/// TUI variant of [`build_runtime`]. (M6-03)
///
/// Constructs the orchestrator with [`tui_core::orchestrator_bridge::BridgeOutputStream`] as
/// its `output` so streaming `emit_text` calls route into the bridge
/// channel returned alongside the runtime. The TUI render loop drains
/// this channel through `tui::streaming::apply_event`.
///
/// (Task 8) The interactive TUI path now threads the CLI-resolved session
/// permission mode (`initialPermissionModeFromCLI` via
/// [`crate::resolve_permission_mode`]) into the orchestrator's
/// `DesktopConfig.permission_mode`, instead of the previously-hardwired
/// `Default`. The bypass-safety guard is NOT re-run here — `run_cli` already
/// ran it once before dispatch (a refusal exits before this fn is reached).
pub async fn build_runtime_for_tui(argv: &Argv) -> Result<TuiBuild, InitError> {
    build_runtime_for_tui_inner(argv, None).await
}

/// [`build_runtime_for_tui`] with an explicit resume session id. When `Some`,
/// the engine's JSONL writer is named `<resume_session_id>.jsonl` (via
/// `session_id_override`) and opened in APPEND mode, so a resumed turn continues
/// the SAME on-disk file the history was loaded from — instead of forking a
/// fresh-uuid file (which split the conversation across files sharing one
/// sessionId). The fresh-launch path passes `None` (unchanged).
pub async fn build_runtime_for_tui_inner(
    argv: &Argv,
    resume_session_id: Option<uuid::Uuid>,
) -> Result<TuiBuild, InitError> {
    build_runtime_for_tui_inner_with_parent(argv, resume_session_id, None).await
}

/// Resume/fork-aware variant that also supplies Anthropic request ancestry.
pub async fn build_runtime_for_tui_inner_with_parent(
    argv: &Argv,
    resume_session_id: Option<uuid::Uuid>,
    parent_session_id: Option<String>,
) -> Result<TuiBuild, InitError> {
    crate::startup_trace::mark("tui_runtime_build_start");
    let (bridge_tx, bridge_rx) = tokio::sync::mpsc::unbounded_channel();
    // (MULTIMODAL.1) Clone the sender BEFORE it is moved into the
    // `BridgeOutputStream` so the TUI's turn-spawn pump can emit
    // `TurnStarted`/`TurnEnded` on the same channel the orchestrator streams on.
    let turn_tx = bridge_tx.clone();
    let bridge_output = tui_core::orchestrator_bridge::BridgeOutputStream::new(bridge_tx);
    bridge_output.set_thinking_display(argv.thinking_display.as_deref());
    let bridge: Arc<dyn OutputStream> = Arc::new(bridge_output);
    // (Task 8) Thread the CLI-resolved mode through the interactive TUI path.
    // The guard already ran in `run_cli` (notice already printed there too), so
    // this drops the notice and takes only the mode.
    let (permission_mode, _notice) = crate::resolve_permission_mode(argv);

    // (TUI-PERM) Resolve config ONCE so the gate's persist paths come from the
    // SAME cfg the engine builds with (no double resolve, no lost paths).
    let mut cfg = resolve_desktop_config(argv, permission_mode);

    // RESUME: name the JSONL writer's file by the RESUMED session id so new turns
    // append to `<id>.jsonl` (the same file the history loaded from) rather than
    // forking a fresh-uuid file. The writer opens append-mode, and these session
    // files carry no meta header (each line is a message), so appending is safe.
    if let Some(id) = resume_session_id {
        cfg.session_id_override = Some(id.to_string());
    }
    cfg.parent_session_id = parent_session_id;

    // Interactive permission gate: an unresolved mutating `Ask` surfaces the
    // TUI dialog over this channel instead of auto-allowing. AllowAlways
    // persists to <cwd>/.lingxi/settings.local.json (via `.with_persist`).
    let (perm_tx, perm_rx) =
        tokio::sync::mpsc::channel::<tui_core::permission_bridge::PermissionExchange>(16);
    let (ask_user_question_tx, ask_user_question_rx) = tokio::sync::mpsc::channel::<
        tui_core::ask_user_question_bridge::AskUserQuestionExchange,
    >(16);
    cfg.ask_user_question_tx = Some(ask_user_question_tx);
    let (computer_access_tx, computer_access_rx) =
        tokio::sync::mpsc::channel::<tui_core::computer_access_bridge::ComputerAccessExchange>(16);
    cfg.computer_access_tx = Some(computer_access_tx);
    let session_allow_rules = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    // (/permissions) The interactive editor reuses BOTH the gate's live
    // allow-rule bucket (for this-session effect of an added allow rule) and
    // the same settings-file roots the gate persists to. Clone before the
    // originals move into the gate below.
    let editor_session_allow_rules = session_allow_rules.clone();
    let permission_paths = permission::PermissionPaths {
        lingxi_home: cfg.lingxi_home.clone(),
        cwd: cfg.cwd.clone(),
    };
    let gate = std::sync::Arc::new(
        tui::permission_gate::TuiPermissionGate::new(perm_tx, session_allow_rules)
            .with_persist(permission_paths.clone()),
    );
    cfg.injected_permission_gate =
        Some(gate as std::sync::Arc<dyn permission::gate::PermissionGate>);

    let flag_settings = cfg.flag_settings.clone();
    let mut runtime = build_runtime_from_config(cfg, bridge).await?;
    auto_connect_ide_if_requested(argv, &runtime).await;
    let workflow_events = runtime.workflow_events.take();
    crate::startup_trace::mark("tui_runtime_build_end");
    // (companyAnnouncements) Read the merged array honoring `--setting-sources`;
    // the composition root (`run_ratatui`) selects + renders it at startup.
    let (incl_user, incl_project) = if argv.restricted_enabled() {
        (false, false)
    } else {
        setting_source_flags(argv.setting_sources.as_deref())
    };
    let company_announcements = load_settings_company_announcements(incl_user, incl_project);
    let emoji_completion_enabled =
        load_settings_emoji_completion_enabled(incl_user, incl_project).unwrap_or(true);
    let startup_view_mode = load_settings_view_mode(incl_user, incl_project);
    Ok(TuiBuild {
        runtime,
        initial_permission_mode: permission_mode,
        bypass_available: !argv.restricted_enabled()
            && (argv.allow_dangerously_skip_permissions
                || permission_mode == permission::PermissionMode::BypassPermissions),
        bridge_rx,
        turn_tx,
        workflow_events,
        permission_rx: perm_rx,
        ask_user_question_rx,
        computer_access_rx,
        session_allow_rules: editor_session_allow_rules,
        permission_paths,
        company_announcements,
        emoji_completion_enabled,
        startup_view_mode,
        flag_settings,
        status_line_source_scope: status_line_source_flags(argv.setting_sources.as_deref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_source_flags_default_and_scoped() {
        // Flag absent ⟶ all sources (both layers).
        assert_eq!(setting_source_flags(None), (true, true));
        // Single source.
        assert_eq!(setting_source_flags(Some("user")), (true, false));
        assert_eq!(setting_source_flags(Some("project")), (false, true));
        // `local` folds onto the project layer (no distinct lingxi layer).
        assert_eq!(setting_source_flags(Some("local")), (false, true));
        // Combined + whitespace + case-insensitive.
        assert_eq!(setting_source_flags(Some(" User , Project ")), (true, true));
        assert_eq!(setting_source_flags(Some("project,local")), (false, true));
        // Unknown / empty ⟶ neither file layer (env + defaults still apply).
        assert_eq!(setting_source_flags(Some("bogus")), (false, false));
        assert_eq!(setting_source_flags(Some("")), (false, false));
    }

    /// §27a: the oracle's `--mcp-config` handler (`Tl = {...ws, scope:"dynamic"}`)
    /// stamps every explicit entry `dynamic`, never `project` — so an
    /// unapproved `--mcp-config` server in a fresh project (no
    /// `enabledMcpjsonServers` record at all) must connect on the first
    /// launch, not sit `ProjectPendingApproval`. Before the fix this parsed at
    /// `ConfigScope::Project` and `McpPolicyContext::decide` blocked it.
    #[test]
    fn parse_cli_mcp_servers_are_never_project_approval_gated() {
        let raw = r#"{"mcpServers":{"docs":{"command":"docs-server"}}}"#.to_string();
        let cfgs = parse_cli_mcp_servers(Some(&vec![raw]));
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].scope, mcp::ConfigScope::Dynamic);

        // A brand-new project's policy snapshot: no approval record for
        // "docs" at all (not approved, not rejected, `enableAll` unset).
        let policy = mcp::McpPolicyContext::default();
        assert_eq!(
            policy.decide(&cfgs[0]),
            mcp::McpServerDecision::Allow,
            "a --mcp-config server must never wait on the .mcp.json project \
             approval gate, which owns Project scope only"
        );
    }

    #[tokio::test]
    async fn build_runtime_for_tui_wires_permission_channel() {
        // Non-print (interactive) argv: the TUI path injects the gate + channel.
        let argv = Argv::default();
        let build = build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        // The receiver exists and is open (the gate holds the sender).
        // try_recv on an empty-but-open channel returns Empty, not Disconnected.
        let mut rx = build.permission_rx;
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "permission_rx must be wired + open (gate holds the sender)"
        );
        let mut ask_rx = build.ask_user_question_rx;
        assert!(
            matches!(
                ask_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "ask_user_question_rx must be wired + open (resolver holds the sender)"
        );
        let mut computer_access_rx = build.computer_access_rx;
        assert!(
            matches!(
                computer_access_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "computer_access_rx must be wired + open (resolver holds the sender)"
        );
    }

    #[tokio::test]
    async fn build_runtime_with_defaults() {
        let argv = Argv {
            prompt: Some("hi".into()),
            print: true,
            no_stream: true,
            ..Argv::default()
        };
        let output: Arc<dyn OutputStream> =
            Arc::new(orchestrator::test_support::MockOutputStream::new());
        let r = build_runtime(&argv, output, permission::PermissionMode::Default).await;
        let r = r.expect("build_runtime failed");
        // M6-06: cost tracker must be wired.
        assert!(
            r.orchestrator.has_cost_tracker(),
            "build_runtime did not wire CostTracker"
        );
        // M6-07: three registries must be wired.
        assert!(
            r.orchestrator.has_mcp_registry(),
            "build_runtime did not wire McpRegistry"
        );
        assert!(
            r.orchestrator.has_hook_registry(),
            "build_runtime did not wire HookRegistry"
        );
        assert!(
            r.orchestrator.has_agent_catalog(),
            "build_runtime did not wire agent catalog"
        );
        // M6-08: compactor must be wired.
        assert!(
            r.orchestrator.has_compaction(),
            "build_runtime did not wire CompactionOrchestrator"
        );
    }

    /// Opus-fallback hop: `--fallback-model` threads into
    /// `DesktopConfig.fallback_model` in `--print` mode, and is dropped (left
    /// `None`) in interactive mode — mirroring claude-code's "only works with
    /// --print" restriction.
    #[test]
    fn fallback_model_threads_through_in_print_mode() {
        let base = Argv {
            prompt: Some("hi".into()),
            print: true,
            fallback_model: Some("claude-opus-4-20250514".into()),
            no_stream: true,
            ..Argv::default()
        };

        // `--print` ⟶ honored.
        let cfg = resolve_desktop_config(&base, permission::PermissionMode::Default);
        assert_eq!(
            cfg.fallback_model.as_deref(),
            Some("claude-opus-4-20250514")
        );

        // Interactive (no `--print`) ⟶ dropped to `None`.
        let interactive = Argv {
            print: false,
            ..base.clone()
        };
        let cfg = resolve_desktop_config(&interactive, permission::PermissionMode::Default);
        assert!(cfg.fallback_model.is_none());

        // No flag at all ⟶ `None` even in print mode.
        let absent = Argv {
            fallback_model: None,
            ..base
        };
        let cfg = resolve_desktop_config(&absent, permission::PermissionMode::Default);
        assert!(cfg.fallback_model.is_none());
    }

    /// `--max-turns` / `--max-budget-usd` are "only works with --print" in
    /// claude-code: they thread into the config in print mode and are dropped to
    /// `None` (caps inert) in interactive mode — same soft restriction as
    /// `--fallback-model`.
    #[test]
    fn max_turns_and_budget_are_print_gated() {
        let printed = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--max-turns",
            "3",
            "--max-budget-usd",
            "1.5",
            "hi",
        ])
        .unwrap();
        let cfg = resolve_desktop_config(&printed, permission::PermissionMode::Default);
        assert_eq!(cfg.max_turns, Some(3));
        assert_eq!(cfg.max_budget_usd, Some(1.5));

        // Interactive (no `--print`) ⟶ both dropped to `None`.
        let interactive =
            Argv::from_iter(["lingxi-cli", "--max-turns", "3", "--max-budget-usd", "1.5"]).unwrap();
        let cfg = resolve_desktop_config(&interactive, permission::PermissionMode::Default);
        assert!(cfg.max_turns.is_none());
        assert!(cfg.max_budget_usd.is_none());
    }

    /// (M3 cc2.1.198) `--safe-mode` / `--bare` gating in `resolve_desktop_config`:
    /// gates thread into `DesktopConfig.customization_gates`; safe mode nulls the
    /// discovered `.mcp.json` paths (binary `fQ`: only flag-supplied servers
    /// survive) and drops the memory provider; bare keeps MCP discovery and only
    /// drops the memory provider when no `--add-dir` is given (`eue()`'s
    /// `explicitlyRequested:cI().length>0`).
    #[test]
    fn safe_mode_and_bare_gate_desktop_config() {
        // Baseline: no reduced mode — provider present, real MCP paths.
        let base = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&base, permission::PermissionMode::Default);
        assert!(!cfg.customization_gates.safe_mode);
        assert!(!cfg.customization_gates.bare);
        assert!(cfg.memory_provider.is_some());
        assert!(cfg.mcp_paths.iter().any(|p| p.ends_with(".mcp.json")));

        // Safe mode: gates set, memory off, discovered MCP paths nulled.
        let safe = Argv::from_iter(["lingxi-cli", "--safe-mode", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&safe, permission::PermissionMode::Default);
        assert!(cfg.customization_gates.safe_mode);
        assert!(
            cfg.memory_provider.is_none(),
            "safe mode disables LINGXI.md"
        );
        assert_eq!(cfg.mcp_paths[0], cfg.mcp_paths[1]);
        assert!(!cfg.mcp_paths[0].exists());
        assert!(cfg.mcp_paths[0]
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.starts_with(".lingxi-no-mcp-config-")));

        // Safe mode + --add-dir: NO escape (unlike bare).
        let safe_dir =
            Argv::from_iter(["lingxi-cli", "--safe-mode", "--add-dir", "/tmp", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&safe_dir, permission::PermissionMode::Default);
        assert!(
            cfg.memory_provider.is_none(),
            "--add-dir does not re-enable in safe mode"
        );

        // Bare: gates set, memory off, but MCP discovery KEPT (bare's help
        // never lists MCP among the skips; `V5d.mcpAutoDiscovered:!1`).
        let bare = Argv::from_iter(["lingxi-cli", "--bare", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&bare, permission::PermissionMode::Default);
        assert!(cfg.customization_gates.bare);
        assert!(
            cfg.memory_provider.is_none(),
            "bare skips CLAUDE.md auto-discovery"
        );
        assert!(cfg.mcp_paths.iter().any(|p| p.ends_with(".mcp.json")));

        // Bare + --add-dir: explicit request re-enables the memory hierarchy.
        let bare_dir =
            Argv::from_iter(["lingxi-cli", "--bare", "--add-dir", "/tmp", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&bare_dir, permission::PermissionMode::Default);
        assert!(
            cfg.memory_provider.is_some(),
            "--add-dir re-enables LINGXI.md in bare"
        );
    }

    /// (M3 cc2.1.198) `--no-session-persistence` threads into
    /// `DesktopConfig.session_persistence` print-gated (the binary hard-errors
    /// on non-print use; this guard keeps direct callers faithful too).
    #[test]
    fn no_session_persistence_is_print_gated() {
        let printed =
            Argv::from_iter(["lingxi-cli", "--print", "--no-session-persistence", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&printed, permission::PermissionMode::Default);
        assert!(!cfg.session_persistence);

        // Flag absent (print) ⟶ persist.
        let plain = Argv::from_iter(["lingxi-cli", "--print", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&plain, permission::PermissionMode::Default);
        assert!(cfg.session_persistence);

        // Interactive misuse never reaches here (run_cli hard-errors), but a
        // direct caller stays faithful: non-print keeps persistence on.
        let interactive =
            Argv::from_iter(["lingxi-cli", "--no-session-persistence", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&interactive, permission::PermissionMode::Default);
        assert!(cfg.session_persistence);
    }

    #[test]
    fn resolve_api_base_default() {
        // Snapshot the env first to avoid clobbering other tests.
        let prior = std::env::var("LINGXI_API_BASE_URL").ok();
        std::env::remove_var("LINGXI_API_BASE_URL");
        let base = resolve_api_base();
        assert_eq!(base, "https://api.anthropic.com");
        if let Some(v) = prior {
            std::env::set_var("LINGXI_API_BASE_URL", v);
        }
    }

    // ---- ANTHROPIC_MODEL env-pin resolution (claude-code D4) ----------------
    //
    // `resolve_desktop_config` reads the process-global `ANTHROPIC_MODEL`; the
    // tests serialize on one lock and restore the prior value.
    use std::sync::Mutex;
    static MODEL_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ModelEnvGuard {
        prev: Option<String>,
    }
    impl ModelEnvGuard {
        fn set(val: &str) -> Self {
            let prev = std::env::var("ANTHROPIC_MODEL").ok();
            std::env::set_var("ANTHROPIC_MODEL", val);
            Self { prev }
        }
        fn unset() -> Self {
            let prev = std::env::var("ANTHROPIC_MODEL").ok();
            std::env::remove_var("ANTHROPIC_MODEL");
            Self { prev }
        }
    }
    impl Drop for ModelEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("ANTHROPIC_MODEL", v),
                None => std::env::remove_var("ANTHROPIC_MODEL"),
            }
        }
    }

    #[test]
    fn anthropic_model_env_pins_default_model() {
        let _lock = MODEL_ENV_LOCK.lock().unwrap();
        // A non-empty ANTHROPIC_MODEL overrides the settings-derived / built-in
        // default (it is applied ABOVE load_settings_model), and marks the model
        // env-pinned so the boot connected-provider fallback is exempt.
        let _g = ModelEnvGuard::set("claude-opus-4-1-20250805");
        let argv = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&argv, permission::PermissionMode::Default);
        assert_eq!(cfg.default_model, "claude-opus-4-1-20250805");
        assert!(cfg.default_model_env_pinned);
        // Kept SEPARATE from `--model` explicitness (no `--model` flag here).
        assert!(!cfg.default_model_explicit);
    }

    #[test]
    fn model_flag_beats_anthropic_model_env() {
        let _lock = MODEL_ENV_LOCK.lock().unwrap();
        // claude-code D4: `--model` (`r`) wins over `process.env.ANTHROPIC_MODEL`.
        let _g = ModelEnvGuard::set("claude-opus-4-1-20250805");
        let argv =
            Argv::from_iter(["lingxi-cli", "--model", "claude-sonnet-4-5-20250929", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&argv, permission::PermissionMode::Default);
        assert_eq!(cfg.default_model, "claude-sonnet-4-5-20250929");
        // `--model` is the explicit user choice.
        assert!(cfg.default_model_explicit);
    }

    #[test]
    fn empty_or_blank_anthropic_model_env_ignored() {
        let _lock = MODEL_ENV_LOCK.lock().unwrap();
        // Empty string ⟶ not a pin (claude-code `?? Mi()?.model` falls through).
        let _g = ModelEnvGuard::set("");
        let argv = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&argv, permission::PermissionMode::Default);
        assert!(
            !cfg.default_model_env_pinned,
            "empty ANTHROPIC_MODEL must not pin the model"
        );
        // Whitespace-only is likewise ignored (trimmed-empty).
        let _g2 = ModelEnvGuard::set("   ");
        let cfg = resolve_desktop_config(&argv, permission::PermissionMode::Default);
        assert!(
            !cfg.default_model_env_pinned,
            "blank ANTHROPIC_MODEL must not pin the model"
        );
    }

    #[test]
    fn absent_anthropic_model_env_leaves_model_unpinned() {
        let _lock = MODEL_ENV_LOCK.lock().unwrap();
        let _g = ModelEnvGuard::unset();
        let argv = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        let cfg = resolve_desktop_config(&argv, permission::PermissionMode::Default);
        assert!(!cfg.default_model_env_pinned);
    }

    #[test]
    fn flag_settings_env_reads_only_string_values_and_filters_color_controls() {
        let env = flag_settings_env(
            r#"{"env":{"TOOL":"flag-tool","COUNT":3,"NO_COLOR":"1","FORCE_COLOR":"1"}}"#,
        );
        assert_eq!(env.get("TOOL").map(String::as_str), Some("flag-tool"));
        assert!(!env.contains_key("COUNT"));
        assert!(!env.contains_key("NO_COLOR"));
        assert!(!env.contains_key("FORCE_COLOR"));
    }

    #[test]
    fn parse_flag_settings_accepts_inline_json_and_file_paths() {
        let inline = parse_flag_settings(Some(r#"{"workflowSizeGuideline":"large"}"#)).unwrap();
        assert_eq!(inline.workflow_size_guideline.as_deref(), Some("large"));

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("flag-settings.json");
        std::fs::write(&path, r#"{"workflowSizeGuideline":"small"}"#).unwrap();
        let from_file = parse_flag_settings(path.to_str()).unwrap();
        assert_eq!(from_file.workflow_size_guideline.as_deref(), Some("small"));
    }

    #[test]
    fn parse_flag_settings_ignores_missing_or_invalid_input() {
        assert!(parse_flag_settings(None).is_none());
        assert!(parse_flag_settings(Some("{not-json")).is_none());
        assert!(parse_flag_settings(Some("/definitely/not/a/settings/file")).is_none());
    }
}
