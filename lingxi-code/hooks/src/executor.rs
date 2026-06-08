//! Hook executor.
//!
//! M1.4 shipped [`BuiltinHookHandler`] and the [`HookExecutorImpl`] with the
//! `Builtin` arm fully wired and the `Http` / `Command` / `Agent` arms
//! stubbed. M5-06 filled in the HTTP and Agent arms (delegating to
//! [`crate::http_executor::HttpExecutor`] / [`crate::agent_executor::AgentExecutor`]).
//! The Command arm is now wired too: it feeds the serialized hook payload to a
//! child process on stdin via the [`ProcessRunner`] + [`Sandbox`] traits and
//! maps the process exit code / stdout to a [`HookResult`] per the claude-code
//! command-hook contract (`claude-code/src/utils/hooks.ts`).

use crate::agent_executor::{AgentExecutionSignal, AgentExecutor};
use crate::async_registry::{AsyncHookRegistry, HookWork};
use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use crate::hook_payload::{
    parse_response, ConfigChangePayload, CwdChangedPayload, ElicitationPayload, FileChangedPayload,
    HookEventNameConfigChange, HookEventNameCwdChanged, HookEventNameElicitation,
    HookEventNameFileChanged, HookEventNameInstructionsLoaded, HookEventNameNotification,
    HookEventNamePermissionDenied, HookEventNamePermissionRequest, HookEventNamePost,
    HookEventNamePostCompact,
    HookEventNamePostToolUseFailure, HookEventNamePre, HookEventNamePreCompact,
    HookEventNameSessionEnd, HookEventNameSessionStart, HookEventNameSetup, HookEventNameStop,
    HookEventNameStopFailure, HookEventNameSubagentStart, HookEventNameSubagentStop,
    HookEventNameTaskCompleted, HookEventNameTaskCreated, HookEventNameTeammateIdle,
    HookEventNameUserPromptSubmit,
    HookEventNameWorktreeCreate,
    HookEventNameWorktreeRemove, InstructionsLoadedPayload, NotificationPayload,
    PermissionDeniedPayload, PermissionRequestPayload, PostCompactPayload,
    PostToolUseFailurePayload, PostToolUsePayload,
    PreCompactPayload, PreToolUsePayload, SessionEndPayload, SessionStartPayload, SetupPayload,
    StopFailurePayload, StopPayload, SubagentStartPayload, SubagentStopPayload,
    TaskCompletedPayload, TaskCreatedPayload, TeammateIdlePayload, UserPromptSubmitPayload,
    WorktreeCreatePayload,
    WorktreeRemovePayload,
};
use crate::http_executor::{HttpExecutionSignal, HttpExecutor};
use crate::prompt_executor::{
    HookPromptRunner, PromptExecutionSignal, PromptExecutor, HOOK_PROMPT_TIMEOUT_MS,
};
use crate::registry::{HookContext, HookRegistry};
use crate::response::{AggregateHookResult, HookDecision, HookOutcome, HookResponse, HookResult};
use crate::ssrf_guard::SsrfGuard;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use traits::subagent_spawn::SubagentSpawner;
use traits::{
    HttpTransport, ProcessCommand, ProcessError, ProcessRunner, RuntimeSpawner, Sandbox,
};

/// Default HTTP hook timeout (10 minutes — matches
/// `claude-code/src/utils/hooks/execHttpHook.ts:12` `DEFAULT_HTTP_HOOK_TIMEOUT_MS`).
pub const HOOK_HTTP_TIMEOUT_MS: u64 = 600_000;

/// Default command hook timeout (10 minutes — matches
/// `claude-code/src/utils/hooks.ts:166` `TOOL_HOOK_EXECUTION_TIMEOUT_MS`).
pub const HOOK_COMMAND_TIMEOUT_MS: u64 = 600_000;

/// Default agent hook timeout (60 seconds — matches
/// `claude-code/src/utils/hooks/execAgentHook.ts:75` fall-through default).
pub const HOOK_AGENT_TIMEOUT_MS: u64 = 60_000;

/// In-process Rust handler for [`HookExecutor::Builtin`] hooks.
///
/// Implementations are registered with [`HookExecutorImpl::register_builtin`]
/// and looked up by `id` when an event fires.
#[async_trait]
pub trait BuiltinHookHandler: Send + Sync {
    /// Handle the event and produce a [`HookResult`]. Implementations should
    /// avoid blocking work — long operations should be deferred to the
    /// background async registry.
    async fn handle(&self, event: &HookEvent, ctx: &HookContext) -> HookResult;
    /// Stable handler identifier matching the `handler_id` carried by
    /// [`HookExecutor::Builtin`] definitions.
    fn id(&self) -> &str;
}

/// Default executor — dispatches each matched hook to the appropriate
/// runner kind and aggregates their responses.
///
/// The `http` transport is shared with the rest of the engine so requests
/// flow through the same retry / telemetry plumbing.
pub struct HookExecutorImpl {
    registry: Arc<RwLock<HookRegistry>>,
    http: Arc<dyn HttpTransport>,
    #[allow(dead_code)] // Command arm uses ProcessRunner not RuntimeSpawner.
    runtime: Arc<dyn RuntimeSpawner>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    ssrf_guard: SsrfGuard,
    /// Optional subagent spawner — attached via [`Self::with_agent_spawner`].
    /// When `None`, the `Agent` arm returns `HookOutcome::Error` with
    /// `stderr: "Hook {id} failed: agent executor not wired"`. M5-06.
    agent_spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Optional single-turn LLM runner — attached via
    /// [`Self::with_prompt_runner`]. When `None`, the `Prompt` arm returns a
    /// structured "not wired" no-op (never blocks). The orchestrator wires this
    /// over its existing one-shot `messages_create` call to keep the hooks
    /// crate decoupled from the api-client.
    prompt_runner: Option<Arc<dyn HookPromptRunner>>,
    /// Optional process runner — attached via [`Self::with_process_runner`].
    /// When `None`, the `Command` arm returns `HookOutcome::Error` with
    /// `stderr: "Hook {id} failed: command executor not wired"`.
    process: Option<Arc<dyn ProcessRunner>>,
    /// Optional sandbox — required alongside `process` to mint the
    /// [`traits::SandboxedCommand`] the runner accepts. Attached via
    /// [`Self::with_process_runner`].
    sandbox: Option<Arc<dyn Sandbox>>,
    /// Optional background registry for non-blocking (`blocking == false`)
    /// hooks (B5). Attached via [`Self::with_async_registry`]. When `None`, a
    /// non-blocking hook falls back to running synchronously (so its result is
    /// never silently dropped) — the engine simply gains no backgrounding.
    async_registry: Option<Arc<AsyncHookRegistry>>,
}

impl HookExecutorImpl {
    /// Build a new executor backed by the supplied registry, HTTP transport,
    /// and runtime spawner.
    ///
    /// The Agent-arm is uncwired by default. Call [`Self::with_agent_spawner`]
    /// post-construction to attach a `SubagentSpawner` for Agent hooks.
    #[must_use]
    pub fn new(
        registry: Arc<RwLock<HookRegistry>>,
        http: Arc<dyn HttpTransport>,
        runtime: Arc<dyn RuntimeSpawner>,
    ) -> Self {
        Self {
            registry,
            http,
            runtime,
            builtin_handlers: HashMap::new(),
            ssrf_guard: SsrfGuard::with_defaults(),
            agent_spawner: None,
            prompt_runner: None,
            process: None,
            sandbox: None,
            async_registry: None,
        }
    }

    /// Attach an [`AsyncHookRegistry`] so non-blocking (`blocking == false`)
    /// hooks are backgrounded instead of awaited (B5). Without this, a
    /// non-blocking hook still runs synchronously (its result is not dropped),
    /// but the engine gains no backgrounding for it.
    #[must_use]
    pub fn with_async_registry(mut self, registry: Arc<AsyncHookRegistry>) -> Self {
        self.async_registry = Some(registry);
        self
    }

    /// Attach a [`SubagentSpawner`] so the `Agent` arm can fork subagents.
    /// Without this, `HookExecutor::Agent` hooks return a structured
    /// "not wired" error. M5-06.
    #[must_use]
    pub fn with_agent_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.agent_spawner = Some(spawner);
        self
    }

    /// Attach a [`HookPromptRunner`] so the `Prompt` arm can evaluate inline
    /// single-turn LLM queries (`execPromptHook.ts`). Without this,
    /// [`HookExecutor::Prompt`] hooks return a structured "not wired" no-op and
    /// never block. The orchestrator implements the runner over its existing
    /// one-shot `messages_create` call, keeping the hooks crate independent of
    /// the api-client.
    #[must_use]
    pub fn with_prompt_runner(mut self, runner: Arc<dyn HookPromptRunner>) -> Self {
        self.prompt_runner = Some(runner);
        self
    }

    /// Attach a [`ProcessRunner`] + [`Sandbox`] so the `Command` arm can spawn
    /// child processes. Both are required: the runner only accepts a
    /// [`traits::SandboxedCommand`], which only the sandbox can mint (the
    /// D2 / spec A1 sandbox-decision invariant). Without this, `Command`
    /// hooks return a structured "not wired" error.
    #[must_use]
    pub fn with_process_runner(
        mut self,
        process: Arc<dyn ProcessRunner>,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        self.process = Some(process);
        self.sandbox = Some(sandbox);
        self
    }

    /// Register a builtin handler. Subsequent hook definitions referencing
    /// `h.id()` via [`HookExecutor::Builtin`] will dispatch to this handler.
    pub fn register_builtin(&mut self, h: Arc<dyn BuiltinHookHandler>) {
        self.builtin_handlers.insert(h.id().into(), h);
    }

    /// Borrow the shared HTTP transport.
    #[allow(dead_code)]
    pub(crate) fn http(&self) -> &Arc<dyn HttpTransport> {
        &self.http
    }

    /// Snapshot the cheaply-cloneable dispatcher backing a single hook
    /// invocation. Every field is an `Arc` / `Clone` so the snapshot can be
    /// moved into a `'static` background future (B5) or borrowed inline for the
    /// synchronous path — both go through the same [`Dispatcher::dispatch`].
    fn dispatcher(&self) -> Dispatcher {
        Dispatcher {
            http: self.http.clone(),
            ssrf_guard: self.ssrf_guard.clone(),
            builtin_handlers: self.builtin_handlers.clone(),
            agent_spawner: self.agent_spawner.clone(),
            prompt_runner: self.prompt_runner.clone(),
            process: self.process.clone(),
            sandbox: self.sandbox.clone(),
        }
    }

    /// Whether any registered hook subscribes to `event_type` (ignoring
    /// per-hook matchers). Cheap gate read over the live registry — see
    /// [`HookRegistry::has_hooks_for`]. Used by best-effort lifecycle fire
    /// paths (e.g. the CLI repl idle-prompt timer) to avoid arming work when
    /// no subscriber exists.
    pub async fn has_hooks_for(&self, event_type: &crate::events::HookEventType) -> bool {
        self.registry.read().await.has_hooks_for(event_type)
    }

    /// Fire `event` and return the aggregated result of every matching hook.
    ///
    /// Hooks are evaluated in priority-descending order; processing stops
    /// early on the first `Block` decision.
    ///
    /// B5 — a matched hook with `blocking == false` is routed to the
    /// [`AsyncHookRegistry`] (when wired) instead of being awaited: it is
    /// backgrounded, EXCLUDED from the aggregate, and so can NEVER contribute a
    /// `Block` decision. A `blocking == true` hook still runs synchronously,
    /// exactly as before — the regression-guarded common case. (When no async
    /// registry is wired, a non-blocking hook degrades to running synchronously
    /// so its result is not silently dropped, but it is STILL excluded from the
    /// aggregate to preserve the "non-blocking can't block" contract.)
    pub async fn execute(&self, event: HookEvent, ctx: HookContext) -> AggregateHookResult {
        let reg = self.registry.read().await;
        let matched: Vec<HookDefinition> =
            reg.match_event(&event, &ctx).into_iter().cloned().collect();
        drop(reg);
        let mut agg = AggregateHookResult::default();
        let hook_event = format!("{:?}", event.event_type());
        for hook in &matched {
            // Emit a `hook_progress` event for every matching hook *before* it
            // runs, carrying the per-hook `status_message` (claude-code
            // `utils/hooks.ts:2094-2116`). The spinner substitutes this text
            // for the generic running line when present.
            agg.progress.push(crate::events::HookProgressEvent {
                hook_event: hook_event.clone(),
                hook_name: hook.name.clone(),
                status_message: hook.status_message.clone(),
            });
            if hook.blocking {
                // Synchronous path — unchanged from M5-06.
                let result = self.dispatcher().dispatch(hook, &event, &ctx).await;
                // `once` runtime removal (claude-code `registerSkillHooks.ts:35-36`,
                // `utils/hooks.ts:2918-2919`): drop the hook from the registry
                // only after it runs with a *success* outcome, so it never fires
                // again. An erroring `once` hook is left in place.
                if hook.once && matches!(result.outcome, HookOutcome::Success) {
                    self.registry.write().await.remove_once_hook(hook.id);
                }
                Self::merge(&mut agg, hook, result);
                if matches!(agg.decision, Some(crate::response::HookDecision::Block)) {
                    break;
                }
            } else {
                // B5 config-`async` path: background the hook and continue. It
                // is excluded from `agg`, so it cannot block.
                self.background_hook(hook, &event, &ctx).await;
            }
        }
        agg
    }

    /// Route a `blocking == false` hook to the background async registry (B5).
    ///
    /// Mirrors claude-code `executeInBackground` (`utils/hooks.ts:995-1030`):
    /// the engine proceeds immediately and the hook's eventual result folds
    /// back through the registry's completion channel. When no registry is
    /// wired the hook degrades to a synchronous run whose result is discarded
    /// from the aggregate (it still cannot block) — this keeps a misconfigured
    /// engine from silently no-op'ing the hook entirely.
    async fn background_hook(&self, hook: &HookDefinition, event: &HookEvent, ctx: &HookContext) {
        let Some(registry) = &self.async_registry else {
            // No registry wired: run inline but discard from the aggregate so
            // the "non-blocking can't block" contract still holds.
            let _ = self.dispatcher().dispatch(hook, event, ctx).await;
            return;
        };
        let dispatcher = self.dispatcher();
        let hook_owned = hook.clone();
        let event_owned = event.clone();
        let ctx_owned = ctx.clone();
        let work: HookWork = Box::pin(async move {
            dispatcher
                .dispatch(&hook_owned, &event_owned, &ctx_owned)
                .await
        });
        if let Err(e) = registry.spawn(hook.id, hook.timeout, work).await {
            tracing::warn!(
                hook_id = %hook.id,
                error = %e,
                "failed to background async hook; it will not run",
            );
        }
    }

}

