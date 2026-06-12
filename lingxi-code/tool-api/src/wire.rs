//! Wire serialization of [`Tool`] definitions for the model's `tools` array.
//!
//! Each tool becomes the session-stable base schema claude-code sends on the
//! `messages.create` request: `{ name, description, input_schema }`, where
//! `description` is the tool's long-form [`Tool::prompt`] output (matching
//! `claude-code/src/utils/api.ts:169-178`). The orchestrator's streaming +
//! batched turn loops and the subagent seam all funnel their tool set through
//! [`tools_to_wire`] so the wire bytes are produced one way.
//!
//! FORCED DIVERGENCE / deferrals (documented, not bugs):
//! - The `strict`, `cache_control`, and `eager_input_streaming` extras
//!   (`api.ts:180-200`) are feature-flag / session-schema-cache gated in
//!   claude-code; only the base triple is emitted here.
//! - ORDER: claude-code does NOT sort — it emits tools in source-list order
//!   (`claude.ts` `filteredTools.map(...)`, builtin-registration order first).
//!   This port sorts by `name` instead, a deliberate determinism divergence:
//!   the registry's MCP / plugin partitions iterate a `HashMap` (unstable
//!   order), and there is no per-session `toolSchemaCache` (claude-code memoizes
//!   the serialized array to freeze prompt-cache bytes across turns). Sorting
//!   keeps the bytes deterministic and consistent with the system-prompt
//!   `<tools>` block (which `registry.rs` also sorts by name). The model
//!   resolves tools by name, not array position, so no tool-precedence contract
//!   is broken — but the Rust wire bytes will not byte-match claude-code's
//!   builtin-first order. A session-level cache (and preserving builtin order
//!   while sorting only the `HashMap` partitions) is the recommended follow-up.

use crate::tool_trait::{PromptOptions, Tool};
use serde_json::{json, Value};
use std::sync::Arc;

/// Serialize one tool to its wire definition `{ name, description, input_schema }`.
///
/// `description` is awaited from [`Tool::prompt`] (the long-form tool prompt,
/// the same text claude-code places in the API tool definition's `description`).
pub async fn tool_to_wire(tool: &dyn Tool, opts: &PromptOptions) -> Value {
    json!({
        "name": tool.name(),
        "description": tool.prompt(opts).await,
        "input_schema": tool.input_schema(),
    })
}

/// Serialize a tool set to the wire `tools` array, sorted by `name` for
/// deterministic bytes (see the module note on the missing session cache).
pub async fn tools_to_wire(tools: &[Arc<dyn Tool>], opts: &PromptOptions) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(tools.len());
    for tool in tools {
        out.push(tool_to_wire(tool.as_ref(), opts).await);
    }
    out.sort_by(|a, b| wire_name(a).cmp(wire_name(b)));
    out
}

/// Borrow the `name` of a wire tool definition (empty string if absent — only
/// reachable if a caller hands in a non-tool `Value`, which the public API
/// never does).
fn wire_name(v: &Value) -> &str {
    v.get("name").and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ToolUseContext;
    use crate::tool_trait::{
        DescriptionOptions, PromptOptions, ToolCallResult, ToolError, ToolStaticContext,
    };
    use async_trait::async_trait;
    use permission::result::PermissionMetadata;
    use permission::{PermissionDecisionReason, PermissionResult};
    use serde_json::json;

    /// Minimal stub tool with a configurable name / prompt / schema.
    struct StubTool {
        name: &'static str,
        prompt: &'static str,
        schema: Value,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            self.name
        }
        fn input_schema(&self) -> &Value {
            &self.schema
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &Value) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &ToolUseContext,
        ) -> PermissionResult {
            PermissionResult::Allow {
                reason: PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: PermissionMetadata::default(),
            }
        }
        async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
            self.name.into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            self.prompt.into()
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: ToolUseContext,
            _tx: crate::progress::ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            Ok(ToolCallResult {
                data: json!({"ok": true}),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            })
        }
    }

    fn opts() -> PromptOptions {
        PromptOptions {
            include_examples: true,
        }
    }

    #[tokio::test]
    async fn one_tool_serializes_to_base_triple() {
        let tool = StubTool {
            name: "Read",
            prompt: "Reads a file from disk.",
            schema: json!({"type": "object", "properties": {"file_path": {"type": "string"}}}),
        };
        let wire = tool_to_wire(&tool, &opts()).await;
        assert_eq!(wire["name"], "Read");
        assert_eq!(wire["description"], "Reads a file from disk.");
        assert_eq!(wire["input_schema"]["type"], "object");
        // Exactly the base triple — no stray keys.
        let obj = wire.as_object().unwrap();
        assert_eq!(obj.len(), 3, "only name/description/input_schema: {wire}");
    }

    #[tokio::test]
    async fn tools_are_sorted_by_name() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubTool {
                name: "Bravo",
                prompt: "b",
                schema: json!({"type": "object"}),
            }),
            Arc::new(StubTool {
                name: "Alpha",
                prompt: "a",
                schema: json!({"type": "object"}),
            }),
            Arc::new(StubTool {
                name: "Charlie",
                prompt: "c",
                schema: json!({"type": "object"}),
            }),
        ];
        let wire = tools_to_wire(&tools, &opts()).await;
        let names: Vec<&str> = wire.iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[tokio::test]
    async fn empty_set_serializes_to_empty_vec() {
        let wire = tools_to_wire(&[], &opts()).await;
        assert!(wire.is_empty());
    }
}
