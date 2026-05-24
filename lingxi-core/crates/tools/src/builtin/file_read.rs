//! `FileReadTool` — read a UTF-8 file with size guard + binary detection.
//!
//! Wire-locked constants:
//! - `MAX_FILE_READ_SIZE = 262_144` bytes (256 KB) per spec §7.
//! - Binary detection scans first `NUL_SCAN_WINDOW = 8 * 1024` bytes for NUL.
//! - Default encoding UTF-8 (BOM-aware) per spec §7.
//! - 1-based line indexing on tool input/output per spec §7.
//! - Errors carry byte-locked human strings (see [`format_too_large`],
//!   [`format_binary`]).

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::shared::file_kit::{decode_utf8_strict, looks_binary, NUL_SCAN_WINDOW};
use crate::shared::path_validation::{canonicalize_and_validate, emit_blocked_event};
use crate::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{READ_COMPLETED, READ_FAILED, READ_STARTED};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Maximum file size FileReadTool will load. Spec §7 lock (256 KB).
pub const MAX_FILE_READ_SIZE: u64 = 262_144;

/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Read";

/// Build the byte-locked too-large error message per spec §5.
#[must_use]
pub fn format_too_large(path: &std::path::Path, size: u64) -> String {
    format!(
        "File {} ({}B) exceeds 256KB read limit",
        path.display(),
        size
    )
}

/// Build the byte-locked binary-file error message per spec §5.
#[must_use]
pub fn format_binary(path: &std::path::Path) -> String {
    format!(
        "File {} appears to be binary (first 8KB contains NUL bytes)",
        path.display()
    )
}

/// `FileReadTool` — reads a UTF-8 file inside the trusted-dirs whitelist.
pub struct FileReadTool {
    ctx: BuiltinToolContext,
}

impl FileReadTool {
    /// Construct a new tool. Cheap — only clones the shared `Arc`s.
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
            "_PROTO_file_path".to_string(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(path.display().to_string()).into_inner(),
            ),
        );
        self.ctx.bus.log_event(READ_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, bytes_read: u64, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "bytes_read".to_string(),
            AnalyticsValue::Int(bytes_read as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(READ_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, failure_kind: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "failure_kind".to_string(),
            AnalyticsValue::String(Verified::assert_safe(failure_kind.to_string()).into_inner()),
        );
        self.ctx.bus.log_event(READ_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file_path"],
        "properties": {
            "file_path": { "type": "string" },
            "offset": { "type": "integer", "minimum": 1 },
            "limit":  { "type": "integer", "minimum": 1 }
        }
    })
});

