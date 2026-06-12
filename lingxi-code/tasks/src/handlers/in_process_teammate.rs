//! In-process-teammate task handler — M2 implementation.
//!
//! A teammate is a **persistent, message-driven** subagent. Unlike a one-shot
//! `local_agent` run, it does not terminate at the end of a turn-set: after the
//! model stops it parks awaiting the next inbound user message, runs the next
//! turn-set, and so on, until cooperatively shut down. This mirrors the
//! claude-code `InProcessTeammateTask` lifecycle, whose state machine alternates
//! between *processing turns* and *idle-awaiting-input*, accepting injected
//! messages whenever the task is not terminal (`injectUserMessageToTeammate`).
//!
//! ## How persistence is wired
//!
//! The persistence lives entirely in the [`agent`] crate: [`spawn`](Task::spawn)
//! builds a [`agent::SubagentContext`] with `persistent = true` and hands it to
//! [`agent::StateMachinePool::allocate`]. The pool's runner parks on its inbound
//! `event_rx` between turn-sets. This handler:
//!
//! * routes typed text into the running agent via
//!   [`agent::StateMachinePool::send_event`] with an
//!   [`engine::Event::UserMessage`] (the Rust analogue of
//!   `injectUserMessageToTeammate`), and
//! * pumps the agent's outbound [`agent::SubagentEvent`] stream into the task's
//!   spool file (one line per event), reporting terminal status to a
//!   [`TaskStatusSink`] on `Completed` / `Failed` / `Killed`.
//!
//! ## Shutdown ordering (cooperative, then hard)
//!
//! [`kill`](Task::kill) sends [`engine::Event::UserExit`] first (giving the
//! runner a chance to emit a clean `Killed` the streaming worker spools), then
//! hard-cancels the slot via [`agent::StateMachinePool::deallocate`]. This
//! matches the TS `requestTeammateShutdown` (cooperative) → `kill` (hard)
//! ordering.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};
use crate::id::{generate_task_id, TaskType};
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};

use agent::context::SubagentContext;
use agent::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use agent::display::{AgentColor, AgentDisplay};
use agent::pool::{PoolError, StateMachinePool};
use agent::resolve_agent_model;
use agent::runner::SubagentEvent;
use agent::SubagentApiClient;

/// Handler name reported by [`Task::name`] and used as the runtime task-name
/// prefix.
const HANDLER_NAME: &str = "in_process_teammate";

/// Resolves the static [`AgentDefinition`] for a teammate spawn.
///
/// The handler does not own the agent catalog (it would be an over-broad
/// dependency), so definition lookup is delegated through this narrow seam —
/// the same pattern as [`TaskStatusSink`]. The wire step plugs in an adapter
/// over the host's loaded agent registry; [`DefaultTeammateDefinition`] makes
/// the handler usable standalone (and in unit tests) by synthesizing a
/// permissive built-in definition.
pub trait TeammateDefinitionResolver: Send + Sync {
    /// Resolve the definition for the teammate identified by `agent_id` /
    /// `name`. Returns `None` when no such definition exists.
    fn resolve(&self, agent_id: &protocol::AgentId, name: &str) -> Option<AgentDefinition>;
}

/// Default resolver that synthesizes a permissive built-in definition. Lets the
/// handler run without a wired agent catalog (tests / standalone use).
pub struct DefaultTeammateDefinition;

impl TeammateDefinitionResolver for DefaultTeammateDefinition {
    fn resolve(&self, _agent_id: &protocol::AgentId, name: &str) -> Option<AgentDefinition> {
        Some(AgentDefinition {
            agent_type: name.to_string(),
            when_to_use: String::new(),
            tools: AgentToolPolicy::All {
                use_exact_tools: true,
            },
            max_turns: 64,
            model: AgentModel::Inherit,
            permission_mode: AgentPermissionMode::Bubble,
            source: AgentSource::BuiltIn,
            base_dir: "/tmp".into(),
            system_prompt: None,
            mcp_servers: vec![],
            frontmatter_hooks: vec![],
            icon: None,
            allowed_tools: vec![],
            worktree_requirement: None,
        })
    }
}

/// Per-task control block held by the handler so [`Task::send_message`] can
/// route input to the live slot and [`Task::kill`] can tear it down.
struct TeammateEntry {
    /// Slot key in the [`StateMachinePool`].
    agent_id: protocol::AgentId,
    /// Cooperative stop flag for the streaming worker. The worker also exits
    /// naturally when `out_rx` closes (the slot's runner future drops its
    /// sender on deallocate); the flag is the belt-and-braces fast path.
    stop: Arc<std::sync::atomic::AtomicBool>,
}

