//! `GlobTool` — expand a `**/*.rs`-style pattern against a base directory.
//!
//! Wire locks (spec §7):
//! - `MAX_GLOB_MATCHES = 100`
//! - Excess truncated → returns `truncated: true` field on output.
//! - Results sorted newest-first by mtime (claude-code parity).

use async_trait::async_trait;
use globset::Glob;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{GLOB_COMPLETED, GLOB_FAILED, GLOB_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;
use walkdir::WalkDir;

use crate::grep::to_relative_path;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Glob";

/// Maximum match count returned. Spec §7 lock.
pub const MAX_GLOB_MATCHES: usize = 100;

/// Advisory appended to the model-facing result when matches were capped at
/// `MAX_GLOB_MATCHES` (`GlobTool.ts:190-194`, byte-exact).
const TRUNCATION_ADVISORY: &str =
    "(Results are truncated. Consider using a more specific path or pattern.)";

/// Model-facing string when no files matched (`GlobTool.ts:178-183`, byte-exact).
const NO_FILES_FOUND: &str = "No files found";

/// `extractGlobBaseDirectory` (`utils/glob.ts:17-64`): peel the static base
/// directory (everything before the first glob metachar `* ? [ {`) off a
/// pattern, returning `(base_dir, relative_pattern)`. Used to re-root absolute
/// patterns — `Glob::new` matches against paths stripped of the canonical base,
/// so an absolute pattern can never match unless its static prefix becomes the
/// search root and the remainder is compiled instead (`utils/glob.ts:78-84`).
///
/// Returns an empty `base_dir` when there is no static directory prefix to peel
/// off (the caller then keeps the original base + pattern).
fn extract_glob_base_directory(pattern: &str) -> (String, String) {
    // First glob special character: * ? [ {
    let Some(idx) = pattern.find(['*', '?', '[', '{']) else {
        // No glob characters — literal path: dirname / basename split.
        let p = Path::new(pattern);
        let dir = p
            .parent()
            .map_or_else(String::new, |d| d.to_string_lossy().into_owned());
        let file = p
            .file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        return (dir, file);
    };

    // Everything before the first glob char; find the last separator within it.
    let static_prefix = &pattern[..idx];
    match static_prefix.rfind('/') {
        // No separator before the glob — pattern is relative to the base.
        None => (String::new(), pattern.to_string()),
        // Root-directory pattern (e.g. `/*.txt`): base dir is `/`.
        Some(0) => ("/".to_string(), pattern[1..].to_string()),
        Some(sep) => (pattern[..sep].to_string(), pattern[sep + 1..].to_string()),
    }
}

/// `GlobTool` — pattern walker.
pub struct GlobTool {
    ctx: BuiltinToolContext,
}

impl GlobTool {
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
        self.ctx.bus.log_event(GLOB_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, matches: u64, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("matches".to_string(), AnalyticsValue::Int(matches as i64));
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(GLOB_COMPLETED, md).await;
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
        self.ctx.bus.log_event(GLOB_FAILED, md).await;
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["pattern"],
        "properties": {
            "pattern": { "type": "string" },
            "path":    { "type": "string" }
        }
    })
});

