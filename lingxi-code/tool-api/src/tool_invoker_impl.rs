//! `ToolInvoker` impl wrapping a `ToolRegistry`.
//!
//! `RegistryToolInvoker` is the production adapter `AgentTool` uses to hand
//! a tool-dispatch surface to the spawner. The wrapper stores
//! `Arc<ToolRegistry>` so the recursion-lock invariant (parent + child
//! subagent share the *same* `Arc`) survives across the spawn boundary —
//! a property asserted via `Arc::ptr_eq` in M4-05 wiring tests.

use crate::registry::ToolRegistry;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use traits::permission_gate::{PermissionDecision, PermissionGate};
use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

/// Wraps an `Arc<ToolRegistry>` as a `dyn ToolInvoker`.
///
/// Cheap to construct; clones share the same registry `Arc`.
pub struct RegistryToolInvoker {
    registry: Arc<ToolRegistry>,
    /// Permission gate consulted before every dispatch (enforcement 3b).
    /// `None` (the default) preserves the legacy always-dispatch behavior for
    /// tests / hosts without enforcement wired. When `Some`, this is the SAME
    /// gate the main loop consults — so subagent and teammate tool calls are
    /// now subject to the same allow/deny rules, closing the bypass.
    gate: Option<Arc<dyn PermissionGate>>,
}

impl RegistryToolInvoker {
    /// Construct an invoker bound to `registry`. The `Arc` is stored
    /// verbatim — `Arc::ptr_eq` between this invoker's clone and the
    /// parent's clone returns `true`.
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            gate: None,
        }
    }

    /// Attach the permission gate consulted before each dispatch (3b). Without
    /// it, dispatch is unconditional (legacy behavior).
    #[must_use]
    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// Borrow the underlying registry `Arc`.
    #[must_use]
    pub fn registry_arc(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }
}

