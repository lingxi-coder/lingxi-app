//! `ToolInvoker` — narrow trait used by the M4-05 `AgentTool` to dispatch
//! sub-tool calls into a recursive subagent without taking a direct dep on
//! `lingxi-tools` (which would form a cycle).
//!
//! The recursion-lock invariant (a child subagent reuses the same
//! `Arc<ToolRegistry>` as its parent) is asserted in `lingxi-tools` tests via
//! `Arc::ptr_eq` on the trait-object passed through this surface.
//!
//! See M4-05 wiring follow-up plan.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use serde_json::Value;
use thiserror::Error;

/// Per-call invocation context handed to a [`ToolInvoker`].
///
/// Carries the identity of the parent agent (used by the recursion-lock test
/// to verify the child inherits the parent's registry / budget) plus any
/// metadata the trait abstraction needs to expose without leaking the
/// concrete `ToolUseContext` from `lingxi-tools`.
#[derive(Debug, Clone)]
pub struct SubagentInvocationContext {
    /// Parent agent id (the agent that is dispatching the child).
    pub parent_agent_id: Option<AgentId>,
}

/// Failure modes for [`ToolInvoker::invoke`].
#[derive(Debug, Error)]
pub enum ToolInvokerError {
    /// The named tool was not found in the registry.
    #[error("ToolInvoker: tool '{0}' not found")]
    NotFound(String),
    /// The tool surfaced an invalid input.
    #[error("ToolInvoker: invalid input: {0}")]
    InvalidInput(String),
    /// Any other internal failure.
    #[error("ToolInvoker: internal error: {0}")]
    Internal(String),
}

/// Tool invocation seam used by `AgentTool` to recurse into the registry.
///
/// Concrete impls live in `lingxi-tools` (production wrapper around
/// `ToolRegistry`) and in test fixtures (recording mock).
#[async_trait]
pub trait ToolInvoker: Send + Sync {
    /// Invoke the tool named `name` with the supplied JSON `input`.
    async fn invoke(
        &self,
        name: &str,
        input: Value,
        ctx: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn ToolInvoker>> = None;
    }
}
