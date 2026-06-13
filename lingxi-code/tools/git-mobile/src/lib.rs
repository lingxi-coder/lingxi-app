//! Mobile-only structured `Git` tool crate (Android, spec G1–G8).
//!
//! `GitTool` is the model-facing git tool on Android, backed by libgit2 (the
//! `git2` crate) running **in-process** — there is no `git` binary, no exec, no
//! sandbox/minijail involvement. Operations are a fixed enum (clone / fetch /
//! pull / status / diff / log / show / `branch_list` / checkout / add / commit
//! / `branch_create` / merge),
//! NOT a free-form shell string. v1 is read + local-write only: there is **no
//! push**, merge/pull are **fast-forward-only**, and remotes are **HTTPS-only**
//! (token supplied in-memory by the Kotlin host via the libgit2 credential
//! callback — never to disk or a child-process env).
//!
//! The tool is registered ONLY when the device + config gate passes
//! (`ctx.android_git.enabled`); the gate itself is computed in `android-aar`
//! (enable flag + workspace-ready + CA-store-reachable) and threaded through
//! `MobileConfig` -> `BuiltinToolContext.android_git`. On desktop / iOS the
//! `android_git` carrier is `None`, so the tool is absent (not erroring).
//!
//! `git2` is a safe wrapper; the only C is `libgit2-sys` at build time, so the
//! crate stays `#![forbid(unsafe_code)]`.

#![forbid(unsafe_code)]

pub mod ops;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH;
use tool_api::BuiltinToolContext;

/// Tool name byte-lock — the model-facing name for the mobile Git tool.
pub const TOOL_NAME: &str = "Git";

/// The fixed set of supported operations (schema `operation` enum). The tool is
/// structured: the model picks one of these, NOT a free-form command.
const OPERATIONS: &[&str] = &[
    "clone",
    "fetch",
    "pull",
    "status",
    "diff",
    "log",
    "show",
    "branch_list",
    "checkout",
    "add",
    "commit",
    "branch_create",
    "merge",
];

/// `GitTool` — run a structured, in-process git operation via libgit2.
#[derive(Clone)]
pub struct GitTool {
    ctx: BuiltinToolContext,
}

impl GitTool {
    /// Construct from the builtin tool context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "operation":   { "type": "string", "enum": OPERATIONS, "description": "The git operation to run." },
            "repo_url":    { "type": "string", "description": "Remote HTTPS URL (clone)." },
            "remote":      { "type": "string", "description": "Remote name (fetch/pull); defaults to origin." },
            "branch":      { "type": "string", "description": "Branch name (checkout/branch_create/merge target)." },
            "refspec":     { "type": "string", "description": "Refspec for fetch." },
            "paths":       { "type": "array", "items": { "type": "string" }, "description": "Paths to stage (add) or limit diff/status to." },
            "message":     { "type": "string", "description": "Commit message (commit)." },
            "rev":         { "type": "string", "description": "A single revision/oid (show/checkout)." },
            "rev_range":   { "type": "string", "description": "A revision range (log/diff)." },
            "new_branch":  { "type": "string", "description": "Name of the branch to create (branch_create)." },
            "description": { "type": "string", "description": "Optional short description of what this operation does." }
        },
        "required": ["operation"]
    })
});

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        // Defensive double-gate: registration (`register_all`) already filters
        // on this same flag, but keep the tool inert if it ever lands in a
        // registry without the gate set.
        self.ctx.android_git.as_ref().is_some_and(|g| g.enabled)
    }

    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Conservative: a git operation can mutate the index / worktree / refs,
        // so never run it concurrently with other tools.
        false
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // Git mutates (add/commit/checkout/merge/clone/fetch/pull); even the
        // read ops share a tool that is not read-only overall.
        false
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // Mirrors the desktop/shell-mobile stub: real allow/ask gating lives in
        // the engine's `AdapterPermissionGate` (wired on mobile via
        // `PermissionRequestSink` -> Kotlin UI), NOT in tool-level rule code.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "android-git (engine AdapterPermissionGate handles allow/ask)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _opts: &DescriptionOptions) -> String {
        match input.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => "Run a structured git operation".into(),
        }
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        let has_token = self.ctx.android_git.as_ref().is_some_and(|g| g.has_token);

        let mut prompt = String::new();
        prompt.push_str(
            "Run a STRUCTURED git operation on this Android device. This is NOT a \
             shell: pick one `operation` from a fixed set (it is an enum, not a \
             free-form command line) and supply the relevant typed parameters \
             (repo_url, remote, branch, refspec, paths, message, rev, rev_range, \
             new_branch).\n\n",
        );
        prompt.push_str(&format!(
            "Supported operations: {}.\n\n",
            OPERATIONS.join(", ")
        ));
        prompt.push_str(
            "v1 is READ + LOCAL-WRITE only: there is NO push. merge and pull are \
             FAST-FORWARD-ONLY (a non-fast-forward is reported as a named error, \
             never left as conflict markers). Remotes are HTTPS-ONLY (git@/ssh \
             URLs are rejected).\n\n",
        );
        if has_token {
            prompt.push_str(
                "Network operations (clone/fetch/pull) use the host-supplied HTTPS \
                 credentials.\n",
            );
        } else {
            prompt.push_str(
                "No git credentials are configured: network operations \
                 (clone/fetch/pull) are unavailable until HTTPS credentials are \
                 provided by the host. Local operations (status/diff/log/show/\
                 branch_list/checkout/add/commit/branch_create/merge) still work.\n",
            );
        }
        prompt
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let op = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("missing `operation`".into()))?;
        if op.is_empty() {
            return Err(ValidationError("`operation` must not be empty".into()));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // Extract + validate the operation.
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing operation".into()))?;
        if operation.is_empty() {
            return Err(ToolError::InvalidInput(
                "operation must not be empty".into(),
            ));
        }

        // T3 dispatch stub: every operation is wired in Tasks 4-6/8 to a real
        // `ops::` call. Until then, return a named (non-panicking) error so the
        // tool registers and answers without `todo!`/`unimplemented!`.
        match operation {
            "clone" | "fetch" | "pull" | "status" | "diff" | "log" | "show" | "branch_list"
            | "checkout" | "add" | "commit" | "branch_create" | "merge" => Err(
                ToolError::InvalidInput(format!("git operation '{operation}' not yet implemented")),
            ),
            other => Err(ToolError::InvalidInput(format!(
                "unknown git operation '{other}'"
            ))),
        }
    }
}

