//! `LspTool` — exposes LSP capabilities through the engine's `Tool`
//! interface.
//!
//! M1.18 ships the contract surface (input parsing, permission shape,
//! schema). The production dispatch lands in Plan 16 once the
//! posix-minimal `LspTransport` is available.
//!
//! See spec §25.4 (LSP tool).

use crate::action::LspAction;
use crate::registry::LspRegistry;
use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_tools::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolProgressSender,
    ToolStaticContext, ToolUseContext,
};
use once_cell::sync::Lazy;
use serde_json::Value;
use std::sync::Arc;

/// Tool implementation that forwards LSP actions to the [`LspRegistry`].
pub struct LspTool {
    /// Shared registry; kept here so future dispatch wiring can route
    /// through it without changing the constructor.
    #[allow(dead_code)] // Plan 16 wires registry-driven dispatch.
    registry: Arc<LspRegistry>,
}

impl LspTool {
    /// Build a new `LspTool` over `registry`.
    #[must_use]
    pub fn new(registry: Arc<LspRegistry>) -> Self {
        Self { registry }
    }
}

/// Static JSON-Schema for the LSP tool's input. M1 ships the loose
/// `{type: object}` schema; tighter validation lands in Plan 16.
static LSP_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| serde_json::json!({ "type": "object" }));

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "LSP"
    }

    fn input_schema(&self) -> &Value {
        &LSP_INPUT_SCHEMA
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        65_536
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_lsp(&self) -> bool {
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "lsp tool".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Invoke LSP".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        String::new()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // Validate the input shape — production dispatch lands in Plan 16.
        let _action: LspAction = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        Ok(ToolCallResult {
            data: serde_json::json!({ "stub": true }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}