/// Handler for [`TaskType::InProcessTeammate`].
///
/// Holds the host slot pool plus the constructor-injected dependencies needed
/// to build a persistent [`SubagentContext`] (`api_client`, optional
/// `tool_invoker`, the definition resolver) and stream its output (the spool
/// `output` manager). `fs` + `runtime` arrive per-call via [`TaskContext`].
pub struct InProcessTeammateHandler {
    /// Host slot pool: `spawn` → `allocate`, `send_message` → `send_event`,
    /// `kill` → `send_event(UserExit)` + `deallocate`.
    pool: Arc<StateMachinePool>,
    /// Spool-file owner (same role as in `LocalBash` / `MonitorMcp`).
    output: Arc<TaskOutputManager>,
    /// Model API seam handed to every spawned teammate's runner.
    api_client: Arc<dyn SubagentApiClient>,
    /// Tool dispatch seam inherited by the teammate. `None` means the teammate
    /// cannot dispatch tools (a `tool_use` then surfaces a runner failure).
    tool_invoker: Option<Arc<dyn traits::ToolInvoker>>,
    /// Resolves the [`AgentDefinition`] for a spawn.
    definitions: Arc<dyn TeammateDefinitionResolver>,
    /// Parent / main-loop model used to resolve a teammate definition's
    /// `AgentModel::Inherit` / family aliases to a concrete wire id (mirrors
    /// `PoolSubagentSpawner::default_model`). Set at boot from `cfg.model` via
    /// [`Self::with_default_model`]. `None` (the default / tests) leaves the
    /// definition's model RAW (legacy: the runner emits `Inherit`→`"inherit"`).
    default_model: Option<String>,
    /// Terminal-status sink (same seam as `LocalBashHandler`).
    status_sink: Arc<dyn TaskStatusSink>,
    /// Best-effort seam to fire the `TeammateIdle` hook each time the persistent
    /// runner finishes a turn-set and the teammate is about to park awaiting the
    /// next message ("about to go idle"). `None` (the default) => strict no-op;
    /// the orchestrator injects a real firer via
    /// [`with_teammate_idle_firer`](Self::with_teammate_idle_firer). Mirrors the
    /// `TaskStatusSink` decoupling: the `tasks` leaf cannot reach a live hook
    /// executor, so it calls through this narrow trait instead.
    teammate_idle_firer: hooks::OptionalTeammateIdleFirer,
    /// `task_id` → control block, so `send_message` / `kill` can find the slot.
    entries: Arc<Mutex<HashMap<String, TeammateEntry>>>,
}

