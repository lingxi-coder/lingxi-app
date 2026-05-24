//! `NotebookEditTool` — replace / insert / delete a cell in a Jupyter `.ipynb`.
//!
//! `.ipynb` is JSON with `cells: [{ id, source, ... }, ...]`. We round-trip
//! through `serde_json::Value` so `preserve_order` (workspace feature) keeps
//! key ordering byte-stable.

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::shared::path_validation::{canonicalize_and_validate, emit_blocked_event};
use crate::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{NOTEBOOK_COMPLETED, NOTEBOOK_FAILED, NOTEBOOK_STARTED};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "NotebookEdit";

/// Allowed `edit_mode` value: replace a cell's source.
pub const EDIT_MODE_REPLACE: &str = "replace";
/// Allowed `edit_mode` value: insert a new cell after the named anchor.
pub const EDIT_MODE_INSERT: &str = "insert";
/// Allowed `edit_mode` value: delete the named cell.
pub const EDIT_MODE_DELETE: &str = "delete";

/// `NotebookEditTool` — mutate one cell of a Jupyter notebook.
pub struct NotebookEditTool {
    ctx: BuiltinToolContext,
}

impl NotebookEditTool {
    /// Construct a new tool.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str, path: &std::path::Path) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "_PROTO_notebook_path".to_string(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(path.display().to_string()).into_inner(),
            ),
        );
        self.ctx.bus.log_event(NOTEBOOK_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, cells_edited: u32, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "cells_edited".to_string(),
            AnalyticsValue::Int(cells_edited as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(NOTEBOOK_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, kind: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "failure_kind".to_string(),
            AnalyticsValue::String(Verified::assert_safe(kind.to_string()).into_inner()),
        );
        self.ctx.bus.log_event(NOTEBOOK_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["notebook_path", "cell_id", "edit_mode"],
        "properties": {
            "notebook_path": { "type": "string" },
            "cell_id":       { "type": "string" },
            "new_source":    { "type": "string" },
            "edit_mode":     {
                "type": "string",
                "enum": ["replace", "insert", "delete"]
            }
        }
    })
});

#[async_trait]
impl Tool for NotebookEditTool {
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
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        "Replace / insert / delete a cell in a Jupyter notebook.".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Edit a Jupyter cell by id. edit_mode in {replace, insert, delete}.".to_string()
    }

    fn get_path(&self, input: &Value) -> Option<PathBuf> {
        input
            .get("notebook_path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = crate::builtin::file_read::ulid_or_uuid();
        let notebook_path = input
            .get("notebook_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("notebook_path is required".into()))?;
        let cell_id = input
            .get("cell_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("cell_id is required".into()))?;
        let edit_mode = input
            .get("edit_mode")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("edit_mode is required".into()))?;
        let new_source = input
            .get("new_source")
            .and_then(Value::as_str)
            .map(std::string::ToString::to_string);

        let path = PathBuf::from(notebook_path);
        let started = Instant::now();
        self.emit_started(&invocation_id, &path).await;

        let canon = match canonicalize_and_validate(&path, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path });
            }
        };

        let raw = match tokio::fs::read_to_string(&canon).await {
            Ok(s) => s,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_read").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };

        let mut nb: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                self.emit_failed(&invocation_id, "json_parse").await;
                return Err(ToolError::Io(format!("notebook JSON parse: {e}")));
            }
        };

        let cells = nb
            .get_mut("cells")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| ToolError::InvalidInput("notebook missing `cells` array".into()))?;

        let idx = cells
            .iter()
            .position(|c| c.get("id").and_then(Value::as_str) == Some(cell_id));

        match edit_mode {
            EDIT_MODE_REPLACE => {
                let i = idx.ok_or_else(|| {
                    ToolError::InvalidInput(format!("cell_id {cell_id} not found"))
                })?;
                let src = new_source.ok_or_else(|| {
                    ToolError::InvalidInput("new_source required for replace".into())
                })?;
                cells[i]["source"] = json!(src);
            }
            EDIT_MODE_INSERT => {
                let src = new_source.ok_or_else(|| {
                    ToolError::InvalidInput("new_source required for insert".into())
                })?;
                let new_cell = json!({
                    "cell_type": "code",
                    "id": cell_id,
                    "source": src,
                    "metadata": {},
                    "outputs": [],
                    "execution_count": null
                });
                match idx {
                    Some(i) => cells.insert(i + 1, new_cell),
                    None => cells.push(new_cell),
                }
            }
            EDIT_MODE_DELETE => {
                let i = idx.ok_or_else(|| {
                    ToolError::InvalidInput(format!("cell_id {cell_id} not found"))
                })?;
                cells.remove(i);
            }
            other => {
                self.emit_failed(&invocation_id, "bad_edit_mode").await;
                return Err(ToolError::InvalidInput(format!(
                    "unknown edit_mode {other:?}"
                )));
            }
        }

        let serialized = match serde_json::to_string_pretty(&nb) {
            Ok(s) => s,
            Err(e) => {
                self.emit_failed(&invocation_id, "json_emit").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };
        if let Err(e) = tokio::fs::write(&canon, serialized.as_bytes()).await {
            self.emit_failed(&invocation_id, "io_write").await;
            return Err(ToolError::Io(e.to_string()));
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, 1, duration_ms).await;

        Ok(ToolCallResult {
            data: json!({ "cells_edited": 1 }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};
    use lingxi_telemetry::{AnalyticsBus, InMemorySink};
    use std::sync::Arc;
    use tempfile::TempDir;

    pub(crate) fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        (
            BuiltinToolContext {
                fs: make_dummy_fs(),
                bus,
                trusted_dirs: vec![tmp.path().to_path_buf()],
            },
            sink,
        )
    }

    pub(crate) fn sample_notebook() -> String {
        serde_json::to_string_pretty(&json!({
            "cells": [
                { "cell_type": "code", "id": "c1", "source": "print('hello')", "metadata": {}, "outputs": [], "execution_count": null },
                { "cell_type": "markdown", "id": "c2", "source": "# Heading", "metadata": {} }
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        })).unwrap()
    }

    #[test]
    fn tool_name_is_notebook_edit() {
        assert_eq!(TOOL_NAME, "NotebookEdit");
    }

    #[tokio::test]
    async fn replace_mode_swaps_source() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let _ = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "replace",
                    "new_source": "print('world')"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let modified: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(modified["cells"][0]["source"], "print('world')");
    }

    #[tokio::test]
    async fn delete_mode_removes_cell() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let _ = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "delete"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let modified: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        let cells = modified["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0]["id"], "c2");
    }

    #[tokio::test]
    async fn insert_mode_adds_cell_after_target() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let _ = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "insert",
                    "new_source": "print('inserted')"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let modified: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        let cells = modified["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[1]["source"], "print('inserted')");
    }

    #[tokio::test]
    async fn rejects_path_outside_trusted() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "delete"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::PathBlocked { .. } => {}
            other => panic!("expected PathBlocked, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bad.ipynb");
        std::fs::write(&target, "this is not json").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "delete"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("notebook JSON parse"));
    }

    #[tokio::test]
    async fn rejects_missing_cell_for_replace() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = NotebookEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "nonexistent",
                    "edit_mode": "replace",
                    "new_source": "x"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
