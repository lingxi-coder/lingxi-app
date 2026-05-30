//! `AgentTool` — spawns a subagent via the M1 agent pool.
//!
//! Spec §4 Flow D + §7 line 491-499. claude-code source:
//! `claude-code/src/tools/AgentTool/AgentTool.tsx` + `built-in/*.ts`.
//!
//! **Architectural note (M4-05):** the existing M1 dep graph has
//! `lingxi-tasks` and `lingxi-agent` already depending on `lingxi-tools`,
//! which prevents this crate from depending on either. Subagent spawn and
//! `BudgetEnforcer` inheritance therefore land here as a tool *surface*
//! (locked schemas + locked error strings + locked telemetry events); the
//! actual wiring through `StateMachinePool::allocate` + parent
//! `Arc<BudgetEnforcer>` happens in `lingxi-coordinator` post-M5 when the
//! cycle issue is resolved by extracting `ToolRegistry` into a leaf crate.
//! All byte-locks (tool name, 6 subagent types, M3-05 denial format,
//! 3 telemetry events) are preserved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{AGENT_COMPLETED_M4_05, AGENT_FAILED, AGENT_STARTED};
use telemetry::AnalyticsBus;
use traits::budget::BudgetError;
use traits::subagent_spawn::{SubagentInheritance, SubagentResult, SubagentSpawnRequest};

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

/// `Agent` — canonical tool name (claude-code `AGENT_TOOL_NAME`).
pub const AGENT_TOOL_NAME: &str = "Agent";

/// `Task` — legacy alias the dispatcher must accept (claude-code
/// `LEGACY_AGENT_TOOL_NAME`).
pub const LEGACY_AGENT_TOOL_NAME: &str = "Task";

/// Six built-in subagent types — byte-aligned with upstream
/// `claude-code/src/tools/AgentTool/built-in/*.ts`.
pub const BUILTIN_SUBAGENT_TYPES: &[&str] = &[
    "general-purpose",
    "Plan",
    "Explore",
    "verification",
    "claude-code-guide",
    "statusline-setup",
];

/// Prefix locked by M3-05 (`cost/src/budget.rs` budget-exceeded test fixtures).
/// Production constructs the full string via
/// `format!("Budget exceeded (${:.2}); stopped.", dollars)`.
pub const SUBAGENT_BUDGET_DENIED_PREFIX: &str = "Budget exceeded ($";

/// Input shape accepted by `AgentTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentToolInput {
    /// One of [`BUILTIN_SUBAGENT_TYPES`].
    pub subagent_type: String,
    /// Initial prompt seeded into the subagent's first turn.
    pub prompt: String,
    /// Optional context files (paths) injected as `system`-tagged messages.
    #[serde(default)]
    pub context_paths: Vec<PathBuf>,
}

static AGENT_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "subagent_type": { "type": "string", "minLength": 1 },
            "prompt":        { "type": "string", "minLength": 1 },
            "context_paths": { "type": "array", "items": { "type": "string" }, "default": [] }
        },
        "required": ["subagent_type", "prompt"]
    })
});

/// Format the M3-05 byte-locked budget-exceeded denial string.
///
/// Caller passes `current_nano_usd`; output is the literal
/// `"Budget exceeded (${dollars:.2}); stopped."` (see `cost/src/budget.rs`).
#[must_use]
pub fn format_budget_denied(current_nano_usd: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let dollars = current_nano_usd as f64 / 1_000_000_000.0;
    format!("Budget exceeded (${dollars:.2}); stopped.")
}

/// `AgentTool` — spawn a subagent (surface only; recursive dispatch lands
/// in `lingxi-coordinator` post-M5).
pub struct AgentTool {
    ctx: BuiltinToolContext,
}

