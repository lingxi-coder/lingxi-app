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
use agent::PermissionMode;
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
    pub fn skill_loader_handle(
        &self,
    ) -> Arc<OnceLock<Arc<dyn traits::skill_loader::SkillLoader>>> {
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
    pub fn with_teammate_idle_firer(
        mut self,
        firer: Arc<dyn hooks::TeammateIdleFirer>,
    ) -> Self {
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
    ) -> SubagentContext {
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
                )
                .await
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
        SubagentContext {
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
            rendered_system_prompt: None,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon,
            },
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
        let definition = self
            .definitions
            .resolve(&agent_id, &name)
            .ok_or_else(|| TaskError::Internal(format!("no agent definition for teammate {name}")))?;
        let subagent_ctx = self
            .build_context(agent_id, &name, &team_name, &description, definition)
            .await;

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
            stop_details: None,
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

    #[tokio::test]
    async fn build_context_resolves_inherit_to_default_model() {
        // DefaultTeammateDefinition yields AgentModel::Inherit; with a default
        // model wired the teammate ctx carries a concrete wire id (folded via
        // the same `resolve_agent_model` seam as the spawner).
        let handler = model_test_handler(Some("claude-opus-4-7"));
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler
            .build_context(protocol::AgentId::new(), "lead", "alpha", "go research", def)
            .await;
        assert!(matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-opus-4-7"));
        // Swarm identity threaded onto the context (claude-code
        // `TeammateContext.agentName` / `.teamName`).
        assert_eq!(ctx.agent_name.as_deref(), Some("lead"));
        assert_eq!(ctx.team_name.as_deref(), Some("alpha"));
        // The TeamCreate description becomes the teammate's first user message.
        assert_eq!(ctx.prompt_messages.len(), 1);
        assert_eq!(ctx.prompt_messages[0].text_content(), "go research");
    }

    // Full teammate parity (P1): the handler inherits a budget enforcer, seeds
    // the TeamCreate description as the first user message, and runs the shared
    // tool resolver when a registry is wired (advertising a real pool instead of
    // the prior chat-only empty set).
    #[tokio::test]
    async fn build_context_wires_budget_description_and_tool_resolution() {
        struct DummyBudget;
        #[async_trait]
        impl traits::budget::BudgetEnforcerHandle for DummyBudget {
            async fn check_and_charge(&self, _: u64) -> Result<(), traits::budget::BudgetError> {
                Ok(())
            }
            async fn snapshot_total_nano_usd(&self) -> u64 {
                0
            }
        }
        let handler = model_test_handler(Some("claude-opus-4-7"))
            .with_budget_enforcer(Arc::new(DummyBudget))
            // An EMPTY registry still exercises the resolution path (returns an
            // empty pool); a populated registry is covered by the agent crate's
            // `resolve_subagent_tools` tests (shared code path).
            .with_tool_registry(Arc::new(agent::ToolRegistry::new()));
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler
            .build_context(protocol::AgentId::new(), "lead", "alpha", "do the task", def)
            .await;
        // Budget inherited (was None before this fix).
        assert!(ctx.budget.is_some(), "teammate must inherit the budget enforcer");
        // Description seeded as the first user message (was empty before).
        assert_eq!(ctx.prompt_messages.len(), 1);
        assert_eq!(ctx.prompt_messages[0].text_content(), "do the task");
        // The resolver ran (empty registry ⇒ empty pool, but the path is wired —
        // no panic, and the allow-list mirrors the advertised set).
        assert_eq!(ctx.tool_schemas.len(), ctx.allowed_tools.len());
    }

    #[tokio::test]
    async fn build_context_without_default_model_leaves_model_raw() {
        // No default model wired → legacy behavior: Inherit is left untouched.
        let handler = model_test_handler(None);
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler
            .build_context(protocol::AgentId::new(), "lead", "", "", def)
            .await;
        assert!(matches!(&ctx.agent_definition.model, AgentModel::Inherit));
        // Empty team_name spawns standalone → team_name is None (leader default).
        assert_eq!(ctx.agent_name.as_deref(), Some("lead"));
        assert_eq!(ctx.team_name, None);
        // Empty description ⇒ no seed message (parks awaiting first injection).
        assert!(ctx.prompt_messages.is_empty());
    }

    // ---- #15: opusplan + plan mode resolves an Inherit teammate to Opus -------
    //
    // The composition root threads `with_permission_mode(cfg.permission_mode)` +
    // `with_model_setting(cfg.default_model)` (the RAW user alias, e.g.
    // "opusplan") onto the handler alongside
    // `with_default_model(resolve_user_specified_model(orch_cfg.model))` — the
    // RESOLVED main-loop id (Sonnet for an opusplan install). To stay FAITHFUL to
    // production these tests derive the parent the SAME way: feed
    // `resolve_user_specified_model("opusplan")` (= "claude-sonnet-4-6") as
    // `with_default_model`, not a hand-picked literal the wired path never emits.
    // These three together drive `resolve_agent_model`'s `getRuntimeMainLoopModel`
    // branch (model.ts:145-167), proving the wired path end-to-end: an
    // `AgentModel::Inherit` teammate on an `opusplan` install IN PLAN MODE resolves
    // to Opus (without `[1m]`), NOT the resolved Sonnet main-loop model — i.e. the
    // plan-mode swap fires through the builders the composition root populates.

    #[tokio::test]
    async fn build_context_opusplan_plan_mode_resolves_inherit_to_opus() {
        // Pin firstParty so `getDefaultOpusModel()` is deterministic regardless of
        // any provider env this process inherits.
        let _g = OpusEnvGuard::clear_providers();
        // Derive the parent EXACTLY as the composition root does: resolve the raw
        // "opusplan" alias to the main-loop wire id (Sonnet outside plan mode) —
        // proving the swap below is to OPUS, not a pass-through of a literal.
        let parent = agent::model_resolution::resolve_user_specified_model("opusplan");
        let handler = model_test_handler(Some(&parent))
            .with_permission_mode(PermissionMode::Plan)
            .with_model_setting("opusplan");
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler
            .build_context(protocol::AgentId::new(), "lead", "", "", def)
            .await;
        assert!(
            matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-opus-4-7"),
            "opusplan + plan mode must resolve an Inherit teammate to Opus, got {:?}",
            ctx.agent_definition.model
        );
    }

    #[tokio::test]
    async fn build_context_opusplan_default_mode_returns_resolved_parent() {
        // Same opusplan setting but NOT in plan mode → the Inherit branch returns
        // the resolved main-loop model unchanged (Sonnet), proving the swap is
        // gated on plan mode (not on the setting alone). Parent derived via the
        // resolver, exactly as the composition root produces it.
        let _g = OpusEnvGuard::clear_providers();
        let parent = agent::model_resolution::resolve_user_specified_model("opusplan");
        assert_eq!(parent, "claude-sonnet-4-6", "opusplan resolves to Sonnet outside plan mode");
        let handler = model_test_handler(Some(&parent))
            .with_permission_mode(PermissionMode::Default)
            .with_model_setting("opusplan");
        let def = DefaultTeammateDefinition
            .resolve(&protocol::AgentId::new(), "lead")
            .unwrap();
        let ctx = handler
            .build_context(protocol::AgentId::new(), "lead", "", "", def)
            .await;
        assert!(
            matches!(&ctx.agent_definition.model, AgentModel::Explicit(m) if m == "claude-sonnet-4-6"),
            "opusplan outside plan mode must keep the resolved parent (Sonnet), got {:?}",
            ctx.agent_definition.model
        );
    }

    /// RAII guard that clears the three provider env vars (Bedrock/Vertex/Foundry)
    /// for the duration of a test so `getDefaultOpusModel()` resolves on the
    /// firstParty branch deterministically (mirrors `model_resolution`'s test
    /// guard). Restores prior values on drop.
    struct OpusEnvGuard {
        prev: Vec<(&'static str, Option<String>)>,
    }
    impl OpusEnvGuard {
        fn clear_providers() -> Self {
            let keys = [
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ];
            let prev = keys
                .iter()
                .map(|k| {
                    let v = std::env::var(k).ok();
                    std::env::remove_var(k);
                    (*k, v)
                })
                .collect();
            Self { prev }
        }
    }
    impl Drop for OpusEnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.prev {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
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
                    team_name: "alpha".into(),
                    description: String::new(),
                },
                c.clone(),
            )
            .await
            .unwrap();
        assert!(h.task_id.starts_with('t'), "teammate ids prefix 't'");
        assert!(h.cleanup.is_some(), "cleanup hook present");
        assert_eq!(handler.entries.lock().await.len(), 1, "spawn registers slot");

        let spool = dir.path().join(format!("{}.output", h.task_id));
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
                    team_name: "alpha".into(),
                    description: String::new(),
                },
                c,
            )
            .await
            .unwrap();

        let spool = dir.path().join(format!("{}.output", h.task_id));
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
                    team_name: "alpha".into(),
                    description: String::new(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        // Let turn-set 1 land so the runner is parked and reachable.
        let spool = dir.path().join(format!("{}.output", h.task_id));
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
    /// the "about to go idle" moment — carrying the teammate name and the team
    /// name threaded from the spawn input. Driving a second turn-set via an
    /// injected message proves it fires again each time the teammate parks.
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
                    team_name: "alpha".into(),
                    description: String::new(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        let spool = dir.path().join(format!("{}.output", h.task_id));
        let spool_str = spool.to_str().unwrap().to_string();

        // Turn-set 1 completes → exactly one idle fire so far.
        await_spool(&fs, &spool_str, |b| b.contains("answer one")).await;
        let fires = await_fires(&firer, 1).await;
        assert_eq!(fires.len(), 1, "one idle fire after turn-set 1: {fires:?}");
        assert_eq!(fires[0].teammate_name, "buddy", "carries the teammate name");
        assert_eq!(
            fires[0].team_name, "alpha",
            "team_name is threaded from the spawn input (claude-code getTeamName())"
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
                    team_name: "alpha".into(),
                    description: String::new(),
                },
                c.clone(),
            )
            .await
            .unwrap();

        // The turn-set completes and parks exactly as before — no firer, no
        // panic, no behavioral change.
        let spool = dir.path().join(format!("{}.output", h.task_id));
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
