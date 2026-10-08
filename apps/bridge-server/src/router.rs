//! F2-08 — Full command/event routing.
//!
//! The walking skeleton ([`crate::server::BridgeConnection`], F2-05/F2-06)
//! routes only the turn + permission path. This module is the engine-routing
//! seam for the FULL [`ClientCommand`] surface: model switch + listings + slash
//! commands + session control + the background-task poll loop.
//!
//! ## The routing seam (mirrors `TurnDriver`)
//!
//! [`CommandRouter`] is the abstraction the connection delegates every
//! non-turn/non-permission command to — exactly the way
//! [`crate::server::TurnDriver`] abstracts the turn entry. The production server
//! binds an [`EngineCommandRouter`] wrapping the real engine handles
//! ([`lingxi_core::host::OrchestratorHandle`], [`lingxi_core::host::AuthHandle`],
//! [`lingxi_core::host::task_registry::TaskRegistryHandle`], and the slash dispatcher); a
//! test binds the SAME router over the engine's `MockOrchestratorHandle` / mock
//! task + auth handles, so the routing-and-lowering path under test is the
//! production one (no test-only router shim).
//!
//! Each command maps to its engine entry, and the engine's reply is lowered to
//! a [`ClientEvent`] (via the pure `client::adapter::lowering` fns — the single
//! parity surface, §0.2) and pushed out through the connection's
//! [`ClientEventSink`]:
//!
//! | command | engine entry | reply event |
//! |---|---|---|
//! | `SetModel` | `OrchestratorHandle::switch_model` | `ModelChanged` |
//! | `SetPermissionMode` | `OrchestratorHandle::set_permission_mode` | `PermissionModeChanged` |
//! | `ListModels` / `RefreshListings{Models}` | `list_available_models` + status | `ModelList` |
//! | `RefreshListings{Mcp}` | `list_mcp_servers` | `McpServers` |
//! | `RefreshListings{Hooks}` | `list_hooks` | `Hooks` |
//! | `RefreshListings{Agents}` | `list_agents` | `Agents` |
//! | `RefreshListings{Status}` | `get_status_snapshot` | `StatusSnapshot` |
//! | `RefreshListings{Doctor}` | `run_doctor_checks` | `DoctorReport` |
//! | `RefreshListings{Auth}` / `Login` / `Logout` | `AuthHandle::*` | `AuthState` |
//! | `RefreshListings{Tasks}` / `TaskList` | `TaskRegistryHandle::list` | `TaskRow`× |
//! | `RunSlashCommand` | `SlashCommandDispatcher::dispatch` | `SlashCommandResult` or normal turn stream |
//! | `TaskOutput` | `TaskRegistryHandle::output` | `TaskOutputChunk` |
//! | `TaskStop` | `TaskRegistryHandle::kill` | `TaskStatusChanged` |
//! | `ForceCompact` | `force_compact` | `CompactionCompleted` |
//! | `ClearSession` | `clear_session` (REJECTED mid-turn) | `SessionEnded` / `Error` |
//! | `ListSessions` / `RefreshListings{Sessions}` | persisted JSONL catalog | `SessionList` |
//! | `NewSession` | `clear_session` + optional `switch_model` | `SessionStarted` / `Error` |
//! | `ResumeSession` | JSONL replay + `resume_session` | `SessionResumed` / `Error` |
//! | `RefreshListings{Settings}` | `settings_bridge::build_snapshot` | `SettingsSnapshot` |
//! | `UpdatePermissionRules` | `permission::persist_permission_rule_set` | `SettingsSnapshot` / `Error` |
//! | `SetDefaultPermissionMode` | `permission::persist_permission_mode` | `SettingsSnapshot` / `Error` |
//! | `UpdateWorkspaceDirectories` | `permission::persist_workspace_directories` | `SettingsSnapshot` / `Error` |
//! | `RequestExit` | `request_exit` | — |
//!
//! ## Mid-turn semantics
//!
//! Session mutations are **rejected while a turn is in flight** (plan §2): the
//! connection sets [`EngineCommandRouter::set_turn_active`] on `SendPrompt` and
//! clears it on turn end; a clear/new/resume arriving in that window is refused
//! with a [`ClientEvent::Error`] and never reaches the engine/store.
//!
//! ## Reserved / feed-deferred
//!
//! Per governing decisions §0.7/§0.9, the router NEVER live-sources the reserved
//! DTOs (`ThinkingDelta`, `UsageUpdate`, `CoordinatorStatus`); the corresponding
//! engine sources do not exist in the foundation, so no command maps to them.
//! The listing kind with no engine handle or host store in the foundation
//! (`Memory`) remains unrouted here. `Settings` IS routed: it reads the layered
//! settings files through [`crate::settings_bridge`] using the
//! [`crate::settings_bridge::SettingsContext`] the composition root supplies.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
pub(crate) use client::adapter::controls::decode_reasoning_selection;
use client::adapter::lowering::{
    lower_provider_model_catalog_entry, lower_status_snapshot, lower_task_output_chunk,
};
use client::adapter::ClientEventSink;
use client::protocol::commands::{ClientCommand, ListingKindDto};
use client::protocol::events::{ClientEvent, ErrorKindDto};
use client::protocol::listings::{AuthStateDto, SessionModeDto};
use command_api::model::CommandSource;
use command_api::parser::parse_slash_command;
use command_api::registry::CommandRegistry;
use lingxi_core::host::auth::{AuthHandle, LoginInfo};
use lingxi_core::host::orchestrator::HandleError;
use lingxi_core::host::orchestrator::OrchestratorHandle;
use lingxi_core::host::task_registry::{TaskListFilter, TaskRegistryHandle};
use lingxi_core::host::SlashCommandDispatcher;
use tokio::sync::RwLock;

use crate::mcp_bridge::McpPaths;
use crate::settings_bridge::SettingsContext;

/// Default number of recent sessions returned when `ListSessions` omits its
/// explicit limit. This matches the CLI `/resume` picker and the mobile host.
const DEFAULT_SESSION_LIST_LIMIT: usize = 5;

/// Inputs needed to enumerate and replay the persisted JSONL session store.
///
/// Kept optional on [`EngineCommandRouter`] so existing embedded/test callers
/// that only need model/auth/task routing remain source-compatible. Production
/// bridge boot always supplies this context.
pub struct SessionStoreContext {
    lingxi_home: PathBuf,
    session_cwd: String,
    fs: Arc<dyn lingxi_core::host::FileSystem>,
    transcript_writer: Option<Arc<session::jsonl::JsonlWriter>>,
}

impl SessionStoreContext {
    /// Retain the prepared session's authoritative writer for identity snapshots.
    pub fn with_transcript_writer(mut self, writer: Option<Arc<session::jsonl::JsonlWriter>>) -> Self {
        self.transcript_writer = writer;
        self
    }

    /// Retain a zero-message conversation that owns a durable scheduled task.
    async fn ensure_scheduled_chat(&self, session_id: &str) -> Result<(), String> {
        let session_uuid = lingxi_core::types::SessionId::parse_prefixed(session_id)
            .ok_or("Invalid scheduled task chat identity")?
            .as_uuid()
            .to_string();
        let session_id = session_uuid.as_str();
        let path = orchestrator::transcript_paths::main_transcript_path(
            &self.lingxi_home,
            &self.session_cwd,
            session_id,
        );
        let reader = session::jsonl::reader::JsonlReader::new(path.clone(), self.fs.clone());
        let mut title = "Scheduled task".to_string();
        match reader.read_routed().await {
            Ok(loaded) => {
                if !loaded.messages_in_order.is_empty()
                    || loaded.mobile_empty_sessions.contains(session_id)
                {
                    return Ok(());
                }
                if loaded.malformed_line_count > 0 {
                    return Err("Cannot attach a scheduled task to an unreadable chat".into());
                }
                if let Some(existing) = loaded.custom_titles.get(session_id) {
                    title.clone_from(existing);
                }
            }
            Err(session::jsonl::reader::ReaderError::Fs(lingxi_core::host::FsError::NotFound(
                _,
            ))) => {}
            Err(error) => {
                // The desktop filesystem may wrap ENOENT as FsError::Io.
                // Verify absence without treating permission/corruption errors as empty.
                if !matches!(tokio::fs::try_exists(&path).await, Ok(false)) {
                    return Err(format!("Cannot read the scheduled task's chat: {error}"));
                }
            }
        }
        session::jsonl::writer::JsonlWriter::new(path, self.fs.clone())
            .append_mobile_empty_session(session_id, &title)
            .await
            .map_err(|error| format!("Cannot retain the scheduled task's chat: {error}"))
    }

