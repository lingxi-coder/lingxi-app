//! `FileReadTool` — read a UTF-8 file with size guard + binary detection.
//!
//! Wire-locked constants:
//! - `MAX_FILE_READ_SIZE = 262_144` bytes (256 KB) per spec §7.
//! - Binary detection scans first `NUL_SCAN_WINDOW = 8 * 1024` bytes for NUL.
//! - Default encoding UTF-8 (BOM-aware) per spec §7.
//! - 1-based line indexing on tool input/output per spec §7.
//! - Errors carry byte-locked human strings (see [`format_too_large`],
//!   [`format_binary`]).

use crate::shared::{decode_utf8_strict, looks_binary, NUL_SCAN_WINDOW};
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
use telemetry::tengu::tool::{READ_COMPLETED, READ_FAILED, READ_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::ids::ulid_or_uuid;
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

/// Maximum file size FileReadTool will load. Spec §7 lock (256 KB).
pub const MAX_FILE_READ_SIZE: u64 = 262_144;

/// Default per-read output token budget — byte-locked to claude-code
/// `DEFAULT_MAX_OUTPUT_TOKENS` (`FileReadTool/limits.ts:18`). A full text read
/// whose estimated token count exceeds this errors with
/// [`format_max_tokens_exceeded`]. LingXi has no GrowthBook / env override and
/// `ToolUseContext` carries no `fileReadingLimits`, so the effective budget is
/// always this default (the `tengu_amber_wren` GB override + the
/// `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS` env tier are unported — 3P default).
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 25_000;

/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Read";

/// Bytes-per-token ratio for the rough local token estimate, keyed by file
/// extension — 1:1 with `bytesPerTokenForFileType` (`tokenEstimation.ts:215-224`).
/// Dense JSON has many single-char tokens, so its real ratio is ~2 not 4.
#[must_use]
fn bytes_per_token_for_file_type(ext: Option<&str>) -> u64 {
    match ext {
        Some("json" | "jsonl" | "jsonc") => 2,
        _ => 4,
    }
}

/// `roughTokenCountEstimationForFileType(content, ext)` — `Math.round(len /
/// bytesPerToken)` (`tokenEstimation.ts:203-242`). Round-half-up over a
/// nonnegative byte length is `(len + bpt/2) / bpt`. Uses the UTF-8 byte length
/// (TS uses the UTF-16 string `.length`); identical for ASCII, the same
/// convention the microcompact / mcp-output ports already use.
#[must_use]
fn rough_token_count_estimation_for_file_type(content: &str, ext: Option<&str>) -> u64 {
    let bpt = bytes_per_token_for_file_type(ext);
    u64::try_from(content.len())
        .unwrap_or(u64::MAX)
        .saturating_add(bpt / 2)
        / bpt
}

/// Build the byte-locked max-tokens-exceeded error — 1:1 with
/// `MaxFileReadTokenExceededError` (`FileReadTool.ts:175-185`).
#[must_use]
pub fn format_max_tokens_exceeded(token_count: u64, max_tokens: u64) -> String {
    format!(
        "File content ({token_count} tokens) exceeds maximum allowed tokens ({max_tokens}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
    )
}

/// Local-estimate token gate — `validateContentTokens(content, ext, maxTokens)`
/// (`FileReadTool.ts:755-772`). TS computes `tokenEstimate =
/// roughTokenCountEstimationForFileType(content, ext)`; early-passes when
/// `!tokenEstimate || tokenEstimate <= maxTokens/4`; otherwise refines with the
/// count_tokens API as `effectiveCount = apiCount ?? tokenEstimate` and errors
/// when `effectiveCount > maxTokens`.
///
/// The count_tokens API client is not reachable from the tool context, so this
/// ports TS's own offline fallback exactly: `apiCount` is `None`, hence
/// `effectiveCount == tokenEstimate`, and the gate errors iff
/// `tokenEstimate > maxTokens`. The `maxTokens/4` early-pass band still admits
/// reads in `(maxTokens/4, maxTokens]` (TS would API-refine them, but its
/// `?? tokenEstimate` fallback also admits them when the API is unavailable),
/// so behavior is identical to claude-code running offline / on a provider
/// without count_tokens. Returns the byte-locked error string on overflow.
fn validate_content_tokens(content: &str, ext: Option<&str>, max_tokens: u64) -> Result<(), String> {
    let token_estimate = rough_token_count_estimation_for_file_type(content, ext);
    if token_estimate == 0 || token_estimate <= max_tokens / 4 {
        return Ok(());
    }
    // No reachable count_tokens API → `effectiveCount = tokenEstimate`.
    if token_estimate > max_tokens {
        return Err(format_max_tokens_exceeded(token_estimate, max_tokens));
    }
    Ok(())
}

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

/// Cyber-risk mitigation reminder appended to the model-facing text of a
/// successful file read — byte-locked to claude-code (`FileReadTool.ts:729-730`).
/// Two leading `\n` separate it from the file body; one trailing `\n` closes it.
/// Skipped for models in [`MITIGATION_EXEMPT_MODELS`]. The TUI never shows this
/// (`FileReadTool.ts:409-413`: UI renders summary chrome only) — hence it lives
/// in the model-only `model_content` field, not the TUI-facing `content`.
pub const CYBER_RISK_MITIGATION_REMINDER: &str = "\n\n<system-reminder>\nWhenever you read a file, you should consider whether it would be considered malware. You CAN and SHOULD provide analysis of malware, what it is doing. But you MUST refuse to improve or augment the code. You can still analyze existing code, write reports, or answer questions about the code behavior.\n</system-reminder>\n";

/// Model-facing stub for the Read dedup (`file_unchanged`) case — byte-locked to
/// claude-code (`FileReadTool/prompt.ts:7-8`). The dedup decision itself (compare
/// the prior read's mtime + range from the read-file-state registry and
/// short-circuit) is a later batch; this constant locks the string the model
/// will see when that lands.
pub const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read tool_result in this conversation is still current — refer to that instead of re-reading.";

/// Model-facing warning when a read targets an existing but empty file —
/// byte-locked to claude-code (`FileReadTool.ts:705-706`).
pub const EMPTY_FILE_WARNING: &str =
    "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>";

/// Models for which the cyber-risk mitigation reminder is skipped — byte-locked
/// to claude-code (`FileReadTool.ts:733`). NOTE: claude-code canonicalizes the
/// model name (`getCanonicalName`) before this set lookup; LingXi compares the
/// raw `main_loop_model`, so only the already-canonical form matches — a
/// documented, behavior-neutral divergence (LingXi's models are not in this set).
pub const MITIGATION_EXEMPT_MODELS: &[&str] = &["claude-opus-4-6"];

/// Whether to append [`CYBER_RISK_MITIGATION_REMINDER`] for `model` — mirrors
/// `shouldIncludeFileReadMitigation()` (`FileReadTool.ts:735-738`).
#[must_use]
pub fn should_include_file_read_mitigation(model: &str) -> bool {
    !MITIGATION_EXEMPT_MODELS.contains(&model)
}

/// Build the model-facing offset-beyond-EOF warning — byte-locked to claude-code
/// (`FileReadTool.ts:707`). `offset` is the requested 1-based start line
/// (`data.file.startLine`); `total_lines` is TS `readFileInRange.totalLines`
/// (newline count + 1; see the FILE.4 note at the call site).
#[must_use]
pub fn format_offset_beyond_eof(offset: u64, total_lines: u64) -> String {
    format!(
        "<system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {total_lines} lines.</system-reminder>"
    )
}

/// `cat -n` line numbering for the model-facing read output — 1:1 with
/// claude-code's compact-format `addLineNumbers` (`utils/file.ts:290-319`,
/// killswitch off = the current default). Each line becomes `{n}\t{line}` where
/// `n` counts up from `start_line` (1-based); lines are joined by `\n`. Empty
/// content yields `""`. Splitting mirrors the TS `/\r?\n/` regex (a trailing
/// `\r` is stripped per line). The legacy padded-arrow format
/// (`String(n).padStart(6, ' ') + "→"`) only applied with the killswitch on and
/// is intentionally not ported.
#[must_use]
pub fn add_line_numbers(content: &str, start_line: u64) -> String {
    if content.is_empty() {
        return String::new();
    }
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .zip(start_line..)
        .map(|(line, n)| format!("{n}\t{line}"))
        .collect::<Vec<_>>()
        .join("\n")
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
            "offset": { "type": "integer", "minimum": 0 },
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
        ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = ulid_or_uuid();
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("file_path is required".into()))?;
        // Raw input values, preserved verbatim for the read-state registry
        // (TS `readFileState.set` stores the `offset`/`limit` as provided —
        // `undefined` when absent). `offset` below defaults to `1` only for
        // slicing; the registry records the un-defaulted `Option`.
        let input_offset = input.get("offset").and_then(Value::as_u64);
        let input_limit = input.get("limit").and_then(Value::as_u64);
        let offset = input_offset.unwrap_or(1);
        let limit = input_limit;

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

        let metadata = match tokio::fs::metadata(&canon).await {
            Ok(m) => m,
            Err(e) => {
                self.emit_failed(&invocation_id, "io_metadata").await;
                return Err(ToolError::Io(e.to_string()));
            }
        };
        let size = metadata.len();
        // Floor-truncated mtime in ms, matching TS `Math.floor(mtimeMs)` for
        // the read-state registry (`readFileState.set`). A missing mtime
        // (rare; e.g. platforms without mtime) falls back to the epoch (`0`).
        let mtime_ms = metadata
            .modified()
            .map(tool_api::read_file_state::mtime_ms_floor)
            .unwrap_or(0);

        // Read dedup (`FileReadTool.ts:536-573`): if this EXACT range was
        // already read (a prior `Read`, full view, same offset+limit) and the
        // file's mtime is unchanged on disk, return the byte-locked
        // `file_unchanged` stub instead of re-sending the content. The earlier
        // Read tool_result is still in context; two full copies waste
        // cache_creation tokens on every later turn. Gated on:
        //   * a recorded entry that came from a `Read` (`from_read`; TS
        //     `existingState.offset !== undefined`) — never an Edit/Write
        //     post-write entry, whose content/mtime reflect the post-edit
        //     state and would misdirect the model;
        //   * a FULL view (`offset`/`limit` both `None`; TS
        //     `!existingState.isPartialView`, approximated as in the staleness
        //     guard);
        //   * the SAME range (`entry.offset == input_offset &&
        //     entry.limit == input_limit`; TS `rangeMatch`);
        //   * unchanged mtime (`mtime_ms == entry.mtime_ms`; TS
        //     `mtimeMs === existingState.timestamp`).
        // The GB killswitch (`tengu_read_dedup_killswitch`) is unported — LingXi
        // has no GrowthBook; dedup is always enabled (3P default = killswitch
        // off). NOTE: the partial-view (`isPartialView`) flag set by TS's
        // CLAUDE.md / memory auto-injection has no LingXi analog; the
        // offset/limit approximation matches for normal `Read`-sourced entries.
        if let Some(entry) = tool_api::read_file_state::get(&self.ctx.read_file_state, &canon) {
            let is_full_view = entry.offset.is_none() && entry.limit.is_none();
            let range_match = entry.offset == input_offset && entry.limit == input_limit;
            if entry.from_read && is_full_view && range_match && mtime_ms == entry.mtime_ms {
                // Behaves like the TS early return: the model sees the stub via
                // `model_content`; the TUI payload `content` mirrors it (there
                // is no fresh file body to render). `total_lines`/`line_range`
                // are omitted — this is the `file_unchanged` result variant.
                self.emit_completed(&invocation_id, 0, started.elapsed().as_millis() as u64)
                    .await;
                return Ok(ToolCallResult {
                    data: json!({
                        "type": "file_unchanged",
                        "content": FILE_UNCHANGED_STUB,
                        "model_content": FILE_UNCHANGED_STUB,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                });
            }
        }

        // TS applies the byte cap ONLY when no `limit` is supplied
        // (`readFileInRange(..., limit === undefined ? maxSizeBytes : undefined)`
        // — FileReadTool.ts:1026). A ranged read (offset+limit) of a >256KB file
        // must succeed and return just the requested lines, so the cap is gated
        // on `input_limit.is_none()`. A no-limit oversize read still errors with
        // the byte-locked template (fixture-pinned `error_template`).
        if input_limit.is_none() && size > MAX_FILE_READ_SIZE {
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

        // Notebook (`.ipynb`) structured-cell read (`FileReadTool.ts:822-863`).
        // A notebook is parsed into an ARRAY of structured cells — NOT the
        // line-numbered plain text below — so the model sees per-cell
        // `{ cellType, source, execution_count?, cell_id, language?, outputs? }`
        // exactly as TS's `readNotebook`. The TUI `data` carries the structured
        // `cells`; the model-facing string (`model_content`) is the text-block
        // projection of TS's `mapNotebookCellsToToolResult`. The `ext` is taken
        // from the ORIGINAL input path (TS `path.extname(file_path)`), matching
        // the same case-insensitive `.ipynb` test FileEditTool uses to route to
        // NotebookEdit. A parse failure surfaces the JSON error (TS's
        // `jsonParse` throws → propagates out of `call`).
        let ext = std::path::Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if ext.as_deref() == Some("ipynb") {
            let cells = match crate::notebook_read::read_notebook(&content) {
                Ok(c) => c,
                Err(e) => {
                    self.emit_failed(&invocation_id, "notebook_parse").await;
                    return Err(ToolError::Io(e));
                }
            };
            let model_content = crate::notebook_read::render_cells_model_text(&cells);

            // Serialized cells JSON — TS's `cellsJson = jsonStringify(cells)`
            // (`FileReadTool.ts:824`). One serialization, reused for both the
            // token gate and the registry entry (matches TS's single `cellsJson`).
            let cells_json = serde_json::to_string(&cells).unwrap_or_default();

            // Token-budget gate on the cells JSON — TS runs
            // `validateContentTokens(cellsJson, ext, maxTokens)`
            // (`FileReadTool.ts:838`) on the notebook path too, after the
            // byte-size check and before recording state. `ext` is `"ipynb"`
            // (bytesPerToken 4). The Notebook byte-size cap (`cellsJsonBytes >
            // maxSizeBytes`, FileReadTool.ts:827) is a separate pre-existing
            // gap not in scope here.
            if let Err(msg) =
                validate_content_tokens(&cells_json, Some("ipynb"), DEFAULT_MAX_OUTPUT_TOKENS)
            {
                self.emit_failed(&invocation_id, "max_tokens_exceeded").await;
                return Err(ToolError::Io(msg));
            }

            let duration_ms = started.elapsed().as_millis() as u64;
            self.emit_completed(&invocation_id, bytes.len() as u64, duration_ms)
                .await;

            // Record the read — TS stores the serialized cells JSON as the
            // entry `content` (`FileReadTool.ts:842-847`) with the DEFAULTED
            // offset/limit. To keep the registry's full-view discriminator
            // (used by the staleness guard + dedup) consistent with the rest of
            // the Rust port, a no-range notebook read records `offset`/`limit`
            // as the verbatim (un-defaulted) input — `None` for a full read —
            // matching `read.rs`'s text path. The stored `content` is the
            // cells JSON (so a same-range re-read dedups byte-for-byte).
            tool_api::read_file_state::set(
                &self.ctx.read_file_state,
                canon.clone(),
                tool_api::read_file_state::ReadFileEntry {
                    content: cells_json,
                    mtime_ms,
                    offset: input_offset,
                    limit: input_limit,
                    from_read: true,
                },
            );

            return Ok(ToolCallResult {
                data: json!({
                    "type": "notebook",
                    "file_path": file_path,
                    "cells": cells,
                    "model_content": model_content,
                }),
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            });
        }

        let all_lines: Vec<&str> = content.split_inclusive('\n').collect();
        // FILE.4: `total_lines` must match TS `readFileInRange.totalLines`
        // (`utils/readFileInRange.ts:188`) = (number of '\n') + 1. TS does an
        // UNCONDITIONAL post-loop `lineIndex++` that counts the (possibly empty)
        // final fragment after the last newline, so a TRAILING newline adds a
        // phantom final line (e.g. "a\nb\nc\n" => 4, not the natural 3). The prior
        // `split_inclusive().len()` undercounted trailing-newline files by one in
        // the model-facing `total_lines` field + the offset-beyond-EOF warning.
        // An empty file stays 0 so the byte-locked EMPTY_FILE_WARNING branch still
        // fires (TS's empty path is upstream of readFileInRange and not reproduced
        // here). NOTE: `all_lines` (split_inclusive) is what the range SLICING below
        // uses — only this model-facing count is TS-aligned.
        let total_lines = if content.is_empty() {
            0
        } else {
            content.bytes().filter(|&b| b == b'\n').count() as u64 + 1
        };
        let start_idx = (offset.saturating_sub(1) as usize).min(all_lines.len());
        let end_idx = match limit {
            Some(l) => (start_idx + l as usize).min(all_lines.len()),
            None => all_lines.len(),
        };
        let slice: String = all_lines[start_idx..end_idx].concat();
        let line_range_start = offset;
        let line_range_end = end_idx as u64;

        // Token-budget gate — `validateContentTokens(content, ext, maxTokens)`
        // (`FileReadTool.ts:1030`). Runs on the range-limited slice, BEFORE the
        // success/state side-effects, so an over-budget read throws (TS) /
        // errors (Rust) with no completed event and nothing recorded. `ext` is
        // the lowercased extension without the dot (TS `path.extname(...).
        // slice(1)`). LingXi has no `fileReadingLimits`, so the budget is the
        // default. See [`validate_content_tokens`] for the offline-fallback
        // mapping of TS's count_tokens API refinement.
        if let Err(msg) = validate_content_tokens(&slice, ext.as_deref(), DEFAULT_MAX_OUTPUT_TOKENS) {
            self.emit_failed(&invocation_id, "max_tokens_exceeded").await;
            return Err(ToolError::Io(msg));
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, bytes.len() as u64, duration_ms)
            .await;

        // Record the read into the shared read-state registry — 1:1 with TS
        // `readFileState.set(fullFilePath, {content, timestamp, offset,
        // limit})` (`FileReadTool.ts:1032`). `content` is the range-limited
        // slice TS stores (from `readFileInRange`), `mtime_ms` is floored, and
        // `offset`/`limit` are the verbatim (un-defaulted) input values. The
        // key is the canonicalized absolute path. Behavior-neutral side-effect:
        // nothing reads this map yet (staleness guards + Read dedup are later
        // batches), so the tool's result shape is unchanged.
        tool_api::read_file_state::set(
            &self.ctx.read_file_state,
            canon.clone(),
            tool_api::read_file_state::ReadFileEntry {
                content: slice.clone(),
                mtime_ms,
                offset: input_offset,
                limit: input_limit,
                // This entry is the product of a `Read` — the dedup gate
                // (`FileReadTool.ts:550` `offset !== undefined`) only short-
                // circuits against Read-sourced entries.
                from_read: true,
            },
        );

        // Model-facing serialization (FILE.A). `content` above stays the RAW
        // slice — that is what the TUI renders (`emit_tool_result` payload). The
        // model instead sees `model_content`: cat -n line numbers (+ the
        // cyber-risk reminder) for non-empty reads, or the byte-locked empty /
        // offset-beyond-EOF `<system-reminder>` warning otherwise. This mirrors
        // claude-code's `FileReadTool` mapper (`FileReadTool.ts:692-714`), where
        // the model string diverges from the UI chrome.
        let model_content = if slice.is_empty() {
            if total_lines == 0 {
                EMPTY_FILE_WARNING.to_string()
            } else {
                format_offset_beyond_eof(offset, total_lines)
            }
        } else {
            let mut mc = add_line_numbers(&slice, offset);
            if should_include_file_read_mitigation(&ctx.options.main_loop_model) {
                mc.push_str(CYBER_RISK_MITIGATION_REMINDER);
            }
            mc
        };

        Ok(ToolCallResult {
            data: json!({
                "content": slice,
                "model_content": model_content,
                "line_range": [line_range_start, line_range_end],
                "total_lines": total_lines
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
        let fs = make_dummy_fs();
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        (
            tool_api::test_support::ctx_for_file_tools(fs, bus, vec![tmp.path().to_path_buf()]),
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
        // FILE.4: TS counts the trailing newline's empty final fragment as a line
        // ("hello\nworld\n" => 3, matching readFileInRange `lineIndex`).
        assert_eq!(result.data["total_lines"], 3);
        // Model-facing string: cat -n (compact tab format, 1-based from offset)
        // + the cyber-risk reminder (ctx model "test" is not exempt).
        assert_eq!(
            result.data["model_content"],
            format!("1\thello\n2\tworld\n3\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
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
    async fn ranged_read_of_oversize_file_returns_requested_lines() {
        // FILE.1: TS gates the 256KB cap on `limit === undefined`
        // (FileReadTool.ts:1026). A >256KB file read with offset+limit must
        // return just the requested range, NOT the too-large error.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big_ranged.txt");
        // Build a >256KB file made of distinct 1-based lines so we can assert
        // the slice precisely. Each "lineNN\n" is small; pad to exceed the cap.
        let mut body = String::from("first\nsecond\nthird\n");
        // Fill past MAX_FILE_READ_SIZE with filler lines.
        while body.len() as u64 <= MAX_FILE_READ_SIZE {
            body.push_str("filler-line-of-some-length\n");
        }
        assert!(body.len() as u64 > MAX_FILE_READ_SIZE);
        std::fs::write(&target, &body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 2 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ranged read of an oversize file must succeed");
        assert_eq!(result.data["content"], "second\nthird\n");
    }

    #[tokio::test]
    async fn no_limit_read_of_oversize_file_still_errors() {
        // FILE.1 invariant: with no `limit`, the byte cap still applies and the
        // byte-locked too-large error is returned (fixture-pinned template).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("big_nolimit.txt");
        let mut body = String::from("first\nsecond\nthird\n");
        while body.len() as u64 <= MAX_FILE_READ_SIZE {
            body.push_str("filler-line-of-some-length\n");
        }
        std::fs::write(&target, &body).unwrap();
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
        // cat -n numbers from offset=1.
        assert_eq!(
            result.data["model_content"],
            format!("1\tline1\n2\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
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
        // Numbering starts at the requested offset (2), not 1.
        assert_eq!(
            result.data["model_content"],
            format!("2\tline2\n3\t{CYBER_RISK_MITIGATION_REMINDER}")
        );
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
        // FILE.4: trailing newline => phantom final line ("...\n" => 4).
        assert_eq!(result.data["total_lines"], 4);
    }

    #[tokio::test]
    async fn read_populates_read_file_state_map_with_offset_limit() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("a.txt");
        std::fs::write(&target, "line1\nline2\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        // Hold a handle to the shared registry BEFORE the ctx is moved into the
        // tool — the tool's `readFileState.set` mutates this same `Arc`.
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        // canonicalize the target the same way the tool keys the entry.
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry =
            tool_api::read_file_state::get(&map, &canon).expect("registry entry recorded on read");
        // Content is the range-limited slice (TS stores `readFileInRange`'s
        // output), and offset/limit are the verbatim input values.
        assert_eq!(entry.content, "line2\n");
        assert_eq!(entry.offset, Some(2));
        assert_eq!(entry.limit, Some(1));
        // mtime recorded as a non-negative floor-truncated millisecond value.
        assert!(entry.mtime_ms >= 0);
    }

    #[tokio::test]
    async fn read_without_offset_limit_records_none() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("b.txt");
        std::fs::write(&target, "alpha\nbeta\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry = tool_api::read_file_state::get(&map, &canon).unwrap();
        assert_eq!(entry.content, "alpha\nbeta\n");
        assert_eq!(entry.offset, None);
        assert_eq!(entry.limit, None);
    }

    #[tokio::test]
    async fn failed_read_does_not_populate_map() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("missing.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let _ = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await;
        // A read that errored (missing file) records nothing.
        assert!(map.lock().unwrap().is_empty());
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

    #[tokio::test]
    async fn empty_file_emits_empty_warning_model_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("empty.txt");
        std::fs::write(&target, "").unwrap();
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
        // TUI payload `content` stays the raw (empty) slice; the model sees the
        // empty-file warning instead of an empty string.
        assert_eq!(result.data["content"], "");
        assert_eq!(result.data["total_lines"], 0);
        assert_eq!(result.data["model_content"], EMPTY_FILE_WARNING);
    }

    #[tokio::test]
    async fn offset_beyond_eof_emits_offset_warning_model_content() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("short.txt");
        std::fs::write(&target, "a\nb\nc\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 10 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["content"], "");
        // FILE.4: "a\nb\nc\n" has 3 newlines => TS total_lines 4 (phantom final line).
        assert_eq!(result.data["total_lines"], 4);
        assert_eq!(
            result.data["model_content"],
            "<system-reminder>Warning: the file exists but is shorter than the provided offset (10). The file has 4 lines.</system-reminder>"
        );
    }

    #[test]
    fn add_line_numbers_compact_format() {
        assert_eq!(add_line_numbers("", 1), "");
        assert_eq!(
            add_line_numbers("hello\nworld\n", 1),
            "1\thello\n2\tworld\n3\t"
        );
        // Numbering starts at `start_line`.
        assert_eq!(add_line_numbers("x", 5), "5\tx");
        // CRLF: a trailing \r is stripped per line (mirrors the TS /\r?\n/ split).
        assert_eq!(add_line_numbers("a\r\nb", 1), "1\ta\n2\tb");
    }

    #[test]
    fn mitigation_reminder_gated_on_model() {
        assert!(should_include_file_read_mitigation("claude-opus-4-8"));
        assert!(should_include_file_read_mitigation("test"));
        // The one exempt model skips the reminder.
        assert!(!should_include_file_read_mitigation("claude-opus-4-6"));
    }

    // ───────────────────────── Notebook (.ipynb) structured read ────────────

    fn sample_ipynb() -> String {
        serde_json::to_string(&json!({
            "cells": [
                {
                    "cell_type": "code",
                    "id": "c1",
                    "source": ["print(", "'hi')"],
                    "execution_count": 2,
                    "outputs": [
                        { "output_type": "stream", "name": "stdout", "text": "hi\n" }
                    ]
                },
                { "cell_type": "markdown", "id": "c2", "source": "# Title" }
            ],
            "metadata": { "language_info": { "name": "python" } },
            "nbformat": 4,
            "nbformat_minor": 5
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn ipynb_read_returns_structured_cells() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_ipynb()).unwrap();
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
        // Notebook result variant: structured `cells`, NOT line-numbered text.
        assert_eq!(result.data["type"], "notebook");
        let cells = result.data["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 2);
        // Code cell: camelCase cellType, joined source, execution_count, language.
        assert_eq!(cells[0]["cellType"], "code");
        assert_eq!(cells[0]["source"], "print('hi')");
        assert_eq!(cells[0]["execution_count"], 2);
        assert_eq!(cells[0]["cell_id"], "c1");
        assert_eq!(cells[0]["language"], "python");
        assert_eq!(cells[0]["outputs"][0]["output_type"], "stream");
        assert_eq!(cells[0]["outputs"][0]["text"], "hi\n");
        // Markdown cell: cellType markdown, no language/outputs.
        assert_eq!(cells[1]["cellType"], "markdown");
        assert!(cells[1].get("language").is_none());
        // The model-facing string is the cell-block text projection.
        assert_eq!(
            result.data["model_content"],
            "<cell id=\"c1\">print('hi')</cell id=\"c1\">\n\nhi\n\n<cell id=\"c2\"><cell_type>markdown</cell_type># Title</cell id=\"c2\">"
        );
    }

    #[tokio::test]
    async fn ipynb_read_records_full_view_registry_entry() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("nb.ipynb");
        std::fs::write(&target, sample_ipynb()).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let tool = FileReadTool::new(ctx);
        let canon = std::fs::canonicalize(&target).unwrap();
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let entry = tool_api::read_file_state::get(&map, &canon).expect("notebook read recorded");
        // A no-range notebook read is a full view (offset/limit None) and is
        // flagged as Read-sourced so the staleness guard accepts a follow-up
        // NotebookEdit and the dedup gate sees a Read entry.
        assert_eq!(entry.offset, None);
        assert_eq!(entry.limit, None);
        assert!(entry.from_read);
        // Stored content is the serialized cells JSON (so a same-range re-read
        // dedups byte-for-byte).
        assert!(entry.content.contains("\"cellType\""));
    }

    #[tokio::test]
    async fn ipynb_invalid_json_errors() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("bad.ipynb");
        std::fs::write(&target, "not a notebook").unwrap();
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
        assert!(err.to_string().contains("notebook JSON parse"), "got: {err}");
    }

    // ───────────────────────── Read dedup (file_unchanged) ──────────────────

    #[tokio::test]
    async fn unchanged_reread_returns_file_unchanged_stub() {
        // A full Read, then an immediate identical re-read with no on-disk
        // change → the dedup short-circuit returns the byte-locked stub.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dedup.txt");
        std::fs::write(&target, "alpha\nbeta\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        // First read populates the registry.
        let first = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(first.data["content"], "alpha\nbeta\n");
        // Second identical read → file_unchanged stub.
        let second = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(second.data["type"], "file_unchanged");
        assert_eq!(second.data["content"], FILE_UNCHANGED_STUB);
        assert_eq!(second.data["model_content"], FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn changed_file_reread_does_not_dedup() {
        use filetime::{set_file_mtime, FileTime};
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("changed.txt");
        std::fs::write(&target, "v0\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        // External modification bumps mtime → re-read must NOT dedup.
        std::fs::write(&target, "v1\n").unwrap();
        set_file_mtime(&target, FileTime::from_unix_time(2_000_000_000, 0)).unwrap();
        let again = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Fresh content returned, not the stub.
        assert_eq!(again.data["content"], "v1\n");
        assert!(again.data.get("type").is_none());
    }

    #[tokio::test]
    async fn different_range_reread_does_not_dedup() {
        // First read full, then a ranged re-read of the same (unchanged) file:
        // the range differs, so dedup must NOT fire.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("range.txt");
        std::fs::write(&target, "l1\nl2\nl3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        tool.call(
            json!({ "file_path": target.to_str().unwrap() }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .unwrap();
        let ranged = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 2, "limit": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Different range → real content, no stub.
        assert_eq!(ranged.data["content"], "l2\n");
        assert!(ranged.data.get("type").is_none());
    }

    #[tokio::test]
    async fn write_then_read_does_not_dedup_against_post_write_entry() {
        // A Write records a post-write registry entry with `from_read=false`.
        // A subsequent Read of the same path must NOT dedup against it (TS gates
        // dedup on `offset !== undefined`, i.e. Read-sourced entries only).
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("wr.txt");
        let (ctx, _sink) = make_ctx(&tmp);
        let map = ctx.read_file_state.clone();
        let canon = {
            std::fs::write(&target, "seed\n").unwrap();
            std::fs::canonicalize(&target).unwrap()
        };
        let mtime_ms = std::fs::metadata(&canon)
            .ok()
            .and_then(|m| m.modified().ok())
            .map_or(0, tool_api::read_file_state::mtime_ms_floor);
        // Simulate a Write's post-write entry: full view, but NOT from a Read.
        tool_api::read_file_state::set(
            &map,
            canon,
            tool_api::read_file_state::ReadFileEntry {
                content: "seed\n".into(),
                mtime_ms,
                offset: None,
                limit: None,
                from_read: false,
            },
        );
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Real content returned (the write entry is not a dedup candidate).
        assert_eq!(result.data["content"], "seed\n");
        assert!(result.data.get("type").is_none());
    }

    // ───────────────────────── Token-budget gate ────────────────────────────

    #[test]
    fn rough_token_estimate_byte_locked() {
        // Round-half-up over byte length, bpt=4 for non-json: (len+2)/4.
        assert_eq!(rough_token_count_estimation_for_file_type("", None), 0);
        assert_eq!(rough_token_count_estimation_for_file_type("ab", None), 1); // (2+2)/4
        assert_eq!(
            rough_token_count_estimation_for_file_type(&"a".repeat(100_000), None),
            25_000
        );
        // json/jsonl/jsonc use bpt=2 (denser tokens): (len+1)/2.
        assert_eq!(bytes_per_token_for_file_type(Some("json")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("jsonl")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("jsonc")), 2);
        assert_eq!(bytes_per_token_for_file_type(Some("txt")), 4);
        assert_eq!(bytes_per_token_for_file_type(None), 4);
        assert_eq!(
            rough_token_count_estimation_for_file_type(&"a".repeat(50_000), Some("json")),
            25_000
        );
    }

    #[test]
    fn max_tokens_message_byte_locked() {
        assert_eq!(
            format_max_tokens_exceeded(30_000, 25_000),
            "File content (30000 tokens) exceeds maximum allowed tokens (25000). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
        );
        assert_eq!(DEFAULT_MAX_OUTPUT_TOKENS, 25_000);
    }

    #[test]
    fn validate_content_tokens_gate_behavior() {
        // Below maxTokens/4 (6250) → early pass.
        assert!(validate_content_tokens(&"a".repeat(1_000), None, 25_000).is_ok());
        // In the (maxTokens/4, maxTokens] band → passes (offline API fallback
        // admits it: effectiveCount == estimate <= maxTokens).
        // 80_000 bytes / 4 = 20_000 tokens (> 6250, <= 25000).
        assert!(validate_content_tokens(&"a".repeat(80_000), None, 25_000).is_ok());
        // Above maxTokens → error. 100_004 bytes / 4 = 25_001 > 25_000.
        assert!(validate_content_tokens(&"a".repeat(100_004), None, 25_000).is_err());
        // json density: 50_002 bytes / 2 = 25_001 > 25_000 → error.
        assert!(validate_content_tokens(&"a".repeat(50_002), Some("json"), 25_000).is_err());
    }

    #[tokio::test]
    async fn full_read_over_token_budget_errors() {
        // A full read whose estimated tokens exceed DEFAULT_MAX_OUTPUT_TOKENS
        // (25000) but stay under the 256KB byte cap must error with the
        // byte-locked max-tokens message — and emit read_failed, not completed.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dense.txt");
        // ~120 KB of ASCII (under 256KB), estimate 120000/4 = 30000 > 25000.
        let body = "x".repeat(120_000);
        std::fs::write(&target, &body).unwrap();
        let (ctx, sink) = make_ctx(&tmp);
        ctx.bus.attach_sink(sink.clone()).await;
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
        assert!(
            msg.contains("exceeds maximum allowed tokens (25000)"),
            "got: {msg}"
        );
        let events = sink.events().await;
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"tengu_tool_read_failed"));
        assert!(!names.contains(&"tengu_tool_read_completed"));
    }

    #[tokio::test]
    async fn ranged_read_under_token_budget_of_huge_file_passes() {
        // The gate runs on the range-limited slice, not the whole file. A huge
        // file read with offset+limit returning a small slice must pass.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("huge.txt");
        // > token budget if read whole, but we slice 2 lines.
        let mut body = String::from("first\nsecond\nthird\n");
        while body.len() < 120_000 {
            body.push_str("filler-line\n");
        }
        std::fs::write(&target, &body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = FileReadTool::new(ctx);
        let result = tool
            .call(
                json!({ "file_path": target.to_str().unwrap(), "offset": 1, "limit": 2 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("small slice of a huge file passes the token gate");
        assert_eq!(result.data["content"], "first\nsecond\n");
    }

    #[tokio::test]
    async fn read_under_token_budget_passes() {
        // A small file is well under the budget — no token error.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("small.txt");
        std::fs::write(&target, "hello\nworld\n").unwrap();
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
        assert_eq!(result.data["content"], "hello\nworld\n");
    }

    #[test]
    fn model_facing_constants_byte_locked() {
        assert_eq!(
            EMPTY_FILE_WARNING,
            "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>"
        );
        assert_eq!(
            format_offset_beyond_eof(500, 12),
            "<system-reminder>Warning: the file exists but is shorter than the provided offset (500). The file has 12 lines.</system-reminder>"
        );
        assert!(CYBER_RISK_MITIGATION_REMINDER.starts_with("\n\n<system-reminder>\n"));
        assert!(CYBER_RISK_MITIGATION_REMINDER.ends_with("</system-reminder>\n"));
        assert!(CYBER_RISK_MITIGATION_REMINDER.contains("would be considered malware"));
        assert!(FILE_UNCHANGED_STUB.starts_with("File unchanged since last read."));
    }
}
