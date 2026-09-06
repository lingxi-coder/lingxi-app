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
//! ([`platform_api::OrchestratorHandle`], [`platform_api::AuthHandle`],
//! [`platform_api::task_registry::TaskRegistryHandle`], and the slash dispatcher); a
//! test binds the SAME router over the engine's `MockOrchestratorHandle` / mock
//! task + auth handles, so the routing-and-lowering path under test is the
//! production one (no test-only router shim).
//!
//! Each command maps to its engine entry, and the engine's reply is lowered to
//! a [`ClientEvent`] (via the pure `client_adapter::lowering` fns — the single
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
use std::time::Duration;

use async_trait::async_trait;
use client_adapter::lowering::{
    lower_agent_info, lower_doctor_report, lower_hook_info, lower_mcp_server_info,
    lower_provider_model_catalog_entry, lower_skill_info, lower_status_snapshot,
    lower_task_output_chunk, lower_task_record,
};
use client_adapter::ClientEventSink;
use client_protocol::commands::{
    ClientCommand, HookAdminCommandDto, ListingKindDto, McpAdminCommandDto, McpScopeDto,
    PermissionBehaviorDto, PluginAdminCommandDto, SettingsDestinationDto, SkillAdminCommandDto,
};
use client_protocol::controls::{
    ConversationControlsDto, ReasoningControlStateDto, ReasoningSelectionDto,
};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use client_protocol::listings::{
    AuthStateDto, ConfigurationDomainDto, ConfigurationEffectDto, ConfigurationOperationStatusDto,
    SessionAgentSummaryDto, SessionModeDto, SlashCommandDto, TaskStatusDto,
};
use command_api::builtin_support::names::{core_description, is_palette_hidden};
use command_api::model::CommandSource;
use command_api::parser::parse_slash_command;
use command_api::registry::CommandRegistry;
use platform_api::auth::{AuthHandle, LoginInfo};
use platform_api::orchestrator::OrchestratorHandle;
use platform_api::task_registry::{TaskListFilter, TaskRegistryHandle};
use platform_api::SlashCommandDispatcher;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::mcp_bridge::McpPaths;
use crate::settings_bridge::{
    apply_patch, build_snapshot, lower_snapshot, permission_destination, permission_paths,
    permission_rule_from_wire, SettingsContext,
};
use crate::{hook_admin, mcp_admin, plugin_admin, skills_admin};

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
    fs: Arc<dyn platform_api::FileSystem>,
}

impl SessionStoreContext {
    /// Build a session-store context rooted at the desktop config directory and
    /// the connection's project cwd.
    #[must_use]
    pub fn new(
        lingxi_home: PathBuf,
        session_cwd: String,
        fs: Arc<dyn platform_api::FileSystem>,
    ) -> Self {
        Self {
            lingxi_home,
            session_cwd,
            fs,
        }
    }
}

/// The engine-routing seam for the full [`ClientCommand`] surface (everything
/// except the turn + permission path the [`crate::server::BridgeConnection`]
/// handles directly).
///
/// Abstracting routing behind a trait keeps the connection loop decoupled from
/// HOW the engine was assembled: the production server builds an
/// [`EngineCommandRouter`] from `engine_desktop::build`'s `DesktopRuntime`
/// handles, while a routing test builds the SAME router over the engine's mock
/// handles.
#[async_trait]
pub trait CommandRouter: Send + Sync + 'static {
    /// Route one decoded [`ClientCommand`], emitting any reply event(s) through
    /// `sink`. Pure side-effecting: it never blocks the caller for a turn (the
    /// turn path lives on [`crate::server::TurnDriver`]).
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>);

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

/// One already-dispatched slash result plus authoritative out-of-band changes
/// observed during that same dispatch. Keeping both together lets the bridge
/// connection route prompt commands without dispatching the command twice.
#[derive(Debug, Clone)]
pub struct SlashDispatchOutcome {
    /// The single engine dispatch result for the submitted command.
    pub result: platform_api::SlashDispatchResult,
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
    listings: &[platform_api::ModelListing],
) -> Vec<client_protocol::listings::ProviderModelCatalogEntryDto> {
    platform_api::provider_model_catalog(listings)
        .iter()
        .map(lower_provider_model_catalog_entry)
        .collect()
}

/// The production [`CommandRouter`]: wraps the real engine handles and lowers
/// each reply with the pure `client_adapter::lowering` parity fns.
pub struct EngineCommandRouter {
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
    /// Shared provider credential manager. Production bridge boot wires the
    /// exact manager used by the runtime; tests/embedded clients may omit it.
    credentials: Option<Arc<secret::CredentialManager>>,
    /// Settings-visible provider model directory assembled from the full
    /// provider config, including providers not currently routable.
    provider_model_catalog_listings: Vec<platform_api::ModelListing>,
    /// Packaged Desktop owns persistence in its signed credential broker; in
    /// that mode bridge credential mutations update only this process cache.
    provider_credentials_ephemeral: bool,
    /// HTTP transport used by provider connection probes. Production boot
    /// supplies the same guarded transport as the runtime's LLM clients.
    http: Option<Arc<dyn platform_api::HttpTransport>>,
    /// Optional layered-settings context backing the `Settings` listing.
    /// Production boot wires it from the desktop composition root; lightweight
    /// users of the routing seam may omit it, in which case the listing
    /// reports the missing context rather than emitting nothing.
    settings: Option<SettingsContext>,
    /// Optional MCP-scope roots backing `UpsertMcpServer` / `RemoveMcpServer`.
    /// Production boot wires it from the SAME `.mcp.json` / global-config
    /// paths `engine_desktop` resolves the read-side registry from;
    /// lightweight users of the routing seam may omit it, in which case the
    /// two commands report the missing context rather than doing nothing.
    mcp: Option<McpPaths>,
    /// Live MCP registry for Desktop-only configuration snapshots and hot reloads.
    mcp_registry: Option<Arc<mcp::McpRegistry>>,
    /// Live plugin runtime for Desktop-only refresh after install/config changes.
    plugin_runtime: Option<Arc<engine_desktop::PluginRuntime>>,
    /// Live hook registry for source-scoped hot swaps.
    hook_registry: Option<Arc<RwLock<hooks::HookRegistry>>>,
    /// Existing repo-root catalog reloader used for skills/plugins live refresh.
    repo_root_reloader: Option<Arc<dyn platform_api::RepoRootReloader>>,
    /// FileChanged watcher controller used to replace watch matchers after hook edits.
    file_changed_watcher: Option<engine_desktop::file_changed_watch::FileChangedWatcherController>,
    /// Set while a turn is in flight — `ClearSession` is rejected in this window
    /// (plan §2 mid-turn semantics).
    turn_active: AtomicBool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlashAuthoritySnapshot {
    session_id: String,
    model: String,
    permission_mode: Option<String>,
    auth: AuthStateDto,
    catalog: Option<Vec<SlashCommandDto>>,
}

fn session_agent_activity(messages: &[client_protocol::message::MessageDto]) -> Option<String> {
    messages
        .iter()
        .rev()
        .flat_map(|message| message.blocks.iter().rev())
        .find_map(|block| {
            let text = match block {
                client_protocol::message::MessageBlockDto::Text { text }
                | client_protocol::message::MessageBlockDto::Thinking { thinking: text, .. } => {
                    text
                }
                client_protocol::message::MessageBlockDto::ToolUse { tool, .. } => tool,
                client_protocol::message::MessageBlockDto::ToolResult { tool, .. } => tool,
                _ => return None,
            };
            let line = text.lines().find(|line| !line.trim().is_empty())?.trim();
            if line.is_empty()
                || matches!(
                    line,
                    "completed" | "cancelled" | "failed" | "idle" | "running"
                )
            {
                return None;
            }
            Some(line.chars().take(160).collect())
        })
}

async fn read_session_agent_summary(
    agent_id: String,
    root: &std::path::Path,
    path: &std::path::Path,
    raw: &[u8],
) -> Option<SessionAgentSummaryDto> {
    let messages = engine_desktop::session_agents::lower_transcript(raw);
    let mut status = "running".to_string();
    let mut name = None;
    let mut agent_type = None;
    let mut model = None;
    let mut model_profile = None;
    for line in raw
        .split(|byte| *byte == b'\n')
        .rev()
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        name = value
            .get("agent_name")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(name);
        agent_type = value
            .get("agent_type")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(agent_type);
        model = value
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(model);
        model_profile = value
            .get("model_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(model_profile);
        if let Some(value) = value.get("status").and_then(serde_json::Value::as_str) {
            status = match value {
                "completed" | "failed" | "killed" | "cancelled" | "idle" | "running" => {
                    value.to_string()
                }
                _ => "unknown".to_string(),
            };
            break;
        }
    }
    if let Some(parent) = path.parent() {
        let row_path = session::agent_rows::row_path(parent, &agent_id);
        let relative = row_path
            .strip_prefix(root)
            .ok()
            .map(std::path::Path::to_path_buf);
        let root = root.to_path_buf();
        let row = if let Some(relative) = relative {
            tokio::task::spawn_blocking(move || {
                platform_api::rooted_fs::read_to_string_limited(
                    &root,
                    &relative,
                    session::agent_rows::ROW_MAX_BYTES,
                )
                .ok()
                .and_then(|text| {
                    serde_json::from_str::<session::agent_rows::ParkedAgentRow>(&text).ok()
                })
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        if let Some(row) = row {
            name = name.or(row.request.name).or(row.request.description);
            agent_type = agent_type.or_else(|| {
                (!row.request.subagent_type.is_empty()).then_some(row.request.subagent_type)
            });
            model = model.or(row.request.model);
            model_profile = model_profile.or(row.request.model_profile);
            if status == "running" {
                status = "idle".to_string();
            }
        }
    }
    let agent_type = agent_type.unwrap_or_else(|| "unknown".to_string());
    let name = name
        .or_else(|| (agent_type != "unknown").then(|| agent_type.clone()))
        .unwrap_or_else(|| {
            agent_id
                .strip_prefix("agent:")
                .unwrap_or(&agent_id)
                .chars()
                .take(8)
                .collect()
        });
    let updated_at_ms = tokio::fs::symlink_metadata(path)
        .await
        .ok()
        .filter(|metadata| metadata.is_file())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    Some(SessionAgentSummaryDto {
        agent_id,
        name,
        agent_type,
        model,
        model_profile,
        status,
        latest_activity: session_agent_activity(&messages),
        updated_at_ms,
    })
}

impl EngineCommandRouter {
    async fn dispatch_desktop_slash(&self, raw: &str) -> Option<platform_api::SlashDispatchResult> {
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
                Some(platform_api::SlashDispatchResult::RunAsTurn { prompt })
            }
            "plan" => Some(self.dispatch_desktop_plan(&parsed.raw_args).await),
            _ => None,
        }
    }

    async fn dispatch_desktop_plan(&self, args: &str) -> platform_api::SlashDispatchResult {
        let previous_permission = self
            .handle
            .permission_mode()
            .await
            .unwrap_or_else(|| "default".to_string());
        let was_plan_mode = self.handle.plan_mode().await;

        if previous_permission != "plan" {
            if let Err(error) = self.handle.set_permission_mode("plan").await {
                return platform_api::SlashDispatchResult::Handled {
                    display: format!("Could not enable plan mode: {error}"),
                };
            }
            platform_api::live_sessions::set_process_permission_mode("plan", false);
        }
        if !was_plan_mode {
            if let Err(error) = self.handle.set_plan_mode(true).await {
                if previous_permission != "plan" {
                    let _ = self.handle.set_permission_mode(&previous_permission).await;
                    platform_api::live_sessions::set_process_permission_mode(
                        &previous_permission,
                        previous_permission == "bypassPermissions",
                    );
                }
                return platform_api::SlashDispatchResult::Handled {
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
            platform_api::SlashDispatchResult::Handled {
                display: display.to_string(),
            }
        } else {
            platform_api::SlashDispatchResult::RunAsTurn {
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
            handle,
            auth,
            tasks,
            dispatcher,
            slash_registry,
            session_store: None,
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
    pub fn with_credentials(mut self, credentials: Arc<secret::CredentialManager>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    #[must_use]
    pub fn with_provider_model_catalog_listings(
        mut self,
        listings: Vec<platform_api::ModelListing>,
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
    pub fn with_http(mut self, http: Arc<dyn platform_api::HttpTransport>) -> Self {
        self.http = Some(http);
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
        plugin_runtime: Option<Arc<engine_desktop::PluginRuntime>>,
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
        repo_root_reloader: Arc<dyn platform_api::RepoRootReloader>,
    ) -> Self {
        self.repo_root_reloader = Some(repo_root_reloader);
        self
    }

    /// Attach the FileChanged watcher controller for replacing settings-derived
    /// matcher sets after hook edits.
    #[must_use]
    pub fn with_file_changed_watcher(
        mut self,
        watcher: Option<engine_desktop::file_changed_watch::FileChangedWatcherController>,
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

    async fn emit_controls_snapshot(&self, sink: &dyn ClientEventSink) {
        let Some(controls) = self.handle.conversation_controls().await else {
            return;
        };
        let fast_mode = self.handle.fast_mode().await;
        let reasoning = controls.reasoning_spec.clone();
        sink.emit(ClientEvent::ConversationControlsChanged {
            controls: ConversationControlsDto {
                qualified_model: controls.model_reference.clone(),
                permission: client_protocol::controls::PermissionControlStateDto {
                    requested: controls.permission.requested,
                    effective: controls.permission.effective,
                    options: controls
                        .permission
                        .modes
                        .into_iter()
                        .map(|mode| client_protocol::controls::PermissionModeOptionDto {
                            mode: mode.mode,
                            available: mode.available,
                            disabled_reason: mode.disabled_reason.map(|code| {
                                client_protocol::controls::ControlDisabledReasonDto {
                                    code,
                                    message: None,
                                }
                            }),
                        })
                        .collect(),
                },
                reasoning: ReasoningControlStateDto {
                    requested: lower_reasoning_selection(&controls.requested_reasoning_selection),
                    effective: lower_reasoning_selection(&controls.effective_reasoning_selection),
                    spec: client_adapter::lowering::lower_reasoning_control_spec(&reasoning),
                },
            },
        })
        .await;

        sink.emit(ClientEvent::FastModeChanged { enabled: fast_mode })
            .await;
    }

    /// Read, merge and emit the layered settings. This listing was defined in
    /// the protocol from the start and, until now, matched the same do-nothing
    /// arm as `Memory`: it logged a debug line and emitted nothing at all.
    ///
    /// Without a settings context there is nothing to read, so it says so
    /// rather than reverting to silence — silence is precisely the defect this
    /// path exists to remove.
    async fn emit_settings_snapshot(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "settings listing unavailable: this connection was built without a \
                          settings context"
                    .to_string(),
            })
            .await;
            return;
        };

        let snapshot = build_snapshot(
            &context.paths,
            context.active_snapshot(),
            context.managed.clone(),
        );
        let lowered = lower_snapshot(&snapshot);
        sink.emit(ClientEvent::SettingsSnapshot {
            effective_json: lowered.effective_json,
            provenance_json: lowered.provenance_json,
            files_json: Some(lowered.files_json),
            active_json: Some(lowered.active_json),
            locked: Some(lowered.locked),
            layers_json: Some(lowered.layers_json),
            merged_keys: Some(lowered.merged_keys),
        })
        .await;
    }

    /// Decode `patch_json` via [`parse_settings_patch`] and apply it to
    /// `destination` through [`crate::settings_bridge::apply_patch`]. On
    /// success, resends the settings snapshot so the caller sees the write it
    /// just made reflected back (rather than requiring a separate
    /// `RefreshListings{Settings}` round-trip). On any failure — no settings
    /// context, a patch that fails to parse (see [`parse_settings_patch`]), or
    /// `apply_patch`'s own errors (reserved key, broken destination file,
    /// write failure) — emits the SAME [`ClientEvent::Error`] path the rest of
    /// this router uses, rather than a dedicated failure event.
    async fn apply_settings_patch(
        &self,
        destination: SettingsDestinationDto,
        patch_json: &str,
        sink: &dyn ClientEventSink,
    ) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "settings update unavailable: this connection was built without a \
                          settings context"
                    .to_string(),
            })
            .await;
            return;
        };

        let patch = match parse_settings_patch(patch_json) {
            Ok(patch) => patch,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message,
                })
                .await;
                return;
            }
        };

        match apply_patch(&context.paths, destination, patch) {
            Ok(()) => {
                self.emit_settings_snapshot(sink).await;
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await;
            }
        }
    }

