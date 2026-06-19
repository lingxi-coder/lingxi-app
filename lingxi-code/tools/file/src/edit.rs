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

/// Verbatim port of claude-code `FileEditTool/prompt.ts` `getEditToolDescription()`
/// (== the tool's DESCRIPTION). The TS builder has two runtime variations, both of
/// which resolve deterministically for this 3P port:
///   - `isCompactLinePrefixEnabled()` is `true` by default (killswitch off), and the
///     Read-tool port already canonicalized the compact format → `prefixFormat` is
///     `"line number + tab"` (the padded-arrow legacy form is intentionally not ported).
///   - `process.env.USER_TYPE === 'ant'` is the Anthropic-internal build; this 3P port
///     is never `ant` → `minimalUniquenessHint` is empty.
///   - `getPreReadInstruction()` interpolates `FILE_READ_TOOL_NAME = 'Read'`, and begins
///     with `\n-` (so `Usage:` is immediately followed by the first bullet) and ends with
///     a trailing space after "before editing." — both preserved byte-for-byte below.
const EDIT_DESCRIPTION: &str = r#"Performs exact string replacements in files.

Usage:
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file. 
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: line number + tab. Everything after that is the actual file content to match. Never include any part of the line number prefix in the old_string or new_string.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless asked.
- The edit will FAIL if `old_string` is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use `replace_all` to change every instance of `old_string`.
- Use `replace_all` for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance."#;

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
    let base = if replace_all {
        format!("The file {path} has been updated. All occurrences were successfully replaced.")
    } else {
        format!("The file {path} has been updated successfully.")
    };
    format!("{base}{}", crate::FILE_STATE_CURRENT_SUFFIX)
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
        EDIT_DESCRIPTION.to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        EDIT_DESCRIPTION.to_string()
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
        // Read the raw bytes once; also capture the *current* mtime (floored to
        // ms) and a raw-UTF-8 decode that matches how the `Read` tool stores
        // its content, so the read-before-write staleness guard (Batch F) can
        // compare like with like.
        #[allow(clippy::type_complexity)]
        let existing: Option<(
            String,
            crate::file_meta::Encoding,
            crate::file_meta::LineEnding,
            i64,
            String,
        )> = match tokio::fs::read(&canon).await {
            Ok(bytes) => {
                // Current mtime (floored ms) — `None`/error falls back to epoch
                // `0`, matching `read.rs`'s handling.
                let mtime_ms = tokio::fs::metadata(&canon)
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map_or(0, tool_api::read_file_state::mtime_ms_floor);
                // Raw (non-LF-normalized) UTF-8 decode — the exact form the
                // `Read` tool records into `read_file_state`. Used only for the
                // content-equality fallback; falls back to the LF view if the
                // file is not strict UTF-8 (e.g. UTF-16LE), in which case the
                // mtime check alone governs.
                let raw_decoded = crate::shared::decode_utf8_strict(&bytes).ok();
                let (content, enc, ending) = crate::file_meta::read_with_metadata(&bytes);
                let raw_for_cmp = raw_decoded.unwrap_or_else(|| content.clone());
                Some((content, enc, ending, mtime_ms, raw_for_cmp))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_read").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };

        // New files are created as UTF-8/LF (TS `readFileForEdit` ENOENT
        // branch returns encoding `utf8`, lineEndings `LF`). `existing` carries
        // the LF-normalized content plus the staleness-guard inputs (mtime +
        // raw-decoded current content) when the file already exists.
        let (existing, enc, ending, guard_mtime_ms, guard_raw_content) = match existing {
            None => (
                None,
                crate::file_meta::Encoding::Utf8,
                crate::file_meta::LineEnding::Lf,
                0i64,
                String::new(),
            ),
            Some((content, enc, ending, mtime_ms, raw_for_cmp)) => {
                (Some(content), enc, ending, mtime_ms, raw_for_cmp)
            }
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
                // Read-before-write staleness guard (Batch F). The file exists,
                // so a prior full `Read` is required and the file must not have
                // changed on disk since (TS `FileEditTool.ts:275-311` validate +
                // `:451-468` call-time re-check). New-file creation hits the
                // `None` arm above and skips this entirely (TS ENOENT →
                // `result:true`). `guard_raw_content` is the current on-disk
                // content decoded the same way `Read` records it, for the
                // full-read content-equality fallback.
                if let Err(e) = crate::check_read_before_write(
                    &self.ctx.read_file_state,
                    &canon,
                    guard_mtime_ms,
                    &guard_raw_content,
                ) {
                    self.emit_failed(&invocation_id, "stale_read").await;
                    return Err(e);
                }

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
                        // Byte-locked string-to-replace-not-found message
                        // (FileEditTool.ts:321). Echoes the ORIGINAL `old_string`
                        // input (TS `${old_string}`), not `actual_old`.
                        return Err(ToolError::InvalidInput(format!(
                            "String to replace not found in file.\nString: {old_string}"
                        )));
                    }
                    if !replace_all && count > 1 {
                        self.emit_failed(&invocation_id, "ambiguous_match").await;
                        // Byte-locked multiple-match message (FileEditTool.ts:336).
                        // `count` == TS `matches`; trailing `String:` echoes the
                        // ORIGINAL `old_string` input (TS `${old_string}`).
                        return Err(ToolError::InvalidInput(format!(
                            "Found {count} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: {old_string}"
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

        // Post-write: update the read-state registry so an immediate second
        // Edit/Write succeeds and Read-dedup sees the new mtime (TS
        // `FileEditTool.ts:519-525` `readFileState.set({content: updatedFile,
        // timestamp: <new mtime>, offset: undefined, limit: undefined})`).
        // `after` is the LF-normalized written content (TS stores the same
        // LF-normalized `updatedFile`); offset/limit cleared so the next read
        // counts as a full read.
        let new_mtime_ms = tokio::fs::metadata(&canon)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        tool_api::read_file_state::set(
            &self.ctx.read_file_state,
            canon.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: after.clone(),
                mtime_ms: new_mtime_ms,
                offset: None,
                limit: None,
                // Post-edit entry — not a Read; the dedup gate skips it.
                from_read: false,
            },
        );

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

    /// Simulate a prior full `Read` of `target` so the read-before-write
    /// staleness guard (Batch F) is satisfied: records the file's current
    /// raw-UTF-8 content (the same form `Read` stores) and its current floored
    /// mtime under the canonicalized key, with `offset`/`limit` = `None` (a
    /// full read). Call AFTER writing the file's bytes so the seeded mtime
    /// matches the on-disk mtime (guard fires only when current mtime is
    /// strictly greater).
    fn seed_full_read(ctx: &BuiltinToolContext, target: &std::path::Path) {
        let canon = std::fs::canonicalize(target).unwrap();
        let bytes = std::fs::read(&canon).unwrap();
        // Decode the SAME way the guard compares (raw UTF-8, BOM-stripped),
        // falling back to lossy for non-UTF-8 fixtures (e.g. UTF-16LE), where
        // the mtime check alone governs.
        let content = crate::shared::decode_utf8_strict(&bytes)
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
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
                // Simulates a prior full `Read`.
                from_read: true,
            },
        );
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
            "The file /tmp/a.txt has been updated successfully. (file state is current in your context — no need to Read it back)"
        );
    }

    #[test]
    fn edit_result_message_replace_all_is_byte_locked() {
        // FileEditTool.ts:581-586 (non-interactive, modifiedNote empty) + the
        // file-state-current suffix (binary const `Pyn`).
        assert_eq!(
            edit_result_message("/tmp/a.txt", true),
            "The file /tmp/a.txt has been updated. All occurrences were successfully replaced. (file state is current in your context — no need to Read it back)"
        );
    }

    #[test]
    fn edit_result_message_echoes_original_path_verbatim() {
        // claude-code echoes the input path, not a canonicalized form.
        assert_eq!(
            edit_result_message("./relative/../weird/path.txt", false),
            "The file ./relative/../weird/path.txt has been updated successfully. (file state is current in your context — no need to Read it back)"
        );
    }

    #[tokio::test]
    async fn single_replacement_succeeds() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello world").unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
        // Simulate the prior full Read that the staleness guard now requires.
        seed_full_read(&ctx, &target);
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
            edit_result_message(input_path, false)
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        // Byte-locked multiple-match message (FileEditTool.ts:336): `count` ==
        // TS `matches`, trailing `String:` echoes the original `old_string`.
        match err {
            ToolError::InvalidInput(m) => assert_eq!(
                m,
                "Found 3 matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.\nString: foo"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "foo foo foo");
    }

    #[tokio::test]
    async fn replace_all_handles_multiple_matches() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "foo foo foo").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
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
            edit_result_message(input_path, true)
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "bar bar bar");
    }

    #[tokio::test]
    async fn rejects_no_match() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "hello").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
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
        // Byte-locked string-to-replace-not-found message (FileEditTool.ts:321),
        // echoing the original `old_string` ("absent") verbatim.
        match err {
            ToolError::InvalidInput(m) => {
                assert_eq!(m, "String to replace not found in file.\nString: absent");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn patch_preview_truncates_with_suffix() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big.txt");
        let big: String = (0..100).map(|i| format!("L{i}\n")).collect();
        std::fs::write(&target, &big).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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
        seed_full_read(&ctx, &target);
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

    // ───────────────────────── Batch F: staleness guard ─────────────────────

    #[tokio::test]
    async fn edit_without_prior_read_errors_not_read() {
        // Editing an existing file with NO recorded Read → FILE_NOT_READ_ERROR.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("unread.txt");
        std::fs::write(&target, "hello world").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        // Deliberately do NOT seed a prior read.
        let tool = FileEditTool::new(ctx);
        let err = tool
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
            .unwrap_err();
        // ToolError::Display prefixes "invalid input: "; assert the exact
        // byte-locked message on the InvalidInput payload.
        match err {
            ToolError::InvalidInput(m) => assert_eq!(m, crate::FILE_NOT_READ_ERROR),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        // File untouched.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn partial_read_then_edit_errors_not_read() {
        // A partial (offset/limit) read does not count as having read the file:
        // Edit → FILE_NOT_READ_ERROR (isPartialView approximation).
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("partial.txt");
        std::fs::write(&target, "a\nb\nc\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let canon = std::fs::canonicalize(&target).unwrap();
        let mtime_ms = std::fs::metadata(&canon)
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        // Seed a PARTIAL read (offset set) — not a full view.
        tool_api::read_file_state::set(
            &ctx.read_file_state,
            canon,
            tool_api::read_file_state::ReadFileEntry {
                content: "a\nb\nc\n".into(),
                mtime_ms,
                offset: Some(1),
                limit: Some(2),
                // Simulates a prior PARTIAL `Read` (offset/limit set).
                from_read: true,
            },
        );
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "b",
                    "new_string": "B"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        // ToolError::Display prefixes "invalid input: "; assert the exact
        // byte-locked message on the InvalidInput payload.
        match err {
            ToolError::InvalidInput(m) => assert_eq!(m, crate::FILE_NOT_READ_ERROR),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "a\nb\nc\n");
    }

    #[tokio::test]
    async fn external_modify_then_edit_errors_modified() {
        // Read → external modify (mtime bumps AND content changes) → Edit must
        // refuse with FILE_UNEXPECTEDLY_MODIFIED_ERROR.
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("ext.txt");
        std::fs::write(&target, "original\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        // Seed the full read as of the OLD mtime/content.
        seed_full_read(&ctx, &target);
        // Now an external actor rewrites the file AND bumps the mtime forward.
        std::fs::write(&target, "tampered\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
        let tool = FileEditTool::new(ctx);
        let err = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "tampered",
                    "new_string": "x"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert_eq!(m, crate::FILE_UNEXPECTEDLY_MODIFIED_ERROR),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        // Edit refused → file left as the external content.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "tampered\n");
    }

    #[tokio::test]
    async fn same_content_touch_then_edit_proceeds_via_fallback() {
        // Read(full) → mtime bumped but bytes UNCHANGED (cloud-sync/antivirus
        // touch) → Edit proceeds via the content-equality fallback.
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("touched.txt");
        std::fs::write(&target, "keep me\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
        // Touch: bump mtime forward WITHOUT changing content.
        set_file_mtime(&target, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
        let tool = FileEditTool::new(ctx);
        let result = tool
            .call(
                json!({
                    "file_path": target.to_str().unwrap(),
                    "old_string": "keep me",
                    "new_string": "edited"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["replacements"], 1);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited\n");
    }

    #[tokio::test]
    async fn post_write_set_lets_immediate_second_edit_succeed() {
        // First Edit succeeds with a seeded read; its post-write `set` updates
        // the registry so a SECOND immediate Edit (no re-seed) also succeeds.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("twice.txt");
        std::fs::write(&target, "alpha beta\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
        let tool = FileEditTool::new(ctx);
        tool.call(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "alpha",
                "new_string": "ALPHA"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // No re-seed: the post-write map.set from the first edit must satisfy
        // the guard for the second edit.
        let result = tool
            .call(
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
        assert_eq!(result.data["replacements"], 1);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "ALPHA BETA\n");
    }

    #[tokio::test]
    async fn lf_file_round_trips_as_lf() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("lf.txt");
        std::fs::write(&target, b"alpha\nbeta\ngamma\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        seed_full_read(&ctx, &target);
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

    #[tokio::test]
    async fn description_is_verbatim_ts() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileEditTool::new(ctx);
        let d = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        // Header + the `\n-` pre-read seam (Usage: immediately followed by the
        // first bullet, which carries a trailing space after "before editing.").
        assert!(d.starts_with(
            "Performs exact string replacements in files.\n\nUsage:\n- You must use your `Read` tool"
        ));
        assert!(d.contains("before editing. This tool will error"));
        // Locks the compact-format decision (line number + tab, not padded-arrow).
        assert!(d.contains("The line number prefix format is: line number + tab."));
        // Final bullet, with NO trailing newline.
        assert!(d.ends_with(
            "This parameter is useful if you want to rename a variable for instance."
        ));
        // Locks `minimalUniquenessHint` empty (non-`ant` 3P build).
        assert!(!d.contains("smallest old_string"));
        // prompt() equals DESCRIPTION for Edit.
        let p = tool
            .prompt(&PromptOptions {
                include_examples: false,
            })
            .await;
        assert_eq!(p, d);
    }
}
