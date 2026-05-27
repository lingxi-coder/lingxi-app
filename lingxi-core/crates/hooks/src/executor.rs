//! Hook executor scaffold.
//!
//! M1.4 ships the [`BuiltinHookHandler`] trait and the [`HookExecutorImpl`]
//! that wires the registry to the four executor kinds. The `Builtin` arm is
//! fully functional; the `Http`, `Command`, and `Agent` arms are stubbed and
//! filled in by Plan 09.
//!
//! The HTTP executor consults the [`SsrfGuard`] before issuing any request;
//! the Command / Agent executors will use the corresponding traits in their
//! Plan 09 implementations.

use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use crate::registry::{HookContext, HookRegistry};
use crate::response::{AggregateHookResult, HookOutcome, HookResult};
use crate::ssrf_guard::SsrfGuard;
use async_trait::async_trait;
use lingxi_traits::{HttpTransport, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
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
    #[allow(dead_code)] // Used by the Plan 09 Command / Agent executor bodies.
    runtime: Arc<dyn RuntimeSpawner>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    ssrf_guard: SsrfGuard,
}

impl HookExecutorImpl {
    /// Build a new executor backed by the supplied registry, HTTP transport,
    /// and runtime spawner.
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
        }
    }

    /// Register a builtin handler. Subsequent hook definitions referencing
    /// `h.id()` via [`HookExecutor::Builtin`] will dispatch to this handler.
    pub fn register_builtin(&mut self, h: Arc<dyn BuiltinHookHandler>) {
        self.builtin_handlers.insert(h.id().into(), h);
    }

    /// Borrow the shared HTTP transport (used by Plan 09 HTTP executor body).
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
        let matched = reg.match_event(&event, &ctx);
        let mut agg = AggregateHookResult::default();
        for hook in matched {
            let result = self.execute_single(hook, &event, &ctx).await;
            Self::merge(&mut agg, hook, result);
            if matches!(agg.decision, Some(crate::response::HookDecision::Block)) {
                break;
            }
        }
        agg
    }

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
            HookExecutor::Http { url, .. } => {
                if self.ssrf_guard.check_url(url).is_err() {
                    return HookResult {
                        outcome: HookOutcome::Error,
                        stdout: String::new(),
                        stderr: "SSRF guard rejected url".into(),
                        exit_code: None,
                        response: None,
                    };
                }
                // Full impl: build HttpRequest, POST event JSON, parse the
                // body into a HookResponse. Stubbed for M1.4 — lands in
                // Plan 09 alongside the SSE / retry plumbing.
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    response: None,
                }
            }
            HookExecutor::Command { .. } | HookExecutor::Agent { .. } => {
                // Full impl: spawn process via the runtime spawner / fork
                // agent via the agent registry. Stubbed for M1.4 — lands in
                // Plan 09.
                HookResult {
                    outcome: HookOutcome::Success,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    response: None,
                }
            }
        }
    }

    #[allow(dead_code)]
    fn _constants_anchor() {}

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
