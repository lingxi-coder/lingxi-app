//! `GlobTool` — expand a `**/*.rs`-style pattern against a base directory.
//!
//! 1:1 port of claude-code's `GlobTool.ts` + `utils/glob.ts`. claude-code shells
//! out to the bundled `rg --files --glob <pattern> --sort=modified`; this Rust
//! port keeps an **in-process** engine built on the `ignore` +
//! `ignore::overrides` crates (the same crates ripgrep itself is built on), so
//! flag parity is *behavioral*, not process-identical — exactly the seam GrepTool
//! already uses.
//!
//! Engine semantics (`utils/glob.ts:91-119`):
//! - `OverrideBuilder::add(pattern)` registers a whitelist override; in
//!   gitignore/ripgrep `--glob` semantics a bare glob (`*.rs`) matches the
//!   BASENAME at ANY depth — so `*.rs` matches `sub/x.rs`, not just the root.
//!   (This fixes the headline bug where the old `globset` matcher was tested
//!   against the base-relative path, making a bare `*.rs` root-only.)
//! - `LINGXI_GLOB_NO_IGNORE` (DEFAULT **true** → `--no-ignore`): when truthy,
//!   `.gitignore`/global/exclude files are NOT respected.
//! - `LINGXI_GLOB_HIDDEN` (DEFAULT **true** → `--hidden`): when truthy,
//!   hidden (dot) files ARE included.
//!   NB (verified against ripgrep 14.1.1): a whitelist `--glob` force-includes a
//!   gitignored/hidden top-level *file* regardless of these toggles — the
//!   toggles govern whether gitignored/hidden *directories* are descended into.
//!   The `ignore` crate matches `rg` here exactly.
//! - `LINGXI_GLOB_TIMEOUT_SECONDS` (default 20; 60 on WSL): wall-clock budget
//!   on the walk (`utils/ripgrep.ts:130-133`).
//!
//! Wire locks:
//! - `MAX_GLOB_MATCHES = 100` (`GlobTool.ts:157`).
//! - Excess truncated → returns `truncated: true` field on output + appends the
//!   dynamic `(Showing N of M matching files; K more are not listed…)` notice
//!   (binary GlobTool `zem`).
//! - Results sorted OLDEST-first by mtime, capped to the first 100 (claude-code
//!   `--sort=modified` is oldest-first + `slice(0, limit)`, `utils/glob.ts:94,127`).

use async_trait::async_trait;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::path_validation::{canonicalize_and_validate, emit_blocked_event};
use tool_api::BuiltinToolContext;

use crate::grep::{is_env_truthy, ripgrep_timeout, to_relative_path, RIPGREP_TIMEOUT_MSG};

/// Tool name byte-lock.
pub const TOOL_NAME: &str = "Glob";

/// Maximum match count returned (`GlobTool.ts:157`: `globLimits?.maxResults ?? 100`).
pub const MAX_GLOB_MATCHES: usize = 100;

/// Advisory appended to the model-facing result when matches were capped at
/// `MAX_GLOB_MATCHES` (`GlobTool.ts:190-194`, byte-exact).
// (The static "(Results are truncated…)" advisory is the binary's
// `totalMatches===undefined` fallback in `zem()`; the live Glob path always has
// a defined `totalMatches`, so it emits the dynamic count message instead — see
// the truncated branch below. The static string is intentionally not emitted.)

/// Model-facing string when no files matched (`GlobTool.ts:178-183`, byte-exact).
const NO_FILES_FOUND: &str = "No files found";

/// Model-facing description / prompt — VERBATIM from `GlobTool/prompt.ts:3-7`
/// (`DESCRIPTION`). TS `GlobTool` exposes only `description`; `prompt()` returns
/// the same `DESCRIPTION` (`GlobTool.ts:143-145`), so both methods return this.
const GLOB_DESCRIPTION: &str = r#"- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead"#;

/// The SHORT Glob prompt — byte-locked VERBATIM to claude-code `Jhi(e)`'s
/// `Dh(e)===true` branch (binary offset 195605886), served to current-gen
/// default models. Single line, no interpolation.
const GLOB_PROMPT_SHORT: &str = r#"Fast file pattern matching. Supports glob patterns like "**/*.js" or "src/**/*.ts". Returns matching file paths sorted by modification time."#;