    /// Build a session-store context rooted at the desktop config directory and
    /// the connection's project cwd.
    #[must_use]
    pub fn new(
        lingxi_home: PathBuf,
        session_cwd: String,
        fs: Arc<dyn lingxi_core::host::FileSystem>,
    ) -> Self {
        Self {
            lingxi_home,
            session_cwd,
            fs,
            transcript_writer: None,
        }
    }
}

/// The engine-routing seam for the full [`ClientCommand`] surface (everything
/// except the turn + permission path the [`crate::server::BridgeConnection`]
/// handles directly).
///
/// Abstracting routing behind a trait keeps the connection loop decoupled from
/// HOW the engine was assembled: the production server builds an
/// [`EngineCommandRouter`] from `harness_runtime::desktop::build`'s `DesktopRuntime`
/// handles, while a routing test builds the SAME router over the engine's mock
/// handles.
#[async_trait]
pub trait CommandRouter: Send + Sync + 'static {
    /// Route one decoded [`ClientCommand`], emitting any reply event(s) through
    /// `sink`. Pure side-effecting: it never blocks the caller for a turn (the
    /// turn path lives on [`crate::server::TurnDriver`]).
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>);

    /// Product-only correlated read: existing protocol events form one atomic
    /// response rather than interleaving with live background status pushes.
    /// The caller fences outbound delivery through this read. Implementations
    /// must collect locally: never emit through the connection, mutate engine
    /// state, drain an interaction broker, or wait for the SDK turn gate.
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        Err("desktop runtime snapshot is unavailable".into())
    }

    /// Dispatch a raw `/<name> [args]` line and hand back the dispatch RESULT so
    /// the connection (which owns the [`crate::server::TurnDriver`] + queue) can
    /// decide whether to PRINT it (display-only `type: "local"` commands) or run
    /// it AS a turn (`type: "prompt"` commands like `/loop`, plus Markdown /
    /// Plugin prompt commands — claude-code injects the expanded prompt as the
    /// user message). Returns `None` when no dispatcher is wired (the connection
    /// then falls back to the display-only `route` path).
    async fn dispatch_slash(&self, _raw: &str) -> Option<SlashDispatchOutcome> {
        None
    }
}

#[derive(Default)]
struct RuntimeSnapshotEvents(std::sync::Mutex<Vec<ClientEvent>>);

#[async_trait]
impl ClientEventSink for RuntimeSnapshotEvents {
    async fn emit(&self, event: ClientEvent) {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
    }
}

/// One already-dispatched slash result plus authoritative out-of-band changes
/// observed during that same dispatch. Keeping both together lets the bridge
/// connection route prompt commands without dispatching the command twice.
#[derive(Debug, Clone)]
pub struct SlashDispatchOutcome {
    /// The single engine dispatch result for the submitted command.
    pub result: lingxi_core::host::SlashDispatchResult,
    /// Full authoritative state/catalog events produced by that dispatch.
    pub authority_events: Vec<ClientEvent>,
}

/// Lower an `Option<LoginInfo>` to the auth-state DTO (`current_user` → wire).
fn lower_auth_state(info: Option<LoginInfo>) -> AuthStateDto {
    match info {
        Some(li) => AuthStateDto::SignedIn {
            email: li.email,
            org_id: li.org_id,
        },
        None => AuthStateDto::SignedOut,
    }
}

fn provider_model_catalog_from_listings(
    listings: &[lingxi_core::host::ModelListing],
) -> Vec<client::protocol::listings::ProviderModelCatalogEntryDto> {
    lingxi_core::host::provider_model_catalog(listings)
        .iter()
        .map(lower_provider_model_catalog_entry)
        .collect()
}

/// The production [`CommandRouter`]: wraps the real engine handles and lowers
/// each reply with the pure `client::adapter::lowering` parity fns.
pub struct EngineCommandRouter {
    session_cron: Option<Arc<cron::CronScheduler>>,
    cron_firer: Option<Arc<crate::cron_host::HostCronFirer>>,
    handle: Arc<dyn OrchestratorHandle>,
    auth: Arc<dyn AuthHandle>,
    tasks: Arc<dyn TaskRegistryHandle>,
    /// Optional slash dispatcher. The desktop runtime always supplies one; it is
    /// `Option` so a transport that has not yet built the command registry can
    /// still route the rest of the surface.
    dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
    /// Optional shared slash-command registry backing the dispatcher. When
    /// present it lets the router emit `SlashCommandCatalog` pulls and
    /// `CommandsChanged` pushes from the same live registry snapshot.
    slash_registry: Option<Arc<RwLock<CommandRegistry>>>,
    /// Optional persisted-session catalog/replay context. Production boot wires
    /// it; lightweight users of the routing seam can omit it.
    session_store: Option<SessionStoreContext>,
    /// Process-owned observer facts supplement the live task registry for
    /// foreground children and allocation before the first transcript write.
    session_agent_observer:
        Option<Arc<harness_runtime::desktop::session_agents::DesktopSessionAgentObserver>>,
    /// Current coordinator ownership supplements persisted agent transcripts
    /// when a host reconnects after missing background completion events.
    team_registry: Option<Arc<dyn lingxi_core::host::team_registry::TeamRegistryHandle>>,
    /// Shared provider credential manager. Production bridge boot wires the
    /// exact manager used by the runtime; tests/embedded clients may omit it.
    catalog_registry: harness_runtime::desktop::FusionCatalogRegistry,
    credentials: Option<Arc<secret::CredentialManager>>,
    /// Settings-visible provider model directory assembled from the full
    /// provider config, including providers not currently routable.
    provider_model_catalog_listings: Vec<lingxi_core::host::ModelListing>,
    /// Packaged Desktop owns persistence in its signed credential broker; in
    /// that mode bridge credential mutations update only this process cache.
    provider_credentials_ephemeral: bool,
    /// HTTP transport used by provider connection probes. Production boot
    /// supplies the same guarded transport as the runtime's LLM clients.
    http: Option<Arc<dyn lingxi_core::host::HttpTransport>>,
    /// Optional layered-settings context backing the `Settings` listing.
    /// Production boot wires it from the desktop composition root; lightweight
    /// users of the routing seam may omit it, in which case the listing
    /// reports the missing context rather than emitting nothing.
    settings: Option<SettingsContext>,
    /// Optional MCP-scope roots backing `UpsertMcpServer` / `RemoveMcpServer`.
    /// Production boot wires it from the SAME `.mcp.json` / global-config
    /// paths `harness_runtime::desktop` resolves the read-side registry from;
    /// lightweight users of the routing seam may omit it, in which case the
    /// two commands report the missing context rather than doing nothing.
    mcp: Option<McpPaths>,
    /// Live MCP registry for Desktop-only configuration snapshots and hot reloads.
    mcp_registry: Option<Arc<mcp::McpRegistry>>,
    /// Live plugin runtime for Desktop-only refresh after install/config changes.
    plugin_runtime: Option<Arc<harness_runtime::desktop::PluginRuntime>>,
    /// Live hook registry for source-scoped hot swaps.
    hook_registry: Option<Arc<RwLock<hooks::HookRegistry>>>,
    /// Existing repo-root catalog reloader used for skills/plugins live refresh.
    repo_root_reloader: Option<Arc<dyn lingxi_core::host::RepoRootReloader>>,
    /// FileChanged watcher controller used to replace watch matchers after hook edits.
    file_changed_watcher:
        Option<harness_runtime::desktop::file_changed_watch::FileChangedWatcherController>,
    /// Set while a turn is in flight — `ClearSession` is rejected in this window
    /// (plan §2 mid-turn semantics).
    turn_active: AtomicBool,
}

