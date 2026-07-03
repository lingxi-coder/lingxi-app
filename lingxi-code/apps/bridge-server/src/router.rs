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
//! | `ListModels` / `RefreshListings{Models}` | `list_available_models` + status | `ModelList` |
//! | `RefreshListings{Mcp}` | `list_mcp_servers` | `McpServers` |
//! | `RefreshListings{Hooks}` | `list_hooks` | `Hooks` |
//! | `RefreshListings{Agents}` | `list_agents` | `Agents` |
//! | `RefreshListings{Status}` | `get_status_snapshot` | `StatusSnapshot` |
//! | `RefreshListings{Doctor}` | `run_doctor_checks` | `DoctorReport` |
//! | `RefreshListings{Auth}` / `Login` / `Logout` | `AuthHandle::*` | `AuthState` |
//! | `RefreshListings{Tasks}` / `TaskList` | `TaskRegistryHandle::list` | `TaskRow`× |
//! | `RunSlashCommand` | `SlashCommandDispatcher::dispatch` | `TextDelta` (lossy display) |
//! | `TaskOutput` | `TaskRegistryHandle::output` | `TaskOutputChunk` |
//! | `TaskStop` | `TaskRegistryHandle::kill` | `TaskStatusChanged` |
//! | `ForceCompact` | `force_compact` | `CompactionCompleted` |
//! | `ClearSession` | `clear_session` (REJECTED mid-turn) | `SessionEnded` / `Error` |
//! | `RequestExit` | `request_exit` | — |
//!
//! ## Mid-turn semantics
//!
//! `ClearSession` is **rejected while a turn is in flight** (plan §2): the
//! connection sets [`EngineCommandRouter::set_turn_active`] on `SendPrompt` and
//! clears it on turn end; a `ClearSession` arriving in that window is refused
//! with an [`ClientEvent::Error`] and never reaches the engine.
//!
//! ## Reserved / feed-deferred
//!
//! Per governing decisions §0.7/§0.9, the router NEVER live-sources the reserved
//! DTOs (`ThinkingDelta`, `UsageUpdate`, `CoordinatorStatus`); the corresponding
//! engine sources do not exist in the foundation, so no command maps to them.
//! Listing kinds with no engine handle in the foundation (`Sessions`, `Memory`,
//! `Settings`, `SlashCommands`) are left unrouted here — they are HOST/engine-tier
//! reads the binary wires when it has the desktop runtime in hand (plan §2);
//! routing them is additive and does not change this seam's shape.

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
use client_protocol::listings::{AuthStateDto, TaskStatusDto};
use tokio_util::sync::CancellationToken;
use traits::auth::{AuthHandle, LoginInfo};
use traits::orchestrator::OrchestratorHandle;
use traits::task_registry::{TaskListFilter, TaskRegistryHandle};
use traits::SlashCommandDispatcher;

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
    async fn dispatch_slash(&self, _raw: &str) -> Option<traits::SlashDispatchResult> {
        None
    }
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
    /// Set while a turn is in flight — `ClearSession` is rejected in this window
    /// (plan §2 mid-turn semantics).
    turn_active: AtomicBool,
}

impl EngineCommandRouter {
    /// Build a router from the engine handles a desktop runtime exposes.
    #[must_use]
    pub fn new(
        handle: Arc<dyn OrchestratorHandle>,
        auth: Arc<dyn AuthHandle>,
        tasks: Arc<dyn TaskRegistryHandle>,
        dispatcher: Option<Arc<dyn SlashCommandDispatcher>>,
    ) -> Self {
        Self {
            handle,
            auth,
            tasks,
            dispatcher,
            turn_active: AtomicBool::new(false),
        }
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

    /// Pull + emit a single listing kind. Listing kinds with no engine handle in
    /// the foundation are skipped (see module docs).
    async fn emit_listing(&self, kind: ListingKindDto, sink: &dyn ClientEventSink) {
        match kind {
            ListingKindDto::Models => {
                let models = self.handle.list_available_models().await;
                let current = self.handle.get_status_snapshot().await.model;
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
            // HOST/engine-tier reads the binary wires once it holds the desktop
            // runtime (plan §2). No engine handle for these in the foundation —
            // routing them is additive and does not change this seam's shape.
            ListingKindDto::Sessions
            | ListingKindDto::SlashCommands
            | ListingKindDto::Memory
            | ListingKindDto::Settings => {
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
    async fn dispatch_slash(&self, raw: &str) -> Option<traits::SlashDispatchResult> {
        let dispatcher = self.dispatcher.as_ref()?;
        Some(dispatcher.dispatch(raw).await)
    }

    // The full command dispatch is one match over the command surface; splitting
    // it would scatter the one-place-routes-everything map this module exists to
    // be. (Same convention as `engine_desktop::build`.)
    #[allow(clippy::too_many_lines)]
    async fn route(&self, command: ClientCommand, sink: Arc<dyn ClientEventSink>) {
        match command {
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
                        sink.emit(ClientEvent::ModelChanged { model: model_id })
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

            // ── Slash commands (LOSSY: display surfaced as TextDelta) ────────
            ClientCommand::RunSlashCommand { raw } => {
                if let Some(dispatcher) = self.dispatcher.as_ref() {
                    let display = match dispatcher.dispatch(&raw).await {
                        traits::SlashDispatchResult::Handled { display }
                        | traits::SlashDispatchResult::Unknown { display, .. } => display,
                        // A `type: "prompt"` command reached the display-only
                        // fallback (the connection should have intercepted it via
                        // `dispatch_slash` and run it as a turn). Surface the
                        // expanded prompt so nothing is silently dropped.
                        traits::SlashDispatchResult::RunAsTurn { prompt } => prompt,
                        traits::SlashDispatchResult::NotASlashCommand => {
                            format!("not a slash command: {raw}")
                        }
                    };
                    sink.emit(ClientEvent::TextDelta { text: display }).await;
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
            ClientCommand::TaskStop { task_id } => match self.tasks.kill(&task_id).await {
                Ok(rec) => {
                    sink.emit(ClientEvent::TaskStatusChanged {
                        task_id: rec.task_id,
                        status: client_adapter::lowering::lower_task_status(&rec.status),
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
            },

            // ── Handled elsewhere / not routed by this seam ──────────────────
            //
            // `SendPrompt` + `Cancel` are the turn path (`TurnDriver`), and
            // `ApprovePermission`/`DenyPermission` the permission path — both on
            // `BridgeConnection` directly. Session New/Resume + their listing
            // (`Sessions`) are HOST-driven swaps the binary owns (decision §0.5).
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

/// Lower a [`TaskStatusDto`] back to the registry's wire status string for the
/// `TaskListFilter` (the inverse of `client_adapter::lowering::lower_task_status`).
fn task_status_wire(status: TaskStatusDto) -> String {
    match status {
        TaskStatusDto::Running => "running",
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
