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
    ElicitationResultPayload, FileChangedPayload,
    HookEventNameConfigChange, HookEventNameCwdChanged, HookEventNameElicitation,
    HookEventNameElicitationResult,
    HookEventNameFileChanged, HookEventNameInstructionsLoaded, HookEventNameNotification,
    HookEventNamePermissionDenied, HookEventNamePermissionRequest, HookEventNamePost,
    HookEventNamePostCompact, HookEventNamePostToolUseFailure, HookEventNamePre,
    HookEventNamePreCompact, HookEventNameSessionEnd, HookEventNameSessionStart,
    HookEventNameSetup, HookEventNameStop, HookEventNameStopFailure, HookEventNameSubagentStart,
    HookEventNameSubagentStop, HookEventNameTaskCompleted, HookEventNameTaskCreated,
    HookEventNameMessageDisplay, HookEventNamePostToolBatch, HookEventNameTeammateIdle,
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
use traits::{HttpTransport, OutputStream, ProcessCommand, ProcessError, ProcessRunner, RuntimeSpawner, Sandbox};

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
    pub async fn execute_session_end(&self, event: HookEvent, ctx: HookContext) -> AggregateHookResult {
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
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(batch_timeout_ms);

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
                        .emit_hook_started(
                            &hook.id.to_string(),
                            &hook.name,
                            &hook_event,
                        )
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
                        .emit_hook_started(
                            &hook.id.to_string(),
                            &hook.name,
                            &hook_event,
                        )
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
                child_env.insert(
                    "LINGXI_SESSION_ID".to_string(),
                    ctx.session_id.to_string(),
                );
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
                let (result, timed_out) =
                    map_command_output(hook, process.run(&sandboxed).await, expected_event);
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
        tracing::info!(
            "Skipping hooks for {label} due to 'disableAllHooks' managed setting"
        );
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
            };
            Some(("Stop", serde_json::to_string(&payload).ok()?))
        }
        HookEvent::SubagentStop {
            agent_id, agent_type, ..
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

    #[test]
    fn session_end_floor_and_cap_match_binary() {
        // claude-code `nzn=1500`, `rym=60000` (BIN off 205765355 / 205765364).
        assert_eq!(SESSION_END_HOOK_TIMEOUT_FLOOR_MS, 1_500);
        assert_eq!(SESSION_END_HOOK_TIMEOUT_CAP_MS, 60_000);
        assert_eq!(
            SESSION_END_HOOKS_TIMEOUT_ENV,
            "LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS"
        );
    }
}

#[cfg(test)]
mod session_end_timeout_tests {
    use super::{max_per_hook_timeout_ms, session_end_batch_timeout_ms};
    use crate::definition::{HookDefinition, HookExecutor, HookSource};
    use crate::events::HookEventType;
    use protocol::HookId;
    use std::time::Duration;

    fn hook_with_timeout(timeout: Option<Duration>) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "test-session-end".into(),
            events: vec![HookEventType::SessionEnd],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: "noop".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[test]
    fn default_with_no_per_hook_timeouts_collapses_to_floor() {
        // No env, no per-hook timeout ⇒ max(1500, min(0, 60000)) = 1500.
        assert_eq!(session_end_batch_timeout_ms(None, 0), 1_500);
    }

    #[test]
    fn default_clamps_below_floor_up_to_1500() {
        // max per-hook = 800ms ⇒ max(1500, min(800, 60000)) = 1500.
        assert_eq!(session_end_batch_timeout_ms(None, 800), 1_500);
    }

    #[test]
    fn default_uses_max_per_hook_when_between_floor_and_cap() {
        // max per-hook = 30000ms ⇒ max(1500, min(30000, 60000)) = 30000.
        assert_eq!(session_end_batch_timeout_ms(None, 30_000), 30_000);
    }

    #[test]
    fn default_caps_above_60000_down_to_cap() {
        // max per-hook = 120000ms ⇒ max(1500, min(120000, 60000)) = 60000.
        assert_eq!(session_end_batch_timeout_ms(None, 120_000), 60_000);
    }

    #[test]
    fn env_override_is_used_verbatim_and_unclamped() {
        // A finite, positive env value is returned EXACTLY (bypasses floor/cap).
        assert_eq!(session_end_batch_timeout_ms(Some("3000"), 0), 3_000);
        // Below the floor — still returned verbatim (env override is unclamped).
        assert_eq!(session_end_batch_timeout_ms(Some("100"), 30_000), 100);
        // Above the cap — still returned verbatim.
        assert_eq!(
            session_end_batch_timeout_ms(Some("999999"), 0),
            999_999
        );
    }

