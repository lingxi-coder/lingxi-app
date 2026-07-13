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
    parse_response, ConfigChangePayload, CwdChangedPayload, ElicitationPayload,
    ElicitationResultPayload, FileChangedPayload, HookEventNameConfigChange,
    HookEventNameCwdChanged, HookEventNameElicitation, HookEventNameElicitationResult,
    HookEventNameFileChanged, HookEventNameInstructionsLoaded, HookEventNameMessageDisplay,
    HookEventNameNotification, HookEventNamePermissionDenied, HookEventNamePermissionRequest,
    HookEventNamePost, HookEventNamePostCompact, HookEventNamePostToolBatch,
    HookEventNamePostToolUseFailure, HookEventNamePre, HookEventNamePreCompact,
    HookEventNameSessionEnd, HookEventNameSessionStart, HookEventNameSetup, HookEventNameStop,
    HookEventNameStopFailure, HookEventNameSubagentStart, HookEventNameSubagentStop,
    HookEventNameTaskCompleted, HookEventNameTaskCreated, HookEventNameTeammateIdle,
    HookEventNameUserPromptExpansion, HookEventNameUserPromptSubmit, HookEventNameWorktreeCreate,
    HookEventNameWorktreeRemove, InstructionsLoadedPayload, MessageDisplayPayload,
    NotificationPayload, PermissionDeniedPayload, PermissionRequestPayload, PostCompactPayload,
    PostToolBatchPayload, PostToolUseFailurePayload, PostToolUsePayload, PreCompactPayload,
    PreToolUsePayload, SessionEndPayload, SessionStartPayload, SetupPayload, StopFailurePayload,
    StopPayload, SubagentStartPayload, SubagentStopPayload, TaskCompletedPayload,
    TaskCreatedPayload, TeammateIdlePayload, UserPromptExpansionPayload, UserPromptSubmitPayload,
    WorktreeCreatePayload, WorktreeRemovePayload,
};
use crate::http_executor::{HttpExecutionSignal, HttpExecutor};
use crate::prompt_executor::{
    HookPromptRunner, PromptExecutionSignal, PromptExecutor, HOOK_PROMPT_TIMEOUT_MS,
};
use crate::registry::{HookContext, HookRegistry};
use crate::response::{AggregateHookResult, HookDecision, HookOutcome, HookResponse, HookResult};
use crate::ssrf_guard::SsrfGuard;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use traits::subagent_spawn::SubagentSpawner;
use traits::{
    HttpTransport, OutputStream, ProcessCommand, ProcessError, ProcessRunner, RuntimeSpawner,
    Sandbox,
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

/// Process-env override for the `SessionEnd` hook *batch* shutdown deadline
/// (claude-code `Wqt`, BIN off 205715763:
/// `process.env.LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS`). When this parses as a
/// finite integer `> 0` it is used verbatim (NOT clamped — the binary returns it
/// directly, bypassing the floor/cap).
pub const SESSION_END_HOOKS_TIMEOUT_ENV: &str = "LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS";

/// Floor for the computed `SessionEnd` batch deadline (claude-code `nzn`, BIN off
/// 205765355: `nzn=1500`, exported as `SESSION_END_HOOK_TIMEOUT_MS_DEFAULT`).
/// `Wqt` returns `Math.max(nzn, Math.min(n, rym))`, so the batch deadline is
/// never shorter than this when the env override is absent. Also doubles as the
/// per-hook default timeout the `SessionEnd` runner passes (`lje` →
/// `cH({…, timeoutMs: nzn})`, BIN off 205706285).
pub const SESSION_END_HOOK_TIMEOUT_FLOOR_MS: u64 = 1_500;

/// Cap for the computed `SessionEnd` batch deadline (claude-code `rym`, BIN off
/// 205765364: `rym=60000`). `Wqt` returns `Math.max(nzn, Math.min(n, rym))`, so
/// the computed deadline is never longer than this when the env override is
/// absent.
pub const SESSION_END_HOOK_TIMEOUT_CAP_MS: u64 = 60_000;

/// Compute the `SessionEnd` hook *batch* shutdown deadline in milliseconds, faithful
/// to claude-code `Wqt` (BIN off 205715763):
///
/// ```text
/// function Wqt(){
///   let e=process.env.LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS,
///       t=e?parseInt(e,10):NaN;
///   if(Number.isFinite(t)&&t>0)return t;            // env override wins, UNCLAMPED
///   let n=0,r=uE()?[]:Kj()?.SessionEnd??[],
///       o=[...f5()?.SessionEnd??[],...r];
///   for(let s of o)for(let i of s.hooks)
///     if(i.timeout&&i.timeout*1000>n)n=i.timeout*1000; // max per-hook timeout (s→ms)
///   return Math.max(nzn,Math.min(n,rym))             // clamp to [1500, 60000]
/// }
/// ```
///
/// * `env_value` is the raw `LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS` string (or
///   `None`). A value that parses (base-10, leading-digit, JS `parseInt`-style)
///   to a finite `> 0` integer is returned VERBATIM — the env override bypasses
///   the floor/cap, exactly as the binary does.
/// * Otherwise the deadline is `max(FLOOR, min(max_per_hook_ms, CAP))`, where
///   `max_per_hook_ms` is the largest declared per-hook timeout across the
///   matched `SessionEnd` hooks (each hook's `Duration` in ms; an absent / zero
///   timeout contributes `0`). With no per-hook timeouts this collapses to the
///   1500 ms floor.
#[must_use]
pub fn session_end_batch_timeout_ms(env_value: Option<&str>, max_per_hook_ms: u64) -> u64 {
    if let Some(raw) = env_value {
        if let Some(parsed) = parse_int_base10(raw) {
            // `Number.isFinite(t) && t > 0` — `parseInt` of a non-numeric prefix
            // yields NaN (→ `None` here, falls through); a non-positive value is
            // rejected and also falls through to the computed clamp.
            if parsed > 0 {
                // u64 already bounds finiteness; a positive value is returned
                // unclamped, mirroring `return t`.
                #[allow(clippy::cast_sign_loss)]
                return parsed as u64;
            }
        }
    }
    SESSION_END_HOOK_TIMEOUT_FLOOR_MS.max(max_per_hook_ms.min(SESSION_END_HOOK_TIMEOUT_CAP_MS))
}

/// Largest declared per-hook timeout (in milliseconds) across a matched hook set
/// — the `n` accumulator in `Wqt` (`if(i.timeout&&i.timeout*1000>n)n=i.timeout*1000`).
/// A hook with no timeout (`None`) or a zero timeout contributes `0`.
fn max_per_hook_timeout_ms(hooks: &[HookDefinition]) -> u64 {
    hooks
        .iter()
        .filter_map(|h| h.timeout)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .max()
        .unwrap_or(0)
}

/// JS `parseInt(s, 10)`-style parse: skip leading ASCII whitespace, accept an
/// optional sign, then consume the leading run of decimal digits and ignore any
/// trailing non-digit suffix (so `"3000abc"` → `3000`, `"abc"` → `NaN`/`None`).
/// Returns `None` for no leading digits (the `NaN` case) so the caller falls
/// through to the computed clamp.
fn parse_int_base10(s: &str) -> Option<i64> {
    let bytes = s.trim_start().as_bytes();
    let mut idx = 0;
    let mut negative = false;
    if let Some(&first) = bytes.first() {
        if first == b'+' || first == b'-' {
            negative = first == b'-';
            idx = 1;
        }
    }
    let digits_start = idx;
    let mut value: i64 = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add(i64::from(bytes[idx] - b'0'));
        idx += 1;
    }
    if idx == digits_start {
        // No digits consumed → `parseInt` returns NaN.
        return None;
    }
    Some(if negative { -value } else { value })
}

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
    /// #41: runner-head gate (`h$`, BIN off 195521423 =
    /// `Hn("policySettings")?.disableAllHooks===!0`). When `true`, EVERY
    /// `execute*` path skips all matched hooks and logs
    /// `Skipping hooks for {event[:matcher]} due to 'disableAllHooks' managed
    /// setting` — mirroring the binary's runner head `if(h$())return …,[]` ahead
    /// of the workspace-trust gate. Sourced from resolved managed/policy settings
    /// at the engine composition root and attached via
    /// [`Self::with_policy_disable_all_hooks`]; defaults `false` (the no-managed
    /// -policy path) so it is behavior-neutral until a policy is wired.
    policy_disable_all_hooks: bool,
    /// Optional output stream observer for `--include-hook-events` frames.
    ///
    /// When set, `execute` and `execute_session_end` emit
    /// `hook_started` / `hook_response` frames through this sink BEFORE and
    /// AFTER each blocking hook dispatches. Non-blocking (background) hooks
    /// are excluded — their completion is asynchronous and cannot be paired
    /// with a reliable "before" frame. Attached via
    /// [`Self::with_hook_observer`]; `None` by default (no-op, zero cost).
    hook_observer: Option<Arc<dyn OutputStream>>,
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
            policy_disable_all_hooks: false,
            hook_observer: None,
        }
    }

    /// Attach the #41 runner-head gate (`h$`). When `disable_all_hooks` is
    /// `true`, every `execute*` path skips all matched hooks and logs the
    /// `'disableAllHooks' managed setting` skip line — mirroring the binary's
    /// `if(h$())return …,[]` runner head. Sourced at the composition root from
    /// the resolved `policySettings.disableAllHooks` managed setting (see
    /// [`crate::loader::HookPolicyGate`]); the default (`false`) is the
    /// no-managed-policy path and is behavior-neutral.
    #[must_use]
    pub fn with_policy_disable_all_hooks(mut self, disable_all_hooks: bool) -> Self {
        self.policy_disable_all_hooks = disable_all_hooks;
        self
    }

    /// Attach an output stream as a hook lifecycle observer. When set,
    /// `execute` and `execute_session_end` call
    /// [`OutputStream::emit_hook_started`] before each blocking hook and
    /// [`OutputStream::emit_hook_response`] after it completes. Non-blocking
    /// (background) hooks are excluded. Used by `--include-hook-events` in the
    /// stream-json CLI path; default `None` (behavior-neutral).
    #[must_use]
    pub fn with_hook_observer(mut self, observer: Arc<dyn OutputStream>) -> Self {
        self.hook_observer = Some(observer);
        self
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

    /// Register an agent's frontmatter hooks scoped to `agent_id` (G4 — claude
    /// `registerFrontmatterHooks(setAppState, agentId, hooks, …, isAgent)`,
    /// runAgent.ts:557-575). Writes through the shared registry so the hooks fire
    /// for the lifetime of the running subagent; pass `is_agent = true` to
    /// retarget `Stop` subscriptions to `SubagentStop`. Pair with
    /// [`Self::clear_agent_hooks`] in the runner's terminal/cleanup path
    /// (claude's `clearSessionHooks` in the `runAgent` finally).
    pub async fn register_agent_hooks(
        &self,
        agent_id: protocol::AgentId,
        hooks: &[HookDefinition],
        is_agent: bool,
    ) {
        self.registry
            .write()
            .await
            .register_agent_hooks(agent_id, hooks, is_agent);
    }

    /// Remove every frontmatter hook scoped to `agent_id` (G4 — claude
    /// `clearSessionHooks(rootSetAppState, agentId)`, runAgent.ts finally).
    /// Returns the number of hooks dropped.
    pub async fn clear_agent_hooks(&self, agent_id: protocol::AgentId) -> usize {
        self.registry.write().await.clear_agent_hooks(agent_id)
    }

    /// Fire the `SessionEnd` hook batch against a *batch-wide shutdown deadline*
    /// (claude-code `lje` → `cH({…, signal: AbortSignal.timeout(Wqt())})`,
    /// BIN off 205706285 / 205715763).
    ///
    /// Distinct from [`Self::execute`]: at session teardown claude-code does not
    /// give the `SessionEnd` batch the generic 10-minute per-hook budget — it caps
    /// the WHOLE batch with a single deadline so a slow / hung `SessionEnd` hook
    /// cannot stall process exit. The deadline is
    /// [`session_end_batch_timeout_ms`] (env override, else
    /// `max(1500, min(max_per_hook_ms, 60000))`).
    ///
    /// The deadline is applied as a wall-clock budget shared across the batch:
    /// each matched hook is dispatched under `min(remaining_deadline, its own
    /// per-hook timeout)`, mirroring the binary, where every hook's effective
    /// signal is `xP(batchSignal, {timeoutMs: perHook})` — abort on EITHER the
    /// batch deadline OR the per-hook timeout (BIN off 205755512 / 200906242).
    /// Once the batch deadline elapses, the remaining hooks see an already-fired
    /// deadline and are skipped (the binary's `if(r?.aborted)return[]` / empty
    /// per-hook output), so a hung early hook cannot starve teardown.
    ///
    /// Like [`Self::execute`]: priority-descending order, every matched hook
    /// dispatched (no first-`Block` break — #45(b); `Block` is folded sticky),
    /// `once` removal, B5 non-blocking backgrounding (those are not awaited so
    /// they never consume the deadline), and a strict no-op (default aggregate)
    /// when no `SessionEnd` hook is registered. The batch still stops early when
    /// the shared shutdown deadline elapses.
    pub async fn execute_session_end(
        &self,
        event: HookEvent,
        ctx: HookContext,
    ) -> AggregateHookResult {
        // #41 runner-head gate (`h$`): `policySettings.disableAllHooks` skips the
        // SessionEnd batch entirely (the binary's `cH` head runs before the
        // SessionEnd deadline race too).
        if let Some(skipped) = self.policy_disable_gate(&event) {
            return skipped;
        }
        let reg = self.registry.read().await;
        let matched: Vec<HookDefinition> =
            reg.match_event(&event, &ctx).into_iter().cloned().collect();
        drop(reg);

        // Compute the batch deadline from the env override / the max declared
        // per-hook timeout across the matched set (claude-code `Wqt`).
        let env_value = std::env::var(SESSION_END_HOOKS_TIMEOUT_ENV).ok();
        let batch_timeout_ms =
            session_end_batch_timeout_ms(env_value.as_deref(), max_per_hook_timeout_ms(&matched));
        let deadline = tokio::time::Instant::now() + Duration::from_millis(batch_timeout_ms);

        let mut agg = AggregateHookResult::default();
        let hook_event = format!("{:?}", event.event_type());
        for hook in &matched {
            agg.progress.push(crate::events::HookProgressEvent {
                hook_event: hook_event.clone(),
                hook_name: hook.name.clone(),
                status_message: hook.status_message.clone(),
            });
            if hook.blocking {
                // Emit hook_started BEFORE dispatch (for --include-hook-events).
                if let Some(observer) = &self.hook_observer {
                    observer
                        .emit_hook_started(&hook.id.to_string(), &hook.name, &hook_event)
                        .await;
                }
                // Bound this hook by the remaining batch budget. Once the batch
                // deadline has passed, `timeout_at` fires immediately, so the
                // remaining hooks are skipped (the binary's already-aborted
                // signal → empty output), and the batch stops — a hung early
                // hook cannot starve teardown.
                let Ok(result) = tokio::time::timeout_at(
                    deadline,
                    self.dispatcher().dispatch(hook, &event, &ctx),
                )
                .await
                else {
                    // Batch deadline elapsed mid-dispatch (or before this hook
                    // started). Record a timeout outcome and stop the batch —
                    // no later SessionEnd hook gets a turn, faithful to the
                    // aborted batch signal.
                    emit_session_end_batch_timeout(hook, batch_timeout_ms);
                    let timed_out = HookResult {
                        outcome: HookOutcome::Timeout,
                        stdout: String::new(),
                        stderr: format!(
                            "SessionEnd hook {} aborted: batch deadline ({batch_timeout_ms}ms) exceeded",
                            hook.id
                        ),
                        exit_code: None,
                        response: None,
                    };
                    // Emit timeout hook_response before merging.
                    if let Some(observer) = &self.hook_observer {
                        observer
                            .emit_hook_response(
                                &hook.id.to_string(),
                                &hook.name,
                                &hook_event,
                                "",
                                &timed_out.stdout,
                                &timed_out.stderr,
                                timed_out.exit_code,
                                "timeout",
                            )
                            .await;
                    }
                    Self::merge(&mut agg, hook, timed_out);
                    break;
                };
                // Emit hook_response AFTER dispatch (for --include-hook-events).
                if let Some(observer) = &self.hook_observer {
                    let outcome_str = match result.outcome {
                        HookOutcome::Success => "success",
                        HookOutcome::Error => "error",
                        HookOutcome::Cancelled => "error",
                        HookOutcome::Timeout => "timeout",
                    };
                    let resp_text = result
                        .response
                        .as_ref()
                        .and_then(|r| r.system_message.as_deref())
                        .unwrap_or("");
                    let combined_output = if resp_text.is_empty() {
                        result.stdout.clone()
                    } else if result.stdout.is_empty() {
                        resp_text.to_string()
                    } else {
                        format!("{}\n{}", result.stdout, resp_text)
                    };
                    observer
                        .emit_hook_response(
                            &hook.id.to_string(),
                            &hook.name,
                            &hook_event,
                            &combined_output,
                            &result.stdout,
                            &result.stderr,
                            result.exit_code,
                            outcome_str,
                        )
                        .await;
                }
                if hook.once && matches!(result.outcome, HookOutcome::Success) {
                    self.registry.write().await.remove_once_hook(hook.id);
                }
                Self::merge(&mut agg, hook, result);
                // #45(b): no early break on first `Block`. SessionEnd's decision is
                // a shutdown-path verdict that is never consumed for blocking, and
                // claude's `cH` runner runs every matched hook regardless; running
                // the rest of the batch (still bounded by the batch deadline above)
                // preserves later SessionEnd hooks' side effects. The `break` above
                // remains for the batch-deadline-elapsed case only.
            } else {
                // B5 non-blocking hooks are backgrounded (not awaited), so they
                // never consume the batch deadline — identical to `execute`.
                self.background_hook(hook, &event, &ctx).await;
            }
        }
        agg
    }

    /// Fire `event` and return the aggregated result of every matching hook.
    ///
    /// Hooks are evaluated in priority-descending order; EVERY matched hook is
    /// dispatched (no early break on the first `Block` — #45(b), mirroring
    /// claude-code's `cH` runner). The aggregate verdict is `Block` if ANY hook
    /// blocked (`merge` keeps `Block` sticky), so later hooks' side effects
    /// (systemMessage / additionalContext / updatedInput / `once`-removal) are
    /// preserved while the blocking decision is unchanged.
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
        // #41 runner-head gate (`h$`): `policySettings.disableAllHooks` skips
        // ALL hooks before any matching/dispatch.
        if let Some(skipped) = self.policy_disable_gate(&event) {
            return skipped;
        }
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
                // Emit hook_started BEFORE dispatch (for --include-hook-events).
                if let Some(observer) = &self.hook_observer {
                    observer
                        .emit_hook_started(&hook.id.to_string(), &hook.name, &hook_event)
                        .await;
                }
                // Synchronous path — unchanged from M5-06.
                let result = self.dispatcher().dispatch(hook, &event, &ctx).await;
                // Emit hook_response AFTER dispatch (for --include-hook-events).
                if let Some(observer) = &self.hook_observer {
                    let outcome_str = match result.outcome {
                        HookOutcome::Success => "success",
                        HookOutcome::Error => "error",
                        HookOutcome::Cancelled => "error",
                        HookOutcome::Timeout => "timeout",
                    };
                    // `output` = combined response text (stdout + any systemMessage).
                    let resp_text = result
                        .response
                        .as_ref()
                        .and_then(|r| r.system_message.as_deref())
                        .unwrap_or("");
                    let combined_output = if resp_text.is_empty() {
                        result.stdout.clone()
                    } else if result.stdout.is_empty() {
                        resp_text.to_string()
                    } else {
                        format!("{}\n{}", result.stdout, resp_text)
                    };
                    observer
                        .emit_hook_response(
                            &hook.id.to_string(),
                            &hook.name,
                            &hook_event,
                            &combined_output,
                            &result.stdout,
                            &result.stderr,
                            result.exit_code,
                            outcome_str,
                        )
                        .await;
                }
                // `once` runtime removal (claude-code `registerSkillHooks.ts:35-36`,
                // `utils/hooks.ts:2918-2919`): drop the hook from the registry
                // only after it runs with a *success* outcome, so it never fires
                // again. An erroring `once` hook is left in place.
                if hook.once && matches!(result.outcome, HookOutcome::Success) {
                    self.registry.write().await.remove_once_hook(hook.id);
                }
                Self::merge(&mut agg, hook, result);
                // #45(b): NO early break on the first `Block`. claude-code's `cH`
                // runner dispatches every matched hook (BIN off 205755512) and
                // folds `blocked = some(t.blocked)` afterwards, so later hooks'
                // systemMessage / additionalContext / updatedInput / `once`-removal
                // side effects must NOT be lost. `merge` makes `Block` sticky, so
                // the aggregate verdict is identical to the old short-circuit (any
                // block wins) while the side effects are now preserved.
            } else {
                // B5 config-`async` path: background the hook and continue. It
                // is excluded from `agg`, so it cannot block.
                self.background_hook(hook, &event, &ctx).await;
            }
        }
        agg
    }

    /// Fire `event` against ONLY the frontmatter hooks scoped to `agent_id`
    /// (source / plugin / other-agent buckets excluded).
    ///
    /// The child runner uses this to fire `SubagentStop` for its OWN frontmatter
    /// Stop→SubagentStop hooks (claude fires the subagent's stop hooks inside the
    /// child, runAgent.ts) at the loop's end, BEFORE [`Self::clear_agent_hooks`]
    /// removes them — otherwise those retargeted hooks are dead code (registered,
    /// then cleared before the orchestrator-side `SubagentStop` chokepoint runs).
    /// Scoping to this agent's bucket avoids double-firing session / plugin
    /// `SubagentStop` hooks, which the chokepoint already covers. A strict no-op
    /// (no matched hooks) when the agent registered none, so the no-frontmatter
    /// path is byte-identical to legacy.
    ///
    /// Mirrors [`Self::execute`]'s per-hook dispatch (progress beacon, `blocking`
    /// vs. background routing, `once` removal, no-early-`Block`-break with a
    /// sticky `Block` fold — #45(b)) so a frontmatter hook behaves identically
    /// whether it fires here or via the general path.
    pub async fn execute_agent_scoped(
        &self,
        event: HookEvent,
        ctx: HookContext,
        agent_id: protocol::AgentId,
    ) -> AggregateHookResult {
        // #41 runner-head gate (`h$`).
        if let Some(skipped) = self.policy_disable_gate(&event) {
            return skipped;
        }
        let reg = self.registry.read().await;
        let matched: Vec<HookDefinition> = reg
            .match_event_agent_scoped(&event, agent_id)
            .into_iter()
            .cloned()
            .collect();
        drop(reg);
        let mut agg = AggregateHookResult::default();
        let hook_event = format!("{:?}", event.event_type());
        for hook in &matched {
            agg.progress.push(crate::events::HookProgressEvent {
                hook_event: hook_event.clone(),
                hook_name: hook.name.clone(),
                status_message: hook.status_message.clone(),
            });
            if hook.blocking {
                let result = self.dispatcher().dispatch(hook, &event, &ctx).await;
                if hook.once && matches!(result.outcome, HookOutcome::Success) {
                    self.registry.write().await.remove_once_hook(hook.id);
                }
                Self::merge(&mut agg, hook, result);
                // #45(b): no early break on first `Block` — see `execute`. All
                // matched hooks dispatch; `merge` keeps `Block` sticky.
            } else {
                self.background_hook(hook, &event, &ctx).await;
            }
        }
        agg
    }

    /// Fire `event` against every matching hook EXCEPT the frontmatter bucket
    /// scoped to `exclude_agent_id` (source / plugin / other-agent buckets still
    /// fire).
    ///
    /// The orchestrator-side `SubagentStop` chokepoint uses this so it fires the
    /// session / plugin `SubagentStop` hooks WITHOUT re-firing the child's OWN
    /// frontmatter `Stop`→`SubagentStop` hooks — those already fired in-child via
    /// [`Self::execute_agent_scoped`] (claude fires a subagent's stop hooks inside
    /// `runAgent`). Excluding by id is race-free vs. the runner's
    /// `clear_agent_hooks` (which removes the bucket only after the terminal
    /// event). A strict no-op difference from [`Self::execute`] when the excluded
    /// agent registered no frontmatter hooks (every `FakeAgentTool` fixture).
    ///
    /// Mirrors [`Self::execute`]'s per-hook dispatch (progress beacon, `blocking`
    /// vs. background routing, `once` removal, no-early-`Block`-break with a
    /// sticky `Block` fold — #45(b)).
    pub async fn execute_excluding_agent(
        &self,
        event: HookEvent,
        ctx: HookContext,
        exclude_agent_id: protocol::AgentId,
    ) -> AggregateHookResult {
        // #41 runner-head gate (`h$`).
        if let Some(skipped) = self.policy_disable_gate(&event) {
            return skipped;
        }
        let reg = self.registry.read().await;
        let matched: Vec<HookDefinition> = reg
            .match_event_excluding_agent(&event, exclude_agent_id)
            .into_iter()
            .cloned()
            .collect();
        drop(reg);
        let mut agg = AggregateHookResult::default();
        let hook_event = format!("{:?}", event.event_type());
        for hook in &matched {
            agg.progress.push(crate::events::HookProgressEvent {
                hook_event: hook_event.clone(),
                hook_name: hook.name.clone(),
                status_message: hook.status_message.clone(),
            });
            if hook.blocking {
                let result = self.dispatcher().dispatch(hook, &event, &ctx).await;
                if hook.once && matches!(result.outcome, HookOutcome::Success) {
                    self.registry.write().await.remove_once_hook(hook.id);
                }
                Self::merge(&mut agg, hook, result);
                // #45(b): no early break on first `Block` — see `execute`. All
                // matched hooks dispatch; `merge` keeps `Block` sticky.
            } else {
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
                // B2: inject `LINGXI_PROJECT_DIR` into the child env so hook
                // scripts referencing `$LINGXI_PROJECT_DIR` resolve to the
                // stable project root. claude-code builds the env as
                // `{ ...subprocessEnv(), LINGXI_PROJECT_DIR: toHookPath(projectDir) }`
                // (`utils/hooks.ts:882-885`): the engine value is spread AFTER
                // the base env, so it wins over any pre-existing entry. We
                // mirror that precedence — start from the hook's declared `env`
                // (our analog of the base/subprocess env), then `insert` the
                // engine value last so it overwrites a user-supplied
                // `LINGXI_PROJECT_DIR`. The value is the stable project root,
                // falling back to `ctx.cwd` when no root is wired yet
                // (`HookContext.project_dir == None`).
                //
                // Divergence (documented, not a gap): Windows `toHookPath`
                // POSIX-path conversion is skipped — macOS/Linux parity target,
                // consistent with `turn_loop.rs::absolutize`. Full
                // `subprocessEnv()` base-env replication and `CLAUDE_ENV_FILE`
                // are out of B2 scope.
                let mut child_env = env.clone();
                // #43: `...Uot(o)` child-session env spread. claude-code assembles
                // the hook command env as `P={...WO(), ...Uot(o), LINGXI_PROJECT_DIR}`
                // (BIN off 205727901) where `o=u9e(hookInput)={sessionId:session_id,
                // effortLevel:effort?.level, source:"harness"}` (BIN off 199137330).
                // `Uot` emits, in order:
                //   LINGXI="1"                       (always)
                //   LINGXI_SESSION_ID=sessionId     (always)
                //   LINGXI_CHILD_SESSION="1"        (always)
                //   AI_AGENT=Mer("agent")                ONLY when source==="agent"
                //   LINGXI_EFFORT=effortLevel            ONLY when effortLevel set
                //   TRACEPARENT=<otel>                   ONLY when Evt() (OTel on)
                // For hooks `source==="harness"`, so `AI_AGENT` is NEVER emitted on a
                // hook child — distinct from the Bash spawn (`source:"agent"`, which
                // DOES set AI_AGENT). We mirror exactly: set LINGXI /
                // LINGXI_SESSION_ID / LINGXI_CHILD_SESSION unconditionally,
                // LINGXI_EFFORT only when `ctx.effort` carries a level, and we do NOT
                // set AI_AGENT. Spread BEFORE LINGXI_PROJECT_DIR so the engine project
                // dir still wins (no key overlap, so order is cosmetic, but it tracks
                // the binary's spread position).
                //
                // Residual: TRACEPARENT (OTel) — LingXi has no per-turn OTel span, so
                // `Evt()` is effectively false and the binary would omit it too; the
                // same documented residual as the Bash spawn path.
                child_env.insert("LINGXI".to_string(), "1".to_string());
                child_env.insert("LINGXI_SESSION_ID".to_string(), ctx.session_id.to_string());
                child_env.insert("LINGXI_CHILD_SESSION".to_string(), "1".to_string());
                if let Some(effort) = &ctx.effort {
                    child_env.insert("LINGXI_EFFORT".to_string(), effort.level.clone());
                }
                let project_dir = ctx.project_dir.clone().unwrap_or_else(|| ctx.cwd.clone());
                let project_dir_str = project_dir.to_string_lossy().into_owned();
                child_env.insert("LINGXI_PROJECT_DIR".to_string(), project_dir_str.clone());
                // #43: COLUMNS/LINES from the controlling-terminal size. claude
                // reads `{columns:L,rows:D}=process.stdout` then
                // `if(L)P.COLUMNS=String(L);if(D)P.LINES=String(D)` (BIN off
                // 205727903) — set ONLY when truthy (the `if(L)`/`if(D)` falsy
                // guard). The size is threaded in via `HookContext` from the
                // engine's stdout (the crate has no TTY of its own); `None` /
                // `0` matches a non-TTY `process.stdout.columns === undefined`.
                if let Some(cols) = ctx.terminal_columns.filter(|c| *c != 0) {
                    child_env.insert("COLUMNS".to_string(), cols.to_string());
                }
                if let Some(rows) = ctx.terminal_rows.filter(|r| *r != 0) {
                    child_env.insert("LINES".to_string(), rows.to_string());
                }
                // #43: literal `${LINGXI_PROJECT_DIR}` token substitution in the
                // command AND every arg (claude-code `_e` mapper, BIN off
                // 205727901: `if(!fe.includes("${"))return fe; fe=fe.replaceAll(
                // "${LINGXI_PROJECT_DIR}",()=>S)`, applied as `k=[_e(e.command),
                // e.args.map(_e)]`). A non-shell exec / arg never gets a shell to
                // expand `$LINGXI_PROJECT_DIR`, so the literal `${…}` token must be
                // replaced here. `${LINGXI_PLUGIN_ROOT}` / `${LINGXI_PLUGIN_DATA}`
                // need plugin scope (not on `HookExecutor::Command`) and are a
                // documented residual — a string carrying only those tokens passes
                // through unchanged, matching claude when no plugin scope is bound.
                let command = substitute_project_dir(command, &project_dir_str);
                let args: Vec<String> = args
                    .iter()
                    .map(|a| substitute_project_dir(a, &project_dir_str))
                    .collect();
                // claude-code writes `jsonStringify(hookInput) + '\n'` to the
                // child's stdin then closes it (`hooks.ts:1006`/`1210`). The
                // trailing newline is load-bearing: a bash `read -r line`
                // returns exit 1 on EOF-before-delimiter without it.
                let pcmd = ProcessCommand {
                    command,
                    args,
                    cwd: cwd.clone().or_else(|| Some(ctx.cwd.clone())),
                    env: child_env,
                    timeout: Some(effective_timeout),
                    stdin: Some(format!("{body}\n")),
                };
                // Hooks require workspace trust upstream (claude-code
                // `shouldSkipHookDueToTrust`), so an audited bypass is the
                // parity-honest construction here.
                let sandboxed = sandbox.bypass_with_audit(pcmd, "hook_command");
                // Runtime `{"async":true}` first-line detection (claude-code
                // `hooks.ts:1117-1166`): a hook whose first stdout line is that
                // marker is backgrounded and contributes no synchronous decision.
                // A runner without a streaming implementation reports `Completed`
                // for every hook (the default trait method), so non-async hooks —
                // i.e. every hook that does not print the marker — behave exactly
                // as the buffered path did.
                let default_async_timeout =
                    Duration::from_millis(crate::async_registry::DEFAULT_ASYNC_HOOK_TIMEOUT_MS);
                let (result, timed_out) = match process
                    .run_hook_with_async_detection(&sandboxed, default_async_timeout)
                    .await
                {
                    Ok(traits::HookRunOutcome::Backgrounded) => (
                        HookResult {
                            outcome: HookOutcome::Success,
                            stdout: String::new(),
                            stderr: String::new(),
                            exit_code: None,
                            response: None,
                        },
                        false,
                    ),
                    Ok(traits::HookRunOutcome::Completed(output)) => {
                        map_command_output(hook, Ok(output), expected_event)
                    }
                    Err(e) => map_command_output(hook, Err(e), expected_event),
                };
                if timed_out {
                    emit_command_timeout(hook, effective_timeout);
                }
                result
            }
            HookExecutor::Agent {
                agent_type,
                prompt,
                model,
            } => {
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
                        model.as_deref(),
                    )
                    .await;
                emit_agent_signal(hook, &outcome.signal, effective_timeout);
                outcome.result
            }
            HookExecutor::Prompt {
                prompt,
                model,
                continue_on_block,
            } => {
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
                    .execute(hook, prompt, model.as_deref(), *continue_on_block, &body)
                    .await;
                emit_prompt_signal(hook, &outcome.signal, effective_timeout);
                outcome.result
            }
        }
    }
}

