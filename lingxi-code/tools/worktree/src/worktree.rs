//! `EnterWorktreeTool` + `ExitWorktreeTool` — manage disposable git worktrees
//! via the M2-01 [`WorktreeManager`] trait. Locks (spec §7 lines 488-489):
//! - Branch prefix: `worktree-<flatten(slug)>`
//! - Path layout:   `<repo>/.lingxi/worktrees/<flatten(slug)>`
//! - Slug flatten:  `'/' -> '+'` (injective; `'+'` outside allowed charset)
//!
//! The slug-validation and flatten helpers are reimplemented locally in this
//! module to match the M2-01 contract byte-for-byte, avoiding a dependency
//! cycle (`lingxi-tools -> lingxi-platform-posix -> lingxi-lsp ->
//! lingxi-tools`). The `parity_workflow_tools` driver cross-checks the
//! locked literals.

use std::path::{Path, PathBuf};
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    ENTER_WORKTREE_COMPLETED, ENTER_WORKTREE_FAILED, ENTER_WORKTREE_STARTED,
    EXIT_WORKTREE_COMPLETED, EXIT_WORKTREE_FAILED, EXIT_WORKTREE_STARTED, WORKTREE_CREATED,
    WORKTREE_ENTERED_EXISTING,
};
use traits::worktree::{WorktreeError, WorktreeHandle};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Byte-locked git branch prefix for disposable worktrees (spec §7 line 488,
/// M2-01 lock). The full branch is `worktree-<flatten(slug)>`.
pub const WORKTREE_BRANCH_PREFIX: &str = "worktree-";
/// Byte-locked path segment under the repo root (spec §7 line 489, M2-01 lock).
pub const WORKTREE_PATH_SEGMENT: &str = ".lingxi/worktrees";
/// Byte-locked flatten character: every `/` in the slug becomes `+` (M2-01 lock).
pub const WORKTREE_FLATTEN_CHAR: char = '+';
/// Maximum allowed total length of a worktree slug (M2-01 lock).
pub const MAX_WORKTREE_SLUG_LENGTH: usize = 64;

/// Canonical tool name in the registry for `EnterWorktreeTool`.
pub const ENTER_TOOL_NAME: &str = "EnterWorktree";
/// Canonical tool name in the registry for `ExitWorktreeTool`.
pub const EXIT_TOOL_NAME: &str = "ExitWorktree";

/// Validate a caller-supplied worktree slug (mirrors M2-01 `validate_worktree_slug`).
///
/// Rules (mirrors claude-code's `src/utils/worktree.ts` + M2-01 platform-posix):
/// - Total length 1..=64 chars.
/// - Each `/`-separated segment matches `^[a-zA-Z0-9._-]+$`.
/// - No empty segments (rejects `"/foo"`, `"foo/"`, `"a//b"`, `""`).
///
/// # Errors
/// Returns [`WorktreeError::InvalidSlug`] with a human-readable detail.
pub fn validate_worktree_slug(slug: &str) -> Result<(), WorktreeError> {
    // 1:1 with the binary `_Tt`: a length cap (64 = `oac`), then per-`/`-segment
    // checks — reject the `.`/`..` path segments, reject the reserved `.git`
    // directory name (case-insensitive, trailing dots stripped), and require the
    // allowed set `ytf=/^[a-zA-Z0-9._-]+$/` (which also rejects empty segments).
    // Error messages are byte-exact: the binary wraps the slug/segment in LITERAL
    // double-quotes (`"${e}"`/`"${t}"`), so we format `"{slug}"` — NOT `{slug:?}`.
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        return Err(WorktreeError::InvalidSlug(format!(
            "Invalid worktree name: must be {MAX_WORKTREE_SLUG_LENGTH} characters or fewer (got {})",
            slug.len()
        )));
    }
    for segment in slug.split('/') {
        if segment == "." || segment == ".." {
            return Err(WorktreeError::InvalidSlug(format!(
                "Invalid worktree name \"{slug}\": must not contain \".\" or \"..\" path segments"
            )));
        }
        if segment.to_lowercase().trim_end_matches('.') == ".git" {
            return Err(WorktreeError::InvalidSlug(format!(
                "Invalid worktree name \"{slug}\": \"{segment}\" is a reserved git directory name"
            )));
        }
        if segment.is_empty()
            || !segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
        {
            return Err(WorktreeError::InvalidSlug(format!(
                "Invalid worktree name \"{slug}\": each \"/\"-separated segment must be non-empty and contain only letters, digits, dots, underscores, and dashes"
            )));
        }
    }
    Ok(())
}

/// Flatten a `/`-separated slug into a single filesystem-friendly name.
/// Replaces every `/` with `+`. Mirrors M2-01 `flatten_slug`.
#[must_use]
pub fn flatten_slug(slug: &str) -> String {
    slug.replace('/', "+")
}

/// The `{branch}` suffix in the 206 success messages: `` ` on branch {branch}` ``
/// when a real branch exists, else `""`. Byte-exact to `SCd.call`'s
/// `` r.worktreeBranch?` on branch ${r.worktreeBranch}`:"" ``.
///
/// `enter_existing` returns `branch_name == "HEAD"` for a detached-HEAD
/// worktree (posix `git rev-parse --abbrev-ref HEAD` prints the literal
/// string `"HEAD"` when detached) — treated as NO branch, matching claude's
/// falsy-`worktreeBranch` check (`"HEAD"` is truthy in JS, but claude-code
/// never surfaces a bare `"HEAD"` as `worktreeBranch`; the port's contract
/// carves this out explicitly so a detached worktree never renders
/// `" on branch HEAD"`).
#[must_use]
fn branch_suffix(branch_name: &str) -> String {
    if branch_name.is_empty() || branch_name == "HEAD" {
        String::new()
    } else {
        format!(" on branch {branch_name}")
    }
}