impl EngineCommandRouter {
    pub(crate) fn with_session_cron(mut self, scheduler: Option<Arc<cron::CronScheduler>>) -> Self {
        self.session_cron = scheduler;
        self
    }
    pub(crate) fn with_cron_firer(mut self, firer: Arc<crate::cron_host::HostCronFirer>) -> Self {
        self.cron_firer = Some(firer);
        self
    }

    /// Move this process's live-session presence only after the engine has
    /// successfully activated the destination session.  The bridge process
    /// owns one mutable presence record, while durable coordinators retain
    /// old-session leases independently for late background work; updating
    /// this record must therefore never unregister/release the old writer.
    async fn refresh_process_session_presence(
        &self,
        previous: lingxi_core::types::SessionId,
        current: lingxi_core::types::SessionId,
    ) -> Option<String> {
        if let Some(observer) = &self.session_agent_observer {
            observer.set_session_id(current.as_uuid().to_string());
        }
        // Collect, never early-return: the presence record is independent of the
        // cron scheduler, and skipping it leaves this PID advertising the OLD
        // session after a switch that already committed.
        let mut problems: Vec<String> = Vec::new();
        if let Some(scheduler) = &self.session_cron {
            if let Err(error) = scheduler
                .set_session_id(current.as_uuid().to_string())
                .await
            {
                problems.push(format!("scheduled tasks could not restart: {error}"));
            }
        }
        if let Err(error) =
            harness_runtime::desktop::refresh_process_session_presence(previous, current).await
        {
            problems.push(error.to_string());
        }
        (!problems.is_empty()).then(|| format!("session switched, but {}", problems.join("; ")))
    }

    async fn dispatch_desktop_slash(
        &self,
        raw: &str,
    ) -> Option<lingxi_core::host::SlashDispatchResult> {
        let parsed = parse_slash_command(raw)?;
        match parsed.name.as_str() {
            "diff" => {
                let focus = parsed.raw_args.trim();
                let prompt = if focus.is_empty() {
                    "Inspect the current working tree and show the user the uncommitted diff. Do not modify any files.".to_string()
                } else {
                    format!(
                        "Inspect the current working tree and show the user the uncommitted diff, focusing on: {focus}. Do not modify any files."
                    )
                };
                Some(lingxi_core::host::SlashDispatchResult::RunAsTurn { prompt })
            }
            "plan" => Some(self.dispatch_desktop_plan(&parsed.raw_args).await),
            _ => None,
        }
    }

    async fn dispatch_desktop_plan(&self, args: &str) -> lingxi_core::host::SlashDispatchResult {
        let previous_permission = self
            .handle
            .permission_mode()
            .await
            .unwrap_or_else(|| "default".to_string());
        let was_plan_mode = self.handle.plan_mode().await;

        if previous_permission != "plan" {
            if let Err(error) = self.handle.set_permission_mode("plan").await {
                return lingxi_core::host::SlashDispatchResult::Handled {
                    display: format!("Could not enable plan mode: {error}"),
                };
            }
            lingxi_core::host::live_sessions::set_process_permission_mode("plan", false);
        }
        if !was_plan_mode {
            if let Err(error) = self.handle.set_plan_mode(true).await {
                if previous_permission != "plan" {
                    let _ = self.handle.set_permission_mode(&previous_permission).await;
                    lingxi_core::host::live_sessions::set_process_permission_mode(
                        &previous_permission,
                        previous_permission == "bypassPermissions",
                    );
                }
                return lingxi_core::host::SlashDispatchResult::Handled {
                    display: format!("Could not enable plan mode: {error}"),
                };
            }
        }

        let request = args.trim();
        if request.is_empty() || matches!(request, "open" | "share") {
            let display = if was_plan_mode {
                "Plan mode is already enabled. The current plan is shown above the composer."
            } else {
                "Plan mode enabled. The next request will be planned before any changes are made."
            };
            lingxi_core::host::SlashDispatchResult::Handled {
                display: display.to_string(),
            }
        } else {
            lingxi_core::host::SlashDispatchResult::RunAsTurn {
                prompt: request.to_string(),
            }
        }
    }

    /// Build a router from the engine handles a desktop runtime exposes.
    #[must_use]
    pub fn new(
        handle: Arc<dyn OrchestratorHandle>,
        auth: Arc<dyn AuthHandle>,
        tasks: Arc<dyn TaskRegistryHandle>,
        dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
        slash_registry: Option<Arc<RwLock<CommandRegistry>>>,
    ) -> Self {
        Self {
            cron_firer: None,
            session_cron: None,
            handle,
            auth,
            tasks,
            dispatcher,
            slash_registry,
            session_store: None,
            session_agent_observer: None,
            team_registry: None,
            catalog_registry: harness_runtime::desktop::FusionCatalogRegistry::default(),
            credentials: None,
            provider_model_catalog_listings: Vec::new(),
            provider_credentials_ephemeral: false,
            http: None,
            settings: None,
            mcp: None,
            mcp_registry: None,
            plugin_runtime: None,
            hook_registry: None,
            repo_root_reloader: None,
            file_changed_watcher: None,
            turn_active: AtomicBool::new(false),
        }
    }

    /// Attach the runtime's shared credential manager so Desktop credential
    /// operations use the same secure-store entries as CLI and TUI.
    #[must_use]
    pub fn with_catalog_registry(
        mut self,
        registry: harness_runtime::desktop::FusionCatalogRegistry,
    ) -> Self {
        self.catalog_registry = registry;
        self
    }