/// `extractGlobBaseDirectory` (`utils/glob.ts:17-64`): peel the static base
/// directory (everything before the first glob metachar `* ? [ {`) off a
/// pattern, returning `(base_dir, relative_pattern)`. Used to re-root absolute
/// patterns — ripgrep's `--glob` flag only works with relative patterns
/// (`utils/glob.ts:76-84`), so an absolute pattern is split into a search root +
/// relative remainder.
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
    /// Optional shared live-cwd cell (claude-code `getCwd()`/`Ct()`). When
    /// injected (desktop), the no-`path` default dir, the "Directory does not
    /// exist" cwd note, and result relativization follow the post-`cd`
    /// directory; when absent (mobile/tests) they fall back to `ctx.workspace`.
    live_cwd: Option<tool_api::LiveCwdCell>,
}

impl GlobTool {
    /// Construct a new tool.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self {
            ctx,
            live_cwd: None,
        }
    }

    /// Inject the shared live-cwd cell (builder; default is `None`). The desktop
    /// composition root passes the SAME cell the `BashTool` writes on a `cd`, so
    /// Glob's live cwd tracks the shell — 1:1 with claude-code's `Ct()`.
    #[must_use]
    pub fn with_live_cwd(mut self, cell: tool_api::LiveCwdCell) -> Self {
        self.live_cwd = Some(cell);
        self
    }

    /// The effective live cwd: the injected cell's value if present (OURS
    /// P2-08 — tracks a foreground Bash `cd`), else the session cwd
    /// [`BuiltinToolContext::cwd`] (THEIRS worktree-206 — the boot cwd until an
    /// `EnterWorktree`/`ExitWorktree` swaps `session_cwd`). Both are the
    /// claude-code `Ct()` fallback; they coincide at boot with no cell injected.
    fn cwd_now(&self) -> std::path::PathBuf {
        self.live_cwd
            .as_ref()
            .map(|c| c.lock().unwrap().clone())
            .unwrap_or_else(|| self.ctx.cwd())
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["pattern"],
        "properties": {
            "pattern": { "type": "string", "description": "The glob pattern to match files against" },
            "path":    { "type": "string", "description": "The directory to search in. If not specified, the current working directory will be used. IMPORTANT: Omit this field to use the default directory. DO NOT enter \"undefined\" or \"null\" - simply omit it for the default behavior. Must be a valid directory path if provided." }
        }
    })
});

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified against the
    /// binary, 2 hits).
    fn search_hint(&self) -> Option<&str> {
        Some("find files by name pattern or wildcard")
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

    /// 1:1 with claude-code Glob `validateInput({path})`: a supplied `path` must
    /// be an existing directory, else a distinct "Directory does not exist" /
    /// "Path is not a directory" message (see [`crate::dir_validate`]). The cwd
    /// (`Pt()`) is the tool's workspace.
    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        if let Some(path) = input.get("path").and_then(Value::as_str) {
            crate::dir_validate::validate_search_directory(path, &self.cwd_now())?;
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
        GLOB_DESCRIPTION.to_string()
    }

    async fn prompt(&self, opts: &PromptOptions) -> String {
        // Model-gated, mirroring claude-code `prompt({model:e}){return Jhi(e)}`
        // where `Jhi(e){if(Dh(e))return SHORT; return KBr}` (binary offset
        // 195605879). `description()` stays the static LONG `KBr`. Predicate
        // shared with TodoWrite via `tool_api`.
        if tool_api::dh_simple_system_prompt(opts.model.as_deref()) {
            GLOB_PROMPT_SHORT.to_string()
        } else {
            GLOB_DESCRIPTION.to_string()
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // `trusted_dirs` snapshot for this whole call (canonicalize gate). The
        // effective cwd (base fallback + relativize) comes from `cwd_now()`,
        // which prefers the injected live-cwd cell (OURS P2-08, tracks Bash
        // `cd`) and otherwise falls back to `ctx.cwd()` (THEIRS session_cwd).
        let trusted = self.ctx.trusted_dirs();

        let pattern = input
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("pattern is required".into()))?;

        // claude-code Glob `getPath({path:e}){return e?$i(e):Ct()}`: a supplied
        // `path` is used as-is, otherwise the LIVE cwd (`Ct()`). The no-cell
        // fallback (`cwd_now()`==`ctx.workspace`) equals the former
        // `trusted_dirs.first()` boot cwd, so behavior is byte-identical until a
        // Bash `cd` moves the shared cell.
        let base = input
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd_now());

        let started = Instant::now();

        // An absolute pattern (e.g. `/abs/proj/**/*.rs`) is split into a static
        // base dir + relative remainder — ripgrep's `--glob` only works with
        // relative patterns (`utils/glob.ts:76-84`). The OverrideBuilder is
        // anchored at the (re-rooted) base, so the remainder is what we register.
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

        let canon_base = match canonicalize_and_validate(&base, &trusted) {
            Ok(p) => p,
            Err(_) => {
                emit_blocked_event(&self.ctx.bus, TOOL_NAME, &base).await;
                return Err(ToolError::PathBlocked { path: base });
            }
        };

        // --- Build the `--glob <pattern>` whitelist override (`utils/glob.ts:100-107`) ---
        // In OverrideBuilder, a bare glob is a whitelist and (gitignore/ripgrep
        // `--glob` semantics) matches the basename at ANY depth — so `*.rs`
        // matches `sub/x.rs`, fixing the old root-only behavior.
        let mut ob = OverrideBuilder::new(&canon_base);
        if let Err(e) = ob.add(pattern) {
            return Err(ToolError::InvalidInput(format!(
                "invalid glob pattern {pattern:?}: {e}"
            )));
        }
        // Read(deny) exclusions (`glob.ts` `lLa()`: `for(_ of l) d.push("--glob",
        // `!${_}`)`). Each active `Read`-`deny` rule (resolved at boot by
        // `permission::read_deny_exclude_globs`) is added as a negated override
        // so a denied/sensitive path is never listed. The reference prefixes a
        // bare `!` to every entry — `F4e` already prepends a leading `/` to the
        // rooted entries (and unrooted entries pass through relative), so we
        // mirror that with `!{p}` verbatim. A `!`-prefixed `OverrideBuilder`
        // pattern is an ignore.
        for p in &self.ctx.read_deny_exclude_globs {
            let _ = ob.add(&format!("!{p}"));
        }
        let overrides = match ob.build() {
            Ok(o) => o,
            Err(e) => {
                return Err(ToolError::InvalidInput(format!(
                    "invalid glob pattern {pattern:?}: {e}"
                )));
            }
        };

        // --- Env toggles (`utils/glob.ts:98-99`, `isEnvTruthy(... || 'true')`) ---
        // DEFAULT TRUE for both: NO_IGNORE → don't respect .gitignore;
        // HIDDEN → include dotfiles.
        let no_ignore = is_env_truthy("LINGXI_GLOB_NO_IGNORE", true);
        let hidden = is_env_truthy("LINGXI_GLOB_HIDDEN", true);

        let mut wb = WalkBuilder::new(&canon_base);
        wb.overrides(overrides);
        if no_ignore {
            // `--no-ignore`: ignore every ignore source.
            wb.git_ignore(false)
                .ignore(false)
                .git_global(false)
                .git_exclude(false);
        }
        if hidden {
            // `--hidden`: INCLUDE hidden files (WalkBuilder hides them by default).
            wb.hidden(false);
        }

        // --- Wall-clock budget on the walk (`utils/ripgrep.ts:130-133`) ---
        // `LINGXI_GLOB_TIMEOUT_SECONDS` overrides; else 20s (60s on WSL).
        // (The TS path shells `rg` with an execFile timeout + SIGKILL; the
        // in-process equivalent is a deadline checked each walk step. No tokio
        // timer / new dep needed — the walk is synchronous CPU/IO work.)
        let is_wsl = self.ctx.platform.as_str() == "wsl";
        let timeout = ripgrep_timeout(is_wsl);
        let deadline = started + timeout;
        let mut timed_out = false;

        let mut hits: Vec<(PathBuf, SystemTime)> = Vec::new();
        for entry in wb.build() {
            if Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            // Skip the search root itself + any non-file entries.
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            hits.push((entry.path().to_path_buf(), mtime));
        }

        // Mirror `utils/ripgrep.ts:444-454`: a timeout with NO results is a hard
        // error (so the model knows the search didn't complete); a timeout WITH
        // partial results returns them.
        if timed_out && hits.is_empty() {
            return Err(ToolError::Io(RIPGREP_TIMEOUT_MSG(is_wsl)));
        }

        let total = hits.len();
        // Oldest-first by mtime, matching claude-code `--sort=modified`
        // (`utils/glob.ts:94`) + `slice(0, limit)` keeping the OLDEST 100.
        hits.sort_by(|a, b| a.1.cmp(&b.1)); // oldest first
        let truncated = total > MAX_GLOB_MATCHES;
        if truncated {
            hits.truncate(MAX_GLOB_MATCHES);
        }

        // Relativize each hit against the canonicalized LIVE cwd (TS
        // `files.map(toRelativePath)`, `GlobTool.ts:166`, where `toRelativePath`
        // is relative to `Ct()`). The walk yields canonicalized paths, so the cwd
        // must be canonicalized too for `strip_prefix` to match — same rule
        // GrepTool uses.
        let live_cwd = self.cwd_now();
        let cwd_for_rel = std::fs::canonicalize(&live_cwd).unwrap_or_else(|_| live_cwd.clone());
        let matches: Vec<String> = hits
            .iter()
            .map(|(p, _)| to_relative_path(p, &cwd_for_rel))
            .collect();

        // Model-facing string (`mapToolResultToToolResultBlockParam`,
        // `GlobTool.ts:177-197`): "No files found" when empty, else the joined
        // paths plus the truncation advisory when capped.
        let content = if matches.is_empty() {
            NO_FILES_FOUND.to_string()
        } else if truncated {
            // Binary GlobTool `zem(e)`: the live walk always sets
            // `totalMatches` (defined) and `countIsComplete=true`, so the
            // truncation notice is the DYNAMIC count message, not the static
            // "(Results are truncated…)" fallback (which only fires when
            // `totalMatches===undefined` — never on this path).
            let shown = matches.len();
            let remaining = total - shown;
            format!(
                "{}\n(Showing {shown} of {total} matching files; {remaining} more are not listed. Narrow the pattern or path to see the rest.)",
                matches.join("\n")
            )
        } else {
            matches.join("\n")
        };

        // `data` is the structured metadata block, byte-1:1 with claude-code's
        // Glob `call()` return (`GlobTool.ts`): `{filenames, durationMs, numFiles,
        // truncated, totalMatches, countIsComplete}`, in this field order/casing.
        // - `filenames` (was `matches`): the cwd-relative paths.
        // - `durationMs`: wall-clock since `started` (TS `Date.now()-o`).
        // - `numFiles`: `filenames.len()` (after truncation), TS `u.length`.
        // - `totalMatches`: total hits BEFORE the 100-cap (TS `h.length`).
        // - `countIsComplete`: `true` — the in-process walk never truncates its
        //   own output upstream (TS `!f`, where `f` is always `false` here).
        // The model-facing string moves OUT of `data` onto `model_content` so the
        // model text is the tool's STRING, not a JSON dump of `data`.
        let duration_ms = started.elapsed().as_millis() as u64;
        let num_files = matches.len();
        Ok(ToolCallResult {
            data: json!({
                "filenames": matches,
                "durationMs": duration_ms,
                "numFiles": num_files,
                "truncated": truncated,
                "totalMatches": total,
                "countIsComplete": true,
            }),
            model_content: Some(content),
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

    #[tokio::test]
    async fn prompt_is_model_gated() {
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        // description() is the static LONG list (KBr), never gated.
        let d = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        assert_eq!(d, GLOB_DESCRIPTION);
        // prompt(model:None) ⇒ Dh(undefined)=false ⇒ LONG (== KBr).
        let long = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: None,
                model_profile: None,
            })
            .await;
        assert_eq!(long, GLOB_DESCRIPTION);
        // prompt(model:claude-opus-4-8) ⇒ Dh=true ⇒ SHORT (byte-anchor).
        let short = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: Some("claude-opus-4-8".to_string()),
                model_profile: None,
            })
            .await;
        assert_eq!(short, GLOB_PROMPT_SHORT);
        assert_eq!(
            short,
            "Fast file pattern matching. Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\". Returns matching file paths sorted by modification time."
        );
    }

    /// The glob env toggles (`CLAUDE_CODE_GLOB_*`) are process-global; cargo runs
    /// tests in this module concurrently. Serialize every env-sensitive test on
    /// this mutex so a toggle test can't leak `NO_IGNORE=false`/`HIDDEN=false`
    /// into a default-path test mid-walk. A `tokio::sync::Mutex` is used (not
    /// `std::sync::Mutex`) so the guard can be held across the `.call().await`
    /// without tripping `clippy::await_holding_lock`; it also never poisons.
    static ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Acquire the env lock and clear the glob toggles so a hostile ambient env
    /// can't perturb the default-path tests (defaults are NO_IGNORE=true,
    /// HIDDEN=true). The returned guard is held for the whole test body.
    async fn lock_and_clear_glob_env() -> tokio::sync::MutexGuard<'static, ()> {
        let g = ENV_MUTEX.lock().await;
        std::env::remove_var("LINGXI_GLOB_NO_IGNORE");
        std::env::remove_var("LINGXI_GLOB_HIDDEN");
        std::env::remove_var("LINGXI_GLOB_TIMEOUT_SECONDS");
        g
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
        let _env = lock_and_clear_glob_env().await;
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
        let matches = result.data["filenames"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(result.data["numFiles"], 2);
        assert_eq!(result.data["truncated"], false);
        assert_eq!(result.data["totalMatches"], 2);
        assert_eq!(result.data["countIsComplete"], true);
        // `durationMs` is always present (a non-negative integer).
        assert!(result.data["durationMs"].is_u64());
    }

    /// Worktree parity plan (Task 2) INERT INVARIANT: `BuiltinToolContext`
    /// now carries `session_cwd: Arc<SessionCwd>` instead of frozen
    /// `workspace`/`trusted_dirs` fields. With nothing ever calling
    /// `session_cwd.swap(..)` (no `EnterWorktree` in this test), `ctx.cwd()`
    /// must equal the boot cwd it was constructed with, and `Glob` must
    /// return the SAME result it did before the migration — proving the
    /// accessor indirection is byte-identical when inert.
    #[tokio::test]
    async fn no_swap_is_identical() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);

        // No `session_cwd.swap(..)` call anywhere in this test — `cwd()` must
        // still read back exactly the boot value `make_ctx` constructed.
        assert_eq!(ctx.cwd(), tmp.path());
        assert_eq!(ctx.trusted_dirs(), vec![tmp.path().to_path_buf()]);

        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        // Byte-identical to `matches_rs_files` above (pre-migration behavior).
        let matches = result.data["filenames"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(result.data["numFiles"], 2);
        assert_eq!(result.data["truncated"], false);
        assert_eq!(result.data["totalMatches"], 2);
        assert_eq!(result.data["countIsComplete"], true);
    }

    /// Read(deny) exclude globs (`glob.ts` `lLa()`): a populated
    /// `read_deny_exclude_globs` prunes the matching paths from the listing.
    /// A `/secrets/**` rooted entry skips that dir; an empty set leaves every
    /// file visible (behavior unchanged).
    #[tokio::test]
    async fn read_deny_exclude_globs_prune_listing() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("visible.rs"), "x").unwrap();
        let secrets = tmp.path().join("secrets");
        std::fs::create_dir(&secrets).unwrap();
        std::fs::write(secrets.join("key.rs"), "x").unwrap();

        // Baseline: NO excludes → both files are listed.
        {
            let (ctx, _sink) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let matches: Vec<String> = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap()
                .data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(matches.iter().any(|m| m.ends_with("secrets/key.rs")));
            assert!(matches.iter().any(|m| m.ends_with("visible.rs")));
        }

        // Populated: `/secrets/**` (the shape `read_deny_exclude_globs` yields
        // for a `Read(/secrets/**)` deny at cwd) prunes the secrets dir.
        {
            let (mut ctx, _sink) = make_ctx(&tmp);
            ctx.read_deny_exclude_globs = vec!["/secrets/**".to_string()];
            let tool = GlobTool::new(ctx);
            let matches: Vec<String> = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap()
                .data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                !matches.iter().any(|m| m.ends_with("secrets/key.rs")),
                "Read(deny) exclude must prune secrets/: {matches:?}"
            );
            assert!(
                matches.iter().any(|m| m.ends_with("visible.rs")),
                "non-denied file stays visible: {matches:?}"
            );
        }
    }

    /// The headline recursion fix: a BARE `*.rs` must match files at ANY depth
    /// (basename match), not just the search root. Under the old `globset`
    /// matcher tested against the base-relative path, `sub/x.rs` was missed.
    #[tokio::test]
    async fn bare_glob_matches_nested_dirs() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("root.rs"), "x").unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("x.rs"), "x").unwrap();
        let deep = sub.join("deeper");
        std::fs::create_dir(&deep).unwrap();
        std::fs::write(deep.join("y.rs"), "x").unwrap();
        std::fs::write(sub.join("note.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches: Vec<String> = result.data["filenames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        // All three .rs (root + nested + deeper-nested), no .txt.
        assert_eq!(matches.len(), 3, "bare *.rs must recurse: {matches:?}");
        assert!(matches.iter().any(|m| m.ends_with("root.rs")));
        assert!(matches
            .iter()
            .any(|m| m.replace('\\', "/").ends_with("sub/x.rs")));
        assert!(matches
            .iter()
            .any(|m| m.replace('\\', "/").ends_with("sub/deeper/y.rs")));
    }

    /// `**/*.rs` (the documented recursive form) still works.
    #[tokio::test]
    async fn double_star_glob_matches_nested() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("x.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("root.rs"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches = result.data["filenames"].as_array().unwrap();
        assert_eq!(matches.len(), 2, "**/*.rs should match both: {matches:?}");
    }

    #[tokio::test]
    async fn caps_at_100_with_truncated_flag() {
        let _env = lock_and_clear_glob_env().await;
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
        let matches = result.data["filenames"].as_array().unwrap();
        assert_eq!(matches.len(), MAX_GLOB_MATCHES);
        assert_eq!(result.data["numFiles"], MAX_GLOB_MATCHES);
        assert_eq!(result.data["truncated"], true);
        assert_eq!(result.data["totalMatches"], 150);
        assert_eq!(result.data["countIsComplete"], true);
    }

    #[tokio::test]
    async fn results_sorted_oldest_first() {
        use filetime::{set_file_mtime, FileTime};
        let _env = lock_and_clear_glob_env().await;
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
        let matches: Vec<&str> = result.data["filenames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // oldest-first (claude-code `--sort=modified`).
        assert!(matches[0].ends_with("old.rs"));
        assert!(matches[1].ends_with("mid.rs"));
        assert!(matches[2].ends_with("new.rs"));
    }

    #[tokio::test]
    async fn rejects_invalid_pattern() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let err = tool
            .call(json!({ "pattern": "[invalid" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid glob pattern"));
    }

    // --- ignore toggle (LINGXI_GLOB_NO_IGNORE) ---

    /// `--no-ignore` controls whether ignored *directories* are descended into.
    /// A `.ignore` file (honored standalone by `rg` / the `ignore` crate — no
    /// `.git` repo needed, verified against ripgrep 14.1.1) is the test vehicle:
    /// `.gitignore` is inert outside a git repo in BOTH rg and this engine, so a
    /// `.ignore` file is what reliably exercises the toggle. (A whitelist
    /// `--glob` still force-includes an ignored top-level *file*, matching rg —
    /// so the toggle is exercised via an ignored DIRECTORY.)
    ///
    /// NB: env mutation is process-global; the `_env` guard serializes + owns it.
    #[tokio::test]
    async fn ignore_dir_respected_only_when_no_ignore_false() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".ignore"), "vendor/\n").unwrap();
        let vendor = tmp.path().join("vendor");
        std::fs::create_dir(&vendor).unwrap();
        std::fs::write(vendor.join("dep.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("top.rs"), "x").unwrap();

        // Default (NO_IGNORE=true → --no-ignore): vendor/ IS descended.
        {
            let (ctx, _s) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let result = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap();
            let matches: Vec<String> = result.data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                matches.iter().any(|m| m.ends_with("vendor/dep.rs")),
                "default no_ignore=true should descend ignored dir: {matches:?}"
            );
        }

        // NO_IGNORE=false → respect .gitignore: vendor/ is pruned.
        std::env::set_var("LINGXI_GLOB_NO_IGNORE", "false");
        {
            let (ctx, _s) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let result = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap();
            let matches: Vec<String> = result.data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                !matches.iter().any(|m| m.ends_with("vendor/dep.rs")),
                "no_ignore=false should prune ignored dir: {matches:?}"
            );
            assert!(matches.iter().any(|m| m.ends_with("top.rs")));
        }
        // `_env` guard clears the toggles + releases the lock at scope end.
    }

    // --- hidden toggle (LINGXI_GLOB_HIDDEN) ---

    /// `--hidden` controls whether hidden (dot) *directories* are descended into.
    /// (Like gitignore, a whitelist `--glob` force-includes a hidden top-level
    /// *file*, matching real `rg`, so the toggle is exercised via a hidden
    /// DIRECTORY.)
    #[tokio::test]
    async fn hidden_dir_included_by_default_excluded_when_off() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        let hidden_dir = tmp.path().join(".hiddendir");
        std::fs::create_dir(&hidden_dir).unwrap();
        std::fs::write(hidden_dir.join("h.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("visible.rs"), "x").unwrap();

        // Default (HIDDEN=true → --hidden): .hiddendir/ IS descended.
        {
            let (ctx, _s) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let result = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap();
            let matches: Vec<String> = result.data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                matches.iter().any(|m| m.ends_with(".hiddendir/h.rs")),
                "default hidden=true should descend hidden dir: {matches:?}"
            );
        }

        // HIDDEN=false: .hiddendir/ is pruned.
        std::env::set_var("LINGXI_GLOB_HIDDEN", "false");
        {
            let (ctx, _s) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let result = tool
                .call(json!({ "pattern": "**/*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap();
            let matches: Vec<String> = result.data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                !matches.iter().any(|m| m.ends_with(".hiddendir/h.rs")),
                "hidden=false should prune hidden dir: {matches:?}"
            );
            assert!(matches.iter().any(|m| m.ends_with("visible.rs")));
        }
        // `_env` guard clears the toggles + releases the lock at scope end.
    }

    // --- model-facing `content` + cwd-relative paths ---

    #[tokio::test]
    async fn content_and_matches_are_cwd_relative() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        // `matches` is relativized (not the canonical absolute path).
        let matches: Vec<&str> = result.data["filenames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(matches, vec!["a.rs"]);
        // The model-facing string now lives on `ToolCallResult.model_content`
        // (NOT a `data` field) — joined relative paths.
        assert_eq!(
            result.model_content.as_deref(),
            Some("a.rs"),
            "model_content is the joined relative paths"
        );
        // `data` is pure metadata: no `content`/`matches` keys remain.
        assert!(result.data.get("content").is_none());
        assert!(result.data.get("matches").is_none());
    }

    #[tokio::test]
    async fn content_no_files_found_when_empty() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let result = tool
            .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        assert_eq!(result.model_content.as_deref(), Some("No files found"));
        assert_eq!(result.data["filenames"].as_array().unwrap().len(), 0);
        assert_eq!(result.data["numFiles"], 0);
        assert_eq!(result.data["truncated"], false);
        assert_eq!(result.data["totalMatches"], 0);
        assert_eq!(result.data["countIsComplete"], true);
    }

    #[tokio::test]
    async fn content_appends_truncation_advisory_when_capped() {
        let _env = lock_and_clear_glob_env().await;
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
        let content = result.model_content.as_deref().unwrap();
        // Binary `zem`: dynamic count message (150 hits, capped at 100 → 50 more).
        assert!(
            content.ends_with(
                "\n(Showing 100 of 150 matching files; 50 more are not listed. Narrow the pattern or path to see the rest.)"
            ),
            "model_content should end with the dynamic truncation notice: {content}"
        );
        assert_eq!(result.data["truncated"], true);
        // `totalMatches` is the pre-cap count (150); `numFiles` is the capped 100.
        assert_eq!(result.data["numFiles"], MAX_GLOB_MATCHES);
        assert_eq!(result.data["totalMatches"], 150);
        assert_eq!(result.data["countIsComplete"], true);
    }

    // --- absolute patterns re-rooted via extractGlobBaseDirectory ---

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
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "x").unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        // An absolute pattern is re-rooted (static base dir split out) so the
        // remaining `*.rs` is registered as a relative whitelist override.
        let pattern = format!("{}/*.rs", tmp.path().display());
        let result = tool
            .call(json!({ "pattern": pattern }), fresh_ctx(), fresh_tx())
            .await
            .unwrap();
        let matches = result.data["filenames"].as_array().unwrap();
        assert_eq!(
            matches.len(),
            2,
            "absolute pattern should match: {matches:?}"
        );
        assert_eq!(result.data["truncated"], false);
    }

    #[tokio::test]
    async fn description_is_verbatim_ts() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let tool = GlobTool::new(ctx);
        let d = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        // Verbatim GlobTool/prompt.ts DESCRIPTION (5 bullets).
        assert!(
            d.starts_with("- Fast file pattern matching tool that works with any codebase size\n")
        );
        assert!(d.contains("- Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\""));
        assert!(d.ends_with("use the Agent tool instead"));
        // prompt() equals DESCRIPTION for Glob.
        let p = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: None,
                model_profile: None,
            })
            .await;
        assert_eq!(p, d);
    }

    // ── P2-08: live-cwd cell drives the no-path default dir + relativization ───

    #[tokio::test]
    async fn live_cwd_cell_drives_default_dir_and_relativization() {
        let _env = lock_and_clear_glob_env().await;
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("root_only.rs"), "x").unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("in_sub.rs"), "x").unwrap();

        // No cell → default dir is the workspace (tmp); a bare `*.rs` matches at
        // any depth, so BOTH files are found — byte-identical to the pre-cell
        // behavior.
        {
            let (ctx, _sink) = make_ctx(&tmp);
            let tool = GlobTool::new(ctx);
            let names: Vec<String> = tool
                .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap()
                .data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert!(
                names.iter().any(|m| m.ends_with("root_only.rs")),
                "{names:?}"
            );
            assert!(names.iter().any(|m| m.ends_with("in_sub.rs")), "{names:?}");
        }

        // Cell = tmp/sub (a post-`cd`): the default dir follows the cell, so only
        // the file under sub is found, relativized against sub.
        {
            let (ctx, _sink) = make_ctx(&tmp);
            let cell = std::sync::Arc::new(std::sync::Mutex::new(sub.clone()));
            let tool = GlobTool::new(ctx).with_live_cwd(cell);
            let names: Vec<String> = tool
                .call(json!({ "pattern": "*.rs" }), fresh_ctx(), fresh_tx())
                .await
                .unwrap()
                .data["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().replace('\\', "/"))
                .collect();
            assert_eq!(
                names,
                vec!["in_sub.rs".to_string()],
                "cell must drive both the default dir and relativization: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn live_cwd_cell_drives_directory_not_found_note() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let (ctx, _sink) = make_ctx(&tmp);
        let cell = std::sync::Arc::new(std::sync::Mutex::new(sub.clone()));
        let tool = GlobTool::new(ctx).with_live_cwd(cell);
        let err = tool
            .validate_input(&json!({ "path": "no_such_dir" }), &fresh_ctx())
            .await
            .unwrap_err();
        // The "does not exist" note prints the LIVE cwd (canonicalized sub), NOT
        // the workspace (tmp).
        let canon_sub = std::fs::canonicalize(&sub).unwrap();
        assert_eq!(
            err.0,
            format!(
                "Directory does not exist: no_such_dir. Note: your current working directory is {}.",
                canon_sub.display()
            )
        );
    }
}