/// Cheaply-cloneable snapshot of the executor dependencies needed to run a
/// single hook. Built by [`HookExecutorImpl::dispatcher`]. Because every field
/// is an `Arc` / `Clone`, a `Dispatcher` can be moved into a `'static`
/// background future (B5 async path) or borrowed inline for the synchronous
/// path — both reach the identical [`Self::dispatch`] arm logic.
#[derive(Clone)]
struct Dispatcher {
    http: Arc<dyn HttpTransport>,
    ssrf_guard: SsrfGuard,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    agent_spawner: Option<Arc<dyn SubagentSpawner>>,
    prompt_runner: Option<Arc<dyn HookPromptRunner>>,
    process: Option<Arc<dyn ProcessRunner>>,
    sandbox: Option<Arc<dyn Sandbox>>,
}

impl Dispatcher {
    #[allow(
        clippy::too_many_lines,
        reason = "arm dispatch fan-out — splitting hurts readability"
    )]
    async fn dispatch(
        &self,
        hook: &HookDefinition,
        event: &HookEvent,
        ctx: &HookContext,
    ) -> HookResult {
        match &hook.executor {
            HookExecutor::Builtin { handler_id } => {
                if let Some(h) = self.builtin_handlers.get(handler_id) {
                    return h.handle(event, ctx).await;
                }
                HookResult {
                    outcome: HookOutcome::Error,
                    stdout: String::new(),
                    stderr: format!("builtin {handler_id} not found"),
                    exit_code: None,
                    response: None,
                }
            }
            HookExecutor::Http {
                url,
                headers,
                timeout,
                ..
            } => {
                // Resolve event-specific expected_event marker + serialize payload.
                let Some((expected_event, body)) = build_envelope_body(event, ctx) else {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!(
                            "Hook {} failed: HTTP arm only supports PreToolUse / PostToolUse",
                            hook.id
                        ),
                        exit_code: None,
                        response: None,
                    };
                };
                let effective = if timeout.is_zero() {
                    Duration::from_millis(HOOK_HTTP_TIMEOUT_MS)
                } else {
                    *timeout
                };
                let exec = HttpExecutor {
                    http: self.http.clone(),
                    ssrf_guard: self.ssrf_guard.clone(),
                    timeout: effective,
                };
                let outcome = exec
                    .execute(hook, url, headers, &body, expected_event)
                    .await;
                emit_http_signal(hook, &outcome.signal, effective);
                outcome.result
            }
            HookExecutor::Command {
                command,
                args,
                env,
                cwd,
            } => {
                // Both the runner and the sandbox must be wired: the runner
                // only accepts a `SandboxedCommand`, which only the sandbox can
                // mint (D2 / spec A1). Either missing ⇒ structured "not wired".
                let (Some(process), Some(sandbox)) = (&self.process, &self.sandbox) else {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!("Hook {} failed: command executor not wired", hook.id),
                        exit_code: None,
                        response: None,
                    };
                };
                let Some((expected_event, body)) = build_envelope_body(event, ctx) else {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!(
                            "Hook {} failed: Command arm only supports PreToolUse / PostToolUse",
                            hook.id
                        ),
                        exit_code: None,
                        response: None,
                    };
                };
                // Mirror the HTTP arm's zero-duration handling: an explicit
                // `timeout: 0` (or no timeout) falls back to the 10-minute
                // default rather than instantly timing out the child.
                let effective_timeout = match hook.timeout {
                    Some(t) if !t.is_zero() => t,
                    _ => Duration::from_millis(HOOK_COMMAND_TIMEOUT_MS),
                };
                // B2: inject `CLAUDE_PROJECT_DIR` into the child env so hook
                // scripts referencing `$CLAUDE_PROJECT_DIR` resolve to the
                // stable project root. claude-code builds the env as
                // `{ ...subprocessEnv(), CLAUDE_PROJECT_DIR: toHookPath(projectDir) }`
                // (`utils/hooks.ts:882-885`): the engine value is spread AFTER
                // the base env, so it wins over any pre-existing entry. We
                // mirror that precedence — start from the hook's declared `env`
                // (our analog of the base/subprocess env), then `insert` the
                // engine value last so it overwrites a user-supplied
                // `CLAUDE_PROJECT_DIR`. The value is the stable project root,
                // falling back to `ctx.cwd` when no root is wired yet
                // (`HookContext.project_dir == None`).
                //
                // Divergence (documented, not a gap): Windows `toHookPath`
                // POSIX-path conversion is skipped — macOS/Linux parity target,
                // consistent with `turn_loop.rs::absolutize`. Full
                // `subprocessEnv()` base-env replication and `CLAUDE_ENV_FILE`
                // are out of B2 scope.
                let mut child_env = env.clone();
                let project_dir = ctx
                    .project_dir
                    .clone()
                    .unwrap_or_else(|| ctx.cwd.clone());
                child_env.insert(
                    "CLAUDE_PROJECT_DIR".to_string(),
                    project_dir.to_string_lossy().into_owned(),
                );
                // claude-code writes `jsonStringify(hookInput) + '\n'` to the
                // child's stdin then closes it (`hooks.ts:1006`/`1210`). The
                // trailing newline is load-bearing: a bash `read -r line`
                // returns exit 1 on EOF-before-delimiter without it.
                let pcmd = ProcessCommand {
                    command: command.clone(),
                    args: args.clone(),
                    cwd: cwd.clone().or_else(|| Some(ctx.cwd.clone())),
                    env: child_env,
                    timeout: Some(effective_timeout),
                    stdin: Some(format!("{body}\n")),
                };
                // Hooks require workspace trust upstream (claude-code
                // `shouldSkipHookDueToTrust`), so an audited bypass is the
                // parity-honest construction here.
                let sandboxed = sandbox.bypass_with_audit(pcmd, "hook_command");
                let (result, timed_out) =
                    map_command_output(hook, process.run(&sandboxed).await, expected_event);
                if timed_out {
                    emit_command_timeout(hook, effective_timeout);
                }
                result
            }
            HookExecutor::Agent { agent_type, prompt } => {
                let Some((expected_event, body)) = build_envelope_body(event, ctx) else {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!(
                            "Hook {} failed: Agent arm only supports PreToolUse / PostToolUse",
                            hook.id
                        ),
                        exit_code: None,
                        response: None,
                    };
                };
                let effective_timeout = hook
                    .timeout
                    .unwrap_or(Duration::from_millis(HOOK_AGENT_TIMEOUT_MS));
                let exec = AgentExecutor {
                    spawner: self.agent_spawner.clone(),
                    timeout: effective_timeout,
                };
                let outcome = exec
                    .execute(
                        hook,
                        agent_type,
                        prompt,
                        &body,
                        expected_event,
                        ctx.inherit.clone(),
                    )
                    .await;
                emit_agent_signal(hook, &outcome.signal, effective_timeout);
                outcome.result
            }
            HookExecutor::Prompt { prompt, model } => {
                let Some((_expected_event, body)) = build_envelope_body(event, ctx) else {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: format!(
                            "Hook {} failed: Prompt arm only supports PreToolUse / PostToolUse",
                            hook.id
                        ),
                        exit_code: None,
                        response: None,
                    };
                };
                // `execPromptHook.ts:55`: `hook.timeout * 1000` (seconds → ms),
                // else the 30 s default. Our `hook.timeout` is already a
                // `Duration`; a zero/absent timeout defers to the default.
                let effective_timeout = match hook.timeout {
                    Some(t) if !t.is_zero() => t,
                    _ => Duration::from_millis(HOOK_PROMPT_TIMEOUT_MS),
                };
                let exec = PromptExecutor {
                    runner: self.prompt_runner.clone(),
                    timeout: effective_timeout,
                };
                let outcome = exec
                    .execute(hook, prompt, model.as_deref(), &body)
                    .await;
                emit_prompt_signal(hook, &outcome.signal, effective_timeout);
                outcome.result
            }
        }
    }
}

impl HookExecutorImpl {
    fn merge(agg: &mut AggregateHookResult, hook: &HookDefinition, r: HookResult) {
        if let Some(resp) = &r.response {
            if resp.decision.is_some() {
                agg.decision = resp.decision;
            }
            if let Some(reason) = &resp.reason {
                agg.reason = Some(reason.clone());
            }
            if let Some(input) = &resp.updated_input {
                agg.modified_input = Some(input.clone());
            }
            if let Some(msg) = &resp.system_message {
                agg.system_messages.push(msg.clone());
            }
            // B4: OR-fold the preventContinuation (`continue:false`) signal so
            // a single lifecycle hook can terminate the agent loop even when
            // earlier hooks did not. Independent of `decision`.
            if resp.prevent_continuation {
                agg.prevent_continuation = true;
            }
            // Elicitation answer: keep the latest non-empty response, mirroring
            // `executeElicitationHooks`'s loop (`utils/hooks.ts:4512-4520`)
            // which overwrites `elicitationResponse` with each parsed result.
            if let Some(er) = &resp.elicitation_response {
                agg.elicitation_response = Some(er.clone());
            }
            // PostToolUse `updatedMCPToolOutput`: keep the latest replacement
            // any folded hook returned, mirroring TS's per-hook assignment
            // (`result.updatedMCPToolOutput = …`, `utils/hooks.ts:647`). The
            // orchestrator applies it only for MCP tools (`isMcpTool(tool)`).
            if let Some(out) = &resp.updated_mcp_tool_output {
                agg.updated_mcp_tool_output = Some(out.clone());
            }
            agg.attachments.extend(resp.attachments.clone());
        }
        agg.all_results.push((hook.id, r));
    }
}

