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
//! Each lookup checks all partitions. The wire / prompt order is parity-fixed
//! by [`available_tools`](ToolRegistry::available_tools): builtins sorted by
//! `locale_cmp` as a contiguous prefix, then the MCP / LSP / plugin partitions
//! sorted by `locale_cmp` — mirroring claude-code's `assembleToolPool` /
//! `mergeAndFilterTools` (`tools.ts:345-367`, `utils/toolPool.ts:65-70`). The
//! MCP / plugin partitions are stored insertion-ordered (`Vec` of keyed
//! entries, not `HashMap`) so that order is deterministic before the sort.

use crate::tool_trait::{Tool, ToolStaticContext};
use crate::wire::locale_cmp;
use protocol::{McpConnectionId, PluginId};
use std::sync::Arc;

/// Registry of all [`Tool`] instances available to the dispatcher.
///
/// Cloning is not supported; share via `Arc<ToolRegistry>` instead.
///
/// The MCP and plugin partitions are insertion-ordered `Vec`s of `(key, tools)`
/// entries (rather than a `HashMap`) so iteration order is deterministic; the
/// dedup/sort that produces the wire order is applied in [`Self::available_tools`].
pub struct ToolRegistry {
    builtin: Vec<Arc<dyn Tool>>,
    mcp_tools: Vec<(McpConnectionId, Vec<Arc<dyn Tool>>)>,
    lsp_tools: Vec<Arc<dyn Tool>>,
    plugin_tools: Vec<(PluginId, Vec<Arc<dyn Tool>>)>,
}