#[async_trait]
impl Tool for FileReadTool {
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
        true
    }
    fn is_read_only(&self, _input: &Value) -> bool {
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
        "Read a file from the workspace.".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Read a UTF-8 text file. Returns content, line range, total lines.".to_string()
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
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = ulid_or_uuid();
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;
        let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(1);
        let limit = input.get("limit").and_then(Value::as_u64);

        let started = Instant::now();
        let path = PathBuf::from(file_path);
        self.emit_started(&invocation_id, &path).await;

        let canon = match canonicalize_and_validate(&path, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path });
            }
        };

        let size = match tokio::fs::metadata(&canon).await {
            Ok(m) => m.len(),
            Err(e) => {
                self.emit_failed(&invocation_id, "io_metadata").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };
        if size > MAX_FILE_READ_SIZE {
            self.emit_failed(&invocation_id, "file_too_large").await;
            return Err(ToolError::Io(format_too_large(&canon, size)));
        }

        let bytes = match tokio::fs::read(&canon).await {
            Ok(b) => b,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_read").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };

        let head = &bytes[..bytes.len().min(NUL_SCAN_WINDOW)];
        if looks_binary(head) {
            self.emit_failed(&invocation_id, "binary_file").await;
            return Err(ToolError::Io(format_binary(&canon)));
        }

        let content = match decode_utf8_strict(&bytes) {
            Ok(s) => s,
            Err(_) => {
                self.emit_failed(&invocation_id, "non_utf8").await;
                return Err(ToolError::Io(format!(
                    "File {} is not valid UTF-8",
                    canon.display()
                )));
            }
        };

        let all_lines: Vec<&str> = content.split_inclusive('\n').collect();
        let total_lines = all_lines.len() as u64;
        let start_idx = (offset.saturating_sub(1) as usize).min(all_lines.len());
        let end_idx = match limit {
            Some(l) => (start_idx + l as usize).min(all_lines.len()),
            None => all_lines.len(),
        };
        let slice: String = all_lines[start_idx..end_idx].concat();
        let line_range_start = offset;
        let line_range_end = end_idx as u64;

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, bytes.len() as u64, duration_ms)
            .await;

        Ok(ToolCallResult {
            data: json!({
                "content": slice,
                "line_range": [line_range_start, line_range_end],
                "total_lines": total_lines
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

pub(crate) fn ulid_or_uuid() -> String {
    // M4-01 uses a tiny counter-based ID until M4-09 wires a real uuid crate.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    format!("inv-{micros}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};
    use lingxi_telemetry::{AnalyticsBus, InMemorySink};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let fs = make_dummy_fs();
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        (
            BuiltinToolContext {
                fs,
                bus,
                trusted_dirs: vec![tmp.path().to_path_buf()],
            },
            sink,
        )
    }

    #[test]
    fn max_size_byte_locked() {
        assert_eq!(MAX_FILE_READ_SIZE, 262_144);
    }

    #[test]
    fn tool_name_is_read() {
        assert_eq!(TOOL_NAME, "Read");
    }

    #[test]
    fn too_large_message_byte_locked() {
        let msg = format_too_large(std::path::Path::new("/tmp/x"), 300_000);
        assert_eq!(msg, "File /tmp/x (300000B) exceeds 256KB read limit");
    }

    #[test]
    fn binary_message_byte_locked() {
        let msg = format_binary(std::path::Path::new("/tmp/x"));
        assert_eq!(
            msg,
            "File /tmp/x appears to be binary (first 8KB contains NUL bytes)"
        );
    }

    #[tokio::test]
    async fn happy_path_reads_full_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello\nworld\n").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "hello\nworld\n");
        assert_eq!(result.data["total_lines"], 2);
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_read_started"));
        assert!(names.contains(&"tengu_tool_read_completed"));
    }

    #[tokio::test]
    async fn rejects_oversize_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big.txt");
        std::fs::write(&target, vec![b'A'; (MAX_FILE_READ_SIZE + 1) as usize]).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exceeds 256KB read limit"), "got: {msg}");
    }

    #[tokio::test]
    async fn rejects_binary_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bin");
        let mut data = vec![b'A'; 100];
        data[10] = 0;
        std::fs::write(&target, &data).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("appears to be binary (first 8KB contains NUL bytes)"));
    }

    #[tokio::test]
    async fn offset_1_returns_from_first_line() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 1, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line1\n");
    }

    #[tokio::test]
    async fn offset_2_skips_one_line() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line2\n");
    }

    #[tokio::test]
    async fn limit_greater_than_remaining_returns_what_exists() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 50 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "line2\n");
    }

    #[tokio::test]
    async fn no_offset_no_limit_returns_full_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "alpha\nbeta\ngamma\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "alpha\nbeta\ngamma\n");
        assert_eq!(result.data["total_lines"], 3);
    }

    #[tokio::test]
    async fn utf8_bom_stripped_on_read() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bom.txt");
        // BOM + "hello"
        let mut content = vec![0xEF, 0xBB, 0xBF];
        content.extend_from_slice(b"hello");
        std::fs::write(&target, &content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "hello");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_path_outside_trusted() {
        // Deviation from plan: std::fs::canonicalize requires every path
        // component to exist, so a bare outside path errors via Io rather
        // than Outside. Use a symlink inside the trusted tempdir pointing
        // at an outside location — canonicalize follows the symlink and
        // produces a real "Outside" rejection.
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target_real = outside.path().join("a.txt");
        std::fs::write(&target_real, "x").unwrap();
        let link = tmp.path().join("escape");
        std::os::unix::fs::symlink(&target_real, &link).unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileReadTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": link.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::PathBlocked { .. } => {}
            other => panic!("expected PathBlocked, got {other:?}"),
        }
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_file_path_blocked"));
        assert!(names.contains(&"tengu_tool_read_failed"));
    }
}