/// Detect whether `cwd` is already inside a `.lingxi/worktrees/<slug>`
/// directory — the port's stand-in for claude's `ky()` "already in a
/// worktree session" flag. `SessionCwd` (Task 2) tracks only the current
/// cwd, not a dedicated boolean, so this walks the path components looking
/// for the adjacent `.lingxi`, `worktrees` pair ([`WORKTREE_PATH_SEGMENT`]
/// split on `/`).
#[must_use]
fn cwd_is_in_worktree(cwd: &std::path::Path) -> bool {
    let segment: Vec<&str> = WORKTREE_PATH_SEGMENT.split('/').collect();
    let comps: Vec<&str> = cwd
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    comps.windows(segment.len()).any(|w| w == segment.as_slice())
}

/// Generate a random worktree name when the caller supplies neither `name`
/// nor `path`. claude-code's `S1e()` picks a word-pair name checked against a
/// collision set; the port has no such word list wired, so this derives a
/// short, slug-legal, effectively-unique name from a fresh ULID/UUID. Not
/// byte-matched to the oracle (the upstream name is itself random), but
/// satisfies the same contract: a fresh, valid, human-scannable slug.
#[must_use]
fn gen_random_slug() -> String {
    let id = tool_api::util::ids::ulid_or_uuid();
    let lower: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(8)
        .collect();
    format!("session-{lower}")
}

/// 206 `EnterWorktree` input: `{name?, path?}`, no required field (byte-exact
/// `_wy` shape — `E.strictObject({name: ..., path: ...})`, both optional and
/// mutually exclusive). `path` present ⇒ switch into an EXISTING worktree;
/// `path` absent ⇒ create a new one, using `name` as the slug (or a random
/// name when `name` is absent too).
#[derive(Debug, Deserialize, Default)]
struct EnterInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

/// 206 guard message (`validateInput`'s `ky()&&!e.path` branch, `errorCode:2`):
/// fires only when CREATING (no `path`) while the session is already inside a
/// worktree. Byte-exact, extracted from the 2.1.206 binary.
const ALREADY_IN_WORKTREE_MESSAGE: &str = "Already in a worktree session. Pass `path` to switch into another existing worktree, or use ExitWorktree to leave this one before creating a new worktree.";

/// 206 `yCd()` tool-use prompt (byte-exact, extracted via `grep -abo` /
/// latin-1 slicing from the 2.1.206 binary at `function yCd(){return\`...\`}`),
/// with two LingXi rebrands applied to the extracted text:
/// - `.claude/worktrees/` → `.lingxi/worktrees/` (2 occurrences; matches
///   [`WORKTREE_PATH_SEGMENT`]).
/// - `CLAUDE.md` → `LINGXI.md` (3 occurrences; matches the codebase-wide
///   memory-file rebrand, `branding::MEMORY_FILE` /
///   `orchestrator::prompt::memory_section`).
/// `settings.json` is kept verbatim — the port's settings filename is
/// unchanged (only relocated under `.lingxi/`; see
/// `tools/meta/src/config.rs::CONFIG_FILE_NAME`).
const ENTER_WORKTREE_PROMPT: &str = r#"Use this tool ONLY when explicitly instructed to work in a worktree — either by the user directly, or by project instructions (LINGXI.md / memory). This tool creates an isolated git worktree and switches the current session into it.

## When to Use

- The user explicitly says "worktree" (e.g., "start a worktree", "work in a worktree", "create a worktree", "use a worktree")
- LINGXI.md or memory instructions direct you to work in a worktree for the current task

## When NOT to Use

- The user asks to create a branch, switch branches, or work on a different branch — use git commands instead
- The user asks to fix a bug or work on a feature — use normal git workflow unless worktrees are explicitly requested by the user or project instructions
- Never use this tool unless "worktree" is explicitly mentioned by the user or in LINGXI.md / memory instructions

## Requirements

- Must be in a git repository, OR have WorktreeCreate/WorktreeRemove hooks configured in settings.json
- Must not already be in a worktree session when creating a new worktree (`name`); switching into another existing worktree via `path` is allowed

## Behavior

- In a git repository: creates a new git worktree inside `.lingxi/worktrees/` on a new branch. The base ref is governed by the `worktree.baseRef` setting: `fresh` (default) branches from origin/<default-branch>; `head` branches from your current local HEAD
- Outside a git repository: delegates to WorktreeCreate/WorktreeRemove hooks for VCS-agnostic isolation
- Switches the session's working directory to the new worktree
- Use ExitWorktree to leave the worktree mid-session (keep or remove). On session exit, if still in the worktree, the user will be prompted to keep or remove it

## Entering an existing worktree

Pass `path` instead of `name` to switch the session into a worktree that already exists (e.g., one you just created with `git worktree add`). On first entry from the launch directory, the path must appear in `git worktree list` for the repository that owns it — the current repository or, in a multi-repo workspace, a repository nested inside it; paths registered by neither are rejected. ExitWorktree will not remove a worktree entered this way; use `action: "keep"` to return to the original directory.

Switching with `path` also works when the session is already in a worktree (the previous worktree is left on disk, untouched, and only the new one is tracked for exit-time cleanup), and from agents whose working directory was pinned at launch (subagent isolation or explicit cwd). In both cases the target must be a worktree under `.lingxi/worktrees/` of the same repository, and from a pinned agent the switch only affects this agent, not the parent session. After a further switch, previously-visited worktrees are no longer writable — re-issue EnterWorktree with `path` to return to one.

