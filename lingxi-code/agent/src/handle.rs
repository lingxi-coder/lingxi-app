//! `SubagentSpawner` trait impl.
//!
//! `PoolSubagentSpawner` wraps a `StateMachinePool` reference, allocates one
//! slot per spawn, and pumps the slot's `SubagentEvent` channel until a
//! terminal event arrives. The trait surface lives in `lingxi-traits` so
//! `AgentTool` in `lingxi-tools` can dispatch into the production pool
//! without taking a cyclic path-dep.
//!
//! The recursion-lock + budget-inheritance invariants flow through the
//! `SubagentInheritance` bundle (`Arc<dyn ToolInvoker>`,
//! `Arc<dyn BudgetEnforcerHandle>`) — the adapter stashes them on the child
//! `SubagentContext` so the child runner sees the same `Arc`s as the parent.

use crate::api::SubagentApiClient;
use crate::context::SubagentContext;
use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};
use crate::display::{AgentColor, AgentDisplay};
use crate::pool::StateMachinePool;
use crate::runner::SubagentEvent;
use async_trait::async_trait;
use protocol::AgentId;
use std::sync::Arc;
use traits::subagent_spawn::{
    SubagentInheritance, SubagentResult, SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner,
    SubagentUsage,
};

/// Production [`SubagentSpawner`] backed by a [`StateMachinePool`].
///
/// Constructed and registered on the host `BuiltinToolContext` so
/// `AgentTool` can dispatch real subagent spawns. The parent's
/// `Arc<dyn ToolInvoker>` and `Arc<dyn BudgetEnforcerHandle>` arrive on
/// every `spawn` call via [`SubagentInheritance`] — the adapter is
/// responsible for handing those Arcs to the child's runner without
/// cloning them.
pub struct PoolSubagentSpawner {
    pool: Arc<StateMachinePool>,
    /// Optional model API seam handed to every child runner via the
    /// child's [`SubagentContext`]. `None` keeps the legacy stub behavior
    /// (the runner emits a synthetic completion without calling the model).
    api_client: Option<Arc<dyn SubagentApiClient>>,
    /// Wire tool definitions (`{name, description, input_schema}`) stashed on
    /// every child's [`SubagentContext::tool_schemas`] so the spawned subagent
    /// advertises tools to the model. Unset (the default) = no tools.
    ///
    /// A SET-ONCE cell (not a plain `Vec`) so the boot path can break the
    /// construction cycle: the spawner is consumed into the `BuiltinToolContext`
    /// that builds the registry, so the registry does not exist when the spawner
    /// is constructed. The host grabs a clone of this cell via
    /// [`Self::tool_schemas_handle`] BEFORE boxing the spawner, then fills it
    /// (with [`tool_api::wire::tools_to_wire`] over the live registry) AFTER the
    /// registry is built. Reads at spawn time, so a fill that lands before the
    /// first spawn is visible. NOTE: the advertised set is NOT per-agent
    /// `AgentToolPolicy`-filtered — see the [`SubagentContext::tool_schemas`]
    /// WARNING; the runner enforces `ctx.allowed_tools` at dispatch time.
    tool_schemas: Arc<std::sync::OnceLock<Vec<serde_json::Value>>>,
}

impl PoolSubagentSpawner {
    /// Construct an adapter wrapping `pool` with no API client (legacy stub
    /// runner). Use [`Self::with_api_client`] to enable the real multi-turn
    /// loop.
    #[must_use]
    pub fn new(pool: Arc<StateMachinePool>) -> Self {
        Self {
            pool,
            api_client: None,
            tool_schemas: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Builder: attach the model API seam the child runner uses to drive the
    /// real multi-turn loop. Without this, `spawn` produces stub completions.
    #[must_use]
    pub fn with_api_client(mut self, api_client: Arc<dyn SubagentApiClient>) -> Self {
        self.api_client = Some(api_client);
        self
    }

    /// Builder: fill the wire tool definitions (`{name, description,
    /// input_schema}`, e.g. from [`tool_api::wire::tools_to_wire`]) every
    /// spawned child advertises to the model. Sets the cell immediately — use
    /// this when the schemas are known at construction (tests). The boot path
    /// instead uses [`Self::tool_schemas_handle`] to fill the cell later (the
    /// registry does not exist yet at construction).
    #[must_use]
    pub fn with_tool_schemas(self, tool_schemas: Vec<serde_json::Value>) -> Self {
        let _ = self.tool_schemas.set(tool_schemas);
        self
    }

    /// Return a clone of the set-once tool-schema cell so the host can fill it
    /// AFTER the registry is built (breaking the construction cycle). The cell
    /// is shared with the boxed spawner, so a later `cell.set(...)` is seen by
    /// every `spawn`. Filling more than once is a no-op (the first wins).
    #[must_use]
    pub fn tool_schemas_handle(&self) -> Arc<std::sync::OnceLock<Vec<serde_json::Value>>> {
        self.tool_schemas.clone()
    }

    /// Snapshot the (possibly boot-filled) tool schemas for a child context.
    /// Unset cell → empty (no tools advertised).
    fn resolve_tool_schemas(&self) -> Vec<serde_json::Value> {
        self.tool_schemas.get().cloned().unwrap_or_default()
    }

    fn make_subagent_context(subagent_type: &str, prompt: &str) -> SubagentContext {
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_definition: AgentDefinition {
                agent_type: subagent_type.into(),
                when_to_use: String::new(),
                tools: AgentToolPolicy::All {
                    use_exact_tools: true,
                },
                max_turns: 1,
                model: AgentModel::Inherit,
                permission_mode: AgentPermissionMode::Bubble,
                source: AgentSource::BuiltIn,
                base_dir: "/tmp".into(),
                system_prompt: None,
                mcp_servers: vec![],
                frontmatter_hooks: vec![],
                icon: None,
                allowed_tools: vec![],
                worktree_requirement: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            persistent: false,
            can_show_permission_prompts: false,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt: Some(Arc::from(prompt)),
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon: None,
            },
            // Set by `spawn` from `self.api_client` / `inherit.tool_invoker` /
            // `inherit.budget` / `self.tool_schemas` just before pool allocation.
            api_client: None,
            tool_invoker: None,
            tool_schemas: vec![],
            budget: None,
        }
    }
}

#[async_trait]
impl SubagentSpawner for PoolSubagentSpawner {
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        // `inherit` carries the parent's Arc<dyn ToolInvoker> +
        // Arc<dyn BudgetEnforcerHandle>. The adapter stashes the tool invoker
        // on the child's `SubagentContext` so the recursion-lock + budget-
        // inheritance invariants survive across the spawn boundary; the
        // child runner dispatches `tool_use` blocks through the very same
        // `Arc<dyn ToolInvoker>` the parent holds.
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        let mut ctx = Self::make_subagent_context(&request.subagent_type, &request.prompt);
        // Hand the child the parent's tool invoker, the parent's budget
        // enforcer, and our model API seam so the runner can drive the real
        // multi-turn loop and enforce the inherited budget per turn.
        ctx.tool_invoker = Some(inherit.tool_invoker);
        ctx.budget = Some(inherit.budget);
        ctx.api_client.clone_from(&self.api_client);
        // Read the (possibly boot-filled) tool-schema cell; unset = no tools.
        ctx.tool_schemas = self.resolve_tool_schemas();
        let agent_id = ctx.agent_id;
        let (_aid, mut rx) = self
            .pool
            .allocate(ctx)
            .await
            .map_err(|e| SubagentSpawnError::Runtime(e.to_string()))?;

