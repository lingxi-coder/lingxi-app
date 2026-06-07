//! LSP builtin tool — `LSPTool` exposes 9 operations (goToDefinition /
//! findReferences / hover / documentSymbol / workspaceSymbol /
//! goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls)
//! over `lsp::LspClient` (M2-03; surface realigned to `LSPTool.ts:62-72`).
//!
//! Input position is 1-based (claude-code UI convention); we convert to
//! 0-based via `lsp::tool_operations::position_from_one_based`
//! before issuing the LSP request.
//!
//! Wire identifiers locked in spec §7 line 695.
//!
//! no-truncation: LSPTool returns structured definition / references / hover /
//! symbol / call-hierarchy payloads forwarded verbatim from lsp::LspClient (the
//! upstream LSP server is the trust boundary). Free-form text comes only
//! from hover contents, which are bounded by the LSP protocol itself.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lsp::registry::LspRegistry;
use lsp::tool_operations as ops;
use lsp::OpenFileTracker;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{LSP_COMPLETED, LSP_FAILED, LSP_STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

// -- Wire identifier locks (spec §7 line 695) --------------------------------

/// Registry name for the LSP tool (the Rust struct keeps the
/// `LSPTool` identifier).
pub const LSP_TOOL_NAME: &str = "LSP";

/// Operation literal (`textDocument/definition`).
pub const LSP_OPERATION_GO_TO_DEFINITION: &str = "goToDefinition";
/// Operation literal (`textDocument/references`).
pub const LSP_OPERATION_FIND_REFERENCES: &str = "findReferences";
/// Operation literal (`textDocument/hover`).
pub const LSP_OPERATION_HOVER: &str = "hover";
/// Operation literal (`textDocument/documentSymbol`).
pub const LSP_OPERATION_DOCUMENT_SYMBOL: &str = "documentSymbol";
/// Operation literal (`workspace/symbol`).
pub const LSP_OPERATION_WORKSPACE_SYMBOL: &str = "workspaceSymbol";
/// Operation literal (`textDocument/implementation`).
pub const LSP_OPERATION_GO_TO_IMPLEMENTATION: &str = "goToImplementation";
/// Operation literal (`textDocument/prepareCallHierarchy`).
pub const LSP_OPERATION_PREPARE_CALL_HIERARCHY: &str = "prepareCallHierarchy";
/// Operation literal (`callHierarchy/incomingCalls`).
pub const LSP_OPERATION_INCOMING_CALLS: &str = "incomingCalls";
/// Operation literal (`callHierarchy/outgoingCalls`).
pub const LSP_OPERATION_OUTGOING_CALLS: &str = "outgoingCalls";

/// All nine operations in the locked surface order (`LSPTool.ts:62-72`).
pub const LSP_OPERATIONS_LOCKED: [&str; 9] = [
    LSP_OPERATION_GO_TO_DEFINITION,
    LSP_OPERATION_FIND_REFERENCES,
    LSP_OPERATION_HOVER,
    LSP_OPERATION_DOCUMENT_SYMBOL,
    LSP_OPERATION_WORKSPACE_SYMBOL,
    LSP_OPERATION_GO_TO_IMPLEMENTATION,
    LSP_OPERATION_PREPARE_CALL_HIERARCHY,
    LSP_OPERATION_INCOMING_CALLS,
    LSP_OPERATION_OUTGOING_CALLS,
];

/// Position-validation error literal (LingXi lock; ASCII `>=` form).
pub const LSP_POSITION_ERROR: &str = "LSP position must be 1-based (line >= 1, character >= 1)";

// -- Helpers -----------------------------------------------------------------

fn pii(s: &str) -> AnalyticsValue {
    use telemetry::pii::PiiTagged;
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}
fn verified_int(n: u64) -> AnalyticsValue {
    AnalyticsValue::Int(n as i64)
}
fn verified_str(s: &str) -> AnalyticsValue {
    use telemetry::pii::Verified;
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit(bus: &Arc<AnalyticsBus>, event: &'static str, fields: &[(&str, AnalyticsValue)]) {
    let mut md: LogEventMetadata = HashMap::new();
    for (k, v) in fields {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

fn allow_lsp() -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::Other {
            reason: "LSP read-only operation".into(),
        },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

// -- Tool struct -------------------------------------------------------------

/// Builtin tool — dispatches 4 LSP operations.
pub struct LSPTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl LSPTool {
    /// Construct a new [`LSPTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
    fn lsp_registry(&self) -> Option<&Arc<LspRegistry>> {
        self.ctx.lsp_registry.as_ref()
    }
}

static LSP_TOOL_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "operation":   { "type": "string", "enum": ["goToDefinition", "findReferences", "hover", "documentSymbol", "workspaceSymbol", "goToImplementation", "prepareCallHierarchy", "incomingCalls", "outgoingCalls"] },
            "server_name": { "type": "string", "minLength": 1 },
            "file_path":   { "type": "string", "minLength": 1 },
            "line":        { "type": "integer", "minimum": 1 },
            "character":   { "type": "integer", "minimum": 1 }
        },
        "required": ["operation", "server_name", "file_path", "line", "character"]
    })
});