impl ToolRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builtin: Vec::new(),
            mcp_tools: Vec::new(),
            lsp_tools: Vec::new(),
            plugin_tools: Vec::new(),
        }
    }

    /// Register a builtin tool. Insertion order is preserved.
    pub fn register_builtin(&mut self, tool: Arc<dyn Tool>) {
        self.builtin.push(tool);
    }

    /// Return all tools enabled for the given static context, in claude-code's
    /// wire order: builtins (gated on [`Tool::is_enabled`]) sorted by
    /// [`locale_cmp`] as a contiguous prefix, then the MCP + LSP + plugin tools
    /// sorted by [`locale_cmp`]. This mirrors `assembleToolPool` /
    /// `mergeAndFilterTools` (`tools.ts:345-367`, `utils/toolPool.ts:65-70`),
    /// which partition-sort with `name.localeCompare` keeping built-ins a
    /// contiguous prefix for the server-side prompt-cache breakpoint. Builtins
    /// win on a name conflict (their entry is emitted; the later dynamic one is
    /// dropped), matching `uniqBy`'s built-in-precedence dedup.
    ///
    /// MCP / LSP / plugin tools are passed through unconditionally (their
    /// lifecycle is managed by their owning subsystem).
    #[must_use]
    pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>> {
        // Builtin prefix: enabled builtins, locale-sorted by name.
        let mut builtins: Vec<Arc<dyn Tool>> = self
            .builtin
            .iter()
            .filter(|t| t.is_enabled(ctx))
            .cloned()
            .collect();
        builtins.sort_by(|a, b| locale_cmp(a.name(), b.name()));

        // Dynamic partition: MCP + LSP + plugin, locale-sorted by name.
        let mut dynamic: Vec<Arc<dyn Tool>> = Vec::new();
        for (_id, ts) in &self.mcp_tools {
            dynamic.extend(ts.iter().cloned());
        }
        dynamic.extend(self.lsp_tools.iter().cloned());
        for (_id, ts) in &self.plugin_tools {
            dynamic.extend(ts.iter().cloned());
        }
        dynamic.sort_by(|a, b| locale_cmp(a.name(), b.name()));

        // Builtin-precedence dedup (uniqBy): drop a dynamic tool whose name
        // collides with a builtin already in the prefix.
        let builtin_names: std::collections::HashSet<String> =
            builtins.iter().map(|t| t.name().to_string()).collect();
        let mut out = builtins;
        for t in dynamic {
            if !builtin_names.contains(t.name()) {
                out.push(t);
            }
        }
        out
    }

    /// Find a tool by name or alias across all partitions. Returns the first
    /// matching tool in iteration order (builtin first).
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.builtin
            .iter()
            .chain(self.mcp_tools.iter().flat_map(|(_id, ts)| ts.iter()))
            .chain(self.lsp_tools.iter())
            .chain(self.plugin_tools.iter().flat_map(|(_id, ts)| ts.iter()))
            .find(|t| t.name() == name || t.aliases().contains(&name))
            .cloned()
    }

    /// Register all tools exposed by a freshly-connected MCP server. If the
    /// connection id was already registered, its tool set is replaced in place
    /// (preserving its insertion position), matching the prior `HashMap::insert`
    /// upsert semantics.
    pub fn register_mcp_tools(&mut self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>) {
        if let Some(entry) = self.mcp_tools.iter_mut().find(|(id, _)| *id == conn_id) {
            entry.1 = tools;
        } else {
            self.mcp_tools.push((conn_id, tools));
        }
    }

    /// Drop every tool sourced from the given MCP connection.
    pub fn unregister_mcp_tools(&mut self, conn_id: McpConnectionId) {
        self.mcp_tools.retain(|(id, _)| *id != conn_id);
    }

    /// Register all tools exposed by a freshly-loaded plugin. Re-registering an
    /// existing plugin id replaces its tool set in place (upsert).
    pub fn register_plugin_tools(&mut self, plugin_id: PluginId, tools: Vec<Arc<dyn Tool>>) {
        if let Some(entry) = self
            .plugin_tools
            .iter_mut()
            .find(|(id, _)| *id == plugin_id)
        {
            entry.1 = tools;
        } else {
            self.plugin_tools.push((plugin_id, tools));
        }
    }

    /// Drop every tool sourced from the given plugin.
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.plugin_tools.retain(|(id, _)| id != plugin_id);
    }

    /// Return ALL registered tool names across every partition (builtin
    /// + MCP + LSP + plugin), unfiltered by `Tool::is_enabled`.
    ///
    /// Used by `lingxi-orchestrator`'s M5-03 system prompt assembler:
    /// the `<tools>` block emits names alphabetically so the model
    /// knows what to call by name. Filtering by enable-flag would
    /// require a `ToolStaticContext`, which only carries meaning at
    /// dispatch time. The order returned here is partition iteration
    /// order; the assembler re-sorts alphabetically for byte stability.
    #[must_use]
    pub fn all_names(&self) -> Vec<String> {
        self.builtin
            .iter()
            .chain(self.mcp_tools.iter().flat_map(|(_id, ts)| ts.iter()))
            .chain(self.lsp_tools.iter())
            .chain(self.plugin_tools.iter().flat_map(|(_id, ts)| ts.iter()))
            .map(|t| t.name().to_string())
            .collect()
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
    use permission::result::PermissionMetadata;
    use permission::{PermissionDecisionReason, PermissionResult};
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
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
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

    /// A stub tool with a configurable name (for ordering tests).
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
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
            self.0.into()
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
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    /// `available_tools` emits builtins in claude-code's `localeCompare` order
    /// (NOT registration order, NOT byte order): `ListMcpResourcesTool` before
    /// `LSP`, and `REPL` LAST after `RemoteTrigger` — the exact divergent pairs
    /// from `assembleToolPool`'s `name.localeCompare` (`tools.ts:345-367`).
    #[test]
    fn available_tools_orders_builtins_by_localecompare() {
        let mut r = ToolRegistry::new();
        // Register in a deliberately scrambled order.
        for n in [
            "REPL",
            "LSP",
            "Read",
            "RemoteTrigger",
            "ListMcpResourcesTool",
            "ReadMcpResourceTool",
        ] {
            r.register_builtin(Arc::new(NamedTool(n)));
        }
        let names: Vec<String> = r
            .available_tools(&ToolStaticContext::default())
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "ListMcpResourcesTool",
                "LSP",
                "Read",
                "ReadMcpResourceTool",
                "RemoteTrigger",
                "REPL",
            ]
        );
        // Distinguish from a byte sort, which would put LSP first & REPL early.
        let mut byte_sorted = names.clone();
        byte_sorted.sort_unstable();
        assert_ne!(names, byte_sorted, "must be localeCompare, not byte order");
    }

    /// MCP / plugin partitions are insertion-ordered (Vec, not HashMap) and are
    /// emitted AFTER the builtin prefix, each locale-sorted; a dynamic tool
    /// whose name collides with a builtin is dropped (builtin-precedence dedup).
    #[test]
    fn available_tools_partitions_builtin_prefix_then_dynamic() {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(NamedTool("Write")));
        r.register_builtin(Arc::new(NamedTool("Bash")));
        let conn = McpConnectionId::new();
        r.register_mcp_tools(
            conn,
            vec![
                Arc::new(NamedTool("mcp__b")) as Arc<dyn Tool>,
                Arc::new(NamedTool("mcp__a")) as Arc<dyn Tool>,
                // Collides with a builtin — must be dropped (uniqBy precedence).
                Arc::new(NamedTool("Bash")) as Arc<dyn Tool>,
            ],
        );
        let names: Vec<String> = r
            .available_tools(&ToolStaticContext::default())
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        // Builtin prefix (locale-sorted), then MCP (locale-sorted); no dup Bash.
        assert_eq!(names, vec!["Bash", "Write", "mcp__a", "mcp__b"]);
    }

    /// Re-registering an MCP connection upserts in place; unregister drops it.
    #[test]
    fn mcp_register_is_order_preserving_upsert() {
        let mut r = ToolRegistry::new();
        let conn = McpConnectionId::new();
        r.register_mcp_tools(conn, vec![Arc::new(NamedTool("mcp__x")) as Arc<dyn Tool>]);
        assert!(r.find_by_name("mcp__x").is_some());
        // Upsert replaces the tool set for the same id.
        r.register_mcp_tools(conn, vec![Arc::new(NamedTool("mcp__y")) as Arc<dyn Tool>]);
        assert!(r.find_by_name("mcp__x").is_none());
        assert!(r.find_by_name("mcp__y").is_some());
        r.unregister_mcp_tools(conn);
        assert!(r.find_by_name("mcp__y").is_none());
    }
}