impl HookExecutorImpl {
    /// #41 runner-head gate (`h$`/`cH`, BIN off 195521423 / 205755512). When
    /// the `policySettings.disableAllHooks` managed setting is active, every
    /// `execute*` path returns the empty (default) aggregate WITHOUT dispatching
    /// any hook, after logging the binary's skip line
    /// `Skipping hooks for {event[:match_query]} due to 'disableAllHooks' managed
    /// setting` (the binary's label is `event:matchQuery` when a `matchQuery` is
    /// present, else just `event`). Returns `Some(default_aggregate)` when gated,
    /// `None` otherwise.
    fn policy_disable_gate(&self, event: &HookEvent) -> Option<AggregateHookResult> {
        if !self.policy_disable_all_hooks {
            return None;
        }
        let event_name = format!("{:?}", event.event_type());
        let label = match Self::runner_match_query(event) {
            Some(q) => format!("{event_name}:{q}"),
            None => event_name,
        };
        tracing::info!("Skipping hooks for {label} due to 'disableAllHooks' managed setting");
        Some(AggregateHookResult::default())
    }

    /// The `matchQuery` the binary's `cH` runner is invoked with for `event`
    /// (the tool name for tool/permission events, else `None`) — used only to
    /// format the #41 skip-log label. Mirrors `HookRegistry::match_query_for`.
    fn runner_match_query(event: &HookEvent) -> Option<String> {
        match event {
            HookEvent::PreToolUse { tool_name, .. }
            | HookEvent::PostToolUse { tool_name, .. }
            | HookEvent::PostToolUseFailure { tool_name, .. }
            | HookEvent::PermissionRequest { tool_name, .. }
            | HookEvent::PermissionDenied { tool_name, .. } => Some(tool_name.clone()),
            _ => None,
        }
    }

