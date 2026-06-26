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
//! - There is NO per-file match cap (matching TS / the `rg` CLI): count mode
//!   reports the TRUE per-file + total counts, and content mode is bounded only
//!   by `head_limit` (default 250). A large `RECORDS_CAP` (10_000) bounds the
//!   *recorded* lines per file as a memory safety valve, but never limits the
//!   *count* — so totals stay accurate even when recording stops.
//! - A wall-clock walk budget honors `CLAUDE_CODE_GLOB_TIMEOUT_SECONDS`
//!   (default 20s, 60s on WSL); a timeout with zero results is surfaced as an
//!   error (`utils/ripgrep.ts:130-133,444-454`), partial results are returned.
//! - File-read ignore-patterns (`GrepTool.ts:411-427`) are WIRED: the active
//!   `Read`-`deny` permission rules are resolved to `--glob` excludes at engine
//!   boot by `permission::read_deny_exclude_globs` (1:1 port of
//!   `F4e(U4e(toolPermissionContext), cwd)`), threaded in via
//!   `BuiltinToolContext::read_deny_exclude_globs`, and applied here as negated
//!   `ignore`-crate overrides with the reference prefixing (rooted `/P` → `!P`;
//!   relative `P` → `!**/P`). Empty (no `Read`-deny rule) ⇒ VCS excludes only.

use async_trait::async_trait;
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Grep";

/// Memory safety valve: cap on the number of *recorded* (content-mode) lines
/// per file. This bounds peak memory for a pathological single-file match storm
/// WITHOUT capping the *count* — `head_limit` (default 250) is the real,
/// TS-faithful truncation, and count mode always reports the true total. Set
/// generously so it never trips for realistic inputs (TS / the `rg` CLI have no
/// per-file cap at all).
pub const GREP_RECORDS_CAP: usize = 10_000;

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

/// The SHORT Grep prompt — byte-locked VERBATIM to claude-code `Ajr(e)`'s
/// `Dh(e)===true` branch (binary offset 197076712), served to current-gen
/// default models. `${ns}`=Bash; the two em-dashes are U+2014. The regex
/// snippets render with single backslashes (`function\s+\w+`, `interface\{\}`)
/// — the JS template literal's `\\` collapse to one `\` in the final string.
const GREP_PROMPT_SHORT: &str = r#"Content search built on ripgrep. Prefer this over `grep`/`rg` via Bash — results integrate with the permission UI and file links.

- Full regex syntax (e.g. "log.*Error", "function\s+\w+"). Ripgrep, not grep — escape literal braces (`interface\{\}`).
- Filter with `glob` (e.g. "**/*.tsx") or `type` (e.g. "js", "py", "rust").
- `output_mode`: "content" (matching lines), "files_with_matches" (paths only, default), or "count".
- `multiline: true` for patterns that span lines."#;

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

/// Approximate JS `String.prototype.localeCompare(b)` under the default
/// (en-US root) collation, used for the equal-mtime filename tiebreak in
/// `files_with_matches` (binary @200995124: `P[0].localeCompare(L[0])`). The
/// observable divergence from Rust byte `Ord` is CASE: localeCompare orders
/// letters case-insensitively at the PRIMARY level (so `a` < `B` < `c`, unlike
/// byte Ord where `B`(66) < `a`(97)), and breaks an otherwise-equal compare by
/// case at the tertiary level with LOWERCASE before uppercase (`a` < `A`,
/// `readme` < `README`). Verified against `node`:
///   "a".localeCompare("B")===-1, "a".localeCompare("A")===-1,
///   "README".localeCompare("readme")===1, "_x".localeCompare("ax")===-1,
///   "file2".localeCompare("file10")===1 (NOT numeric).
///
/// Implementation: a two-level compare — primary is the lowercased codepoint
/// order (case-insensitive); on a primary tie, the first position whose chars
/// differ (necessarily only by case) is decided lowercase-first. This is exact
/// for the ASCII filename domain (letters/digits/`._-`); full ICU/DUCET
/// weighting for arbitrary Unicode is not ported (no collation dep) — a
/// documented approximation for the rare equal-mtime non-ASCII tiebreak.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    // Primary: case-insensitive (lowercased) codepoint order.
    let primary = a.to_lowercase().cmp(&b.to_lowercase());
    if primary != Ordering::Equal {
        return primary;
    }
    // Tertiary tiebreak: at the first differing char (which can only differ by
    // case given the primary tie), lowercase sorts before uppercase.
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            match (ca.is_lowercase(), cb.is_lowercase()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => return ca.cmp(&cb),
            }
        }
    }
    a.chars().count().cmp(&b.chars().count())
}

