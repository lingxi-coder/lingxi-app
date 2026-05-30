//! `EnterWorktreeTool` + `ExitWorktreeTool` — manage disposable git worktrees
//! via the M2-01 [`WorktreeManager`] trait. Locks (spec §7 lines 488-489):
//! - Branch prefix: `worktree-<flatten(slug)>`
//! - Path layout:   `<repo>/.claude/worktrees/<flatten(slug)>`
//! - Slug flatten:  `'/' -> '+'` (injective; `'+'` outside allowed charset)
//!
//! The slug-validation and flatten helpers are reimplemented locally in this
//! module to match the M2-01 contract byte-for-byte, avoiding a dependency
//! cycle (`lingxi-tools -> lingxi-platform-posix -> lingxi-lsp ->
//! lingxi-tools`). The `parity_workflow_tools` driver cross-checks the
//! locked literals.

use std::path::PathBuf;
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
    EXIT_WORKTREE_COMPLETED, EXIT_WORKTREE_FAILED, EXIT_WORKTREE_STARTED,
};
use traits::worktree::{WorktreeError, WorktreeHandle};

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

/// Byte-locked git branch prefix for disposable worktrees (spec §7 line 488,
/// M2-01 lock). The full branch is `worktree-<flatten(slug)>`.
pub const WORKTREE_BRANCH_PREFIX: &str = "worktree-";
/// Byte-locked path segment under the repo root (spec §7 line 489, M2-01 lock).
pub const WORKTREE_PATH_SEGMENT: &str = ".claude/worktrees";
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
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug("slug is empty".into()));
    }
    if slug.len() > MAX_WORKTREE_SLUG_LENGTH {
        return Err(WorktreeError::InvalidSlug(format!(
            "slug exceeds {MAX_WORKTREE_SLUG_LENGTH} chars (got {})",
            slug.len()
        )));
    }
    for segment in slug.split('/') {
        if segment.is_empty() {
            return Err(WorktreeError::InvalidSlug(format!(
                "slug contains empty segment: {slug:?}"
            )));
        }
        for ch in segment.chars() {
            let allowed = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
            if !allowed {
                return Err(WorktreeError::InvalidSlug(format!(
                    "slug contains invalid character {ch:?} in segment {segment:?}"
                )));
            }
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

#[derive(Debug, Deserialize)]
struct EnterInput {
    slug: String,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    copy_includes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExitInput {
    path: String,
    branch_name: String,
}

static ENTER_INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "slug":          { "type": "string", "minLength": 1, "maxLength": 64 },
            "base_branch":   { "type": "string" },
            "copy_includes": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["slug"]
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
}

#[async_trait]
impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        ENTER_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &ENTER_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Create a disposable git worktree".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "EnterWorktree creates `<repo>/.claude/worktrees/<flatten(slug)>` on branch \
         `worktree-<flatten(slug)>` via the configured worktree manager."
            .into()
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
        self.emit_started(&invocation_id, &parsed.slug).await;

        // Pre-flight slug validation so we surface the locked literal even if
        // the manager would otherwise return a different error path.
        if let Err(err) = validate_worktree_slug(&parsed.slug) {
            let detail = match err {
                WorktreeError::InvalidSlug(d) => d,
                other => format!("{other:?}"),
            };
            self.emit_failed(
                &invocation_id,
                "invalid_slug",
                started_at.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "EnterWorktree: invalid slug: {detail}"
            )));
        }

        let copy_includes: Vec<PathBuf> = parsed.copy_includes.iter().map(PathBuf::from).collect();
        let result = self
            .ctx
            .worktree
            .create_worktree(&parsed.slug, parsed.base_branch.as_deref(), &copy_includes)
            .await;
        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(handle) => {
                debug_assert_eq!(
                    handle.branch_name,
                    format!("{WORKTREE_BRANCH_PREFIX}{}", flatten_slug(&parsed.slug)),
                    "M2-01 contract broken: branch_name must be worktree-<flatten(slug)>"
                );
                self.emit_completed(&invocation_id, &handle.branch_name, duration_ms)
                    .await;
                Ok(ToolCallResult {
                    data: json!({
                        "path": handle.path.to_string_lossy(),
                        "branch_name": handle.branch_name,
                    }),
                    new_messages: Vec::new(),
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(WorktreeError::InvalidSlug(detail)) => {
                self.emit_failed(&invocation_id, "invalid_slug", duration_ms)
                    .await;
                Err(ToolError::InvalidInput(format!(
                    "EnterWorktree: invalid slug: {detail}"
                )))
            }
            Err(WorktreeError::Unsupported) => {
                self.emit_failed(&invocation_id, "unsupported", duration_ms)
                    .await;
                Err(ToolError::Internal(
                    "EnterWorktree: worktrees are not supported on this platform".into(),
                ))
            }
            Err(WorktreeError::Git(msg)) => {
                self.emit_failed(&invocation_id, "git", duration_ms).await;
                Err(ToolError::Internal(format!(
                    "EnterWorktree: git error: {msg}"
                )))
            }
            Err(WorktreeError::Io(msg)) => {
                self.emit_failed(&invocation_id, "io", duration_ms).await;
                Err(ToolError::Io(format!("EnterWorktree: io error: {msg}")))
            }
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
    fn input_schema(&self) -> &Value {
        &EXIT_INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        };
        let result = self.ctx.worktree.remove_worktree(&handle).await;
        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                self.emit_completed(&invocation_id, duration_ms).await;
                Ok(ToolCallResult {
                    data: json!({
                        "removed": true,
                        "branch_name": parsed.branch_name,
                    }),
                    new_messages: Vec::new(),
                    context_modifier: None,
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
    use crate::builtin::test_support::{
        ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs, MockWorktreeManager,
    };
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
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
        assert_eq!(WORKTREE_PATH_SEGMENT, ".claude/worktrees");
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
            .call(json!({ "slug": "user/feature" }), fresh_ctx(), fresh_tx())
            .await
            .expect("create must succeed");
        assert_eq!(res.data["branch_name"], "worktree-user+feature");
        let path = res.data["path"].as_str().unwrap();
        assert!(
            path.ends_with("/tmp/repo-A/.claude/worktrees/user+feature"),
            "path layout off: {path}"
        );
        assert_eq!(mock.created().len(), 1);
        assert_eq!(mock.created()[0].0, "user/feature");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&ENTER_WORKTREE_STARTED.to_string()));
        assert!(names.contains(&ENTER_WORKTREE_COMPLETED.to_string()));
    }

    #[tokio::test]
    async fn enter_rejects_slug_with_space() {
        let mock = Arc::new(MockWorktreeManager::new());
        let (bctx, sink) = make_bctx(mock);
        bctx.bus.attach_sink(sink.clone()).await;
        let tool = EnterWorktreeTool::new(bctx);
        let err = tool
            .call(json!({ "slug": "bad slug" }), fresh_ctx(), fresh_tx())
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
            .call(json!({ "slug": "ok-slug" }), fresh_ctx(), fresh_tx())
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
            .call(json!({ "slug": "ok-slug" }), fresh_ctx(), fresh_tx())
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
            .call(json!({ "slug": "ok-slug" }), fresh_ctx(), fresh_tx())
            .await;
        // Only the scripted error fires; created list stays empty (no retry).
        assert_eq!(mock.created().len(), 0);
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
                    "path": "/tmp/repo-B/.claude/worktrees/user+feature",
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
