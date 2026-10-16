//! Shared test tool fixture for the bridge-server e2e suites.
//!
//! `AlwaysOkTool` mirrors the fixture used across the orchestrator streaming
//! tests (`orchestrator/tests/streaming_mid_stream_tool_test.rs`): a trivial
//! read-only tool that always succeeds. The orchestrator-level
//! `AdapterPermissionGate` runs `check()` for it BEFORE dispatch (the live
//! source of the permission round-trip), so the tool's own `check_permissions`
//! is irrelevant to the gate handshake — it just needs to return a result once
//! approved.

#![allow(dead_code)]

use async_trait::async_trait;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolInputRepairMetadata,
    ToolInputRepairSchema, ToolInputSchemaProvenance, ToolStaticContext, ValidationError,
};

/// Read-only tool that accepts any input and returns `{ "ok": true }`.
pub struct AlwaysOkTool;

#[async_trait]
impl Tool for AlwaysOkTool {
    fn name(&self) -> &str {
        "AlwaysOk"
    }
    fn input_schema(&self) -> &Utf16JsonProjection {
        static SCHEMA: once_cell::sync::Lazy<Utf16JsonProjection> =
            once_cell::sync::Lazy::new(|| {
                Utf16JsonProjection::parse(r#"{"type":"object"}"#)
                    .expect("test schema is valid JSON")
            });
        &SCHEMA
    }
    fn input_repair_metadata(&self) -> ToolInputRepairMetadata {
        ToolInputRepairMetadata::Known {
            schema: ToolInputRepairSchema::Advertised,
            schema_provenance: ToolInputSchemaProvenance::GeneratedBuiltin,
            exempt_top_level_keys: None,
        }
    }
    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024
    }
    fn is_concurrency_safe(&self, _input: &Utf16JsonProjection) -> bool {
        true
    }
    fn is_read_only(&self, _input: &Utf16JsonProjection) -> bool {
        true
    }
    async fn validate_input(
        &self,
        _input: &Utf16JsonProjection,
        _ctx: &tool_api::context::ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }
    async fn check_permissions(
        &self,
        _input: &Utf16JsonProjection,
        _ctx: &tool_api::context::ToolUseContext,
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
        _input: &Utf16JsonProjection,
        _opts: &DescriptionOptions,
    ) -> String {
        "AlwaysOk".into()
    }
    async fn prompt(&self, _opts: &PromptOptions) -> Utf16JsonProjection {
        String::new().into()
    }
    async fn call(
        &self,
        _input: Utf16JsonProjection,
        _ctx: tool_api::context::ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let data = Utf16JsonProjection::parse(r#"{"ok":true}"#).expect("test result is valid JSON");
        Ok(ToolCallResult::from_data(data))
    }
}
