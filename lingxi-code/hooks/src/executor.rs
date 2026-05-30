//! Hook executor.
//!
//! M1.4 shipped [`BuiltinHookHandler`] and the [`HookExecutorImpl`] with the
//! `Builtin` arm fully wired and the `Http` / `Command` / `Agent` arms
//! stubbed. M5-06 fills in the HTTP and Agent arms (delegating to
//! [`crate::http_executor::HttpExecutor`] / [`crate::agent_executor::AgentExecutor`])
//! and leaves the Command arm as a documented stub pending stdin support
//! on `ProcessRunner` (deferred to a future plan).

use crate::agent_executor::{AgentExecutionSignal, AgentExecutor};
use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use crate::hook_payload::{
    HookEventNamePost, HookEventNamePre, PostToolUsePayload, PreToolUsePayload,
};
use crate::http_executor::{HttpExecutionSignal, HttpExecutor};
use crate::registry::{HookContext, HookRegistry};
use crate::response::{AggregateHookResult, HookOutcome, HookResult};
use crate::ssrf_guard::SsrfGuard;
use async_trait::async_trait;
use lingxi_traits::subagent_spawn::SubagentSpawner;
use lingxi_traits::{HttpTransport, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

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
    #[allow(dead_code)] // Command arm uses ProcessRunner not RuntimeSpawner — deferred.
    runtime: Arc<dyn RuntimeSpawner>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    ssrf_guard: SsrfGuard,
    /// Optional subagent spawner — attached via [`Self::with_agent_spawner`].
    /// When `None`, the `Agent` arm returns `HookOutcome::Error` with
    /// `stderr: "Hook {id} failed: agent executor not wired"`. M5-06.
    agent_spawner: Option<Arc<dyn SubagentSpawner>>,
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
            HookExecutor::Command { .. } => {
                // M5-06 plan deviation: `ProcessRunner` doesn't yet support
                // stdin payload + raw command/args/env/cwd (only sandboxed
                // commands). Command-arm hooks are deferred to a future plan
                // that extends ProcessRunner. The stub returns a structured
                // "not wired" error so the orchestrator surfaces it rather
                // than silently allowing.
                HookResult {
                    outcome: HookOutcome::Error,
                    stdout: String::new(),
                    stderr: format!(
                        "Hook {} failed: command executor not yet wired (stdin support deferred)",
                        hook.id
                    ),
                    exit_code: None,
                    response: None,
                }
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

/// Emit arm-level telemetry for an HTTP signal (SSRF / timeout). Other
/// telemetry (`HOOK_PRE_*` / `HOOK_POST_*`) is fired by the orchestrator's
/// `dispatch_tool_with_hooks` (M5-06 Task 14).
fn emit_http_signal(hook: &HookDefinition, signal: &HttpExecutionSignal, timeout: Duration) {
    match signal {
        HttpExecutionSignal::SsrfBlocked(reason) => {
            tracing::info!(
                event = lingxi_telemetry::tengu::orchestrator::HOOK_HTTP_SKIPPED_SSRF,
                hook_id = %hook.id,
                reason = %reason,
            );
        }
        HttpExecutionSignal::TimedOut => {
            // hook timeout bounded to seconds — u128 ms cannot exceed u64::MAX
            #[allow(clippy::cast_possible_truncation)]
            let timeout_ms = timeout.as_millis() as u64;
            tracing::info!(
                event = lingxi_telemetry::tengu::orchestrator::HOOK_TIMEOUT,
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
            event = lingxi_telemetry::tengu::orchestrator::HOOK_TIMEOUT,
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