impl AgentTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn fresh_invocation_id() -> String {
        tool_api::util::ids::ulid_or_uuid()
    }

    async fn emit_started(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        subagent_type: &str,
        prompt_chars: usize,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "subagent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(subagent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "prompt_chars".into(),
            AnalyticsValue::Int(prompt_chars as i64),
        );
        bus.log_event(AGENT_STARTED, md).await;
    }

    async fn emit_completed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        duration_ms: u64,
        subagent_type: &str,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "subagent_type".into(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(subagent_type.to_string()).into_inner(),
            ),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(AGENT_COMPLETED_M4_05, md).await;
    }

    async fn emit_failed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        error_kind: &str,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(AGENT_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        AGENT_TOOL_NAME
    }
    fn aliases(&self) -> &[&str] {
        const ALIASES: &[&str] = &[LEGACY_AGENT_TOOL_NAME];
        ALIASES
    }
    fn input_schema(&self) -> &Value {
        &AGENT_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Agent spawn delegates to host runtime; no direct side-effect".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Spawn a subagent of one of the built-in types".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use Agent to dispatch a focused subagent (general-purpose / Plan / \
         Explore / verification / claude-code-guide / statusline-setup) with \
         a seeded prompt. The subagent runs in its own state-machine slot \
         and inherits the parent's BudgetEnforcer."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = Self::fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // 1. Parse input.
        let parsed: AgentToolInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "invalid_input",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "Agent: invalid input shape: {e}"
                )));
            }
        };

        // 2. Validate subagent_type.
        if !BUILTIN_SUBAGENT_TYPES.contains(&parsed.subagent_type.as_str()) {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "unknown_subagent_type",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Agent: unknown subagent_type '{}' (known: {})",
                parsed.subagent_type,
                BUILTIN_SUBAGENT_TYPES.join(", ")
            )));
        }

        // 3. Validate prompt.
        if parsed.prompt.trim().is_empty() {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "empty_prompt",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput("Agent: prompt is empty".into()));
        }

        // 4. Wiring guard: spawner + budget + registry must be present.
        let spawner = self.ctx.subagent_spawner.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: SubagentSpawner not wired into BuiltinToolContext".into(),
            )
        })?;
        let budget = self.ctx.budget_enforcer.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: BudgetEnforcerHandle not wired into BuiltinToolContext".into(),
            )
        })?;
        let parent_registry = ctx.subagent_registry.clone().ok_or_else(|| {
            ToolError::Internal(
                "AgentTool: parent ToolRegistry not threaded via ToolUseContext.subagent_registry"
                    .into(),
            )
        })?;

        // 5. M3-05 byte-locked budget gate. `check_and_charge(0)` re-runs the
        // pre-call gate; on Exceeded we surface the locked denial string.
        if let Err(BudgetError::Exceeded { current_nano_usd }) = budget.check_and_charge(0).await {
            Self::emit_failed(
                &bus,
                &invocation_id,
                "budget_exceeded",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::Internal(format_budget_denied(current_nano_usd)));
        }

        // 6. Emit started.
        Self::emit_started(
            &bus,
            &invocation_id,
            &parsed.subagent_type,
            parsed.prompt.chars().count(),
        )
        .await;

        // 7. Build the inheritance bundle and dispatch into the spawner.
        // The recursion-lock invariant: `parent_registry` is passed verbatim
        // into the bundle via `RegistryToolInvoker::new(parent_registry)`.
        // The budget Arc is cloned (no deep clone — `Arc::clone` only bumps
        // the refcount), so `Arc::ptr_eq` between parent + child holds.
        let invoker: Arc<dyn traits::tool_invoker::ToolInvoker> = Arc::new(
            crate::tool_invoker_impl::RegistryToolInvoker::new(parent_registry.clone()),
        );
        let inherit = SubagentInheritance {
            tool_invoker: invoker,
            budget: budget.clone(),
        };
        let request = SubagentSpawnRequest {
            subagent_type: parsed.subagent_type.clone(),
            prompt: parsed.prompt.clone(),
            context_paths: parsed.context_paths.clone(),
        };

        let outcome = spawner.spawn(request, inherit).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(SubagentResult::Completed { content, .. }) => {
                Self::emit_completed(&bus, &invocation_id, duration_ms, &parsed.subagent_type)
                    .await;
                Ok(ToolCallResult {
                    data: json!({
                        "subagent_type": parsed.subagent_type,
                        "result": content,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Ok(SubagentResult::Failed { reason }) => {
                Self::emit_failed(&bus, &invocation_id, "subagent_failed", duration_ms).await;
                Err(ToolError::Internal(reason))
            }
            Ok(SubagentResult::Killed) => {
                Self::emit_failed(&bus, &invocation_id, "killed", duration_ms).await;
                Err(ToolError::Internal("Agent: subagent was killed".into()))
            }
            Err(e) => {
                Self::emit_failed(&bus, &invocation_id, "spawn_error", duration_ms).await;
                Err(ToolError::Internal(format!("Agent: spawner failed: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::agent_test_support::{
        arc_mock_budget, arc_mock_mailbox, arc_mock_spawner, arc_mock_task_registry,
        MockBudgetEnforcerHandle, MockSubagentSpawner,
    };
    use crate::builtin::test_support::{ctx_for_file_tools, fresh_tx, make_dummy_fs};
    use crate::context::{ToolUseContext, ToolUseOptions};
    use crate::registry::ToolRegistry;
    use std::path::PathBuf;
    use telemetry::AnalyticsBus;
    use traits::budget::BudgetEnforcerHandle;
    use traits::subagent_spawn::SubagentSpawner;

    /// Build a `BuiltinToolContext` wired with all four M4-05 mocks.
    fn wired_ctx(
        spawner: Arc<MockSubagentSpawner>,
        registry: Arc<crate::builtin::agent_test_support::MockTaskRegistryHandle>,
        mailbox: Arc<crate::builtin::agent_test_support::MockMailboxRouterHandle>,
        budget: Arc<MockBudgetEnforcerHandle>,
    ) -> BuiltinToolContext {
        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry = Some(registry as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router = Some(mailbox as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(budget.clone() as Arc<dyn BudgetEnforcerHandle>);
        bctx
    }

    fn fresh_ctx_with_registry(registry: Arc<ToolRegistry>) -> ToolUseContext {
        ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "test".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: None,
            content_replacement_state: None,
            session: None,
            subagent_registry: Some(registry),
        }
    }

    // =====================================================================
    // CRITICAL TEST 1 — recursion-lock: AgentTool passes parent's
    // Arc<ToolRegistry> verbatim into the child via SubagentInheritance.
    // The mock spawner captures the inheritance bundle so we can read back
    // the Arc<dyn ToolInvoker> and pull the inner Arc<ToolRegistry> out.
    // =====================================================================
    #[tokio::test]
    async fn recursion_lock_child_inherits_parent_tool_registry_arc() {
        let parent_registry = Arc::new(ToolRegistry::new());
        let spawner = arc_mock_spawner();
        let budget = arc_mock_budget(u64::MAX);
        let bctx = wired_ctx(
            spawner.clone(),
            arc_mock_task_registry(),
            arc_mock_mailbox(),
            budget,
        );

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(parent_registry.clone());
        let input = serde_json::json!({
            "subagent_type": "general-purpose",
            "prompt": "hi"
        });
        tool.call(input, ctx, fresh_tx())
            .await
            .expect("spawner returns Completed by default");

        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1, "exactly one spawn call");
        let captured_invoker = &invocations[0].inherit.tool_invoker;
        // Downcast to RegistryToolInvoker via Arc::downcast on the concrete
        // type. Since we can't downcast Arc<dyn>, we instead introspect
        // through the public accessor on our concrete wrapper. The
        // production wrapper preserves the Arc<ToolRegistry> verbatim, so
        // we reach for it via the test-only seam.
        // SAFETY: the mock spawner returns the exact Arc the production
        // AgentTool::call passed; we wrap parent_registry in a fresh
        // RegistryToolInvoker on the call path, so the trait-object pointer
        // is unique to this invocation. We compare the inner Arcs.
        let captured = (**captured_invoker)
            .as_any()
            .downcast_ref::<crate::tool_invoker_impl::RegistryToolInvoker>()
            .map(|i| i.registry_arc().clone());
        assert!(
            captured.is_some(),
            "captured invoker must be a RegistryToolInvoker"
        );
        assert!(
            Arc::ptr_eq(&parent_registry, captured.as_ref().unwrap()),
            "recursion lock: child must inherit parent's Arc<ToolRegistry> verbatim"
        );
    }

    // =====================================================================
    // CRITICAL TEST 2 — budget inheritance: AgentTool passes parent's
    // Arc<dyn BudgetEnforcerHandle> verbatim into the child via
    // SubagentInheritance. Arc::ptr_eq on the trait-object Arc holds.
    // =====================================================================
    #[tokio::test]
    async fn budget_inheritance_child_inherits_parent_budget_arc() {
        let parent_budget: Arc<dyn BudgetEnforcerHandle> =
            Arc::new(MockBudgetEnforcerHandle::new(u64::MAX));
        let spawner = arc_mock_spawner();

        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry =
            Some(arc_mock_task_registry() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router =
            Some(arc_mock_mailbox() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(parent_budget.clone());

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "subagent_type": "general-purpose",
            "prompt": "hi"
        });
        tool.call(input, ctx, fresh_tx()).await.unwrap();

        let invocations = spawner.invocations();
        assert_eq!(invocations.len(), 1);
        let captured = &invocations[0].inherit.budget;
        assert!(
            Arc::ptr_eq(&parent_budget, captured),
            "budget inheritance: child must inherit parent's Arc<dyn BudgetEnforcerHandle> verbatim"
        );
    }

    // =====================================================================
    // M3-05 byte-locked denial format flows through Budget -> AgentTool.
    // =====================================================================
    #[tokio::test]
    async fn budget_exceeded_yields_m3_05_byte_locked_denial_string() {
        let budget = Arc::new(MockBudgetEnforcerHandle::new(1_500_000_000)); // $1.50 cap
        budget.set_total(2_000_000_000); // $2.00 already spent

        let spawner = arc_mock_spawner();
        let mut bctx = ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        );
        bctx.subagent_spawner = Some(spawner.clone() as Arc<dyn SubagentSpawner>);
        bctx.task_registry =
            Some(arc_mock_task_registry() as Arc<dyn traits::task_registry::TaskRegistryHandle>);
        bctx.mailbox_router =
            Some(arc_mock_mailbox() as Arc<dyn traits::mailbox::MailboxRouterHandle>);
        bctx.budget_enforcer = Some(budget.clone() as Arc<dyn BudgetEnforcerHandle>);

        let tool = AgentTool::new(bctx);
        let ctx = fresh_ctx_with_registry(Arc::new(ToolRegistry::new()));
        let input = serde_json::json!({
            "subagent_type": "Plan",
            "prompt": "do work"
        });
        let err = tool
            .call(input, ctx, fresh_tx())
            .await
            .expect_err("budget gate must trip and surface denial");
        let msg = format!("{err}");
        // M3-05 byte-locked: "Budget exceeded ($X.YZ); stopped."
        assert!(
            msg.contains(SUBAGENT_BUDGET_DENIED_PREFIX),
            "denial msg must start with M3-05 byte-locked prefix: {msg}"
        );
        assert!(msg.contains("); stopped."));
        // Spawner must NOT have been called.
        assert!(
            spawner.invocations().is_empty(),
            "spawner must not be invoked once budget gate trips"
        );
    }

    #[test]
    fn agent_tool_name_locked() {
        assert_eq!(AGENT_TOOL_NAME, "Agent");
        assert_eq!(LEGACY_AGENT_TOOL_NAME, "Task");
    }

    #[test]
    fn six_builtin_subagent_types_byte_aligned() {
        assert_eq!(
            BUILTIN_SUBAGENT_TYPES,
            &[
                "general-purpose",
                "Plan",
                "Explore",
                "verification",
                "claude-code-guide",
                "statusline-setup"
            ]
        );
        assert_eq!(BUILTIN_SUBAGENT_TYPES.len(), 6);
    }

    #[test]
    fn budget_exceeded_format_matches_m3_05_lock() {
        // M3-05 byte-locked string format.
        assert_eq!(
            format_budget_denied(150_750_000_000),
            "Budget exceeded ($150.75); stopped."
        );
        assert_eq!(
            format_budget_denied(1_500_000_000),
            "Budget exceeded ($1.50); stopped."
        );
    }

    #[test]
    fn budget_denied_prefix_locked() {
        assert_eq!(SUBAGENT_BUDGET_DENIED_PREFIX, "Budget exceeded ($");
        assert!(format_budget_denied(0).starts_with(SUBAGENT_BUDGET_DENIED_PREFIX));
    }

    #[test]
    fn agent_input_serde_roundtrip() {
        let v = json!({
            "subagent_type": "general-purpose",
            "prompt": "Explore the repo structure.",
            "context_paths": []
        });
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.subagent_type, "general-purpose");
        assert_eq!(parsed.prompt, "Explore the repo structure.");
        assert!(parsed.context_paths.is_empty());
    }

    #[test]
    fn agent_input_defaults_context_paths_to_empty() {
        let v = json!({"subagent_type": "Plan", "prompt": "Design."});
        let parsed: AgentToolInput = serde_json::from_value(v).unwrap();
        assert!(parsed.context_paths.is_empty());
    }
}