    pub fn with_credentials(mut self, credentials: Arc<secret::CredentialManager>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    #[must_use]
    pub fn with_provider_model_catalog_listings(
        mut self,
        listings: Vec<lingxi_core::host::ModelListing>,
    ) -> Self {
        self.provider_model_catalog_listings = listings;
        self
    }

    /// Keep provider credential writes process-local because the parent owns
    /// the persistent Data Protection Keychain entry.
    #[must_use]
    pub fn with_ephemeral_provider_credentials(mut self, enabled: bool) -> Self {
        self.provider_credentials_ephemeral = enabled;
        self
    }

    /// Attach the runtime's provider HTTP transport for low-cost connection
    /// probes from Desktop settings.
    #[must_use]
    pub fn with_http(mut self, http: Arc<dyn lingxi_core::host::HttpTransport>) -> Self {
        self.http = Some(http);
        self
    }

    /// Share the same process-local liveness source used for pushed agent events.
    #[must_use]
    pub fn with_session_agent_observer(
        mut self,
        observer: Arc<harness_runtime::desktop::session_agents::DesktopSessionAgentObserver>,
    ) -> Self {
        self.session_agent_observer = Some(observer);
        self
    }

    #[must_use]
    pub fn with_team_registry(
        mut self,
        registry: Arc<dyn lingxi_core::host::team_registry::TeamRegistryHandle>,
    ) -> Self {
        self.team_registry = Some(registry);
        self
    }

    /// Attach the persisted JSONL session store used by list/resume commands.
    #[must_use]
    pub fn with_session_store(mut self, session_store: SessionStoreContext) -> Self {
        self.session_store = Some(session_store);
        self
    }

    /// Attach the layered-settings context the `Settings` listing reads. The
    /// composition root supplies the settings roots, the file-layer values read
    /// at session start, and the administrator's managed overlay; this router
    /// only reads and lowers them.
    #[must_use]
    pub fn with_settings_context(mut self, settings: SettingsContext) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Attach the MCP-scope roots the `UpsertMcpServer` / `RemoveMcpServer`
    /// commands write through. The composition root supplies the SAME
    /// project directory and global-config path the read-side registry was
    /// loaded from (`resolve_desktop_config`'s `project_mcp_path` /
    /// `global_mcp_path`), so a write always lands where the next reload
    /// would look for it.
    #[must_use]
    pub fn with_mcp_paths(mut self, mcp: McpPaths) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// Attach the live MCP registry for Desktop configuration snapshots and
    /// hot reconnect/replacement.
    #[must_use]
    pub fn with_mcp_registry(mut self, mcp_registry: Arc<mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(mcp_registry);
        self
    }

    /// Attach the live plugin runtime so Desktop settings can refresh plugin
    /// registries after a successful mutation.
    #[must_use]
    pub fn with_plugin_runtime(
        mut self,
        plugin_runtime: Option<Arc<harness_runtime::desktop::PluginRuntime>>,
    ) -> Self {
        self.plugin_runtime = plugin_runtime;
        self
    }

    /// Attach the live hook registry for Desktop hook hot swaps.
    #[must_use]
    pub fn with_hook_registry(mut self, hook_registry: Arc<RwLock<hooks::HookRegistry>>) -> Self {
        self.hook_registry = Some(hook_registry);
        self
    }

    /// Attach the repo-root reloader for skills/plugins live refreshes.
    #[must_use]
    pub fn with_repo_root_reloader(
        mut self,
        repo_root_reloader: Arc<dyn lingxi_core::host::RepoRootReloader>,
    ) -> Self {
        self.repo_root_reloader = Some(repo_root_reloader);
        self
    }

    /// Attach the FileChanged watcher controller for replacing settings-derived
    /// matcher sets after hook edits.
    #[must_use]
    pub fn with_file_changed_watcher(
        mut self,
        watcher: Option<harness_runtime::desktop::file_changed_watch::FileChangedWatcherController>,
    ) -> Self {
        self.file_changed_watcher = watcher;
        self
    }

    /// Mark whether a turn is currently in flight. The connection flips this
    /// `true` when it spawns a turn on `SendPrompt` and `false` when the turn
    /// ends; `ClearSession` is rejected while it is `true`.
    pub fn set_turn_active(&self, active: bool) {
        self.turn_active.store(active, Ordering::SeqCst);
    }

    /// True iff a turn is currently in flight.
    #[must_use]
    pub fn is_turn_active(&self) -> bool {
        self.turn_active.load(Ordering::SeqCst)
    }

    async fn dispatch_mod_ui_control_result(
        &self,
        request_id: String,
        request: Result<lingxi_core::types::utf16_json::Utf16JsonProjection, String>,
        sink: &dyn ClientEventSink,
    ) {
        let result = match request {
            Ok(request) => self.handle.mod_ui_control(request).await,
            Err(error) => Err(HandleError::ActionFailed(error)),
        };
        sink.emit(client::adapter::turn::mod_ui_control_result_event(
            request_id, result,
        ))
        .await;
    }

    async fn dispatch_mod_ui_client_operation_result(
        &self,
        request_id: String,
        operation: Result<lingxi_core::types::utf16_json::Utf16JsonProjection, String>,
        sink: &dyn ClientEventSink,
    ) {
        let result = match operation {
            Ok(operation) => self.handle.mod_ui_client_operation(operation).await,
            Err(error) => Err(HandleError::ActionFailed(error)),
        };
        sink.emit(client::adapter::turn::mod_ui_client_operation_result_event(
            request_id, result,
        ))
        .await;
    }
}

#[async_trait]
impl CommandRouter for EngineCommandRouter {
    async fn runtime_snapshot(&self) -> Result<Vec<ClientEvent>, String> {
        if self.team_registry.is_none() {
            return Err("desktop runtime snapshot is unavailable".into());
        }
        let events = RuntimeSnapshotEvents::default();
        self.emit_session_agent_list(&events).await;
        self.emit_coordinator_snapshot(&events).await;
        tasks::emit_correlated_task_list(
            self.tasks.as_ref(),
            &events,
            TaskListFilter::default(),
            "desktop-runtime-snapshot".into(),
        )
        .await;
        let events = events
            .0
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner());
        if !events
            .iter()
            .any(|event| matches!(event, ClientEvent::SessionAgentList { .. }))
        {
            return Err("session agent snapshot is unavailable".into());
        }
        if let Some(error) = events.iter().find_map(|event| match event {
            ClientEvent::TaskListComplete {
                error: Some(error), ..
            } => Some(error),
            _ => None,
        }) {
            return Err(format!("desktop task snapshot is unavailable: {error}"));
        }
        Ok(events)
    }

    async fn dispatch_slash(&self, raw: &str) -> Option<SlashDispatchOutcome> {
        let before = self.capture_slash_authority().await;
        let result = match self.dispatch_desktop_slash(raw).await {
            Some(result) => result,
            None => self.dispatcher.as_ref()?.dispatch(raw).await,
        };
        // Display-only feedback belongs to the client's command surface. Persist
        // actual task turns and command effects through their existing paths.
        let after = self.capture_slash_authority().await;
        Some(SlashDispatchOutcome {
            result,
            authority_events: Self::slash_authority_change_events(&before, &after),
        })
    }

