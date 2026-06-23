//! `MultiEditTool` — batch file-edit routing (claude-code parity).
//!
//! # Binary evidence
//! `MultiEdit` appears in the built-in-tool-names array at offset 188243808
//! (`Read,Write,Edit,MultiEdit,Bash,Glob,Grep,…`), the activity map `hAm`
//! (`MultiEdit:"Editing"` @ 206110348), and the dispatch shim `V4l` (@
//! 206690719): `case PE.name: if("edits" in t){let{old_string,new_string,
//! replace_all,...s}=t; return s}`. The binary strips the `edits` array from
//! the persisted input and returns the rest (the `{file_path}` shell), treating
//! `edits[0]` as the single replacement op.
//!
//! # Parity disposition
//! MultiEdit is NOT a separately-registered `Ks()` tool in the binary — it is
//! NAME-ROUTED into the Edit handler via the `V4l` dispatch shim. When the
//! model emits `tool_use{name:"MultiEdit", input:{file_path, edits:[{old_string,
//! new_string, replace_all?}]}}`, the binary:
//!   1. Detects `"edits" in input`.
//!   2. Extracts `edits[0]` and builds a flat `{file_path, old_string,
//!      new_string, replace_all}` shape.
//!   3. Dispatches to `PE` (FileEditTool) with that shape.
//!
//! LingXi's `MultiEditTool` replicates this: it registers under the name
//! `"MultiEdit"` with a schema accepting `{file_path, edits: []}`, then on
//! `call()` it unpacks `edits[0]` and delegates to [`crate::FileEditTool`]'s
//! call logic by re-using `FileEditTool::new(ctx).call(flat_input, …)`.
//!
//! Only `edits[0]` is executed (matching the binary's `V4l` shim, which picks
//! the FIRST element). This is an important parity detail: the model is expected
//! to emit one MultiEdit per operation; the `edits` wrapper is a structural
//! artifact from the input schema, not a bulk-apply directive.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::path::PathBuf;
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock — matches claude-code's built-in-tool-names array entry.
pub const TOOL_NAME: &str = "MultiEdit";

/// MultiEdit input schema — `{file_path, edits: [{old_string, new_string,
/// replace_all?}]}`. The `edits` array schema mirrors the Edit tool's fields
/// (same descriptions, same types, same `replace_all` default). The binary's
/// `L$n()` (Edit schema) uses `strictObject`; `edits` items use the same shape.
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file_path", "edits"],
        "properties": {
            "file_path": {
                "type": "string",
                "description": "The absolute path to the file to modify"
            },
            "edits": {
                "type": "array",
                "description": "An array of edits to apply to the file",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["old_string", "new_string"],
                    "properties": {
                        "old_string":  { "type": "string", "description": "The text to replace" },
                        "new_string":  { "type": "string", "description": "The text to replace it with (must be different from old_string)" },
                        "replace_all": { "type": "boolean", "default": false, "description": "Replace all occurrences of old_string (default false)" }
                    }
                }
            }
        }
    })
});

/// `MultiEditTool` — name-routes into [`crate::FileEditTool`] via the `edits`
/// array dispatch (parity with claude-code's `V4l` shim).
pub struct MultiEditTool {
    ctx: BuiltinToolContext,
}

impl MultiEditTool {
    /// Construct a new tool.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-01 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        // The binary's `hAm` activity-map entry is `MultiEdit:"Editing"` — the
        // tool's display label is the same "A tool for editing files" description
        // as Edit (they share `PE`). Mirror that here.
        "A tool for editing files".to_string()
    }

    async fn prompt(&self, opts: &PromptOptions) -> String {
        // Same prompt as FileEditTool — the binary routes through the same `PE`
        // registration for both names.
        crate::FileEditTool::new(self.ctx.clone()).prompt(opts).await
    }

    fn get_path(&self, input: &Value) -> Option<PathBuf> {
        input
            .get("file_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // Extract `file_path` and `edits[0]` — mirror the binary's `V4l` dispatch
        // shim: `if("edits" in t){ let{old_string,new_string,replace_all,...s}=t;
        // return s}`. Only the first edit in the array is applied (the binary's
        // `V4l` destructures `edits[0]` — subsequent elements are silently
        // discarded at the dispatch layer). This matches the binary.
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;

        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::InvalidInput("edits array is required".into()))?;

        let first = edits
            .first()
            .ok_or_else(|| ToolError::InvalidInput("edits array must not be empty".into()))?;

        let old_string = first
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("edits[0].old_string is required".into()))?;
        let new_string = first
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("edits[0].new_string is required".into()))?;
        let replace_all = first
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Build the flat Edit input and delegate to FileEditTool.
        let flat = json!({
            "file_path": file_path,
            "old_string": old_string,
            "new_string": new_string,
            "replace_all": replace_all
        });

        crate::FileEditTool::new(self.ctx.clone())
            .call(flat, ctx, tx)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;
    use tool_api::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};
    use tool_api::BuiltinToolContext;
    use tool_api::read_file_state::{set, ReadFileEntry};
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};

    fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        (
            tool_api::test_support::ctx_for_file_tools(
                make_dummy_fs(),
                bus,
                vec![tmp.path().to_path_buf()],
            ),
            sink,
        )
    }

    fn seed_full_read(ctx: &BuiltinToolContext, target: &std::path::Path) {
        let canon = std::fs::canonicalize(target).unwrap();
        let bytes = std::fs::read(&canon).unwrap();
        let content = crate::shared::decode_utf8_strict(&bytes)
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
        let mtime_ms = std::fs::metadata(&canon)
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        set(
            &ctx.read_file_state,
            canon,
            ReadFileEntry {
                content,
                mtime_ms,
                offset: None,
                limit: None,
                from_read: true,
            },
        );
    }

    #[test]
    fn tool_name_is_multi_edit() {
        assert_eq!(TOOL_NAME, "MultiEdit");
    }

    #[test]
    fn schema_has_file_path_and_edits_array() {
        let schema = MultiEditTool::new(
            tool_api::test_support::ctx_for_file_tools(
                make_dummy_fs(),
                Arc::new(AnalyticsBus::new()),
                vec![],
            ),
        );
        let s = schema.input_schema();
        assert_eq!(s["type"], "object");
        assert!(s["properties"]["file_path"].is_object());
        assert!(s["properties"]["edits"].is_object());
        assert_eq!(s["properties"]["edits"]["type"], "array");
    }

    #[tokio::test]
    async fn multi_edit_applies_first_edit() {
        // MultiEdit with a single-element edits array applies the first
        // (and only) edit via FileEditTool dispatch — binary V4l parity.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("me.txt");
        std::fs::write(&target, "hello world").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
        let tool = MultiEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "edits": [{"old_string": "world", "new_string": "Rust"}]
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 1);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello Rust");
    }

    #[tokio::test]
    async fn multi_edit_uses_first_edit_only() {
        // Binary V4l only dispatches edits[0]; subsequent elements are ignored.
        // Verify: even with two edits in the array, only the first is applied.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("me2.txt");
        std::fs::write(&target, "alpha beta gamma").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
        let tool = MultiEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "edits": [
                        {"old_string": "alpha", "new_string": "ALPHA"},
                        {"old_string": "beta",  "new_string": "BETA"}
                    ]
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 1);
        // Only "alpha" → "ALPHA" applied; "beta" unchanged.
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "ALPHA beta gamma"
        );
    }

    #[tokio::test]
    async fn multi_edit_requires_non_empty_edits() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("me3.txt");
        std::fs::write(&target, "content").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = MultiEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "edits": []
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "expected 'must not be empty', got: {err}"
        );
    }
}