impl InProcessTeammateHandler {
    /// Construct a handler with the injected execution dependencies.
    ///
    /// Uses [`DefaultTeammateDefinition`] + [`NoopStatusSink`] by default; swap
    /// them via [`Self::with_definitions`] / [`Self::with_status_sink`].
    #[must_use]
    pub fn new(
        pool: Arc<StateMachinePool>,
        output: Arc<TaskOutputManager>,
        api_client: Arc<dyn SubagentApiClient>,
    ) -> Self {
        Self {
            pool,
            output,
            api_client,
            tool_invoker: None,
            definitions: Arc::new(DefaultTeammateDefinition),
            default_model: None,
            status_sink: Arc::new(NoopStatusSink),
            teammate_idle_firer: None,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach the tool dispatch seam inherited by spawned teammates.
    #[must_use]
    pub fn with_tool_invoker(mut self, invoker: Arc<dyn traits::ToolInvoker>) -> Self {
        self.tool_invoker = Some(invoker);
        self
    }

    /// Set the parent / main-loop model used to resolve a teammate's
    /// `AgentModel::Inherit` / bare family aliases to a concrete wire id (see
    /// [`agent::resolve_agent_model`]). Wire this from `cfg.model` at boot;
    /// without it, the definition's model is passed through raw (legacy).
    #[must_use]
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Attach a custom [`TeammateDefinitionResolver`] (e.g. an adapter over the
    /// host's loaded agent catalog).
    #[must_use]
    pub fn with_definitions(mut self, definitions: Arc<dyn TeammateDefinitionResolver>) -> Self {
        self.definitions = definitions;
        self
    }

    /// Attach a [`TaskStatusSink`] so terminal transitions are reported.
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Inject the best-effort `TeammateIdle` hook firer. Default-`None` builder
    /// (the firer-seam pattern): existing constructors and tests stay no-op; the
    /// composition root threads the orchestrator's firer here so each completed
    /// turn-set (the teammate parking awaiting the next message) fires the
    /// `TeammateIdle` hook — claude-code `executeTeammateIdleHooks`
    /// (`stopHooks.ts:403`).
    #[must_use]
    pub fn with_teammate_idle_firer(
        mut self,
        firer: Arc<dyn hooks::TeammateIdleFirer>,
    ) -> Self {
        self.teammate_idle_firer = Some(firer);
        self
    }

    /// Build the persistent [`SubagentContext`] for a spawn.
    fn build_context(
        &self,
        agent_id: protocol::AgentId,
        mut definition: AgentDefinition,
    ) -> SubagentContext {
        // Resolve the model preference to a concrete wire id, mirroring the
        // `PoolSubagentSpawner` seam (`Inherit`→parent model, family alias→
        // concrete id), so a wired teammate runs against a live provider. Unset
        // `default_model` (tests / no boot wiring) leaves it RAW (legacy).
        if let Some(parent_model) = &self.default_model {
            definition.model =
                AgentModel::Explicit(resolve_agent_model(&definition.model, parent_model));
        }
        let icon = definition.icon.clone();
        SubagentContext {
            agent_id,
            parent_agent_id: None,
            agent_definition: definition,
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            // The defining trait of a teammate: park between turn-sets and
            // resume on the next injected UserMessage.
            persistent: true,
            can_show_permission_prompts: true,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt: None,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon,
            },
            api_client: Some(self.api_client.clone()),
            tool_invoker: self.tool_invoker.clone(),
            // Teammates do not yet advertise tool schemas to the model (same
            // boot-wiring gap as `PoolSubagentSpawner`). Empty = no tools
            // advertised, so the teammate cannot emit `tool_use`. NOTE: the
            // dispatch seam does NOT enforce per-agent policy today (see the
            // `SubagentContext::tool_schemas` WARNING), so keeping this empty is
            // also what prevents advertising tools the agent's policy forbids.
            tool_schemas: vec![],
            // Teammates do not currently inherit a budget enforcer; `None`
            // preserves today's behavior (no per-turn budget gate) and is
            // purely additive.
            budget: None,
        }
    }
}

/// Render one outbound [`SubagentEvent`] as a spool line (no trailing newline;
/// the appender adds it). `None` for events we do not surface.
fn event_line(ev: &SubagentEvent) -> String {
    match ev {
        SubagentEvent::Progress {
            tool_use_count,
            token_count,
            ..
        } => format!("progress: tool_uses={tool_use_count} tokens={token_count}"),
        SubagentEvent::Message { message, .. } => {
            format!("message: {message}")
        }
        SubagentEvent::Completed { result, .. } => format!("completed: {result}"),
        SubagentEvent::Failed { error, .. } => format!("failed: {error}"),
        SubagentEvent::Killed { .. } => "killed".to_string(),
    }
}

/// Map a [`SubagentEvent`] to the terminal [`TaskStatus`] that ends the
/// teammate. `None` for events that do NOT terminate it.
///
/// Crucially, a persistent teammate emits a `Completed` at the end of *every*
/// turn-set yet keeps running (it then parks awaiting the next message), so
/// `Completed` is NOT terminal here — treating it as terminal would make the
/// streaming worker stop, drop `out_rx`, and strand all subsequent turn-sets on
/// a closed channel. Only `Failed` / `Killed` truly end the teammate.
fn terminal_status(ev: &SubagentEvent) -> Option<TaskStatus> {
    match ev {
        SubagentEvent::Failed { .. } => Some(TaskStatus::Failed),
        SubagentEvent::Killed { .. } => Some(TaskStatus::Killed),
        SubagentEvent::Completed { .. }
        | SubagentEvent::Progress { .. }
        | SubagentEvent::Message { .. } => None,
    }
}

/// Whether `ev` marks the teammate "about to go idle" — i.e. a turn-set finished
/// and the persistent runner is about to park awaiting the next message.
///
/// This is the Rust analogue of claude-code's `isTeammate()`-gated
/// `executeTeammateIdleHooks` fire (`stopHooks.ts:403`), which runs after the
/// teammate's query loop stops. A persistent teammate emits exactly one
/// `Completed` at the end of *every* turn-set yet keeps running (it is NOT
/// terminal here — see [`terminal_status`]), so `Completed` is precisely the
/// idle moment. `Failed` / `Killed` are terminal (the teammate ends, it does not
/// idle), and `Progress` / `Message` are mid-turn, so none of them are idle.
fn is_idle_event(ev: &SubagentEvent) -> bool {
    matches!(ev, SubagentEvent::Completed { .. })
}

#[async_trait]
impl Task for InProcessTeammateHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::InProcessTeammate
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        // 1. Only the InProcessTeammate variant is accepted.
        let TaskSpawnInput::InProcessTeammate { agent_id, name } = input else {
            return Err(TaskError::Internal(
                "in_process_teammate handler received a non-InProcessTeammate spawn input".into(),
            ));
        };

        // 2. Allocate the task id + spool file.
        let task_id = generate_task_id(TaskType::InProcessTeammate);
        let spool_path = self
            .output
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        let spool = spool_path
            .to_str()
            .ok_or_else(|| TaskError::Internal("spool path is not valid UTF-8".into()))?
            .to_string();

        // 3. Resolve the definition and build a persistent SubagentContext.
        let definition = self
            .definitions
            .resolve(&agent_id, &name)
            .ok_or_else(|| TaskError::Internal(format!("no agent definition for teammate {name}")))?;
        let subagent_ctx = self.build_context(agent_id, definition);

        // 4. Allocate the slot — the pool spawns the persistent runner and
        //    hands back the outbound SubagentEvent stream.
        let (aid, mut out_rx) = self
            .pool
            .allocate(subagent_ctx)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        // 5. Spawn the streaming worker through the runtime (never tokio::spawn
        //    — D17). It pumps out_rx -> spool, one line per event, and reports
        //    terminal status. It stops on Failed / Killed or when out_rx closes
        //    (the slot's runner dropped its sender on deallocate); it does NOT
        //    stop on Completed, since a persistent teammate emits one Completed
        //    per turn-set yet keeps running.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_loop = stop.clone();
        let fs = ctx.fs.clone();
        let status_sink = self.status_sink.clone();
        // Best-effort `TeammateIdle` firer + the teammate name it carries. The
        // team_name is not reachable from this leaf scope (it lives on the
        // coordinator's TeamRegistry), so it rides as `""` — faithful to
        // claude-code's `getTeamName() ?? ''` fallback and the same documented
        // leaf-scope gap as the `TaskCompleted` `team_name`.
        let idle_firer = self.teammate_idle_firer.clone();
        let idle_name = name.clone();
        let worker_task_id = task_id.clone();
        let worker = Box::pin(async move {
            status_sink
                .set_status(&worker_task_id, TaskStatus::Running)
                .await;
            while let Some(ev) = out_rx.recv().await {
                if stop_loop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let line = event_line(&ev);
                if let Err(e) = fs.append_file(&spool, &format!("{line}\n")).await {
                    tracing::warn!(
                        target: "lingxi_tasks::in_process_teammate",
                        spool, error = %e, "spool append failed"
                    );
                }
                // A completed (but non-terminal) turn-set is the "about to go
                // idle" moment: the runner is about to park awaiting the next
                // message. Fire the `TeammateIdle` hook best-effort — claude-code
                // `executeTeammateIdleHooks` (`stopHooks.ts:403`). No firer wired
                // => no-op (byte-identical to the pre-firer build).
                if is_idle_event(&ev) {
                    if let Some(firer) = &idle_firer {
                        firer
                            .fire(hooks::TeammateIdleFire {
                                teammate_name: idle_name.clone(),
                                team_name: String::new(),
                            })
                            .await;
                    }
                }
                if let Some(status) = terminal_status(&ev) {
                    // Failed / Killed end the teammate; a per-turn-set Completed
                    // does not (terminal_status returns None for it), so the
                    // worker keeps pumping subsequent turn-sets.
                    status_sink.set_status(&worker_task_id, status).await;
                    break;
                }
            }
        });

        ctx.runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        // 6. Record the control block.
        self.entries.lock().await.insert(
            task_id.clone(),
            TeammateEntry {
                agent_id: aid,
                stop: stop.clone(),
            },
        );

        // 7. Cleanup seam: synchronous, so it only flips the streaming worker's
        //    stop flag. Authoritative teardown (UserExit + deallocate) flows
        //    through the async `Task::kill`, which the registry invokes.
        let cleanup_stop = stop;
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            cleanup_stop.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        Ok(TaskHandle {
            task_id,
            cleanup: Some(cleanup),
        })
    }