    /// Shared preflight for the three persisted-permission commands
    /// ([`ClientCommand::UpdatePermissionRules`],
    /// [`ClientCommand::SetDefaultPermissionMode`],
    /// [`ClientCommand::UpdateWorkspaceDirectories`]): resolve the active
    /// [`SettingsContext`] into the `permission` crate's two-root paths, or
    /// report the same "no settings context" gap
    /// [`Self::apply_settings_patch`] reports on the generic path.
    async fn require_permission_paths(
        &self,
        sink: &dyn ClientEventSink,
    ) -> Option<permission::PermissionPaths> {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "permission update unavailable: this connection was built without a \
                          settings context"
                    .to_string(),
            })
            .await;
            return None;
        };
        Some(permission_paths(&context.paths))
    }

    /// Route [`ClientCommand::UpdatePermissionRules`] to
    /// `permission::persist_permission_rule_set` — never reimplementing its
    /// per-destination lock, atomic write, or alias-normalizing de-dup. `add`
    /// and `remove` are each persisted in their own call (the persister does
    /// one same-behavior set per transaction); a rule string is never
    /// rejected here — [`permission_rule_from_wire`] parses infallibly,
    /// matching claude-code's own parser.
    async fn apply_permission_rule_update(
        &self,
        destination: SettingsDestinationDto,
        behavior: PermissionBehaviorDto,
        add: Vec<String>,
        remove: Vec<String>,
        sink: &dyn ClientEventSink,
    ) {
        let Some(paths) = self.require_permission_paths(sink).await else {
            return;
        };
        let dest = permission_destination(destination);
        let to_add: Vec<permission::PermissionRule> = add
            .iter()
            .map(|raw| permission_rule_from_wire(raw, behavior, destination))
            .collect();
        let to_remove: Vec<permission::PermissionRule> = remove
            .iter()
            .map(|raw| permission_rule_from_wire(raw, behavior, destination))
            .collect();

        let mut changed = false;
        for (rules, add_flag) in [(&to_add, true), (&to_remove, false)] {
            if rules.is_empty() {
                continue;
            }
            match permission::persist_permission_rule_set(rules, add_flag, dest, &paths).await {
                Ok(did_change) => changed |= did_change,
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("failed to persist permission rules: {error}"),
                    })
                    .await;
                    // `add` and `remove` are two SEPARATE transactions. If the
                    // `add` half already landed durably before this half
                    // errored, the file has genuinely changed — telling the
                    // caller only "it failed" would leave its view of
                    // settings stale and silently wrong. The error still says
                    // the operation did not complete; the snapshot says what
                    // is actually on disk now. Both are true.
                    if changed {
                        self.emit_settings_snapshot(sink).await;
                    }
                    return;
                }
            }
        }

        if changed {
            self.emit_settings_snapshot(sink).await;
        } else {
            // Ok(false) with no error means nothing on disk actually moved —
            // an empty add/remove set, or every entry already matched what
            // was there. Silence here would look identical to a successful
            // write from the caller's side, so it is reported rather than
            // swallowed.
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Rejected,
                message: "no permission rule changed: `add`/`remove` were empty, or every \
                          entry already matched the file"
                    .to_string(),
            })
            .await;
        }
    }

    /// Route [`ClientCommand::SetDefaultPermissionMode`] to
    /// `permission::persist_permission_mode`. Distinct from the
    /// session-scoped [`ClientCommand::SetPermissionMode`]: this writes the
    /// DEFAULT mode a future session boots into. `persist_permission_mode`
    /// deliberately refuses to persist `"bypassPermissions"` (a security
    /// property — persisting it would silently re-enter bypass mode on the
    /// next session load), and that refusal is reported here rather than
    /// swallowed.
    async fn apply_default_permission_mode(
        &self,
        destination: SettingsDestinationDto,
        mode: String,
        sink: &dyn ClientEventSink,
    ) {
        let Some(paths) = self.require_permission_paths(sink).await else {
            return;
        };
        let dest = permission_destination(destination);
        match permission::persist_permission_mode(&mode, dest, &paths).await {
            Ok(true) => self.emit_settings_snapshot(sink).await,
            Ok(false) if mode == "bypassPermissions" => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: "`bypassPermissions` is session-scoped and is deliberately never \
                              persisted as the default mode"
                        .to_string(),
                })
                .await;
            }
            Ok(false) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Rejected,
                    message: format!(
                        "default permission mode was not persisted: `{mode}` is unrecognized, \
                         or already the current default"
                    ),
                })
                .await;
            }
            Err(error) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: format!("failed to persist default permission mode: {error}"),
                })
                .await;
            }
        }
    }

    /// Route [`ClientCommand::UpdateWorkspaceDirectories`] to
    /// `permission::persist_workspace_directories`, the same
    /// add-then-remove, report-if-nothing-changed shape as
    /// [`Self::apply_permission_rule_update`].
    async fn apply_workspace_directories_update(
        &self,
        destination: SettingsDestinationDto,
        add: Vec<String>,
        remove: Vec<String>,
        sink: &dyn ClientEventSink,
    ) {
        let Some(paths) = self.require_permission_paths(sink).await else {
            return;
        };
        let dest = permission_destination(destination);

        let mut changed = false;
        for (directories, add_flag) in [(&add, true), (&remove, false)] {
            if directories.is_empty() {
                continue;
            }
            match permission::persist_workspace_directories(directories, add_flag, dest, &paths)
                .await
            {
                Ok(did_change) => changed |= did_change,
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("failed to persist workspace directories: {error}"),
                    })
                    .await;
                    // Same honesty property as `apply_permission_rule_update`:
                    // `add` and `remove` are two SEPARATE transactions, so an
                    // `add` that already landed before `remove` errored is a
                    // real, durable change. Report it alongside the error
                    // rather than leaving the caller's view stale.
                    if changed {
                        self.emit_settings_snapshot(sink).await;
                    }
                    return;
                }
            }
        }

        if changed {
            self.emit_settings_snapshot(sink).await;
        } else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Rejected,
                message: "no workspace directory changed: `add`/`remove` were empty, or every \
                          entry already matched the file"
                    .to_string(),
            })
            .await;
        }
    }

    /// Route [`ClientCommand::UpsertMcpServer`] to `mcp_bridge::upsert_server`.
    /// `config_json` is decoded and validated as a JSON object HERE (a
    /// malformed client payload is [`ErrorKindDto::Protocol`], matching
    /// [`apply_settings_patch`](Self::apply_settings_patch)'s
    /// `patch_json` handling); a write that fails once the shape is valid
    /// (broken destination file, non-object `mcpServers`, legacy bare-map
    /// `.mcp.json`, I/O failure) is [`ErrorKindDto::Internal`], matching every
    /// other write-failure path in this router. There is no dedicated success
    /// event — the desktop already has the wired `RefreshListings{Mcp}` path
    /// to observe the change (decision: adding a parallel listing here would
    /// duplicate that path, and the live `McpRegistry` snapshot it reads is
    /// not reloaded from disk by a bare file write, so re-emitting it here
    /// would not even show the new value).
    async fn apply_mcp_upsert(
        &self,
        scope: McpScopeDto,
        name: &str,
        config_json: &str,
        sink: &dyn ClientEventSink,
    ) {
        let Some(mcp) = self.mcp.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP server update unavailable: this connection was built without an \
                          MCP context"
                    .to_string(),
            })
            .await;
            return;
        };

        if name.trim().is_empty() {
            // `mcp::json_config::build_servers_from_map` would turn a `""`
            // map key into a nameless server entry — reject at the wire
            // boundary rather than writing it. (Reserved-name collisions,
            // e.g. `computer-use` per `mcp/src/server_gate.rs`, are
            // deliberately NOT blocked here: a hardcoded name blocklist in
            // the writer would be a second source of truth that drifts from
            // `server_gate`'s own allowlist.)
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Protocol,
                message: "MCP server name must not be empty".to_string(),
            })
            .await;
            return;
        }

        let config = match parse_mcp_config_json(config_json) {
            Ok(config) => config,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message,
                })
                .await;
                return;
            }
        };

        if let Err(message) = crate::mcp_bridge::upsert_server(mcp, scope, name, config) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message,
            })
            .await;
        }
    }

    /// Route [`ClientCommand::RemoveMcpServer`] to `mcp_bridge::remove_server`.
    /// Same context/error-kind shape as [`Self::apply_mcp_upsert`], minus the
    /// `config_json` decode (there is nothing to parse for a removal).
    async fn apply_mcp_remove(&self, scope: McpScopeDto, name: &str, sink: &dyn ClientEventSink) {
        let Some(mcp) = self.mcp.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP server update unavailable: this connection was built without an \
                          MCP context"
                    .to_string(),
            })
            .await;
            return;
        };

        if let Err(message) = crate::mcp_bridge::remove_server(mcp, scope, name) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message,
            })
            .await;
        }
    }

    async fn emit_configuration_operation(
        &self,
        sink: &dyn ClientEventSink,
        domain: ConfigurationDomainDto,
        operation_id: u64,
        status: ConfigurationOperationStatusDto,
        effect: ConfigurationEffectDto,
        message: Option<String>,
        details_json: Option<String>,
    ) {
        sink.emit(ClientEvent::ConfigurationOperation {
            domain,
            operation_id,
            status,
            effect,
            message,
            details_json,
        })
        .await;
    }

    async fn emit_skill_catalog(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.skills_admin_context() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "skill catalog unavailable: this connection was built without a settings context".to_string(),
            }).await;
            return;
        };
        match skills_admin::catalog_json(&context) {
            Ok(catalog_json) => {
                let catalog_json = match serde_json::from_str::<serde_json::Value>(&catalog_json) {
                    Ok(mut catalog) => {
                        let runtime_skills = self.handle.list_skills().await;
                        let entries = catalog
                            .get_mut("entries")
                            .and_then(serde_json::Value::as_array_mut);
                        if let Some(entries) = entries {
                            let known = entries
                                .iter()
                                .filter_map(|entry| {
                                    entry.get("directory").and_then(serde_json::Value::as_str)
                                })
                                .map(ToOwned::to_owned)
                                .collect::<std::collections::BTreeSet<_>>();
                            for skill in runtime_skills {
                                let directory = skill.source_dir.to_string_lossy().into_owned();
                                if known.contains(&directory) {
                                    continue;
                                }
                                let source = if directory.starts_with('<') {
                                    "mcp"
                                } else if directory.starts_with(
                                    &context
                                        .lingxi_home
                                        .join("plugins")
                                        .to_string_lossy()
                                        .into_owned(),
                                ) {
                                    "plugin"
                                } else {
                                    "runtime"
                                };
                                entries.push(serde_json::json!({
                                    "id": format!("runtime:{}", skill.name),
                                    "name": skill.name,
                                    "source": source,
                                    "rootDir": directory,
                                    "directory": directory,
                                    "writable": false,
                                    "readonlyReason": format!("{source} skills are runtime-derived and read-only."),
                                    "diagnosticsJson": "{\"status\":\"runtime\"}"
                                }));
                            }
                            if let Some(registry) = self.slash_registry.as_ref() {
                                let registry = registry.read().await;
                                for command in registry.list_all() {
                                    match &command.kind {
                                        command_api::SlashCommandKind::Plugin {
                                            plugin_id,
                                            file_path,
                                            ..
                                        } if command.skill_root.is_some() => {
                                            let directory = command
                                                .skill_root
                                                .as_deref()
                                                .or_else(|| file_path.parent())
                                                .unwrap_or(file_path)
                                                .to_string_lossy()
                                                .into_owned();
                                            let revision =
                                                std::fs::read(file_path).ok().map(|bytes| {
                                                    crate::config_admin::sha256_hex(&bytes)
                                                });
                                            entries.push(serde_json::json!({
                                                "id": format!("command:{}", command.name),
                                                "name": command.name,
                                                "source": "plugin",
                                                "pluginOwner": plugin_id.to_string(),
                                                "rootDir": directory,
                                                "directory": directory,
                                                "writable": false,
                                                "readonlyReason": "Plugin skills are managed by their owning plugin.",
                                                "revision": revision,
                                                "description": command.description,
                                                "whenToUse": command.when_to_use,
                                                "diagnosticsJson": "{\"ok\":true,\"issues\":[]}",
                                            }));
                                        }
                                        command_api::SlashCommandKind::Mcp {
                                            connection_id,
                                            prompt_name,
                                            ..
                                        } => {
                                            let directory =
                                                format!("<mcp:{connection_id}:{prompt_name}>");
                                            entries.push(serde_json::json!({
                                                "id": format!("command:{}", command.name),
                                                "name": command.name,
                                                "source": "mcp",
                                                "rootDir": directory,
                                                "directory": directory,
                                                "writable": false,
                                                "readonlyReason": "MCP skills are provided by the connected server.",
                                                "description": command.description,
                                                "whenToUse": command.when_to_use,
                                                "diagnosticsJson": "{\"status\":\"runtime\"}",
                                            }));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        catalog.to_string()
                    }
                    Err(_) => catalog_json,
                };
                sink.emit(ClientEvent::SkillCatalog { catalog_json }).await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }

    async fn emit_skill_document(&self, skill_id: &str, sink: &dyn ClientEventSink) {
        let Some(context) = self.skills_admin_context() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "skill document unavailable: this connection was built without a settings context".to_string(),
            }).await;
            return;
        };
        if let Some(name) = skill_id.strip_prefix("command:") {
            let command = match self.slash_registry.as_ref() {
                Some(registry) => registry.read().await.resolve(name).cloned(),
                None => None,
            };
            if let Some(command) = command {
                let document = match command.kind {
                    command_api::SlashCommandKind::Plugin {
                        plugin_id,
                        file_path,
                        ..
                    } if command.skill_root.is_some() => {
                        let markdown = std::fs::read_to_string(&file_path).unwrap_or_default();
                        let directory = command
                            .skill_root
                            .as_deref()
                            .or_else(|| file_path.parent())
                            .unwrap_or(&file_path)
                            .to_string_lossy()
                            .into_owned();
                        serde_json::json!({
                            "id": skill_id,
                            "name": command.name,
                            "source": "plugin",
                            "pluginOwner": plugin_id.to_string(),
                            "directory": directory,
                            "rootDir": directory,
                            "markdown": markdown,
                            "writable": false,
                            "revision": crate::config_admin::sha256_hex(markdown.as_bytes()),
                            "readonlyReason": "Plugin skills are managed by their owning plugin.",
                            "diagnosticsJson": "{\"ok\":true,\"issues\":[]}",
                        })
                    }
                    command_api::SlashCommandKind::Mcp {
                        connection_id,
                        prompt_name,
                        ..
                    } => {
                        let directory = format!("<mcp:{connection_id}:{prompt_name}>");
                        serde_json::json!({
                            "id": skill_id,
                            "name": command.name,
                            "source": "mcp",
                            "directory": directory,
                            "rootDir": directory,
                            "markdown": "",
                            "writable": false,
                            "revision": crate::config_admin::sha256_hex(&[]),
                            "readonlyReason": "MCP skills are provided by the connected server.",
                            "diagnosticsJson": "{\"status\":\"runtime\"}",
                        })
                    }
                    _ => serde_json::Value::Null,
                };
                if !document.is_null() {
                    sink.emit(ClientEvent::SkillDocument {
                        document_json: document.to_string(),
                    })
                    .await;
                    return;
                }
            }
        }
        if let Some(name) = skill_id.strip_prefix("runtime:") {
            if let Some(skill) = self
                .handle
                .list_skills()
                .await
                .into_iter()
                .find(|skill| skill.name == name)
            {
                let directory = skill.source_dir.to_string_lossy().into_owned();
                let source = if directory.starts_with('<') {
                    "mcp"
                } else if directory.starts_with(
                    &context
                        .lingxi_home
                        .join("plugins")
                        .to_string_lossy()
                        .into_owned(),
                ) {
                    "plugin"
                } else {
                    "runtime"
                };
                let document_json = serde_json::json!({
                    "id": skill_id,
                    "name": skill.name,
                    "source": source,
                    "directory": directory,
                    "rootDir": directory,
                    "markdown": "",
                    "writable": false,
                    "revision": crate::config_admin::sha256_hex(&[]),
                    "readonlyReason": format!("{source} skills are runtime-derived and read-only."),
                    "diagnosticsJson": "{\"status\":\"runtime\"}"
                })
                .to_string();
                sink.emit(ClientEvent::SkillDocument { document_json })
                    .await;
                return;
            }
        }
        match skills_admin::document_json(skill_id, &context) {
            Ok(document_json) => {
                sink.emit(ClientEvent::SkillDocument { document_json })
                    .await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }

    fn skills_admin_context(&self) -> Option<skills_admin::SkillsAdminContext> {
        self.settings
            .as_ref()
            .map(|context| skills_admin::SkillsAdminContext {
                cwd: context.paths.project_dir.clone(),
                lingxi_home: context.paths.lingxi_home.clone(),
            })
    }

    async fn refresh_skills_runtime(&self) -> Result<(), String> {
        let Some(reloader) = self.repo_root_reloader.as_ref() else {
            return Err("no skill catalog reloader wired".to_string());
        };
        let root = self
            .settings
            .as_ref()
            .map(|settings| settings.paths.project_dir.clone())
            .ok_or_else(|| "no skill settings context wired".to_string())?;
        let outcome = reloader
            .reload(platform_api::RepoRootReloadRequest {
                root,
                reload_skills: true,
                reload_plugins: false,
            })
            .await;
        if outcome.skills_reloaded && outcome.errors.is_empty() {
            Ok(())
        } else if outcome.errors.is_empty() {
            Err("skill catalog did not confirm a live reload".to_string())
        } else {
            Err(outcome.errors.join("; "))
        }
    }

    async fn apply_skill_admin(&self, command: SkillAdminCommandDto, sink: &dyn ClientEventSink) {
        match command.action.as_str() {
            "get_catalog" => self.emit_skill_catalog(sink).await,
            "get_document" => {
                let Some(skill_id) = command.target.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "skill document target is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_skill_document(skill_id, sink).await;
            }
            "save_document" | "create_skill" | "move_skill" | "trash_skill" | "restore_skill"
            | "purge_trash_skill" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "skill mutation operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Skill,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Applying skill change…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .skills_admin_context()
                    .ok_or_else(|| "skill admin unavailable: missing settings context".to_string())
                    .and_then(|context| skills_admin::handle(command.clone(), &context));
                match result {
                    Ok(skills_admin::SkillAdminOutcome::Changed {
                        catalog_json,
                        document_json,
                    }) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_skills_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Applied skill change.".to_string()),
                            None,
                        )
                        .await;
                        sink.emit(ClientEvent::SkillCatalog { catalog_json }).await;
                        if let Some(document_json) = document_json {
                            sink.emit(ClientEvent::SkillDocument { document_json })
                                .await;
                        }
                        self.emit_listing(ListingKindDto::Skills, sink).await;
                    }
                    Ok(_) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some("skill mutation returned no change payload".to_string()),
                            None,
                        )
                        .await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await;
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported skill admin action: {action}"),
                })
                .await
            }
        }
    }

    async fn emit_mcp_configuration_snapshot(&self, sink: &dyn ClientEventSink) {
        let (Some(context), Some(paths)) = (self.settings.as_ref(), self.mcp.as_ref()) else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP configuration snapshot unavailable: missing settings or MCP context"
                    .to_string(),
            })
            .await;
            return;
        };
        let mut runtime_servers = Vec::new();
        for server in self.handle.list_mcp_servers().await {
            let config = match self.mcp_registry.as_ref() {
                Some(registry) => registry.get_config(&server.name).await,
                None => None,
            };
            let (source, writable, read_only_reason) = match config.as_ref() {
                Some(config)
                    if config.metadata.agent_source == Some(mcp::McpAgentSource::Plugin) =>
                {
                    ("plugin", false, Some("由插件注入；请在 Plugins 设置中管理"))
                }
                Some(config) => match config.scope {
                    mcp::ConfigScope::User => ("user", true, None),
                    mcp::ConfigScope::Local => ("local", true, None),
                    mcp::ConfigScope::Project => ("project", true, None),
                    mcp::ConfigScope::Dynamic => ("dynamic", false, Some("由当前会话动态注入")),
                    mcp::ConfigScope::Enterprise => ("enterprise", false, Some("由企业配置管理")),
                    mcp::ConfigScope::ClaudeAi => {
                        ("claude_ai", false, Some("由 Claude.ai 连接提供"))
                    }
                    mcp::ConfigScope::Managed => ("managed", false, Some("由管理员策略管理")),
                    mcp::ConfigScope::Agent => ("agent", false, Some("由 Agent frontmatter 注入")),
                },
                None => ("runtime", false, Some("仅存在于当前运行态")),
            };
            runtime_servers.push(serde_json::json!({
                "name": server.name,
                "status": match server.status {
                    platform_api::McpStatus::Connected => serde_json::json!("connected"),
                    platform_api::McpStatus::Disconnected => serde_json::json!("disconnected"),
                    platform_api::McpStatus::Error(reason) => serde_json::json!({ "type": "error", "reason": reason }),
                },
                "transport": server.transport,
                "source": source,
                "writable": writable,
                "read_only_reason": read_only_reason,
            }));
        }
        match mcp_admin::snapshot_json(context, paths, runtime_servers) {
            Ok(snapshot_json) => {
                sink.emit(ClientEvent::McpConfigurationSnapshot { snapshot_json })
                    .await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }

    async fn apply_mcp_admin(&self, command: McpAdminCommandDto, sink: &dyn ClientEventSink) {
        match command.action.as_str() {
            "get_snapshot" => self.emit_mcp_configuration_snapshot(sink).await,
            "save_server" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving MCP server…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .mcp
                    .as_ref()
                    .ok_or_else(|| "MCP save unavailable: missing MCP context".to_string())
                    .and_then(|paths| {
                        mcp_admin::save_server_entry(paths, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_mcp_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved MCP server configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "remove_server" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Removing MCP server…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .mcp
                    .as_ref()
                    .ok_or_else(|| "MCP remove unavailable: missing MCP context".to_string())
                    .and_then(|paths| {
                        mcp_admin::remove_server_entry(paths, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_mcp_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Removed MCP server configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "set_approval" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving MCP approval…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .settings
                    .as_ref()
                    .ok_or_else(|| "MCP approval unavailable: missing settings context".to_string())
                    .and_then(|context| {
                        mcp_admin::set_project_approval(context, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            match self.refresh_mcp_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledMcpjsonServers",
                                            "disabledMcpjsonServers",
                                            "enableAllProjectMcpServers",
                                        ]);
                                    }
                                    ConfigurationEffectDto::Applied
                                }
                                Err(_) => ConfigurationEffectDto::RestartRequired,
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved project MCP approval.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported MCP admin action: {action}"),
                })
                .await
            }
        }
    }

    async fn emit_plugin_catalog(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "plugin catalog unavailable: missing settings context".to_string(),
            })
            .await;
            return;
        };
        match plugin_admin::catalog_json(context, self.credentials.as_deref()).await {
            Ok(catalog_json) => sink.emit(ClientEvent::PluginCatalog { catalog_json }).await,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }

    async fn refresh_plugins_runtime(&self) -> Result<(), String> {
        let Some(runtime) = self.plugin_runtime.as_ref() else {
            return Err("plugin runtime is unavailable".to_string());
        };
        let counts = runtime.refresh().await;
        if counts.errors == 0 {
            Ok(())
        } else {
            Err(format!(
                "plugin runtime refresh reported {} component error(s)",
                counts.errors
            ))
        }
    }

    async fn refresh_mcp_runtime(&self) -> Result<(), String> {
        let Some(registry) = self.mcp_registry.as_ref() else {
            return Err("MCP registry is unavailable".to_string());
        };
        let Some(paths) = self.mcp.as_ref() else {
            return Err("MCP paths are unavailable".to_string());
        };
        crate::mcp_bridge::reconcile_writable_servers(registry, paths).await
    }

    async fn apply_plugin_admin(&self, command: PluginAdminCommandDto, sink: &dyn ClientEventSink) {
        match command.action.as_str() {
            "get_catalog" => self.emit_plugin_catalog(sink).await,
            "preview_operation" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin preview operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin preview payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                let details = plugin_admin::preview_operation(
                    self.settings.as_ref(),
                    command.revision.as_deref(),
                    payload_json,
                )
                .await;
                match details {
                    Ok(details_json) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            ConfigurationEffectDto::NotApplicable,
                            Some("Plugin operation preview ready.".to_string()),
                            Some(details_json),
                        )
                        .await
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "apply_operation" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Plugin,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Applying plugin operation…".to_string()),
                    None,
                )
                .await;
                let result = match self.settings.as_ref() {
                    Some(context) => {
                        plugin_admin::apply_operation(context, revision_sha256, payload_json).await
                    }
                    None => Err("plugin apply unavailable: missing settings context".to_string()),
                };
                match result {
                    Ok(operation_message) => {
                        let (effect, reload_error) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
                        } else {
                            match self.refresh_plugins_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ]);
                                    }
                                    (ConfigurationEffectDto::Applied, None)
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error))
                                }
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some(operation_message),
                            reload_error.map(|error| {
                                serde_json::json!({ "reloadError": error }).to_string()
                            }),
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_plugin_catalog(sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "save_config" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Plugin,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving plugin settings…".to_string()),
                    None,
                )
                .await;
                let result = match self.settings.as_ref() {
                    Some(context) => {
                        plugin_admin::save_config(context, revision_sha256, payload_json).await
                    }
                    None => {
                        Err("plugin config save unavailable: missing settings context".to_string())
                    }
                };
                match result {
                    Ok(()) => {
                        let (effect, reload_error) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
                        } else {
                            match self.refresh_plugins_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ]);
                                    }
                                    (ConfigurationEffectDto::Applied, None)
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error))
                                }
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved plugin settings.".to_string()),
                            reload_error.map(|error| {
                                serde_json::json!({ "reloadError": error }).to_string()
                            }),
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_plugin_catalog(sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported plugin admin action: {action}"),
                })
                .await
            }
        }
    }

    async fn emit_hook_document(&self, scope: Option<&str>, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "hook document unavailable: missing settings context".to_string(),
            })
            .await;
            return;
        };
        match hook_admin::document_json(context, scope) {
            Ok(details_json) => {
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Hook,
                    0,
                    ConfigurationOperationStatusDto::Succeeded,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Hook document loaded.".to_string()),
                    Some(details_json),
                )
                .await;
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }

    async fn apply_hook_admin(&self, command: HookAdminCommandDto, sink: &dyn ClientEventSink) {
        match command.action.as_str() {
            "get_document" => {
                self.emit_hook_document(command.scope.as_deref(), sink)
                    .await
            }
            "validate_document" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook validate operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook validate payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                match hook_admin::validate_document(payload_json) {
                    Ok(()) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            ConfigurationEffectDto::NotApplicable,
                            Some("Hook document is valid.".to_string()),
                            None,
                        )
                        .await
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "save_document" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Hook,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving hooks…".to_string()),
                    None,
                )
                .await;
                let candidate = hook_admin::runtime_candidate(payload_json);
                let result = candidate.and_then(|candidate| {
                    self.settings
                        .as_ref()
                        .ok_or_else(|| {
                            "hook save unavailable: missing settings context".to_string()
                        })
                        .and_then(|context| {
                            hook_admin::save_document(context, revision_sha256, payload_json)
                        })
                        .map(|_| candidate)
                });
                match result {
                    Ok(candidate) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else if let Some(registry) = self.hook_registry.as_ref() {
                            let matchers = {
                                let mut registry = registry.write().await;
                                registry.replace_source_hooks(candidate.source, candidate.hooks);
                                registry.file_changed_matchers()
                            };
                            if let Some(watcher) = self.file_changed_watcher.as_ref() {
                                if let Some(settings) = self.settings.as_ref() {
                                    watcher.replace_matchers(
                                        matchers,
                                        settings.paths.project_dir.clone(),
                                    );
                                }
                            }
                            if let Some(settings) = self.settings.as_ref() {
                                settings.mark_keys_applied(&["hooks"]);
                            }
                            ConfigurationEffectDto::Applied
                        } else {
                            ConfigurationEffectDto::RestartRequired
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved hooks configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Hooks, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported hook admin action: {action}"),
                })
                .await
            }
        }
    }

    async fn emit_provider_credential_status(
        &self,
        operation_id: u64,
        provider_ids: &[String],
        preview_provider_ids: &[String],
        operation_error: Option<String>,
        sink: &dyn ClientEventSink,
    ) {
        let Some(credentials) = self.credentials.as_ref() else {
            sink.emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids: Vec::new(),
                unavailable_provider_ids: provider_ids.to_vec(),
                storage_encrypted: false,
                credential_previews: HashMap::new(),
                error: Some("provider credential storage is unavailable".to_string()),
            })
            .await;
            return;
        };

        if let Some(error) = operation_error {
            sink.emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids: Vec::new(),
                unavailable_provider_ids: provider_ids.to_vec(),
                storage_encrypted: credentials.provider_key_storage_is_encrypted(),
                credential_previews: HashMap::new(),
                error: Some(error),
            })
            .await;
            return;
        }

        let mut configured_provider_ids = Vec::new();
        let mut unavailable_provider_ids = Vec::new();
        let mut credential_previews = HashMap::new();
        let mut failures = Vec::new();
        for provider_id in provider_ids {
            match credentials.has_provider_key(provider_id).await {
                Ok(true) => {
                    configured_provider_ids.push(provider_id.clone());
                    if preview_provider_ids.contains(provider_id) {
                        match credentials.get_provider_key(provider_id).await {
                            Ok(Some(secret)) => {
                                credential_previews.insert(
                                    provider_id.clone(),
                                    secret::masked_credential_preview(secret.expose_secret()),
                                );
                            }
                            Ok(None) => {}
                            Err(failure) => {
                                failures.push(format!("{provider_id} preview: {failure}"))
                            }
                        }
                    }
                }
                Ok(false) => {}
                Err(failure) => {
                    unavailable_provider_ids.push(provider_id.clone());
                    failures.push(format!("{provider_id}: {failure}"));
                }
            }
        }
        let error = (!failures.is_empty()).then(|| {
            format!(
                "provider credential storage is unavailable ({})",
                failures.join("; ")
            )
        });

        sink.emit(ClientEvent::ProviderCredentialStatus {
            operation_id,
            configured_provider_ids,
            unavailable_provider_ids,
            storage_encrypted: credentials.provider_key_storage_is_encrypted(),
            credential_previews,
            error,
        })
        .await;
    }

    /// Spawn the background-task poll loop (matches the TUI: `TaskRegistryHandle::list`
    /// on an interval). Each tick lists the tasks and emits one
    /// [`ClientEvent::TaskRow`] per task through `sink`. The returned
    /// [`TaskPoll`] stops the loop on [`TaskPoll::stop`] or drop.
    #[must_use]
    pub fn spawn_task_poll(&self, sink: Arc<dyn ClientEventSink>, interval: Duration) -> TaskPoll {
        let tasks = self.tasks.clone();
        let token = CancellationToken::new();
        let child = token.clone();
        let join = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    () = child.cancelled() => break,
                    _ = ticker.tick() => {
                        emit_task_rows(&*tasks, &*sink, TaskListFilter::default()).await;
                    }
                }
            }
        });
        TaskPoll {
            token,
            join: Some(join),
        }
    }

    /// List tasks (optionally filtered) and emit one `TaskRow` per task.
    async fn emit_task_list(&self, filter: TaskListFilter, sink: &dyn ClientEventSink) {
        emit_task_rows(&*self.tasks, sink, filter).await;
    }

    /// Enumerate the real persisted JSONL catalog. A missing/empty store is a
    /// successful empty listing; other loader failures are surfaced as a
    /// recoverable client error without leaking filesystem details.
    async fn emit_session_list(&self, limit: usize, sink: &dyn ClientEventSink) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::SessionList {
                sessions: Vec::new(),
            })
            .await;
            return;
        };

        match session::jsonl::list_recent_sessions_with_diagnostics(
            &store.lingxi_home,
            &store.session_cwd,
            limit,
            store.fs.clone(),
        )
        .await
        {
            Ok(catalog) => {
                let sessions = catalog
                    .sessions
                    .iter()
                    .map(client_adapter::lowering::lower_session_metadata)
                    .collect();
                sink.emit(ClientEvent::SessionList { sessions }).await;
                if catalog.skipped_files > 0 {
                    tracing::warn!(
                        skipped_files = catalog.skipped_files,
                        "bridge-server: unreadable sessions skipped"
                    );
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: "Some unreadable sessions were skipped. Repair or remove damaged session files, then retry.".to_string(),
                    })
                    .await;
                }
            }
            Err(session::jsonl::LoaderError::EmptyDirectory) => {
                sink.emit(ClientEvent::SessionList {
                    sessions: Vec::new(),
                })
                .await;
            }
            Err(error) => {
                tracing::warn!(%error, "bridge-server: session catalog unavailable");
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: "session catalog is unavailable. Repair or remove unreadable session files, then retry.".to_string(),
                })
                .await;
            }
        }
    }

    /// Emit the real session-agent roster. `main` is included for protocol
    /// parity with mobile; Electron filters it from the Agents section because
    /// the main conversation already occupies the central Stage.
    async fn emit_session_agent_list(&self, sink: &dyn ClientEventSink) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message:
                    "session agent listing unavailable: persisted session store is unavailable"
                        .to_string(),
            })
            .await;
            return;
        };
        let session_id = self.handle.current_session_id().await;
        let status = if self.is_turn_active() {
            "running"
        } else {
            "idle"
        };
        let model_snapshot = self.handle.get_status_snapshot().await;
        let mut agents = vec![SessionAgentSummaryDto {
            agent_id: "main".to_string(),
            name: "Main agent".to_string(),
            agent_type: "main".to_string(),
            model: Some(model_snapshot.model),
            model_profile: model_snapshot.model_profile,
            status: status.to_string(),
            latest_activity: None,
            updated_at_ms: None,
        }];
        let dir = orchestrator::transcript_paths::subagents_dir(
            &store.lingxi_home,
            &store.session_cwd,
            &session_id.as_uuid().to_string(),
        );
        let mut unreadable = 0usize;
        match engine_desktop::session_agents::collect_transcript_paths(&dir).await {
            Ok(paths) => {
                for path in paths {
                    let Some(agent_id) = engine_desktop::session_agents::agent_id_from_path(&path)
                    else {
                        continue;
                    };
                    let raw = match engine_desktop::session_agents::read_transcript(&dir, &path)
                        .await
                    {
                        Ok(raw) => raw,
                        Err(error) => {
                            unreadable = unreadable.saturating_add(1);
                            tracing::warn!(%error, "bridge-server: session agent transcript unreadable");
                            continue;
                        }
                    };
                    if let Some(line) =
                        engine_desktop::session_agents::first_corrupt_transcript_line(&raw)
                    {
                        unreadable = unreadable.saturating_add(1);
                        tracing::warn!(
                            line,
                            "bridge-server: corrupt session agent transcript skipped"
                        );
                        continue;
                    }
                    if let Some(summary) =
                        read_session_agent_summary(agent_id, &dir, &path, &raw).await
                    {
                        agents.push(summary);
                    }
                }
            }
            Err(error) => {
                unreadable = unreadable.saturating_add(1);
                tracing::warn!(%error, "bridge-server: session agent transcript directory unreadable");
            }
        }
        agents[1..].sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
        sink.emit(ClientEvent::SessionAgentList {
            session_id: session_id.as_uuid().to_string(),
            agents,
        })
        .await;
        if unreadable > 0 {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: format!(
                    "{unreadable} unreadable or corrupt session agent transcript(s) were skipped"
                ),
            })
            .await;
        }
    }

    /// Read one agent's complete transcript with an append-only revision. The
    /// requested id is validated before path lookup and the result is dropped
    /// if the session changed while the filesystem read was in flight.
    async fn emit_session_agent_transcript(&self, agent_id: String, sink: &dyn ClientEventSink) {
        let Some(store) = self.session_store.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message:
                    "session agent transcript unavailable: persisted session store is unavailable"
                        .to_string(),
            })
            .await;
            return;
        };
        let requested_session_id = self.handle.current_session_id().await;
        let (messages, revision) = if agent_id == "main" {
            let uuid = requested_session_id.as_uuid();
            match orchestrator::replay_session_state(
                &store.lingxi_home,
                &store.session_cwd,
                uuid,
                store.fs.clone(),
            )
            .await
            {
                Ok(replayed) => {
                    let path = orchestrator::transcript_paths::main_transcript_path(
                        &store.lingxi_home,
                        &store.session_cwd,
                        &uuid.to_string(),
                    );
                    let raw = match tokio::fs::read(path).await {
                        Ok(raw) => raw,
                        Err(error) => {
                            tracing::warn!(%error, "bridge-server: main transcript unreadable");
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Internal,
                                message:
                                    "load main agent transcript failed: transcript is unreadable"
                                        .to_string(),
                            })
                            .await;
                            return;
                        }
                    };
                    (
                        client_adapter::lowering::lower_transcript(&replayed.state.history),
                        engine_desktop::session_agents::transcript_revision(&raw),
                    )
                }
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("load main agent transcript failed: {error}"),
                    })
                    .await;
                    return;
                }
            }
        } else {
            let Some(parsed) = protocol::AgentId::parse_prefixed(&agent_id) else {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("malformed session agent id: {agent_id:?}"),
                })
                .await;
                return;
            };
            let dir = orchestrator::transcript_paths::subagents_dir(
                &store.lingxi_home,
                &store.session_cwd,
                &requested_session_id.as_uuid().to_string(),
            );
            let path = match engine_desktop::session_agents::find_transcript_path(
                &dir,
                &parsed.to_string(),
            )
            .await
            {
                Ok(path) => path,
                Err(error) => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("load session agent transcript failed: {error}"),
                    })
                    .await;
                    return;
                }
            };
            let raw = match path {
                Some(path) => {
                    match engine_desktop::session_agents::read_transcript(&dir, &path).await {
                        Ok(raw) => raw,
                        Err(error) => {
                            tracing::warn!(%error, "bridge-server: session agent transcript unreadable");
                            sink.emit(ClientEvent::Error {
                                kind: ErrorKindDto::Internal,
                                message:
                                    "load session agent transcript failed: transcript is unreadable"
                                        .to_string(),
                            })
                            .await;
                            return;
                        }
                    }
                }
                None => {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "session agent transcript was not found".to_string(),
                    })
                    .await;
                    return;
                }
            };
            if let Some(line) = engine_desktop::session_agents::first_corrupt_transcript_line(&raw)
            {
                tracing::warn!(
                    line,
                    "bridge-server: corrupt session agent transcript rejected"
                );
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: "load session agent transcript failed: transcript is corrupt"
                        .to_string(),
                })
                .await;
                return;
            }
            (
                engine_desktop::session_agents::lower_transcript(&raw),
                engine_desktop::session_agents::transcript_revision(&raw),
            )
        };
        if self.handle.current_session_id().await != requested_session_id {
            return;
        }
        sink.emit(ClientEvent::SessionAgentTranscript {
            session_id: requested_session_id.as_uuid().to_string(),
            agent_id,
            next_message_index: messages.len() as u64,
            messages,
            revision,
        })
        .await;
    }

    /// Snapshot the live slash-command catalog from the shared registry, adding
    /// the local `/reload-plugins` command when the registry itself does not
    /// carry it.
    async fn slash_command_catalog(&self) -> Option<Vec<SlashCommandDto>> {
        let registry = self.slash_registry.as_ref()?;
        let reg = registry.read().await;
        Some(Self::slash_command_catalog_from_registry(&reg))
    }

    fn slash_command_catalog_from_registry(reg: &CommandRegistry) -> Vec<SlashCommandDto> {
        let mut commands: Vec<SlashCommandDto> = reg
            .palette_commands()
            .into_iter()
            .map(|cmd| SlashCommandDto {
                hidden: is_palette_hidden(&cmd.name),
                source: command_source_string(cmd.source).to_string(),
                name: cmd.name,
                description: cmd.description,
                aliases: cmd.aliases,
                argument_hint: cmd.argument_hint,
                menu_description: cmd.menu_description,
            })
            .collect();
        if !commands.iter().any(|cmd| cmd.name == "reload-plugins")
            && !is_palette_hidden("reload-plugins")
        {
            commands.push(SlashCommandDto {
                name: "reload-plugins".to_string(),
                description: core_description("reload-plugins").to_string(),
                source: "builtin".to_string(),
                aliases: Vec::new(),
                argument_hint: None,
                menu_description: None,
                hidden: false,
            });
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }

    async fn capture_slash_authority(&self) -> SlashAuthoritySnapshot {
        let snapshot = self.handle.get_status_snapshot().await;
        SlashAuthoritySnapshot {
            session_id: self.handle.current_session_id().await.to_string(),
            model: platform_api::qualified_model_ref(
                &snapshot.model,
                snapshot.model_profile.as_deref(),
            ),
            permission_mode: self.handle.permission_mode().await,
            auth: lower_auth_state(self.auth.current_user().await),
            catalog: self.slash_command_catalog().await,
        }
    }

    async fn emit_slash_authority_changes(
        &self,
        before: &SlashAuthoritySnapshot,
        after: &SlashAuthoritySnapshot,
        sink: &dyn ClientEventSink,
    ) {
        for event in Self::slash_authority_change_events(before, after) {
            sink.emit(event).await;
        }
    }

    fn slash_authority_change_events(
        before: &SlashAuthoritySnapshot,
        after: &SlashAuthoritySnapshot,
    ) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        if before.session_id != after.session_id {
            events.push(ClientEvent::SessionEnded);
        }
        if before.model != after.model {
            events.push(ClientEvent::ModelChanged {
                model: after.model.clone(),
            });
        }
        if before.permission_mode != after.permission_mode {
            if let Some(mode) = after.permission_mode.clone() {
                events.push(ClientEvent::PermissionModeChanged { mode });
            }
        }
        if before.auth != after.auth {
            events.push(ClientEvent::AuthState {
                state: after.auth.clone(),
            });
        }
        if before.catalog != after.catalog {
            if let Some(commands) = after.catalog.clone() {
                events.push(ClientEvent::CommandsChanged { commands });
            }
        }
        events
    }

    /// Pull + emit a single listing kind. Listing kinds with no engine handle in
    /// the foundation are skipped (see module docs).
    async fn emit_listing(&self, kind: ListingKindDto, sink: &dyn ClientEventSink) {
        match kind {
            ListingKindDto::Models => {
                let available = self.handle.list_available_models().await;
                let listings = self.handle.list_model_listings().await;
                let snapshot = self.handle.get_status_snapshot().await;
                let provider_catalog = if self.provider_model_catalog_listings.is_empty() {
                    provider_model_catalog_from_listings(&listings)
                } else {
                    provider_model_catalog_from_listings(&self.provider_model_catalog_listings)
                };
                sink.emit(ClientEvent::ProviderModelCatalog {
                    providers: provider_catalog,
                })
                .await;
                let curated = platform_api::curated_model_listings(
                    &listings,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let models = platform_api::curated_model_refs(
                    &listings,
                    &available,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let current = platform_api::qualified_model_ref(
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                sink.emit(ClientEvent::ModelList {
                    models,
                    current,
                    details: curated
                        .iter()
                        .map(client_adapter::lowering::lower_model_details)
                        .collect(),
                })
                .await;
            }
            ListingKindDto::Mcp => {
                let servers = self
                    .handle
                    .list_mcp_servers()
                    .await
                    .iter()
                    .map(lower_mcp_server_info)
                    .collect();
                sink.emit(ClientEvent::McpServers { servers }).await;
            }
            ListingKindDto::Skills => {
                let skills = self
                    .handle
                    .list_skills()
                    .await
                    .iter()
                    .map(lower_skill_info)
                    .collect();
                sink.emit(ClientEvent::Skills { skills }).await;
            }
            ListingKindDto::Hooks => {
                let hooks = self
                    .handle
                    .list_hooks()
                    .await
                    .iter()
                    .map(lower_hook_info)
                    .collect();
                sink.emit(ClientEvent::Hooks { hooks }).await;
            }
            ListingKindDto::Agents => {
                let agents = self
                    .handle
                    .list_agents()
                    .await
                    .iter()
                    .map(lower_agent_info)
                    .collect();
                sink.emit(ClientEvent::Agents { agents }).await;
            }
            ListingKindDto::Status => {
                let snapshot = lower_status_snapshot(&self.handle.get_status_snapshot().await);
                sink.emit(ClientEvent::StatusSnapshot { snapshot }).await;
            }
            ListingKindDto::Doctor => {
                let report = lower_doctor_report(&self.handle.run_doctor_checks().await);
                sink.emit(ClientEvent::DoctorReport { report }).await;
            }
            ListingKindDto::Auth => {
                let state = lower_auth_state(self.auth.current_user().await);
                sink.emit(ClientEvent::AuthState { state }).await;
            }
            ListingKindDto::Tasks => {
                self.emit_task_list(TaskListFilter::default(), sink).await;
            }
            ListingKindDto::SlashCommands => {
                if let Some(commands) = self.slash_command_catalog().await {
                    sink.emit(ClientEvent::SlashCommandCatalog { commands })
                        .await;
                } else {
                    tracing::debug!(
                        "bridge-server: slash-command catalog unavailable (no shared registry)"
                    );
                }
            }
            // HOST/engine-tier reads the binary wires once it holds the desktop
            // runtime (plan §2). No engine handle for these in the foundation —
            // routing them is additive and does not change this seam's shape.
            ListingKindDto::Sessions => {
                self.emit_session_list(DEFAULT_SESSION_LIST_LIMIT, sink)
                    .await;
            }
            ListingKindDto::Settings => {
                self.emit_settings_snapshot(sink).await;
            }
            ListingKindDto::Memory => {
                tracing::debug!(
                    ?kind,
                    "bridge-server: listing kind has no engine handle in the foundation"
                );
            }
            // `#[non_exhaustive]` catch-all: a future listing kind is additive.
            _ => {
                tracing::debug!(?kind, "bridge-server: unhandled listing kind");
            }
        }
    }
}

/// Shared helper: list tasks through the handle and emit one `TaskRow` per task.
async fn emit_task_rows(
    tasks: &dyn TaskRegistryHandle,
    sink: &dyn ClientEventSink,
    filter: TaskListFilter,
) {
    match tasks.list(filter).await {
        Ok(records) => {
            for rec in &records {
                sink.emit(ClientEvent::TaskRow {
                    task: lower_task_record(rec),
                })
                .await;
            }
        }
        Err(e) => {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: format!("task list failed: {e}"),
            })
            .await;
        }
    }
}