/// `rg -o` / `--only-matching`: the matched substrings within `text`, in order
/// (a line with N matches yields N entries). Uses the SAME `grep_regex` matcher
/// the search ran with, so the extracted spans are byte-identical to ripgrep's.
fn only_matching_spans(matcher: &RegexMatcher, text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at <= bytes.len() {
        match matcher.find_at(bytes, at) {
            Ok(Some(m)) => {
                if let Ok(s) = std::str::from_utf8(&bytes[m.start()..m.end()]) {
                    out.push(s.to_string());
                }
                // Advance past the match; bump zero-width matches by one to
                // avoid looping on the same position.
                at = m.end();
                if m.start() == m.end() {
                    at += 1;
                }
            }
            _ => break,
        }
    }
    out
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
///
/// `pub(crate)` so the sibling `glob` module reuses the exact same relativize
/// rule (GLOB.1) instead of duplicating it.
pub(crate) fn to_relative_path(abs: &Path, cwd: &Path) -> String {
    match abs.strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => abs.to_string_lossy().into_owned(),
    }
}

/// `isEnvTruthy(process.env[name] || default_str)` (`envUtils.ts:32-37` +
/// `glob.ts:97-99` call form). The env var is read; if unset OR empty, the
/// `default` is used as the value; the result is truthy iff (lower+trim) is one
/// of `1`/`true`/`yes`/`on`. Mirrors TS's `||` (not `??`) so an *empty* env
/// string also falls back to the default.
///
/// `pub(crate)` so the sibling `glob` module reuses the exact same toggle rule.
pub(crate) fn is_env_truthy(name: &str, default: bool) -> bool {
    let raw = std::env::var(name).unwrap_or_default();
    let effective = if raw.is_empty() {
        // `... || 'true'`: empty/unset falls back to the default's string form.
        if default {
            "true".to_string()
        } else {
            "false".to_string()
        }
    } else {
        raw
    };
    matches!(
        effective.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Walk wall-clock budget (`utils/ripgrep.ts:130-133`):
/// `CLAUDE_CODE_GLOB_TIMEOUT_SECONDS` (parsed as integer seconds, >0) overrides;
/// otherwise the platform default of 20s (60s on WSL, which has a 3-5x file-read
/// penalty). `pub(crate)` so `glob` shares the identical budget.
pub(crate) fn ripgrep_timeout(is_wsl: bool) -> Duration {
    let default_secs = if is_wsl { 60 } else { 20 };
    let secs = std::env::var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}

/// Verbatim `RipgrepTimeoutError` message (`utils/ripgrep.ts:447-450`). The
/// `{20|60}` second figure follows the platform default. `pub(crate)` for reuse.
#[allow(non_snake_case)]
pub(crate) fn RIPGREP_TIMEOUT_MSG(is_wsl: bool) -> String {
    let secs = if is_wsl { 60 } else { 20 };
    format!(
        "Ripgrep search timed out after {secs} seconds. The search may have matched files but did not complete in time. Try searching a more specific path or pattern."
    )
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
/// lines. Counting is DECOUPLED from recording: `match_count` always counts
/// every match (so count mode reports the true total and content mode is bounded
/// only by `head_limit`, matching TS / `rg`); recording stops past
/// `GREP_RECORDS_CAP` as a pure memory valve, but the search keeps running so the
/// count stays accurate.
struct GrepSink {
    /// `(line_number, text)` records in encounter order (content mode only).
    records: Vec<(Option<u64>, String)>,
    /// True count of *matched* (not context) lines seen — never capped.
    match_count: usize,
    /// Set when the records valve (`GREP_RECORDS_CAP`) stopped *recording*
    /// lines for this file (counting continued). Surfaces `truncated: true`.
    overflow: bool,
    /// Push match + context lines into `records` (content mode).
    record_lines: bool,
    /// Stop after the first match (`files_with_matches`, mirrors `rg -l`).
    first_match_only: bool,
}

impl Sink for GrepSink {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // Always count first — counting is never limited (TS / `rg` have no
        // per-file cap; count mode must report the true total).
        self.match_count += 1;

        // `files_with_matches`: one match is enough; stop early like `rg -l`.
        if self.first_match_only {
            return Ok(false);
        }

        // Record lines only while under the memory valve. Past the valve we KEEP
        // SEARCHING (return Ok(true)) so the count keeps climbing — we just stop
        // appending to `records`.
        if self.record_lines && self.records.len() < GREP_RECORDS_CAP {
            self.records
                .push((mat.line_number(), decode_line(mat.bytes())));
            if self.records.len() >= GREP_RECORDS_CAP {
                self.overflow = true;
            }
        }
        Ok(true)
    }

    fn context(&mut self, _s: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        // Context lines also respect the records valve (they share the buffer).
        if self.record_lines && self.records.len() < GREP_RECORDS_CAP {
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
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["pattern"],
        "properties": {
            "pattern":     { "type": "string", "description": "The regular expression pattern to search for in file contents" },
            "path":        { "type": "string", "description": "File or directory to search in (rg PATH). Defaults to current working directory." },
            "glob":        { "type": "string", "description": "Glob pattern to filter files (e.g. \"*.js\", \"*.{ts,tsx}\") - maps to rg --glob" },
            "type":        { "type": "string", "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than include for standard file types." },
            "output_mode": {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "default": "files_with_matches",
                "description": "Output mode: \"content\" shows matching lines (supports -A/-B/-C context, -n line numbers, head_limit), \"files_with_matches\" shows file paths (supports head_limit), \"count\" shows match counts (supports head_limit). Defaults to \"files_with_matches\"."
            },
            "-A":          { "type": "number", "description": "Number of lines to show after each match (rg -A). Requires output_mode: \"content\", ignored otherwise." },
            "-B":          { "type": "number", "description": "Number of lines to show before each match (rg -B). Requires output_mode: \"content\", ignored otherwise." },
            "-C":          { "type": "number", "description": "Alias for context." },
            "context":     { "type": "number", "description": "Number of lines to show before and after each match (rg -C). Requires output_mode: \"content\", ignored otherwise." },
            "-n":          { "type": "boolean", "default": true, "description": "Show line numbers in output (rg -n). Requires output_mode: \"content\", ignored otherwise. Defaults to true." },
            "-i":          { "type": "boolean", "default": false, "description": "Case insensitive search (rg -i)" },
            "-o":          { "type": "boolean", "default": false, "description": "Print only the matched (non-empty) parts of each matching line, one match per output line (rg -o / --only-matching). Requires output_mode: \"content\", ignored otherwise. Defaults to false." },
            "head_limit":  { "type": "number", "description": "Limit output to first N lines/entries, equivalent to \"| head -N\". Works across all output modes: content (limits output lines), files_with_matches (limits file paths), count (limits count entries). Defaults to 250 when unspecified. Pass 0 for unlimited (use sparingly — large result sets waste context)." },
            "offset":      { "type": "number", "default": 0, "description": "Skip first N lines/entries before applying head_limit, equivalent to \"| tail -n +N | head -N\". Works across all output modes. Defaults to 0." },
            "multiline":   { "type": "boolean", "default": false, "description": "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall). Default: false." }
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

    /// 1:1 with claude-code Grep `validateInput({path})` (identical to Glob's):
    /// a supplied `path` must be an existing directory, else the distinct
    /// "Directory does not exist" / "Path is not a directory" message (see
    /// [`crate::dir_validate`]). The cwd (`Pt()`) is the tool's workspace.
    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        if let Some(path) = input.get("path").and_then(Value::as_str) {
            crate::dir_validate::validate_search_directory(path, &self.ctx.workspace)?;
        }
        Ok(())
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

    async fn prompt(&self, opts: &PromptOptions) -> String {
        // Model-gated, mirroring claude-code `prompt({model:e}){return Ajr(e)}`
        // where `Ajr(e){if(Dh(e))return SHORT; return LONG}` (binary offset
        // 197076712). `description(){return Ajr(void 0)}` is hard-pinned to the
        // LONG (`Dh(undefined)`=false). Predicate shared with TodoWrite via
        // `tool_api`.
        if tool_api::dh_simple_system_prompt(opts.model.as_deref()) {
            GREP_PROMPT_SHORT.to_string()
        } else {
            GREP_DESCRIPTION.to_string()
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
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
        // `-o` / `--only-matching` (rg -o): emit only the matched substrings, one
        // per line. Only meaningful in content mode (claude-code: "Requires
        // output_mode: content") and ignores -A/-B/-C context (like rg -o).
        let only_matching = input.get("-o").and_then(value_as_bool).unwrap_or(false);
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

        let canon_base = match canonicalize_and_validate(&base, &self.ctx.trusted_dirs) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &base).await;
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
        // Read(deny) exclusions (GrepTool.ts:417-427): each active `Read`-`deny`
        // rule (resolved by `permission::read_deny_exclude_globs` at boot) is
        // turned into a negated override so a denied/sensitive path never
        // appears in results. Prefix EXACTLY as the reference does: a rooted
        // (`/`-anchored) entry → `!P`; a bare relative entry → `!**/P` (match at
        // any depth). In `OverrideBuilder`, a `!`-prefixed pattern is an ignore.
        for p in &self.ctx.read_deny_exclude_globs {
            let neg = if p.starts_with('/') {
                format!("!{p}")
            } else {
                format!("!**/{p}")
            };
            let _ = ob.add(&neg);
        }
        if let Some(g) = glob_filter {
            for pat in split_glob_patterns(g) {
                if let Err(e) = ob.add(&pat) {
                    return Err(ToolError::InvalidInput(format!("invalid glob {pat:?}: {e}")));
                }
            }
        }
        let overrides = match ob.build() {
            Ok(o) => o,
            Err(e) => {
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
                        return Err(ToolError::InvalidInput(format!("invalid type {t:?}: {e}")));
                    }
                }
            }
            None => None,
        };

        // Effective context windows: context > -C > (-B and/or -A). Content only,
        // and never with `-o` (rg -o ignores context entirely).
        let (ctx_before, ctx_after) = if content_mode && !only_matching {
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

        // --- Wall-clock budget on the walk (`utils/ripgrep.ts:130-133`) ---
        // `CLAUDE_CODE_GLOB_TIMEOUT_SECONDS` overrides; else 20s (60s on WSL).
        // The in-process equivalent of `rg`'s execFile timeout is a deadline
        // checked each walk step (the walk is synchronous CPU/IO work — no tokio
        // timer / extra dep needed). `Platform::as_str()` returns `getPlatform()`
        // spelling ("wsl") — compared by value so this file needs no `sandbox` dep.
        let is_wsl = self.ctx.platform.as_str() == "wsl";
        let deadline = started + ripgrep_timeout(is_wsl);
        let mut timed_out = false;

        for entry in wb.build() {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();

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

            total_matches += sink.match_count as u64;
            if sink.match_count == 0 {
                continue;
            }

            if content_mode {
                let rel = to_relative_path(path, &cwd_for_rel);
                for (lnum, text) in &sink.records {
                    if only_matching {
                        // rg -o: one matched substring per output line (a line
                        // with multiple matches yields multiple output lines).
                        for m in only_matching_spans(&matcher, text) {
                            let line = match (show_line_numbers, lnum) {
                                (true, Some(n)) => format!("{rel}:{n}:{m}"),
                                _ => format!("{rel}:{m}"),
                            };
                            content_lines.push(line);
                        }
                    } else {
                        let line = match (show_line_numbers, lnum) {
                            (true, Some(n)) => format!("{rel}:{n}:{text}"),
                            _ => format!("{rel}:{text}"),
                        };
                        content_lines.push(line);
                    }
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

        // Mirror `utils/ripgrep.ts:444-454`: a timeout with NO results is a hard
        // error (so the model knows the search didn't complete rather than
        // assuming "no matches"); a timeout WITH partial results returns them.
        let had_results =
            total_matches > 0 || !files_matched.is_empty() || !content_lines.is_empty();
        if timed_out && !had_results {
            return Err(ToolError::Io(RIPGREP_TIMEOUT_MSG(is_wsl)));
        }

        // --- Assemble per-mode result data (claude-code GrepTool outputSchema,
        // 2.1.191) + the model text. Field order mirrors the binary's construction
        // (preserve_order). content/count keep the matching text IN `data.content`
        // (the model reads it via the fallback); files_with_matches has NO content
        // (the filenames ARE the result), so its model text rides on
        // `model_content`. No `truncated` — not part of the binary result. ---
        let mut data = Map::new();
        let mut mc_channel: Option<String> = None;
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
            // binary: {mode, numFiles, filenames:[], content, numLines, appliedLimit?, appliedOffset?}
            data.insert("mode".to_string(), json!("content"));
            data.insert("numFiles".to_string(), json!(0));
            data.insert("filenames".to_string(), json!([] as [String; 0]));
            data.insert("content".to_string(), json!(model));
            data.insert("numLines".to_string(), json!(num_lines));
            if let Some(l) = applied_limit {
                data.insert("appliedLimit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("appliedOffset".to_string(), json!(o));
            }
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
            // binary: {mode, numFiles, filenames:[], content, numMatches, appliedLimit?, appliedOffset?}
            data.insert("mode".to_string(), json!("count"));
            data.insert("numFiles".to_string(), json!(file_count));
            data.insert("filenames".to_string(), json!([] as [String; 0]));
            data.insert("content".to_string(), json!(model));
            data.insert("numMatches".to_string(), json!(total));
            if let Some(l) = applied_limit {
                data.insert("appliedLimit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("appliedOffset".to_string(), json!(o));
            }
        } else {
            // files_with_matches (default). Sort mtime-desc + filename tiebreak;
            // pure filename sort under cfg!(test) (TS NODE_ENV === 'test').
            // The equal-mtime tiebreak uses JS `localeCompare` semantics (binary
            // @200995124: `.sort((P,L)=>{let D=L[1]-P[1];if(D===0)return
            // P[0].localeCompare(L[0]);return D})`), NOT Rust byte Ord — see
            // [`locale_compare`].
            files_matched.sort_by(|a, b| {
                if cfg!(test) {
                    locale_compare(&a.0.to_string_lossy(), &b.0.to_string_lossy())
                } else {
                    match b.1.cmp(&a.1) {
                        std::cmp::Ordering::Equal => {
                            locale_compare(&a.0.to_string_lossy(), &b.0.to_string_lossy())
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
            // binary: {mode, filenames, numFiles, appliedLimit?, appliedOffset?} —
            // NO content (the filenames ARE the result); model text → channel.
            data.insert("mode".to_string(), json!("files_with_matches"));
            data.insert("filenames".to_string(), json!(relpaths));
            data.insert("numFiles".to_string(), json!(num_files));
            if let Some(l) = applied_limit {
                data.insert("appliedLimit".to_string(), json!(l));
            }
            if let Some(o) = applied_offset {
                data.insert("appliedOffset".to_string(), json!(o));
            }
            mc_channel = Some(model);
        }

        Ok(ToolCallResult {
            data: Value::Object(data),
            model_content: mc_channel,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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

    #[test]
    fn locale_compare_matches_js_localecompare() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        // Cases probed against node `String.prototype.localeCompare` (en-US):
        assert_eq!(locale_compare("a", "B"), Less); // case-insensitive primary (byte Ord would be Greater)
        assert_eq!(locale_compare("B", "c"), Less);
        assert_eq!(locale_compare("a", "A"), Less); // lowercase before uppercase
        assert_eq!(locale_compare("A", "a"), Greater);
        assert_eq!(locale_compare("abc", "Abc"), Less);
        assert_eq!(locale_compare("README", "readme"), Greater);
        assert_eq!(locale_compare("_x", "ax"), Less); // '_' before 'a'
        assert_eq!(locale_compare("file2", "file10"), Greater); // NOT numeric: '2' > '1'
        assert_eq!(locale_compare("Z", "a"), Greater);
        assert_eq!(locale_compare("a.txt", "A.txt"), Less);
        assert_eq!(locale_compare("same", "same"), Equal);
    }

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
        // content/count keep the text in `data.content`; files_with_matches has no
        // content field, so its model text rides on `model_content`.
        result
            .data
            .get("content")
            .and_then(serde_json::Value::as_str)
            .or(result.model_content.as_deref())
            .expect("content in data or model_content channel")
            .to_string()
    }

    #[test]
    fn tool_name_is_grep() {
        assert_eq!(TOOL_NAME, "Grep");
    }

    #[tokio::test]
    async fn prompt_is_model_gated() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        // description() = Ajr(void 0) ⇒ always LONG.
        let d = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        assert_eq!(d, GREP_DESCRIPTION);
        // prompt(model:None) ⇒ Dh(undefined)=false ⇒ LONG.
        let long = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: None,
            })
            .await;
        assert_eq!(long, GREP_DESCRIPTION);
        // prompt(model:claude-opus-4-8) ⇒ Dh=true ⇒ SHORT (byte-anchor).
        let short = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: Some("claude-opus-4-8".to_string()),
            })
            .await;
        assert_eq!(short, GREP_PROMPT_SHORT);
        assert!(short.starts_with(
            "Content search built on ripgrep. Prefer this over `grep`/`rg` via Bash \u{2014} results integrate with the permission UI and file links."
        ));
        // Regex snippets render with single backslashes.
        assert!(short.contains("\"function\\s+\\w+\""));
        assert!(short.contains("escape literal braces (`interface\\{\\}`)."));
        assert!(short.ends_with("- `multiline: true` for patterns that span lines."));
    }

    /// Count mode must report the TRUE per-file + total count, with NO per-file
    /// cap (matching TS / `rg`). A file with 150 matches reports 150, not 100.
    #[tokio::test]
    async fn count_mode_reports_true_count_no_cap() {
        let tmp = TempDir::new().unwrap();
        let content: String = (0..150).map(|i| format!("fn f{i}() {{}}\n")).collect();
        std::fs::write(tmp.path().join("big.rs"), content).unwrap();
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
        // True count (150), not the old GREP_PER_FILE_CAP (100).
        assert_eq!(result.data["numMatches"], 150);
        assert_eq!(result.data["numFiles"], 1);
        let c = content_str(&result);
        assert!(c.contains("big.rs:150"), "per-file count should be 150: {c}");
        assert!(
            c.ends_with("\n\nFound 150 total occurrences across 1 file."),
            "summary should report 150: {c}"
        );
    }

    /// Content mode is bounded only by `head_limit` (default 250), not a per-file
    /// cap: a 150-match file under unlimited `head_limit` records ALL 150 lines.
    #[tokio::test]
    async fn content_mode_no_per_file_cap_under_unlimited() {
        let tmp = TempDir::new().unwrap();
        let content: String = (0..150).map(|i| format!("fn f{i}() {{}}\n")).collect();
        std::fs::write(tmp.path().join("big.rs"), content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "head_limit": 0 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // All 150 lines recorded (old cap would have stopped at 100).
        assert_eq!(result.data["numLines"], 150);
        // (`truncated` is not part of the binary GrepTool result.)
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
    fn is_env_truthy_mirrors_ts() {
        // Unset/empty → falls back to the default's string (|| 'true').
        std::env::remove_var("LX_TEST_TOGGLE");
        assert!(is_env_truthy("LX_TEST_TOGGLE", true));
        assert!(!is_env_truthy("LX_TEST_TOGGLE", false));
        std::env::set_var("LX_TEST_TOGGLE", "");
        assert!(is_env_truthy("LX_TEST_TOGGLE", true));
        // Explicit truthy tokens (lower+trim).
        for v in ["1", "true", "YES", " on "] {
            std::env::set_var("LX_TEST_TOGGLE", v);
            assert!(is_env_truthy("LX_TEST_TOGGLE", false), "{v} should be truthy");
        }
        // Anything else is falsy (even with default=true, an explicit value wins).
        for v in ["0", "false", "no", "off", "garbage"] {
            std::env::set_var("LX_TEST_TOGGLE", v);
            assert!(!is_env_truthy("LX_TEST_TOGGLE", true), "{v} should be falsy");
        }
        std::env::remove_var("LX_TEST_TOGGLE");
    }

    #[test]
    fn ripgrep_timeout_defaults_and_override() {
        std::env::remove_var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS");
        assert_eq!(ripgrep_timeout(false), Duration::from_secs(20));
        assert_eq!(ripgrep_timeout(true), Duration::from_secs(60)); // WSL
        std::env::set_var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS", "5");
        assert_eq!(ripgrep_timeout(false), Duration::from_secs(5));
        assert_eq!(ripgrep_timeout(true), Duration::from_secs(5)); // override wins over WSL
        // Non-positive / garbage → default.
        std::env::set_var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS", "0");
        assert_eq!(ripgrep_timeout(false), Duration::from_secs(20));
        std::env::set_var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS", "nope");
        assert_eq!(ripgrep_timeout(false), Duration::from_secs(20));
        std::env::remove_var("CLAUDE_CODE_GLOB_TIMEOUT_SECONDS");
    }

    #[test]
    fn ripgrep_timeout_msg_verbatim() {
        assert_eq!(
            RIPGREP_TIMEOUT_MSG(false),
            "Ripgrep search timed out after 20 seconds. The search may have matched files but did not complete in time. Try searching a more specific path or pattern."
        );
        assert_eq!(
            RIPGREP_TIMEOUT_MSG(true),
            "Ripgrep search timed out after 60 seconds. The search may have matched files but did not complete in time. Try searching a more specific path or pattern."
        );
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
        assert_eq!(result.data["numFiles"], 2);
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
        assert_eq!(result.data["numFiles"], 0);
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

    /// Read(deny) exclude globs (GrepTool.ts:417-427): a populated
    /// `read_deny_exclude_globs` removes denied paths from the search. A rooted
    /// `/secrets/**` entry (`!P` prefixing) and a bare relative `.env` entry
    /// (`!**/P` prefixing) are both honored; an empty set is a no-op.
    #[tokio::test]
    async fn read_deny_exclude_globs_skip_matches() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}").unwrap();
        std::fs::write(tmp.path().join(".env"), "fn secret() {}").unwrap();
        let secrets = tmp.path().join("secrets");
        std::fs::create_dir(&secrets).unwrap();
        std::fs::write(secrets.join("key.rs"), "fn bar() {}").unwrap();

        // Baseline: no excludes → all three files match.
        {
            let (ctx, _sink) = make_ctx(&tmp);
            let tool = GrepTool::new(ctx);
            let out = content_str(
                &tool
                    .call(json!({ "pattern": "fn" }), fresh_ctx(), fresh_tx())
                    .await
                    .unwrap(),
            );
            assert!(out.contains("a.rs"), "baseline: {out}");
            assert!(out.contains("secrets/key.rs"), "baseline: {out}");
            assert!(out.contains(".env"), "baseline: {out}");
        }

        // Populated: rooted `/secrets/**` (→ `!/secrets/**`) prunes the dir, and
        // bare `.env` (→ `!**/.env`) prunes the dotfile; `a.rs` survives.
        {
            let (mut ctx, _sink) = make_ctx(&tmp);
            ctx.read_deny_exclude_globs =
                vec!["/secrets/**".to_string(), ".env".to_string()];
            let tool = GrepTool::new(ctx);
            let out = content_str(
                &tool
                    .call(json!({ "pattern": "fn" }), fresh_ctx(), fresh_tx())
                    .await
                    .unwrap(),
            );
            assert!(out.contains("a.rs"), "non-denied survives: {out}");
            assert!(
                !out.contains("secrets/key.rs"),
                "rooted deny pruned secrets/: {out}"
            );
            assert!(!out.contains(".env"), "relative deny pruned .env: {out}");
        }
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
        assert_eq!(result.data["numLines"], 2);
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
    async fn content_mode_only_matching() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": r"fn \w+", "output_mode": "content", "-o": true }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // -o (rg --only-matching): only the matched substrings, with line numbers
        // (-n defaults true). `fn \w+` matches "fn foo" / "fn bar", not the rest.
        assert_eq!(content_str(&result), "a.rs:1:fn foo\na.rs:2:fn bar");
    }

    #[tokio::test]
    async fn content_mode_only_matching_multiple_per_line() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "ab ab ab\n").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);
        let result = tool
            .call(
                json!({ "pattern": "ab", "output_mode": "content", "-o": true, "-n": false }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // Three matches on one line -> three output lines.
        assert_eq!(content_str(&result), "a.txt:ab\na.txt:ab\na.txt:ab");
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
        assert_eq!(result.data["numLines"], 3);
        assert_eq!(result.data["appliedLimit"], 3);
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
        assert_eq!(result.data["appliedOffset"], 2);
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
        assert_eq!(result.data["numMatches"], 3);
        assert_eq!(result.data["numFiles"], 2);
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

    /// The records valve (`GREP_RECORDS_CAP`) is a pure memory bound that trips
    /// `truncated` once recording stops — but it NEVER caps the count. A file
    /// with one match over the valve records exactly `GREP_RECORDS_CAP` content
    /// lines yet still reports the true total in count mode.
    #[tokio::test]
    async fn records_valve_sets_truncated_but_not_count() {
        let tmp = TempDir::new().unwrap();
        let n = GREP_RECORDS_CAP + 5;
        let content: String = (0..n).map(|i| format!("fn f{i}() {{}}\n")).collect();
        std::fs::write(tmp.path().join("big.rs"), content).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GrepTool::new(ctx);

        // Content mode, unlimited head_limit → records valve is the only bound.
        let result = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "content", "head_limit": 0 }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        // The records valve trips at GREP_RECORDS_CAP — observable as numLines
        // capped (`truncated` is not part of the binary GrepTool result).
        assert_eq!(result.data["numLines"], GREP_RECORDS_CAP as i64);

        // Count mode still reports the TRUE total (valve doesn't cap counting).
        let count = tool
            .call(
                json!({ "pattern": "fn", "output_mode": "count" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .unwrap();
        assert_eq!(count.data["numMatches"], n as i64);
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
