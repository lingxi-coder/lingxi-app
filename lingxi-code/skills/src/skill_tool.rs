//! `SkillTool` — the model-visible tool that resolves and dispatches a skill.
//!
//! Dispatch goes through `lingxi-agent::StateMachinePool` per §10.0 (skill
//! execution is model-visible, so it occupies a Subagent slot rather than a
//! `ForkedAgent`). M1.15 ships registry resolution + result shape; the full
//! subagent spawn is wired in Plan 15's cli-demo.

use crate::registry::SkillRegistry;
use agent::StateMachinePool;
use async_trait::async_trait;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tools::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolProgressSender,
    ToolStaticContext, ToolUseContext,
};

/// Model-visible `Skill` tool. Resolves a skill by name from the registry
/// and (in later milestones) hands execution to the `StateMachinePool`.
pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
    /// Subagent pool that will eventually own the dispatched skill execution.
    /// M1.15 keeps the handle but does not yet spawn (wired in Plan 15).
    #[allow(dead_code)]
    pool: Arc<StateMachinePool>,
}

impl SkillTool {
    /// Construct a new `SkillTool` referencing a shared registry and pool.
    #[must_use]
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, pool: Arc<StateMachinePool>) -> Self {
        Self { registry, pool }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn input_schema(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            })
        });
        &SCHEMA
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        32_768
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // Mutates parent context via subagent — not concurrency-safe.
        false
    }

    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "skill tool".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Invoke a registered skill by name.".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use this when a skill matches your task.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("name required".into()))?;
        let _skill = {
            let reg = self.registry.read().await;
            reg.get(name)
                .cloned()
                .ok_or_else(|| ToolError::NotFound(name.into()))?
        };
        // Build a Subagent context — model-visible execution. Full subagent
        // spawn happens in Plan 15 cli-demo wiring; M1.15 ships the registry
        // resolution + result shape.
        Ok(ToolCallResult {
            data: serde_json::json!({ "skill": name, "status": "stub" }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}