#[async_trait]
impl ToolInvoker for RegistryToolInvoker {
    async fn invoke(
        &self,
        name: &str,
        input: Value,
        ctx: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError> {
        let tool = self
            .registry
            .find_by_name(name)
            .ok_or_else(|| ToolInvokerError::NotFound(name.to_string()))?;

        // Permission gate (enforcement 3b). The subagent/teammate dispatch
        // surface now consults the same gate as the main loop — previously it
        // dispatched any registered tool unconditionally (the bypass). A `Deny`
        // is surfaced as `Internal` (the frozen `ToolInvokerError` carries no
        // `Denied` variant; the subagent loop renders any `Err` as an
        // `is_error` tool_result the model can recover from). Checked AFTER
        // `find_by_name` so an unknown tool stays `NotFound`, and BEFORE the
        // borrow of `input` is moved into `call`.
        if let Some(gate) = &self.gate {
            if let PermissionDecision::Deny { reason } = gate.check(name, &input).await {
                return Err(ToolInvokerError::Internal(reason));
            }
        }

        // Synthesize a minimal ToolUseContext. The recursion-lock invariant
        // requires `subagent_registry` to carry the SAME Arc<ToolRegistry>
        // this invoker wraps (so a recursive AgentTool call inside the
        // dispatched tool reuses the same registry — no fresh Arc).
        let tool_use_ctx = crate::context::ToolUseContext {
            options: crate::context::ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "subagent".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: ctx.parent_agent_id,
            content_replacement_state: None,
            session: None,
            subagent_registry: Some(self.registry.clone()),
        };

        // Drop the progress receiver immediately — production tools tolerate
        // a closed progress channel, and we don't surface progress here.
        let (progress_tx, _progress_rx) =
            tokio::sync::mpsc::channel::<crate::progress::ToolProgress>(8);

        let result = tool
            .call(input, tool_use_ctx, progress_tx)
            .await
            .map_err(|e| match e {
                crate::tool_trait::ToolError::InvalidInput(s) => ToolInvokerError::InvalidInput(s),
                other => ToolInvokerError::Internal(format!("{other}")),
            })?;

        Ok(result.data)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
#[allow(clippy::option_option)]
mod tests {
    use super::*;
    use crate::context::ToolUseContext;
    use crate::progress::ToolProgressSender;
    use crate::tool_trait::{
        DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
        ToolStaticContext, ValidationError,
    };
    use async_trait::async_trait;
    use once_cell::sync::Lazy;
    use permission::result::PermissionMetadata;
    use permission::{PermissionDecisionReason, PermissionResult};
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    fn allow_for_tests() -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "test".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    static ECHO_INPUT_SCHEMA: Lazy<serde_json::Value> =
        Lazy::new(|| json!({ "type": "object", "additionalProperties": true }));

    /// Minimal Tool impl returning input under {"echo": <input>}.
    /// Lives entirely under `#[cfg(test)]`.
    struct TestEchoTool;

    #[async_trait]
    impl Tool for TestEchoTool {
        fn name(&self) -> &str {
            "TestEcho"
        }
        fn input_schema(&self) -> &serde_json::Value {
            &ECHO_INPUT_SCHEMA
        }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
            _ctx: &ToolUseContext,
        ) -> PermissionResult {
            allow_for_tests()
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "echo".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            "echo tool".into()
        }
        async fn call(
            &self,
            input: serde_json::Value,
            _ctx: ToolUseContext,
            _progress_tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({ "echo": input }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    static RECORDING_INPUT_SCHEMA: Lazy<serde_json::Value> =
        Lazy::new(|| json!({ "type": "object" }));

    /// Tool fixture that records the ToolUseContext.subagent_registry it
    /// receives so tests can introspect Arc identity.
    struct RecordingTool {
        captured: Arc<StdMutex<Option<Option<Arc<ToolRegistry>>>>>,
    }

    #[async_trait]
    impl Tool for RecordingTool {
        fn name(&self) -> &str {
            "RecordingTool"
        }
        fn input_schema(&self) -> &serde_json::Value {
            &RECORDING_INPUT_SCHEMA
        }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _: &serde_json::Value) -> bool {
            true
        }
        async fn validate_input(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }
        async fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> PermissionResult {
            allow_for_tests()
        }
        async fn description(&self, _: &serde_json::Value, _: &DescriptionOptions) -> String {
            "rec".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            "rec".into()
        }
        async fn call(
            &self,
            _: serde_json::Value,
            ctx: ToolUseContext,
            _: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.subagent_registry.clone());
            Ok(ToolCallResult {
                data: json!({}),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    fn registry_with_echo() -> Arc<ToolRegistry> {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(TestEchoTool));
        Arc::new(r)
    }

    // ──── existing M4-05 test (preserved) ──────────────
    #[test]
    fn registry_invoker_preserves_arc_identity() {
        let r = Arc::new(ToolRegistry::new());
        let inv = RegistryToolInvoker::new(r.clone());
        assert!(Arc::ptr_eq(&r, inv.registry_arc()));
    }

    // ──── Task 9: dispatch returns tool's data ──────────────
    #[tokio::test]
    async fn registry_invoker_routes_to_tool_call_and_returns_data() {
        let registry = registry_with_echo();
        let invoker = RegistryToolInvoker::new(registry.clone());

        let input = json!({ "hello": "world", "n": 42 });
        let ctx = SubagentInvocationContext {
            parent_agent_id: None,
        };

        let result = invoker
            .invoke("TestEcho", input.clone(), ctx)
            .await
            .expect("dispatch succeeds");

        assert_eq!(
            result,
            json!({ "echo": { "hello": "world", "n": 42 } }),
            "invoker returns the tool's data verbatim"
        );
    }

    #[tokio::test]
    async fn registry_invoker_unknown_tool_surfaces_not_found() {
        let registry = registry_with_echo();
        let invoker = RegistryToolInvoker::new(registry);
        let result = invoker
            .invoke(
                "NotARealTool",
                json!({}),
                SubagentInvocationContext {
                    parent_agent_id: None,
                },
            )
            .await;
        match result {
            Err(ToolInvokerError::NotFound(name)) => assert_eq!(name, "NotARealTool"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // ──── Task 10: dispatch preserves subagent_registry Arc identity ──────────────
    #[tokio::test]
    async fn registry_invoker_preserves_subagent_registry_arc_into_tool_use_ctx() {
        let captured: Arc<StdMutex<Option<Option<Arc<ToolRegistry>>>>> =
            Arc::new(StdMutex::new(None));

        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(RecordingTool {
            captured: captured.clone(),
        }));
        let parent_registry = Arc::new(registry);

        let invoker = RegistryToolInvoker::new(parent_registry.clone());
        invoker
            .invoke(
                "RecordingTool",
                json!({}),
                SubagentInvocationContext {
                    parent_agent_id: None,
                },
            )
            .await
            .expect("dispatch ok");

        let captured = captured.lock().unwrap();
        let inner = captured.as_ref().expect("RecordingTool::call ran");
        let registry_in_ctx = inner.as_ref().expect("subagent_registry was Some");
        assert!(
            Arc::ptr_eq(&parent_registry, registry_in_ctx),
            "RegistryToolInvoker must thread the parent Arc<ToolRegistry> into ToolUseContext.subagent_registry verbatim — this preserves the M4-05 recursion-lock contract across the dispatch boundary"
        );
    }

    // ──── enforcement 3b: permission gate before dispatch ──────────────
    use traits::permission_gate::{PermissionDecision as GateDecision, PermissionGate as Gate};

    /// Gate that returns a fixed decision and records each tool name it sees.
    struct FixedGate {
        decision: GateDecision,
        seen: Arc<StdMutex<Vec<String>>>,
    }
    #[async_trait]
    impl Gate for FixedGate {
        async fn check(&self, name: &str, _input: &Value) -> GateDecision {
            self.seen.lock().unwrap().push(name.to_string());
            self.decision.clone()
        }
    }

    fn no_ctx() -> SubagentInvocationContext {
        SubagentInvocationContext {
            parent_agent_id: None,
        }
    }

    #[tokio::test]
    async fn gate_deny_blocks_dispatch_and_surfaces_reason() {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let gate = Arc::new(FixedGate {
            decision: GateDecision::Deny {
                reason: "denied by permission rule Bash".into(),
            },
            seen: seen.clone(),
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        match invoker
            .invoke("TestEcho", json!({ "a": 1 }), no_ctx())
            .await
        {
            Err(ToolInvokerError::Internal(reason)) => {
                assert!(
                    reason.contains("denied by permission rule Bash"),
                    "got {reason}"
                );
            }
            other => panic!("expected Internal(deny), got {other:?}"),
        }
        // Gate was consulted; the tool was NOT run (echo would have returned data).
        assert_eq!(seen.lock().unwrap().as_slice(), ["TestEcho"]);
    }

    #[tokio::test]
    async fn gate_allow_dispatches_to_tool() {
        let gate = Arc::new(FixedGate {
            decision: GateDecision::Allow,
            seen: Arc::new(StdMutex::new(Vec::new())),
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        let out = invoker
            .invoke("TestEcho", json!({ "k": "v" }), no_ctx())
            .await
            .expect("allow dispatches");
        assert_eq!(out, json!({ "echo": { "k": "v" } }));
    }

    #[tokio::test]
    async fn no_gate_dispatches_unconditionally_legacy() {
        // Default `new` (no gate) preserves the pre-3b always-dispatch behavior.
        let invoker = RegistryToolInvoker::new(registry_with_echo());
        let out = invoker
            .invoke("TestEcho", json!({}), no_ctx())
            .await
            .expect("legacy dispatch");
        assert_eq!(out, json!({ "echo": {} }));
    }

    #[tokio::test]
    async fn unknown_tool_is_not_found_before_gate_runs() {
        // `find_by_name` precedes the gate → an unknown tool is NotFound and
        // the gate is never consulted (no spurious deny on a non-existent tool).
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let gate = Arc::new(FixedGate {
            decision: GateDecision::Deny {
                reason: "should not run".into(),
            },
            seen: seen.clone(),
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        let result = invoker.invoke("NotARealTool", json!({}), no_ctx()).await;
        assert!(matches!(result, Err(ToolInvokerError::NotFound(_))));
        assert!(
            seen.lock().unwrap().is_empty(),
            "gate not consulted for unknown tool"
        );
    }
}
