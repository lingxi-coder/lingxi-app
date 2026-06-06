//! `GrepTool` — ripgrep-faithful content search.
//!
//! 1:1 port of claude-code's `GrepTool.ts`. claude-code shells out to the
//! bundled `rg` binary; this Rust port keeps an **in-process** engine built on
//! the `ignore` + `grep_*` crates (which is what ripgrep itself is built on),
//! so flag parity is *behavioral*, not process-identical. The model-facing
//! result is a single `content` string (read verbatim by the turn-loop
//! `tool_result_to_model_text` serialization rule).
//!
//! Documented divergences from the `rg` CLI (behavioral parity, not blockers):
//! - Engine is `ignore::WalkBuilder` + `grep_searcher`, not the `rg` process.
//! - `--type` coverage is whatever `ignore::types::TypesBuilder::add_defaults`
//!   ships — may be a sub/superset of the bundled `rg` type list.
//! - Context lines (`-A`/`-B`/`-C`) are gathered via a custom `Sink`; they are
//!   relativized and use the same `relpath:line:text` field separator as match
//!   lines (the `rg` CLI uses `-` separators + keeps absolute paths for context
//!   lines because of a TS relativize quirk). No `--` group separators emitted.
//! - `--max-columns 500` is approximated by hard-truncating emitted text to 500
//!   chars (`rg` prints an omission marker instead).
//! - gitignore-relative `--glob` negation nuance (`GrepTool.ts:417-427`) is
//!   approximated via `ignore::overrides::OverrideBuilder`.
//! - mtime-desc file sort falls back to a pure filename sort under `cfg!(test)`
//!   (mirrors TS `NODE_ENV === 'test'`).
//! - `GREP_PER_FILE_CAP` (100) is a Rust-specific safety bound; `head_limit`
//!   (default 250) is the primary truncation, matching TS.

use async_trait::async_trait;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{GREP_COMPLETED, GREP_FAILED, GREP_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Grep";

/// Maximum matches per file. Spec §7 lock. Rust-specific safety bound layered
/// under `head_limit` (the primary, TS-faithful truncation).
pub const GREP_PER_FILE_CAP: usize = 100;

/// Default cap on grep results when `head_limit` is unspecified
/// (`GrepTool.ts:108`). Pass `head_limit=0` for unlimited.
const DEFAULT_HEAD_LIMIT: usize = 250;

/// VCS directories excluded from searches (`GrepTool.ts:95-102`).
const VCS_DIRECTORIES_TO_EXCLUDE: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// `--max-columns 500` equivalent (`GrepTool.ts:338`).
const MAX_COLUMNS: usize = 500;

/// Model-facing description / prompt (`prompt.ts:6-18`, byte-faithful with
/// `GREP_TOOL_NAME='Grep'`, `AGENT_TOOL_NAME='Agent'`, `BASH_TOOL_NAME='Bash'`).
const GREP_DESCRIPTION: &str = r#"A powerful search tool built on ripgrep

  Usage:
  - ALWAYS use Grep for search tasks. NEVER invoke `grep` or `rg` as a Bash command. The Grep tool has been optimized for correct permissions and access.
  - Supports full regex syntax (e.g., "log.*Error", "function\s+\w+")
  - Filter files with glob parameter (e.g., "*.js", "**/*.tsx") or type parameter (e.g., "js", "py", "rust")
  - Output modes: "content" shows matching lines, "files_with_matches" shows only file paths (default), "count" shows match counts
  - Use Agent tool for open-ended searches requiring multiple rounds
  - Pattern syntax: Uses ripgrep (not grep) - literal braces need escaping (use `interface\{\}` to find `interface{}` in Go code)
  - Multiline matching: By default patterns match within single lines only. For cross-line patterns like `struct \{[\s\S]*?field`, use `multiline: true`
"#;

/// `applyHeadLimit` (`GrepTool.ts:110-128`). `limit==Some(0)` → unlimited
/// escape hatch; otherwise slice `[offset, offset+limit)` and report the
/// applied limit only when truncation actually occurred.
fn apply_head_limit<T>(items: Vec<T>, limit: Option<usize>, offset: usize) -> (Vec<T>, Option<usize>) {
    // Explicit 0 = unlimited escape hatch.
    if limit == Some(0) {
        return (items.into_iter().skip(offset).collect(), None);
    }
    let effective = limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let len = items.len();
    let sliced: Vec<T> = items.into_iter().skip(offset).take(effective).collect();
    // Only report the applied limit when truncation actually occurred, so the
    // model knows there may be more results and can paginate with offset.
    let was_truncated = len.saturating_sub(offset) > effective;
    (sliced, if was_truncated { Some(effective) } else { None })
}

/// `formatLimitInfo` (`GrepTool.ts:134-142`). `appliedLimit` is only set when
/// truncation occurred, so build parts conditionally.
fn format_limit_info(applied_limit: Option<usize>, applied_offset: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = applied_limit {
        parts.push(format!("limit: {l}"));
    }
    if applied_offset > 0 {
        parts.push(format!("offset: {applied_offset}"));
    }
    parts.join(", ")
}

/// `plural` (`stringUtils.ts:32-38`).
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// `toRelativePath` (`path.ts:95-99`): relative to `cwd` if `cwd` is an ancestor,
/// else keep absolute (TS keeps absolute when `relative()` would start with `..`).
fn to_relative_path(abs: &Path, cwd: &Path) -> String {
    match abs.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => abs.to_string_lossy().into_owned(),
    }
}