/// Register the mobile `Git` tool against `reg` — ONLY when the gate passes.
///
/// The gate is `ctx.android_git.enabled` (`enable_git` + workspace-ready +
/// CA-store-reachable, computed in `android-aar`). When the gate is unmet the
/// tool is simply not registered — **absent, not erroring** (spec invariant).
/// On desktop / iOS the `android_git` field is `None`, so this is a no-op.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    if ctx.android_git.as_ref().is_some_and(|g| g.enabled) {
        reg.register_builtin(Arc::new(GitTool::new(ctx)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::shell_test_ctx;
    use tool_api::{AndroidGitToolCtx, ToolRegistry};
    use traits::process::ProcessOutput;

    fn ok_output() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Build a ctx with `android_git` enabled and a token, anchored at a fresh
    /// tempdir workspace root.
    fn test_ctx_git_enabled() -> BuiltinToolContext {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = shell_test_ctx(ok_output());
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: true,
            workspace_root: dir.path().to_string_lossy().into_owned(),
        });
        // Keep the tempdir alive for the duration of the process — the tests
        // only read the path string, never the directory, so leaking it is
        // fine and avoids a premature cleanup.
        std::mem::forget(dir);
        ctx
    }

    /// Same as `test_ctx_git_enabled` but with `has_token: false`.
    fn test_ctx_git_no_token() -> BuiltinToolContext {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ctx = shell_test_ctx(ok_output());
        ctx.android_git = Some(AndroidGitToolCtx {
            enabled: true,
            has_token: false,
            workspace_root: dir.path().to_string_lossy().into_owned(),
        });
        std::mem::forget(dir);
        ctx
    }

    #[test]
    fn name_is_git_and_schema_has_operation() {
        let t = GitTool::new(test_ctx_git_enabled());
        assert_eq!(t.name(), "Git");
        let schema = t.input_schema();
        let props = &schema["properties"];
        assert!(props.get("operation").is_some(), "operation param required");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "operation"));
    }

    #[test]
    fn is_enabled_follows_android_git_flag() {
        let static_ctx = ToolStaticContext::default();

        let enabled = GitTool::new(test_ctx_git_enabled());
        assert!(enabled.is_enabled(&static_ctx));

        // android_git: None -> disabled.
        let mut none_ctx = shell_test_ctx(ok_output());
        none_ctx.android_git = None;
        let none = GitTool::new(none_ctx);
        assert!(!none.is_enabled(&static_ctx));

        // Some(enabled: false) -> disabled.
        let mut off_ctx = shell_test_ctx(ok_output());
        off_ctx.android_git = Some(AndroidGitToolCtx {
            enabled: false,
            has_token: true,
            workspace_root: "/tmp".into(),
        });
        let off = GitTool::new(off_ctx);
        assert!(!off.is_enabled(&static_ctx));
    }

    #[tokio::test]
    async fn prompt_declares_structured_git_no_push() {
        let opts = PromptOptions {
            include_examples: false,
        };

        let with_token = GitTool::new(test_ctx_git_enabled());
        let prompt = with_token.prompt(&opts).await;
        assert!(
            prompt.contains("git"),
            "prompt should mention git: {prompt}"
        );
        assert!(
            prompt.contains("push"),
            "prompt should state the no-push rule: {prompt}"
        );
        assert!(
            prompt.contains("status"),
            "prompt should list an operation name: {prompt}"
        );

        // has_token == false -> prompt mentions credentials.
        let no_token = GitTool::new(test_ctx_git_no_token());
        let prompt = no_token.prompt(&opts).await;
        assert!(
            prompt.contains("credentials"),
            "no-token prompt should mention credentials: {prompt}"
        );
    }

    #[test]
    fn register_all_gates_on_enabled() {
        // Enabled -> registry contains "Git".
        let mut reg = ToolRegistry::new();
        register_all(&mut reg, test_ctx_git_enabled());
        assert!(
            reg.find_by_name("Git").is_some(),
            "enabled gate should register Git"
        );

        // android_git: None -> Git absent (not erroring).
        let mut reg = ToolRegistry::new();
        let mut none_ctx = shell_test_ctx(ok_output());
        none_ctx.android_git = None;
        register_all(&mut reg, none_ctx);
        assert!(
            reg.find_by_name("Git").is_none(),
            "absent gate should NOT register Git"
        );
    }
}
