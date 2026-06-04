//! `NotebookEditTool` — replace / insert / delete a cell in a Jupyter `.ipynb`.
//!
//! `.ipynb` is JSON with `cells: [{ id, source, ... }, ...]`. We round-trip
//! through `serde_json::Value` so `preserve_order` (workspace feature) keeps
//! key ordering byte-stable.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{NOTEBOOK_COMPLETED, NOTEBOOK_FAILED, NOTEBOOK_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

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
        let invocation_id = tool_api::util::ids::ulid_or_uuid();
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

        // Read-before-write staleness guard (Batch F). The notebook always
        // exists here (the `read_to_string` above errors otherwise), so a prior
        // full read + unchanged-mtime is required (TS
        // `NotebookEditTool.ts:221-237`). We reuse the shared
        // `check_read_before_write` helper for uniformity with Edit/Write,
        // which also applies the isPartialView + content-equality fallback —
        // TS's NotebookEdit guard is the simpler `!lastRead` / `mtime >
        // timestamp` form without those, but the extra checks only ever relax
        // (content-equality) or tighten (partial-view) in cases a notebook does
        // not reach in practice.
        //
        // KNOWN PARTIAL COVERAGE (flagged per spec): the Rust `Read` tool does
        // NOT support `.ipynb` (no notebook media), so it never populates
        // `read_file_state` for a notebook. The guard can therefore only trip
        // on a prior NotebookEdit's OWN post-write `set` below — a plain
        // Read→NotebookEdit cannot satisfy the guard. This gap closes only when
        // Read gains notebook support, which is out of faithful reach (no Rust
        // crate equivalent for the TS notebook media pipeline). `raw` is the
        // current on-disk content for the (rarely-reached) content fallback.
        let current_mtime_ms = tokio::fs::metadata(&canon)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        if let Err(e) = crate::check_read_before_write(
            &self.ctx.read_file_state,
            &canon,
            current_mtime_ms,
            &raw,
        ) {
            self.emit_failed(&invocation_id, "stale_read").await;
            return Err(e);
        }

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

        // Post-write: update the read-state registry so an immediate second
        // NotebookEdit succeeds and the staleness guard sees the new mtime (TS
        // `NotebookEditTool.ts:437-442` `readFileState.set({content:
        // updatedContent, timestamp: <new mtime>, offset: undefined, limit:
        // undefined})`). `serialized` is the just-written notebook JSON;
        // offset/limit cleared so it counts as a full read.
        let new_mtime_ms = tokio::fs::metadata(&canon)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        tool_api::read_file_state::set(
            &self.ctx.read_file_state,
            canon.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: serialized.clone(),
                mtime_ms: new_mtime_ms,
                offset: None,
                limit: None,
            },
        );

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
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tempfile::TempDir;
    use tool_api::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};

    pub(crate) fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
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

    /// Simulate a prior full read of `target` (a notebook) so the
    /// read-before-write staleness guard (Batch F) is satisfied. Records the
    /// file's current content + floored mtime under the canonicalized key with
    /// `offset`/`limit` = `None`. Call AFTER writing the file's bytes.
    ///
    /// NB: the Rust `Read` tool cannot populate this for `.ipynb` (no notebook
    /// media support), so in production only a prior NotebookEdit's own
    /// post-write `set` satisfies the guard — these tests seed it directly to
    /// exercise the post-guard logic. (Documented partial-coverage gap.)
    fn seed_full_read(ctx: &BuiltinToolContext, target: &std::path::Path) {
        let canon = std::fs::canonicalize(target).unwrap();
        let content = std::fs::read_to_string(&canon).unwrap();
        let mtime_ms = std::fs::metadata(&canon)
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        tool_api::read_file_state::set(
            &ctx.read_file_state,
            canon,
            tool_api::read_file_state::ReadFileEntry {
                content,
                mtime_ms,
                offset: None,
                limit: None,
            },
        );
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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

    // ───────────────────────── Batch F: staleness guard ─────────────────────

    #[tokio::test]
    async fn notebook_edit_without_prior_read_errors_not_read() {
        // No recorded read of the notebook → FILE_NOT_READ_ERROR. This is the
        // common production path: Rust `Read` cannot populate the registry for
        // `.ipynb` (no notebook media), so a plain edit-without-edit is refused.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        // Deliberately do NOT seed a prior read.
        let tool = NotebookEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "notebook_path": target.to_str().unwrap(),
                    "cell_id": "c1",
                    "edit_mode": "replace",
                    "new_source": "print('blocked')"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert_eq!(m, crate::FILE_NOT_READ_ERROR),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn post_write_set_lets_immediate_second_notebook_edit_succeed() {
        // First NotebookEdit succeeds with a seeded read; its post-write `set`
        // updates the registry so a SECOND immediate NotebookEdit (no re-seed)
        // also succeeds — the only way the guard is satisfied in production
        // (Read does not populate the registry for notebooks).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
        let tool = NotebookEditTool::new(ctx);
        tool.call(
            json!({
                "notebook_path": target.to_str().unwrap(),
                "cell_id": "c1",
                "edit_mode": "replace",
                "new_source": "print('first')"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // No re-seed: the post-write set from the first edit satisfies the guard.
        tool.call(
            json!({
                "notebook_path": target.to_str().unwrap(),
                "cell_id": "c2",
                "edit_mode": "replace",
                "new_source": "## Updated"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let modified: Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(modified["cells"][0]["source"], "print('first')");
        assert_eq!(modified["cells"][1]["source"], "## Updated");
    }

    #[tokio::test]
    async fn rejects_missing_cell_for_replace() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_notebook()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
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