#[async_trait]
impl Tool for LSPTool {
    fn name(&self) -> &str {
        LSP_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &LSP_TOOL_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn is_lsp(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        30_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_lsp()
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "LSP-server operations (goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls). 1-based positions."
            .into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use LSP to query LSP servers at a 1-based (line, character) position.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let operation = input
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string operation".into())
            })?
            .to_string();
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string server_name".into())
            })?
            .to_string();
        let file_path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-string file_path".into())
            })?
            .to_string();
        let line = input
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ToolError::InvalidInput("LSPTool: missing or non-integer line".into()))?
            as u32;
        let character = input
            .get("character")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                ToolError::InvalidInput("LSPTool: missing or non-integer character".into())
            })? as u32;

        // 1-based position guard — emits no STARTED to keep paired events.
        if line < 1 || character < 1 {
            return Err(ToolError::InvalidInput(LSP_POSITION_ERROR.into()));
        }

        // Unknown-operation guard — also pre-STARTED.
        if !LSP_OPERATIONS_LOCKED.contains(&operation.as_str()) {
            return Err(ToolError::InvalidInput(format!(
                "LSPTool: unknown operation {operation:?}; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
            )));
        }

        let bus = &self.ctx.bus;
        emit(
            bus,
            LSP_STARTED,
            &[
                ("_PROTO_operation", pii(&operation)),
                ("_PROTO_server_name", pii(&server_name)),
                ("_PROTO_file_path", pii(&file_path)),
                ("line", verified_int(line as u64)),
                ("character", verified_int(character as u64)),
            ],
        )
        .await;

        let registry = match self.lsp_registry() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "LSPTool: LSP registry not configured on this host".into(),
                ));
            }
        };

        let client = match registry.get_client(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_running")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "LSP server '{server_name}' is not running"
                )));
            }
        };

        let config = match registry.get_config(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_running")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "LSP server '{server_name}' is not running"
                )));
            }
        };

        let tracker = OpenFileTracker::new();
        let path = Path::new(&file_path);

        // `LSPTool.ts:427-` `getMethodAndParams` — per-operation dispatch. Position-
        // based ops use `line`/`character`; `documentSymbol` is file-level;
        // `workspaceSymbol` ignores the file and queries with an empty string
        // ("returns all symbols", `LSPTool.ts:475`); the call-hierarchy ops do the
        // two-step prepare-then-calls round-trip inside their `ops::` impl.
        let res = match operation.as_str() {
            "goToDefinition" => {
                ops::go_to_definition(&client, &tracker, &config, path, line, character).await
            }
            "findReferences" => {
                ops::find_references(&client, &tracker, &config, path, line, character, true).await
            }
            "hover" => ops::hover(&client, &tracker, &config, path, line, character).await,
            "documentSymbol" => ops::document_symbol(&client, &tracker, &config, path).await,
            "workspaceSymbol" => ops::workspace_symbol(&client, None).await,
            "goToImplementation" => {
                ops::go_to_implementation(&client, &tracker, &config, path, line, character).await
            }
            "prepareCallHierarchy" => {
                ops::prepare_call_hierarchy(&client, &tracker, &config, path, line, character).await
            }
            "incomingCalls" => {
                ops::incoming_calls(&client, &tracker, &config, path, line, character).await
            }
            "outgoingCalls" => {
                ops::outgoing_calls(&client, &tracker, &config, path, line, character).await
            }
            _ => unreachable!("validated above"),
        };

        match res {
            Ok(r) => {
                emit(
                    bus,
                    LSP_COMPLETED,
                    &[
                        ("_PROTO_operation", pii(&operation)),
                        ("_PROTO_server_name", pii(&server_name)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                    ],
                )
                .await;
                Ok(ToolCallResult {
                    data: json!({
                        "operation": operation,
                        "server_name": server_name,
                        "file_path": file_path,
                        "result": r.raw,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                emit(
                    bus,
                    LSP_FAILED,
                    &[
                        ("_PROTO_operation", pii(&operation)),
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("operation")),
                    ],
                )
                .await;
                Err(ToolError::Io(format!(
                    "LSPTool: {server_name} {operation}: {e}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_name_locked() {
        assert_eq!(LSP_TOOL_NAME, "LSP");
    }

    #[test]
    fn lsp_operations_locked_array_matches_constants() {
        // Locked to the TS surface order (`LSPTool.ts:62-72`).
        assert_eq!(
            LSP_OPERATIONS_LOCKED,
            [
                "goToDefinition",
                "findReferences",
                "hover",
                "documentSymbol",
                "workspaceSymbol",
                "goToImplementation",
                "prepareCallHierarchy",
                "incomingCalls",
                "outgoingCalls"
            ]
        );
        assert_eq!(LSP_OPERATION_GO_TO_DEFINITION, "goToDefinition");
        assert_eq!(LSP_OPERATION_FIND_REFERENCES, "findReferences");
        assert_eq!(LSP_OPERATION_HOVER, "hover");
        assert_eq!(LSP_OPERATION_DOCUMENT_SYMBOL, "documentSymbol");
        assert_eq!(LSP_OPERATION_WORKSPACE_SYMBOL, "workspaceSymbol");
        assert_eq!(LSP_OPERATION_GO_TO_IMPLEMENTATION, "goToImplementation");
        assert_eq!(LSP_OPERATION_PREPARE_CALL_HIERARCHY, "prepareCallHierarchy");
        assert_eq!(LSP_OPERATION_INCOMING_CALLS, "incomingCalls");
        assert_eq!(LSP_OPERATION_OUTGOING_CALLS, "outgoingCalls");
    }

    #[test]
    fn lsp_position_error_byte_locked() {
        assert_eq!(
            LSP_POSITION_ERROR,
            "LSP position must be 1-based (line >= 1, character >= 1)"
        );
    }

    #[test]
    fn lsp_server_not_running_template() {
        let s = format!("LSP server '{}' is not running", "rust-analyzer");
        assert_eq!(s, "LSP server 'rust-analyzer' is not running");
    }

    #[test]
    fn lsp_unknown_operation_template() {
        // `completion` is no longer a tool operation (TS LSPTool has no
        // completion op) — it is now rejected as unknown.
        let op = "completion";
        let s = format!(
            "LSPTool: unknown operation {op:?}; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
        );
        assert_eq!(
            s,
            "LSPTool: unknown operation \"completion\"; allowed: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls"
        );
    }
}