/// Build the serialized envelope body + `expected_event` marker for an event.
///
/// Returns `None` for event variants the HTTP / Agent arms don't yet support.
/// Tool events (`PreToolUse` / `PostToolUse`, M5-06) plus the lifecycle events
/// `Stop` / `SubagentStop` / `TaskCompleted` / `UserPromptSubmit` /
/// `SessionStart` / `StopFailure` (B1) are serialized here; every other variant
/// still falls through to `None` until its wire schema is ported.
///
/// Where a [`HookEvent`] variant carries fewer fields than the claude-code wire
/// schema (e.g. `Stop` has no `stop_hook_active` / `last_assistant_message`
/// yet, `TaskCompleted` only carries `task_id`), the available fields are
/// populated and the rest defaulted (`false` / `None` / empty string). The
/// missing fields are filled by later B-cluster batches that thread richer
/// context through `HookEvent` / `HookContext`.
fn build_envelope_body(event: &HookEvent, ctx: &HookContext) -> Option<(&'static str, String)> {
    match event {
        HookEvent::PreToolUse {
            tool_name,
            tool_input,
            tool_use_id,
        } => {
            let payload = PreToolUsePayload {
                hook_event_name: HookEventNamePre,
                session_id: ctx.session_id.to_string(),
                transcript_path: ctx.transcript_path.to_string_lossy().into_owned(),
                cwd: ctx.cwd.to_string_lossy().into_owned(),
                permission_mode: ctx.permission_mode.clone(),
                agent_id: ctx.agent_id.as_ref().map(ToString::to_string),
                agent_type: ctx.agent_type.clone(),
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_use_id: tool_use_id.to_string(),
            };
            Some(("PreToolUse", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::PostToolUse {
            tool_name,
            tool_input,
            tool_output,
            tool_use_id,
        } => {
            let payload = PostToolUsePayload {
                hook_event_name: HookEventNamePost,
                session_id: ctx.session_id.to_string(),
                transcript_path: ctx.transcript_path.to_string_lossy().into_owned(),
                cwd: ctx.cwd.to_string_lossy().into_owned(),
                permission_mode: ctx.permission_mode.clone(),
                agent_id: ctx.agent_id.as_ref().map(ToString::to_string),
                agent_type: ctx.agent_type.clone(),
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_response: tool_output.clone(),
                tool_use_id: tool_use_id.to_string(),
            };
            Some(("PostToolUse", serde_json::to_string(&payload).ok()?))
        }
        // Lifecycle events (B1) share the `createBaseHookInput` base shape; they
        // are split into a helper to keep this dispatch readable.
        _ => build_lifecycle_envelope_body(event, ctx),
    }
}

/// The `createBaseHookInput` base shape (`utils/hooks.ts:301-328`) extracted
/// from a [`HookContext`], reused by every lifecycle payload.
struct BaseHookFields {
    session_id: String,
    transcript_path: String,
    cwd: String,
    permission_mode: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
}

impl BaseHookFields {
    fn from_ctx(ctx: &HookContext) -> Self {
        Self {
            session_id: ctx.session_id.to_string(),
            transcript_path: ctx.transcript_path.to_string_lossy().into_owned(),
            cwd: ctx.cwd.to_string_lossy().into_owned(),
            permission_mode: ctx.permission_mode.clone(),
            agent_id: ctx.agent_id.as_ref().map(ToString::to_string),
            agent_type: ctx.agent_type.clone(),
        }
    }
}

/// Serialize the B1 lifecycle events (`Stop` / `SubagentStop` /
/// `TaskCompleted` / `UserPromptSubmit` / `SessionStart` / `StopFailure`) plus
/// the B6 additions (`PostToolUseFailure` / `SessionEnd` / `PreCompact` /
/// `PostCompact` / `Notification` / `PermissionRequest` / `Setup` /
/// `SubagentStart` / `CwdChanged` / `FileChanged` / `WorktreeRemove`).
///
/// Where a [`HookEvent`] variant carries fewer fields than the claude-code wire
/// schema, the available fields are populated and the rest defaulted
/// (`false` / `None` / empty string) — filled by later B-cluster batches.
///
/// The final four events (`ConfigChange` / `InstructionsLoaded` /
/// `Elicitation` / `WorktreeCreate`) — previously deferred because their
/// `HookEvent` variant lacked a field to source a *required* wire value — now
/// carry those fields and serialize faithfully. `PermissionDenied` is likewise
/// ported here (the hook-firing batch extended its variant with `tool_input` /
/// `tool_use_id`). `TaskCreated` is now serialized too (mirroring the
/// `TaskCompleted` arm, fired through the `TaskCreatedFirer` seam). Every
/// remaining (not-yet-ported) variant — `TeammateIdle`, `ElicitationResult` —
/// returns `None` until its wire schema is ported.
#[allow(
    clippy::too_many_lines,
    reason = "per-event payload construction fan-out — splitting hurts readability"
)]
fn build_lifecycle_envelope_body(
    event: &HookEvent,
    ctx: &HookContext,
) -> Option<(&'static str, String)> {
    let b = BaseHookFields::from_ctx(ctx);
    match event {
        HookEvent::Stop { .. } => {
            let payload = StopPayload {
                hook_event_name: HookEventNameStop,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                stop_hook_active: false,
                last_assistant_message: None,
            };
            Some(("Stop", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SubagentStop { agent_id, .. } => {
            let payload = SubagentStopPayload {
                hook_event_name: HookEventNameSubagentStop,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                stop_hook_active: false,
                agent_id: agent_id.to_string(),
                agent_transcript_path: String::new(),
                agent_type: b.agent_type.unwrap_or_default(),
                last_assistant_message: None,
            };
            Some(("SubagentStop", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::TaskCompleted {
            task_id,
            task_subject,
            task_description,
            teammate_name,
            team_name,
            ..
        } => {
            let payload = TaskCompletedPayload {
                hook_event_name: HookEventNameTaskCompleted,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                task_id: task_id.clone(),
                task_subject: task_subject.clone(),
                task_description: task_description.clone(),
                teammate_name: teammate_name.clone(),
                team_name: team_name.clone(),
            };
            Some(("TaskCompleted", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::TaskCreated {
            task_id,
            task_type,
            description,
        } => {
            // `executeTaskCreatedHooks` (`utils/hooks.ts:3756-3764`): the wire
            // payload carries `task_subject` (required) + optional
            // `task_description` / `teammate_name` / `team_name`. The Rust
            // `TaskCreated` variant sources the subject from the task's
            // `task_type` taxonomy bucket and the description from
            // `description`; `teammate_name` / `team_name` are not stored on the
            // task state, so they ride as `None` (same documented gap as the
            // `TaskCompleted` arm).
            let payload = TaskCreatedPayload {
                hook_event_name: HookEventNameTaskCreated,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                task_id: task_id.clone(),
                task_subject: task_type.clone(),
                task_description: Some(description.clone()),
                teammate_name: None,
                team_name: None,
            };
            Some(("TaskCreated", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::UserPromptSubmit { prompt } => {
            let payload = UserPromptSubmitPayload {
                hook_event_name: HookEventNameUserPromptSubmit,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                prompt: prompt.clone(),
            };
            Some(("UserPromptSubmit", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SessionStart { source, .. } => {
            let payload = SessionStartPayload {
                hook_event_name: HookEventNameSessionStart,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                source: source.clone(),
                agent_type: b.agent_type,
                model: None,
            };
            Some(("SessionStart", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::StopFailure { error } => {
            let payload = StopFailurePayload {
                hook_event_name: HookEventNameStopFailure,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                error: error.clone(),
                error_details: None,
                last_assistant_message: None,
            };
            Some(("StopFailure", serde_json::to_string(&payload).ok()?))
        }
        // B6 — additional events whose `HookEvent` variant already exists.
        // Where the variant carries fewer fields than the wire schema (e.g.
        // `Setup`/`PostCompact` lack `trigger`), the missing fields default
        // exactly as the B1 arms above (`""` / `Value::Null` / `None`) until
        // richer context is threaded through `HookEvent` / `HookContext`.
        // `PostToolUseFailure` now carries the dispatched `tool_input` (the
        // same `effective_input` the `PostToolUse` arm threads), matching the
        // claude-code `PostToolUseFailure` input schema.
        HookEvent::PostToolUseFailure {
            tool_name,
            tool_input,
            error,
            tool_use_id,
        } => {
            let payload = PostToolUseFailurePayload {
                hook_event_name: HookEventNamePostToolUseFailure,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_use_id: tool_use_id.to_string(),
                error: error.clone(),
                is_interrupt: None,
            };
            Some(("PostToolUseFailure", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SessionEnd { reason, .. } => {
            let payload = SessionEndPayload {
                hook_event_name: HookEventNameSessionEnd,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                reason: reason.clone(),
            };
            Some(("SessionEnd", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::PreCompact { reason } => {
            let payload = PreCompactPayload {
                hook_event_name: HookEventNamePreCompact,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                // `HookEvent::PreCompact.reason` is the `manual` / `auto`
                // trigger in the wire schema.
                trigger: reason.clone(),
                custom_instructions: None,
            };
            Some(("PreCompact", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::PostCompact { summary, .. } => {
            let payload = PostCompactPayload {
                hook_event_name: HookEventNamePostCompact,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                trigger: String::new(),
                compact_summary: summary.clone(),
            };
            Some(("PostCompact", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::Notification { message, kind } => {
            let payload = NotificationPayload {
                hook_event_name: HookEventNameNotification,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                message: message.clone(),
                title: None,
                notification_type: kind.clone(),
            };
            Some(("Notification", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::PermissionRequest {
            tool_name,
            tool_input,
            ..
        } => {
            let payload = PermissionRequestPayload {
                hook_event_name: HookEventNamePermissionRequest,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                permission_suggestions: None,
            };
            Some(("PermissionRequest", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::PermissionDenied {
            tool_name,
            tool_input,
            tool_use_id,
            reason,
        } => {
            let payload = PermissionDeniedPayload {
                hook_event_name: HookEventNamePermissionDenied,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_use_id: tool_use_id.to_string(),
                reason: reason.clone(),
            };
            Some(("PermissionDenied", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::Setup => {
            let payload = SetupPayload {
                hook_event_name: HookEventNameSetup,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                trigger: String::new(),
            };
            Some(("Setup", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SubagentStart {
            agent_id,
            agent_type,
            ..
        } => {
            let payload = SubagentStartPayload {
                hook_event_name: HookEventNameSubagentStart,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: agent_id.to_string(),
                agent_type: agent_type.clone(),
            };
            Some(("SubagentStart", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::CwdChanged { old, new } => {
            let payload = CwdChangedPayload {
                hook_event_name: HookEventNameCwdChanged,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                old_cwd: old.to_string_lossy().into_owned(),
                new_cwd: new.to_string_lossy().into_owned(),
            };
            Some(("CwdChanged", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::FileChanged { path, kind } => {
            let payload = FileChangedPayload {
                hook_event_name: HookEventNameFileChanged,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                file_path: path.to_string_lossy().into_owned(),
                event: kind.clone(),
            };
            Some(("FileChanged", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::WorktreeRemove { path } => {
            let payload = WorktreeRemovePayload {
                hook_event_name: HookEventNameWorktreeRemove,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                worktree_path: path.to_string_lossy().into_owned(),
            };
            Some(("WorktreeRemove", serde_json::to_string(&payload).ok()?))
        }
        // Deferred-completion batch — the final four events. Their `HookEvent`
        // variant now carries the field(s) needed to source each *required*
        // wire value, so they serialize faithfully.
        //
        // claude-code parity note on `permission_mode`: `ConfigChange`,
        // `InstructionsLoaded`, and `WorktreeCreate` build their base shape with
        // `createBaseHookInput(undefined)` (`utils/hooks.ts:4220` / `4354` /
        // `4932`), so they emit NO `permission_mode` regardless of the engine's
        // current mode — we pass `None` rather than `b.permission_mode`.
        // `Elicitation` alone uses `createBaseHookInput(permissionMode)`
        // (`utils/hooks.ts:4492`), so it threads `b.permission_mode`.
        HookEvent::ConfigChange { source, file_path } => {
            let payload = ConfigChangePayload {
                hook_event_name: HookEventNameConfigChange,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: None,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                source: *source,
                file_path: file_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            };
            Some(("ConfigChange", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::InstructionsLoaded {
            file_path,
            memory_type,
            load_reason,
            globs,
            trigger_file_path,
            parent_file_path,
        } => {
            let payload = InstructionsLoadedPayload {
                hook_event_name: HookEventNameInstructionsLoaded,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: None,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                file_path: file_path.to_string_lossy().into_owned(),
                memory_type: *memory_type,
                load_reason: *load_reason,
                globs: globs.clone(),
                trigger_file_path: trigger_file_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                parent_file_path: parent_file_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
            };
            Some(("InstructionsLoaded", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::Elicitation {
            server_name,
            message,
            mode,
            url,
            elicitation_id,
            requested_schema,
        } => {
            let payload = ElicitationPayload {
                hook_event_name: HookEventNameElicitation,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                mcp_server_name: server_name.clone(),
                message: message.clone(),
                mode: *mode,
                url: url.clone(),
                elicitation_id: elicitation_id.clone(),
                requested_schema: requested_schema.clone(),
            };
            Some(("Elicitation", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::WorktreeCreate { name, .. } => {
            let payload = WorktreeCreatePayload {
                hook_event_name: HookEventNameWorktreeCreate,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: None,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                name: name.clone(),
            };
            Some(("WorktreeCreate", serde_json::to_string(&payload).ok()?))
        }
        // `executeTeammateIdleHooks` (`utils/hooks.ts:3716-3720`): the wire
        // payload carries `teammate_name` + `team_name` (BOTH required strings).
        // Unlike `ConfigChange` / `InstructionsLoaded` / `WorktreeCreate`, this
        // fires through `createBaseHookInput(permissionMode)` (the `permissionMode`
        // threaded from `stopHooks.ts`), so it emits `permission_mode` — same as
        // `Elicitation`. claude-code sources `team_name` from `getTeamName() ?? ''`,
        // so a `""` here is faithful when the firing scope has no team identity.
        HookEvent::TeammateIdle {
            teammate_name,
            team_name,
        } => {
            let payload = TeammateIdlePayload {
                hook_event_name: HookEventNameTeammateIdle,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                teammate_name: teammate_name.clone(),
                team_name: team_name.clone(),
            };
            Some(("TeammateIdle", serde_json::to_string(&payload).ok()?))
        }
        _ => None,
    }
}

/// Map a [`ProcessRunner::run`] result onto a [`HookResult`] per the
/// claude-code command-hook contract (`claude-code/src/utils/hooks.ts`).
///
/// Returns `(result, timed_out)` where the bool tells the caller whether to
/// emit `HOOK_TIMEOUT` telemetry.
///
/// Decision layering (mirrors `hooks.ts:2499-2697`):
/// 1. If stdout (trimmed) starts with `{`, parse it via [`parse_response`].
///    A parsed `HookResponse.decision` drives blocking through `merge`.
/// 2. Otherwise apply the exit-code fallback:
///    - `0` ⇒ success, no decision.
///    - `2` ⇒ **block**, with stderr as the reason (`hooks.ts:2648-2666`).
///    - any other non-zero ⇒ non-blocking error (`Error`, no `Block` decision).
fn map_command_output(
    hook: &HookDefinition,
    run: Result<traits::ProcessOutput, ProcessError>,
    expected_event: &'static str,
) -> (HookResult, bool) {
    match run {
        Err(ProcessError::Timeout) => (
            HookResult {
                outcome: HookOutcome::Timeout,
                stdout: String::new(),
                stderr: format!("Hook {} failed: command timed out", hook.id),
                exit_code: None,
                response: None,
            },
            true,
        ),
        Err(e @ (ProcessError::Io(_) | ProcessError::Unsupported)) => (
            HookResult {
                outcome: HookOutcome::Error,
                stdout: String::new(),
                stderr: format!("Hook {} failed: process error: {e}", hook.id),
                exit_code: None,
                response: None,
            },
            false,
        ),
        Ok(o) if o.timed_out => (
            HookResult {
                outcome: HookOutcome::Timeout,
                stdout: o.stdout,
                stderr: o.stderr,
                exit_code: Some(o.exit_code),
                response: None,
            },
            true,
        ),
        Ok(o) => {
            // Layer (a): JSON stdout. Guard on a leading `{` to match
            // `parseHookOutput`'s plain-text bypass (`hooks.ts:404-408`).
            if o.stdout.trim_start().starts_with('{') {
                if let Ok(parsed) = parse_response(&o.stdout, expected_event) {
                    let outcome = if o.exit_code == 0 {
                        HookOutcome::Success
                    } else {
                        HookOutcome::Error
                    };
                    return (
                        HookResult {
                            outcome,
                            stdout: o.stdout,
                            stderr: o.stderr,
                            exit_code: Some(o.exit_code),
                            response: Some(parsed),
                        },
                        false,
                    );
                }
            }
            // Layer (b): exit-code fallback for plain-text / unparsable output.
            match o.exit_code {
                0 => (
                    HookResult {
                        outcome: HookOutcome::Success,
                        stdout: o.stdout,
                        stderr: o.stderr,
                        exit_code: Some(0),
                        response: None,
                    },
                    false,
                ),
                2 => {
                    // exit 2 ⇒ BLOCK; stderr is the reason
                    // (`hooks.ts:2648-2666`). Empty stderr ⇒ placeholder.
                    let stderr_trim = o.stderr.trim();
                    let reason = if stderr_trim.is_empty() {
                        "No stderr output".to_string()
                    } else {
                        stderr_trim.to_string()
                    };
                    (
                        HookResult {
                            outcome: HookOutcome::Error,
                            stdout: o.stdout,
                            stderr: o.stderr,
                            exit_code: Some(2),
                            response: Some(HookResponse {
                                decision: Some(HookDecision::Block),
                                reason: Some(reason),
                                ..HookResponse::default()
                            }),
                        },
                        false,
                    )
                }
                other => (
                    // Any other non-zero ⇒ non-blocking error: NO Block
                    // decision (`hooks.ts:2670-2696`).
                    HookResult {
                        outcome: HookOutcome::Error,
                        stdout: o.stdout,
                        stderr: o.stderr,
                        exit_code: Some(other),
                        response: None,
                    },
                    false,
                ),
            }
        }
    }
}

/// Emit `HOOK_TIMEOUT` telemetry for a Command hook that exceeded its
/// timeout (mirrors [`emit_http_signal`] / [`emit_agent_signal`]).
fn emit_command_timeout(hook: &HookDefinition, timeout: Duration) {
    // hook timeout bounded to seconds — u128 ms cannot exceed u64::MAX
    #[allow(clippy::cast_possible_truncation)]
    let timeout_ms = timeout.as_millis() as u64;
    tracing::info!(
        event = telemetry::tengu::orchestrator::HOOK_TIMEOUT,
        hook_id = %hook.id,
        hook_kind = "command",
        timeout_ms = timeout_ms,
    );
}

/// Emit arm-level telemetry for an HTTP signal (SSRF / timeout). Other
/// telemetry (`HOOK_PRE_*` / `HOOK_POST_*`) is fired by the orchestrator's
/// `dispatch_tool_with_hooks` (M5-06 Task 14).
fn emit_http_signal(hook: &HookDefinition, signal: &HttpExecutionSignal, timeout: Duration) {
    match signal {
        HttpExecutionSignal::SsrfBlocked(reason) => {
            tracing::info!(
                event = telemetry::tengu::orchestrator::HOOK_HTTP_SKIPPED_SSRF,
                hook_id = %hook.id,
                reason = %reason,
            );
        }
        HttpExecutionSignal::TimedOut => {
            // hook timeout bounded to seconds — u128 ms cannot exceed u64::MAX
            #[allow(clippy::cast_possible_truncation)]
            let timeout_ms = timeout.as_millis() as u64;
            tracing::info!(
                event = telemetry::tengu::orchestrator::HOOK_TIMEOUT,
                hook_id = %hook.id,
                hook_kind = "http",
                timeout_ms = timeout_ms,
            );
        }
        HttpExecutionSignal::Ok => {}
    }
}

fn emit_agent_signal(hook: &HookDefinition, signal: &AgentExecutionSignal, timeout: Duration) {
    if matches!(signal, AgentExecutionSignal::TimedOut) {
        // hook timeout bounded to seconds — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let timeout_ms = timeout.as_millis() as u64;
        tracing::info!(
            event = telemetry::tengu::orchestrator::HOOK_TIMEOUT,
            hook_id = %hook.id,
            hook_kind = "agent",
            timeout_ms = timeout_ms,
        );
    }
}

/// Emit arm-level telemetry for a Prompt signal (timeout). Mirrors
/// [`emit_agent_signal`]; the success / not-met / parse-error / not-wired
/// signals carry no dedicated telemetry event (parity with the Agent arm,
/// which only emits on timeout).
fn emit_prompt_signal(hook: &HookDefinition, signal: &PromptExecutionSignal, timeout: Duration) {
    if matches!(signal, PromptExecutionSignal::TimedOut) {
        // hook timeout bounded to seconds — u128 ms cannot exceed u64::MAX
        #[allow(clippy::cast_possible_truncation)]
        let timeout_ms = timeout.as_millis() as u64;
        tracing::info!(
            event = telemetry::tengu::orchestrator::HOOK_TIMEOUT,
            hook_id = %hook.id,
            hook_kind = "prompt",
            timeout_ms = timeout_ms,
        );
    }
}

#[cfg(test)]
mod constants_tests {
    use super::*;

    #[test]
    fn http_timeout_is_10_minutes() {
        assert_eq!(HOOK_HTTP_TIMEOUT_MS, 600_000);
    }
    #[test]
    fn command_timeout_is_10_minutes() {
        assert_eq!(HOOK_COMMAND_TIMEOUT_MS, 600_000);
    }
    #[test]
    fn agent_timeout_is_60_seconds() {
        assert_eq!(HOOK_AGENT_TIMEOUT_MS, 60_000);
    }
}

#[cfg(test)]
mod command_arm_tests {
    use super::*;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use crate::response::HookDecision;
    use protocol::{HookId, ToolUseId};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use traits::sandbox::{SandboxBackend, SandboxCapability, SandboxedTag};
    use traits::{
        ProcessHandle, ProcessOutput, RuntimeError, SandboxPolicy, SandboxedCommand,
    };

    /// Mock `ProcessRunner` that returns a canned `ProcessOutput` (or
    /// `ProcessError`) and records the `SandboxedCommand` it was handed so the
    /// test can assert the stdin payload + trailing newline.
    struct MockRunner {
        result: Mutex<Option<Result<ProcessOutput, ProcessError>>>,
        recorded_stdin: Mutex<Option<String>>,
        /// B2: the child env the arm handed to the sandbox, captured so tests
        /// can assert `CLAUDE_PROJECT_DIR` injection + precedence.
        recorded_env: Mutex<Option<HashMap<String, String>>>,
    }

    impl MockRunner {
        fn ok(output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Ok(output))),
                recorded_stdin: Mutex::new(None),
                recorded_env: Mutex::new(None),
            })
        }
        fn err(e: ProcessError) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Err(e))),
                recorded_stdin: Mutex::new(None),
                recorded_env: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            *self.recorded_stdin.lock().unwrap() = cmd.inner().stdin.clone();
            *self.recorded_env.lock().unwrap() = Some(cmd.inner().env.clone());
            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(ProcessError::Io("no script".into())))
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }
        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    /// Minimal `Sandbox` that mints a `SandboxedCommand` via the documented
    /// external-impl seam (`__new_sandboxed`) so the runner has something to
    /// accept. No real isolation — adequate for unit-testing the arm's
    /// output-mapping logic.
    struct StubSandbox;

    #[async_trait]
    impl Sandbox for StubSandbox {
        fn is_available(&self) -> bool {
            true
        }
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::None
        }
        fn prepare(
            &self,
            cmd: ProcessCommand,
            _policy: &SandboxPolicy,
        ) -> Result<SandboxedCommand, traits::SandboxError> {
            Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: "test".into(),
                },
            ))
        }
        fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
            SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: reason.into(),
                },
            )
        }
        async fn probe_capability(&self) -> SandboxCapability {
            SandboxCapability {
                available: true,
                reason: None,
                features: traits::SandboxFeatures::default(),
            }
        }
    }

    /// `RuntimeSpawner` stub — the runtime arm is never exercised here.
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, RuntimeError> {
            Err(RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _duration: Duration) {}
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// `HttpTransport` stub — the HTTP arm is never exercised here.
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    fn output(stdout: &str, stderr: &str, exit_code: i32) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
            timed_out: false,
        }
    }

    fn command_hook() -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "test-command".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Command {
                command: "hook.sh".into(),
                args: vec!["--check".into()],
                env: HashMap::new(),
                cwd: None,
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    fn pre_event() -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: ToolUseId::new(),
        }
    }

    /// Build an executor whose registry already holds a single Command hook
    /// matching `PreToolUse`, with the supplied runner + stub sandbox wired.
    fn executor_with(process: Arc<dyn ProcessRunner>) -> HookExecutorImpl {
        let mut registry = HookRegistry::new();
        registry.register(command_hook());
        let reg = Arc::new(RwLock::new(registry));
        HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime))
            .with_process_runner(process, Arc::new(StubSandbox))
    }

    #[tokio::test]
    async fn exit_zero_allows_no_decision() {
        let runner = MockRunner::ok(output("approved\n", "", 0));
        let exec = executor_with(runner.clone());

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, None, "exit 0 plain text must not block");
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success));
        assert_eq!(r.exit_code, Some(0));
        assert!(r.response.is_none());
        // stdin payload carries the serialized PreToolUse envelope + trailing
        // newline (claude-code parity).
        let stdin = runner.recorded_stdin.lock().unwrap().clone().unwrap();
        assert!(stdin.contains(r#""hook_event_name":"PreToolUse""#));
        assert!(stdin.contains(r#""tool_name":"Bash""#));
        assert!(stdin.ends_with('\n'), "trailing newline is load-bearing");
    }

    #[tokio::test]
    async fn exit_two_blocks_with_stderr_reason() {
        let runner = MockRunner::ok(output("", "policy violation", 2));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(agg.reason.as_deref(), Some("policy violation"));
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Error));
        assert_eq!(r.exit_code, Some(2));
    }

    #[tokio::test]
    async fn exit_two_empty_stderr_falls_back_to_placeholder() {
        let runner = MockRunner::ok(output("", "   ", 2));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(agg.reason.as_deref(), Some("No stderr output"));
    }

    #[tokio::test]
    async fn json_stdout_decision_drives_block() {
        let runner = MockRunner::ok(output(
            r#"{"decision":"block","stopReason":"json said no","continue":false}"#,
            "",
            0,
        ));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(agg.reason.as_deref(), Some("json said no"));
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success), "exit 0 + JSON");
    }

    #[tokio::test]
    async fn json_stdout_permission_allow_approves() {
        let runner = MockRunner::ok(output(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#,
            "",
            0,
        ));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, Some(HookDecision::Approve));
    }

    #[tokio::test]
    async fn other_nonzero_is_non_blocking_error() {
        let runner = MockRunner::ok(output("", "transient", 1));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, None, "non-0/non-2 must NOT block");
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Error));
        assert_eq!(r.exit_code, Some(1));
        assert!(r.response.is_none());
    }

    #[tokio::test]
    async fn process_io_error_surfaces_as_error() {
        let runner = MockRunner::err(ProcessError::Io("spawn failed".into()));
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, None);
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Error));
        assert!(r.stderr.contains("process error"));
        assert!(r.stderr.contains("spawn failed"));
    }

    #[tokio::test]
    async fn timed_out_output_maps_to_timeout() {
        let runner = MockRunner::ok(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        });
        let exec = executor_with(runner);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Timeout));
        assert_eq!(agg.decision, None);
    }

    #[tokio::test]
    async fn not_wired_returns_structured_error() {
        let mut registry = HookRegistry::new();
        registry.register(command_hook());
        let reg = Arc::new(RwLock::new(registry));
        // No `.with_process_runner(..)` — Command arm must report "not wired".
        let exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, None, "not-wired must not block");
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Error));
        assert!(r.stderr.contains("command executor not wired"));
    }

    // ---- B2: CLAUDE_PROJECT_DIR injection into the Command child env -----

    /// A Command hook whose declared `env` is seeded with `entries`.
    fn command_hook_with_env(entries: &[(&str, &str)]) -> HookDefinition {
        let mut h = command_hook();
        if let DefHookExecutor::Command { env, .. } = &mut h.executor {
            for (k, v) in entries {
                env.insert((*k).to_string(), (*v).to_string());
            }
        }
        h
    }

    /// Executor wired with a single Command hook (custom `env`) + the recording
    /// runner, so the test can assert the child env the sandbox received.
    fn executor_with_hook(hook: HookDefinition, process: Arc<dyn ProcessRunner>) -> HookExecutorImpl {
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(RwLock::new(registry));
        HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime))
            .with_process_runner(process, Arc::new(StubSandbox))
    }

    #[tokio::test]
    async fn command_env_contains_project_dir_from_ctx() {
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let ctx = HookContext {
            project_dir: Some(PathBuf::from("/repo/root")),
            cwd: PathBuf::from("/repo/root/worktree"),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert_eq!(
            env.get("CLAUDE_PROJECT_DIR").map(String::as_str),
            Some("/repo/root"),
            "engine project_dir is injected verbatim",
        );
    }

    #[tokio::test]
    async fn command_env_engine_project_dir_wins_over_user_env() {
        // The hook declares its own CLAUDE_PROJECT_DIR; the engine value is set
        // AFTER the base spread in claude-code (`utils/hooks.ts:882-885`), so
        // the engine value wins. Match that precedence.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with_hook(
            command_hook_with_env(&[
                ("CLAUDE_PROJECT_DIR", "/user/override"),
                ("MY_VAR", "keep-me"),
            ]),
            runner.clone(),
        );
        let ctx = HookContext {
            project_dir: Some(PathBuf::from("/engine/root")),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert_eq!(
            env.get("CLAUDE_PROJECT_DIR").map(String::as_str),
            Some("/engine/root"),
            "engine value overrides the user-supplied hook.env entry",
        );
        // Unrelated user env entries are preserved.
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("keep-me"));
    }

    #[tokio::test]
    async fn command_env_project_dir_falls_back_to_cwd() {
        // No project_dir wired → CLAUDE_PROJECT_DIR falls back to ctx.cwd, the
        // faithful approximation until the orchestrator populates a project root.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let ctx = HookContext {
            project_dir: None,
            cwd: PathBuf::from("/some/cwd"),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert_eq!(
            env.get("CLAUDE_PROJECT_DIR").map(String::as_str),
            Some("/some/cwd"),
            "absent project_dir falls back to ctx.cwd",
        );
    }

    // ---- B1: lifecycle events now serialize through the Command arm -----

    /// A Command hook subscribed to a single lifecycle `event` type.
    fn command_hook_for(event: HookEventType) -> HookDefinition {
        let mut h = command_hook();
        h.events = vec![event];
        h
    }

    /// Executor wired with a Command hook subscribed to `event`.
    fn executor_for(event: HookEventType, process: Arc<dyn ProcessRunner>) -> HookExecutorImpl {
        let mut registry = HookRegistry::new();
        registry.register(command_hook_for(event));
        let reg = Arc::new(RwLock::new(registry));
        HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime))
            .with_process_runner(process, Arc::new(StubSandbox))
    }

    /// Dispatch `event` through a Command hook and return the stdin the child
    /// would have received (the serialized envelope). Empty if no hook fired.
    async fn dispatch_and_capture(event_type: HookEventType, event: HookEvent) -> String {
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_for(event_type, runner.clone());
        let agg = exec.execute(event, HookContext::default()).await;
        assert_eq!(agg.all_results.len(), 1, "exactly one hook must fire");
        let captured = runner.recorded_stdin.lock().unwrap().clone().unwrap();
        captured
    }

    #[tokio::test]
    async fn stop_event_serializes_through_command_arm() {
        let stdin = dispatch_and_capture(
            HookEventType::Stop,
            HookEvent::Stop {
                reason: "done".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"Stop""#));
        assert!(stdin.contains(r#""stop_hook_active":false"#));
        assert!(stdin.ends_with('\n'), "trailing newline is load-bearing");
    }

    #[tokio::test]
    async fn subagent_stop_event_serializes_agent_id() {
        let agent_id = protocol::AgentId::new();
        let stdin = dispatch_and_capture(
            HookEventType::SubagentStop,
            HookEvent::SubagentStop {
                agent_id,
                status: "completed".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"SubagentStop""#));
        assert!(stdin.contains(&format!(r#""agent_id":"{agent_id}""#)));
        assert!(stdin.contains(r#""stop_hook_active":false"#));
    }

    #[tokio::test]
    async fn task_completed_event_serializes_task_id() {
        let stdin = dispatch_and_capture(
            HookEventType::TaskCompleted,
            HookEvent::TaskCompleted {
                task_id: "task-99".into(),
                status: "completed".into(),
                task_subject: "ship the thing".into(),
                task_description: Some("do the work".into()),
                teammate_name: Some("buddy".into()),
                team_name: Some("alpha".into()),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"TaskCompleted""#));
        assert!(stdin.contains(r#""task_id":"task-99""#));
        // Full wire payload (coreSchemas.ts:614-625) — sourced from the event,
        // not defaulted. `status` is routing-only and must NOT appear.
        assert!(stdin.contains(r#""task_subject":"ship the thing""#));
        assert!(stdin.contains(r#""task_description":"do the work""#));
        assert!(stdin.contains(r#""teammate_name":"buddy""#));
        assert!(stdin.contains(r#""team_name":"alpha""#));
        assert!(
            !stdin.contains(r#""status""#),
            "TaskCompleted wire payload has no `status` field: {stdin}"
        );
    }

    #[tokio::test]
    async fn user_prompt_submit_event_serializes_prompt() {
        let stdin = dispatch_and_capture(
            HookEventType::UserPromptSubmit,
            HookEvent::UserPromptSubmit {
                prompt: "do the thing".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"UserPromptSubmit""#));
        assert!(stdin.contains(r#""prompt":"do the thing""#));
    }

    #[tokio::test]
    async fn session_start_event_serializes_source() {
        let stdin = dispatch_and_capture(
            HookEventType::SessionStart,
            HookEvent::SessionStart {
                session_id: protocol::SessionId::nil(),
                source: "startup".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"SessionStart""#));
        assert!(stdin.contains(r#""source":"startup""#));
    }

    #[tokio::test]
    async fn stop_failure_event_serializes_error() {
        let stdin = dispatch_and_capture(
            HookEventType::StopFailure,
            HookEvent::StopFailure {
                error: "rate_limit".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"StopFailure""#));
        assert!(stdin.contains(r#""error":"rate_limit""#));
    }

    // ---- B6: additional events now serialize through the Command arm -----

    #[tokio::test]
    async fn post_tool_use_failure_event_serializes_error_and_tool_input() {
        let stdin = dispatch_and_capture(
            HookEventType::PostToolUseFailure,
            HookEvent::PostToolUseFailure {
                tool_name: "Bash".into(),
                tool_input: json!({"command": "ls"}),
                error: "boom".into(),
                tool_use_id: ToolUseId::new(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"PostToolUseFailure""#));
        assert!(stdin.contains(r#""tool_name":"Bash""#));
        assert!(stdin.contains(r#""error":"boom""#));
        // The dispatched `tool_input` is now carried verbatim, not `null`.
        assert!(stdin.contains(r#""tool_input":{"command":"ls"}"#));
    }

    #[tokio::test]
    async fn session_end_event_serializes_reason() {
        let stdin = dispatch_and_capture(
            HookEventType::SessionEnd,
            HookEvent::SessionEnd {
                session_id: protocol::SessionId::nil(),
                reason: "logout".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"SessionEnd""#));
        assert!(stdin.contains(r#""reason":"logout""#));
    }

    #[tokio::test]
    async fn pre_compact_event_serializes_trigger_and_null_instructions() {
        let stdin = dispatch_and_capture(
            HookEventType::PreCompact,
            HookEvent::PreCompact {
                reason: "manual".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"PreCompact""#));
        assert!(stdin.contains(r#""trigger":"manual""#));
        // `.nullable()` field is always present as `null` when absent.
        assert!(stdin.contains(r#""custom_instructions":null"#));
    }

    #[tokio::test]
    async fn post_compact_event_serializes_summary() {
        let stdin = dispatch_and_capture(
            HookEventType::PostCompact,
            HookEvent::PostCompact {
                summary: "did the thing".into(),
                tokens_freed: 1234,
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"PostCompact""#));
        assert!(stdin.contains(r#""compact_summary":"did the thing""#));
    }

    #[tokio::test]
    async fn notification_event_serializes_message_and_type() {
        let stdin = dispatch_and_capture(
            HookEventType::Notification,
            HookEvent::Notification {
                message: "build done".into(),
                kind: "info".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"Notification""#));
        assert!(stdin.contains(r#""message":"build done""#));
        assert!(stdin.contains(r#""notification_type":"info""#));
    }

    #[tokio::test]
    async fn permission_request_event_serializes_tool() {
        let stdin = dispatch_and_capture(
            HookEventType::PermissionRequest,
            HookEvent::PermissionRequest {
                tool_name: "Bash".into(),
                tool_input: json!({"command": "rm -rf /"}),
                reason: "destructive".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"PermissionRequest""#));
        assert!(stdin.contains(r#""tool_name":"Bash""#));
        assert!(stdin.contains(r#""tool_input":{"command":"rm -rf /"}"#));
        // The variant's `reason` has no wire counterpart and must NOT appear.
        assert!(!stdin.contains(r#""reason""#));
    }

    #[tokio::test]
    async fn setup_event_serializes_through_command_arm() {
        let stdin = dispatch_and_capture(HookEventType::Setup, HookEvent::Setup).await;
        assert!(stdin.contains(r#""hook_event_name":"Setup""#));
        assert!(stdin.contains(r#""trigger":"""#));
    }

    #[tokio::test]
    async fn subagent_start_event_serializes_agent() {
        let agent_id = protocol::AgentId::new();
        let stdin = dispatch_and_capture(
            HookEventType::SubagentStart,
            HookEvent::SubagentStart {
                agent_id,
                agent_type: "general-purpose".into(),
                parent_agent_id: None,
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"SubagentStart""#));
        assert!(stdin.contains(&format!(r#""agent_id":"{agent_id}""#)));
        assert!(stdin.contains(r#""agent_type":"general-purpose""#));
    }

    #[tokio::test]
    async fn cwd_changed_event_serializes_paths() {
        let stdin = dispatch_and_capture(
            HookEventType::CwdChanged,
            HookEvent::CwdChanged {
                old: std::path::PathBuf::from("/old"),
                new: std::path::PathBuf::from("/new"),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"CwdChanged""#));
        assert!(stdin.contains(r#""old_cwd":"/old""#));
        assert!(stdin.contains(r#""new_cwd":"/new""#));
    }

    #[tokio::test]
    async fn file_changed_event_serializes_path_and_event() {
        let stdin = dispatch_and_capture(
            HookEventType::FileChanged,
            HookEvent::FileChanged {
                path: std::path::PathBuf::from("/work/src/main.rs"),
                kind: "change".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"FileChanged""#));
        assert!(stdin.contains(r#""file_path":"/work/src/main.rs""#));
        assert!(stdin.contains(r#""event":"change""#));
    }

    #[tokio::test]
    async fn worktree_remove_event_serializes_path() {
        let stdin = dispatch_and_capture(
            HookEventType::WorktreeRemove,
            HookEvent::WorktreeRemove {
                path: std::path::PathBuf::from("/work/.worktrees/feat"),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"WorktreeRemove""#));
        assert!(stdin.contains(r#""worktree_path":"/work/.worktrees/feat""#));
    }

    // ---- deferred-completion batch: the final four events now serialize -----

    #[tokio::test]
    async fn config_change_event_serializes_source_and_file_path() {
        let stdin = dispatch_and_capture(
            HookEventType::ConfigChange,
            HookEvent::ConfigChange {
                source: crate::events::ConfigChangeSource::LocalSettings,
                file_path: Some(std::path::PathBuf::from("/work/.claude/settings.local.json")),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"ConfigChange""#));
        // `source` serializes to the snake_case wire literal.
        assert!(stdin.contains(r#""source":"local_settings""#));
        assert!(stdin.contains(r#""file_path":"/work/.claude/settings.local.json""#));
    }

    #[tokio::test]
    async fn config_change_event_omits_absent_file_path() {
        let stdin = dispatch_and_capture(
            HookEventType::ConfigChange,
            HookEvent::ConfigChange {
                source: crate::events::ConfigChangeSource::PolicySettings,
                file_path: None,
            },
        )
        .await;
        assert!(stdin.contains(r#""source":"policy_settings""#));
        // `.optional()` field is skipped (not `null`) when absent.
        assert!(!stdin.contains(r#""file_path""#));
        // `createBaseHookInput(undefined)` ⇒ no permission_mode on the wire.
        assert!(!stdin.contains(r#""permission_mode""#));
    }

    #[tokio::test]
    async fn instructions_loaded_event_serializes_required_fields() {
        let stdin = dispatch_and_capture(
            HookEventType::InstructionsLoaded,
            HookEvent::InstructionsLoaded {
                file_path: std::path::PathBuf::from("/work/CLAUDE.md"),
                memory_type: crate::events::InstructionsMemoryType::Project,
                load_reason: crate::events::InstructionsLoadReason::SessionStart,
                globs: None,
                trigger_file_path: None,
                parent_file_path: None,
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"InstructionsLoaded""#));
        assert!(stdin.contains(r#""file_path":"/work/CLAUDE.md""#));
        // `memory_type` serializes PascalCase (no rename) per the TS enum.
        assert!(stdin.contains(r#""memory_type":"Project""#));
        // `load_reason` serializes snake_case per the TS enum.
        assert!(stdin.contains(r#""load_reason":"session_start""#));
        // optional fields skipped when absent.
        assert!(!stdin.contains(r#""globs""#));
        assert!(!stdin.contains(r#""trigger_file_path""#));
        assert!(!stdin.contains(r#""parent_file_path""#));
    }

    #[tokio::test]
    async fn instructions_loaded_event_serializes_optionals() {
        let stdin = dispatch_and_capture(
            HookEventType::InstructionsLoaded,
            HookEvent::InstructionsLoaded {
                file_path: std::path::PathBuf::from("/work/rules/api.md"),
                memory_type: crate::events::InstructionsMemoryType::Local,
                load_reason: crate::events::InstructionsLoadReason::PathGlobMatch,
                globs: Some(vec!["src/**/*.rs".into()]),
                trigger_file_path: Some(std::path::PathBuf::from("/work/src/main.rs")),
                parent_file_path: Some(std::path::PathBuf::from("/work/CLAUDE.md")),
            },
        )
        .await;
        assert!(stdin.contains(r#""memory_type":"Local""#));
        assert!(stdin.contains(r#""load_reason":"path_glob_match""#));
        assert!(stdin.contains(r#""globs":["src/**/*.rs"]"#));
        assert!(stdin.contains(r#""trigger_file_path":"/work/src/main.rs""#));
        assert!(stdin.contains(r#""parent_file_path":"/work/CLAUDE.md""#));
    }

    #[tokio::test]
    async fn elicitation_event_serializes_server_and_message() {
        let stdin = dispatch_and_capture(
            HookEventType::Elicitation,
            HookEvent::Elicitation {
                server_name: "github".into(),
                message: "Authorize access?".into(),
                mode: None,
                url: None,
                elicitation_id: None,
                requested_schema: None,
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"Elicitation""#));
        assert!(stdin.contains(r#""mcp_server_name":"github""#));
        assert!(stdin.contains(r#""message":"Authorize access?""#));
        // optional fields skipped when absent.
        assert!(!stdin.contains(r#""mode""#));
        assert!(!stdin.contains(r#""url""#));
        assert!(!stdin.contains(r#""elicitation_id""#));
        assert!(!stdin.contains(r#""requested_schema""#));
    }

    #[tokio::test]
    async fn elicitation_event_serializes_optionals() {
        let stdin = dispatch_and_capture(
            HookEventType::Elicitation,
            HookEvent::Elicitation {
                server_name: "linear".into(),
                message: "Pick a project".into(),
                mode: Some(crate::events::ElicitationMode::Form),
                url: Some("https://example.test/auth".into()),
                elicitation_id: Some("elic-42".into()),
                requested_schema: Some(json!({"type": "object"})),
            },
        )
        .await;
        assert!(stdin.contains(r#""mode":"form""#));
        assert!(stdin.contains(r#""url":"https://example.test/auth""#));
        assert!(stdin.contains(r#""elicitation_id":"elic-42""#));
        assert!(stdin.contains(r#""requested_schema":{"type":"object"}"#));
    }

    #[tokio::test]
    async fn worktree_create_event_serializes_name() {
        let stdin = dispatch_and_capture(
            HookEventType::WorktreeCreate,
            HookEvent::WorktreeCreate {
                name: "feature-x".into(),
                path: std::path::PathBuf::from("/work/.worktrees/feature-x"),
                branch: "feature-x".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"WorktreeCreate""#));
        assert!(stdin.contains(r#""name":"feature-x""#));
        // Only `name` is on the wire — the engine-side path/branch must NOT leak.
        assert!(!stdin.contains(r#""path""#));
        assert!(!stdin.contains(r#""branch""#));
        assert!(!stdin.contains(r#"".worktrees""#));
    }

    #[test]
    fn still_unported_events_return_none() {
        // The deferred-completion batch ported the final four events that lacked
        // a field to source a *required* wire value; the hook-firing batch then
        // ported `PermissionDenied` (extending its `HookEvent` variant with
        // `tool_input` / `tool_use_id`), the lifecycle-firing batch ported
        // `TaskCreated` (fired through the `TaskCreatedFirer` seam, mirroring
        // `TaskCompleted`), and the teammate-idle batch ported `TeammateIdle`
        // (extending its variant with `teammate_name` / `team_name`, fired
        // through the `TeammateIdleFirer` seam). The only event still without a
        // ported wire schema (`ElicitationResult`) must continue to fall through
        // to `None` until its schema is ported.
        let ctx = HookContext::default();
        let unported = vec![HookEvent::ElicitationResult {
            server_name: "srv".into(),
            result: json!({}),
        }];
        for ev in unported {
            assert!(
                build_envelope_body(&ev, &ctx).is_none(),
                "still-unported event must not serialize: {ev:?}"
            );
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "exhaustive event-marker table — one entry per covered event"
    )]
    fn build_envelope_body_returns_expected_event_markers() {
        let ctx = HookContext::default();
        let cases: Vec<(HookEvent, &'static str)> = vec![
            (HookEvent::Stop { reason: "r".into() }, "Stop"),
            (
                HookEvent::SubagentStop {
                    agent_id: protocol::AgentId::new(),
                    status: "completed".into(),
                },
                "SubagentStop",
            ),
            (
                HookEvent::TaskCompleted {
                    task_id: "t".into(),
                    status: "completed".into(),
                    task_subject: "s".into(),
                    task_description: None,
                    teammate_name: None,
                    team_name: None,
                },
                "TaskCompleted",
            ),
            (
                HookEvent::TaskCreated {
                    task_id: "t".into(),
                    task_type: "LocalBash".into(),
                    description: "do the work".into(),
                },
                "TaskCreated",
            ),
            (
                HookEvent::TeammateIdle {
                    teammate_name: "buddy".into(),
                    team_name: "alpha".into(),
                },
                "TeammateIdle",
            ),
            (
                HookEvent::UserPromptSubmit { prompt: "p".into() },
                "UserPromptSubmit",
            ),
            (
                HookEvent::SessionStart {
                    session_id: protocol::SessionId::nil(),
                    source: "startup".into(),
                },
                "SessionStart",
            ),
            (
                HookEvent::StopFailure {
                    error: "unknown".into(),
                },
                "StopFailure",
            ),
            // B6 additions.
            (
                HookEvent::PostToolUseFailure {
                    tool_name: "Bash".into(),
                    tool_input: json!({}),
                    error: "boom".into(),
                    tool_use_id: ToolUseId::new(),
                },
                "PostToolUseFailure",
            ),
            (
                HookEvent::SessionEnd {
                    session_id: protocol::SessionId::nil(),
                    reason: "logout".into(),
                },
                "SessionEnd",
            ),
            (
                HookEvent::PreCompact {
                    reason: "manual".into(),
                },
                "PreCompact",
            ),
            (
                HookEvent::PostCompact {
                    summary: "s".into(),
                    tokens_freed: 0,
                },
                "PostCompact",
            ),
            (
                HookEvent::Notification {
                    message: "m".into(),
                    kind: "info".into(),
                },
                "Notification",
            ),
            (
                HookEvent::PermissionRequest {
                    tool_name: "Bash".into(),
                    tool_input: json!({}),
                    reason: "r".into(),
                },
                "PermissionRequest",
            ),
            (
                HookEvent::PermissionDenied {
                    tool_name: "Bash".into(),
                    tool_input: json!({ "command": "git push" }),
                    tool_use_id: protocol::ToolUseId::new(),
                    reason: "denied".into(),
                },
                "PermissionDenied",
            ),
            (HookEvent::Setup, "Setup"),
            (
                HookEvent::SubagentStart {
                    agent_id: protocol::AgentId::new(),
                    agent_type: "general-purpose".into(),
                    parent_agent_id: None,
                },
                "SubagentStart",
            ),
            (
                HookEvent::CwdChanged {
                    old: std::path::PathBuf::from("/o"),
                    new: std::path::PathBuf::from("/n"),
                },
                "CwdChanged",
            ),
            (
                HookEvent::FileChanged {
                    path: std::path::PathBuf::from("/f"),
                    kind: "change".into(),
                },
                "FileChanged",
            ),
            (
                HookEvent::WorktreeRemove {
                    path: std::path::PathBuf::from("/w"),
                },
                "WorktreeRemove",
            ),
            // Deferred-completion batch.
            (
                HookEvent::ConfigChange {
                    source: crate::events::ConfigChangeSource::UserSettings,
                    file_path: None,
                },
                "ConfigChange",
            ),
            (
                HookEvent::InstructionsLoaded {
                    file_path: std::path::PathBuf::from("/work/CLAUDE.md"),
                    memory_type: crate::events::InstructionsMemoryType::User,
                    load_reason: crate::events::InstructionsLoadReason::Include,
                    globs: None,
                    trigger_file_path: None,
                    parent_file_path: None,
                },
                "InstructionsLoaded",
            ),
            (
                HookEvent::Elicitation {
                    server_name: "srv".into(),
                    message: "m".into(),
                    mode: None,
                    url: None,
                    elicitation_id: None,
                    requested_schema: None,
                },
                "Elicitation",
            ),
            (
                HookEvent::WorktreeCreate {
                    name: "feat".into(),
                    path: std::path::PathBuf::from("/w"),
                    branch: "feat".into(),
                },
                "WorktreeCreate",
            ),
        ];
        for (ev, expected) in cases {
            let (marker, body) = build_envelope_body(&ev, &ctx).expect("must serialize");
            assert_eq!(marker, expected);
            // The serialized body's hook_event_name must equal the marker, and
            // round-trips through parse_response without a mismatch error.
            assert!(body.contains(&format!(r#""hook_event_name":"{expected}""#)));
        }
    }
}

// ============================================================================
// B5 — config-`async` (blocking == false) backgrounding through the executor.
// ============================================================================
#[cfg(test)]
mod async_path_tests {
    //! A non-blocking Command hook is routed to the [`AsyncHookRegistry`]
    //! instead of being awaited: `execute` returns immediately, the hook can
    //! NEVER contribute a `Block` to the aggregate, and a `blocking == true`
    //! hook still runs synchronously (the regression guard).
    use super::*;
    use crate::async_registry::AsyncHookRegistry;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use crate::response::HookDecision;
    use protocol::{HookId, ToolUseId};
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::{mpsc, Notify};
    use traits::sandbox::{SandboxBackend, SandboxCapability, SandboxedTag};
    use traits::{
        BackgroundTaskHandle, ProcessError, ProcessHandle, ProcessOutput, RuntimeError,
        RuntimeSpawner, SandboxPolicy, SandboxedCommand,
    };

    /// Tokio-backed runtime — the hooks crate already depends on tokio, so the
    /// background hook future can be spawned with `tokio::spawn` here.
    struct TestRuntime {
        next_id: AtomicU64,
        handles: StdMutex<HashMap<u64, tokio::task::JoinHandle<()>>>,
    }
    impl TestRuntime {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                next_id: AtomicU64::new(1),
                handles: StdMutex::new(HashMap::new()),
            })
        }
    }
    #[async_trait]
    impl RuntimeSpawner for TestRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ) -> Result<BackgroundTaskHandle, RuntimeError> {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let h = tokio::spawn(task);
            self.handles.lock().unwrap().insert(id, h);
            Ok(BackgroundTaskHandle {
                task_name: name.into(),
                task_id: id,
            })
        }
        async fn sleep(&self, duration: Duration) {
            tokio::time::sleep(duration).await;
        }
        async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
            if let Some(h) = self.handles.lock().unwrap().remove(&handle.task_id) {
                h.abort();
            }
            Ok(())
        }
    }

    /// `ProcessRunner` that parks on a [`Notify`] before producing its output,
    /// so a test can prove the hook is still in-flight when `execute` returns.
    struct GatedRunner {
        gate: Arc<Notify>,
        output: StdMutex<Option<ProcessOutput>>,
        ran: Arc<Notify>,
    }
    impl GatedRunner {
        fn new(gate: Arc<Notify>, ran: Arc<Notify>, output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                gate,
                output: StdMutex::new(Some(output)),
                ran,
            })
        }
    }
    #[async_trait]
    impl ProcessRunner for GatedRunner {
        async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            self.ran.notify_one();
            self.gate.notified().await;
            Ok(self.output.lock().unwrap().take().unwrap())
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }
        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    /// Minimal sandbox that mints a `SandboxedCommand` via the external-impl seam.
    struct StubSandbox;
    #[async_trait]
    impl Sandbox for StubSandbox {
        fn is_available(&self) -> bool {
            true
        }
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::None
        }
        fn prepare(
            &self,
            cmd: ProcessCommand,
            _policy: &SandboxPolicy,
        ) -> Result<SandboxedCommand, traits::SandboxError> {
            Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: "test".into(),
                },
            ))
        }
        fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
            SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: reason.into(),
                },
            )
        }
        async fn probe_capability(&self) -> SandboxCapability {
            SandboxCapability {
                available: true,
                reason: None,
                features: traits::SandboxFeatures::default(),
            }
        }
    }

    /// `HttpTransport` stub — never exercised by these Command-arm tests.
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    fn out(stdout: &str, stderr: &str, exit_code: i32) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
            timed_out: false,
        }
    }

    /// A Command hook with the supplied `blocking` flag, subscribed to `PreToolUse`.
    fn command_hook(blocking: bool) -> HookDefinition {
        command_hook_with_priority(blocking, 0)
    }

    /// As [`command_hook`] but with an explicit `priority` so a test can pin the
    /// firing order of a mixed (async + blocking) set deterministically.
    fn command_hook_with_priority(blocking: bool, priority: i32) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "async-cmd".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Command {
                command: "hook.sh".into(),
                args: vec![],
                env: HashMap::new(),
                cwd: None,
            },
            source: HookSource::User,
            blocking,
            timeout: None,
            priority,
            once: false,
            status_message: None,
        }
    }

    /// A `ProcessRunner` that returns a pre-canned [`ProcessOutput`] immediately
    /// (no gate) and records how many times it was invoked. Used by the mixed
    /// test where both the async and the blocking hook share one runner: we only
    /// need to assert the aggregate, not park either run.
    struct CountingRunner {
        output: StdMutex<ProcessOutput>,
        runs: AtomicU64,
    }
    impl CountingRunner {
        fn new(output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                output: StdMutex::new(output),
                runs: AtomicU64::new(0),
            })
        }
    }
    #[async_trait]
    impl ProcessRunner for CountingRunner {
        async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(self.output.lock().unwrap().clone())
        }
        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }
        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    fn pre_event() -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_use_id: ToolUseId::new(),
        }
    }

    /// (1) A `blocking == false` Command hook does NOT block the aggregate:
    /// even though the hook would exit 2 (a Block in the sync path), `execute`
    /// returns immediately with no decision while the hook is still parked.
    #[tokio::test]
    async fn non_blocking_hook_never_blocks_aggregate() {
        let runtime = TestRuntime::new();
        let (tx, _rx) = mpsc::channel(4);
        let async_reg = Arc::new(AsyncHookRegistry::new(runtime, tx));

        let gate = Arc::new(Notify::new());
        let ran = Arc::new(Notify::new());
        // exit 2 ⇒ would BLOCK on the synchronous path.
        let runner = GatedRunner::new(gate.clone(), ran.clone(), out("", "denied", 2));

        let hook = command_hook(false);
        let hook_id = hook.id;
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            // The executor's own `runtime` field is unused on the async path
            // (the registry owns spawning); a second TestRuntime satisfies the
            // constructor.
            TestRuntime::new(),
        )
        .with_process_runner(runner, Arc::new(StubSandbox))
        .with_async_registry(async_reg.clone());

        // `execute` must return WITHOUT awaiting the (still-parked) hook.
        let agg = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(
            agg.decision, None,
            "a backgrounded hook can never contribute a Block decision"
        );
        assert!(
            agg.all_results.is_empty(),
            "backgrounded hooks are excluded from the aggregate entirely"
        );

        // The hook is genuinely backgrounded and in-flight (its run() is parked
        // on the gate). Wait until the runner has actually begun.
        ran.notified().await;
        assert!(
            async_reg.is_in_flight(hook_id).await,
            "the backgrounded hook must be tracked in-flight"
        );

        // Let it finish so the test runtime doesn't leak the task.
        gate.notify_one();
    }

    /// (2) + (3) The registry records the in-flight handle and publishes the
    /// eventual result on `completion_tx`; here the hook completes normally.
    #[tokio::test]
    async fn non_blocking_hook_publishes_completion() {
        let runtime = TestRuntime::new();
        let (tx, mut rx) = mpsc::channel(4);
        let async_reg = Arc::new(AsyncHookRegistry::new(runtime, tx));

        let gate = Arc::new(Notify::new());
        let ran = Arc::new(Notify::new());
        let runner = GatedRunner::new(gate.clone(), ran.clone(), out("ok", "", 0));

        let hook = command_hook(false);
        let hook_id = hook.id;
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner, Arc::new(StubSandbox))
        .with_async_registry(async_reg.clone());

        let agg = exec.execute(pre_event(), HookContext::default()).await;
        assert!(agg.all_results.is_empty());

        ran.notified().await;
        assert!(async_reg.is_in_flight(hook_id).await);

        // Release the hook; its result must land on completion_tx keyed by id.
        gate.notify_one();
        let (got_id, got) = rx.recv().await.expect("completion must publish");
        assert_eq!(got_id, hook_id);
        assert!(matches!(got.outcome, HookOutcome::Success));
        assert_eq!(got.exit_code, Some(0));
    }

    /// (4) Regression guard: a `blocking == true` Command hook still runs
    /// SYNCHRONOUSLY — `execute` awaits it and its exit-2 Block is reflected in
    /// the aggregate exactly as before B5. (No async registry is even wired.)
    #[tokio::test]
    async fn blocking_hook_runs_synchronously() {
        let gate = Arc::new(Notify::new());
        let ran = Arc::new(Notify::new());
        // Pre-open the gate so the synchronous run() does not park.
        gate.notify_one();
        let runner = GatedRunner::new(gate, ran, out("", "policy violation", 2));

        let mut registry = HookRegistry::new();
        registry.register(command_hook(true));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner, Arc::new(StubSandbox))
        .with_async_registry(Arc::new(AsyncHookRegistry::new(
            TestRuntime::new(),
            mpsc::channel(1).0,
        )));

        let agg = exec.execute(pre_event(), HookContext::default()).await;
        // Synchronous: exit 2 ⇒ Block surfaces in the aggregate, result recorded.
        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(agg.reason.as_deref(), Some("policy violation"));
        assert_eq!(agg.all_results.len(), 1, "blocking hook IS in the aggregate");
    }

    /// (5) Mixed: one async (`blocking == false`) + one blocking hook fire for
    /// the same event. ONLY the blocking hook contributes to the aggregate.
    ///
    /// The async hook is given the HIGHER priority so it is evaluated FIRST
    /// (`match_event` sorts priority-descending). Even though it would exit 2 —
    /// a `Block` on the synchronous path — it is backgrounded and excluded, so
    /// the aggregate's eventual `Block` comes solely from the lower-priority
    /// blocking hook. Both share one immediate runner: it is invoked exactly
    /// twice (once inline for the blocking hook, once in the background for the
    /// async hook), proving the async hook still runs — just not in the
    /// aggregate.
    #[tokio::test]
    async fn mixed_async_and_blocking_only_blocking_contributes() {
        let runtime = TestRuntime::new();
        let (tx, mut rx) = mpsc::channel(4);
        let async_reg = Arc::new(AsyncHookRegistry::new(runtime, tx));

        // exit 2 on BOTH: if the async hook could contribute, the aggregate
        // would still Block — but with the WRONG reason. We assert the reason is
        // the blocking hook's, proving the async exit-2 was never folded in.
        let runner = CountingRunner::new(out("", "blocking-reason", 2));

        // Async hook: HIGHER priority ⇒ evaluated first ⇒ backgrounded, excluded.
        let async_hook = command_hook_with_priority(false, 100);
        let async_id = async_hook.id;
        // Blocking hook: LOWER priority ⇒ evaluated second ⇒ its Block is the
        // aggregate's only decision.
        let blocking_hook = command_hook_with_priority(true, 0);
        let blocking_id = blocking_hook.id;

        let mut registry = HookRegistry::new();
        registry.register(async_hook);
        registry.register(blocking_hook);
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner.clone(), Arc::new(StubSandbox))
        .with_async_registry(async_reg.clone());

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        // The aggregate Block comes ONLY from the blocking hook.
        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(
            agg.reason.as_deref(),
            Some("blocking-reason"),
            "the Block reason must be the blocking hook's, not the backgrounded one's",
        );
        assert_eq!(
            agg.all_results.len(),
            1,
            "only the blocking hook is recorded in the aggregate",
        );
        assert_eq!(
            agg.all_results[0].0, blocking_id,
            "the single aggregate result is the blocking hook's",
        );

        // The async hook still RAN (fire-and-forget) — its completion lands on
        // the channel keyed by its own id, and it was never in the aggregate.
        let (got_id, _got) = rx.recv().await.expect("async hook completion publishes");
        assert_eq!(
            got_id, async_id,
            "the backgrounded completion is the async hook's, separate from the aggregate",
        );

        // Both hooks executed exactly once (inline blocking + background async).
        assert_eq!(
            runner.runs.load(Ordering::SeqCst),
            2,
            "both the blocking (inline) and async (background) hooks ran",
        );
    }
}

#[cfg(test)]
mod once_and_status_message_tests {
    //! RUNTIME wiring for the additive `once` / `status_message` hook fields.
    //!
    //! * `once: true` — a hook is dropped from the registry after it runs with
    //!   a *success* outcome (claude-code `registerSkillHooks.ts:35-36` +
    //!   `utils/hooks.ts:2918-2919`): the second dispatch finds nothing to run.
    //!   A `once` hook whose first run ERRORS is left in place — TS guards
    //!   `onHookSuccess` behind `result.outcome === 'success'`.
    //! * `status_message` — threaded onto the per-hook `hook_progress` event the
    //!   executor emits before each hook (claude-code `utils/hooks.ts:2094-2116`).
    use super::*;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use protocol::{HookId, ToolUseId};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// `HttpTransport` stub — never exercised by these Builtin-arm tests.
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    /// `RuntimeSpawner` stub — backgrounding is never exercised here.
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _duration: Duration) {}
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// A builtin handler that counts invocations and returns a fixed outcome,
    /// so a test can prove a `once` hook ran exactly N times.
    struct CountingBuiltin {
        id: String,
        runs: Arc<AtomicU32>,
        outcome: HookOutcome,
    }
    #[async_trait]
    impl BuiltinHookHandler for CountingBuiltin {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let exit_code = i32::from(!matches!(self.outcome, HookOutcome::Success));
            HookResult {
                outcome: self.outcome,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(exit_code),
                response: None,
            }
        }
        fn id(&self) -> &str {
            &self.id
        }
    }

    fn pre_event() -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({"command": "ls"}),
            tool_use_id: ToolUseId::new(),
        }
    }

    /// A Builtin hook subscribed to `PreToolUse`, with the supplied `once` flag,
    /// `status_message`, and handler id.
    fn builtin_hook(
        handler_id: &str,
        once: bool,
        status_message: Option<&str>,
    ) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "once-hook".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: handler_id.into(),
            },
            source: HookSource::Skill,
            blocking: true,
            timeout: None,
            priority: 0,
            once,
            status_message: status_message.map(Into::into),
        }
    }

    fn executor_with(
        hook: HookDefinition,
        handler: Arc<dyn BuiltinHookHandler>,
    ) -> (HookExecutorImpl, Arc<RwLock<HookRegistry>>) {
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let reg = Arc::new(RwLock::new(registry));
        let mut exec =
            HookExecutorImpl::new(reg.clone(), Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(handler);
        (exec, reg)
    }

    /// A `once: true` hook fires on the first dispatch and is then removed from
    /// the registry, so a second dispatch runs nothing.
    #[tokio::test]
    async fn once_hook_fires_once_then_is_gone() {
        let runs = Arc::new(AtomicU32::new(0));
        let handler = Arc::new(CountingBuiltin {
            id: "once-success".into(),
            runs: runs.clone(),
            outcome: HookOutcome::Success,
        });
        let (exec, reg) = executor_with(builtin_hook("once-success", true, None), handler);

        // First dispatch: the hook runs and its result is in the aggregate.
        let agg1 = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(agg1.all_results.len(), 1, "first dispatch runs the hook");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // It was removed from the registry after the successful run.
        assert!(
            reg.read().await.all_hooks().is_empty(),
            "a once hook is dropped from the registry after success",
        );

        // Second dispatch: nothing matches, nothing runs.
        let agg2 = exec.execute(pre_event(), HookContext::default()).await;
        assert!(
            agg2.all_results.is_empty(),
            "the removed once hook does not fire a second time",
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the handler ran exactly once across both dispatches",
        );
    }

    /// A `once: true` hook whose run ERRORS is NOT removed — it stays in the
    /// registry and runs again on the next dispatch (TS guards removal behind a
    /// `success` outcome).
    #[tokio::test]
    async fn once_hook_that_errors_is_not_removed() {
        let runs = Arc::new(AtomicU32::new(0));
        let handler = Arc::new(CountingBuiltin {
            id: "once-error".into(),
            runs: runs.clone(),
            outcome: HookOutcome::Error,
        });
        let (exec, reg) = executor_with(builtin_hook("once-error", true, None), handler);

        let _ = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(
            reg.read().await.all_hooks().len(),
            1,
            "an erroring once hook is left in the registry",
        );

        // Second dispatch still finds and runs the hook.
        let _ = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "the un-removed once hook fires again after an error",
        );
    }

    /// A `PostToolUse` Builtin hook returning `updated_mcp_tool_output` has it
    /// folded into the aggregate's `updated_mcp_tool_output` by `merge`.
    #[tokio::test]
    async fn post_hook_updated_mcp_tool_output_reaches_aggregate() {
        struct RewriteBuiltin;
        #[async_trait]
        impl BuiltinHookHandler for RewriteBuiltin {
            fn id(&self) -> &str {
                "rewrite"
            }
            async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    response: Some(HookResponse {
                        updated_mcp_tool_output: Some(
                            serde_json::json!({ "content": "rewritten" }),
                        ),
                        ..Default::default()
                    }),
                }
            }
        }
        let hook = HookDefinition {
            id: HookId::new(),
            name: "rewrite".into(),
            events: vec![HookEventType::PostToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "rewrite".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let (exec, _reg) = executor_with(hook, Arc::new(RewriteBuiltin));
        let post = HookEvent::PostToolUse {
            tool_name: "mcp__srv__tool".into(),
            tool_input: serde_json::json!({}),
            tool_output: serde_json::json!({ "content": "original" }),
            tool_use_id: ToolUseId::new(),
        };
        let agg = exec.execute(post, HookContext::default()).await;
        assert_eq!(
            agg.updated_mcp_tool_output,
            Some(serde_json::json!({ "content": "rewritten" })),
            "the hook's updatedMCPToolOutput must reach the aggregate",
        );
    }

    /// A `PostToolUse` hook that does NOT set `updated_mcp_tool_output` leaves
    /// the aggregate's field `None` (strict no-op).
    #[tokio::test]
    async fn post_hook_without_mutation_leaves_aggregate_none() {
        let runs = Arc::new(AtomicU32::new(0));
        let handler = Arc::new(CountingBuiltin {
            id: "observe".into(),
            runs: runs.clone(),
            outcome: HookOutcome::Success,
        });
        let hook = HookDefinition {
            id: HookId::new(),
            name: "observe".into(),
            events: vec![HookEventType::PostToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "observe".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let (exec, _reg) = executor_with(hook, handler);
        let post = HookEvent::PostToolUse {
            tool_name: "mcp__srv__tool".into(),
            tool_input: serde_json::json!({}),
            tool_output: serde_json::json!({ "content": "original" }),
            tool_use_id: ToolUseId::new(),
        };
        let agg = exec.execute(post, HookContext::default()).await;
        assert!(
            agg.updated_mcp_tool_output.is_none(),
            "no mutating hook → the aggregate field stays None",
        );
    }

    /// The per-hook `status_message` is carried onto the emitted `hook_progress`
    /// event; a hook with no `status_message` carries `None`.
    #[tokio::test]
    async fn status_message_is_carried_on_progress_event() {
        let runs = Arc::new(AtomicU32::new(0));
        let handler = Arc::new(CountingBuiltin {
            id: "with-status".into(),
            runs,
            outcome: HookOutcome::Success,
        });
        let (exec, _reg) = executor_with(
            builtin_hook("with-status", false, Some("Formatting\u{2026}")),
            handler,
        );

        let agg = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(agg.progress.len(), 1, "one progress event per matching hook");
        let p = &agg.progress[0];
        assert_eq!(p.hook_event, "PreToolUse");
        assert_eq!(p.hook_name, "once-hook");
        assert_eq!(
            p.status_message.as_deref(),
            Some("Formatting\u{2026}"),
            "the per-hook status_message threads onto the progress event",
        );
    }

    /// A hook with no `status_message` yields a progress event whose
    /// `status_message` is `None` (the spinner falls back to the generic line).
    #[tokio::test]
    async fn absent_status_message_is_none_on_progress_event() {
        let runs = Arc::new(AtomicU32::new(0));
        let handler = Arc::new(CountingBuiltin {
            id: "no-status".into(),
            runs,
            outcome: HookOutcome::Success,
        });
        let (exec, _reg) = executor_with(builtin_hook("no-status", false, None), handler);

        let agg = exec.execute(pre_event(), HookContext::default()).await;
        assert_eq!(agg.progress.len(), 1);
        assert_eq!(agg.progress[0].status_message, None);
    }
}

// ============================================================================
// Http / Agent arm dispatch routing through the full `execute` path.
//
// The loader now emits `HookExecutor::Http` / `HookExecutor::Agent`
// definitions (loader.rs). These tests prove the executor's `dispatch` routes
// each variant to its dedicated runner — the HTTP arm reaches the injected
// `HttpTransport` and the Agent arm reaches the injected `SubagentSpawner` —
// so a settings-declared http/agent hook is actually executed end-to-end.
// ============================================================================
#[cfg(test)]
mod http_agent_dispatch_tests {
    use super::*;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use crate::response::HookDecision;
    use protocol::{HookId, HttpResponse, ToolUseId};
    use serde_json::json;
    use std::sync::Mutex;
    use traits::budget::{BudgetEnforcerHandle, BudgetError};
    use traits::subagent_spawn::{
        SubagentInheritance, SubagentResult, SubagentSpawnError, SubagentSpawnRequest,
        SubagentUsage,
    };
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

    /// `RuntimeSpawner` stub — never exercised by these tests.
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _duration: Duration) {}
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// `HttpTransport` mock that records each request and returns a canned body
    /// — so a test can prove the HTTP arm reached it with the right URL.
    struct RecordingHttp {
        recorded: Mutex<Vec<protocol::HttpRequest>>,
        body: String,
        status: u16,
    }
    #[async_trait]
    impl HttpTransport for RecordingHttp {
        async fn request(
            &self,
            req: protocol::HttpRequest,
        ) -> Result<HttpResponse, traits::HttpError> {
            self.recorded.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: self.status,
                headers: Vec::new(),
                body: self.body.clone(),
            })
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    fn pre_event() -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: ToolUseId::new(),
        }
    }

    /// An `Http` hook subscribed to `PreToolUse` pointing at `url`.
    fn http_hook(url: &str) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: url.into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Http {
                url: url.into(),
                method: "POST".into(),
                headers: HashMap::new(),
                timeout: Duration::from_secs(5),
            },
            source: HookSource::Project,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[tokio::test]
    async fn http_hook_definition_dispatches_to_http_executor() {
        let http = Arc::new(RecordingHttp {
            recorded: Mutex::new(Vec::new()),
            status: 200,
            body:
                r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#
                    .into(),
        });
        let mut registry = HookRegistry::new();
        registry.register(http_hook("https://hooks.example.com/pre"));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            http.clone(),
            Arc::new(UnusedRuntime),
        );

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        // The HTTP arm reached the transport with the hook's URL.
        let recorded = http.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "the Http arm must reach the transport");
        assert_eq!(recorded[0].url, "https://hooks.example.com/pre");
        drop(recorded);
        // The parsed allow response surfaces as an Approve decision.
        assert_eq!(agg.decision, Some(HookDecision::Approve));
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success));
    }

    // ---- agent arm ---------------------------------------------------------

    struct InertInvoker;
    #[async_trait]
    impl ToolInvoker for InertInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            Err(ToolInvokerError::Internal("inert".into()))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    struct InertBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for InertBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }
    fn dummy_inherit() -> SubagentInheritance {
        SubagentInheritance {
            tool_invoker: Arc::new(InertInvoker),
            budget: Arc::new(InertBudget),
        }
    }

    /// `HttpTransport` stub — never exercised by the agent test.
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    /// `SubagentSpawner` mock that records the request and returns a canned
    /// terminal result — so a test can prove the Agent arm reached it with the
    /// hook's `agent_type` + spliced prompt.
    struct RecordingSpawner {
        recorded: Mutex<Vec<SubagentSpawnRequest>>,
        result: Mutex<Option<Result<SubagentResult, SubagentSpawnError>>>,
    }
    #[async_trait]
    impl SubagentSpawner for RecordingSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            self.recorded.lock().unwrap().push(request);
            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(SubagentSpawnError::Internal("no script".into())))
        }
    }

    /// An `Agent` hook subscribed to `PreToolUse`.
    fn agent_hook() -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "agent".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Agent {
                agent_type: "general-purpose".into(),
                prompt: "vet this".into(),
            },
            source: HookSource::Project,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[tokio::test]
    async fn agent_hook_definition_dispatches_to_agent_executor() {
        let spawner = Arc::new(RecordingSpawner {
            recorded: Mutex::new(Vec::new()),
            result: Mutex::new(Some(Ok(SubagentResult::Completed {
                content: json!(r#"{"decision":"approve"}"#),
                usage: SubagentUsage::default(),
            }))),
        });
        let mut registry = HookRegistry::new();
        registry.register(agent_hook());
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        )
        .with_agent_spawner(spawner.clone());

        // The Agent arm needs the inheritance bundle on the context.
        let ctx = HookContext {
            inherit: Some(dummy_inherit()),
            ..Default::default()
        };
        let agg = exec.execute(pre_event(), ctx).await;

        // The Agent arm reached the spawner with the hook's agent_type and a
        // prompt that spliced the template + the serialized payload.
        let recorded = spawner.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "the Agent arm must reach the spawner");
        assert_eq!(recorded[0].subagent_type, "general-purpose");
        assert!(
            recorded[0].prompt.starts_with("vet this"),
            "prompt template is spliced ahead of the payload",
        );
        assert!(
            recorded[0].prompt.contains(r#""hook_event_name":"PreToolUse""#),
            "the serialized event payload is appended to the prompt",
        );
        drop(recorded);
        // The subagent's approve content surfaces as an Approve decision.
        assert_eq!(agg.decision, Some(HookDecision::Approve));
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success));
    }
}

// ============================================================================
// PROMPT-ARM DISPATCH TESTS
//
// Prove the executor's `dispatch` routes a `HookExecutor::Prompt` definition
// to the injected `HookPromptRunner` with the `$ARGUMENTS`-substituted prompt,
// and that the runner's `{ok:false}` verdict surfaces as a Block decision on
// the aggregate. Also proves the no-runner path is a strict no-op.
// ============================================================================
#[cfg(test)]
mod prompt_dispatch_tests {
    use super::*;
    use crate::definition::{HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use crate::prompt_executor::{HookPromptRunner, PromptHookError, PromptHookRequest};
    use crate::response::HookDecision;
    use protocol::{HookId, ToolUseId};
    use serde_json::json;
    use std::sync::Mutex;

    /// `RuntimeSpawner` stub — never exercised by these tests.
    struct UnusedRuntime;
    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<traits::BackgroundTaskHandle, traits::RuntimeError> {
            Err(traits::RuntimeError::Internal("unused".into()))
        }
        async fn sleep(&self, _duration: Duration) {}
        async fn cancel(
            &self,
            _handle: &traits::BackgroundTaskHandle,
        ) -> Result<(), traits::RuntimeError> {
            Ok(())
        }
    }

    /// `HttpTransport` stub — never exercised by these tests.
    struct UnusedHttp;
    #[async_trait]
    impl HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<traits::http::SseStream, traits::HttpError> {
            Err(traits::HttpError::InvalidRequest("unused".into()))
        }
    }

    /// `HookPromptRunner` mock that records the request and returns a canned
    /// body — so a test can prove the Prompt arm reached it with the
    /// `$ARGUMENTS`-substituted prompt.
    struct RecordingRunner {
        recorded: Mutex<Vec<PromptHookRequest>>,
        result: Mutex<Option<Result<String, PromptHookError>>>,
    }
    #[async_trait]
    impl HookPromptRunner for RecordingRunner {
        async fn run(&self, req: PromptHookRequest) -> Result<String, PromptHookError> {
            self.recorded.lock().unwrap().push(req);
            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Err(PromptHookError::Query("no script".into())))
        }
    }

    fn pre_event() -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: json!({"command": "rm -rf /"}),
            tool_use_id: ToolUseId::new(),
        }
    }

    fn prompt_hook() -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "prompt".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Prompt {
                prompt: "Is this safe? $ARGUMENTS".into(),
                model: Some("claude-sonnet-4-6".into()),
            },
            source: HookSource::Project,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[tokio::test]
    async fn prompt_hook_definition_dispatches_to_prompt_runner() {
        let runner = Arc::new(RecordingRunner {
            recorded: Mutex::new(Vec::new()),
            result: Mutex::new(Some(Ok(r#"{"ok": false, "reason": "destructive"}"#.into()))),
        });
        let mut registry = HookRegistry::new();
        registry.register(prompt_hook());
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        )
        .with_prompt_runner(runner.clone());

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        // The Prompt arm reached the runner with the substituted prompt + the
        // serialized event payload + the model override.
        let recorded = runner.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "the Prompt arm must reach the runner");
        assert!(
            recorded[0].prompt.starts_with("Is this safe? "),
            "prompt template precedes the payload: {:?}",
            recorded[0].prompt,
        );
        assert!(
            recorded[0].prompt.contains(r#""hook_event_name":"PreToolUse""#),
            "the serialized event payload is spliced into $ARGUMENTS",
        );
        assert_eq!(recorded[0].model.as_deref(), Some("claude-sonnet-4-6"));
        drop(recorded);
        // The runner's `{ok:false}` verdict surfaces as a Block decision.
        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(
            agg.reason.as_deref(),
            Some("Prompt hook condition was not met: destructive")
        );
        assert!(agg.prevent_continuation);
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success));
    }

    #[tokio::test]
    async fn prompt_hook_ok_true_does_not_block() {
        let runner = Arc::new(RecordingRunner {
            recorded: Mutex::new(Vec::new()),
            result: Mutex::new(Some(Ok(r#"{"ok": true}"#.into()))),
        });
        let mut registry = HookRegistry::new();
        registry.register(prompt_hook());
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        )
        .with_prompt_runner(runner.clone());

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(agg.decision, None, "condition met must not block");
        assert!(!agg.prevent_continuation);
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Success));
    }

    #[tokio::test]
    async fn prompt_hook_without_runner_is_strict_noop() {
        let mut registry = HookRegistry::new();
        registry.register(prompt_hook());
        // No `.with_prompt_runner(..)` — the Prompt arm must be a no-op.
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        );

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        // No decision: a no-runner prompt hook can NEVER block.
        assert_eq!(agg.decision, None);
        assert!(!agg.prevent_continuation);
        let (_, r) = &agg.all_results[0];
        assert!(matches!(r.outcome, HookOutcome::Error));
        assert!(r.stderr.contains("prompt executor not wired"));
    }
}