## Parameters

- `name` (optional): A name for a new worktree. If neither `name` nor `path` is provided, a random name is generated.
- `path` (optional): Path to an existing worktree to enter instead of creating one — of the current repository, or (on first entry from the launch directory) of a repository nested inside it. Mutually exclusive with `name`.
"#;

#[derive(Debug, Deserialize)]
struct ExitInput {
    path: String,
    branch_name: String,
}

/// 206 `_wy` input schema — both param descriptions extracted verbatim from
/// the 2.1.206 binary (`_wy=ye(()=>E.strictObject({name:...describe('...'),
/// path:...describe("...")}))`). `strictObject` ⇒ `additionalProperties:false`;
/// neither field is required.
static ENTER_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Optional name for a new worktree. Each \"/\"-separated segment may contain only letters, digits, dots, underscores, and dashes; max 64 chars total. A random name is generated if not provided. Mutually exclusive with `path`."
            },
            "path": {
                "type": "string",
                "description": "Path to an existing worktree to switch into instead of creating a new one. Must appear in `git worktree list` for the current repo — or, on first entry from the launch directory, for a repo nested inside it (multi-repo workspace). Mutually exclusive with `name`."
            }
        },
        "additionalProperties": false
    })
});

static EXIT_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "path":        { "type": "string", "minLength": 1 },
            "branch_name": { "type": "string", "minLength": 1 }
        },
        "required": ["path", "branch_name"]
    })
});

fn fresh_invocation_id() -> String {
    tool_api::util::ids::ulid_or_uuid()
}

fn verified(s: impl Into<String>) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.into()).into_inner())
}

fn pii_tagged(s: impl Into<String>) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.into()).into_inner())
}

/// `EnterWorktreeTool` — creates a disposable worktree via the M2-01
/// `WorktreeManager` trait. Branch + path layout are byte-locked.
pub struct EnterWorktreeTool {
    ctx: BuiltinToolContext,
}

impl EnterWorktreeTool {
    /// Construct a new tool wired to the shared builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str, slug: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert("_PROTO_slug".into(), pii_tagged(slug));
        self.ctx.bus.log_event(ENTER_WORKTREE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, branch_name: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert("_PROTO_branch_name".into(), pii_tagged(branch_name));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(ENTER_WORKTREE_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, error_kind: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert("error_kind".into(), verified(error_kind));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(ENTER_WORKTREE_FAILED, md).await;
    }

    /// Fire the 206 byte-exact single success event (`tengu_worktree_created`
    /// / `tengu_worktree_entered_existing`) IN ADDITION to the port's own
    /// started/completed/failed lifecycle triad above.
    async fn emit_worktree_event(&self, event_name: &str, branch_name: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("tool_name".into(), verified(ENTER_TOOL_NAME));
        md.insert("_PROTO_branch_name".into(), pii_tagged(branch_name));
        self.ctx.bus.log_event(event_name, md).await;
    }

    /// `path` present: switch the session into an ALREADY-EXISTING worktree
    /// (`SCd.call`'s `e.path` branch). Never creates anything.
    async fn call_enter_existing(
        &self,
        invocation_id: &str,
        started_at: Instant,
        path: String,
    ) -> Result<ToolCallResult, ToolError> {
        self.emit_started(invocation_id, &path).await;
        let result = self.ctx.worktree.enter_existing(Path::new(&path)).await;
        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(handle) => {
                // Switch the session into the worktree — every FS tool reads
                // through `ctx.cwd()`/`ctx.trusted_dirs()` (Task 2), so this
                // single swap is what makes subsequent tool calls observe the
                // worktree; it also fires the orchestrator's registered
                // cache-invalidation callback (Task 5).
                self.ctx
                    .session_cwd
                    .swap(handle.path.clone(), vec![handle.path.clone()]);
                self.emit_completed(invocation_id, &handle.branch_name, duration_ms)
                    .await;
                self.emit_worktree_event(WORKTREE_ENTERED_EXISTING, &handle.branch_name)
                    .await;
                let suffix = branch_suffix(&handle.branch_name);
                let display_path = handle.path.to_string_lossy().into_owned();
                let message = format!(
                    "Entered worktree at {display_path}{suffix}. This agent's working directory and write access now point at the worktree; the previous directory was left untouched."
                );
                Ok(ToolCallResult {
                    data: json!({
                        "path": display_path,
                        "branch_name": handle.branch_name,
                    }),
                    model_content: Some(message),
                    new_messages: Vec::new(),
                    context_modifier: None,
                    is_error: false,
                    mcp_meta: None,
                })
            }
            Err(err) => self.map_error(invocation_id, duration_ms, err).await,
        }
    }

