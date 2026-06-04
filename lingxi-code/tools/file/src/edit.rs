//! `FileEditTool` — literal-search-and-replace inside a UTF-8 file.
//!
//! Wire-locked semantics (no regex by default — claude-code parity):
//! - `replace_all: false` (default): EXACTLY ONE match of `old_string` must
//!   appear in the file; otherwise reject.
//! - `replace_all: true`: replace every occurrence via `String::replace`.
//! - Patch preview truncation suffix template: `"\n\n... [{N} lines truncated] ..."`
//!   (spec §7; `{N}` is the literal placeholder substituted via `replace`).

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
use telemetry::tengu::tool::{EDIT_COMPLETED, EDIT_FAILED, EDIT_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Edit";

/// Patch-preview truncation template — spec §7. `{N}` is a literal that the
/// emitter substitutes with the elided-line count via `String::replace`.
pub const PATCH_TRUNCATION_SUFFIX_TEMPLATE: &str = "\n\n... [{N} lines truncated] ...";

/// Maximum number of patch-preview lines retained before the truncation
/// suffix is appended. Chosen so a typical diff fits.
pub const PATCH_PREVIEW_LINE_LIMIT: usize = 30;

/// Build the byte-locked patch-truncation suffix with `n` substituted.
#[must_use]
pub fn patch_truncation_suffix(n: usize) -> String {
    PATCH_TRUNCATION_SUFFIX_TEMPLATE.replace("{N}", &n.to_string())
}

/// Build the model-facing `tool_result` message for an Edit, byte-faithful to
/// claude-code `FileEditTool.mapToolResultToToolResultBlockParam`
/// (`FileEditTool.ts:575-594`).
///
/// `path` is the ORIGINAL `file_path` input string (claude-code echoes the
/// caller's path verbatim, not a canonicalized form).
///
/// The interactive-only `userModified` variant — which inserts
/// `".  The user modified your proposed changes before accepting them. "` —
/// is out of scope for the non-interactive orchestrator (there is no
/// human-in-the-loop accept step), so `modifiedNote` is always empty here.
#[must_use]
pub fn edit_result_message(path: &str, replace_all: bool) -> String {
    if replace_all {
        format!("The file {path} has been updated. All occurrences were successfully replaced.")
    } else {
        format!("The file {path} has been updated successfully.")
    }
}

/// `FileEditTool` — literal-search replacement in a UTF-8 file.
pub struct FileEditTool {
    ctx: BuiltinToolContext,
}

impl FileEditTool {
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
        self.ctx.bus.log_event(EDIT_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, replacements: u32, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".to_string(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "replacements".to_string(),
            AnalyticsValue::Int(replacements as i64),
        );
        md.insert(
            "duration_ms".to_string(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(EDIT_COMPLETED, md).await;
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
        self.ctx.bus.log_event(EDIT_FAILED, md).await;
    }

    /// Build a `+`/`-`/` ` diff preview, capped at `PATCH_PREVIEW_LINE_LIMIT`
    /// lines. If truncation occurs, appends the byte-locked suffix.
    pub(crate) fn build_patch_preview(before: &str, after: &str) -> String {
        let before_lines: Vec<&str> = before.lines().collect();
        let after_lines: Vec<&str> = after.lines().collect();
        let mut diff: Vec<String> = Vec::new();
        let max = before_lines.len().max(after_lines.len());
        for i in 0..max {
            match (before_lines.get(i), after_lines.get(i)) {
                (Some(b), Some(a)) if b == a => diff.push(format!(" {b}")),
                (Some(b), Some(a)) => {
                    diff.push(format!("-{b}"));
                    diff.push(format!("+{a}"));
                }
                (Some(b), None) => diff.push(format!("-{b}")),
                (None, Some(a)) => diff.push(format!("+{a}")),
                (None, None) => {}
            }
        }
        if diff.len() <= PATCH_PREVIEW_LINE_LIMIT {
            return diff.join("\n");
        }
        let kept: Vec<String> = diff
            .iter()
            .take(PATCH_PREVIEW_LINE_LIMIT)
            .cloned()
            .collect();
        let elided = diff.len() - PATCH_PREVIEW_LINE_LIMIT;
        let mut out = kept.join("\n");
        out.push_str(&patch_truncation_suffix(elided));
        out
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["file_path", "old_string", "new_string"],
        "properties": {
            "file_path":   { "type": "string" },
            "old_string":  { "type": "string" },
            "new_string":  { "type": "string" },
            "replace_all": { "type": "boolean", "default": false }
        }
    })
});

