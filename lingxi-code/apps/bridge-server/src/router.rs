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
//! ([`traits::OrchestratorHandle`], [`traits::AuthHandle`],
//! [`traits::task_registry::TaskRegistryHandle`], and the slash dispatcher); a
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client_adapter::lowering::{
    lower_agent_info, lower_doctor_report, lower_hook_info, lower_mcp_server_info,
    lower_skill_info, lower_status_snapshot, lower_task_output_chunk, lower_task_record,
};
use client_adapter::ClientEventSink;
use client_protocol::commands::{
    ClientCommand, ListingKindDto, McpScopeDto, PermissionBehaviorDto, SettingsDestinationDto,
};
use client_protocol::controls::{
    ConversationControlsDto, ReasoningControlStateDto, ReasoningSelectionDto,
};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use client_protocol::listings::{AuthStateDto, SlashCommandDto, TaskStatusDto};
use command_api::builtin_support::names::{core_description, is_palette_hidden};
use command_api::model::CommandSource;
use command_api::registry::CommandRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use traits::auth::{AuthHandle, LoginInfo};
use traits::orchestrator::OrchestratorHandle;
use traits::task_registry::{TaskListFilter, TaskRegistryHandle};
use traits::SlashCommandDispatcher;

use crate::mcp_bridge::McpPaths;
use crate::settings_bridge::{
    apply_patch, build_snapshot, lower_snapshot, permission_destination, permission_paths,
    permission_rule_from_wire, SettingsContext,
};

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
    fs: Arc<dyn traits::FileSystem>,
}

