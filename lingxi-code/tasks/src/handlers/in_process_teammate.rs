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
use std::sync::{Arc, OnceLock};

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
// `PermissionMode` is re-exported from the `agent` crate (which depends on
// `permission`) so `tasks` can reference it without a new `permission` dep.
use agent::resolve_agent_model;
use agent::runner::SubagentEvent;
use agent::PermissionMode;
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
            // Synthesized stub: no extended frontmatter — all defaults.
            disallowed_tools: vec![],
            skills: vec![],
            required_mcp_servers: vec![],
            background: false,
            isolation: None,
            memory: None,
            effort: None,
            initial_prompt: None,
            color: None,
            observer: None,
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
    /// Live/boot permission-mode anchor threaded into
    /// [`agent::resolve_agent_model`] (so an `AgentModel::Inherit` teammate gets
    /// the plan-mode runtime resolution `opusplan`→Opus / `haiku`→Sonnet). Default
    /// `PermissionMode::Default` keeps the Inherit branch returning the parent
    /// model unchanged (mirrors `PoolSubagentSpawner::permission_mode`).
    permission_mode: PermissionMode,
    /// RAW user model setting string (mirrors `getUserSpecifiedModelSetting()`,
    /// e.g. `"opusplan"` / `"haiku"`). Used ONLY for the opusplan/haiku plan-mode
    /// runtime resolution; without it (the default) the Inherit branch returns the
    /// parent model unchanged (mirrors `PoolSubagentSpawner::model_setting`).
    model_setting: Option<String>,
    /// Live tool registry used to resolve the teammate's advertised tool
    /// SCHEMAS + dispatch allow-list per spawn (claude-code `assembleToolPool`),
    /// mirroring [`agent::PoolSubagentSpawner`]. A SET-ONCE cell (same
    /// construction cycle-break: the registry is built AFTER the handler is
    /// boxed, so the composition root fills it via
    /// [`Self::tool_registry_handle`]). Unfilled (the default / tests) ⇒ no tools
    /// advertised (chat-only — byte-identical to before this seam).
    tool_registry: Arc<OnceLock<Arc<agent::ToolRegistry>>>,
    /// Tool-wide deny-rule names from the boot permission policy, applied in the
    /// per-spawn tool resolution so a blanket-denied tool never leaks into the
    /// teammate's advertised pool (claude-code `filterToolsByDenyRules`).
    /// SET-ONCE; unfilled ⇒ no filtering.
    tool_wide_deny_names: Arc<OnceLock<Vec<String>>>,
    /// Budget enforcer inherited by the teammate so its turns charge the shared
    /// cumulative cost (claude-code teammates share the session budget). `None`
    /// (the default / tests) ⇒ no per-turn budget gate.
    budget_enforcer: Option<Arc<dyn traits::budget::BudgetEnforcerHandle>>,
    /// Hook executor handed to the teammate's runner so it fires `SubagentStart`
    /// (+ frontmatter hooks) like a normal subagent. SET-ONCE cell (same
    /// cycle-break as [`Self::tool_registry`]); unfilled ⇒ the runner skips the
    /// SubagentStart fire (byte-identical legacy).
    hook_executor: Arc<OnceLock<Arc<hooks::HookExecutorImpl>>>,
    /// Skill loader handed to the teammate's runner so it preloads the
    /// definition's frontmatter `skills:`. SET-ONCE; unfilled ⇒ no preloading.
    skill_loader: Arc<OnceLock<Arc<dyn traits::skill_loader::SkillLoader>>>,
    /// Session id + cwd stamped on the `HookContext` the runner builds for the
    /// SubagentStart fire (only consulted when [`Self::hook_executor`] is filled).
    hook_session_id: protocol::SessionId,
    hook_cwd: std::path::PathBuf,
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
            permission_mode: PermissionMode::Default,
            model_setting: None,
            tool_registry: Arc::new(OnceLock::new()),
            tool_wide_deny_names: Arc::new(OnceLock::new()),
            budget_enforcer: None,
            hook_executor: Arc::new(OnceLock::new()),
            skill_loader: Arc::new(OnceLock::new()),
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
            status_sink: Arc::new(NoopStatusSink),
            teammate_idle_firer: None,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return a clone of the set-once tool-registry cell so the composition root
    /// can fill it AFTER the registry is built (same cycle-break as
    /// [`agent::PoolSubagentSpawner::tool_registry_handle`]). Enables per-spawn
    /// tool resolution. First fill wins; unfilled ⇒ chat-only teammate.
    #[must_use]
    pub fn tool_registry_handle(&self) -> Arc<OnceLock<Arc<agent::ToolRegistry>>> {
        self.tool_registry.clone()
    }

    /// Builder: set the tool registry immediately (tests).
    #[must_use]
    pub fn with_tool_registry(self, registry: Arc<agent::ToolRegistry>) -> Self {
        let _ = self.tool_registry.set(registry);
        self
    }

    /// Return a clone of the set-once tool-wide-deny-names cell (filled from the
    /// boot permission policy, like the spawner).
    #[must_use]
    pub fn tool_wide_deny_names_handle(&self) -> Arc<OnceLock<Vec<String>>> {
        self.tool_wide_deny_names.clone()
    }

    /// Builder: inherit a budget enforcer so the teammate's turns charge the
    /// shared cumulative cost.
    #[must_use]
    pub fn with_budget_enforcer(
        mut self,
        enforcer: Arc<dyn traits::budget::BudgetEnforcerHandle>,
    ) -> Self {
        self.budget_enforcer = Some(enforcer);
        self
    }

    /// Return a clone of the set-once hook-executor cell so the composition root
    /// can fill it after the `HookExecutorImpl` exists (enables SubagentStart).
    #[must_use]
    pub fn hook_executor_handle(&self) -> Arc<OnceLock<Arc<hooks::HookExecutorImpl>>> {
        self.hook_executor.clone()
    }

    /// Builder: set the hook executor immediately (the executor already exists
    /// when the teammate handler is constructed at the composition root, unlike
    /// the spawner's deferred cell). Enables the teammate runner's SubagentStart.
    #[must_use]
    pub fn with_hook_executor(self, executor: Arc<hooks::HookExecutorImpl>) -> Self {
        let _ = self.hook_executor.set(executor);
        self
    }

    /// Return a clone of the set-once skill-loader cell.
    #[must_use]
    pub fn skill_loader_handle(&self) -> Arc<OnceLock<Arc<dyn traits::skill_loader::SkillLoader>>> {
        self.skill_loader.clone()
    }

    /// Set the session id + cwd stamped on the teammate runner's SubagentStart
    /// `HookContext` (only consulted when the hook executor is filled).
    #[must_use]
    pub fn with_hook_context(
        mut self,
        session_id: protocol::SessionId,
        cwd: std::path::PathBuf,
    ) -> Self {
        self.hook_session_id = session_id;
        self.hook_cwd = cwd;
        self
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

    /// Set the live/boot permission-mode anchor threaded into
    /// [`agent::resolve_agent_model`] (so an `AgentModel::Inherit` teammate gets
    /// the plan-mode runtime resolution `opusplan`→Opus / `haiku`→Sonnet). Without
    /// it the default (`PermissionMode::Default`) keeps the Inherit branch
    /// returning the parent model unchanged.
    #[must_use]
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Set the RAW user model setting string (mirrors
    /// `getUserSpecifiedModelSetting()`, e.g. `"opusplan"` / `"haiku"`). Used ONLY
    /// for the opusplan/haiku plan-mode runtime resolution; without it the Inherit
    /// branch returns the parent model unchanged.
    #[must_use]
    pub fn with_model_setting(mut self, setting: impl Into<String>) -> Self {
        self.model_setting = Some(setting.into());
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
    pub fn with_teammate_idle_firer(mut self, firer: Arc<dyn hooks::TeammateIdleFirer>) -> Self {
        self.teammate_idle_firer = Some(firer);
        self
    }

    /// Build the persistent [`SubagentContext`] for a spawn.
    ///
    /// `name` is the teammate's DISPLAY name and `team_name` the coordinator
    /// team it belongs to (empty when spawned standalone). Both ride on the
    /// context as [`SubagentContext::agent_name`] / [`SubagentContext::team_name`]
    /// so the runner threads them into every dispatched tool's
    /// [`traits::tool_invoker::SubagentInvocationContext`] — the Rust analogue of
    /// claude-code running the teammate inside `runWithTeammateContext` so
    /// `getAgentName()` / `getTeammateContext()?.teamName` resolve inside its
    /// tool calls.
    async fn build_context(
        &self,
        agent_id: protocol::AgentId,
        name: &str,
        team_name: &str,
        description: &str,
        mut definition: AgentDefinition,
    ) -> Result<SubagentContext, TaskError> {
        // Resolve the model preference to a concrete wire id, mirroring the
        // `PoolSubagentSpawner` seam (`Inherit`→parent model, family alias→
        // concrete id), so a wired teammate runs against a live provider. Unset
        // `default_model` (tests / no boot wiring) leaves it RAW (legacy).
        if let Some(parent_model) = &self.default_model {
            definition.model = AgentModel::Explicit(resolve_agent_model(
                &definition.model,
                parent_model,
                self.permission_mode,
                self.model_setting.as_deref(),
            ));
        }
        // Advertise the teammate's tool pool (claude-code `assembleToolPool`) via
        // the SAME shared resolver `PoolSubagentSpawner` uses, keyed on the
        // resolved model. Unfilled registry ⇒ empty (chat-only, byte-identical to
        // before this seam). With tools advertised the teammate can actually emit
        // `tool_use`; the dispatch allow-list guards what the inherited invoker runs.
        let (tool_schemas, allowed_tools) = match self.tool_registry.get() {
            Some(registry) => {
                let empty: Vec<String> = Vec::new();
                let denied = self.tool_wide_deny_names.get().unwrap_or(&empty);
                agent::resolve_subagent_tools(
                    registry,
                    &definition,
                    denied,
                    self.default_model.as_deref(),
                    0,
                )
                .await
                .map_err(|e| TaskError::Internal(e.to_string()))?
            }
            None => (Vec::new(), Vec::new()),
        };
        // The TeamCreate description IS the teammate's initial task (claude-code
        // the team lead's purpose): seed it as the first user message so the
        // teammate has work to do, not just chat. Empty ⇒ no seed message (parks
        // awaiting the first injected message — prior behavior).
        let prompt_messages = if description.is_empty() {
            vec![]
        } else {
            vec![protocol::ConversationMessage::user(
                protocol::MessageId::new(),
                description.to_string(),
            )]
        };
        let icon = definition.icon.clone();
        Ok(SubagentContext {
            agent_id,
            parent_agent_id: None,
            // Swarm identity (claude-code `TeammateContext.agentName` /
            // `.teamName`): the DISPLAY name is always reachable here (it is the
            // spawn input); `team_name` is threaded from the coordinator team via
            // the spawn input. An empty value (standalone spawn / no team) becomes
            // `None` — the leader / main-thread default. The runner reads these
            // and threads them into every dispatched tool's
            // `SubagentInvocationContext`, so the merged swarm `TaskUpdate`
            // side-effects (auto-owner, owner-change mailbox notification) and
            // `getTaskListId()` actually fire for this teammate.
            agent_name: (!name.is_empty()).then(|| name.to_string()),
            team_name: (!team_name.is_empty()).then(|| team_name.to_string()),
            agent_definition: definition,
            prompt_messages,
            fork_context_messages: None,
            allowed_tools,
            worktree_handle: None,
            // Teammates run in the shared session (no per-agent worktree).
            cwd: None,
            is_async: false,
            // The defining trait of a teammate: park between turn-sets and
            // resume on the next injected UserMessage.
            persistent: true,
            can_show_permission_prompts: true,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            transcript_fs: None,
            resumed_history: None,
            rendered_system_prompt: None,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon,
            },
            model_profile: None,
            api_client: Some(self.api_client.clone()),
            tool_invoker: self.tool_invoker.clone(),
            // Advertised tool schemas (claude-code `assembleToolPool`) — resolved
            // above from the live registry per the definition's policy.
            tool_schemas,
            // Teammates have no structured-output schema.
            schema: None,
            // Inherit the shared budget enforcer when wired (claude-code teammates
            // charge the session's cumulative cost); `None` ⇒ no per-turn gate.
            budget: self.budget_enforcer.clone(),
            // Wire the SubagentStart-hook + skills-preload seam from the set-once
            // cells (filled at the composition root, same as `PoolSubagentSpawner`).
            // Unfilled ⇒ the runner skips them (byte-identical legacy).
            hook_executor: self.hook_executor.get().cloned(),
            skill_loader: self.skill_loader.get().cloned(),
            hook_session_id: self.hook_session_id,
            hook_cwd: self.hook_cwd.clone(),
            depth: 0,
            observer: None,
            permission_mode_override: None,
        })
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
        let TaskSpawnInput::InProcessTeammate {
            agent_id,
            name,
            team_name,
            description,
        } = input
        else {
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
        let definition = self.definitions.resolve(&agent_id, &name).ok_or_else(|| {
            TaskError::Internal(format!("no agent definition for teammate {name}"))
        })?;
        let subagent_ctx = self
            .build_context(agent_id, &name, &team_name, &description, definition)
            .await?;

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
        let output_manager = self.output.clone();
        let worker_spool_path = spool_path.clone();
        let status_sink = self.status_sink.clone();
        // Best-effort `TeammateIdle` firer + the teammate name / team name it
        // carries. The team_name is now threaded from the coordinator through the
        // spawn input (claude-code `getTeamName()`); an empty value (standalone
        // spawn / no team) rides as `""`, matching claude-code's
        // `getTeamName() ?? ''` fallback.
        let idle_firer = self.teammate_idle_firer.clone();
        let idle_name = name.clone();
        let idle_team_name = team_name.clone();
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
                // Routed through the output manager's `append` so the per-file
                // 5GB disk cap is enforced (T17) and the write uses O_NOFOLLOW
                // (claude-code `diskOutput.ts`, T18).
                if let Err(e) = output_manager
                    .append(&worker_spool_path, &format!("{line}\n"))
                    .await
                {
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
                                team_name: idle_team_name.clone(),
                            })
                            .await;
                    }
                }
                if let Some(status) = terminal_status(&ev) {
                    // Failed / Killed end the teammate; a per-turn-set Completed
                    // does not (terminal_status returns None for it), so the
                    // worker keeps pumping subsequent turn-sets.
                    //
                    // A Failed carries its error through `set_failed` so the
                    // lead-facing sink can surface the REASON, not a sentinel —
                    // claude-code 2.1.198's failed idle notification to the
                    // leader (`{idleReason:"failed", completedStatus:"failed",
                    // failureReason}`, binary @216293689).
                    if let SubagentEvent::Failed { error, .. } = &ev {
                        status_sink.set_failed(&worker_task_id, error).await;
                    } else {
                        status_sink.set_status(&worker_task_id, status).await;
                    }
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

        // 7. Cleanup seam: synchronous, so it cannot await, but it must still
        //    release the pool slot. Remove the live entry, stop the streaming
        //    worker, and best-effort schedule the async deallocate on the current
        //    runtime. `Task::kill` remains the authoritative explicit stop path;
        //    cleanup covers parent/session teardown where the registry only has
        //    the returned `TaskHandle`.
        let cleanup_stop = stop;
        let cleanup_pool = self.pool.clone();
        let cleanup_entries = self.entries.clone();
        let cleanup_task_id = task_id.clone();
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            cleanup_stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let Some(agent_id) = cleanup_entries.try_lock().ok().and_then(|mut entries| {
                entries.remove(&cleanup_task_id).map(|entry| entry.agent_id)
            }) else {
                return;
            };
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let pool = cleanup_pool.clone();
                handle.spawn(async move {
                    let _ = pool.deallocate(&agent_id).await;
                });
            }
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

        self.status_sink
            .set_status(task_id, TaskStatus::Killed)
            .await;
        Ok(())
    }
}

#[cfg(test)]
#[path = "in_process_teammate_test.rs"]
mod in_process_teammate_test;
