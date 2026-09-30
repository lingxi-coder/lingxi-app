//! Shared test tool fixture for the bridge-server computer-access e2e suite.
//!
//! `AccessRequestingTool` mirrors `tool_fixture::AlwaysOkTool` (shared across
//! the bridge-server e2e suites): a trivial tool whose `call()` deterministically
//! triggers exactly ONE `request_access`-shaped round-trip through a REAL
//! `tool_computer_use::ComputerAccessResolver` — the SAME resolver trait (and,
//! in the e2e test, the SAME `TuiBridgeResolver` impl) the production `computer`
//! tool's `handle_request_access` calls. This fixture exists because driving the
//! real `computer` tool end to end would additionally require a live
//! `ComputerControl` backend, per-app bundle-id resolution, and tier
//! persistence — none of which is what this suite is proving; it is proving the
//! wire round-trip (`ComputerAccessExchange` → `Frame::ComputerAccessRequest` →
//! `ApproveComputerAccess`/`DenyComputerAccess` → the resolver's `resolve()`
//! future completes), so the fixture calls the resolver directly with a
//! hard-coded request, exactly like `handle_request_access` does internally.

#![allow(dead_code)]

use async_trait::async_trait;
use permission::computer_access::{AccessTier, ComputerAccessRequest, RequestedApp};
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::json;
use std::sync::Arc;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_computer_use::ComputerAccessResolver;

/// A tool whose `call()` requests access to `["Slack", "Chrome"]` at `Full`
/// tier via the injected resolver, then reports the granted subset back as its
/// result — the minimal fixture needed to exercise the computer-access wire
/// round-trip without the real `computer` tool's `ComputerControl` backend.
pub struct AccessRequestingTool {
    pub access_resolver: Arc<dyn ComputerAccessResolver>,
}

#[async_trait]
impl Tool for AccessRequestingTool {
    fn name(&self) -> &str {
        "AccessRequest"
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
        false
    }
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> PermissionResult {
        // The generic gate is irrelevant to this suite (a `NoOpPermissionGate`
        // is bound at the orchestrator level) — `request_access` bypasses it
        // entirely in production too (see `tool_computer_use`'s own doc
        // comment), so this always allows.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "test".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String {
        "AccessRequest".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> String {
        String::new()
    }
    async fn call(
        &self,
        _input: serde_json::Value,
        _ctx: tool_api::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let request = ComputerAccessRequest {
            reason: "automate chat".to_string(),
            apps: vec![
                RequestedApp {
                    label: "Slack".to_string(),
                },
                RequestedApp {
                    label: "Chrome".to_string(),
                },
            ],
            tier: AccessTier::Full,
            clipboard_read: false,
            clipboard_write: false,
            system_key_combos: false,
            tcc_state: None,
        };
        let response = self.access_resolver.resolve(request).await;
        Ok(ToolCallResult {
            data: json!({
                "granted_apps": response.granted_apps,
                "clipboard_read": response.clipboard_read,
                "clipboard_write": response.clipboard_write,
                "system_key_combos": response.system_key_combos,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}
