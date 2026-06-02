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
use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use crate::hook_payload::{
    parse_response, HookEventNamePost, HookEventNamePre, PostToolUsePayload, PreToolUsePayload,
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
        }
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

    /// Fire `event` and return the aggregated result of every matching hook.
    ///
    /// Hooks are evaluated in priority-descending order; processing stops
    /// early on the first `Block` decision.
    pub async fn execute(&self, event: HookEvent, ctx: HookContext) -> AggregateHookResult {
        let reg = self.registry.read().await;
        let matched: Vec<HookDefinition> =
            reg.match_event(&event, &ctx).into_iter().cloned().collect();
        drop(reg);
        let mut agg = AggregateHookResult::default();
        for hook in &matched {
            let result = self.execute_single(hook, &event, &ctx).await;
            Self::merge(&mut agg, hook, result);
            if matches!(agg.decision, Some(crate::response::HookDecision::Block)) {
                break;
            }
        }
        agg
    }

    #[allow(
        clippy::too_many_lines,
        reason = "arm dispatch fan-out — splitting hurts readability"
    )]
    async fn execute_single(
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
                // claude-code writes `jsonStringify(hookInput) + '\n'` to the
                // child's stdin then closes it (`hooks.ts:1006`/`1210`). The
                // trailing newline is load-bearing: a bash `read -r line`
                // returns exit 1 on EOF-before-delimiter without it.
                let pcmd = ProcessCommand {
                    command: command.clone(),
                    args: args.clone(),
                    cwd: cwd.clone().or_else(|| Some(ctx.cwd.clone())),
                    env: env.clone(),
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
/// Returns `None` for event variants the HTTP / Agent arms don't yet
/// support (everything except `PreToolUse` / `PostToolUse` in M5-06).
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
    }

    impl MockRunner {
        fn ok(output: ProcessOutput) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Ok(output))),
                recorded_stdin: Mutex::new(None),
            })
        }
        fn err(e: ProcessError) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(Err(e))),
                recorded_stdin: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            *self.recorded_stdin.lock().unwrap() = cmd.inner().stdin.clone();
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
}
