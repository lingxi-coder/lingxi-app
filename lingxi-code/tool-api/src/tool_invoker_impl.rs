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
use traits::permission_gate::PermissionGate;
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
        self.invoke_with_workspace_lease(name, input, ctx, None).await
    }

    async fn invoke_with_workspace_lease(
        &self,
        name: &str,
        mut input: Value,
        ctx: SubagentInvocationContext,
        workspace_lease_token: Option<u64>,
    ) -> Result<Value, ToolInvokerError> {
        let tool = self
            .registry
            .find_by_name(name)
            .ok_or_else(|| ToolInvokerError::NotFound(name.to_string()))?;

        // BASH-18 `coerceInput` (claude-code 2.1.238 BIN off **294282716**): the
        // tool's own pre-validation normalization of the model's raw arguments.
        // The oracle applies it once, at the TOP of `checkPermissionsAndCallTool`,
        // and the rewritten value is what the permission check and `call` both
        // see — so the subagent dispatch surface must coerce here too, BEFORE the
        // gate below, or a subagent's `Bash{timeout_ms}` would silently lose its
        // timeout where the main loop honours it. `None` for every tool but
        // `Bash` ⇒ strict no-op.
        //
        // (This surface still has no JSON-schema gate and no `validate_input`
        // call — a pre-existing, separately-tracked divergence from the main
        // loop; the coercion is correct with or without them.)
        if let Some(coerced) = tool.coerce_input(&input) {
            input = coerced.input;
        }

        // Permission gate (enforcement 3b). The subagent/teammate dispatch
        // surface now consults the same gate as the main loop — previously it
        // dispatched any registered tool unconditionally (the bypass). A `Deny`
        // is surfaced as `Internal`, while a headless auto-mode breaker trip is
        // preserved as `Abort` so the subagent loop terminates instead of
        // rendering a recoverable `tool_result`. Checked AFTER `find_by_name`
        // so an unknown tool stays `NotFound`, and BEFORE the borrow of `input`
        // is moved into `call`.
        if let Some(gate) = &self.gate {
            // Attribute the prompt to the originating worker (claude-code 2.1.186:
            // a background subagent's permission prompt surfaces in the main
            // session with a `● @name` badge). Attach the worker identity only for
            // a NAMED worker that may surface prompts (`can_show_permission_prompts`)
            // — a one-shot subagent (no display name) carries `None`, leaving the
            // prompt unattributed exactly as before.
            let worker = (ctx.can_show_permission_prompts)
                .then(|| ctx.agent_name.clone())
                .flatten()
                .map(|name| traits::permission_gate::PromptWorker {
                    name,
                    team: ctx.team_name.clone(),
                    is_async: ctx.is_async,
                });
            // Use the richer `check_with_context` (not `check_with_worker`) so the
            // subagent path is byte-faithful to the main loop's: it carries the
            // REAL `tool_use_id` of the dispatching call (so a stdio
            // `can_use_tool` request is correlatable) AND applies the host/policy
            // `updatedInput` rewrite to the input the tool actually runs with —
            // mirroring the turn-loop Ask arm. The default `check_with_context`
            // delegates to `check_with_worker` and maps `Allow`→`Allow{None}`, so
            // a gate that only overrides `check_with_worker` (or `check`) is
            // unchanged. Behavior is identical when `updated_input` is `None`.
            let check_ctx = traits::permission_gate::PermissionCheckContext {
                worker,
                tool_use_id: ctx.tool_use_id.clone(),
                requires_user_interaction: tool.requires_user_interaction(),
                // Per-call permission-mode override (claude-code 2.1.207 Agent
                // `mode` → the child's `toolPermissionContext.mode`): a
                // `mode:"plan"` subagent's dispatch authorizes under Plan so
                // mutations are gated while reads stay frictionless. `None` for the
                // main thread / a spawn with no override.
                mode_override: ctx.mode_override.clone(),
                is_non_interactive_session: ctx.is_non_interactive_session,
                workspace_lease_token,
                ..Default::default()
            };
            match gate
                .check_with_context_or_abort(name, &input, &check_ctx)
                .await
            {
                Ok(traits::permission_gate::PermissionOutcome::Allow { updated_input, .. }) => {
                    if let Some(u) = updated_input {
                        input = u;
                    }
                }
                Ok(traits::permission_gate::PermissionOutcome::Deny { reason }) => {
                    return Err(ToolInvokerError::Internal(reason));
                }
                Err(abort) => return Err(ToolInvokerError::Abort(abort.message)),
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
                // The DISPATCHING subagent's own resolved model (claude-code
                // `runAgent.ts:678` seeds each child's `mainLoopModel:
                // resolvedAgentModel`), so a recursive `Agent` tool call reads the
                // IMMEDIATE parent's model via `toolUseContext.options.mainLoopModel`
                // (claude `AgentTool.tsx:418`). Falls back to the legacy `"subagent"`
                // placeholder when the dispatching runner wired no parent model.
                main_loop_model: ctx
                    .parent_model
                    .clone()
                    .unwrap_or_else(|| "subagent".into()),
                model_profile: ctx.parent_model_profile.clone(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                // Use the effective owner mode captured by the runner. It is
                // deliberately separate from `is_async`: scheduled work can
                // launch a synchronous child that must remain non-interactive.
                is_non_interactive_session: ctx.is_non_interactive_session,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: ctx.parent_agent_id,
            // Swarm identity threaded from the dispatching agent (claude-code
            // `getAgentName()` / `getTeammateContext()?.teamName`): an in-process
            // teammate's dispatched tools now see the teammate's DISPLAY name +
            // team name, so the swarm-only `TaskUpdate` side-effects (auto-owner,
            // owner-change mailbox notification) and `getTaskListId()` key on
            // them. `None` for one-shot subagents / the leader / main thread.
            agent_name: ctx.agent_name,
            team_name: ctx.team_name,
            content_replacement_state: None,
            session: None,
            subagent_registry: Some(self.registry.clone()),
            cancel: None,
            fork_parent_system_prompt: None,
            // Per-agent cwd (worktree isolation / explicit cwd): the dispatched
            // tools operate here instead of the shared session workspace.
            cwd: ctx.cwd,
            // Recursion depth of the DISPATCHING agent (claude `agentContext.depth`)
            // → so a recursive `Agent` call inside this tool computes the child's
            // depth and the resolver can gate `Agent` at `depth < 5`.
            depth: ctx.depth,
            observer: ctx.observer,
            // Subagent tool edits are not checkpointed in v1 (the main-loop
            // turn wires `file_history`; subagent contexts do not carry it).
            file_history: None,
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
    struct TestEchoTool {
        requires_user_interaction: bool,
    }

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
        fn requires_user_interaction(&self) -> bool {
            self.requires_user_interaction
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
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
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
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    fn registry_with_echo() -> Arc<ToolRegistry> {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(TestEchoTool {
            requires_user_interaction: false,
        }));
        Arc::new(r)
    }

    fn registry_with_interactive_echo() -> Arc<ToolRegistry> {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(TestEchoTool {
            requires_user_interaction: true,
        }));
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
        let ctx = no_ctx();

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
        let result = invoker.invoke("NotARealTool", json!({}), no_ctx()).await;
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
            .invoke("RecordingTool", json!({}), no_ctx())
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

    // ──── swarm identity: SubagentInvocationContext name/team → ToolUseContext ────

    /// Tool fixture that records the `(agent_name, team_name)` the dispatched
    /// `ToolUseContext` carries, so a test can assert the swarm identity flows
    /// through the invoker (claude-code `getAgentName()` /
    /// `getTeammateContext()?.teamName`).
    struct NameRecordingTool {
        captured: Arc<StdMutex<Option<(Option<String>, Option<String>)>>>,
    }
    #[async_trait]
    impl Tool for NameRecordingTool {
        fn name(&self) -> &str {
            "NameRecordingTool"
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
            *self.captured.lock().unwrap() = Some((ctx.agent_name.clone(), ctx.team_name.clone()));
            Ok(ToolCallResult {
                data: json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    /// The `agent_name` / `team_name` carried by the `SubagentInvocationContext`
    /// must reach the dispatched tool's `ToolUseContext` verbatim — this is what
    /// makes the merged swarm `TaskUpdate` auto-owner / `getTaskListId` logic
    /// actually fire for an in-process teammate (it keys on these).
    #[tokio::test]
    async fn registry_invoker_threads_swarm_identity_into_tool_use_ctx() {
        let captured: Arc<StdMutex<Option<(Option<String>, Option<String>)>>> =
            Arc::new(StdMutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(NameRecordingTool {
            captured: captured.clone(),
        }));
        let invoker = RegistryToolInvoker::new(Arc::new(registry));

        invoker
            .invoke(
                "NameRecordingTool",
                json!({}),
                SubagentInvocationContext {
                    parent_agent_id: None,
                    agent_name: Some("researcher".to_string()),
                    team_name: Some("alpha".to_string()),
                    is_async: false,
                    is_non_interactive_session: false,
                    can_show_permission_prompts: true,
                    cwd: None,
                    tool_use_id: None,
                    depth: 0,
                    observer: None,
                    parent_model: None,
                    parent_model_profile: None,
                    mode_override: None,
                },
            )
            .await
            .expect("dispatch ok");

        let captured = captured.lock().unwrap();
        let (agent_name, team_name) = captured.as_ref().expect("NameRecordingTool::call ran");
        assert_eq!(
            agent_name.as_deref(),
            Some("researcher"),
            "the teammate DISPLAY name reaches ToolUseContext.agent_name (getAgentName())"
        );
        assert_eq!(
            team_name.as_deref(),
            Some("alpha"),
            "the team name reaches ToolUseContext.team_name (getTeammateContext()?.teamName)"
        );
    }

    /// Tool fixture that records the `main_loop_model` the dispatched
    /// `ToolUseContext` carries, so a test can assert the DISPATCHING subagent's
    /// resolved model reaches a recursive tool call (claude-code
    /// `AgentTool.tsx:418` `toolUseContext.options.mainLoopModel`).
    struct ModelRecordingTool {
        captured: Arc<StdMutex<Option<(String, Option<String>)>>>,
    }
    #[async_trait]
    impl Tool for ModelRecordingTool {
        fn name(&self) -> &str {
            "ModelRecordingTool"
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
            *self.captured.lock().unwrap() = Some((
                ctx.options.main_loop_model.clone(),
                ctx.options.model_profile.clone(),
            ));
            Ok(ToolCallResult {
                data: json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    /// The DISPATCHING subagent's own resolved model
    /// ([`SubagentInvocationContext::parent_model`]) must reach the dispatched
    /// tool's `ToolUseContext.options.main_loop_model` — this is what makes a
    /// NESTED `Agent` tool call resolve its child's model against the IMMEDIATE
    /// parent's model (claude-code `runAgent.ts:678` → `AgentTool.tsx:418`). When
    /// unset it falls back to the legacy `"subagent"` placeholder.
    #[tokio::test]
    async fn registry_invoker_threads_parent_model_into_main_loop_model() {
        let captured: Arc<StdMutex<Option<(String, Option<String>)>>> =
            Arc::new(StdMutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(ModelRecordingTool {
            captured: captured.clone(),
        }));
        let invoker = RegistryToolInvoker::new(Arc::new(registry));

        let mut ctx = no_ctx();
        ctx.parent_model = Some("claude-sonnet-5".to_string());
        ctx.parent_model_profile = Some("anthropic".to_string());
        invoker
            .invoke("ModelRecordingTool", json!({}), ctx)
            .await
            .expect("dispatch ok");
        assert_eq!(
            captured.lock().unwrap().as_ref(),
            Some(&("claude-sonnet-5".to_string(), Some("anthropic".to_string()))),
            "the dispatching subagent's model and provider reach ToolUseContext options"
        );

        // Unset parent_model ⇒ the legacy placeholder (byte-identical fallback).
        *captured.lock().unwrap() = None;
        invoker
            .invoke("ModelRecordingTool", json!({}), no_ctx())
            .await
            .expect("dispatch ok");
        assert_eq!(
            captured.lock().unwrap().as_ref(),
            Some(&("subagent".to_string(), None))
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
            agent_name: None,
            team_name: None,
            is_async: false,
            is_non_interactive_session: traits::session_flags::effective_non_interactive_session(),
            can_show_permission_prompts: false,
            cwd: None,
            tool_use_id: None,
            depth: 0,
            observer: None,
            parent_model: None,
            parent_model_profile: None,
            mode_override: None,
        }
    }

    struct SessionModeRecordingTool {
        captured: Arc<StdMutex<Option<bool>>>,
    }

    #[async_trait]
    impl Tool for SessionModeRecordingTool {
        fn name(&self) -> &str {
            "SessionModeRecordingTool"
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
            "record session mode".into()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            "record session mode".into()
        }
        async fn call(
            &self,
            _: serde_json::Value,
            ctx: ToolUseContext,
            _: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            *self.captured.lock().unwrap() = Some(ctx.options.is_non_interactive_session);
            Ok(ToolCallResult {
                data: json!({}),
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
        fn interrupt_behavior(&self, _input: &serde_json::Value) -> InterruptBehavior {
            InterruptBehavior::Cancel
        }
    }

    #[tokio::test]
    async fn synchronous_subagent_tools_inherit_scheduled_headless_session_mode() {
        let captured = Arc::new(StdMutex::new(None));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(SessionModeRecordingTool {
            captured: captured.clone(),
        }));
        let invoker = RegistryToolInvoker::new(Arc::new(registry));

        traits::session_flags::scope_non_interactive_session(true, async {
            invoker
                .invoke("SessionModeRecordingTool", json!({}), no_ctx())
                .await
                .expect("dispatch ok");
        })
        .await;

        assert_eq!(
            *captured.lock().unwrap(),
            Some(true),
            "a synchronous child must inherit its scheduled-headless owner's mode"
        );
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

    /// Gate that records the [`PromptWorker`] handed to `check_with_worker`.
    struct WorkerRecordingGate {
        seen: Arc<StdMutex<Option<Option<traits::permission_gate::PromptWorker>>>>,
    }
    #[async_trait]
    impl Gate for WorkerRecordingGate {
        async fn check(&self, _name: &str, _input: &Value) -> GateDecision {
            // `check_with_worker`'s default would land here; record None so a
            // missing override is visible. The real override is below.
            *self.seen.lock().unwrap() = Some(None);
            GateDecision::Allow
        }
        async fn check_with_worker(
            &self,
            _name: &str,
            _input: &Value,
            worker: Option<traits::permission_gate::PromptWorker>,
        ) -> GateDecision {
            *self.seen.lock().unwrap() = Some(worker);
            GateDecision::Allow
        }
    }

    fn named_ctx(can_show: bool) -> SubagentInvocationContext {
        SubagentInvocationContext {
            parent_agent_id: None,
            agent_name: Some("researcher".to_string()),
            team_name: Some("alpha".to_string()),
            is_async: true,
            is_non_interactive_session: true,
            can_show_permission_prompts: can_show,
            cwd: None,
            tool_use_id: None,
            depth: 0,
            observer: None,
            parent_model: None,
            parent_model_profile: None,
            mode_override: None,
        }
    }

    #[tokio::test]
    async fn worker_identity_threads_to_gate_when_named_and_eligible() {
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(WorkerRecordingGate { seen: seen.clone() });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        invoker
            .invoke("TestEcho", json!({}), named_ctx(true))
            .await
            .expect("allow dispatches");
        let worker = seen.lock().unwrap().clone().expect("gate consulted");
        let worker = worker.expect("a named, prompt-eligible worker is attributed");
        assert_eq!(worker.name, "researcher");
        assert_eq!(worker.team.as_deref(), Some("alpha"));
        assert!(worker.is_async);
    }

    #[tokio::test]
    async fn no_worker_attribution_for_unnamed_or_ineligible() {
        // Unnamed (one-shot subagent) → no attribution.
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(WorkerRecordingGate { seen: seen.clone() });
        RegistryToolInvoker::new(registry_with_echo())
            .with_gate(gate)
            .invoke("TestEcho", json!({}), no_ctx())
            .await
            .expect("ok");
        assert_eq!(seen.lock().unwrap().clone().expect("consulted"), None);

        // Named but NOT prompt-eligible (`can_show_permission_prompts = false`)
        // → no worker chrome.
        let seen2 = Arc::new(StdMutex::new(None));
        let gate2 = Arc::new(WorkerRecordingGate {
            seen: seen2.clone(),
        });
        RegistryToolInvoker::new(registry_with_echo())
            .with_gate(gate2)
            .invoke("TestEcho", json!({}), named_ctx(false))
            .await
            .expect("ok");
        assert_eq!(seen2.lock().unwrap().clone().expect("consulted"), None);
    }

    /// Gate that overrides `check_with_context` to record the
    /// [`PermissionCheckContext`] it sees and return a fixed
    /// [`PermissionOutcome`] — so a test can assert the subagent dispatch path
    /// threads the REAL `tool_use_id` AND honors a host `updatedInput` rewrite
    /// (the parity gap the stdio `can_use_tool` flow needs), mirroring the main
    /// loop's Ask arm.
    struct ContextRecordingGate {
        seen: Arc<StdMutex<Option<traits::permission_gate::PermissionCheckContext>>>,
        outcome: traits::permission_gate::PermissionOutcome,
    }
    #[async_trait]
    impl Gate for ContextRecordingGate {
        async fn check(&self, _name: &str, _input: &Value) -> GateDecision {
            // Must not be reached — the override below intercepts the dispatch.
            GateDecision::Allow
        }
        async fn check_with_context(
            &self,
            _name: &str,
            _input: &Value,
            ctx: &traits::permission_gate::PermissionCheckContext,
        ) -> traits::permission_gate::PermissionOutcome {
            *self.seen.lock().unwrap() = Some(ctx.clone());
            self.outcome.clone()
        }
    }

    struct AbortGate {
        seen: Arc<StdMutex<Option<traits::permission_gate::PermissionCheckContext>>>,
    }

    #[async_trait]
    impl Gate for AbortGate {
        async fn check(&self, _name: &str, _input: &Value) -> GateDecision {
            panic!("abort-aware dispatch must not fall back to the legacy gate method")
        }

        async fn check_with_context_or_abort(
            &self,
            _name: &str,
            _input: &Value,
            ctx: &traits::permission_gate::PermissionCheckContext,
        ) -> Result<
            traits::permission_gate::PermissionOutcome,
            traits::permission_gate::PermissionAbort,
        > {
            *self.seen.lock().unwrap() = Some(ctx.clone());
            Err(traits::permission_gate::PermissionAbort {
                message: "Agent aborted: too many classifier denials in headless mode".into(),
            })
        }
    }

    fn ctx_with_tool_use_id(id: &str) -> SubagentInvocationContext {
        SubagentInvocationContext {
            parent_agent_id: None,
            agent_name: Some("researcher".to_string()),
            team_name: Some("alpha".to_string()),
            is_async: true,
            is_non_interactive_session: true,
            can_show_permission_prompts: true,
            cwd: None,
            tool_use_id: Some(id.to_string()),
            depth: 0,
            observer: None,
            parent_model: None,
            parent_model_profile: None,
            mode_override: None,
        }
    }

    #[tokio::test]
    async fn dispatch_threads_real_tool_use_id_and_worker_into_context_gate() {
        // The dispatching call's tool_use block id (and the worker attribution)
        // must reach the gate's PermissionCheckContext so a subagent's stdio
        // `can_use_tool` request is byte-faithful (claude-code
        // `createCanUseTool(toolUseID)`), not a freshly minted id.
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(ContextRecordingGate {
            seen: seen.clone(),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        invoker
            .invoke(
                "TestEcho",
                json!({ "a": 1 }),
                ctx_with_tool_use_id("toolu_abc123"),
            )
            .await
            .expect("allow dispatches");
        let ctx = seen
            .lock()
            .unwrap()
            .clone()
            .expect("context gate consulted");
        assert_eq!(
            ctx.tool_use_id.as_deref(),
            Some("toolu_abc123"),
            "the dispatching call's real tool_use_id reaches PermissionCheckContext.tool_use_id"
        );
        assert!(!ctx.requires_user_interaction);
        let worker = ctx
            .worker
            .expect("a named, prompt-eligible worker is attributed");
        assert_eq!(worker.name, "researcher");
        assert_eq!(worker.team.as_deref(), Some("alpha"));
        assert!(worker.is_async);
    }

    #[tokio::test]
    async fn dispatch_preserves_permission_abort_as_terminal_error() {
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(AbortGate { seen: seen.clone() });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);

        let error = invoker
            .invoke("TestEcho", json!({}), ctx_with_tool_use_id("toolu_abort"))
            .await
            .expect_err("a permission abort must stop before tool dispatch");

        assert!(matches!(
            error,
            ToolInvokerError::Abort(ref message)
                if message == "Agent aborted: too many classifier denials in headless mode"
        ));
        let ctx = seen.lock().unwrap().clone().expect("gate consulted");
        assert!(ctx.is_non_interactive_session);
        assert_eq!(ctx.tool_use_id.as_deref(), Some("toolu_abort"));
    }

    #[tokio::test]
    async fn dispatch_threads_tool_interaction_requirement_into_context_gate() {
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(ContextRecordingGate {
            seen: seen.clone(),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        RegistryToolInvoker::new(registry_with_interactive_echo())
            .with_gate(gate)
            .invoke("TestEcho", json!({}), ctx_with_tool_use_id("toolu_ui"))
            .await
            .expect("allow dispatches");

        let ctx = seen
            .lock()
            .unwrap()
            .clone()
            .expect("context gate consulted");
        assert!(ctx.requires_user_interaction);
    }

    #[tokio::test]
    async fn dispatch_threads_mode_override_into_context_gate() {
        // A spawned subagent's effective permission mode (claude-code 2.1.207
        // Agent `mode` → the child's `toolPermissionContext.mode`) must reach the
        // gate's PermissionCheckContext so the dispatch authorizes under that mode.
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(ContextRecordingGate {
            seen: seen.clone(),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        let mut ctx = ctx_with_tool_use_id("toolu_plan");
        ctx.mode_override = Some("plan".to_string());
        invoker
            .invoke("TestEcho", json!({ "a": 1 }), ctx)
            .await
            .expect("allow dispatches");
        let seen = seen
            .lock()
            .unwrap()
            .clone()
            .expect("context gate consulted");
        assert_eq!(
            seen.mode_override.as_deref(),
            Some("plan"),
            "the child's spawn mode reaches PermissionCheckContext.mode_override"
        );
    }

    #[tokio::test]
    async fn dispatch_leaves_mode_override_none_by_default() {
        // A dispatch with no spawn-mode override leaves PermissionCheckContext
        // untagged, so the gate uses its live/boot mode (byte-identical to before).
        let seen = Arc::new(StdMutex::new(None));
        let gate = Arc::new(ContextRecordingGate {
            seen: seen.clone(),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        invoker
            .invoke("TestEcho", json!({}), no_ctx())
            .await
            .expect("allow dispatches");
        let seen = seen
            .lock()
            .unwrap()
            .clone()
            .expect("context gate consulted");
        assert_eq!(seen.mode_override, None);
    }

    #[tokio::test]
    async fn dispatch_applies_updated_input_rewrite_to_tool() {
        // An Allow{updated_input: Some(..)} from the gate (the host's
        // `updatedInput` rewrite) must substitute the input the tool actually
        // runs with — mirroring turn_loop's Ask arm. The echo tool returns
        // whatever input it received, so we can observe the substitution.
        let gate = Arc::new(ContextRecordingGate {
            seen: Arc::new(StdMutex::new(None)),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: Some(json!({ "rewritten": true })),
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        let out = invoker
            .invoke(
                "TestEcho",
                json!({ "original": true }),
                ctx_with_tool_use_id("toolu_x"),
            )
            .await
            .expect("allow dispatches");
        assert_eq!(
            out,
            json!({ "echo": { "rewritten": true } }),
            "the gate-rewritten updatedInput, not the original, is what the tool runs with"
        );
    }

    #[tokio::test]
    async fn dispatch_keeps_original_input_when_updated_input_none() {
        // Behavior identical when updated_input is None: the original input runs.
        let gate = Arc::new(ContextRecordingGate {
            seen: Arc::new(StdMutex::new(None)),
            outcome: traits::permission_gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: Vec::new(),
                decision_classification: None,
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        let out = invoker
            .invoke("TestEcho", json!({ "original": true }), no_ctx())
            .await
            .expect("allow dispatches");
        assert_eq!(out, json!({ "echo": { "original": true } }));
    }

    #[tokio::test]
    async fn dispatch_context_deny_blocks_and_surfaces_reason() {
        // A Deny from the context gate is surfaced as Internal, exactly like the
        // legacy 2-valued path.
        let gate = Arc::new(ContextRecordingGate {
            seen: Arc::new(StdMutex::new(None)),
            outcome: traits::permission_gate::PermissionOutcome::Deny {
                reason: "denied via context gate".into(),
            },
        });
        let invoker = RegistryToolInvoker::new(registry_with_echo()).with_gate(gate);
        match invoker
            .invoke("TestEcho", json!({ "a": 1 }), no_ctx())
            .await
        {
            Err(ToolInvokerError::Internal(reason)) => {
                assert!(reason.contains("denied via context gate"), "got {reason}");
            }
            other => panic!("expected Internal(deny), got {other:?}"),
        }
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
