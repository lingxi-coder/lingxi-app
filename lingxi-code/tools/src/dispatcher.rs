//! `ToolDispatcher` — drives tool execution with concurrency-aware partitioning.
//!
//! The dispatcher partitions a batch of tool calls into runs of
//! concurrency-safe vs unsafe calls so safe runs can execute in parallel
//! while unsafe ones run sequentially. Events emitted during execution are
//! surfaced via [`ToolDispatchEvent`].

use crate::progress::ToolProgress;
use crate::registry::ToolRegistry;
use crate::tool_trait::ToolError;
use protocol::ToolUseId;
use serde_json::Value;
use std::sync::Arc;

/// A single tool call request: id, tool name, and input payload.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Tool-use id assigned to this call (typically by the model).
    pub id: ToolUseId,
    /// Canonical name of the tool to invoke.
    pub name: String,
    /// JSON input payload for the tool.
    pub input: Value,
}

/// A partition of adjacent tool calls sharing the same concurrency safety
/// classification.
#[derive(Debug, Clone)]
pub struct ToolPartition {
    /// True when every call in this partition is concurrency-safe and may
    /// run in parallel; false when calls must run sequentially.
    pub is_concurrency_safe: bool,
    /// The calls in this partition, in original order.
    pub calls: Vec<ToolCall>,
}

/// Boxed `FnOnce` closure that mutates a [`crate::context::ToolUseContext`]
/// between calls (e.g., a `cd` command that changes the working directory).
///
/// Wrapped in a newtype so [`std::fmt::Debug`] can be implemented on it.
pub struct ContextModifierBox(
    Box<dyn FnOnce(crate::context::ToolUseContext) -> crate::context::ToolUseContext + Send>,
);

impl ContextModifierBox {
    /// Construct a `ContextModifierBox` from any compatible `FnOnce`.
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce(crate::context::ToolUseContext) -> crate::context::ToolUseContext
            + Send
            + 'static,
    {
        Self(Box::new(f))
    }

    /// Apply the modifier, consuming it and returning the updated context.
    #[must_use]
    pub fn apply(self, ctx: crate::context::ToolUseContext) -> crate::context::ToolUseContext {
        (self.0)(ctx)
    }
}

impl std::fmt::Debug for ContextModifierBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<ContextModifierBox>")
    }
}

/// Event emitted by the dispatcher during tool execution.
///
/// `Clone` is intentionally not derived — the `ContextModifier` variant
/// holds a `FnOnce` box that cannot be cloned.
#[derive(Debug)]
pub enum ToolDispatchEvent {
    /// A tool call has begun executing.
    Started {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Name of the tool being invoked.
        tool_name: String,
    },
    /// A progress update was forwarded from the tool.
    Progress {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// The progress payload from the tool.
        progress: ToolProgress,
    },
    /// The tool requires a user permission decision before proceeding.
    RequestPermission {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Name of the tool requesting permission.
        tool_name: String,
        /// Human-readable reason shown to the user.
        reason: String,
    },
    /// The tool finished successfully.
    Completed {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Result payload returned to the model.
        result: serde_json::Value,
    },
    /// The tool returned an error.
    Failed {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// The error returned by the tool.
        error: ToolError,
    },
    /// Input validation failed (schema or tool-specific check).
    ValidationFailed {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Human-readable validation error.
        error: String,
    },
    /// No tool was found by the requested name.
    ToolNotFound {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Name that was looked up.
        name: String,
    },
    /// A pre-tool hook blocked the call.
    HookBlocked {
        /// Tool-use id of the call.
        tool_use_id: ToolUseId,
        /// Reason supplied by the hook.
        reason: String,
    },
    /// The tool returned a context modifier to apply before the next call.
    ContextModifier {
        /// Tool-use id of the call that produced the modifier.
        tool_use_id: ToolUseId,
        /// The modifier closure (one-shot).
        modifier: ContextModifierBox,
    },
}

/// Dispatcher that runs batches of tool calls against a [`ToolRegistry`].
pub struct ToolDispatcher {
    #[allow(dead_code)]
    registry: Arc<ToolRegistry>,
    #[allow(dead_code)]
    max_concurrency: usize,
}

impl ToolDispatcher {
    /// Construct a dispatcher backed by the given registry. Defaults to a
    /// max concurrency of 10 for parallel-safe partitions.
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            max_concurrency: 10,
        }
    }

    /// Group adjacent calls of the same concurrency-safety class.
    ///
    /// `is_safe` decides whether each call may run in parallel with adjacent
    /// safe calls. The output preserves input order; partitions alternate
    /// classes whenever `is_safe` flips.
    pub fn partition<F: Fn(&ToolCall) -> bool>(
        calls: &[ToolCall],
        is_safe: F,
    ) -> Vec<ToolPartition> {
        let mut parts: Vec<ToolPartition> = Vec::new();
        for call in calls {
            let safe = is_safe(call);
            if let Some(last) = parts.last_mut() {
                if last.is_concurrency_safe == safe {
                    last.calls.push(call.clone());
                    continue;
                }
            }
            parts.push(ToolPartition {
                is_concurrency_safe: safe,
                calls: vec![call.clone()],
            });
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ToolUseId;
    use serde_json::json;

    #[test]
    fn partition_groups_adjacent_safe_calls() {
        let calls = vec![
            ToolCall {
                id: ToolUseId::new(),
                name: "Read".into(),
                input: json!({}),
            },
            ToolCall {
                id: ToolUseId::new(),
                name: "Read".into(),
                input: json!({}),
            },
            ToolCall {
                id: ToolUseId::new(),
                name: "Write".into(),
                input: json!({}),
            },
            ToolCall {
                id: ToolUseId::new(),
                name: "Read".into(),
                input: json!({}),
            },
        ];
        // Read = concurrency_safe; Write = not safe.
        let safe_predicate = |c: &ToolCall| c.name == "Read";
        let parts = ToolDispatcher::partition(&calls, safe_predicate);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].is_concurrency_safe);
        assert_eq!(parts[0].calls.len(), 2);
        assert!(!parts[1].is_concurrency_safe);
        assert_eq!(parts[1].calls.len(), 1);
        assert!(parts[2].is_concurrency_safe);
    }
}
