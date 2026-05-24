//! `GrepTool` — content search via ripgrep crates (`grep_regex`/`grep_searcher`).
//!
//! Wire lock (spec §7): 100 matches per file cap.
//!
//! Output shape varies by `output_mode`:
//! - `"content"` (default): `matches: [{ path, line, text }]`.
//! - `"files_with_matches"`: `matches: [{ path }]` (no line/text).
//! - `"count"`: `matches: [{ path, line: count }]` — `line` reused as the
//!   per-file match count.

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::shared::path_validation::{canonicalize_and_validate, emit_blocked_event};
use crate::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use async_trait::async_trait;
use globset::Glob;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks::UTF8, SearcherBuilder};
use ignore::WalkBuilder;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::{PiiTagged, Verified};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{GREP_COMPLETED, GREP_FAILED, GREP_STARTED};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Grep";

/// Maximum matches per file. Spec §7 lock.
pub const GREP_PER_FILE_CAP: usize = 100;

/// `GrepTool` — content search.
pub struct GrepTool {
    ctx: BuiltinToolContext,
}

impl GrepTool {
    /// Construct a new tool.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str, pattern: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "_PROTO_pattern".to_string(),
            AnalyticsValue::String(
                PiiTagged::assert_pii_tagged_column(pattern.to_string()).into_inner(),
            ),
        );
        self.ctx.bus.log_event(GREP_STARTED, md).await;
    }

    async fn emit_completed(
        &self,
        invocation_id: &str,
        matches: u64,
        files_scanned: u64,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("matches".to_string(), AnalyticsValue::Int(matches as i64));
        md.insert(
            "files_scanned".to_string(),
            AnalyticsValue::Int(files_scanned as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(GREP_COMPLETED, md).await;
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
        self.ctx.bus.log_event(GREP_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["pattern"],
        "properties": {
            "pattern":          { "type": "string" },
            "path":             { "type": "string" },
            "glob":             { "type": "string" },
            "output_mode":      {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "default": "content"
            },
            "case_insensitive": { "type": "boolean", "default": false },
            "multiline":        { "type": "boolean", "default": false }
        }
    })
});

#[async_trait]
impl Tool for GrepTool {
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
        "Search file contents via regex (ripgrep semantics).".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Grep a regex across files. 100-matches-per-file cap.".to_string()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = crate::builtin::file_read::ulid_or_uuid();
        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("pattern is required".into()))?;
        let base = input
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| self.ctx.trusted_dirs.first().cloned())
            .ok_or_else(|| {
                ToolError::InvalidInput("no path and no trusted_dirs configured".into())
            })?;
        let glob_filter = input.get("glob").and_then(Value::as_str);
        let output_mode = input
            .get("output_mode")
            .and_then(Value::as_str)
            .unwrap_or("content")
            .to_string();
        let case_insensitive = input
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let multiline = input
            .get("multiline")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let started = Instant::now();
        self.emit_started(&invocation_id, pattern).await;

        let canon_base = match canonicalize_and_validate(&base, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &base).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path: base });
            }
        };

        let matcher = match RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .multi_line(multiline)
            .build(pattern)
        {
            Ok(m) => m,
            Err(e) => {
                self.emit_failed(&invocation_id, "bad_regex").await;
                return Err(ToolError::InvalidInput(format!(
                    "invalid regex {pattern:?}: {e}"
                )));
            }
        };

        let glob_matcher = match glob_filter {
            Some(g) => match Glob::new(g) {
                Ok(gl) => Some(gl.compile_matcher()),
                Err(e) => {
                    self.emit_failed(&invocation_id, "bad_glob").await;
                    return Err(ToolError::InvalidInput(format!("invalid glob {g:?}: {e}")));
                }
            },
            None => None,
        };

        let mut matches: Vec<Value> = Vec::new();
        let mut total_matches: u64 = 0;
        let mut files_scanned: u64 = 0;
        let mut truncated = false;

        for entry in WalkBuilder::new(&canon_base).build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map_or(false, |t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            if let Some(ref g) = glob_matcher {
                let rel = path.strip_prefix(&canon_base).unwrap_or(path);
                if !g.is_match(rel) {
                    continue;
                }
            }
            files_scanned += 1;

            let mut file_hits: Vec<(u64, String)> = Vec::new();
            let mut overflow_for_file = false;
            let mut searcher = SearcherBuilder::new().multi_line(multiline).build();
            let _ = searcher.search_path(
                &matcher,
                path,
                UTF8(|lnum, line| {
                    if file_hits.len() >= GREP_PER_FILE_CAP {
                        overflow_for_file = true;
                        return Ok(false);
                    }
                    file_hits.push((lnum, line.trim_end_matches('\n').to_string()));
                    Ok(true)
                }),
            );

            if overflow_for_file {
                truncated = true;
            }
            total_matches += file_hits.len() as u64;

            match output_mode.as_str() {
                "files_with_matches" => {
                    if !file_hits.is_empty() {
                        matches.push(json!({ "path": path.display().to_string() }));
                    }
                }
                "count" => {
                    if !file_hits.is_empty() {
                        matches.push(json!({
                            "path": path.display().to_string(),
                            "line": file_hits.len() as i64
                        }));
                    }
                }
                _ => {
                    for (lnum, text) in file_hits {
                        matches.push(json!({
                            "path": path.display().to_string(),
                            "line": lnum,
                            "text": text
                        }));
                    }
                }
            }
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, total_matches, files_scanned, duration_ms)
            .await;

        Ok(ToolCallResult {
            data: json!({
                "output_mode": output_mode,
                "matches": matches,
                "files_scanned": files_scanned,
                "total_matches": total_matches,
                "truncated": truncated
            }),
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

    #[test]
    fn tool_name_is_grep() {
        assert_eq!(TOOL_NAME, "Grep");
    }

    #[test]
    fn per_file_cap_byte_locked() {
        assert_eq!(GREP_PER_FILE_CAP, 100);
    }

    #[tokio::test]
    async fn finds_pattern_in_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "no match here").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": r"fn \w+" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        assert_eq!(result.data["total_matches"], 2);
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[tokio::test]
    async fn glob_filter_narrows_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "fn bar()").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "glob": "*.rs" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["total_matches"], 1);
    }

    #[tokio::test]
    async fn output_mode_files_with_matches() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn baz() {}").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "files_with_matches" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        for m in matches {
            assert!(m.get("line").is_none());
        }
    }

    #[tokio::test]
    async fn per_file_cap_sets_truncated_flag() {
        let tmp = TempDir::new().unwrap();
        let content: String = (0..150).map(|i| format!("fn f{i}() {{}}\n")).collect();
        std::fs::write(tmp.path().join("big.rs"), content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "fn" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        assert_eq!(result.data["truncated"], true);
        assert_eq!(result.data["total_matches"], GREP_PER_FILE_CAP as i64);
    }

    #[tokio::test]
    async fn rejects_invalid_regex() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let err = tool
            .call(json!({ "pattern": "(unclosed" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid regex"));
    }
}
