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
    parse_response, HookEventNamePost, HookEventNamePre, HookEventNameSessionStart,
    HookEventNameStop, HookEventNameStopFailure, HookEventNameSubagentStop,
    HookEventNameTaskCompleted, HookEventNameUserPromptSubmit, PostToolUsePayload,
    PreToolUsePayload, SessionStartPayload, StopFailurePayload, StopPayload, SubagentStopPayload,
    TaskCompletedPayload, UserPromptSubmitPayload,
};
use crate::http_executor::{HttpExecutionSignal, HttpExecutor};
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
            process: self.process.clone(),
            sandbox: self.sandbox.clone(),
        }
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
        for hook in &matched {
            if hook.blocking {
                // Synchronous path — unchanged from M5-06.
                let result = self.dispatcher().dispatch(hook, &event, &ctx).await;
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
/// `TaskCompleted` / `UserPromptSubmit` / `SessionStart` / `StopFailure`).
///
/// Where a [`HookEvent`] variant carries fewer fields than the claude-code wire
/// schema, the available fields are populated and the rest defaulted
/// (`false` / `None` / empty string) — filled by later B-cluster batches.
/// Every other (not-yet-ported) variant returns `None`.
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
        HookEvent::TaskCompleted { task_id, .. } => {
            let payload = TaskCompletedPayload {
                hook_event_name: HookEventNameTaskCompleted,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                task_id: task_id.clone(),
                task_subject: String::new(),
                task_description: None,
                teammate_name: None,
                team_name: None,
            };
            Some(("TaskCompleted", serde_json::to_string(&payload).ok()?))
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
                status: "success".into(),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"TaskCompleted""#));
        assert!(stdin.contains(r#""task_id":"task-99""#));
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

    #[test]
    fn unsupported_event_still_returns_none() {
        // `SessionEnd` has no ported wire schema yet — must fall through.
        let ev = HookEvent::SessionEnd {
            session_id: protocol::SessionId::nil(),
            reason: "user_exit".into(),
        };
        assert!(build_envelope_body(&ev, &HookContext::default()).is_none());
    }

    #[test]
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
                    status: "success".into(),
                },
                "TaskCompleted",
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
            priority: 0,
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
}