/// Split the `glob` parameter the way `GrepTool.ts:391-409` does: split on
/// whitespace, keep brace-groups intact, otherwise split on commas.
fn split_glob_patterns(glob: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in glob.split_whitespace() {
        if raw.contains('{') && raw.contains('}') {
            out.push(raw.to_string());
        } else {
            for p in raw.split(',') {
                if !p.is_empty() {
                    out.push(p.to_string());
                }
            }
        }
    }
    out
}

/// Decode an emitted line: lossy-UTF8, strip one trailing CRLF, truncate to
/// `MAX_COLUMNS` (`--max-columns 500` approximation).
fn decode_line(bytes: &[u8]) -> String {
    let cow = String::from_utf8_lossy(bytes);
    let s = cow.as_ref();
    let s = s.strip_suffix('\n').unwrap_or(s);
    let s = s.strip_suffix('\r').unwrap_or(s);
    if s.chars().count() > MAX_COLUMNS {
        s.chars().take(MAX_COLUMNS).collect()
    } else {
        s.to_string()
    }
}

/// `semanticNumber`-ish coercion: accept JSON number or numeric string.
fn value_as_usize(v: &Value) -> Option<usize> {
    if let Some(n) = v.as_u64() {
        return Some(n as usize);
    }
    if let Some(f) = v.as_f64() {
        if f >= 0.0 {
            return Some(f as usize);
        }
    }
    if let Some(s) = v.as_str() {
        if let Ok(f) = s.trim().parse::<f64>() {
            if f >= 0.0 {
                return Some(f as usize);
            }
        }
    }
    None
}

/// `semanticBoolean`-ish coercion: accept JSON bool or boolean-ish string.
fn value_as_bool(v: &Value) -> Option<bool> {
    if let Some(b) = v.as_bool() {
        return Some(b);
    }
    if let Some(s) = v.as_str() {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => return Some(true),
            "false" | "0" | "no" | "off" => return Some(false),
            _ => {}
        }
    }
    None
}

/// Custom `grep_searcher` sink collecting match (and, for content mode, context)
/// lines, capped per file at `GREP_PER_FILE_CAP`.
struct GrepSink {
    /// `(line_number, text)` records in encounter order (content mode only).
    records: Vec<(Option<u64>, String)>,
    /// Number of *matched* (not context) lines seen, capped at the per-file cap.
    match_count: usize,
    /// Set when the per-file cap stopped collection.
    overflow: bool,
    /// Push match + context lines into `records` (content mode).
    record_lines: bool,
    /// Stop after the first match (`files_with_matches`, mirrors `rg -l`).
    first_match_only: bool,
}