    #[test]
    fn env_parseint_style_trailing_suffix_is_ignored() {
        // JS `parseInt("3000abc", 10)` ⇒ 3000.
        assert_eq!(session_end_batch_timeout_ms(Some("3000abc"), 0), 3_000);
        assert_eq!(session_end_batch_timeout_ms(Some("  4200 "), 0), 4_200);
    }

    #[test]
    fn env_non_numeric_or_nonpositive_falls_through_to_clamp() {
        // `parseInt("abc")` ⇒ NaN ⇒ computed clamp (here floor).
        assert_eq!(session_end_batch_timeout_ms(Some("abc"), 0), 1_500);
        assert_eq!(session_end_batch_timeout_ms(Some(""), 0), 1_500);
        // Zero / negative are rejected by the `> 0` guard ⇒ computed clamp.
        assert_eq!(session_end_batch_timeout_ms(Some("0"), 30_000), 30_000);
        assert_eq!(session_end_batch_timeout_ms(Some("-5"), 45_000), 45_000);
    }

    #[test]
    fn max_per_hook_timeout_takes_the_largest_declared() {
        let hooks = vec![
            hook_with_timeout(Some(Duration::from_secs(5))),
            hook_with_timeout(Some(Duration::from_secs(42))),
            hook_with_timeout(None),
            hook_with_timeout(Some(Duration::from_secs(3))),
        ];
        // max = 42s = 42000ms.
        assert_eq!(max_per_hook_timeout_ms(&hooks), 42_000);
        // Feeding that into the clamp ⇒ within [1500, 60000] ⇒ 42000.
        assert_eq!(
            session_end_batch_timeout_ms(None, max_per_hook_timeout_ms(&hooks)),
            42_000
        );
    }

    #[test]
    fn max_per_hook_timeout_empty_or_all_none_is_zero() {
        assert_eq!(max_per_hook_timeout_ms(&[]), 0);
        let hooks = vec![hook_with_timeout(None), hook_with_timeout(None)];
        assert_eq!(max_per_hook_timeout_ms(&hooks), 0);
    }
}

/// Integration-style tests that exercise [`HookExecutorImpl::execute_session_end`]
/// end-to-end: a slow `SessionEnd` hook is cut off by the batch shutdown deadline,
/// and a fast one completes normally.
#[cfg(test)]
mod session_end_batch_deadline_tests {
    use super::*;
    use crate::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use crate::events::{HookEvent, HookEventType};
    use crate::registry::{HookContext, HookRegistry};
    use crate::response::{HookOutcome, HookResult};
    use protocol::HookId;
    use std::sync::atomic::{AtomicBool, Ordering};
    use traits::RuntimeError;