#[async_trait]
impl Tool for GlobTool {
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
        "Expand a glob pattern; returns up to 100 newest matches.".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Glob a pattern (e.g. **/*.rs). Sorted by mtime desc, capped at 100.".to_string()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = tool_api::util::ids::ulid_or_uuid();
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
                ToolError::InvalidInput("no path supplied and no trusted_dirs configured".into())
            })?;

        let started = Instant::now();
        self.emit_started(&invocation_id, pattern).await;

        // GLOB.2: an absolute pattern (e.g. `/abs/proj/**/*.rs`) can never match,
        // because the matcher is tested against paths stripped of `canon_base`.
        // Mirror `glob.ts:78-84`: split the static base dir out and re-root the
        // search there, compiling from the relative remainder.
        let (base, pattern): (PathBuf, String) = if Path::new(pattern).is_absolute() {
            let (base_dir, relative_pattern) = extract_glob_base_directory(pattern);
            if base_dir.is_empty() {
                (base, pattern.to_string())
            } else {
                (PathBuf::from(base_dir), relative_pattern)
            }
        } else {
            (base, pattern.to_string())
        };
        let pattern = pattern.as_str();

        let canon_base = match canonicalize_and_validate(&base, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &base).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path: base });
            }
        };

        let glob = match Glob::new(pattern) {
            Ok(g) => g,
            Err(e) => {
                self.emit_failed(&invocation_id, "bad_pattern").await;
                return Err(ToolError::InvalidInput(format!(
                    "invalid glob pattern {pattern:?}: {e}"
                )));
            }
        };
        let matcher = glob.compile_matcher();

        let mut hits: Vec<(PathBuf, SystemTime)> = Vec::new();
        for entry in WalkDir::new(&canon_base).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&canon_base)
                .unwrap_or(entry.path());
            if matcher.is_match(rel) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                hits.push((entry.path().to_path_buf(), mtime));
            }
        }

        let total = hits.len();
        hits.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
        let truncated = total > MAX_GLOB_MATCHES;
        if truncated {
            hits.truncate(MAX_GLOB_MATCHES);
        }

        // Relativize each hit against the canonicalized workspace (TS
        // `files.map(toRelativePath)`, GlobTool.ts:166). The walk yields
        // canonicalized paths, so the cwd must be canonicalized too for
        // `strip_prefix` to match — same rule GrepTool uses.
        let cwd_for_rel = std::fs::canonicalize(&self.ctx.workspace)
            .unwrap_or_else(|_| self.ctx.workspace.clone());
        let matches: Vec<String> = hits
            .iter()
            .map(|(p, _)| to_relative_path(p, &cwd_for_rel))
            .collect();

        // Model-facing string (`mapToolResultToToolResultBlockParam`,
        // GlobTool.ts:177-197): "No files found" when empty, else the joined
        // paths plus the truncation advisory when capped.
        let content = if matches.is_empty() {
            NO_FILES_FOUND.to_string()
        } else if truncated {
            format!("{}\n{TRUNCATION_ADVISORY}", matches.join("\n"))
        } else {
            matches.join("\n")
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, matches.len() as u64, duration_ms)
            .await;

        Ok(ToolCallResult {
            // `content` is what the model sees (turn_loop `tool_result_to_model_text`
            // surfaces `data.content` verbatim); `matches`/`truncated` stay for the
            // TUI + existing tests.
            data: json!({ "content": content, "matches": matches, "truncated": truncated }),
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

    #[test]
    fn tool_name_is_glob() {
        assert_eq!(TOOL_NAME, "Glob");
    }

    #[test]
    fn max_matches_byte_locked() {
        assert_eq!(MAX_GLOB_MATCHES, 100);
    }

    #[tokio::test]
    async fn matches_rs_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(result.data["truncated"], false);
    }

    #[tokio::test]
    async fn caps_at_100_with_truncated_flag() {
        let tmp = TempDir::new().unwrap();
        for i in 0..150 {
            std::fs::write(tmp.path().join(format!("f{i}.rs")), "x").unwrap();
        }
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), MAX_GLOB_MATCHES);
        assert_eq!(result.data["truncated"], true);
    }

    #[tokio::test]
    async fn results_sorted_newest_first() {
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let old = tmp.path().join("old.rs");
        let mid = tmp.path().join("mid.rs");
        let new = tmp.path().join("new.rs");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&mid, "x").unwrap();
        std::fs::write(&new, "x").unwrap();
        set_file_mtime(&old, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        set_file_mtime(&mid, FileTime::from_unix_time(1_500_000_000, 0)).unwrap();
        set_file_mtime(&new, FileTime::from_unix_time(1_700_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(matches[0].ends_with("new.rs"));
        assert!(matches[1].ends_with("mid.rs"));
        assert!(matches[2].ends_with("old.rs"));
    }

    #[tokio::test]
    async fn rejects_invalid_pattern() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let err = tool
            .call(json!({ "pattern": "[invalid" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid glob pattern"));
    }

    // --- GLOB.1: model-facing `content` + cwd-relative paths ---

    #[tokio::test]
    async fn content_and_matches_are_cwd_relative() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        // `matches` is relativized (not the canonical absolute path).
        let matches: Vec<&str> = result.data["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["a.rs"]);
        // `content` is the model-facing string: joined relative paths.
        assert_eq!(result.data["content"].as_str().unwrap(), "a.rs");
    }

    #[tokio::test]
    async fn content_no_files_found_when_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        assert_eq!(result.data["content"].as_str().unwrap(), "No files found");
        assert_eq!(result.data["matches"].as_array().unwrap().len(), 0);
        assert_eq!(result.data["truncated"], false);
    }

    #[tokio::test]
    async fn content_appends_truncation_advisory_when_capped() {
        let tmp = TempDir::new().unwrap();
        for i in 0..150 {
            std::fs::write(tmp.path().join(format!("f{i}.rs")), "x").unwrap();
        }
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let content = result.data["content"].as_str().unwrap();
        assert!(
            content.ends_with(
                "\n(Results are truncated. Consider using a more specific path or pattern.)"
            ),
            "content should end with the truncation advisory: {content}"
        );
        assert_eq!(result.data["truncated"], true);
    }

    // --- GLOB.2: absolute patterns re-rooted via extractGlobBaseDirectory ---

    #[test]
    fn extract_glob_base_directory_splits_static_prefix() {
        assert_eq!(
            extract_glob_base_directory("/abs/proj/**/*.rs"),
            ("/abs/proj".to_string(), "**/*.rs".to_string())
        );
        assert_eq!(
            extract_glob_base_directory("/abs/*.rs"),
            ("/abs".to_string(), "*.rs".to_string())
        );
        // Root-directory pattern → base dir is `/`.
        assert_eq!(
            extract_glob_base_directory("/*.rs"),
            ("/".to_string(), "*.rs".to_string())
        );
        // No separator before the glob → nothing to peel off.
        assert_eq!(
            extract_glob_base_directory("*.rs"),
            (String::new(), "*.rs".to_string())
        );
        // Literal path → dirname / basename.
        assert_eq!(
            extract_glob_base_directory("/abs/proj/file.rs"),
            ("/abs/proj".to_string(), "file.rs".to_string())
        );
    }

    #[tokio::test]
    async fn absolute_pattern_matches() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        // An absolute pattern would yield zero matches before GLOB.2 (the matcher
        // was tested against base-stripped paths). Re-rooting fixes it.
        let pattern = format!("{}/*.rs", tmp.path().display());
        let result = tool
            .call(json!({ "pattern": pattern }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches = result.data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2, "absolute pattern should match: {matches:?}");
        assert_eq!(result.data["truncated"], false);
    }
}