    /// `path` absent: create a NEW worktree, using `name` as the slug (or a
    /// generated random name when `name` is also absent). Refuses when the
    /// session is already inside a worktree (206 `validateInput`'s
    /// `ky()&&!e.path` guard, `errorCode:2`).
    async fn call_create(
        &self,
        invocation_id: &str,
        started_at: Instant,
        name: Option<String>,
    ) -> Result<ToolCallResult, ToolError> {
        if cwd_is_in_worktree(&self.ctx.cwd()) {
            let duration_ms = started_at.elapsed().as_millis() as u64;
            self.emit_failed(invocation_id, "already_in_worktree", duration_ms)
                .await;
            return Err(ToolError::InvalidInput(
                ALREADY_IN_WORKTREE_MESSAGE.to_string(),
            ));
        }

        let slug = name.unwrap_or_else(gen_random_slug);
        self.emit_started(invocation_id, &slug).await;

        // Pre-flight slug validation so we surface the locked literal even if
        // the manager would otherwise return a different error path.
        if let Err(err) = validate_worktree_slug(&slug) {
            let detail = match err {
                WorktreeError::InvalidSlug(d) => d,
                other => format!("{other:?}"),
            };
            self.emit_failed(
                invocation_id,
                "invalid_slug",
                started_at.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "EnterWorktree: invalid slug: {detail}"
            )));
        }

        let result = self.ctx.worktree.create_worktree(&slug, None, &[]).await;
        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(handle) => {
                debug_assert_eq!(
                    handle.branch_name,
                    format!("{WORKTREE_BRANCH_PREFIX}{}", flatten_slug(&slug)),
                    "M2-01 contract broken: branch_name must be worktree-<flatten(slug)>"
                );
                self.ctx
                    .session_cwd
                    .swap(handle.path.clone(), vec![handle.path.clone()]);
                self.emit_completed(invocation_id, &handle.branch_name, duration_ms)
                    .await;
                self.emit_worktree_event(WORKTREE_CREATED, &handle.branch_name)
                    .await;
                let suffix = branch_suffix(&handle.branch_name);
                let display_path = handle.path.to_string_lossy().into_owned();
                let message = format!(
                    "Created worktree at {display_path}{suffix}. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted."
                );
                Ok(ToolCallResult {
                    data: json!({
                        "path": display_path,
                        "branch_name": handle.branch_name,
                    }),
                    model_content: Some(message),
                    new_messages: Vec::new(),
                    context_modifier: None,
                    is_error: false,
                    mcp_meta: None,
                })
            }
            Err(err) => self.map_error(invocation_id, duration_ms, err).await,
        }
    }

    async fn map_error(
        &self,
        invocation_id: &str,
        duration_ms: u64,
        err: WorktreeError,
    ) -> Result<ToolCallResult, ToolError> {
        match err {
            WorktreeError::InvalidSlug(detail) => {
                self.emit_failed(invocation_id, "invalid_slug", duration_ms)
                    .await;
                Err(ToolError::InvalidInput(format!(
                    "EnterWorktree: invalid slug: {detail}"
                )))
            }
            WorktreeError::Unsupported => {
                self.emit_failed(invocation_id, "unsupported", duration_ms)
                    .await;
                Err(ToolError::Internal(
                    "EnterWorktree: worktrees are not supported on this platform".into(),
                ))
            }
            WorktreeError::Git(msg) => {
                self.emit_failed(invocation_id, "git", duration_ms).await;
                Err(ToolError::Internal(format!(
                    "EnterWorktree: git error: {msg}"
                )))
            }
            WorktreeError::Io(msg) => {
                self.emit_failed(invocation_id, "io", duration_ms).await;
                Err(ToolError::Io(format!("EnterWorktree: io error: {msg}")))
            }
        }
    }
}

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        ENTER_TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("create an isolated git worktree and switch into it")
    }
    /// 2.1.206 `userFacingName(e){return e?.path?"Entering worktree":"Creating worktree"}`.
    fn user_facing_name_for_input(&self, input: &Value) -> Option<String> {
        let has_path = input.get("path").and_then(Value::as_str).is_some();
        Some(
            if has_path {
                "Entering worktree"
            } else {
                "Creating worktree"
            }
            .to_string(),
        )
    }
    fn input_schema(&self) -> &Value {
        &ENTER_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    /// 2.1.206 `shouldDefer:!0`.
    fn should_defer(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "EnterWorktree gated by M2-01 WorktreeManager".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    /// 2.1.206 `async description(){return"Creates an isolated worktree (via
    /// git or configured hooks) and switches the session into it"}` (byte-exact).
    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Creates an isolated worktree (via git or configured hooks) and switches the session into it".into()
    }
    /// 2.1.206 `yCd()` tool-use prompt, extracted verbatim from the binary and
    /// rebranded: `.claude/worktrees/` → `.lingxi/worktrees/` (path layout,
    /// [`WORKTREE_PATH_SEGMENT`]), `CLAUDE.md` → `LINGXI.md` (memory-file
    /// convention, `branding::MEMORY_FILE`); `settings.json` is kept verbatim
    /// (the port's settings filename is unchanged, only relocated under
    /// `.lingxi/` — see `tools/meta/src/config.rs::CONFIG_FILE_NAME`).
    async fn prompt(&self, _: &PromptOptions) -> String {
        ENTER_WORKTREE_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = fresh_invocation_id();
        let started_at = Instant::now();

        let parsed: EnterInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                self.emit_failed(
                    &invocation_id,
                    "invalid_input",
                    started_at.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "EnterWorktree: invalid input: {e}"
                )));
            }
        };

        if let Some(path) = parsed.path {
            self.call_enter_existing(&invocation_id, started_at, path)
                .await
        } else {
            self.call_create(&invocation_id, started_at, parsed.name)
                .await
        }
    }
}

/// `ExitWorktreeTool` — removes a worktree created by `EnterWorktreeTool`.
pub struct ExitWorktreeTool {
    ctx: BuiltinToolContext,
}