    /// `HttpTransport` stub — the Builtin arm never touches HTTP.
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
        async fn cancel(&self, _handle: &traits::BackgroundTaskHandle) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    /// A builtin handler that sleeps `delay`, then flips `ran` and returns
    /// success. If the batch deadline cuts it off, the sleep is cancelled and
    /// `ran` stays `false`.
    struct SleepingHandler {
        id: String,
        delay: Duration,
        ran: Arc<AtomicBool>,
    }
    #[async_trait]
    impl BuiltinHookHandler for SleepingHandler {
        async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
            tokio::time::sleep(self.delay).await;
            self.ran.store(true, Ordering::SeqCst);
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: None,
            }
        }
        fn id(&self) -> &str {
            &self.id
        }
    }

    fn session_end_builtin_hook(handler_id: &str) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: "test-session-end".into(),
            events: vec![HookEventType::SessionEnd],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: handler_id.into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    fn session_end_event() -> HookEvent {
        HookEvent::SessionEnd {
            session_id: protocol::SessionId::nil(),
            reason: "logout".into(),
        }
    }

    /// Both env-driven deadline behaviors in ONE test (the two halves share the
    /// process-global `LINGXI_SESSIONEND_HOOKS_TIMEOUT_MS`, so they must not
    /// race in parallel — merging them keeps the env mutation single-threaded):
    ///
    /// * a SessionEnd hook running LONGER than the batch deadline is cut off
    ///   (outcome `Timeout`, side-effect never lands, returns fast); and
    /// * a hook completing WITHIN the deadline runs to completion (the deadline
    ///   is a ceiling, not a forced wait).
    #[tokio::test]
    async fn batch_deadline_cuts_off_slow_hook_but_not_fast_hook() {
        let prev = std::env::var(SESSION_END_HOOKS_TIMEOUT_ENV).ok();

        // --- Half 1: tiny deadline (50ms), hook sleeps 5s → cut off. ---
        std::env::set_var(SESSION_END_HOOKS_TIMEOUT_ENV, "50");
        let slow_ran = Arc::new(AtomicBool::new(false));
        let mut registry = HookRegistry::new();
        registry.register(session_end_builtin_hook("slow"));
        let reg = Arc::new(RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SleepingHandler {
            id: "slow".into(),
            delay: Duration::from_secs(5),
            ran: slow_ran.clone(),
        }));
        let start = std::time::Instant::now();
        let slow_agg = exec
            .execute_session_end(session_end_event(), HookContext::default())
            .await;
        let elapsed = start.elapsed();

        // --- Half 2: generous deadline (5000ms), hook sleeps 10ms → completes. ---
        std::env::set_var(SESSION_END_HOOKS_TIMEOUT_ENV, "5000");
        let fast_ran = Arc::new(AtomicBool::new(false));
        let mut registry = HookRegistry::new();
        registry.register(session_end_builtin_hook("fast"));
        let reg = Arc::new(RwLock::new(registry));
        let mut exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(SleepingHandler {
            id: "fast".into(),
            delay: Duration::from_millis(10),
            ran: fast_ran.clone(),
        }));
        let fast_agg = exec
            .execute_session_end(session_end_event(), HookContext::default())
            .await;

        // Restore env before asserting so a panic doesn't leak it.
        match prev {
            Some(v) => std::env::set_var(SESSION_END_HOOKS_TIMEOUT_ENV, v),
            None => std::env::remove_var(SESSION_END_HOOKS_TIMEOUT_ENV),
        }

        // Half 1 assertions: cut off well before the 5s sleep.
        assert!(
            elapsed < Duration::from_secs(2),
            "batch deadline must abort fast, took {elapsed:?}"
        );
        assert!(
            !slow_ran.load(Ordering::SeqCst),
            "the slow hook's success side-effect must NOT land — it was aborted"
        );
        let (_, slow_r) = &slow_agg.all_results[0];
        assert!(
            matches!(slow_r.outcome, HookOutcome::Timeout),
            "cut-off hook records a Timeout outcome"
        );
        assert!(slow_r.stderr.contains("batch deadline"));

        // Half 2 assertions: ran to completion.
        assert!(
            fast_ran.load(Ordering::SeqCst),
            "a hook finishing within the deadline must run to completion"
        );
        let (_, fast_r) = &fast_agg.all_results[0];
        assert!(matches!(fast_r.outcome, HookOutcome::Success));
    }

    /// No registered SessionEnd hook ⇒ strict no-op (default aggregate), no
    /// deadline machinery observable.
    #[tokio::test]
    async fn no_session_end_hook_is_a_noop() {
        let reg = Arc::new(RwLock::new(HookRegistry::new()));
        let exec = HookExecutorImpl::new(reg, Arc::new(UnusedHttp), Arc::new(UnusedRuntime));

        let agg = exec
            .execute_session_end(session_end_event(), HookContext::default())
            .await;

        assert!(agg.all_results.is_empty());
        assert_eq!(agg.decision, None);
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
    use traits::{ProcessHandle, ProcessOutput, RuntimeError, SandboxPolicy, SandboxedCommand};

    /// Mock `ProcessRunner` that returns a canned `ProcessOutput` (or
    /// `ProcessError`) and records the `SandboxedCommand` it was handed so the
    /// test can assert the stdin payload + trailing newline.
    struct MockRunner {
        result: Mutex<Option<Result<ProcessOutput, ProcessError>>>,
        recorded_stdin: Mutex<Option<String>>,
        /// B2: the child env the arm handed to the sandbox, captured so tests
        /// can assert `LINGXI_PROJECT_DIR` injection + precedence.
        recorded_env: Mutex<Option<HashMap<String, String>>>,
        /// #43: the resolved command + args (after `${LINGXI_PROJECT_DIR}`
        /// substitution), captured so tests can assert the token replacement.
        recorded_command: Mutex<Option<String>>,
        recorded_args: Mutex<Option<Vec<String>>>,
    }

    impl MockRunner {
        fn ok(output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Ok(output))),
                recorded_stdin: Mutex::new(None),
                recorded_env: Mutex::new(None),
                recorded_command: Mutex::new(None),
                recorded_args: Mutex::new(None),
            })
        }
        fn err(e: ProcessError) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Err(e))),
                recorded_stdin: Mutex::new(None),
                recorded_env: Mutex::new(None),
                recorded_command: Mutex::new(None),
                recorded_args: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            *self.recorded_stdin.lock().unwrap() = cmd.inner().stdin.clone();
            *self.recorded_env.lock().unwrap() = Some(cmd.inner().env.clone());
            *self.recorded_command.lock().unwrap() = Some(cmd.inner().command.clone());
            *self.recorded_args.lock().unwrap() = Some(cmd.inner().args.clone());
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
        async fn cancel(&self, _handle: &traits::BackgroundTaskHandle) -> Result<(), RuntimeError> {
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

    // ---- B2: LINGXI_PROJECT_DIR injection into the Command child env -----

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
    fn executor_with_hook(
        hook: HookDefinition,
        process: Arc<dyn ProcessRunner>,
    ) -> HookExecutorImpl {
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
            env.get("LINGXI_PROJECT_DIR").map(String::as_str),
            Some("/repo/root"),
            "engine project_dir is injected verbatim",
        );
    }

    #[tokio::test]
    async fn command_env_engine_project_dir_wins_over_user_env() {
        // The hook declares its own LINGXI_PROJECT_DIR; the engine value is set
        // AFTER the base spread in claude-code (`utils/hooks.ts:882-885`), so
        // the engine value wins. Match that precedence.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with_hook(
            command_hook_with_env(&[
                ("LINGXI_PROJECT_DIR", "/user/override"),
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
            env.get("LINGXI_PROJECT_DIR").map(String::as_str),
            Some("/engine/root"),
            "engine value overrides the user-supplied hook.env entry",
        );
        // Unrelated user env entries are preserved.
        assert_eq!(env.get("MY_VAR").map(String::as_str), Some("keep-me"));
    }

    #[tokio::test]
    async fn command_env_project_dir_falls_back_to_cwd() {
        // No project_dir wired → LINGXI_PROJECT_DIR falls back to ctx.cwd, the
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
            env.get("LINGXI_PROJECT_DIR").map(String::as_str),
            Some("/some/cwd"),
            "absent project_dir falls back to ctx.cwd",
        );
    }

    // ---- #43: COLUMNS/LINES env + ${LINGXI_PROJECT_DIR} substitution -------

    /// A Command hook with a custom `command` + `args`, so #43 substitution can
    /// be asserted against the recorded resolved values.
    fn command_hook_with_cmd_args(command: &str, args: &[&str]) -> HookDefinition {
        let mut h = command_hook();
        if let DefHookExecutor::Command {
            command: c, args: a, ..
        } = &mut h.executor
        {
            *c = command.to_string();
            *a = args.iter().map(|s| (*s).to_string()).collect();
        }
        h
    }

    #[test]
    fn substitute_project_dir_replaces_every_occurrence() {
        // `replaceAll` semantics — every `${LINGXI_PROJECT_DIR}` token is
        // replaced, not just the first.
        assert_eq!(
            substitute_project_dir("${LINGXI_PROJECT_DIR}/a:${LINGXI_PROJECT_DIR}/b", "/root"),
            "/root/a:/root/b",
        );
    }

    #[test]
    fn substitute_project_dir_fast_path_no_template() {
        // `if(!fe.includes("${"))return fe` — a string with no `${` is returned
        // untouched.
        assert_eq!(substitute_project_dir("./fmt.sh --check", "/root"), "./fmt.sh --check");
    }

    #[test]
    fn substitute_project_dir_leaves_plugin_tokens_untouched() {
        // The plugin tokens are NOT substituted here (no plugin scope) — a
        // residual; a string carrying only those passes through unchanged.
        assert_eq!(
            substitute_project_dir("${LINGXI_PLUGIN_ROOT}/x", "/root"),
            "${LINGXI_PLUGIN_ROOT}/x",
        );
    }

    #[tokio::test]
    async fn command_and_args_substitute_project_dir_token() {
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with_hook(
            command_hook_with_cmd_args(
                "${LINGXI_PROJECT_DIR}/.lingxi/fmt.sh",
                &["--root", "${LINGXI_PROJECT_DIR}", "--plain"],
            ),
            runner.clone(),
        );
        let ctx = HookContext {
            project_dir: Some(PathBuf::from("/repo/root")),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let cmd = runner.recorded_command.lock().unwrap().clone().unwrap();
        let args = runner.recorded_args.lock().unwrap().clone().unwrap();
        assert_eq!(cmd, "/repo/root/.lingxi/fmt.sh", "command token substituted");
        assert_eq!(
            args,
            vec!["--root".to_string(), "/repo/root".to_string(), "--plain".to_string()],
            "each arg token substituted; non-token args untouched",
        );
    }

    #[tokio::test]
    async fn command_env_sets_columns_and_lines_from_ctx() {
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let ctx = HookContext {
            terminal_columns: Some(120),
            terminal_rows: Some(40),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert_eq!(env.get("COLUMNS").map(String::as_str), Some("120"));
        assert_eq!(env.get("LINES").map(String::as_str), Some("40"));
    }

    #[tokio::test]
    async fn command_env_omits_columns_lines_when_absent_or_zero() {
        // The binary's `if(L)`/`if(D)` falsy guards: `None` (non-TTY,
        // `process.stdout.columns === undefined`) and `0` set NEITHER env var.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let ctx = HookContext {
            terminal_columns: None,
            terminal_rows: Some(0),
            ..Default::default()
        };

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert!(!env.contains_key("COLUMNS"), "None columns sets no COLUMNS");
        assert!(!env.contains_key("LINES"), "zero rows is falsy → no LINES");
    }

    #[tokio::test]
    async fn command_env_sets_uot_harness_vars() {
        // #43: the hook command env spreads `...Uot(o)` with `source:"harness"`
        // (BIN off 205727901 / 199137330): LINGXI=1, LINGXI_SESSION_ID,
        // LINGXI_CHILD_SESSION=1 are always present. `AI_AGENT` is gated on
        // `source==="agent"`, so a hook child must NOT carry it.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let ctx = HookContext::default();
        let expected_session = ctx.session_id.to_string();

        let _ = exec.execute(pre_event(), ctx).await;

        let env = runner.recorded_env.lock().unwrap().clone().unwrap();
        assert_eq!(env.get("LINGXI").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("LINGXI_CHILD_SESSION").map(String::as_str),
            Some("1"),
        );
        assert_eq!(
            env.get("LINGXI_SESSION_ID").map(String::as_str),
            Some(expected_session.as_str()),
            "Uot threads the session id into the hook child",
        );
        assert!(
            !env.contains_key("AI_AGENT"),
            "hook source is harness, not agent — AI_AGENT must be absent",
        );
    }

    #[tokio::test]
    async fn command_env_sets_effort_only_when_present() {
        // `Uot` sets `LINGXI_EFFORT=effortLevel` only when `effortLevel` is set
        // (`o.effortLevel = hookInput.effort?.level`). No effort on the ctx ⇒
        // the env var is omitted; an effort level ⇒ it is set verbatim.
        let runner = MockRunner::ok(output("", "", 0));
        let exec = executor_with(runner.clone());
        let _ = exec.execute(pre_event(), HookContext::default()).await;
        assert!(
            !runner
                .recorded_env
                .lock()
                .unwrap()
                .clone()
                .unwrap()
                .contains_key("LINGXI_EFFORT"),
            "no effort on ctx ⇒ no LINGXI_EFFORT",
        );

        let runner2 = MockRunner::ok(output("", "", 0));
        let exec2 = executor_with(runner2.clone());
        let ctx = HookContext {
            effort: Some(crate::hook_payload::EffortLevel::new("high")),
            ..Default::default()
        };
        let _ = exec2.execute(pre_event(), ctx).await;
        assert_eq!(
            runner2
                .recorded_env
                .lock()
                .unwrap()
                .clone()
                .unwrap()
                .get("LINGXI_EFFORT")
                .map(String::as_str),
            Some("high"),
            "effort level surfaces as LINGXI_EFFORT",
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
                agent_type: String::new(),
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
                file_path: Some(std::path::PathBuf::from(
                    "/work/.lingxi/settings.local.json",
                )),
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"ConfigChange""#));
        // `source` serializes to the snake_case wire literal.
        assert!(stdin.contains(r#""source":"local_settings""#));
        assert!(stdin.contains(r#""file_path":"/work/.lingxi/settings.local.json""#));
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
                file_path: std::path::PathBuf::from("/work/LINGXI.md"),
                memory_type: crate::events::InstructionsMemoryType::Project,
                load_reason: crate::events::InstructionsLoadReason::SessionStart,
                globs: None,
                trigger_file_path: None,
                parent_file_path: None,
            },
        )
        .await;
        assert!(stdin.contains(r#""hook_event_name":"InstructionsLoaded""#));
        assert!(stdin.contains(r#""file_path":"/work/LINGXI.md""#));
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
                parent_file_path: Some(std::path::PathBuf::from("/work/LINGXI.md")),
            },
        )
        .await;
        assert!(stdin.contains(r#""memory_type":"Local""#));
        assert!(stdin.contains(r#""load_reason":"path_glob_match""#));
        assert!(stdin.contains(r#""globs":["src/**/*.rs"]"#));
        assert!(stdin.contains(r#""trigger_file_path":"/work/src/main.rs""#));
        assert!(stdin.contains(r#""parent_file_path":"/work/LINGXI.md""#));
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
        // through the `TeammateIdleFirer` seam). The parity-fix batch then ported
        // `ElicitationResult` (P0 gap). All 30 `HookEvent` variants are now
        // serializable — this test is a no-op guard for future new unported variants.
        let ctx = HookContext::default();
        let unported: Vec<HookEvent> = vec![];
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
                    agent_type: String::new(),
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
                    teammate_name: None,
                    team_name: None,
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
                    file_path: std::path::PathBuf::from("/work/LINGXI.md"),
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
            // [P0] parity-fix: ElicitationResult (binary-confirmed at BIN off
            // ~201751493; action extracted from result JSON, defaults to "cancel").
            (
                HookEvent::ElicitationResult {
                    server_name: "srv".into(),
                    result: json!({"action": "accept", "content": {"token": "xyz"}}),
                },
                "ElicitationResult",
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

    #[test]
    fn elicitation_result_envelope_extracts_action_from_result() {
        // [P0] parity-fix: `ElicitationResult` wire payload extracts `action`
        // from the embedded `result` JSON blob (binary-confirmed schema).
        let ctx = HookContext::default();
        let ev = HookEvent::ElicitationResult {
            server_name: "my-server".into(),
            result: json!({"action": "decline", "content": {"reason": "no"}}),
        };
        let (marker, body) = build_envelope_body(&ev, &ctx).expect("must serialize");
        assert_eq!(marker, "ElicitationResult");
        assert!(body.contains(r#""mcp_server_name":"my-server""#), "{body}");
        assert!(body.contains(r#""action":"decline""#), "{body}");
        assert!(body.contains(r#""content":{"reason":"no"}"#), "{body}");
        // `elicitation_id` and `mode` default to `None` (absent from wire).
        assert!(!body.contains("elicitation_id"), "{body}");
        assert!(!body.contains("mode"), "{body}");
    }

    #[test]
    fn elicitation_result_envelope_defaults_action_to_cancel_when_absent() {
        // When `result` JSON has no `action`, fall back to `"cancel"` (safe default).
        let ctx = HookContext::default();
        let ev = HookEvent::ElicitationResult {
            server_name: "srv".into(),
            result: json!({}),
        };
        let (_marker, body) = build_envelope_body(&ev, &ctx).expect("must serialize");
        assert!(body.contains(r#""action":"cancel""#), "{body}");
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
        assert_eq!(
            agg.all_results.len(),
            1,
            "blocking hook IS in the aggregate"
        );
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

    // ---- #45b: no first-Block short-circuit + sticky Block; #41 runner gate -

    /// A `ProcessRunner` that returns a different pre-canned [`ProcessOutput`]
    /// per invocation (FIFO), so a multi-hook `execute` can be observed firing
    /// EVERY matched hook (no first-`Block` short-circuit — #45b).
    struct SequenceRunner {
        outputs: StdMutex<std::collections::VecDeque<ProcessOutput>>,
        runs: AtomicU64,
    }
    impl SequenceRunner {
        fn new(outputs: Vec<ProcessOutput>) -> Arc<Self> {
            Arc::new(Self {
                outputs: StdMutex::new(outputs.into_iter().collect()),
                runs: AtomicU64::new(0),
            })
        }
    }
    #[async_trait]
    impl ProcessRunner for SequenceRunner {
        async fn run(&self, _cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let next = self.outputs.lock().unwrap().pop_front();
            Ok(next.unwrap_or_else(|| out("", "", 0)))
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

    /// #45b: `execute` dispatches EVERY matched blocking hook even after one
    /// returns `Block`. The first hook blocks (exit 2); the second still runs and
    /// its `systemMessage` side effect is folded into the aggregate — which the
    /// old first-`Block` `break` would have silently dropped. The aggregate
    /// verdict stays `Block` (sticky), reason frozen at the first blocker.
    #[tokio::test]
    async fn execute_dispatches_all_hooks_after_block_and_keeps_side_effects() {
        let runner = SequenceRunner::new(vec![
            out("", "first blocker", 2),
            out(r#"{"systemMessage":"second ran"}"#, "", 0),
        ]);
        // Higher priority fires first (deterministic order).
        let mut registry = HookRegistry::new();
        registry.register(command_hook_with_priority(true, 10));
        registry.register(command_hook_with_priority(true, 0));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner.clone(), Arc::new(StubSandbox));

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(runner.runs.load(Ordering::SeqCst), 2, "both hooks must run");
        assert_eq!(agg.all_results.len(), 2, "both results folded");
        assert_eq!(agg.decision, Some(HookDecision::Block));
        assert_eq!(agg.reason.as_deref(), Some("first blocker"));
        assert!(
            agg.system_messages.iter().any(|m| m == "second ran"),
            "the later hook's systemMessage must survive the earlier Block: {:?}",
            agg.system_messages,
        );
    }

    /// #45b: a LATER non-Block hook must NOT overwrite an earlier Block verdict
    /// (the OR-fold equivalent of `some(blocked)`).
    #[tokio::test]
    async fn block_is_sticky_against_a_later_allowing_hook() {
        let runner = SequenceRunner::new(vec![
            out("", "blocked", 2),
            out(r#"{"decision":"approve"}"#, "", 0),
        ]);
        let mut registry = HookRegistry::new();
        registry.register(command_hook_with_priority(true, 10));
        registry.register(command_hook_with_priority(true, 0));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner.clone(), Arc::new(StubSandbox));

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(runner.runs.load(Ordering::SeqCst), 2, "both hooks must run");
        assert_eq!(
            agg.decision,
            Some(HookDecision::Block),
            "an earlier Block is sticky — a later approve cannot un-block",
        );
    }

    /// #41 runner-head gate (`h$`): when `policy_disable_all_hooks` is set, NO
    /// hook is dispatched and the default (empty) aggregate is returned.
    #[tokio::test]
    async fn policy_disable_all_hooks_skips_every_hook_at_runner_head() {
        let runner = SequenceRunner::new(vec![out("", "blocked", 2)]);
        let mut registry = HookRegistry::new();
        registry.register(command_hook_with_priority(true, 0));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner.clone(), Arc::new(StubSandbox))
        .with_policy_disable_all_hooks(true);

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(
            runner.runs.load(Ordering::SeqCst),
            0,
            "disableAllHooks must skip dispatch entirely",
        );
        assert_eq!(agg.decision, None, "skipped batch yields the default aggregate");
        assert!(agg.all_results.is_empty());
    }

    /// #41: with the gate OFF (the default no-policy path) hooks fire normally,
    /// so the gate is behavior-neutral until a policy is wired.
    #[tokio::test]
    async fn policy_disable_all_hooks_default_off_fires_hooks() {
        let runner = SequenceRunner::new(vec![out("", "blocked", 2)]);
        let mut registry = HookRegistry::new();
        registry.register(command_hook_with_priority(true, 0));
        let exec = HookExecutorImpl::new(
            Arc::new(RwLock::new(registry)),
            Arc::new(UnusedHttp),
            TestRuntime::new(),
        )
        .with_process_runner(runner.clone(), Arc::new(StubSandbox));

        let agg = exec.execute(pre_event(), HookContext::default()).await;

        assert_eq!(runner.runs.load(Ordering::SeqCst), 1, "gate off ⇒ hook fires");
        assert_eq!(agg.decision, Some(HookDecision::Block));
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
    fn builtin_hook(handler_id: &str, once: bool, status_message: Option<&str>) -> HookDefinition {
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

    /// #9: `execute_agent_scoped` fires ONLY the named agent's frontmatter
    /// SubagentStop hooks — a session-level SubagentStop hook is excluded (the
    /// orchestrator chokepoint owns those), so the in-child fire can't double-run.
    #[tokio::test]
    async fn execute_agent_scoped_fires_only_agent_frontmatter() {
        let agent = protocol::AgentId::new();
        let agent_runs = Arc::new(AtomicU32::new(0));
        let session_runs = Arc::new(AtomicU32::new(0));

        let mut registry = HookRegistry::new();
        // Agent frontmatter Stop hook → retargeted to SubagentStop (isAgent=true).
        let mut fm = builtin_hook("agent-stop", false, None);
        fm.events = vec![HookEventType::Stop];
        fm.name = "agent-stop".into();
        registry.register_agent_hooks(agent, &[fm], true);
        // Session-level SubagentStop hook (must NOT fire on the scoped call).
        let mut sess = builtin_hook("session-stop", false, None);
        sess.events = vec![HookEventType::SubagentStop];
        sess.name = "session-stop".into();
        sess.source = HookSource::User;
        registry.register(sess);

        let reg = Arc::new(RwLock::new(registry));
        let mut exec =
            HookExecutorImpl::new(reg.clone(), Arc::new(UnusedHttp), Arc::new(UnusedRuntime));
        exec.register_builtin(Arc::new(CountingBuiltin {
            id: "agent-stop".into(),
            runs: agent_runs.clone(),
            outcome: HookOutcome::Success,
        }));
        exec.register_builtin(Arc::new(CountingBuiltin {
            id: "session-stop".into(),
            runs: session_runs.clone(),
            outcome: HookOutcome::Success,
        }));

        let ev = HookEvent::SubagentStop {
            agent_id: agent,
            status: "completed".into(),
            agent_type: String::new(),
        };
        exec.execute_agent_scoped(ev, HookContext::default(), agent)
            .await;

        assert_eq!(
            agent_runs.load(Ordering::SeqCst),
            1,
            "the agent's frontmatter SubagentStop fires"
        );
        assert_eq!(
            session_runs.load(Ordering::SeqCst),
            0,
            "the session-level SubagentStop is NOT fired by the scoped call"
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

    /// #38: A `PostToolUse` Builtin hook returning `updated_tool_output` (the
    /// all-tools field) has it folded into the aggregate by `merge`. The outer
    /// `Some` is preserved (`!== void 0` semantics).
    #[tokio::test]
    async fn post_hook_updated_tool_output_reaches_aggregate() {
        struct RewriteAll;
        #[async_trait]
        impl BuiltinHookHandler for RewriteAll {
            fn id(&self) -> &str {
                "rewrite_all"
            }
            async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    response: Some(HookResponse {
                        updated_tool_output: Some(Some(serde_json::json!({ "x": "all" }))),
                        ..Default::default()
                    }),
                }
            }
        }
        let hook = HookDefinition {
            id: HookId::new(),
            name: "rewrite_all".into(),
            events: vec![HookEventType::PostToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "rewrite_all".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let (exec, _reg) = executor_with(hook, Arc::new(RewriteAll));
        let post = HookEvent::PostToolUse {
            // a NON-mcp tool: updated_tool_output applies for all tools
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({}),
            tool_output: serde_json::json!("original"),
            tool_use_id: ToolUseId::new(),
        };
        let agg = exec.execute(post, HookContext::default()).await;
        assert_eq!(
            agg.updated_tool_output,
            Some(Some(serde_json::json!({ "x": "all" }))),
            "the hook's updatedToolOutput must reach the aggregate",
        );
    }

    /// #40: a hook returning a top-level `terminal_sequence` has it folded into
    /// the aggregate by `merge` (latest wins); the consumer validates + emits.
    #[tokio::test]
    async fn hook_terminal_sequence_reaches_aggregate() {
        struct TermHook;
        #[async_trait]
        impl BuiltinHookHandler for TermHook {
            fn id(&self) -> &str {
                "term"
            }
            async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    response: Some(HookResponse {
                        terminal_sequence: Some("\u{0007}".into()),
                        ..Default::default()
                    }),
                }
            }
        }
        let hook = HookDefinition {
            id: HookId::new(),
            name: "term".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "term".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };
        let (exec, _reg) = executor_with(hook, Arc::new(TermHook));
        let pre = HookEvent::PreToolUse {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({}),
            tool_use_id: ToolUseId::new(),
        };
        let agg = exec.execute(pre, HookContext::default()).await;
        assert_eq!(agg.terminal_sequence.as_deref(), Some("\u{0007}"));
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
        assert_eq!(
            agg.progress.len(),
            1,
            "one progress event per matching hook"
        );
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
                allowed_env_vars: Vec::new(),
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
                model: None,
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
                agent_id: protocol::AgentId::new(),
                content: json!(r#"{"decision":"approve"}"#),
                usage: SubagentUsage::default(),
                total_tool_use_count: 0,
                total_duration_ms: 0,
                total_tokens: 0,
                assistant_message_count: 0,
                response_char_count: 0,
                last_request_id: None,
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
            recorded[0]
                .prompt
                .contains(r#""hook_event_name":"PreToolUse""#),
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
                continue_on_block: false,
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
            recorded[0]
                .prompt
                .contains(r#""hook_event_name":"PreToolUse""#),
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