    // The full command dispatch is one match over the command surface; splitting
    // it would scatter the one-place-routes-everything map this module exists to
    // be. (Same convention as `harness_runtime::desktop::build`.)
    #[allow(clippy::too_many_lines)]
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        match command {
            ClientCommand::UiRender {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_render");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientModule { request_id, plugin } => {
                let request = Ok(serde_json::json!({
                    "subtype": "ui_client_module",
                    "plugin": plugin,
                }).into());
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiMessage {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_message",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientFault {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_client_fault",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientPress {
                request_id,
                request_json,
            } => {
                let request = client::adapter::turn::parse_mod_ui_control_request(
                    &request_json,
                    "ui_client_press",
                );
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiPress {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_press");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiInput {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_input");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiSelect {
                request_id,
                request_json,
            } => {
                let request =
                    client::adapter::turn::parse_mod_ui_control_request(&request_json, "ui_select");
                self.dispatch_mod_ui_control_result(request_id, request, sink.as_ref())
                    .await;
            }
            ClientCommand::UiClientOperation {
                request_id,
                operation_json,
            } => {
                let operation =
                    client::adapter::turn::parse_mod_ui_client_operation(&operation_json);
                self.dispatch_mod_ui_client_operation_result(request_id, operation, sink.as_ref())
                    .await;
            }
            // Both arms write the durable automation store — `started` binds a
            // session id into it, `complete` records a run result — so they sit
            // behind the same trust gate as the `CronManage` mutations below.
            // Without it an untrusted workspace can still mutate the file every
            // other write path refuses to touch.
            ClientCommand::CronRunStarted { run_id, session_id } => {
                if !self.handle.workspace_trusted().await {
                    return;
                }
                if let Some(firer) = &self.cron_firer {
                    firer.started(&run_id, &session_id).await;
                }
            }
            ClientCommand::CronRunCompleted {
                run_id,
                session_id,
                summary,
                error,
            } => {
                if !self.handle.workspace_trusted().await {
                    return;
                }
                if let Some(firer) = &self.cron_firer {
                    firer.complete(&run_id, session_id, summary, error).await;
                }
            }
            ClientCommand::CronManage {
                request_id,
                request,
            } => {
                if !matches!(request.action.as_str(), "list" | "history")
                    && !self.handle.workspace_trusted().await
                {
                    sink.emit(ClientEvent::CronResult {
                        request_id,
                        jobs: Vec::new(),
                        error: Some("Trust this workspace before changing scheduled tasks".into()),
                    })
                    .await;
                    return;
                }
                let status = self.handle.get_status_snapshot().await;
                let owner_id = status
                    .session_id
                    .strip_prefix("sess:")
                    .unwrap_or(&status.session_id);
                if request.action == "create" {
                    let anchor = match self.session_store.as_ref() {
                        Some(store) => store.ensure_scheduled_chat(owner_id).await,
                        None => Err("Scheduled task chat storage is unavailable".into()),
                    };
                    if let Err(error) = anchor {
                        sink.emit(ClientEvent::CronResult {
                            request_id,
                            jobs: Vec::new(),
                            error: Some(error),
                        })
                        .await;
                        return;
                    }
                }
                let cwd = status.cwd;
                let fs = crate::HostFileSystem::new(cwd.clone());
                let result = harness_runtime::desktop::cron_management::manage(
                    &fs,
                    &cwd,
                    request,
                    &self.tasks,
                    owner_id,
                )
                .await;
                let (jobs, error) = match result {
                    Ok(jobs) => (jobs, None),
                    Err(error) => (Vec::new(), Some(error)),
                };
                sink.emit(ClientEvent::CronResult {
                    request_id,
                    jobs,
                    error,
                })
                .await;
            }
            // ── Provider credentials ────────────────────────────────────────
            ClientCommand::ListProviderCredentials {
                operation_id,
                provider_ids,
                preview_provider_ids,
            } => {
                let validation_error = if provider_ids.len() > 32
                    || provider_ids.iter().any(|id| !provider_id_is_valid(id))
                    || preview_provider_ids
                        .iter()
                        .any(|id| !provider_ids.contains(id))
                {
                    Some("invalid provider credential query".to_string())
                } else {
                    None
                };
                self.emit_provider_credential_status(
                    operation_id,
                    &provider_ids,
                    &preview_provider_ids,
                    validation_error,
                    &*sink,
                )
                .await;
            }
            ClientCommand::SetProviderCredential {
                operation_id,
                provider_id,
                credential,
            } => {
                let credential_preview =
                    secret::masked_credential_preview(credential.expose_secret());
                let mut error = if !provider_id_is_valid(&provider_id)
                    || credential.expose_secret().is_empty()
                    || credential.expose_secret().len() > 16_384
                    || credential.expose_secret().contains('\0')
                {
                    Some("invalid provider credential".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    if self.provider_credentials_ephemeral {
                        credentials
                            .set_provider_key_ephemeral(&provider_id, credential.expose_secret())
                            .await;
                        None
                    } else {
                        credentials
                            .set_provider_key(&provider_id, credential.expose_secret())
                            .await
                            .err()
                            .map(|failure| {
                                format!("failed to store provider credential: {failure}")
                            })
                    }
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                // Publish before emitting readiness. The helper starts its
                // full-source reconciliation detached; `false` means storage
                // succeeded but this process's fixed auth route needs restart.
                if error.is_none()
                    && !crate::boot::refresh_fusion_catalog_bounded(
                        &self.catalog_registry,
                        vec![provider_id.clone()],
                    )
                    .await
                {
                    error = Some(
                        harness_runtime::desktop::fusion_credential_restart_required_message(
                            &provider_id,
                        ),
                    );
                }
                let applied = error.is_none();
                let credential_previews = if applied {
                    HashMap::from([(provider_id.clone(), credential_preview)])
                } else {
                    HashMap::new()
                };
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: applied
                        .then_some(provider_id.clone())
                        .into_iter()
                        .collect(),
                    unavailable_provider_ids: (!applied)
                        .then_some(provider_id)
                        .into_iter()
                        .collect(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    credential_previews,
                    error,
                })
                .await;
            }
            // Round-12 finding [2]: a credential DELETE must tell Fusion's
            // catalog filter, and it must NOT do so through the sibling `Set`
            // arm's `crate::boot::refresh_fusion_catalog_bounded` one screen
            // up — that routes to
            // `FusionCatalogRefresher::refresh_after_credential_write`, whose
            // `forced` loop publishes `true` for the named profile, i.e. the
            // exact opposite of what a delete means. The refresher's merge
            // rule is additive by design and can never lower a `true`
            // (`refresh_inner`: `if row.available { insert(true) } else {
            // entry(..).or_insert(false) }`), so a re-probe cannot close this
            // either. Hence the dedicated removal fan-out
            // `harness_runtime::desktop::refresh_fusion_catalog_after_credential_delete`
            // below. Without it a provider whose key was deleted mid-session
            // survived `filter_fusion_catalog`, could be auto-selected as a
            // `/fusion` panel, had budget reserved for it, and died on
            // `LlmError::Authentication` at request time instead of being
            // excluded by the §4 preflight. BOTH branches need it — the
            // persistent keychain delete and the packaged/brokered ephemeral
            // one, whose key never touched the keychain at all and so could
            // never be re-probed away even in principle. Unlike the write
            // path this needs no budget wrapper: `mark_credential_removed` is
            // a lock-and-set with no credential-backend I/O.
            ClientCommand::DeleteProviderCredential {
                operation_id,
                provider_id,
            } => {
                let error = if !provider_id_is_valid(&provider_id) {
                    Some("invalid provider id".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    if self.provider_credentials_ephemeral {
                        credentials
                            .delete_provider_key_ephemeral(&provider_id)
                            .await;
                        None
                    } else {
                        credentials
                            .delete_provider_key(&provider_id)
                            .await
                            .err()
                            .map(|failure| {
                                format!("failed to delete provider credential: {failure}")
                            })
                    }
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                let applied = error.is_none();
                if applied {
                    harness_runtime::desktop::refresh_fusion_catalog_after_credential_delete(
                        &self.catalog_registry,
                        &provider_id,
                    )
                    .await;
                }
                sink.emit(ClientEvent::ProviderCredentialStatus {
                    operation_id,
                    configured_provider_ids: Vec::new(),
                    unavailable_provider_ids: (!applied)
                        .then_some(provider_id)
                        .into_iter()
                        .collect(),
                    storage_encrypted: self
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.provider_key_storage_is_encrypted()),
                    credential_previews: HashMap::new(),
                    error,
                })
                .await;
            }
            ClientCommand::TestProviderConnection {
                operation_id,
                provider_id,
                api_base,
                model,
                credential_override,
            } => {
                let event = if provider_id_is_valid(&provider_id) {
                    crate::provider_connection::test(
                        self.credentials.as_ref(),
                        self.http.as_ref(),
                        crate::provider_connection::ProviderConnectionProbe {
                            operation_id,
                            provider_id,
                            api_base,
                            model,
                            credential_override,
                        },
                    )
                    .await
                } else {
                    ClientEvent::ProviderConnectionTested {
                        operation_id,
                        provider_id,
                        connected: false,
                        reachable: false,
                        authenticated: false,
                        model_available: false,
                        http_status: None,
                        latency_ms: 0,
                        message: "Provider 标识无效".to_string(),
                        used_stored_credential: credential_override.is_none(),
                    }
                };
                sink.emit(event).await;
            }

            // ── Permission mode ────────────────────────────────────────────
            ClientCommand::SetPermissionMode { mode } => {
                match self.handle.set_permission_mode(&mode).await {
                    Ok(()) => {
                        let active = self.handle.permission_mode().await.unwrap_or(mode);
                        lingxi_core::host::live_sessions::set_process_permission_mode(
                            &active,
                            active == "bypassPermissions",
                        );
                        sink.emit(ClientEvent::PermissionModeChanged { mode: active })
                            .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Rejected,
                            message: format!("set_permission_mode failed: {e}"),
                        })
                        .await;
                    }
                }
                self.emit_controls_snapshot(&*sink).await;
            }

            // ── Model ──────────────────────────────────────────────────────
            ClientCommand::SetModel { model } => {
                let listings = self.handle.list_model_listings().await;
                let (model_id, profile) = lingxi_core::host::parse_model_ref(&model, &listings);
                match self
                    .handle
                    .switch_model(&model_id, profile.as_deref())
                    .await
                {
                    Ok(()) => {
                        let selected =
                            lingxi_core::host::qualified_model_ref(&model_id, profile.as_deref());
                        sink.emit(ClientEvent::ModelChanged { model: selected })
                            .await;
                        if self.persisted_reasoning_selection().is_some() {
                            if self.restore_persisted_reasoning_selection().await.is_err() {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(
                                        lingxi_core::host::ReasoningSelection::Automatic,
                                    )
                                    .await;
                            }
                        } else if let Some(controls) = self.handle.conversation_controls().await {
                            if controls.requested_reasoning_selection
                                != lingxi_core::host::ReasoningSelection::Automatic
                                && controls.requested_reasoning_selection
                                    != controls.effective_reasoning_selection
                            {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(
                                        lingxi_core::host::ReasoningSelection::Automatic,
                                    )
                                    .await;
                            }
                        }
                        self.emit_controls_snapshot(&*sink).await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("switch_model failed: {e}"),
                        })
                        .await;
                    }
                }
            }
            ClientCommand::ListModels => {
                self.emit_listing(ListingKindDto::Models, &*sink).await;
            }
            ClientCommand::GetConversationControls => {
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::SetReasoningSelection { selection } => {
                let requested = decode_reasoning_selection(selection);
                let previous = self.handle.conversation_controls().await.map(|controls| {
                    (
                        controls.requested_reasoning_selection,
                        controls.effective_reasoning_selection,
                        controls.reasoning_spec.selections_persistable,
                    )
                });
                if let Err(error) = self.handle.set_reasoning_selection(requested).await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_reasoning_selection failed: {error}"),
                    })
                    .await;
                    return;
                }
                let (effective, persistable) = self
                    .handle
                    .conversation_controls()
                    .await
                    .map(|controls| {
                        (
                            controls.effective_reasoning_selection,
                            controls.reasoning_spec.selections_persistable,
                        )
                    })
                    .unwrap_or((lingxi_core::host::ReasoningSelection::Automatic, true));
                let persisted_default = if persistable {
                    effective
                } else {
                    lingxi_core::host::ReasoningSelection::Automatic
                };
                if let Err(error) = self.persist_reasoning_selection(&persisted_default) {
                    let rollback = previous
                        .as_ref()
                        .map(|(requested, _, _)| requested.clone())
                        .unwrap_or(lingxi_core::host::ReasoningSelection::Automatic);
                    let _ = self.handle.set_reasoning_selection(rollback).await;
                    let previous_default = previous.as_ref().map_or(
                        lingxi_core::host::ReasoningSelection::Automatic,
                        |(_, effective, persistable)| {
                            if *persistable {
                                effective.clone()
                            } else {
                                lingxi_core::host::ReasoningSelection::Automatic
                            }
                        },
                    );
                    let _ = self.persist_reasoning_selection(&previous_default);
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("persist reasoning selection failed: {error}"),
                    })
                    .await;
                }
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::SetFastMode { enabled } => {
                if let Err(error) = self.handle.set_fast_mode(enabled).await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_fast_mode failed: {error}"),
                    })
                    .await;
                    return;
                }
                sink.emit(ClientEvent::FastModeChanged {
                    enabled: self.handle.fast_mode().await,
                })
                .await;
                self.emit_controls_snapshot(&*sink).await;
            }

            // ── Slash commands ──────────────────────────────────────────────
            ClientCommand::RunSlashCommand { raw, turn_id } => {
                if let Some(dispatcher) = self.dispatcher.as_ref() {
                    let before = self.capture_slash_authority().await;
                    let context = command_api::ModCommandRunContext {
                        origin: serde_json::json!({"kind":"bridge"}),
                        is_fullscreen: false,
                        columns: 80,
                    };
                    let (display, is_error) = match command_api::with_mod_command_context(
                        context,
                        dispatcher.dispatch(&raw),
                    )
                    .await
                    {
                        lingxi_core::host::SlashDispatchResult::Handled { display } => {
                            (display, false)
                        }
                        lingxi_core::host::SlashDispatchResult::Unknown { display, .. } => {
                            (display, true)
                        }
                        // A `type: "prompt"` command reached the display-only
                        // fallback (the connection should have intercepted it via
                        // `dispatch_slash` and run it as a turn). Surface the
                        // expanded prompt so nothing is silently dropped.
                        lingxi_core::host::SlashDispatchResult::RunAsTurn { prompt } => {
                            (prompt, false)
                        }
                        lingxi_core::host::SlashDispatchResult::NotASlashCommand => {
                            (format!("not a slash command: {raw}"), true)
                        }
                    };
                    sink.emit(ClientEvent::SlashCommandResult {
                        turn_id,
                        display,
                        is_error,
                    })
                    .await;
                    let after = self.capture_slash_authority().await;
                    self.emit_slash_authority_changes(&before, &after, &*sink)
                        .await;
                } else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "no slash-command dispatcher wired".into(),
                    })
                    .await;
                }
            }

            // ── Listings ─────────────────────────────────────────────────────
            ClientCommand::RefreshListings { which } => {
                for kind in which {
                    self.emit_listing(kind, &*sink).await;
                }
            }
            ClientCommand::ListSessionAgents => {
                self.emit_session_agent_list(&*sink).await;
            }
            ClientCommand::LoadSessionAgentTranscript { agent_id } => {
                self.emit_session_agent_transcript(agent_id, &*sink).await;
            }

            // ── Settings ─────────────────────────────────────────────────────
            ClientCommand::UpdateSettings {
                destination,
                patch_json,
            } => {
                self.apply_settings_patch(destination, &patch_json, &*sink)
                    .await;
            }

            // ── Permissions (persisted) ─────────────────────────────────────
            ClientCommand::UpdatePermissionRules {
                destination,
                behavior,
                add,
                remove,
            } => {
                self.apply_permission_rule_update(destination, behavior, add, remove, &*sink)
                    .await;
            }
            ClientCommand::SetDefaultPermissionMode { destination, mode } => {
                self.apply_default_permission_mode(destination, mode, &*sink)
                    .await;
            }
            ClientCommand::UpdateWorkspaceDirectories {
                destination,
                add,
                remove,
            } => {
                self.apply_workspace_directories_update(destination, add, remove, &*sink)
                    .await;
            }

            // ── MCP servers (persisted) ──────────────────────────────────────
            ClientCommand::UpsertMcpServer {
                scope,
                name,
                config_json,
            } => {
                self.apply_mcp_upsert(scope, &name, &config_json, &*sink)
                    .await;
            }
            ClientCommand::RemoveMcpServer { scope, name } => {
                self.apply_mcp_remove(scope, &name, &*sink).await;
            }
            ClientCommand::SkillAdmin { command } => {
                self.apply_skill_admin(command, &*sink).await;
            }
            ClientCommand::McpAdmin { command } => {
                self.apply_mcp_admin(command, &*sink).await;
            }
            ClientCommand::PluginAdmin { command } => {
                self.apply_plugin_admin(command, &*sink).await;
            }
            ClientCommand::HookAdmin { command } => {
                self.apply_hook_admin(command, &*sink).await;
            }

            // ── Auth ───────────────────────────────────────────────────────
            // This is a credential write: the Anthropic OAuth handle persists
            // tokens on success. The shared AuthHandle is wrapped by
            // `FusionCatalogClearingAuth`, which cheaply publishes the canonical
            // `anthropic-oauth` route before returning. Do not repeat the global
            // refresh here; that would create a second reconciliation owner.
            ClientCommand::Login => {
                let state = match self.auth.login().await {
                    Ok(li) => lower_auth_state(Some(li)),
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("login failed: {e}"),
                        })
                        .await;
                        lower_auth_state(self.auth.current_user().await)
                    }
                };
                sink.emit(ClientEvent::AuthState { state }).await;
            }
            // Round-12 finding [2], class member the finding itself did not
            // name: signing OUT is a credential removal too, and
            // `refresh_inner`'s closing `guard.entry("anthropic")
            // .or_insert(..)` is an `or_insert`, so an `anthropic: true` that
            // a sign-in published survives a sign-out for the life of the
            // process. Deliberately NOT fixed with a call here: `self.auth`
            // is the SAME `Arc<dyn AuthHandle>` the TUI's `/logout`
            // (`command_api::builtins::LogoutHandler`) drives, so the clearing lives
            // in `harness_runtime::desktop::FusionCatalogClearingAuth`, which wraps
            // that one handle for every sign-out surface in the process at
            // once. Adding a second call here would double-fire the same
            // fan-out and let the two mechanisms drift.
            ClientCommand::Logout => {
                if let Err(e) = self.auth.logout().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("logout failed: {e}"),
                    })
                    .await;
                }
                sink.emit(ClientEvent::AuthState {
                    state: lower_auth_state(self.auth.current_user().await),
                })
                .await;
            }

            // ── Compaction ───────────────────────────────────────────────────
            ClientCommand::ForceCompact => match self.handle.force_compact().await {
                Ok(summary) => {
                    sink.emit(ClientEvent::CompactionCompleted {
                        messages_before: summary.messages_before,
                        messages_after: summary.messages_after,
                        bytes_saved: summary.bytes_saved,
                        summary: summary.summary,
                    })
                    .await;
                }
                Err(e) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("force_compact failed: {e}"),
                    })
                    .await;
                }
            },

            // ── Session control ──────────────────────────────────────────────
            ClientCommand::ClearSession => {
                // Mid-turn semantics (plan §2): reject `ClearSession` while a
                // turn is in flight — it must NOT reach the engine.
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot clear the session while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }
                let previous_session_id = self.handle.current_session_id().await;
                match self.handle.clear_session().await {
                    Ok(()) => {
                        let current_session_id = self.handle.current_session_id().await;
                        let presence_warning = self
                            .refresh_process_session_presence(
                                previous_session_id,
                                current_session_id,
                            )
                            .await;
                        sink.emit(ClientEvent::SessionEnded).await;
                        if let Some(message) = presence_warning {
                            sink.emit(ClientEvent::SystemNotice {
                                message,
                                is_error: false,
                            })
                            .await;
                        }
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("clear_session failed: {e}"),
                        })
                        .await;
                    }
                }
            }
            ClientCommand::ListSessions { limit } => {
                let limit = limit.map_or(DEFAULT_SESSION_LIST_LIMIT, |value| value as usize);
                self.emit_session_list(limit, &*sink).await;
            }
            ClientCommand::NewSession { cwd: _, model } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot start a new session while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }

                let previous_session_id = self.handle.current_session_id().await;
                if let Err(error) = self.handle.clear_session().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("new session (clear_session) failed: {error}"),
                    })
                    .await;
                    return;
                }
                let current_session_id = self.handle.current_session_id().await;
                let presence_warning = self
                    .refresh_process_session_presence(previous_session_id, current_session_id)
                    .await;

                if let Some(model) = model {
                    let listings = self.handle.list_model_listings().await;
                    let (model_id, profile) = lingxi_core::host::parse_model_ref(&model, &listings);
                    if let Err(error) = self
                        .handle
                        .switch_model(&model_id, profile.as_deref())
                        .await
                    {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("new session model switch failed: {error}"),
                        })
                        .await;
                        return;
                    }
                }
                let _ = self.restore_persisted_reasoning_selection().await;

                let session_id = self.handle.current_session_id().await.to_string();
                sink.emit(ClientEvent::SessionStarted {
                    session_id,
                    mode: SessionModeDto::Code,
                })
                .await;
                if let Some(message) = presence_warning {
                    sink.emit(ClientEvent::SystemNotice {
                        message,
                        is_error: false,
                    })
                    .await;
                }
            }
            ClientCommand::ResumeSession { session_id, cwd } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "cannot resume while a turn is in flight".into(),
                    })
                    .await;
                    return;
                }

                let uuid = match uuid::Uuid::parse_str(&session_id) {
                    Ok(uuid) => uuid,
                    Err(error) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: format!(
                                "resume: malformed session id {session_id:?}: {error}"
                            ),
                        })
                        .await;
                        return;
                    }
                };

                let Some(store) = self.session_store.as_ref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "resume: persisted session store is unavailable".into(),
                    })
                    .await;
                    return;
                };
                let cwd = cwd.unwrap_or_else(|| store.session_cwd.clone());
                let replayed = match orchestrator::replay_session_state(
                    &store.lingxi_home,
                    &cwd,
                    uuid,
                    store.fs.clone(),
                )
                .await
                {
                    Ok(replayed) => replayed,
                    Err(error) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: format!("resume: session {session_id} not resumable: {error}"),
                        })
                        .await;
                        return;
                    }
                };

                // The engine resumes from the compacted history below, while
                // the renderer receives the complete main-thread transcript so
                // pre-compaction messages remain visible without re-entering
                // the LLM context.
                let messages = client::adapter::lowering::lower_transcript_with_tool_results(
                    &replayed.display_history,
                    &replayed.client_state_tool_results,
                );
                let resume_plan_mode = replayed.state.plan_mode;
                let previous_session_id = self.handle.current_session_id().await;
                let previous_permission_mode = self
                    .handle
                    .permission_mode()
                    .await
                    .unwrap_or_else(|| "default".to_string());
                let previous_plan_mode = self.handle.plan_mode().await;
                let runtime_snapshot = replayed.handle_runtime_snapshot();
                let restored_usage = runtime_snapshot.current_usage;
                if resume_plan_mode {
                    if let Err(error) = self.handle.set_permission_mode("plan").await {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("resume plan permission mode failed: {error}"),
                        })
                        .await;
                        return;
                    }
                    if let Err(error) = self.handle.set_plan_mode(true).await {
                        let _ = self
                            .handle
                            .set_permission_mode(&previous_permission_mode)
                            .await;
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("resume plan mode failed: {error}"),
                        })
                        .await;
                        return;
                    }
                }
                if let Err(error) = self
                    .handle
                    .resume_session(
                        lingxi_core::types::SessionId::from_uuid(uuid),
                        replayed.state.history,
                        replayed.last_message_uuid.map(|value| value.to_string()),
                        replayed.state.active_goal.clone().map(|goal| {
                            lingxi_core::host::ActiveGoalSnapshot {
                                condition: goal.condition,
                                set_at: goal.set_at,
                                last_reason: goal.last_reason,
                                iterations: goal.iterations,
                                tokens_at_start: goal.tokens_at_start,
                            }
                        }),
                        runtime_snapshot,
                    )
                    .await
                {
                    if resume_plan_mode {
                        let _ = self.handle.set_plan_mode(previous_plan_mode).await;
                        let _ = self
                            .handle
                            .set_permission_mode(&previous_permission_mode)
                            .await;
                    }
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("resume_session failed: {error}"),
                    })
                    .await;
                    return;
                }

                // The device-level reasoning default is the counterpart to
                // Electron's persisted model/permission preferences. Apply it
                // after transcript hydration so an old assistant row cannot
                // silently replace the user's last selected effort on restart.
                let _ = self.restore_persisted_reasoning_selection().await;

                let presence_warning = self
                    .refresh_process_session_presence(
                        previous_session_id,
                        lingxi_core::types::SessionId::from_uuid(uuid),
                    )
                    .await;

                sink.emit(ClientEvent::SessionResumed {
                    session_id: uuid.to_string(),
                    mode: SessionModeDto::Code,
                    messages,
                })
                .await;
                if let Some(usage) = restored_usage {
                    sink.emit(client::adapter::lowering::lower_current_usage(usage))
                        .await;
                }
                if let Some(message) = presence_warning {
                    sink.emit(ClientEvent::SystemNotice {
                        message,
                        is_error: false,
                    })
                    .await;
                }
                // resume_session restores the model/effort held by this exact
                // transcript. Publish that authoritative session state after
                // activation so clients do not keep showing the model selected
                // in whichever session happened to be open previously.
                let status = self.handle.get_status_snapshot().await;
                sink.emit(ClientEvent::StatusSnapshot {
                    snapshot: lower_status_snapshot(&status),
                })
                .await;
                if !status.model.is_empty() {
                    sink.emit(ClientEvent::ModelChanged {
                        model: lingxi_core::host::qualified_model_ref(
                            &status.model,
                            status.model_profile.as_deref(),
                        ),
                    })
                    .await;
                }
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::RequestExit => {
                self.handle.request_exit().await;
            }

            // ── Tasks ──────────────────────────────────────────────────────
            ClientCommand::TaskList {
                status_filter,
                request_id,
            } => {
                let filter = TaskListFilter {
                    status: status_filter.map(task_status_wire),
                };
                if let Some(request_id) = request_id {
                    emit_correlated_task_list(&*self.tasks, &*sink, filter, request_id).await;
                } else {
                    self.emit_task_list(filter, &*sink).await;
                }
            }
            ClientCommand::TaskOutput { task_id, offset } => {
                match self.tasks.output(&task_id, Some(offset)).await {
                    Ok(chunk) => {
                        let (id, content, total_lines, truncated) = lower_task_output_chunk(&chunk);
                        sink.emit(ClientEvent::TaskOutputChunk {
                            task_id: id,
                            content,
                            total_lines,
                            truncated,
                        })
                        .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("task output failed: {e}"),
                        })
                        .await;
                    }
                }
            }
            // A stop from the desktop client is the USER's — claude-code's
            // UI/control-channel `stopTask` passes `source:"user"` and inherits
            // the stop helper's `killedBy = "user"` default, so the killed
            // notification reads "was stopped by user".
            ClientCommand::TaskMessage { task_id, message } => {
                if !self.handle.workspace_trusted().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "Trust this workspace before messaging a task".into(),
                    })
                    .await;
                } else {
                    match self.tasks.send_human_task_message(&task_id, &message).await {
                        Ok(()) => {
                            sink.emit(ClientEvent::SystemNotice {
                                message: format!("Message accepted for task {task_id}"),
                                is_error: false,
                            })
                            .await
                        }
                        Err(error) => {
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Rejected,
                                message: format!("task message failed: {error}"),
                            })
                            .await
                        }
                    }
                }
            }
            ClientCommand::TaskStop { task_id } => {
                match self.tasks.kill_with_reason(&task_id, "user").await {
                    Ok(rec) => {
                        sink.emit(ClientEvent::TaskStatusChanged {
                            task_id: rec.task_id,
                            status: client::adapter::lowering::lower_task_status(&rec.status),
                            origin_session_id: None,
                            // A user stop is `killed`, never `failed` — no
                            // handler-reported reason to forward.
                            error: None,
                        })
                        .await;
                    }
                    Err(e) => {
                        sink.emit(ClientEvent::Error {
                            kind: ErrorKindDto::Internal,
                            message: format!("task stop failed: {e}"),
                        })
                        .await;
                    }
                }
            }

            // Workflow resume is a mobile/local-app host capability. The
            // desktop bridge has no workflow launcher bound into this router;
            // surface that fact as a protocol error instead of silently
            // dropping the command in the catch-all below.
            ClientCommand::ResumeWorkflow { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume_workflow is unavailable on this bridge".to_string(),
                })
                .await;
            }

            // Durable turn recovery (`attach_turn` / `resume_turn` /
            // `pause_turn`) is a MOBILE host capability: it needs the retained
            // per-turn event log and the recovery state machine that
            // `harness-runtime::mobile`'s host owns (`host.rs`'s `AttachTurn` /
            // `ResumeTurn` / `PauseTurn` arms). This bridge keeps no retained
            // event window and no `TurnRecoverySnapshotDto` state, so there is
            // nothing here to attach to, resume, or pause.
            //
            // Each of the three is a REQUEST-REPLY command on the mobile host —
            // the client sends it and waits for a `turn_recovery_state` (and,
            // for `attach_turn`, a `turn_event_replay` burst). Dropping them in
            // the catch-all below leaves such a client waiting forever for a
            // reply that will never come, so answer with the same explicit
            // typed rejection `ResumeWorkflow` uses. One arm per command so the
            // message names the command the client actually sent.
            ClientCommand::AttachTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "attach_turn is unavailable on this bridge".to_string(),
                })
                .await;
            }
            ClientCommand::ResumeTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "resume_turn is unavailable on this bridge".to_string(),
                })
                .await;
            }
            ClientCommand::PauseTurn { .. } => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "pause_turn is unavailable on this bridge".to_string(),
                })
                .await;
            }

            // ── Handled elsewhere / not routed by this seam ──────────────────
            //
            // `SendPrompt` + `Cancel` are the turn path (`TurnDriver`), and
            // `ApprovePermission`/`DenyPermission` the permission path — both on
            // `BridgeConnection` directly.
            // The `#[non_exhaustive]` enum also requires a catch-all.
            other => {
                tracing::debug!(
                    ?other,
                    "bridge-server: command not routed by EngineCommandRouter"
                );
            }
        }
    }
}