    fn supports_messages(&self) -> bool {
        true
    }

    async fn send_message(
        &self,
        task_id: &str,
        message: String,
        _ctx: TaskContext,
    ) -> Result<(), TaskError> {
        // Look up the live slot. An unknown id means the teammate was never
        // spawned (or was already killed and removed).
        let agent_id = {
            let entries = self.entries.lock().await;
            entries
                .get(task_id)
                .map(|e| e.agent_id)
                .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?
        };

        // Route the typed text into the running agent. The runner's persist-mode
        // recv() picks it up, appends it to history, and runs the next turn-set
        // — the Rust analogue of injectUserMessageToTeammate.
        self.pool
            .send_event(
                &agent_id,
                engine::Event::UserMessage {
                    message_id: protocol::MessageId::new(),
                    request_id: protocol::RequestId::new(),
                    content: message,
                },
            )
            .await
            .map_err(|e| match e {
                // The slot is gone (runner dropped its receiver) ⇒ the task is
                // effectively terminated — mirror the TS drop-when-terminal guard.
                PoolError::AgentGone | PoolError::NoSuchAgent => TaskError::TerminatedTask,
                other => TaskError::Internal(other.to_string()),
            })
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        // Remove the control block. An absent entry is a graceful no-op
        // (already killed / never spawned), mirroring local_bash.
        let entry = self.entries.lock().await.remove(task_id);
        let Some(entry) = entry else {
            return Ok(());
        };

        // Cooperative stop first: give the runner a chance to emit a clean
        // Killed (which the streaming worker spools) before the hard cancel.
        // A send failure (slot already gone) is non-fatal — proceed to
        // deallocate, which is itself idempotent.
        let _ = self
            .pool
            .send_event(&entry.agent_id, engine::Event::UserExit)
            .await;

        // Stop the streaming worker, then hard-cancel the slot (deallocate
        // cancels the run_subagent task). out_rx closes when the slot drops, so
        // the worker would exit on its own too; the flag is the fast path.
        entry.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.pool
            .deallocate(&entry.agent_id)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        self.status_sink.set_status(task_id, TaskStatus::Killed).await;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::sync::Mutex as TokioMutex;
    use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use traits::RuntimeSpawner;

    // ---- In-memory FileSystem (mirrors the other handler tests) ------------

    struct InMemoryFs {
        files: TokioMutex<StdHashMap<String, String>>,
    }
    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(StdHashMap::new()),
            }
        }
    }
    #[async_trait]
    impl FileSystem for InMemoryFs {
        async fn read_file(
            &self,
            path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content,
                truncated: false,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            map.entry(path.to_string()).or_default().push_str(body);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    // ---- Scripted SubagentApiClient ----------------------------------------

    /// Hands back a pre-scripted queue of responses, one per `messages_create`.
    /// When exhausted it returns an `end_turn` text turn so each turn-set
    /// terminates and the persistent runner parks for the next message. An
    /// `Err` entry surfaces as an API error (driving the runner to `Failed`).
    struct ScriptedApiClient {
        responses: StdMutex<VecDeque<Result<llm_client::LlmResponse, String>>>,
        calls: AtomicUsize,
    }
    impl ScriptedApiClient {
        fn new(texts: Vec<&str>) -> Arc<Self> {
            let responses = texts
                .into_iter()
                .map(|t| Ok(text_response(t)))
                .collect::<VecDeque<_>>();
            Arc::new(Self {
                responses: StdMutex::new(responses),
                calls: AtomicUsize::new(0),
            })
        }
        /// Script a single API error so the first turn-set surfaces `Failed`.
        fn new_error(message: &str) -> Arc<Self> {
            let mut responses = VecDeque::new();
            responses.push_back(Err(message.to_string()));
            Arc::new(Self {
                responses: StdMutex::new(responses),
                calls: AtomicUsize::new(0),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl SubagentApiClient for ScriptedApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _system: Option<&str>,
            _messages: Vec<protocol::ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.responses.lock().unwrap().pop_front();
            match next {
                Some(Ok(resp)) => Ok(resp),
                Some(Err(msg)) => Err(llm_client::LlmError::InvalidRequest { message: msg }),
                None => Ok(text_response("(idle)")),
            }
        }
    }

    fn text_response(text: &str) -> llm_client::LlmResponse {
        llm_client::LlmResponse {
            id: "mock".into(),
            model: "mock".into(),
            content: vec![llm_client::ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
            stop_reason: Some("end_turn".into()),
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    // ---- Recording status sink ---------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<(String, TaskStatus)>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, task_id: &str, status: TaskStatus) {
            self.statuses
                .lock()
                .unwrap()
                .push((task_id.to_string(), status));
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().map(|(_, s)| *s)
        }
    }

    // ---- Recording TeammateIdle firer --------------------------------------

    /// A [`hooks::TeammateIdleFirer`] that records every fire it receives, so a
    /// test can assert the per-turn-set idle moment fired with the right payload.
    #[derive(Default)]
    struct RecordingIdleFirer {
        fires: StdMutex<Vec<hooks::TeammateIdleFire>>,
    }
    #[async_trait]
    impl hooks::TeammateIdleFirer for RecordingIdleFirer {
        async fn fire(&self, fire: hooks::TeammateIdleFire) {
            self.fires.lock().unwrap().push(fire);
        }
    }
    impl RecordingIdleFirer {
        fn fires(&self) -> Vec<hooks::TeammateIdleFire> {
            self.fires.lock().unwrap().clone()
        }
    }

    // ---- Helpers ------------------------------------------------------------

    fn make_handler(
        api: Arc<ScriptedApiClient>,
    ) -> (
        tempfile::TempDir,
        Arc<dyn FileSystem>,
        Arc<MockRuntimeSpawner>,
        InProcessTeammateHandler,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let output = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let pool = Arc::new(StateMachinePool::new(
            runtime.clone() as Arc<dyn RuntimeSpawner>,
            8,
        ));
        let handler = InProcessTeammateHandler::new(pool, output, api);
        (dir, fs, runtime, handler)
    }

    /// Like [`make_handler`] but returns the attached [`RecordingSink`] so a
    /// test can observe terminal status transitions.
    fn make_handler_with_sink(
        api: Arc<ScriptedApiClient>,
    ) -> (
        tempfile::TempDir,
        Arc<dyn FileSystem>,
        Arc<MockRuntimeSpawner>,
        InProcessTeammateHandler,
        Arc<RecordingSink>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let output = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let pool = Arc::new(StateMachinePool::new(
            runtime.clone() as Arc<dyn RuntimeSpawner>,
            8,
        ));
        let sink = Arc::new(RecordingSink::default());
        let handler =
            InProcessTeammateHandler::new(pool, output, api).with_status_sink(sink.clone());
        (dir, fs, runtime, handler, sink)
    }

    /// Yield until the sink reports a terminal status, or the budget runs out.
    async fn await_terminal(sink: &Arc<RecordingSink>) -> Option<TaskStatus> {
        for _ in 0..400 {
            if let Some(s) = sink.last_status() {
                if s.is_terminal() {
                    return Some(s);
                }
            }
            tokio::task::yield_now().await;
        }
        sink.last_status()
    }

    fn ctx(fs: Arc<dyn FileSystem>, runtime: Arc<MockRuntimeSpawner>) -> TaskContext {
        TaskContext {
            fs,
            runtime: runtime as Arc<dyn RuntimeSpawner>,
        }
    }

    /// Yield until `pred` over the spool body holds, or the budget runs out.
    async fn await_spool<F: Fn(&str) -> bool>(
        fs: &Arc<dyn FileSystem>,
        spool: &str,
        pred: F,
    ) -> String {
        for _ in 0..400 {
            let body = fs.read_file(spool, None, None).await.unwrap().content;
            if pred(&body) {
                return body;
            }
            tokio::task::yield_now().await;
        }
        fs.read_file(spool, None, None).await.unwrap().content
    }

    // ---- Tests --------------------------------------------------------------

    /// Minimal handler for the `build_context` model-resolution tests (no spawn).
    fn model_test_handler(default_model: Option<&str>) -> InProcessTeammateHandler {
        let dir = tempfile::tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let output = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime as Arc<dyn RuntimeSpawner>, 8));
        let handler = InProcessTeammateHandler::new(pool, output, ScriptedApiClient::new(vec!["ok"]));
        match default_model {
            Some(m) => handler.with_default_model(m),
            None => handler,
        }
    }

    #[test]
    fn build_context_resolves_inherit_to_default_model() {
        // DefaultTeammateDefinition yields AgentModel::Inherit; with a default
        // model wired the teammate ctx carries a concrete wire id (folded via
        // the same `resolve_agent_model` seam as the spawner).
        let handler = model_test_handler(Some("claude-opus-4-7"));
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler.build_context(protocol::AgentId::new(), def);
        assert!(matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-opus-4-7"));
    }

    #[test]
    fn build_context_without_default_model_leaves_model_raw() {
        // No default model wired → legacy behavior: Inherit is left untouched.
        let handler = model_test_handler(None);
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler.build_context(protocol::AgentId::new(), def);
        assert!(matches!(&ctx.agent_definition.model, AgentModel::Inherit));
    }

    #[tokio::test]
    async fn name_type_and_supports_messages() {
        let api = ScriptedApiClient::new(vec![]);
        let (_d, _fs, _rt, handler) = make_handler(api);
        assert_eq!(handler.name(), "in_process_teammate");
        assert_eq!(handler.task_type(), TaskType::InProcessTeammate);
        assert!(handler.supports_messages(), "teammates accept messages");
    }

    #[tokio::test]
    async fn non_teammate_input_is_rejected() {
        let api = ScriptedApiClient::new(vec![]);
        let (_d, fs, rt, handler) = make_handler(api);
        let res = handler
            .spawn(
                TaskSpawnInput::LocalBash {
                    command: "echo".into(),
                    timeout: None,
                },
                ctx(fs, rt),
            )
            .await;
        assert!(matches!(res, Err(TaskError::Internal(_))));
    }

    #[tokio::test]
    async fn spawn_send_message_then_kill_lifecycle() {
        // The persistent runner runs turn-set 1 immediately on spawn (empty
        // history -> "answer one"), then parks. An injected message un-idles it
        // and drives turn-set 2 ("answer two"), proving persistence (the slot
        // did not terminate after turn-set 1). The streaming worker spools a
        // `completed:` line per turn-set. kill then tears the slot down.
        let api = ScriptedApiClient::new(vec!["answer one", "answer two"]);
        let api_handle = api.clone();
        let (dir, fs, rt, handler) = make_handler(api);
        let c = ctx(fs.clone(), rt.clone());

        let h = handler
            .spawn(
                TaskSpawnInput::InProcessTeammate {
                    agent_id: protocol::AgentId::new(),
                    name: "buddy".into(),
                },
                c.clone(),
            )
            .await
            .unwrap();
        assert!(h.task_id.starts_with('t'), "teammate ids prefix 't'");
        assert!(h.cleanup.is_some(), "cleanup hook present");
        assert_eq!(handler.entries.lock().await.len(), 1, "spawn registers slot");

        let spool = dir.path().join(format!("{}.txt", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();

        // Turn-set 1 completes shortly after spawn.
        let body = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
        assert!(body.contains("completed:"), "turn-set 1 spooled: {body:?}");
        assert!(body.contains("answer one"), "turn-set 1 text: {body:?}");

        // Inject a message — drives turn-set 2 after the runner had idled.
        handler
            .send_message(&h.task_id, "follow-up question".into(), c.clone())
            .await
            .expect("send_message routes to the live slot while idle");
        let body = await_spool(&fs, &spool_str, |b| b.contains("answer two")).await;
        assert!(body.contains("answer two"), "turn-set 2 text: {body:?}");
        assert_eq!(api_handle.call_count(), 2, "one round-trip per turn-set");

        // Kill tears it down; the entry is removed.
        handler.kill(&h.task_id, c.clone()).await.unwrap();
        assert!(
            handler.entries.lock().await.is_empty(),
            "kill deregisters the slot"
        );

        // Killing an unknown id is a graceful no-op.
        handler
            .kill("tdeadbeef", c)
            .await
            .expect("kill of unknown id is a no-op success");
    }

    #[tokio::test]
    async fn send_message_to_unknown_task_is_not_found() {
        let api = ScriptedApiClient::new(vec![]);
        let (_d, fs, rt, handler) = make_handler(api);
        let err = handler
            .send_message("tnope", "hi".into(), ctx(fs, rt))
            .await
            .unwrap_err();
        assert!(matches!(err, TaskError::NotFound(_)), "got {err:?}");
    }

    /// A teammate whose first turn-set hits an API error surfaces `Failed`: the
    /// streaming worker spools a `failed:` line and reports `TaskStatus::Failed`
    /// (the terminal-break branch — distinct from the non-terminal per-turn-set
    /// `Completed`).
    #[tokio::test]
    async fn failed_turn_set_spools_failed_and_reports_terminal() {
        let api = ScriptedApiClient::new_error("boom");
        let (dir, fs, rt, handler, sink) = make_handler_with_sink(api);
        let c = ctx(fs.clone(), rt.clone());

        let h = handler
            .spawn(
                TaskSpawnInput::InProcessTeammate {
                    agent_id: protocol::AgentId::new(),
                    name: "buddy".into(),
                },
                c,
            )
            .await
            .unwrap();

        let spool = dir.path().join(format!("{}.txt", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();

        let body = await_spool(&fs, &spool_str, |b| b.contains("failed:")).await;
        assert!(body.contains("failed:"), "failed line spooled: {body:?}");
        assert!(body.contains("boom"), "error text spooled: {body:?}");

        assert_eq!(
            await_terminal(&sink).await,
            Some(TaskStatus::Failed),
            "Failed event reports terminal TaskStatus::Failed"
        );
    }

    /// `send_message` to a teammate that was already killed (slot deallocated +
    /// entry removed) is `NotFound`; and a message routed to a slot whose runner
    /// has terminated (its receiver dropped) surfaces `TerminatedTask`. We drive
    /// the latter by killing the slot via the pool directly so the handler's
    /// entry still exists but the underlying slot is gone.
    #[tokio::test]
    async fn send_message_after_runner_terminated_is_terminated_task() {
        let api = ScriptedApiClient::new(vec!["answer one"]);
        let (dir, fs, rt, handler) = make_handler(api);
        let c = ctx(fs.clone(), rt.clone());

        let h = handler
            .spawn(
                TaskSpawnInput::InProcessTeammate {
                    agent_id: protocol::AgentId::new(),
                    name: "buddy".into(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        // Let turn-set 1 land so the runner is parked and reachable.
        let spool = dir.path().join(format!("{}.txt", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();
        await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;

        // Tear down the underlying slot out-of-band (UserExit so the runner
        // drops its receiver), WITHOUT removing the handler's entry. The
        // handler still has a control block, so send_message looks the slot up
        // and finds the inbound channel closed -> AgentGone -> TerminatedTask.
        let aid = handler
            .entries
            .lock()
            .await
            .get(&h.task_id)
            .map(|e| e.agent_id)
            .unwrap();
        handler
            .pool
            .send_event(&aid, engine::Event::UserExit)
            .await
            .unwrap();
        // Wait until the slot's inbound channel is observably closed.
        for _ in 0..400 {
            if handler
                .pool
                .send_event(&aid, engine::Event::UserInterrupt)
                .await
                .is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }

        let err = handler
            .send_message(&h.task_id, "are you there?".into(), c)
            .await
            .unwrap_err();
        assert!(
            matches!(err, TaskError::TerminatedTask),
            "send to a terminated runner maps to TerminatedTask; got {err:?}"
        );
    }

    /// Like [`make_handler`] but attaches a [`RecordingIdleFirer`] (plus a
    /// `RecordingSink`) so a test can observe the per-turn-set `TeammateIdle`
    /// fire.
    fn make_handler_with_idle_firer(
        api: Arc<ScriptedApiClient>,
    ) -> (
        tempfile::TempDir,
        Arc<dyn FileSystem>,
        Arc<MockRuntimeSpawner>,
        InProcessTeammateHandler,
        Arc<RecordingIdleFirer>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let output = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let pool = Arc::new(StateMachinePool::new(
            runtime.clone() as Arc<dyn RuntimeSpawner>,
            8,
        ));
        let firer = Arc::new(RecordingIdleFirer::default());
        let handler = InProcessTeammateHandler::new(pool, output, api)
            .with_teammate_idle_firer(firer.clone() as Arc<dyn hooks::TeammateIdleFirer>);
        (dir, fs, runtime, handler, firer)
    }

    /// Yield until the firer has recorded at least `n` fires, or the budget runs
    /// out. Returns the recorded fires.
    #[allow(clippy::similar_names)] // `fires` is the plural noun form of `firer`'s fires method
    async fn await_fires(
        firer: &Arc<RecordingIdleFirer>,
        n: usize,
    ) -> Vec<hooks::TeammateIdleFire> {
        for _ in 0..400 {
            let fires = firer.fires();
            if fires.len() >= n {
                return fires;
            }
            tokio::task::yield_now().await;
        }
        firer.fires()
    }

    /// A wired `TeammateIdleFirer` receives one fire per completed turn-set —
    /// the "about to go idle" moment — carrying the teammate name (and `""`
    /// team_name, the leaf-scope gap). Driving a second turn-set via an injected
    /// message proves it fires again each time the teammate parks.
    #[tokio::test]
    #[allow(clippy::similar_names)] // `fires` (results) vs `firer` (sender) are semantically distinct
    async fn completed_turn_set_fires_teammate_idle_hook() {
        let api = ScriptedApiClient::new(vec!["answer one", "answer two"]);
        let (dir, fs, rt, handler, firer) = make_handler_with_idle_firer(api);
        let c = ctx(fs.clone(), rt.clone());

        let h = handler
            .spawn(
                TaskSpawnInput::InProcessTeammate {
                    agent_id: protocol::AgentId::new(),
                    name: "buddy".into(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        let spool = dir.path().join(format!("{}.txt", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();

        // Turn-set 1 completes → exactly one idle fire so far.
        await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
        let fires = await_fires(&firer, 1).await;
        assert_eq!(fires.len(), 1, "one idle fire after turn-set 1: {fires:?}");
        assert_eq!(fires[0].teammate_name, "buddy", "carries the teammate name");
        assert_eq!(
            fires[0].team_name, "",
            "team_name is empty at the leaf scope (no team identity reachable)"
        );

        // Inject a message → drives turn-set 2, which parks again → a 2nd fire.
        handler
            .send_message(&h.task_id, "follow-up".into(), c.clone())
            .await
            .unwrap();
        await_spool(&fs, &spool_str, |b| b.contains("answer two")).await;
        let fires = await_fires(&firer, 2).await;
        assert_eq!(fires.len(), 2, "a fire per completed turn-set: {fires:?}");

        handler.kill(&h.task_id, c).await.unwrap();
    }

    /// With NO firer wired (the default), a completed turn-set is a strict no-op
    /// on the hook path: the teammate still runs and parks normally (the spool
    /// shows the turn-set), proving the fire is purely additive and absent.
    #[tokio::test]
    async fn no_idle_firer_is_a_noop() {
        let api = ScriptedApiClient::new(vec!["answer one"]);
        // make_handler builds the handler WITHOUT a teammate idle firer.
        let (dir, fs, rt, handler) = make_handler(api);
        let c = ctx(fs.clone(), rt.clone());

        let h = handler
            .spawn(
                TaskSpawnInput::InProcessTeammate {
                    agent_id: protocol::AgentId::new(),
                    name: "buddy".into(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        // The turn-set completes and parks exactly as before — no firer, no
        // panic, no behavioral change.
        let spool = dir.path().join(format!("{}.txt", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();
        let body = await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
        assert!(body.contains("completed:"), "turn-set still completes: {body:?}");

        handler.kill(&h.task_id, c).await.unwrap();
    }

    /// `TeammateIdleFire`'s payload maps 1:1 to `HookEvent::TeammateIdle`'s
    /// wire fields — a guard that the seam's struct stays aligned with the event.
    #[test]
    fn idle_fire_payload_shape() {
        let fire = hooks::TeammateIdleFire {
            teammate_name: "buddy".into(),
            team_name: "alpha".into(),
        };
        assert_eq!(fire.teammate_name, "buddy");
        assert_eq!(fire.team_name, "alpha");
    }
}