#[async_trait]
impl Tool for FileEditTool {
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
        "Replace `old_string` with `new_string` in a file (literal, not regex).".to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        "Edit a file: literal find+replace. Default expects exactly one match.".to_string()
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
        let invocation_id = tool_api::util::ids::ulid_or_uuid();
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;
        let old_string = input
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("old_string is required".into()))?;
        let new_string = input
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("new_string is required".into()))?;
        let replace_all = input
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // No-op edit: identical strings (claude-code validateInput, errorCode 1).
        if old_string == new_string {
            self.emit_failed(&invocation_id, "no_change").await;
            return Err(ToolError::InvalidInput(
                "No changes to make: old_string and new_string are exactly the same.".into(),
            ));
        }

        let path = PathBuf::from(file_path);
        let started = Instant::now();
        self.emit_started(&invocation_id, &path).await;

        // `canonicalize_and_validate` tolerates a nonexistent target (it
        // canonicalizes the parent) so an empty `old_string` can create a file.
        let canon = match canonicalize_and_validate(&path, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &path).await;
                self.emit_failed(&invocation_id, "path_blocked").await;
                return Err(ToolError::PathBlocked { path });
            }
        };

        // Distinguish "does not exist" from a real read error so an empty
        // `old_string` can mean new-file creation (claude-code FileEditTool).
        //
        // Read the raw bytes and detect encoding (UTF-16LE via BOM, else UTF-8)
        // and line endings, then present an LF-normalized in-memory `content`
        // for matching — byte-faithful to claude-code `readFileForEdit` →
        // `readFileSyncWithMetadata` (FileEditTool.ts:202-221, 444-449). The
        // detected encoding/line-ending are fed back into `write_with_metadata`
        // so a CRLF or UTF-16LE file round-trips without corruption and an
        // `old_string` spanning a line break matches against the LF view.
        let existing: Option<(String, crate::file_meta::Encoding, crate::file_meta::LineEnding)> =
            match tokio::fs::read(&canon).await {
                Ok(bytes) => Some(crate::file_meta::read_with_metadata(&bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    self.emit_failed(&invocation_id, "io_read").await;
                    return Err(ToolError::Io(e.to_string()));
                }
            };

        // New files are created as UTF-8/LF (TS `readFileForEdit` ENOENT
        // branch returns encoding `utf8`, lineEndings `LF`).
        let (existing, enc, ending) = match existing {
            None => (None, crate::file_meta::Encoding::Utf8, crate::file_meta::LineEnding::Lf),
            Some((content, enc, ending)) => (Some(content), enc, ending),
        };

        let (before, after, replacements): (String, String, u32) = match existing {
            // File does not exist.
            None => {
                if !old_string.is_empty() {
                    self.emit_failed(&invocation_id, "file_not_found").await;
                    return Err(ToolError::InvalidInput(format!(
                        "File does not exist: {}",
                        canon.display()
                    )));
                }
                // Empty `old_string` on a nonexistent file → create it.
                (String::new(), new_string.to_string(), 1)
            }
            // File exists.
            Some(before) => {
                if old_string.is_empty() {
                    // Empty `old_string` is only valid for an (effectively) empty
                    // file — otherwise it's a creation attempt on existing content.
                    if !before.trim().is_empty() {
                        self.emit_failed(&invocation_id, "file_exists").await;
                        return Err(ToolError::InvalidInput(
                            "Cannot create new file - file already exists.".into(),
                        ));
                    }
                    (before, new_string.to_string(), 1)
                } else {
                    // Edit must not corrupt notebooks — route to NotebookEdit.
                    if std::path::Path::new(file_path)
                        .extension()
                        .is_some_and(|e| e == "ipynb")
                    {
                        self.emit_failed(&invocation_id, "ipynb").await;
                        return Err(ToolError::InvalidInput(
                            "File is a Jupyter Notebook. Use the NotebookEdit to edit this file."
                                .into(),
                        ));
                    }
                    // Curly-quote normalization (Batch E): when the file uses
                    // typographic (curly) quotes but the model sent straight
                    // quotes, recover the actual curly text from the file so the
                    // match still locates its target, then re-apply the file's
                    // curly style to `new_string` so the rewrite preserves the
                    // typography. Match/count on `actual_old` and `actual_new`
                    // (claude-code FileEditTool.ts:316,471-479). `before` is the
                    // LF-normalized in-memory view from Batch D, so all curly
                    // matching happens against that normalized content.
                    let actual_old = crate::quotes::find_actual_string(&before, old_string)
                        .unwrap_or_else(|| old_string.to_string());
                    let actual_new =
                        crate::quotes::preserve_quote_style(old_string, &actual_old, new_string);
                    let count = before.matches(actual_old.as_str()).count();
                    if count == 0 {
                        self.emit_failed(&invocation_id, "no_match").await;
                        return Err(ToolError::InvalidInput(format!(
                            "old_string not found in {}",
                            canon.display()
                        )));
                    }
                    if !replace_all && count > 1 {
                        self.emit_failed(&invocation_id, "ambiguous_match").await;
                        return Err(ToolError::InvalidInput(format!(
                            "old_string matched {count} times; pass replace_all=true or expand old_string"
                        )));
                    }
                    let after = if replace_all {
                        before.replace(actual_old.as_str(), &actual_new)
                    } else {
                        before.replacen(actual_old.as_str(), &actual_new, 1)
                    };
                    let replacements = if replace_all { count as u32 } else { 1 };
                    (before, after, replacements)
                }
            }
        };

        // Re-apply the original encoding + line endings on write so a CRLF or
        // UTF-16LE file round-trips byte-for-byte (claude-code
        // `writeTextContent`, file.ts:84-98). `after` is LF-normalized.
        let bytes = crate::file_meta::encode_with_metadata(&after, enc, ending);
        if let Err(e) = tokio::fs::write(&canon, bytes).await {
            self.emit_failed(&invocation_id, "io_write").await;
            return Err(ToolError::Io(e.to_string()));
        }

        let patch_preview = Self::build_patch_preview(&before, &after);
        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, replacements, duration_ms)
            .await;

        // Model-facing result string is byte-faithful to claude-code
        // (`FileEditTool.ts:575-594`); it echoes the ORIGINAL `file_path` arg,
        // not the canonicalized path. Batch A's serialization rule emits
        // `data["content"]` verbatim to the model; `replacements` /
        // `patch_preview` remain for the TUI diff render only.
        let content = edit_result_message(file_path, replace_all);

        Ok(ToolCallResult {
            data: json!({
                "content": content,
                "replacements": replacements,
                "patch_preview": patch_preview
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
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tempfile::TempDir;
    use tool_api::test_support::{fresh_ctx, fresh_tx, make_dummy_fs};

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

    #[test]
    fn tool_name_is_edit() {
        assert_eq!(TOOL_NAME, "Edit");
    }

    #[test]
    fn patch_truncation_template_byte_locked() {
        assert_eq!(
            PATCH_TRUNCATION_SUFFIX_TEMPLATE,
            "\n\n... [{N} lines truncated] ..."
        );
    }

    #[test]
    fn patch_truncation_suffix_substitutes_n() {
        assert_eq!(
            patch_truncation_suffix(42),
            "\n\n... [42 lines truncated] ..."
        );
    }

    #[test]
    fn edit_result_message_single_is_byte_locked() {
        // FileEditTool.ts:589-593 (non-interactive, modifiedNote empty).
        assert_eq!(
            edit_result_message("/tmp/a.txt", false),
            "The file /tmp/a.txt has been updated successfully."
        );
    }

    #[test]
    fn edit_result_message_replace_all_is_byte_locked() {
        // FileEditTool.ts:581-586 (non-interactive, modifiedNote empty).
        assert_eq!(
            edit_result_message("/tmp/a.txt", true),
            "The file /tmp/a.txt has been updated. All occurrences were successfully replaced."
        );
    }

    #[test]
    fn edit_result_message_echoes_original_path_verbatim() {
        // claude-code echoes the input path, not a canonicalized form.
        assert_eq!(
            edit_result_message("./relative/../weird/path.txt", false),
            "The file ./relative/../weird/path.txt has been updated successfully."
        );
    }

    #[tokio::test]
    async fn single_replacement_succeeds() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello world").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = FileEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "world",
                    "new_string": "Rust"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 1);
        // Model-facing `content` is the byte-faithful single-edit message and
        // echoes the ORIGINAL input path verbatim (not canonicalized).
        let input_path = target.to_str().unwrap();
        assert_eq!(
            result.data["content"].as_str().unwrap(),
            format!("The file {input_path} has been updated successfully.")
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello Rust");
    }

    #[tokio::test]
    async fn empty_old_string_creates_new_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("created.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "",
                    "new_string": "brand new contents\n"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "brand new contents\n"
        );
    }

    #[tokio::test]
    async fn empty_old_string_on_existing_content_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("exists.txt");
        std::fs::write(&target, "already here").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "",
                    "new_string": "x"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Cannot create new file - file already exists."));
        // original content untouched
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "already here");
    }

    #[tokio::test]
    async fn empty_old_string_replaces_empty_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("blank.txt");
        std::fs::write(&target, "   \n").unwrap(); // whitespace-only ⇒ effectively empty
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "",
                "new_string": "seeded"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "seeded");
    }

    #[tokio::test]
    async fn rejects_editing_notebook() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, "{\"cells\": []}").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "cells",
                    "new_string": "x"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Jupyter Notebook"));
        assert!(err.to_string().contains("NotebookEdit"));
    }

    #[tokio::test]
    async fn rejects_identical_old_and_new() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "hello",
                    "new_string": "hello"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No changes to make"));
    }

    #[tokio::test]
    async fn nonexistent_file_with_nonempty_old_string_errors() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("missing.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "x",
                    "new_string": "y"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("File does not exist"));
    }

    #[tokio::test]
    async fn rejects_ambiguous_without_replace_all() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "foo foo foo").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "bar"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("matched 3 times"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "foo foo foo");
    }

    #[tokio::test]
    async fn replace_all_handles_multiple_matches() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "foo foo foo").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "bar",
                    "replace_all": true
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 3);
        // replace_all path emits the "All occurrences were successfully
        // replaced." message verbatim to the model.
        let input_path = target.to_str().unwrap();
        assert_eq!(
            result.data["content"].as_str().unwrap(),
            format!(
                "The file {input_path} has been updated. All occurrences were successfully replaced."
            )
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "bar bar bar");
    }

    #[tokio::test]
    async fn rejects_no_match() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "absent",
                    "new_string": "x"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn patch_preview_truncates_with_suffix() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big.txt");
        let big: String = (0..100).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&target, &big).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "L",
                    "new_string": "M",
                    "replace_all": true
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let preview = result.data["patch_preview"].as_str().unwrap();
        assert!(
            preview.contains("lines truncated] ..."),
            "preview missing truncation suffix: {preview}"
        );
        assert!(
            preview.contains("\n\n... ["),
            "preview missing suffix prefix: {preview}"
        );
    }

    #[tokio::test]
    async fn edits_crlf_file_preserving_endings() {
        // old_string spans a line break — only matchable against the
        // LF-normalized in-memory view; the rewrite must re-apply CRLF.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("crlf.txt");
        std::fs::write(&target, b"line one\r\nline two\r\nline three\r\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                // Matches across the CRLF that was normalized to LF.
                "old_string": "line one\nline two",
                "new_string": "first\nsecond"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // ASSERT BYTES: CRLF preserved on disk, including the rewritten region.
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"first\r\nsecond\r\nline three\r\n"
        );
    }

    #[tokio::test]
    async fn edits_utf16le_file_preserving_bom_and_encoding() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("u16.txt");
        // "hello world" in UTF-16LE with BOM.
        let mut bytes = vec![0xFF, 0xFE];
        for u in "hello world".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::write(&target, &bytes).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "world",
                "new_string": "Rust"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // ASSERT BYTES: BOM preserved, re-encoded as UTF-16LE.
        let mut expected = vec![0xFF, 0xFE];
        for u in "hello Rust".encode_utf16() {
            expected.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(std::fs::read(&target).unwrap(), expected);
    }

    #[tokio::test]
    async fn mixed_endings_pick_dominant_crlf() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("mixed.txt");
        // 3 CRLF vs 1 bare LF ⇒ CRLF dominates (TS crlf > lf).
        std::fs::write(&target, b"a\r\nb\r\nc\r\nd\nEDITME").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "EDITME",
                "new_string": "done"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // ASSERT BYTES: the lone LF after `c\r\n` is rewritten to the dominant
        // CRLF (matches TS writeTextContent collapsing then re-applying CRLF).
        assert_eq!(std::fs::read(&target).unwrap(), b"a\r\nb\r\nc\r\nd\r\ndone");
    }

    #[tokio::test]
    async fn curly_double_in_file_matches_straight_and_preserves_curly() {
        // File has curly "hello"; model sends straight "hello". Edit must match
        // and rewrite preserving the file's curly typography (Batch E).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("curly.txt");
        std::fs::write(&target, "say \u{201C}hello\u{201D} now").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "\"hello\"",
                "new_string": "\"world\""
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // new_string straight doubles ⇒ curly applied by open/close context.
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "say \u{201C}world\u{201D} now"
        );
    }

    #[tokio::test]
    async fn curly_single_contraction_keeps_right_single() {
        // File uses curly singles; new_string contains a contraction `don't`.
        // The apostrophe (letter on both sides) must become a right single
        // curly, NOT an opening quote.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("contraction.txt");
        std::fs::write(&target, "a \u{2018}b\u{2019} c").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "'b'",
                "new_string": "don't"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "a don\u{2019}t c"
        );
    }

    #[tokio::test]
    async fn curly_single_open_vs_close_positions() {
        // Leading quote (start of replacement) ⇒ opening; trailing quote (after
        // a letter, end) ⇒ closing.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("quotes.txt");
        std::fs::write(&target, "x \u{2018}q\u{2019} y").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "'q'",
                "new_string": "'word'"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "x \u{2018}word\u{2019} y"
        );
    }

    #[tokio::test]
    async fn no_curly_in_file_leaves_new_string_untouched() {
        // Plain ASCII file ⇒ exact match ⇒ no quote normalization ⇒ new_string
        // straight quotes stay straight.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("plain.txt");
        std::fs::write(&target, "say \"hello\" now").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "\"hello\"",
                "new_string": "\"world\""
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // Straight quotes preserved verbatim (no curly applied).
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "say \"world\" now"
        );
    }

    #[tokio::test]
    async fn lf_file_round_trips_as_lf() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("lf.txt");
        std::fs::write(&target, b"alpha\nbeta\ngamma\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "beta",
                "new_string": "BETA"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // ASSERT BYTES: still pure LF (no CR introduced).
        assert_eq!(std::fs::read(&target).unwrap(), b"alpha\nBETA\ngamma\n");
    }
}