    fn merge(agg: &mut AggregateHookResult, hook: &HookDefinition, r: HookResult) {
        if let Some(resp) = &r.response {
            // #45(b): claude-code's `cH` runner dispatches EVERY matched hook
            // (`c.map(async …)` + await-all, BIN off 205755512 — no break) then
            // folds the verdict via `ctt(e)=e.some(t=>t.blocked)`: any hook's
            // block wins, regardless of order. Now that the per-event loops no
            // longer short-circuit on the first `Block`, `Block` must be STICKY
            // here so a later non-`Block` hook can't overwrite an earlier block
            // (the OR-fold equivalent of `some(blocked)`), and the block `reason`
            // freezes at the FIRST blocker. Every OTHER channel
            // (systemMessage/additionalContext/updatedInput/…) still accumulates
            // or last-wins exactly as before, so later hooks' side effects — which
            // the old early `break` silently dropped — are now preserved.
            let already_blocked =
                matches!(agg.decision, Some(crate::response::HookDecision::Block));
            if resp.decision.is_some() && !already_blocked {
                agg.decision = resp.decision;
            }
            // Freeze the block reason at the first blocker: once blocked, a later
            // hook's `reason` no longer overwrites the aggregate one.
            if let Some(reason) = &resp.reason {
                if !already_blocked {
                    agg.reason = Some(reason.clone());
                }
            }
            if let Some(input) = &resp.updated_input {
                agg.modified_input = Some(input.clone());
            }
            if let Some(msg) = &resp.system_message {
                agg.system_messages.push(msg.clone());
            }
            // `additionalContext` is kept on its OWN aggregate channel, distinct
            // from `system_messages`: only `additional_contexts` reaches the
            // model (claude-code `hook_additional_context`, `messages.ts:4117`),
            // whereas `system_messages` is transcript-facing only (`:4258`). It is
            // injected as its own <system-reminder> message, not folded into the
            // tool_result content (claude-code `toolExecution.ts:845`).
            if let Some(ctx) = &resp.additional_context {
                agg.additional_contexts.push(ctx.clone());
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
            // PostToolUse `updatedToolOutput` (all-tools, #38): keep the latest
            // replacement any folded hook returned (claude-code keeps the most
            // recent — BIN off 205724076). `!== void 0` semantics are preserved
            // by the `Option<Option<Value>>` shape: the outer `Some` means a
            // hook set the key (even to `null`), so we fold whenever it is
            // `Some`. The orchestrator applies it for ALL tools (no isMcp gate).
            if let Some(out) = &resp.updated_tool_output {
                agg.updated_tool_output = Some(out.clone());
            }
            // PermissionDenied `retry`: OR-fold so a single hook saying
            // `retry: true` flips the aggregate, mirroring TS's
            // `if (result.retry) hookSaysRetry = true` (`toolExecution.ts:1090`).
            // A `Some(false)` / `None` leaves it untouched.
            if resp.retry == Some(true) {
                agg.retry = true;
            }
            // #40 `terminalSequence`: keep the latest a folded hook returned
            // (claude-code applies `szn` per hook result). The consumer (TUI
            // terminal writer) runs the allowlist validator + emit on apply.
            if let Some(ts) = &resp.terminal_sequence {
                agg.terminal_sequence = Some(ts.clone());
            }
            // `sessionTitle` (UserPromptSubmit output, BIN off 201754804):
            // keep the latest — mirrors TS's `applyHookSessionTitle` last-wins
            // assignment. `None` for all non-`UserPromptSubmit` hooks.
            if let Some(title) = &resp.session_title {
                agg.session_title = Some(title.clone());
            }
            // `suppressOriginalPrompt` (UserPromptSubmit output): OR-fold so a
            // single hook setting it flips the aggregate (a later hook returning
            // `false` doesn't undo a prior `true`).
            if resp.suppress_original_prompt {
                agg.suppress_original_prompt = true;
            }
            // `displayContent` (MessageDisplay output, BIN off 201757586):
            // keep the latest a folded hook returned — mirrors TS last-wins.
            if let Some(dc) = &resp.display_content {
                agg.display_content = Some(dc.clone());
            }
            // `watchPaths` (FileChanged / CwdChanged output): accumulate every
            // hook's entries in execution order — claude-code's `v3r` / `E3r`
            // return `{ ..., watchPaths }` as the concatenation of each fired
            // hook's `hookSpecificOutput.watchPaths`. The file-changed watcher
            // restarts over the union when the folded set is non-empty.
            if let Some(paths) = &resp.watch_paths {
                agg.watch_paths.extend(paths.iter().cloned());
            }
            agg.attachments.extend(resp.attachments.clone());
        }
        agg.all_results.push((hook.id, r));
    }
}

/// #43: substitute every literal `${LINGXI_PROJECT_DIR}` token in `s` with
/// `project_dir`.
///
/// Byte-faithful port of claude-code's `_e` mapper (BIN off 205727901):
/// `if(!fe.includes("${"))return fe; fe=fe.replaceAll("${LINGXI_PROJECT_DIR}",
/// ()=>S)` — the `${`-presence fast-path guard (a string with no `${` is returned
/// untouched, skipping the scan) and `replaceAll` (EVERY occurrence) semantics.
/// `()=>S` is a replacer FUNCTION in JS, so a literal `$1` / `$&` in the project
/// path is NOT treated as a replacement-pattern special; `str::replace` matches
/// that (it inserts the replacement verbatim).
///
/// The plugin tokens `${LINGXI_PLUGIN_ROOT}` / `${LINGXI_PLUGIN_DATA}` are NOT
/// handled here: they require plugin scope (`pluginRoot` / `pluginData`), which
/// `HookExecutor::Command` does not carry — a documented residual. A string
/// containing only those tokens (and no `${LINGXI_PROJECT_DIR}`) passes through
/// unchanged, matching claude when no plugin scope is bound.
fn substitute_project_dir(s: &str, project_dir: &str) -> String {
    // `if(!fe.includes("${"))return fe` — fast path: no template token at all.
    if !s.contains("${") {
        return s.to_string();
    }
    s.replace("${LINGXI_PROJECT_DIR}", project_dir)
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
                effort: ctx.effort.clone(),
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
            duration_ms,
        } => {
            let payload = PostToolUsePayload {
                hook_event_name: HookEventNamePost,
                session_id: ctx.session_id.to_string(),
                transcript_path: ctx.transcript_path.to_string_lossy().into_owned(),
                cwd: ctx.cwd.to_string_lossy().into_owned(),
                permission_mode: ctx.permission_mode.clone(),
                agent_id: ctx.agent_id.as_ref().map(ToString::to_string),
                agent_type: ctx.agent_type.clone(),
                effort: ctx.effort.clone(),
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_response: tool_output.clone(),
                tool_use_id: tool_use_id.to_string(),
                duration_ms: *duration_ms,
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
    /// Active reasoning-effort level (`effort: { level }`), sourced from the
    /// [`HookContext`]. `None` (omitted on the wire) for session-lifecycle
    /// hooks and effort-incapable models — faithful to claude-code's
    /// conditional `effort` spread in `createBaseHookInput`.
    effort: Option<crate::hook_payload::EffortLevel>,
    /// Current session title (binary-confirmed at BIN off 201745825). Threaded
    /// into `UserPromptSubmit` and `SessionStart` payloads; `None` for all
    /// other events (they ignore this field even if populated in ctx).
    session_title: Option<String>,
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
            effort: ctx.effort.clone(),
            session_title: ctx.session_title.clone(),
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
/// `TaskCompleted` arm, fired through the `TaskCreatedFirer` seam). The parity-fix
/// batch then ported `ElicitationResult` ([P0] gap). All 30 `HookEvent` variants
/// are now serializable; the `_ => None` catch-all is a forward-compatibility guard.
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
                effort: b.effort,
                stop_hook_active: false,
                last_assistant_message: None,
                background_tasks: ctx.background_tasks.clone(),
                session_crons: ctx.session_crons.clone(),
            };
            Some(("Stop", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SubagentStop {
            agent_id,
            agent_type,
            ..
        } => {
            let payload = SubagentStopPayload {
                hook_event_name: HookEventNameSubagentStop,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                stop_hook_active: false,
                agent_id: agent_id.to_string(),
                agent_transcript_path: String::new(),
                // claude `agent_type: a ?? ""` — now carried on the event
                // (mirrors SubagentStart); fall back to the context for older
                // call paths that left the event's `agent_type` empty.
                agent_type: if agent_type.is_empty() {
                    b.agent_type.unwrap_or_default()
                } else {
                    agent_type.clone()
                },
                effort: b.effort,
                last_assistant_message: None,
                background_tasks: ctx.background_tasks.clone(),
                session_crons: ctx.session_crons.clone(),
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
                effort: b.effort,
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
            teammate_name,
            team_name,
        } => {
            // `executeTaskCreatedHooks` (`utils/hooks.ts:3756-3764`): the wire
            // payload carries `task_subject` (required) + optional
            // `task_description` / `teammate_name` / `team_name`. The Rust
            // `TaskCreated` variant sources the subject from the task's
            // `task_type` taxonomy bucket and the description from
            // `description`; `teammate_name` / `team_name` ride from the
            // creating teammate's identity (TS `getAgentName()` / `getTeamName()`)
            // when bound, else `None`.
            let payload = TaskCreatedPayload {
                hook_event_name: HookEventNameTaskCreated,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                task_id: task_id.clone(),
                task_subject: task_type.clone(),
                task_description: Some(description.clone()),
                teammate_name: teammate_name.clone(),
                team_name: team_name.clone(),
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
                effort: b.effort,
                prompt: prompt.clone(),
                session_title: b.session_title,
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
                effort: b.effort,
                model: None,
                session_title: b.session_title,
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
                effort: b.effort,
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
            duration_ms,
        } => {
            let payload = PostToolUseFailurePayload {
                hook_event_name: HookEventNamePostToolUseFailure,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                tool_use_id: tool_use_id.to_string(),
                error: error.clone(),
                is_interrupt: None,
                duration_ms: *duration_ms,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
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
                effort: b.effort,
                teammate_name: teammate_name.clone(),
                team_name: team_name.clone(),
            };
            Some(("TeammateIdle", serde_json::to_string(&payload).ok()?))
        }
        // #39 PostToolBatch (claude-code `G4t`, BIN off 205710327): the full
        // batch of resolved tool calls. Fires through `vd(r,void 0,n)` so it
        // carries the base shape (permission_mode/agent_type from ctx).
        HookEvent::PostToolBatch { tool_calls } => {
            let payload = PostToolBatchPayload {
                hook_event_name: HookEventNamePostToolBatch,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                tool_calls: tool_calls.clone(),
            };
            Some(("PostToolBatch", serde_json::to_string(&payload).ok()?))
        }
        // #39 UserPromptExpansion (claude-code `b$t`, BIN off 201270310).
        HookEvent::UserPromptExpansion {
            expansion_type,
            command_name,
            command_args,
            command_source,
            prompt,
        } => {
            let payload = UserPromptExpansionPayload {
                hook_event_name: HookEventNameUserPromptExpansion,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                expansion_type: *expansion_type,
                command_name: command_name.clone(),
                command_args: command_args.clone(),
                command_source: command_source.clone(),
                prompt: prompt.clone(),
            };
            Some(("UserPromptExpansion", serde_json::to_string(&payload).ok()?))
        }
        // #39 MessageDisplay (claude-code `aAt`, BIN off 205705090). Fires
        // through `vd(void 0)` (no permission mode), with `forceSyncExecution`
        // + `suppressPerInvocationTelemetry` — those two are firing-site flags,
        // not wire fields, so they do not appear in the payload.
        HookEvent::MessageDisplay {
            turn_id,
            message_id,
            index,
            is_final,
            delta,
        } => {
            let payload = MessageDisplayPayload {
                hook_event_name: HookEventNameMessageDisplay,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                // `vd(void 0)` => no permission mode threaded.
                permission_mode: None,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                turn_id: turn_id.clone(),
                message_id: message_id.clone(),
                index: *index,
                is_final: *is_final,
                delta: delta.clone(),
            };
            Some(("MessageDisplay", serde_json::to_string(&payload).ok()?))
        }
        // `executeElicitationResultHooks` (BIN off 67628403). Wire schema
        // (binary-confirmed at BIN off ~201751493):
        // `{hook_event_name:"ElicitationResult", mcp_server_name:string,
        //   elicitation_id?:string, mode?:enum(["form","url"]),
        //   action:enum(["accept","decline","cancel"]),
        //   content?:record(string,unknown)}`.
        // The `action` and `content` fields are extracted from the
        // `HookEvent::ElicitationResult.result` JSON blob (the complete
        // elicitation response). `elicitation_id` and `mode` are not yet
        // threaded through `HookEvent::ElicitationResult` — defaulted to `None`
        // per the B1 default-fill convention. Uses `createBaseHookInput
        // (permissionMode)` (same as `Elicitation`), so permission_mode IS
        // threaded.
        HookEvent::ElicitationResult {
            server_name,
            result,
        } => {
            // Extract `action` from the result JSON. The binary schema requires
            // it; fall back to `"cancel"` (the safe default) when absent, so the
            // hook process still receives a valid payload even if the engine
            // didn't capture the action.
            let action = result
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("cancel")
                .to_string();
            // `content` is a record (optional): pass it through as-is when present.
            let content = result.get("content").cloned();
            let payload = ElicitationResultPayload {
                hook_event_name: HookEventNameElicitationResult,
                session_id: b.session_id,
                transcript_path: b.transcript_path,
                cwd: b.cwd,
                permission_mode: b.permission_mode,
                agent_id: b.agent_id,
                agent_type: b.agent_type,
                effort: b.effort,
                mcp_server_name: server_name.clone(),
                elicitation_id: None,
                mode: None,
                action,
                content,
            };
            Some(("ElicitationResult", serde_json::to_string(&payload).ok()?))
        }
        _ => None,
    }
}

/// Build the `(HookResult, timed_out)` pair for a non-timeout process error.
fn process_error_outcome(hook: &HookDefinition, e: &ProcessError) -> (HookResult, bool) {
    (
        HookResult {
            outcome: HookOutcome::Error,
            stdout: String::new(),
            stderr: format!("Hook {} failed: process error: {e}", hook.id),
            exit_code: None,
            response: None,
        },
        false,
    )
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
        Err(
            e @ (ProcessError::Io(_)
            | ProcessError::Unsupported
            | ProcessError::PolicyUnsupported(_)
            | ProcessError::MalformedSandboxPlan(_)
            | ProcessError::SandboxEnforcementFailed(_)),
        ) => process_error_outcome(hook, &e),
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
                    // exit 2 ⇒ BLOCK. Binary `hooks.ts`:
                    //   blockingError = `[${getHookDisplayText(hook)}]: ${stderr||"No stderr output"}`
                    // — the hook display text in brackets, then the RAW stderr
                    // (NOT trimmed); the "No stderr output" placeholder applies
                    // only when stderr is empty (JS `||`, an empty string is
                    // falsy; a whitespace-only stderr is used verbatim).
                    // `getHookDisplayText` (binary `rCe`) for a command hook is
                    // `args ? [command, ...args].join(" ") : command`.
                    let display = match &hook.executor {
                        HookExecutor::Command { command, args, .. } if !args.is_empty() => {
                            std::iter::once(command.as_str())
                                .chain(args.iter().map(String::as_str))
                                .collect::<Vec<_>>()
                                .join(" ")
                        }
                        HookExecutor::Command { command, .. } => command.clone(),
                        // Non-command executors do not reach this process path;
                        // fall back to the human-readable name defensively.
                        _ => hook.name.clone(),
                    };
                    let body = if o.stderr.is_empty() {
                        "No stderr output"
                    } else {
                        o.stderr.as_str()
                    };
                    let reason = format!("[{display}]: {body}");
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

/// Emit `HOOK_TIMEOUT` telemetry when a `SessionEnd` hook is cut off by the
/// *batch* shutdown deadline (claude-code `Wqt`), distinct from a per-hook
/// timeout. `batch_timeout_ms` is the whole-batch budget the hook overran.
fn emit_session_end_batch_timeout(hook: &HookDefinition, batch_timeout_ms: u64) {
    tracing::info!(
        event = telemetry::tengu::orchestrator::HOOK_TIMEOUT,
        hook_id = %hook.id,
        hook_kind = "session_end_batch",
        timeout_ms = batch_timeout_ms,
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
#[path = "executor_test.rs"]
mod executor_test;