/// Handle to the background-task poll loop spawned by
/// [`EngineCommandRouter::spawn_task_poll`]. Cancels the loop on [`Self::stop`]
/// or drop.
pub struct TaskPoll {
    token: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl TaskPoll {
    /// Stop the poll loop. Idempotent.
    pub fn stop(&self) {
        self.token.cancel();
    }

    /// Stop the loop and await its termination.
    pub async fn shutdown(mut self) {
        self.token.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for TaskPoll {
    fn drop(&mut self) {
        self.token.cancel();
        if let Some(join) = self.join.as_ref() {
            join.abort();
        }
    }
}

#[async_trait]
impl CommandRouter for EngineCommandRouter {
    async fn dispatch_slash(&self, raw: &str) -> Option<SlashDispatchOutcome> {
        let before = self.capture_slash_authority().await;
        let result = match self.dispatch_desktop_slash(raw).await {
            Some(result) => result,
            None => self.dispatcher.as_ref()?.dispatch(raw).await,
        };
        if parse_slash_command(raw).is_some_and(|parsed| parsed.name.eq_ignore_ascii_case("cron")) {
            let display = match &result {
                platform_api::SlashDispatchResult::Handled { display }
                | platform_api::SlashDispatchResult::Unknown { display, .. } => Some(display),
                _ => None,
            };
            if let Some(display) = display {
                if let Err(error) = self
                    .handle
                    .append_slash_command_transcript(raw, display)
                    .await
                {
                    tracing::warn!(error = %error, "could not persist /cron transcript display");
                }
            }
        }
        let after = self.capture_slash_authority().await;
        Some(SlashDispatchOutcome {
            result,
            authority_events: Self::slash_authority_change_events(&before, &after),
        })
    }

    // The full command dispatch is one match over the command surface; splitting
    // it would scatter the one-place-routes-everything map this module exists to
    // be. (Same convention as `engine_desktop::build`.)
    #[allow(clippy::too_many_lines)]
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        match command {
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
                let error = if !provider_id_is_valid(&provider_id)
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
                let applied = error.is_none();
                if applied {
                    // Round-9 review finding [2]: this is the ONLY
                    // credential-add path the Electron desktop has (Settings ->
                    // Provider Credentials), in BOTH the brokered/ephemeral and
                    // the persistent mode, and neither branch above told
                    // Fusion's catalog filter about the write. `/model` and the
                    // ordinary turn loop route the new key on the very next
                    // request (`MultiCredentialProvider` reads
                    // `CredentialManager` per call), while
                    // `FusionCatalogModelSource::list()` kept re-filtering
                    // against the BOOT availability map — so every row of the
                    // just-added provider stayed dropped for the rest of the
                    // engine process (`TooFewModels{eligible:0}` under
                    // `fusion.allowedProfiles`). Same call the TUI key view
                    // makes (`apps/cli/src/mode.rs`'s `run_connect_action`);
                    // `refresh_after_credential_write` force-marks the named
                    // profile available, so it is correct for the ephemeral
                    // branch too, where the re-probe cannot see a key that was
                    // never persisted.
                    //
                    // Round-10 finding N3's class sweep: BOUNDED, because this
                    // arm is awaited straight from the connection read loop
                    // (`server.rs`'s `on_frame`, whose contract is "return
                    // promptly"), and the refresh is an unbounded keychain
                    // re-probe. A contended macOS credential broker would
                    // otherwise stop this connection from READING anything at
                    // all -- interrupts and permission replies included -- for
                    // as long as the broker stalls. Same budget, and the same
                    // detach-rather-than-cancel rule, as the parent-supplied
                    // keys at `boot::seed_parent_supplied_provider_keys`.
                    crate::boot::refresh_fusion_catalog_bounded(vec![provider_id.clone()]).await;
                }
                let credential_previews = applied
                    .then(|| HashMap::from([(provider_id.clone(), credential_preview)]))
                    .unwrap_or_default();
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
            // `engine_desktop::refresh_fusion_catalog_after_credential_delete`
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
                    engine_desktop::refresh_fusion_catalog_after_credential_delete(&provider_id)
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
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "cannot change permission mode while a turn is active".to_string(),
                    })
                    .await;
                    return;
                }
                match self.handle.set_permission_mode(&mode).await {
                    Ok(()) => {
                        let active = self.handle.permission_mode().await.unwrap_or(mode);
                        platform_api::live_sessions::set_process_permission_mode(
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
                let (model_id, profile) = platform_api::parse_model_ref(&model, &listings);
                match self
                    .handle
                    .switch_model(&model_id, profile.as_deref())
                    .await
                {
                    Ok(()) => {
                        let selected =
                            platform_api::qualified_model_ref(&model_id, profile.as_deref());
                        sink.emit(ClientEvent::ModelChanged { model: selected })
                            .await;
                        if let Some(controls) = self.handle.conversation_controls().await {
                            if controls.requested_reasoning_selection
                                != platform_api::ReasoningSelection::Automatic
                                && controls.requested_reasoning_selection
                                    != controls.effective_reasoning_selection
                            {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(
                                        platform_api::ReasoningSelection::Automatic,
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
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "cannot change reasoning selection while a turn is active"
                            .to_string(),
                    })
                    .await;
                    return;
                }
                if let Err(error) = self
                    .handle
                    .set_reasoning_selection(decode_reasoning_selection(selection))
                    .await
                {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: format!("set_reasoning_selection failed: {error}"),
                    })
                    .await;
                    return;
                }
                self.emit_controls_snapshot(&*sink).await;
            }
            ClientCommand::SetFastMode { enabled } => {
                if self.is_turn_active() {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Rejected,
                        message: "cannot change fast mode while a turn is active".to_string(),
                    })
                    .await;
                    return;
                }
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
            }

            // ── Slash commands ──────────────────────────────────────────────
            ClientCommand::RunSlashCommand { raw, turn_id } => {
                if let Some(dispatcher) = self.dispatcher.as_ref() {
                    let before = self.capture_slash_authority().await;
                    let (display, is_error) = match dispatcher.dispatch(&raw).await {
                        platform_api::SlashDispatchResult::Handled { display } => (display, false),
                        platform_api::SlashDispatchResult::Unknown { display, .. } => {
                            (display, true)
                        }
                        // A `type: "prompt"` command reached the display-only
                        // fallback (the connection should have intercepted it via
                        // `dispatch_slash` and run it as a turn). Surface the
                        // expanded prompt so nothing is silently dropped.
                        platform_api::SlashDispatchResult::RunAsTurn { prompt } => (prompt, false),
                        platform_api::SlashDispatchResult::NotASlashCommand => {
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
            // Round-10 finding N5: this IS a credential write (the Anthropic
            // OAuth handle persists tokens on success), and it is the one
            // credential-write arm in this file with no
            // `refresh_fusion_catalog_bounded` call. That is deliberate, not an
            // omission: the same call would be a provable no-op here.
            // `provider_config::assemble` emits NO Anthropic `CredentialSource`
            // in the unauthenticated path (provider-config/src/assemble.rs,
            // `anthropic_profile`'s final arm), so a refresh keyed on
            // `anthropic-oauth` matches nothing and its `forced` list is empty,
            // and `refresh_inner`'s closing
            // `guard.entry("anthropic").or_insert(..)` cannot overwrite the
            // `false` the boot map already holds (`resolve_llm_stack` always
            // seeds an "anthropic" entry before cloning the map into the Fusion
            // refresher). Making a mid-session sign-in visible to `/fusion`
            // needs a NEW engine-desktop API — "this profile just GAINED a
            // credential it had no source for" — the same one the demoted
            // credential-DELETE direction needs; it is not this one-liner.
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
            // (`command_core::LogoutHandler`) drives, so the clearing lives
            // in `engine_desktop::FusionCatalogClearingAuth`, which wraps
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
                match self.handle.clear_session().await {
                    Ok(()) => sink.emit(ClientEvent::SessionEnded).await,
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

                if let Err(error) = self.handle.clear_session().await {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Internal,
                        message: format!("new session (clear_session) failed: {error}"),
                    })
                    .await;
                    return;
                }

                if let Some(model) = model {
                    let listings = self.handle.list_model_listings().await;
                    let (model_id, profile) = platform_api::parse_model_ref(&model, &listings);
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

                let session_id = self.handle.current_session_id().await.to_string();
                sink.emit(ClientEvent::SessionStarted {
                    session_id,
                    mode: SessionModeDto::Code,
                })
                .await;
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

                let messages = client_adapter::lowering::lower_transcript(&replayed.state.history);
                let resume_plan_mode = replayed.state.plan_mode;
                let previous_permission_mode = self
                    .handle
                    .permission_mode()
                    .await
                    .unwrap_or_else(|| "default".to_string());
                let previous_plan_mode = self.handle.plan_mode().await;
                let runtime_snapshot = replayed.handle_runtime_snapshot();
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
                        protocol::SessionId::from_uuid(uuid),
                        replayed.state.history,
                        replayed.last_message_uuid.map(|value| value.to_string()),
                        replayed.state.active_goal.clone().map(|goal| {
                            platform_api::ActiveGoalSnapshot {
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

                sink.emit(ClientEvent::SessionResumed {
                    session_id: uuid.to_string(),
                    mode: SessionModeDto::Code,
                    messages,
                })
                .await;
                // resume_session restores the model/effort held by this exact
                // transcript. Publish that authoritative session state after
                // activation so clients do not keep showing the model selected
                // in whichever session happened to be open previously.
                let status = self.handle.get_status_snapshot().await;
                if !status.model.is_empty() {
                    sink.emit(ClientEvent::ModelChanged {
                        model: platform_api::qualified_model_ref(
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
            ClientCommand::TaskList { status_filter } => {
                let filter = TaskListFilter {
                    status: status_filter.map(task_status_wire),
                };
                self.emit_task_list(filter, &*sink).await;
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
            ClientCommand::TaskStop { task_id } => {
                match self.tasks.kill_with_reason(&task_id, "user").await {
                    Ok(rec) => {
                        sink.emit(ClientEvent::TaskStatusChanged {
                            task_id: rec.task_id,
                            status: client_adapter::lowering::lower_task_status(&rec.status),
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
            // `engine-mobile`'s host owns (`host.rs`'s `AttachTurn` /
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

fn lower_reasoning_selection(
    selection: &platform_api::ReasoningSelection,
) -> ReasoningSelectionDto {
    match selection {
        platform_api::ReasoningSelection::Automatic => ReasoningSelectionDto::Automatic,
        platform_api::ReasoningSelection::Disabled => ReasoningSelectionDto::Disabled,
        platform_api::ReasoningSelection::Enabled => ReasoningSelectionDto::Enabled,
        platform_api::ReasoningSelection::Level { id } => {
            ReasoningSelectionDto::Level { id: id.clone() }
        }
        platform_api::ReasoningSelection::TokenBudget { tokens } => {
            ReasoningSelectionDto::TokenBudget { tokens: *tokens }
        }
    }
}

fn decode_reasoning_selection(
    selection: ReasoningSelectionDto,
) -> platform_api::ReasoningSelection {
    match selection {
        ReasoningSelectionDto::Automatic => platform_api::ReasoningSelection::Automatic,
        ReasoningSelectionDto::Disabled => platform_api::ReasoningSelection::Disabled,
        ReasoningSelectionDto::Enabled => platform_api::ReasoningSelection::Enabled,
        ReasoningSelectionDto::Level { id } => platform_api::ReasoningSelection::Level { id },
        ReasoningSelectionDto::TokenBudget { tokens } => {
            platform_api::ReasoningSelection::TokenBudget { tokens }
        }
        _ => platform_api::ReasoningSelection::Automatic,
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
        CommandSource::User => "user",
        CommandSource::Project => "project",
        CommandSource::Local => "local",
        CommandSource::Plugin => "plugin",
        CommandSource::Managed => "managed",
        CommandSource::Mcp => "mcp",
        CommandSource::Bundled => "bundled",
    }
}

/// Lower a [`TaskStatusDto`] back to the registry's wire status string for the
/// `TaskListFilter` (the inverse of `client_adapter::lowering::lower_task_status`).
fn task_status_wire(status: TaskStatusDto) -> String {
    match status {
        TaskStatusDto::Running => "running",
        TaskStatusDto::Paused => "paused",
        TaskStatusDto::Completed => "completed",
        TaskStatusDto::Failed => "failed",
        // The DTO's user-stop variant maps to the registry's terminal "killed".
        TaskStatusDto::Cancelled => "killed",
        // `Pending` and the `#[non_exhaustive]` catch-all both map to the safe
        // non-terminal default (the inverse of `lower_task_status`'s fallback).
        _ => "pending",
    }
    .to_string()
}

/// Decode a `ClientCommand::UpdateSettings.patch_json` wire string into the
/// shallow `(key, Option<value>)` patch [`crate::settings_bridge::apply_patch`]
/// expects. A JSON `null` value means "delete this key" (documented on the
/// wire field); any other value means "set this key". Pulled out as a pure
/// function — rather than left inline in [`EngineCommandRouter::apply_settings_patch`]
/// — so the untrusted-input decoding step has its own unit tests independent
/// of a router/sink/settings-context fixture.
///
/// # Errors
/// `patch_json` is not valid JSON, or it parses to something other than a
/// JSON object (a bare array/string/number/bool/null patch is rejected, not
/// silently coerced).
fn parse_settings_patch(
    patch_json: &str,
) -> Result<Vec<(String, Option<serde_json::Value>)>, String> {
    match serde_json::from_str::<serde_json::Value>(patch_json) {
        Ok(serde_json::Value::Object(map)) => Ok(map
            .into_iter()
            .map(|(k, v)| {
                let v = if v.is_null() { None } else { Some(v) };
                (k, v)
            })
            .collect()),
        Ok(other) => Err(format!(
            "settings patch must be a JSON object, got: {other}"
        )),
        Err(e) => Err(format!("settings patch is not valid JSON: {e}")),
    }
}

#[cfg(test)]
mod settings_patch_parsing_tests {
    use super::parse_settings_patch;
    use serde_json::json;

    /// A `null` value in the wire patch means "delete this key" — it must be
    /// decoded to `None`, never to a stored `Some(Value::Null)`. A regression
    /// that kept the literal `Value::Null` would still satisfy "the key has
    /// an entry" but would be silently wrong once applied (it would WRITE a
    /// JSON `null`, not delete the key), so this asserts the exact `None`
    /// shape, not just success.
    #[test]
    fn null_value_decodes_to_a_delete_not_a_stored_null() {
        let patch = parse_settings_patch(r#"{"outputStyle": null}"#).unwrap();
        assert_eq!(
            patch,
            vec![("outputStyle".to_string(), None)],
            "a JSON null must decode to None (delete), not Some(Value::Null)"
        );
    }

    /// A non-null value decodes to `Some(value)` (a set, not a delete) —
    /// the companion case to the null test above, so the `is_null` branch is
    /// exercised on both sides.
    #[test]
    fn non_null_value_decodes_to_a_set() {
        let patch = parse_settings_patch(r#"{"outputStyle": "terse"}"#).unwrap();
        assert_eq!(
            patch,
            vec![("outputStyle".to_string(), Some(json!("terse")))]
        );
    }

    /// A JSON array is syntactically valid JSON but not an acceptable patch
    /// shape (there are no keys to patch). It must be rejected, not coerced
    /// or silently accepted as an empty/no-op patch.
    #[test]
    fn a_json_array_patch_is_rejected() {
        let err = parse_settings_patch(r#"["outputStyle"]"#).unwrap_err();
        assert!(
            err.contains("object"),
            "error must say the patch needs to be an object, got: {err}"
        );
    }

    /// Syntactically broken JSON must be rejected with a message a client
    /// can act on, not panic or silently produce an empty patch.
    #[test]
    fn invalid_json_is_rejected() {
        let err = parse_settings_patch("{ not json").unwrap_err();
        assert!(
            err.contains("JSON"),
            "error must say the patch is not valid JSON, got: {err}"
        );
    }
}

/// Decode a `ClientCommand::UpsertMcpServer.config_json` wire string into the
/// `serde_json::Value` [`crate::mcp_bridge::upsert_server`] expects. Mirrors
/// [`parse_settings_patch`]'s object-shape validation: `config_json` must be
/// a JSON object (a `.mcp.json` entry is always `{command: ...}` or
/// `{url: ...}` shaped — never a bare string/array/number).
///
/// # Errors
/// `config_json` is not valid JSON, or it parses to something other than a
/// JSON object.
fn parse_mcp_config_json(config_json: &str) -> Result<serde_json::Value, String> {
    match serde_json::from_str::<serde_json::Value>(config_json) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(value),
        Ok(other) => Err(format!(
            "MCP server config must be a JSON object, got: {other}"
        )),
        Err(e) => Err(format!("MCP server config is not valid JSON: {e}")),
    }
}

#[cfg(test)]
mod mcp_config_json_parsing_tests {
    use super::parse_mcp_config_json;
    use serde_json::json;

    #[test]
    fn a_json_object_decodes_to_itself() {
        let value = parse_mcp_config_json(r#"{"command": "npx"}"#).unwrap();
        assert_eq!(value, json!({ "command": "npx" }));
    }

    #[test]
    fn a_non_object_is_rejected() {
        let err = parse_mcp_config_json(r#"["npx"]"#).unwrap_err();
        assert!(
            err.contains("object"),
            "error must say the config needs to be an object, got: {err}"
        );
    }

    #[test]
    fn invalid_json_is_rejected() {
        let err = parse_mcp_config_json("{ not json").unwrap_err();
        assert!(
            err.contains("JSON"),
            "error must say the config is not valid JSON, got: {err}"
        );
    }
}

/// Round-9 review finding [2]: the Electron desktop's ONLY credential-add path
/// is [`ClientCommand::SetProviderCredential`] (Settings -> Provider
/// Credentials). It must tell Fusion's process-wide catalog refresher about the
/// write, exactly the way the TUI key view does
/// (`apps/cli/src/mode.rs`'s `run_connect_action`); without it a key added in
/// Settings routes on the next ordinary turn but stays invisible to `/fusion`
/// for the rest of the engine process.
#[cfg(test)]
mod fusion_catalog_refresh_tests {
    use super::{CommandRouter, EngineCommandRouter};
    use client_adapter::ClientEventSink;
    use client_protocol::commands::{ClientCommand, ProviderCredentialSecretDto};
    use client_protocol::events::ClientEvent;
    use platform_api::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
        TaskRegistryHandle, TaskUpdatePatch,
    };
    use platform_api::{AuthError, AuthHandle, LoginInfo};
    use platform_posix::{PlainTextSecureStorage, PosixClock, PosixHttp};
    use std::sync::Arc;

    struct SilentSink;
    #[async_trait::async_trait]
    impl ClientEventSink for SilentSink {
        async fn emit(&self, _event: ClientEvent) {}
    }

    struct MockAuth;
    #[async_trait::async_trait]
    impl AuthHandle for MockAuth {
        async fn login(&self) -> Result<LoginInfo, AuthError> {
            Ok(LoginInfo {
                email: "u@x.com".into(),
                org_id: "org_1".into(),
            })
        }
        async fn logout(&self) -> Result<(), AuthError> {
            Ok(())
        }
        async fn current_user(&self) -> Option<LoginInfo> {
            None
        }
    }

    struct MockTaskRegistry;
    #[async_trait::async_trait]
    impl TaskRegistryHandle for MockTaskRegistry {
        async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(Vec::new())
        }
        async fn update(
            &self,
            _id: &str,
            _patch: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn set_status(
            &self,
            _id: &str,
            _status: &str,
        ) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn output(
            &self,
            _id: &str,
            _offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
    }

    async fn router_with_credentials(
        credentials: Arc<secret::CredentialManager>,
        ephemeral: bool,
    ) -> EngineCommandRouter {
        EngineCommandRouter::new(
            Arc::new(orchestrator::test_support::MockOrchestratorHandle::new())
                as Arc<dyn platform_api::orchestrator::OrchestratorHandle>,
            Arc::new(MockAuth) as Arc<dyn AuthHandle>,
            Arc::new(MockTaskRegistry) as Arc<dyn TaskRegistryHandle>,
            None,
            None,
        )
        .with_credentials(credentials)
        .with_ephemeral_provider_credentials(ephemeral)
    }

    /// Both branches of the arm — the persistent keychain write and the
    /// packaged/brokered ephemeral one — must reach the refresher. Asserting on
    /// the SHARED availability map a registered `FusionCatalogRefresher` owns
    /// (the very map `FusionCatalogModelSource::list()` re-filters against)
    /// pins the wiring end to end, not just that some function was called.
    #[tokio::test]
    async fn setting_a_provider_credential_refreshes_the_fusion_catalog() {
        // The refresher registry is process-wide and its entries are fanned out
        // to SERIALLY, so this test must not run alongside the sibling test
        // below, which deliberately registers a never-answering backend.
        let _registry = crate::boot::fusion_refresh_test_support::REGISTRY_LOCK
            .lock()
            .await;
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = Arc::new(
            PlainTextSecureStorage::new(temp.path().join("credentials"))
                .await
                .expect("storage"),
        );
        let credentials = Arc::new(secret::CredentialManager::new(
            storage,
            Arc::new(PosixClock::new()),
            Arc::new(PosixHttp::new()),
        ));

        // The boot availability map of a session that had neither provider
        // credentialed — what Fusion's catalog filter enforces until a
        // credential write refreshes it.
        let availability = Arc::new(std::sync::RwLock::new(
            [
                ("openrouter".to_string(), false),
                ("deepseek".to_string(), false),
            ]
            .into_iter()
            .collect::<std::collections::BTreeMap<String, bool>>(),
        ));
        engine_desktop::register_fusion_catalog_refresher(
            engine_desktop::FusionCatalogRefresher::for_keychain_profiles(
                availability.clone(),
                credentials.clone(),
                &["openrouter", "deepseek"],
            ),
        );

        let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
        router_with_credentials(credentials.clone(), false)
            .await
            .route(
                ClientCommand::SetProviderCredential {
                    operation_id: 1,
                    provider_id: "openrouter".into(),
                    credential: ProviderCredentialSecretDto::new("sk-or-round9".into()),
                },
                sink.clone(),
            )
            .await;
        router_with_credentials(credentials, true)
            .await
            .route(
                ClientCommand::SetProviderCredential {
                    operation_id: 2,
                    provider_id: "deepseek".into(),
                    credential: ProviderCredentialSecretDto::new("sk-ds-round9".into()),
                },
                sink,
            )
            .await;

        let published = availability.read().expect("availability lock").clone();
        assert_eq!(
            published.get("openrouter"),
            Some(&true),
            "the persistent Settings credential write must reach Fusion's \
catalog refresher; a stale `false` here is what silently drops every \
OpenRouter row from /fusion for the rest of the process: {published:?}"
        );
        assert_eq!(
            published.get("deepseek"),
            Some(&true),
            "the packaged/brokered EPHEMERAL branch of the same arm must \
refresh too — its key is never persisted, so only the write-side \
notification can make it visible to Fusion: {published:?}"
        );
    }

    /// Round-10 finding N3's class sweep: the SAME unbounded keychain re-probe
    /// the parent-supplied-keys path had (`boot::seed_parent_supplied_provider_keys`)
    /// also sat on this arm — and this one is awaited straight from the
    /// connection's read loop (`server.rs`'s `on_frame`, contract: "return
    /// promptly"). A contended macOS credential broker would stop the
    /// connection from READING anything at all — interrupts and permission
    /// replies included — for as long as the broker stalled.
    ///
    /// Virtual time (`start_paused`): the assertion is on the DEADLINE.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_credential_backend_never_parks_the_router_arm() {
        let _registry = crate::boot::fusion_refresh_test_support::REGISTRY_LOCK
            .lock()
            .await;
        // "never-answers" has no credential of any kind, so the re-probe this
        // write triggers reaches the never-answering backend.
        let (credentials, _availability, reads) =
            crate::boot::fusion_refresh_test_support::register_stalling_refresher(&[
                "openrouter",
                "never-answers",
            ]);

        let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
        let router = router_with_credentials(credentials, true).await;
        let began = tokio::time::Instant::now();
        let arm_returned = tokio::time::timeout(
            crate::boot::FUSION_CATALOG_REFRESH_BUDGET * 3,
            router.route(
                ClientCommand::SetProviderCredential {
                    operation_id: 7,
                    provider_id: "openrouter".into(),
                    credential: ProviderCredentialSecretDto::new("sk-or-round10".into()),
                },
                sink,
            ),
        )
        .await;
        assert!(
            arm_returned.is_ok(),
            "`SetProviderCredential` must return even when the credential backend \
never answers: this arm is awaited from the connection read loop, so a stall \
here stops the client's interrupts and permission replies from being read"
        );
        assert!(
            began.elapsed() < crate::boot::FUSION_CATALOG_REFRESH_BUDGET * 2,
            "the arm must return within one {:?} refresh budget, waited {:?}",
            crate::boot::FUSION_CATALOG_REFRESH_BUDGET,
            began.elapsed()
        );
        assert!(
            reads.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "the refresh must actually have reached the credential backend — with \
zero reads this test would pass without exercising the stall at all"
        );
    }

    /// Round-12 finding [2]: the DELETE half of the same Settings surface.
    ///
    /// `MultiCredentialProvider` reads `CredentialManager` per call, so the
    /// ordinary turn loop stops routing a deleted provider immediately — but
    /// `FusionCatalogModelSource::list()` keeps re-filtering against the
    /// shared availability map, which nothing ever LOWERS. A provider whose
    /// key was deleted mid-session therefore keeps every one of its catalog
    /// rows, gets auto-selected as a `/fusion` panel, has budget reserved for
    /// it, and dies on `LlmError::Authentication` at request time instead of
    /// being excluded by the §4 preflight.
    ///
    /// Both branches of the arm — persistent keychain delete and the
    /// packaged/brokered ephemeral one — must clear the entry.
    #[tokio::test]
    async fn deleting_a_provider_credential_clears_it_from_the_fusion_catalog() {
        let _registry = crate::boot::fusion_refresh_test_support::REGISTRY_LOCK
            .lock()
            .await;
        let temp = tempfile::tempdir().expect("tempdir");
        let storage = Arc::new(
            PlainTextSecureStorage::new(temp.path().join("credentials"))
                .await
                .expect("storage"),
        );
        let credentials = Arc::new(secret::CredentialManager::new(
            storage,
            Arc::new(PosixClock::new()),
            Arc::new(PosixHttp::new()),
        ));

        let availability = Arc::new(std::sync::RwLock::new(
            [
                ("openrouter".to_string(), false),
                ("deepseek".to_string(), false),
            ]
            .into_iter()
            .collect::<std::collections::BTreeMap<String, bool>>(),
        ));
        engine_desktop::register_fusion_catalog_refresher(
            engine_desktop::FusionCatalogRefresher::for_keychain_profiles(
                availability.clone(),
                credentials.clone(),
                &["openrouter", "deepseek"],
            ),
        );

        let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
        // Boot state: both providers credentialed and published as available.
        router_with_credentials(credentials.clone(), false)
            .await
            .route(
                ClientCommand::SetProviderCredential {
                    operation_id: 1,
                    provider_id: "openrouter".into(),
                    credential: ProviderCredentialSecretDto::new("sk-or-round12".into()),
                },
                sink.clone(),
            )
            .await;
        router_with_credentials(credentials.clone(), true)
            .await
            .route(
                ClientCommand::SetProviderCredential {
                    operation_id: 2,
                    provider_id: "deepseek".into(),
                    credential: ProviderCredentialSecretDto::new("sk-ds-round12".into()),
                },
                sink.clone(),
            )
            .await;
        let seeded = availability.read().expect("availability lock").clone();
        assert_eq!(
            (seeded.get("openrouter"), seeded.get("deepseek")),
            (Some(&true), Some(&true)),
            "precondition: both writes must have published `true`, otherwise the \
delete assertions below would pass without exercising anything: {seeded:?}"
        );

        router_with_credentials(credentials.clone(), false)
            .await
            .route(
                ClientCommand::DeleteProviderCredential {
                    operation_id: 3,
                    provider_id: "openrouter".into(),
                },
                sink.clone(),
            )
            .await;
        router_with_credentials(credentials, true)
            .await
            .route(
                ClientCommand::DeleteProviderCredential {
                    operation_id: 4,
                    provider_id: "deepseek".into(),
                },
                sink,
            )
            .await;

        let published = availability.read().expect("availability lock").clone();
        assert_eq!(
            published.get("openrouter"),
            Some(&false),
            "the persistent Settings credential DELETE must clear Fusion's \
availability entry; a stale `true` here is what lets /fusion auto-select \
openrouter and burn a panel slot on LlmError::Authentication: {published:?}"
        );
        assert_eq!(
            published.get("deepseek"),
            Some(&false),
            "the packaged/brokered EPHEMERAL branch of the same arm must clear \
too — its key never touched the keychain, so only the delete-side \
notification can lower the entry: {published:?}"
        );
    }
}