impl SessionStoreContext {
    /// Build a session-store context rooted at the desktop config directory and
    /// the connection's project cwd.
    #[must_use]
    pub fn new(lingxi_home: PathBuf, session_cwd: String, fs: Arc<dyn traits::FileSystem>) -> Self {
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
    pub result: traits::SlashDispatchResult,
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

impl EngineCommandRouter {
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
            settings: None,
            mcp: None,
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
            context.active.clone(),
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

    async fn emit_provider_credential_status(
        &self,
        operation_id: u64,
        provider_ids: &[String],
        operation_error: Option<String>,
        sink: &dyn ClientEventSink,
    ) {
        let Some(credentials) = self.credentials.as_ref() else {
            sink.emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids: Vec::new(),
                unavailable_provider_ids: provider_ids.to_vec(),
                storage_encrypted: false,
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
                error: Some(error),
            })
            .await;
            return;
        }

        let mut configured_provider_ids = Vec::new();
        let mut unavailable_provider_ids = Vec::new();
        let mut failures = Vec::new();
        for provider_id in provider_ids {
            match credentials.get_provider_key(provider_id).await {
                Ok(Some(_)) => configured_provider_ids.push(provider_id.clone()),
                Ok(None) => {}
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
            model: traits::qualified_model_ref(&snapshot.model, snapshot.model_profile.as_deref()),
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
                let curated = traits::curated_model_listings(
                    &listings,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let models = traits::curated_model_refs(
                    &listings,
                    &available,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let current =
                    traits::qualified_model_ref(&snapshot.model, snapshot.model_profile.as_deref());
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
        let dispatcher = self.dispatcher.as_ref()?;
        let before = self.capture_slash_authority().await;
        let result = dispatcher.dispatch(raw).await;
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
            } => {
                let validation_error = if provider_ids.len() > 32
                    || provider_ids.iter().any(|id| !provider_id_is_valid(id))
                {
                    Some("invalid provider credential query".to_string())
                } else {
                    None
                };
                self.emit_provider_credential_status(
                    operation_id,
                    &provider_ids,
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
                let error = if !provider_id_is_valid(&provider_id)
                    || credential.expose_secret().is_empty()
                    || credential.expose_secret().len() > 16_384
                    || credential.expose_secret().contains('\0')
                {
                    Some("invalid provider credential".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    credentials
                        .set_provider_key(&provider_id, credential.expose_secret())
                        .await
                        .err()
                        .map(|failure| format!("failed to store provider credential: {failure}"))
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                let applied = error.is_none();
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
                    error,
                })
                .await;
            }
            ClientCommand::DeleteProviderCredential {
                operation_id,
                provider_id,
            } => {
                let error = if !provider_id_is_valid(&provider_id) {
                    Some("invalid provider id".to_string())
                } else if let Some(credentials) = self.credentials.as_ref() {
                    credentials
                        .delete_provider_key(&provider_id)
                        .await
                        .err()
                        .map(|failure| format!("failed to delete provider credential: {failure}"))
                } else {
                    Some("provider credential storage is unavailable".to_string())
                };
                let applied = error.is_none();
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
                    error,
                })
                .await;
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
                        traits::live_sessions::set_process_permission_mode(
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
                let (model_id, profile) = traits::parse_model_ref(&model, &listings);
                match self
                    .handle
                    .switch_model(&model_id, profile.as_deref())
                    .await
                {
                    Ok(()) => {
                        let selected = traits::qualified_model_ref(&model_id, profile.as_deref());
                        sink.emit(ClientEvent::ModelChanged { model: selected })
                            .await;
                        if let Some(controls) = self.handle.conversation_controls().await {
                            if controls.requested_reasoning_selection
                                != traits::ReasoningSelection::Automatic
                                && controls.requested_reasoning_selection
                                    != controls.effective_reasoning_selection
                            {
                                let _ = self
                                    .handle
                                    .set_reasoning_selection(traits::ReasoningSelection::Automatic)
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
                        traits::SlashDispatchResult::Handled { display } => (display, false),
                        traits::SlashDispatchResult::Unknown { display, .. } => (display, true),
                        // A `type: "prompt"` command reached the display-only
                        // fallback (the connection should have intercepted it via
                        // `dispatch_slash` and run it as a turn). Surface the
                        // expanded prompt so nothing is silently dropped.
                        traits::SlashDispatchResult::RunAsTurn { prompt } => (prompt, false),
                        traits::SlashDispatchResult::NotASlashCommand => {
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

            // ── Auth ───────────────────────────────────────────────────────
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
                    let (model_id, profile) = traits::parse_model_ref(&model, &listings);
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
                sink.emit(ClientEvent::SessionStarted { session_id }).await;
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
                if let Err(error) =
                    self.handle
                        .resume_session(
                            protocol::SessionId::from_uuid(uuid),
                            replayed.state.history,
                            replayed.last_message_uuid.map(|value| value.to_string()),
                            replayed.state.active_goal.clone().map(|goal| {
                                traits::ActiveGoalSnapshot {
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
                        model: traits::qualified_model_ref(
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

fn lower_reasoning_selection(selection: &traits::ReasoningSelection) -> ReasoningSelectionDto {
    match selection {
        traits::ReasoningSelection::Automatic => ReasoningSelectionDto::Automatic,
        traits::ReasoningSelection::Disabled => ReasoningSelectionDto::Disabled,
        traits::ReasoningSelection::Enabled => ReasoningSelectionDto::Enabled,
        traits::ReasoningSelection::Level { id } => ReasoningSelectionDto::Level { id: id.clone() },
        traits::ReasoningSelection::TokenBudget { tokens } => {
            ReasoningSelectionDto::TokenBudget { tokens: *tokens }
        }
    }
}

fn decode_reasoning_selection(selection: ReasoningSelectionDto) -> traits::ReasoningSelection {
    match selection {
        ReasoningSelectionDto::Automatic => traits::ReasoningSelection::Automatic,
        ReasoningSelectionDto::Disabled => traits::ReasoningSelection::Disabled,
        ReasoningSelectionDto::Enabled => traits::ReasoningSelection::Enabled,
        ReasoningSelectionDto::Level { id } => traits::ReasoningSelection::Level { id },
        ReasoningSelectionDto::TokenBudget { tokens } => {
            traits::ReasoningSelection::TokenBudget { tokens }
        }
        _ => traits::ReasoningSelection::Automatic,
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
