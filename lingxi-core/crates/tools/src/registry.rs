//! `ToolRegistry` — partitioned, dynamic registry of `Tool` implementations.
//!
//! The registry holds builtin tools (statically registered at startup) plus
//! three dynamic partitions:
//!
//! - **MCP**: tools sourced from MCP server connections, keyed by
//!   [`McpConnectionId`] so a disconnect can drop them en masse.
//! - **LSP**: tools sourced from Language-Server-Protocol bridges.
//! - **Plugin**: tools sourced from host plugins, keyed by [`PluginId`]
//!   so an unloaded plugin can drop its tools.
//!
//! Each lookup checks all partitions; insertion order across partitions is
//! preserved by [`available_tools`](ToolRegistry::available_tools).

use crate::tool_trait::{Tool, ToolStaticContext};
use lingxi_protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of all [`Tool`] instances available to the dispatcher.
///
/// Cloning is not supported; share via `Arc<ToolRegistry>` instead.
pub struct ToolRegistry {
    builtin: Vec<Arc<dyn Tool>>,
    mcp_tools: HashMap<McpConnectionId, Vec<Arc<dyn Tool>>>,
    lsp_tools: Vec<Arc<dyn Tool>>,
    plugin_tools: HashMap<PluginId, Vec<Arc<dyn Tool>>>,
}

impl ToolRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builtin: Vec::new(),
            mcp_tools: HashMap::new(),
            lsp_tools: Vec::new(),
            plugin_tools: HashMap::new(),
        }
    }

    /// Register a builtin tool. Insertion order is preserved.
    pub fn register_builtin(&mut self, tool: Arc<dyn Tool>) {
        self.builtin.push(tool);
    }

    /// Return all tools enabled for the given static context.
    ///
    /// Builtin tools are gated on [`Tool::is_enabled`]; MCP / LSP / plugin
    /// tools are passed through unconditionally (their lifecycle is managed
    /// by their owning subsystem).
    #[must_use]
    pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for t in &self.builtin {
            if t.is_enabled(ctx) {
                out.push(t.clone());
            }
        }
        for ts in self.mcp_tools.values() {
            out.extend(ts.iter().cloned());
        }
        out.extend(self.lsp_tools.iter().cloned());
        for ts in self.plugin_tools.values() {
            out.extend(ts.iter().cloned());
        }
        out
    }

    /// Find a tool by name or alias across all partitions. Returns the first
    /// matching tool in iteration order (builtin first).
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.builtin
            .iter()
            .chain(self.mcp_tools.values().flatten())
            .chain(self.lsp_tools.iter())
            .chain(self.plugin_tools.values().flatten())
            .find(|t| t.name() == name || t.aliases().contains(&name))
            .cloned()
    }

    /// Register all tools exposed by a freshly-connected MCP server.
    pub fn register_mcp_tools(&mut self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>) {
        self.mcp_tools.insert(conn_id, tools);
    }

    /// Drop every tool sourced from the given MCP connection.
    pub fn unregister_mcp_tools(&mut self, conn_id: McpConnectionId) {
        self.mcp_tools.remove(&conn_id);
    }

    /// Register all tools exposed by a freshly-loaded plugin.
    pub fn register_plugin_tools(&mut self, plugin_id: PluginId, tools: Vec<Arc<dyn Tool>>) {
        self.plugin_tools.insert(plugin_id, tools);
    }

    /// Drop every tool sourced from the given plugin.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.plugin_tools.remove(plugin_id);
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    };
    use crate::ToolUseContext;
    use async_trait::async_trait;
    use lingxi_permission::result::PermissionMetadata;
    use lingxi_permission::{PermissionDecisionReason, PermissionResult};
    use serde_json::json;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "Dummy"
        }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({"type": "object"}));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            1024
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn check_permissions(
            &self,
            _input: &serde_json::Value,
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
        async fn description(
            &self,
            _input: &serde_json::Value,
            _opts: &DescriptionOptions,
        ) -> String {
            "Dummy".into()
        }
        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _input: serde_json::Value,
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

    #[test]
    fn find_by_name_returns_registered_tool() {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(DummyTool));
        assert!(r.find_by_name("Dummy").is_some());
        assert!(r.find_by_name("Nonexistent").is_none());
    }
}