impl ExitWorktreeTool {
    /// Construct a new tool wired to the shared builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    async fn emit_started(&self, invocation_id: &str, branch_name: &str) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        md.insert("_PROTO_branch_name".into(), pii_tagged(branch_name));
        self.ctx.bus.log_event(EXIT_WORKTREE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(EXIT_WORKTREE_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, error_kind: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert("invocation_id".into(), verified(invocation_id));
        md.insert("tool_name".into(), verified(EXIT_TOOL_NAME));
        md.insert("error_kind".into(), verified(error_kind));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(EXIT_WORKTREE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        EXIT_TOOL_NAME
    }
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("exit a worktree session and return to the original directory")
    }
    fn input_schema(&self) -> &Value {
        &EXIT_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ExitWorktree gated by M2-01 WorktreeManager".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Remove a worktree".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "ExitWorktree removes a worktree by `path` + `branch_name`.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = fresh_invocation_id();
        let started_at = Instant::now();

        let parsed: ExitInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                self.emit_failed(
                    &invocation_id,
                    "invalid_input",
                    started_at.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "ExitWorktree: invalid input: {e}"
                )));
            }
        };
        self.emit_started(&invocation_id, &parsed.branch_name).await;

        let handle = WorktreeHandle {
            path: PathBuf::from(&parsed.path),
            branch_name: parsed.branch_name.clone(),
            // The tool reconstructs the handle from user input (path + branch);
            // it has no captured `originalHeadCommit`, so the ahead-commit count
            // falls to 0 (claude's `if (!headCommit)`). Threading the real
            // baseline here needs the worktree-state side-map (deferred).
            base_commit: None,
        };

        // Capture the worktree's dirty state BEFORE removal — once the
        // worktree directory is gone, `git status` can no longer stat it.
        // `Ok(None)` (git could not be queried) is treated as "unknown": we
        // surface no summary rather than claiming a clean 0/0. Mirrors
        // claude-code's ExitWorktreeTool, which re-counts changes at exit and
        // appends a "Discarded …" note (ExitWorktreeTool.ts:256-318).
        let change_summary = self
            .ctx
            .worktree
            .worktree_change_summary(&handle)
            .await
            .unwrap_or(None);

        let result = self.ctx.worktree.remove_worktree(&handle).await;
        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                self.emit_completed(&invocation_id, duration_ms).await;
                let discard_note = change_summary.map(|s| s.discard_note()).unwrap_or_default();
                let message = format!(
                    "Exited and removed worktree at {}.{discard_note}",
                    handle.path.to_string_lossy()
                );
                let summary_json = change_summary.map(|s| {
                    json!({
                        "changed_files": s.changed_files,
                        "commits": s.commits,
                    })
                });
                Ok(ToolCallResult {
                    data: json!({
                        "removed": true,
                        "branch_name": parsed.branch_name,
                        "change_summary": summary_json,
                        "message": message,
                    }),
                    model_content: None,
                    new_messages: Vec::new(),
                    context_modifier: None,
                    is_error: false,
                    mcp_meta: None,
                })
            }
            Err(WorktreeError::InvalidSlug(detail)) => {
                self.emit_failed(&invocation_id, "invalid_slug", duration_ms)
                    .await;
                Err(ToolError::InvalidInput(format!(
                    "ExitWorktree: invalid slug: {detail}"
                )))
            }
            Err(WorktreeError::Unsupported) => {
                self.emit_failed(&invocation_id, "unsupported", duration_ms)
                    .await;
                Err(ToolError::Internal(
                    "ExitWorktree: worktrees are not supported on this platform".into(),
                ))
            }
            Err(WorktreeError::Git(msg)) => {
                self.emit_failed(&invocation_id, "git", duration_ms).await;
                Err(ToolError::Internal(format!(
                    "ExitWorktree: git error: {msg}"
                )))
            }
            Err(WorktreeError::Io(msg)) => {
                self.emit_failed(&invocation_id, "io", duration_ms).await;
                Err(ToolError::Io(format!("ExitWorktree: io error: {msg}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tool_api::test_support::{
        ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs, MockWorktreeManager,
    };
    use traits::worktree::WorktreeManager;

    fn make_bctx(mock: Arc<MockWorktreeManager>) -> (BuiltinToolContext, Arc<InMemorySink>) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let mut bctx = ctx_for_file_tools(make_dummy_fs(), bus.clone(), vec![std::env::temp_dir()]);
        bctx.worktree = mock as Arc<dyn WorktreeManager>;
        (bctx, sink)
    }

    #[test]
    fn branch_prefix_matches_lock() {
        assert_eq!(WORKTREE_BRANCH_PREFIX, "worktree-");
    }

    #[test]
    fn path_segment_matches_lock() {
        assert_eq!(WORKTREE_PATH_SEGMENT, ".lingxi/worktrees");
    }

    #[test]
    fn flatten_char_is_plus() {
        assert_eq!(WORKTREE_FLATTEN_CHAR, '+');
    }

    #[test]
    fn flatten_slug_uses_plus() {
        assert_eq!(flatten_slug("user/feature"), "user+feature");
        assert_eq!(flatten_slug("a/b/c"), "a+b+c");
        assert_eq!(flatten_slug("plain"), "plain");
    }

    #[test]
    fn validate_accepts_well_formed() {
        assert!(validate_worktree_slug("feature-x").is_ok());
        assert!(validate_worktree_slug("user/feature").is_ok());
        assert!(validate_worktree_slug("team/area/widget").is_ok());
    }

    #[test]
    fn validate_rejects_spaces() {
        assert!(validate_worktree_slug("bad slug").is_err());
    }

    #[test]
    fn validate_rejects_plus_sign() {
        // The flatten output uses `+`; allowing `+` in inputs would break
        // the injectivity guarantee.
        assert!(validate_worktree_slug("a+b").is_err());
    }

    #[test]
    fn validate_rejects_empty_segment() {
        assert!(validate_worktree_slug("foo/").is_err());
        assert!(validate_worktree_slug("/foo").is_err());
        assert!(validate_worktree_slug("a//b").is_err());
    }

    #[test]
    fn validate_rejects_overlong_slug() {
        let huge = "a".repeat(MAX_WORKTREE_SLUG_LENGTH + 1);
        assert!(validate_worktree_slug(&huge).is_err());
    }

    #[test]
    fn full_branch_name_layout() {
        let slug = "user/feature";
        let full = format!("{}{}", WORKTREE_BRANCH_PREFIX, flatten_slug(slug));
        assert_eq!(full, "worktree-user+feature");
    }

    #[tokio::test]
    async fn enter_creates_worktree_with_locked_layout() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-A"));
        let (bctx, sink) = make_bctx(mock.clone());
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterWorktreeTool::new(bctx);
        let res = tool
            .call(json!({ "name": "user/feature" }), fresh_ctx(), fresh_tx())
            .await
            .expect("create must succeed");
        assert_eq!(res.data["branch_name"], "worktree-user+feature");
        let path = res.data["path"].as_str().unwrap();
        assert!(
            path.ends_with("/tmp/repo-A/.lingxi/worktrees/user+feature"),
            "path layout off: {path}"
        );
        assert_eq!(mock.created().len(), 1);
        assert_eq!(mock.created()[0].0, "user/feature");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_STARTED.to_string()));
        assert!(names.contains(&ENTER_WORKTREE_COMPLETED.to_string()));
        assert!(names.contains(&WORKTREE_CREATED.to_string()));
    }

    #[tokio::test]
    async fn enter_rejects_slug_with_space() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, sink) = make_bctx(mock);
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterWorktreeTool::new(bctx);
        let err = tool
            .call(json!({ "name": "bad slug" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("space must reject");
        let msg = format!("{err}");
        assert!(msg.contains("EnterWorktree: invalid slug:"), "msg: {msg}");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn enter_maps_git_error_to_internal() {
        let mock = Arc::new(MockWorktreeManager::new());
        mock.script_create_error(WorktreeError::Git("fatal: not a git repository".into()));
        let (bctx, sink) = make_bctx(mock);
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterWorktreeTool::new(bctx);
        let err = tool
            .call(json!({ "name": "ok-slug" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("git error must surface");
        let msg = format!("{err}");
        assert!(msg.contains("EnterWorktree: git error:"), "msg: {msg}");
        assert!(msg.contains("fatal: not a git repository"), "msg: {msg}");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn enter_maps_unsupported_to_internal() {
        let mock = Arc::new(MockWorktreeManager::new());
        mock.script_create_error(WorktreeError::Unsupported);
        let (bctx, sink) = make_bctx(mock);
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterWorktreeTool::new(bctx);
        let err = tool
            .call(json!({ "name": "ok-slug" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("unsupported must surface");
        assert_eq!(
            format!("{err}"),
            "internal: EnterWorktree: worktrees are not supported on this platform"
        );
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn enter_does_not_retry_internally() {
        let mock = Arc::new(MockWorktreeManager::new());
        mock.script_create_error(WorktreeError::Git("transient".into()));
        let (bctx, _sink) = make_bctx(mock.clone());
        let tool = EnterWorktreeTool::new(bctx);
        let _ = tool
            .call(json!({ "name": "ok-slug" }), fresh_ctx(), fresh_tx())
            .await;
        // Only the scripted error fires; created list stays empty (no retry).
        assert_eq!(mock.created().len(), 0);
    }

    // ===== 206 golden-message + session-cwd-swap tests (Task 7) ===============

    #[tokio::test]
    async fn create_message_exact_with_branch_suffix() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-create"));
        let (bctx, _sink) = make_bctx(mock);
        let tool = EnterWorktreeTool::new(bctx);
        let res = tool
            .call(json!({ "name": "feat" }), fresh_ctx(), fresh_tx())
            .await
            .expect("create must succeed");
        let path = res.data["path"].as_str().unwrap().to_string();
        assert_eq!(
            res.model_content.as_deref(),
            Some(
                format!(
                    "Created worktree at {path} on branch worktree-feat. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted."
                )
                .as_str()
            )
        );
    }

    #[tokio::test]
    async fn create_message_exact_without_branch_suffix() {
        // The mock always returns a real branch name, so exercise the
        // no-suffix path directly through `branch_suffix` — the golden text
        // around the `{branch}` slot is asserted with a synthesized empty
        // branch to prove the "" case renders with no " on branch" fragment
        // and no double space before the period.
        assert_eq!(branch_suffix(""), "");
        let path = "/tmp/repo-create/.lingxi/worktrees/feat";
        let message = format!(
            "Created worktree at {path}{}. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted.",
            branch_suffix("")
        );
        assert_eq!(
            message,
            "Created worktree at /tmp/repo-create/.lingxi/worktrees/feat. The session is now working in the worktree. Use ExitWorktree to leave mid-session, or exit the session to be prompted."
        );
    }

    #[tokio::test]
    async fn enter_existing_message_exact() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-enter"));
        let (bctx, _sink) = make_bctx(mock);
        let tool = EnterWorktreeTool::new(bctx);
        let target = "/tmp/repo-enter/.lingxi/worktrees/feat";
        let res = tool
            .call(json!({ "path": target }), fresh_ctx(), fresh_tx())
            .await
            .expect("enter existing must succeed");
        assert_eq!(
            res.model_content.as_deref(),
            Some(
                "Entered worktree at /tmp/repo-enter/.lingxi/worktrees/feat on branch worktree-feat. This agent's working directory and write access now point at the worktree; the previous directory was left untouched."
            )
        );
    }

    #[tokio::test]
    async fn enter_existing_detached_head_has_no_branch_suffix() {
        // `enter_existing` returning `branch_name == "HEAD"` (detached) must
        // NOT render " on branch HEAD" — treated as no branch.
        assert_eq!(branch_suffix("HEAD"), "");

        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-detached"));
        let target = PathBuf::from("/tmp/repo-detached/.lingxi/worktrees/feat");
        // Script the EXACT handle `enter_existing` returns so its
        // `branch_name` is the literal `"HEAD"` a real posix manager would
        // report for a detached-HEAD worktree (git prints "HEAD" for
        // `--abbrev-ref HEAD` when detached).
        mock.script_enter_existing_handle(WorktreeHandle {
            path: target.clone(),
            branch_name: "HEAD".to_string(),
            base_commit: None,
        });
        let (bctx, _sink) = make_bctx(mock);
        let tool = EnterWorktreeTool::new(bctx);
        let res = tool
            .call(
                json!({ "path": target.to_string_lossy() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("enter existing must succeed");
        let msg = res.model_content.as_deref().unwrap();
        assert!(!msg.contains("on branch HEAD"), "msg: {msg}");
        assert_eq!(
            msg,
            "Entered worktree at /tmp/repo-detached/.lingxi/worktrees/feat. This agent's working directory and write access now point at the worktree; the previous directory was left untouched."
        );
    }

    #[tokio::test]
    async fn already_in_worktree_guard_rejects_with_exact_message_and_no_swap() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-guard"));
        let (bctx, sink) = make_bctx(mock.clone());
        bctx.bus.attach_sink(sink.clone()).await;
        // Pin the session cwd INSIDE a `.lingxi/worktrees/<slug>` directory —
        // the port's stand-in for "already in a worktree session".
        let worktree_cwd = PathBuf::from("/tmp/repo-guard/.lingxi/worktrees/already-here");
        bctx.session_cwd
            .swap(worktree_cwd.clone(), vec![worktree_cwd.clone()]);
        let tool = EnterWorktreeTool::new(bctx);
        let err = tool
            .call(json!({}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("create while already in a worktree must reject");
        assert_eq!(
            format!("{err}"),
            format!("invalid input: {ALREADY_IN_WORKTREE_MESSAGE}")
        );
        // No worktree was created and the cwd was NOT swapped again.
        assert_eq!(mock.created().len(), 0);
        assert_eq!(tool.ctx.cwd(), worktree_cwd, "guard must not swap cwd");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_FAILED.to_string()));
        assert!(!names.contains(&WORKTREE_CREATED.to_string()));
    }

    #[tokio::test]
    async fn entering_existing_worktree_is_allowed_even_when_already_in_one() {
        // The guard only fires when CREATING (no `path`); switching via
        // `path` is allowed even from inside another worktree.
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-switch"));
        let (bctx, _sink) = make_bctx(mock);
        let worktree_cwd = PathBuf::from("/tmp/repo-switch/.lingxi/worktrees/first");
        bctx.session_cwd
            .swap(worktree_cwd.clone(), vec![worktree_cwd]);
        let tool = EnterWorktreeTool::new(bctx);
        let target = "/tmp/repo-switch/.lingxi/worktrees/second";
        let res = tool
            .call(json!({ "path": target }), fresh_ctx(), fresh_tx())
            .await
            .expect("switching via path must succeed even mid-worktree");
        assert_eq!(res.data["path"], target);
    }

    #[tokio::test]
    async fn successful_create_swaps_session_cwd_to_worktree_path() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-swap"));
        let (bctx, _sink) = make_bctx(mock);
        let boot_cwd = bctx.cwd();
        let tool = EnterWorktreeTool::new(bctx);
        let res = tool
            .call(json!({ "name": "swaptest" }), fresh_ctx(), fresh_tx())
            .await
            .expect("create must succeed");
        let path = PathBuf::from(res.data["path"].as_str().unwrap());
        assert_eq!(tool.ctx.cwd(), path, "session cwd must swap to worktree");
        assert_ne!(tool.ctx.cwd(), boot_cwd);
    }

    #[tokio::test]
    async fn error_path_does_not_swap_session_cwd() {
        let mock = Arc::new(MockWorktreeManager::new());
        mock.script_create_error(WorktreeError::Git("boom".into()));
        let (bctx, _sink) = make_bctx(mock);
        let boot_cwd = bctx.cwd();
        let tool = EnterWorktreeTool::new(bctx);
        let _ = tool
            .call(json!({ "name": "ok-slug" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("scripted git error must surface");
        assert_eq!(tool.ctx.cwd(), boot_cwd, "an error path must not swap cwd");
    }

    #[tokio::test]
    async fn invalid_input_error_path_does_not_swap_session_cwd() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, _sink) = make_bctx(mock);
        let boot_cwd = bctx.cwd();
        let tool = EnterWorktreeTool::new(bctx);
        let _ = tool
            .call(json!({ "name": "bad slug" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("invalid slug must reject");
        assert_eq!(tool.ctx.cwd(), boot_cwd, "invalid input must not swap cwd");
    }

    #[test]
    fn description_and_prompt_texts_are_206_byte_exact() {
        assert_eq!(
            ENTER_WORKTREE_PROMPT.contains(".lingxi/worktrees/"),
            true,
            "prompt must use the rebranded path segment"
        );
        assert!(!ENTER_WORKTREE_PROMPT.contains(".claude/worktrees/"));
        assert!(!ENTER_WORKTREE_PROMPT.contains("CLAUDE.md"));
        assert!(ENTER_WORKTREE_PROMPT.contains("LINGXI.md"));
        assert!(ENTER_WORKTREE_PROMPT.contains("settings.json"));
    }

    #[test]
    fn user_facing_name_depends_on_path_presence() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, _sink) = make_bctx(mock);
        let tool = EnterWorktreeTool::new(bctx);
        assert_eq!(
            tool.user_facing_name_for_input(&json!({})).as_deref(),
            Some("Creating worktree")
        );
        assert_eq!(
            tool.user_facing_name_for_input(&json!({ "path": "/x" }))
                .as_deref(),
            Some("Entering worktree")
        );
    }

    #[test]
    fn should_defer_is_true() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, _sink) = make_bctx(mock);
        let tool = EnterWorktreeTool::new(bctx);
        assert!(tool.should_defer());
    }

    #[tokio::test]
    async fn exit_removes_worktree() {
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-B"));
        // Pre-create one worktree.
        let _ = mock
            .create_worktree("user/feature", None, &[])
            .await
            .expect("pre-create");
        let (bctx, sink) = make_bctx(mock.clone());
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = ExitWorktreeTool::new(bctx);
        let res = tool
            .call(
                json!({
                    "path": "/tmp/repo-B/.lingxi/worktrees/user+feature",
                    "branch_name": "worktree-user+feature"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("remove must succeed");
        assert_eq!(res.data["removed"], true);
        assert_eq!(res.data["branch_name"], "worktree-user+feature");
        assert_eq!(mock.removed().len(), 1);
        assert_eq!(mock.removed()[0].branch_name, "worktree-user+feature");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_WORKTREE_STARTED.to_string()));
        assert!(names.contains(&EXIT_WORKTREE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn exit_surfaces_dirty_change_summary() {
        use traits::worktree::WorktreeChangeSummary;
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-C"));
        let _ = mock
            .create_worktree("user/feature", None, &[])
            .await
            .expect("pre-create");
        // Inject a deterministic dirty state — no real git repo involved.
        mock.script_change_summary(Some(WorktreeChangeSummary {
            changed_files: 3,
            commits: 2,
        }));
        let (bctx, sink) = make_bctx(mock.clone());
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = ExitWorktreeTool::new(bctx);
        let res = tool
            .call(
                json!({
                    "path": "/tmp/repo-C/.lingxi/worktrees/user+feature",
                    "branch_name": "worktree-user+feature"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("remove must succeed");
        assert_eq!(res.data["removed"], true);
        // The structured summary is surfaced.
        assert_eq!(res.data["change_summary"]["changed_files"], 3);
        assert_eq!(res.data["change_summary"]["commits"], 2);
        // The message carries the byte-faithful discard note (commits first).
        let msg = res.data["message"].as_str().unwrap();
        assert_eq!(
            msg,
            "Exited and removed worktree at /tmp/repo-C/.lingxi/worktrees/user+feature. \
             Discarded 2 commits and 3 uncommitted files."
        );
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_WORKTREE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn exit_clean_summary_has_no_discard_note() {
        use traits::worktree::WorktreeChangeSummary;
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-D"));
        let _ = mock
            .create_worktree("feat", None, &[])
            .await
            .expect("pre-create");
        mock.script_change_summary(Some(WorktreeChangeSummary {
            changed_files: 0,
            commits: 0,
        }));
        let (bctx, sink) = make_bctx(mock.clone());
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = ExitWorktreeTool::new(bctx);
        let res = tool
            .call(
                json!({
                    "path": "/tmp/repo-D/.lingxi/worktrees/feat",
                    "branch_name": "worktree-feat"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("remove must succeed");
        assert_eq!(res.data["change_summary"]["changed_files"], 0);
        let msg = res.data["message"].as_str().unwrap();
        assert_eq!(
            msg,
            "Exited and removed worktree at /tmp/repo-D/.lingxi/worktrees/feat."
        );
        assert!(!msg.contains("Discarded"), "clean exit has no discard note");
    }

    #[tokio::test]
    async fn exit_unknown_summary_is_null_no_discard_note() {
        // No scripted summary → mock returns Ok(None) ("unknown"). The tool
        // must surface a null change_summary and NOT claim a clean discard.
        let mock = Arc::new(MockWorktreeManager::with_root("/tmp/repo-E"));
        let _ = mock
            .create_worktree("feat", None, &[])
            .await
            .expect("pre-create");
        let (bctx, _sink) = make_bctx(mock.clone());
        let tool = ExitWorktreeTool::new(bctx);
        let res = tool
            .call(
                json!({
                    "path": "/tmp/repo-E/.lingxi/worktrees/feat",
                    "branch_name": "worktree-feat"
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("remove must succeed");
        assert!(
            res.data["change_summary"].is_null(),
            "unknown state surfaces as null, not 0/0"
        );
        let msg = res.data["message"].as_str().unwrap();
        assert!(
            !msg.contains("Discarded"),
            "unknown state adds no discard note"
        );
    }

    #[tokio::test]
    async fn exit_rejects_missing_path_or_branch() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, sink) = make_bctx(mock);
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = ExitWorktreeTool::new(bctx);
        let err = tool
            .call(json!({ "path": "/x" }), fresh_ctx(), fresh_tx())
            .await
            .expect_err("missing branch_name must reject");
        let msg = format!("{err}");
        assert!(msg.contains("ExitWorktree: invalid input"), "msg: {msg}");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&EXIT_WORKTREE_FAILED.to_string()));
    }
}