fn provider_id_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '-' | '_' | '.'))
        })
}

fn command_source_string(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Builtin => "builtin",
        CommandSource::Settings(lingxi_core::types::SettingsScope::User) => "user",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Project) => "project",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Local) => "local",
        CommandSource::Plugin => "plugin",
        CommandSource::Settings(lingxi_core::types::SettingsScope::Managed) => "managed",
        CommandSource::Mcp => "mcp",
        CommandSource::Bundled => "bundled",
    }
}

#[cfg(test)]
#[path = "router/tests/settings_patch_parsing_tests.rs"]
mod settings_patch_parsing_tests;

#[cfg(test)]
#[path = "router/tests/mcp_config_json_parsing_tests.rs"]
mod mcp_config_json_parsing_tests;

/// Round-9 review finding [2]: the Electron desktop's ONLY credential-add path
/// is [`ClientCommand::SetProviderCredential`] (Settings -> Provider
/// Credentials). It must tell Fusion's scoped catalog refresher about the
/// write, exactly the way the TUI key view does
/// (`apps/cli/src/mode.rs`'s `run_connect_action`); without it a key added in
/// Settings routes on the next ordinary turn but stays invisible to `/fusion`
/// for the rest of the engine process.
#[cfg(test)]
#[path = "router/tests/fusion_catalog_refresh_tests.rs"]
mod fusion_catalog_refresh_tests;

#[cfg(test)]
#[path = "router/tests/scheduled_chat_anchor_tests.rs"]
mod scheduled_chat_anchor_tests;

#[cfg(test)]
#[path = "router/tests/runtime_snapshot_tests.rs"]
mod runtime_snapshot_tests;

mod catalog;
mod configuration;
mod sessions;
mod settings;
mod tasks;

#[cfg(test)]
use settings::parse_settings_patch;
use tasks::emit_correlated_task_list;
use tasks::task_status_wire;
pub use tasks::TaskPoll;

#[cfg(test)]
use configuration::parse_mcp_config_json;
