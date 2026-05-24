//! `ToolInvoker` impl wrapping a `ToolRegistry`.
//!
//! `RegistryToolInvoker` is the production adapter `AgentTool` uses to hand
//! a tool-dispatch surface to the spawner. The wrapper stores
//! `Arc<ToolRegistry>` so the recursion-lock invariant (parent + child
//! subagent share the *same* `Arc`) survives across the spawn boundary —
//! a property asserted via `Arc::ptr_eq` in M4-05 wiring tests.

use crate::registry::ToolRegistry;
use async_trait::async_trait;
use lingxi_traits::tool_invoker::{
    SubagentInvocationContext, ToolInvoker, ToolInvokerError,
};
use serde_json::Value;
use std::sync::Arc;

/// Wraps an `Arc<ToolRegistry>` as a `dyn ToolInvoker`.
///
/// Cheap to construct; clones share the same registry `Arc`.
pub struct RegistryToolInvoker {
    registry: Arc<ToolRegistry>,
}

impl RegistryToolInvoker {
    /// Construct an invoker bound to `registry`. The `Arc` is stored
    /// verbatim — `Arc::ptr_eq` between this invoker's clone and the
    /// parent's clone returns `true`.
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
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
        _input: Value,
        _ctx: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError> {
        // Look up the tool to surface a clear NotFound error. Actual
        // invocation routing through ToolRegistry::execute lands when the
        // subagent runner integration (Plan 09+) drives the full agentic
        // loop. The recursion-lock contract here is the registry sharing —
        // execution-side wiring is independent of that contract.
        self.registry
            .find_by_name(name)
            .ok_or_else(|| ToolInvokerError::NotFound(name.to_string()))?;
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_invoker_preserves_arc_identity() {
        let r = Arc::new(ToolRegistry::new());
        let inv = RegistryToolInvoker::new(r.clone());
        assert!(Arc::ptr_eq(&r, inv.registry_arc()));
    }
}