        // Pump the slot until terminal. The runner emits Progress/Message
        // events as it streams turns; we ignore those here and surface only
        // the terminal Completed/Failed/Killed.
        let result = loop {
            match rx.recv().await {
                Some(SubagentEvent::Completed { result, .. }) => {
                    break SubagentResult::Completed {
                        content: result,
                        usage: SubagentUsage::default(),
                    };
                }
                Some(SubagentEvent::Failed { error, .. }) => {
                    break SubagentResult::Failed { reason: error };
                }
                Some(SubagentEvent::Killed { .. }) => {
                    break SubagentResult::Killed;
                }
                Some(_) => continue,
                None => {
                    break SubagentResult::Failed {
                        reason: "subagent channel closed unexpectedly".into(),
                    };
                }
            }
        };

        // Best-effort deallocate; failures here don't change the surfaced
        // result.
        let _ = self.pool.deallocate(&agent_id).await;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use test_harness::mocks::MockRuntimeSpawner;
    use traits::budget::{BudgetEnforcerHandle, BudgetError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};

    struct DummyInvoker;

    #[async_trait]
    impl ToolInvoker for DummyInvoker {
        async fn invoke(
            &self,
            _: &str,
            _: Value,
            _: SubagentInvocationContext,
        ) -> Result<Value, ToolInvokerError> {
            Ok(Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct DummyBudget;

    #[async_trait]
    impl BudgetEnforcerHandle for DummyBudget {
        async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    #[test]
    fn pool_spawner_constructs_with_arc_pool() {
        // The production wiring uses Arc<StateMachinePool>; this test
        // confirms the adapter accepts and stores the Arc cleanly. Driving
        // the runner end-to-end requires the M1.11 stub to receive an
        // inbound `engine::Event`, which lands when the agentic loop
        // arrives in Plan 09+.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let _spawner = PoolSubagentSpawner::new(pool);
    }

    #[test]
    fn tool_schemas_cell_starts_empty_and_late_fill_is_visible() {
        // The cycle-break primitive: the host grabs a handle, fills it AFTER
        // the registry exists, and the spawner's spawn-time read sees it.
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let spawner = PoolSubagentSpawner::new(pool);

        // Unset by default → child advertises no tools.
        assert!(spawner.resolve_tool_schemas().is_empty());

        // Host fills the shared cell late (post-registry-build).
        let cell = spawner.tool_schemas_handle();
        let schemas = vec![serde_json::json!({
            "name": "Read",
            "description": "Reads a file.",
            "input_schema": {"type": "object"}
        })];
        cell.set(schemas.clone()).expect("first fill wins");

        // The spawner's spawn-time read now returns the filled schemas.
        assert_eq!(spawner.resolve_tool_schemas(), schemas);
        // A second fill is a no-op (set-once).
        assert!(cell.set(vec![]).is_err());
        assert_eq!(spawner.resolve_tool_schemas(), schemas);
    }

    #[test]
    fn with_tool_schemas_fills_the_cell_eagerly() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 4));
        let schemas = vec![serde_json::json!({"name": "Bash"})];
        let spawner = PoolSubagentSpawner::new(pool).with_tool_schemas(schemas.clone());
        assert_eq!(spawner.resolve_tool_schemas(), schemas);
    }

    #[test]
    fn inheritance_carries_invoker_and_budget_arcs() {
        let inherit = SubagentInheritance {
            tool_invoker: Arc::new(DummyInvoker),
            budget: Arc::new(DummyBudget),
        };
        // Arc::ptr_eq round-trip — the trait-object Arcs are clonable and
        // equality survives clone (used by the recursion-lock + budget-
        // inheritance tests in lingxi-tools).
        let cloned = inherit.clone();
        assert!(Arc::ptr_eq(&inherit.tool_invoker, &cloned.tool_invoker));
        assert!(Arc::ptr_eq(&inherit.budget, &cloned.budget));
    }
}