impl Sink for GrepSink {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.first_match_only {
            self.match_count += 1;
            return Ok(false);
        }
        if self.match_count >= GREP_PER_FILE_CAP {
            self.overflow = true;
            return Ok(false);
        }
        if self.record_lines {
            self.records
                .push((mat.line_number(), decode_line(mat.bytes())));
        }
        self.match_count += 1;
        if self.match_count >= GREP_PER_FILE_CAP {
            self.overflow = true;
            return Ok(false);
        }
        Ok(true)
    }

    fn context(&mut self, _s: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if self.record_lines {
            self.records
                .push((ctx.line_number(), decode_line(ctx.bytes())));
        }
        Ok(true)
    }
}

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
            "pattern":     { "type": "string" },
            "path":        { "type": "string" },
            "glob":        { "type": "string" },
            "type":        { "type": "string" },
            "output_mode": {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "default": "files_with_matches"
            },
            "-A":          { "type": "number" },
            "-B":          { "type": "number" },
            "-C":          { "type": "number" },
            "context":     { "type": "number" },
            "-n":          { "type": "boolean", "default": true },
            "-i":          { "type": "boolean", "default": false },
            "head_limit":  { "type": "number" },
            "offset":      { "type": "number", "default": 0 },
            "multiline":   { "type": "boolean", "default": false }
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
        GREP_DESCRIPTION.to_string()
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        GREP_DESCRIPTION.to_string()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = tool_api::util::ids::ulid_or_uuid();

        // --- Arg parsing (GrepTool.ts:310-326) ---
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
        let type_filter = input.get("type").and_then(Value::as_str);
        let output_mode = input
            .get("output_mode")
            .and_then(Value::as_str)
            .unwrap_or("files_with_matches")
            .to_string();
        let case_insensitive = input.get("-i").and_then(value_as_bool).unwrap_or(false);
        let show_line_numbers = input.get("-n").and_then(value_as_bool).unwrap_or(true);
        let multiline = input.get("multiline").and_then(value_as_bool).unwrap_or(false);
        let context_before = input.get("-B").and_then(value_as_usize);
        let context_after = input.get("-A").and_then(value_as_usize);
        let context_c = input.get("-C").and_then(value_as_usize);
        let context_param = input.get("context").and_then(value_as_usize);
        let head_limit = input.get("head_limit").and_then(value_as_usize);
        let offset = input.get("offset").and_then(value_as_usize).unwrap_or(0);

        let content_mode = output_mode == "content";
        let files_mode = output_mode == "files_with_matches";
        let count_mode = output_mode == "count";

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

        // Relativize against the (canonicalized) workspace — mirrors TS
        // `toRelativePath(_, getCwd())`. The walk yields canonicalized paths, so
        // the cwd must be canonicalized too for `strip_prefix` to match.
        let cwd_for_rel = std::fs::canonicalize(&self.ctx.workspace)
            .unwrap_or_else(|_| self.ctx.workspace.clone());

        // --- Build regex matcher (multiline → -U --multiline-dotall) ---
        let matcher = match RegexMatcherBuilder::new()
            .case_insensitive(case_insensitive)
            .multi_line(multiline)
            .dot_matches_new_line(multiline)
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

        // --- Build glob/VCS overrides (GrepTool.ts:332-335, 391-409) ---
        // In OverrideBuilder, `!` is an ignore and a bare glob is a whitelist —
        // exactly `rg --glob` semantics (gitignore-style, matches at any depth).
        let mut ob = OverrideBuilder::new(&canon_base);
        for dir in VCS_DIRECTORIES_TO_EXCLUDE {
            // Both the dir and its contents; pruning handles the descent.
            let _ = ob.add(&format!("!{dir}"));
            let _ = ob.add(&format!("!{dir}/**"));
        }
        if let Some(g) = glob_filter {
            for pat in split_glob_patterns(g) {
                if let Err(e) = ob.add(&pat) {
                    self.emit_failed(&invocation_id, "bad_glob").await;
                    return Err(ToolError::InvalidInput(format!("invalid glob {pat:?}: {e}")));
                }
            }
        }
        let overrides = match ob.build() {
            Ok(o) => o,
            Err(e) => {
                self.emit_failed(&invocation_id, "bad_glob").await;
                return Err(ToolError::InvalidInput(format!("invalid glob: {e}")));
            }
        };

        // --- Build --type filter (GrepTool.ts:386-389) ---
        let types_filter = match type_filter {
            Some(t) => {
                let mut tb = TypesBuilder::new();
                tb.add_defaults();
                tb.select(t);
                match tb.build() {
                    Ok(types) => Some(types),
                    Err(e) => {
                        self.emit_failed(&invocation_id, "bad_type").await;
                        return Err(ToolError::InvalidInput(format!("invalid type {t:?}: {e}")));
                    }
                }
            }
            None => None,
        };

        // Effective context windows: context > -C > (-B and/or -A). Content only.
        let (ctx_before, ctx_after) = if content_mode {
            if let Some(c) = context_param {
                (Some(c), Some(c))
            } else if let Some(c) = context_c {
                (Some(c), Some(c))
            } else {
                (context_before, context_after)
            }
        } else {
            (None, None)
        };

        // --- Walk + search ---
        let mut wb = WalkBuilder::new(&canon_base);
        wb.hidden(false); // --hidden: search dotfiles
        wb.overrides(overrides);
        if let Some(t) = types_filter {
            wb.types(t);
        }

        let mut content_lines: Vec<String> = Vec::new();
        let mut count_lines: Vec<String> = Vec::new();
        let mut files_matched: Vec<(PathBuf, SystemTime)> = Vec::new();
        let mut total_matches: u64 = 0;
        let mut files_scanned: u64 = 0;
        let mut overflow_any = false;

        for entry in wb.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            files_scanned += 1;

            let mut sb = SearcherBuilder::new();
            sb.multi_line(multiline);
            sb.line_number(content_mode && show_line_numbers);
            if let Some(b) = ctx_before {
                sb.before_context(b);
            }
            if let Some(a) = ctx_after {
                sb.after_context(a);
            }
            let mut searcher = sb.build();

            let mut sink = GrepSink {
                records: Vec::new(),
                match_count: 0,
                overflow: false,
                record_lines: content_mode,
                first_match_only: files_mode,
            };
            let _ = searcher.search_path(&matcher, path, &mut sink);

            if sink.overflow {
                overflow_any = true;
            }
            total_matches += sink.match_count as u64;
            if sink.match_count == 0 {
                continue;
            }

            if content_mode {
                let rel = to_relative_path(path, &cwd_for_rel);
                for (lnum, text) in &sink.records {
                    let line = match (show_line_numbers, lnum) {
                        (true, Some(n)) => format!("{rel}:{n}:{text}"),
                        _ => format!("{rel}:{text}"),
                    };
                    content_lines.push(line);
                }
            } else if count_mode {
                let rel = to_relative_path(path, &cwd_for_rel);
                count_lines.push(format!("{rel}:{}", sink.match_count));
            } else {
                // files_with_matches: defer relativize until after sort.
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                files_matched.push((path.to_path_buf(), mtime));
            }
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, total_matches, files_scanned, duration_ms)
            .await;

        // --- Assemble per-mode model string + metadata ---
        let mut data = Map::new();
        let applied_offset = if offset > 0 { Some(offset) } else { None };

        if content_mode {
            let (limited, applied_limit) = apply_head_limit(content_lines, head_limit, offset);
            let num_lines = limited.len();
            let content_str = limited.join("\n");
            let limit_info = format_limit_info(applied_limit, offset);
            let result_content = if content_str.is_empty() {
                "No matches found".to_string()
            } else {
                content_str
            };
            let model = if limit_info.is_empty() {
                result_content
            } else {
                format!("{result_content}\n\n[Showing results with pagination = {limit_info}]")
            };
            data.insert("content".to_string(), json!(model));
            data.insert("mode".to_string(), json!("content"));
            data.insert("num_files".to_string(), json!(0));
            data.insert("filenames".to_string(), json!([] as [String; 0]));
            data.insert("num_lines".to_string(), json!(num_lines));
            if let Some(l) = applied_limit {
                data.insert("applied_limit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("applied_offset".to_string(), json!(o));
            }
            data.insert("truncated".to_string(), json!(overflow_any));
        } else if count_mode {
            let (limited, applied_limit) = apply_head_limit(count_lines, head_limit, offset);
            // Re-parse totals from the (limited) `relpath:count` lines.
            let mut total: u64 = 0;
            let mut file_count: u64 = 0;
            for line in &limited {
                if let Some(idx) = line.rfind(':') {
                    if idx > 0 {
                        if let Ok(c) = line[idx + 1..].parse::<u64>() {
                            total += c;
                            file_count += 1;
                        }
                    }
                }
            }
            let limit_info = format_limit_info(applied_limit, offset);
            let raw_content = if limited.is_empty() {
                "No matches found".to_string()
            } else {
                limited.join("\n")
            };
            let occ = if total == 1 { "occurrence" } else { "occurrences" };
            let fpl = if file_count == 1 { "file" } else { "files" };
            let pag = if limit_info.is_empty() {
                String::new()
            } else {
                format!(" with pagination = {limit_info}")
            };
            let summary =
                format!("\n\nFound {total} total {occ} across {file_count} {fpl}.{pag}");
            let model = format!("{raw_content}{summary}");
            data.insert("content".to_string(), json!(model));
            data.insert("mode".to_string(), json!("count"));
            data.insert("num_files".to_string(), json!(file_count));
            data.insert("filenames".to_string(), json!([] as [String; 0]));
            data.insert("num_matches".to_string(), json!(total));
            if let Some(l) = applied_limit {
                data.insert("applied_limit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("applied_offset".to_string(), json!(o));
            }
            data.insert("truncated".to_string(), json!(overflow_any));
        } else {
            // files_with_matches (default). Sort mtime-desc + filename tiebreak;
            // pure filename sort under cfg!(test) (TS NODE_ENV === 'test').
            files_matched.sort_by(|a, b| {
                if cfg!(test) {
                    a.0.to_string_lossy().cmp(&b.0.to_string_lossy())
                } else {
                    match b.1.cmp(&a.1) {
                        std::cmp::Ordering::Equal => {
                            a.0.to_string_lossy().cmp(&b.0.to_string_lossy())
                        }
                        other => other,
                    }
                }
            });
            let sorted: Vec<PathBuf> = files_matched.into_iter().map(|(p, _)| p).collect();
            let (limited, applied_limit) = apply_head_limit(sorted, head_limit, offset);
            let relpaths: Vec<String> = limited
                .iter()
                .map(|p| to_relative_path(p, &cwd_for_rel))
                .collect();
            let num_files = relpaths.len();
            let limit_info = format_limit_info(applied_limit, offset);
            let model = if num_files == 0 {
                "No files found".to_string()
            } else {
                let suffix = if limit_info.is_empty() {
                    String::new()
                } else {
                    format!(" {limit_info}")
                };
                format!(
                    "Found {num_files} {}{suffix}\n{}",
                    plural(num_files, "file"),
                    relpaths.join("\n")
                )
            };
            data.insert("content".to_string(), json!(model));
            data.insert("mode".to_string(), json!("files_with_matches"));
            data.insert("num_files".to_string(), json!(num_files));
            data.insert("filenames".to_string(), json!(relpaths));
            if let Some(l) = applied_limit {
                data.insert("applied_limit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("applied_offset".to_string(), json!(o));
            }
            data.insert("truncated".to_string(), json!(overflow_any));
        }

        Ok(ToolCallResult {
            data: Value::Object(data),
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

    fn content_str(result: &ToolCallResult) -> String {
        result.data["content"].as_str().unwrap().to_string()
    }

    #[test]
    fn tool_name_is_grep() {
        assert_eq!(TOOL_NAME, "Grep");
    }

    #[test]
    fn per_file_cap_byte_locked() {
        assert_eq!(GREP_PER_FILE_CAP, 100);
    }

    #[test]
    fn apply_head_limit_byte_faithful() {
        // limit 0 = unlimited (offset still applied).
        let (out, applied) = apply_head_limit(vec![1, 2, 3, 4, 5], Some(0), 2);
        assert_eq!(out, vec![3, 4, 5]);
        assert_eq!(applied, None);
        // truncation reports the effective limit.
        let (out, applied) = apply_head_limit(vec![1, 2, 3, 4, 5], Some(2), 0);
        assert_eq!(out, vec![1, 2]);
        assert_eq!(applied, Some(2));
        // no truncation reports None.
        let (out, applied) = apply_head_limit(vec![1, 2], Some(5), 0);
        assert_eq!(out, vec![1, 2]);
        assert_eq!(applied, None);
        // default limit (250) unspecified.
        let (out, applied) = apply_head_limit((0..300).collect::<Vec<_>>(), None, 0);
        assert_eq!(out.len(), 250);
        assert_eq!(applied, Some(250));
    }

    #[test]
    fn format_limit_info_byte_faithful() {
        assert_eq!(format_limit_info(None, 0), "");
        assert_eq!(format_limit_info(Some(3), 0), "limit: 3");
        assert_eq!(format_limit_info(None, 5), "offset: 5");
        assert_eq!(format_limit_info(Some(3), 5), "limit: 3, offset: 5");
    }

    #[test]
    fn split_glob_patterns_mirrors_ts() {
        assert_eq!(split_glob_patterns("*.rs"), vec!["*.rs"]);
        assert_eq!(split_glob_patterns("*.js,*.ts"), vec!["*.js", "*.ts"]);
        assert_eq!(split_glob_patterns("*.{ts,tsx}"), vec!["*.{ts,tsx}"]);
        assert_eq!(split_glob_patterns("*.js *.ts"), vec!["*.js", "*.ts"]);
    }

    #[tokio::test]
    async fn default_mode_files_with_matches() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn baz() {}").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "no match here").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "fn" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        // Filename sort under cfg!(test): a.rs before b.rs.
        assert_eq!(content_str(&result), "Found 2 files\na.rs\nb.rs");
        assert_eq!(result.data["num_files"], 2);
        assert_eq!(result.data["mode"], "files_with_matches");
    }

    #[tokio::test]
    async fn default_mode_no_files_found() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "nothing relevant").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "zzz_nomatch" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        assert_eq!(content_str(&result), "No files found");
        assert_eq!(result.data["num_files"], 0);
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
        assert_eq!(content_str(&result), "Found 1 file\na.rs");
    }

    #[tokio::test]
    async fn type_filter_rust_only() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "fn bar()").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "type": "rust" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(content_str(&result), "Found 1 file\na.rs");
    }

    #[tokio::test]
    async fn content_mode_line_numbers() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": r"fn \w+", "output_mode": "content" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // -n defaults true: relpath:line:text.
        assert_eq!(content_str(&result), "a.rs:1:fn foo() {}\na.rs:2:fn bar() {}");
        assert_eq!(result.data["num_lines"], 2);
    }

    #[tokio::test]
    async fn content_mode_no_line_numbers() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "-n": false }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(content_str(&result), "a.rs:fn foo() {}");
    }

    #[tokio::test]
    async fn content_mode_context_window() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "line1\nmatch here\nline3\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "match", "output_mode": "content", "-B": 1, "-A": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(c.contains("a.txt:1:line1"), "before context: {c}");
        assert!(c.contains("a.txt:2:match here"), "match line: {c}");
        assert!(c.contains("a.txt:3:line3"), "after context: {c}");
    }

    #[tokio::test]
    async fn content_mode_context_c_alias() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "l1\nl2\nhit\nl4\nl5\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "hit", "output_mode": "content", "-C": 1 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(content_str(&result), "a.txt:2:l2\na.txt:3:hit\na.txt:4:l4");
    }

    #[tokio::test]
    async fn content_mode_head_limit_pagination() {
        let tmp = TempDir::new().unwrap();
        let body: String = (0..10).map(|i| format!("fn f{i}\n")).collect();
        std::fs::write(tmp.path().join("a.rs"), body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "head_limit": 3 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(
            c.ends_with("\n\n[Showing results with pagination = limit: 3]"),
            "pagination note: {c}"
        );
        assert_eq!(result.data["num_lines"], 3);
        assert_eq!(result.data["applied_limit"], 3);
    }

    #[tokio::test]
    async fn content_mode_offset_only_pagination() {
        let tmp = TempDir::new().unwrap();
        let body: String = (0..5).map(|i| format!("fn f{i}\n")).collect();
        std::fs::write(tmp.path().join("a.rs"), body).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "head_limit": 0, "offset": 2 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(
            c.ends_with("\n\n[Showing results with pagination = offset: 2]"),
            "offset note: {c}"
        );
        // limit 0 = unlimited, so no applied_limit even though offset applied.
        assert!(result.data.get("applied_limit").is_none());
        assert_eq!(result.data["applied_offset"], 2);
    }

    #[tokio::test]
    async fn content_mode_max_columns_truncation() {
        let tmp = TempDir::new().unwrap();
        let long = "a".repeat(600);
        std::fs::write(tmp.path().join("a.txt"), format!("{long}\n")).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "a+", "output_mode": "content", "-n": false }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(c.contains(&"a".repeat(500)), "should keep 500 a's");
        assert!(!c.contains(&"a".repeat(501)), "should truncate at 500");
    }

    #[tokio::test]
    async fn count_mode_summary_plural() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn a\nfn b\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "fn c\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "count" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(
            c.ends_with("\n\nFound 3 total occurrences across 2 files."),
            "count summary: {c}"
        );
        assert!(c.contains("a.rs:2"), "per-file count: {c}");
        assert!(c.contains("b.rs:1"), "per-file count: {c}");
        assert_eq!(result.data["num_matches"], 3);
        assert_eq!(result.data["num_files"], 2);
    }

    #[tokio::test]
    async fn count_mode_summary_singular() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn solo\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "count" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        let c = content_str(&result);
        assert!(
            c.ends_with("\n\nFound 1 total occurrence across 1 file."),
            "singular summary: {c}"
        );
    }

    #[tokio::test]
    async fn multiline_matches_across_lines() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.go"), "struct {\n  field int\n}\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        // Without multiline this would not match; with it, dot/[\s\S] span lines.
        let result = tool
            .call(
                json!({ "pattern": r"struct \{[\s\S]*?field", "multiline": true }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(content_str(&result), "Found 1 file\na.go");
    }

    #[tokio::test]
    async fn vcs_directories_excluded() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git").join("config"), "fn secret").unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn visible").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "fn" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        // .git/config must not appear.
        assert_eq!(content_str(&result), "Found 1 file\na.rs");
    }

    #[tokio::test]
    async fn per_file_cap_sets_truncated_flag() {
        let tmp = TempDir::new().unwrap();
        let content: String = (0..150).map(|i| format!("fn f{i}() {{}}\n")).collect();
        std::fs::write(tmp.path().join("big.rs"), content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        // Content mode with unlimited head_limit so only the per-file cap bounds it.
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "head_limit": 0 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(result.data["truncated"], true);
        assert_eq!(result.data["num_lines"], GREP_PER_FILE_CAP as i64);
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

    #[tokio::test]
    async fn description_is_byte_faithful() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let d = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        assert!(d.starts_with("A powerful search tool built on ripgrep\n"));
        assert!(d.contains("\"files_with_matches\" shows only file paths (default)"));
        assert!(d.contains("use `multiline: true`"));
    }
}
