//! `FileWriteTool` — write a UTF-8 file inside the trusted-dirs whitelist.
//!
//! Behaviour:
//! - If parent directory does not exist AND `mkdir != true` → reject.
//! - If parent directory does not exist AND `mkdir == true` → create parents
//!   recursively, then write.
//! - Overwrite is allowed unconditionally (claude-code matches this).
//! - Content is UTF-8; no BOM is written.

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
use lingxi_telemetry::tengu::tool::{WRITE_COMPLETED, WRITE_FAILED, WRITE_STARTED};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Write";

/// `FileWriteTool` — writes a UTF-8 file inside the trusted-dirs whitelist.
pub struct FileWriteTool {
    ctx: BuiltinToolContext,
}

impl FileWriteTool {
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
            "_PROTO_file_path".to_string(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(path.display().to_string()).into_inner(),
            ),
        );
        self.ctx.bus.log_event(WRITE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, bytes_written: u64, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "bytes_written".to_string(),
            AnalyticsValue::Int(bytes_written as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(WRITE_COMPLETED, md).await;
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
        self.ctx.bus.log_event(WRITE_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file_path", "content"],
        "properties": {
            "file_path": { "type": "string" },
            "content":   { "type": "string" },
            "mkdir":     { "type": "boolean", "default": false }
        }
    })
});

#[async_trait]
impl Tool for FileWriteTool {
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
        "Write a UTF-8 file to the workspace.".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Write a file. Pass `mkdir: true` to create missing parents.".to_string()
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
        let invocation_id = crate::builtin::file_read::ulid_or_uuid();
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("content is required".into()))?;
        let mkdir = input.get("mkdir").and_then(Value::as_bool).unwrap_or(false);

        let path = PathBuf::from(file_path);
        let started = Instant::now();
        self.emit_started(&invocation_id, &path).await;

        // Parent existence gate (BEFORE canonicalize, because canonicalize
        // requires either the file or its parent to exist).
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if !mkdir {
                    self.emit_failed(&invocation_id, "missing_parent").await;
                    return Err(ToolError::InvalidInput(format!(
                        "parent directory {} does not exist; pass mkdir=true to create",
                        parent.display()
                    )));
                }
                // We don't yet know if the parent is *inside* trusted_dirs,
                // so canonicalize the nearest existing ancestor first and
                // gate on that.
                let mut probe = parent.to_path_buf();
                while !probe.exists() {
                    match probe.parent() {
                        Some(p) => probe = p.to_path_buf(),
                        None => break,
                    }
                }
                if canonicalize_and_validate(&probe, &self.ctx.trusted_dirs).is_err() {
                    emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                    self.emit_failed(&invocation_id, "path_blocked").await;
                    return Err(ToolError::PathBlocked { path });
                }
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    self.emit_failed(&invocation_id, "mkdir_failed").await;
                    return Err(ToolError::Io(e.to_string()));
                }
            }
        }

        let canon = match canonicalize_and_validate(&path, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path });
            }
        };

        if let Err(e) = tokio::fs::write(&canon, content.as_bytes()).await {
            self.emit_failed(&invocation_id, "io_write").await;
            return Err(ToolError::Io(e.to_string()));
        }

        let bytes_written = content.as_bytes().len() as u64;
        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, bytes_written, duration_ms)
            .await;

        Ok(ToolCallResult {
            data: json!({ "bytes_written": bytes_written }),
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

    fn make_ctx(tmp: &TempDir) -> (BuiltinToolContext, Arc<InMemorySink>) {
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

    #[test]
    fn tool_name_is_write() {
        assert_eq!(TOOL_NAME, "Write");
    }

    #[tokio::test]
    async fn happy_path_writes_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("out.txt");
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileWriteTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "content": "hello" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["bytes_written"], 5);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"tengu_tool_write_started".to_string()));
        assert!(names.contains(&"tengu_tool_write_completed".to_string()));
    }

    #[tokio::test]
    async fn rejects_missing_parent_without_mkdir() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("sub").join("deep").join("out.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "content": "hi" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist") && msg.contains("mkdir=true"),
            "got: {msg}"
        );
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn creates_parent_with_mkdir() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("sub").join("deep").join("out.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "content": "deep content",
                    "mkdir": true
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["bytes_written"], 12);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "deep content");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("over.txt");
        std::fs::write(&target, "old").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        let _ = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "content": "new" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }

    #[tokio::test]
    async fn rejects_path_outside_trusted() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("a.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        let err = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "content": "x" }),
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
    async fn is_destructive_returns_true() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        assert!(tool.is_destructive(&json!({})));
    }

    #[tokio::test]
    async fn is_concurrency_safe_returns_false() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        assert!(!tool.is_concurrency_safe(&json!({})));
    }

    #[tokio::test]
    async fn is_read_only_returns_false() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        assert!(!tool.is_read_only(&json!({})));
    }

    #[tokio::test]
    async fn input_schema_requires_file_path_and_content() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileWriteTool::new(ctx);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"file_path"));
        assert!(names.contains(&"content"));
    }
}
