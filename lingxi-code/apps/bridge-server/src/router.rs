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
//! Listing kinds with no engine handle or host store in the foundation
//! (`Memory`, `Settings`) remain unrouted here.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client_adapter::lowering::{
    lower_agent_info, lower_doctor_report, lower_hook_info, lower_mcp_server_info,
    lower_status_snapshot, lower_task_output_chunk, lower_task_record,
};
use client_adapter::ClientEventSink;
use client_protocol::commands::{ClientCommand, ListingKindDto};
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
                let models = traits::curated_model_refs(
                    &listings,
                    &available,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let current =
                    traits::qualified_model_ref(&snapshot.model, snapshot.model_profile.as_deref());
                sink.emit(ClientEvent::ModelList { models, current }).await;
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
            ListingKindDto::Memory | ListingKindDto::Settings => {
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
                            .await
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
                let runtime_snapshot = replayed.handle_runtime_snapshot();
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
