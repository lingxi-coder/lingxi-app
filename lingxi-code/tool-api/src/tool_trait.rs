//! `Tool` trait — the contract every tool implements.
//!
//! Static methods reason about the tool itself (name, schemas, capabilities);
//! dynamic methods reason about a specific `(input, ctx)` pair (permission,
//! concurrency safety, validation, execution).

use async_trait::async_trait;
use permission::PermissionResult;
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;

/// Contract implemented by every tool the agent can call.
///
/// A `Tool` exposes static metadata (name, input/output schema, enablement,
/// concurrency hints) and dynamic behavior (permission check, validation,
/// execution). The dispatcher consults the static side to decide scheduling
/// and the dynamic side to actually run a call.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Canonical tool name (e.g. `"Read"`, `"Bash"`).
    fn name(&self) -> &str;

    /// Alternate names the dispatcher should accept for this tool.
    ///
    /// Defaults to no aliases.
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// Short hint shown in tool-search UIs, if any.
    fn search_hint(&self) -> Option<&str> {
        None
    }

    /// Human-readable label shown to the user for this tool (claude-code
    /// `userFacingName()`), distinct from the canonical wire [`name`](Self::name).
    /// `None` means "use the canonical name" — claude-code falls back to the tool
    /// name when no override is supplied.
    fn user_facing_name(&self) -> Option<&str> {
        None
    }

    /// JSON Schema for the tool's input parameters.
    fn input_schema(&self) -> &Value;

    /// Optional JSON Schema for the tool's output.
    fn output_schema(&self) -> Option<&Value> {
        None
    }

    /// Whether this tool is enabled for the given static context
    /// (feature flags, environment, etc.).
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool;

    /// Whether this tool is sourced from an MCP server.
    fn is_mcp(&self) -> bool {
        false
    }

    /// Whether this tool is sourced from an LSP server.
    fn is_lsp(&self) -> bool {
        false
    }

    /// Whether this tool may require user interaction mid-call
    /// (e.g. an interactive prompt).
    fn requires_user_interaction(&self) -> bool {
        false
    }

    /// Whether the dispatcher should defer this tool until later in the
    /// schedule (e.g. heavyweight tools queued behind quick ones).
    fn should_defer(&self) -> bool {
        false
    }

    /// Whether this tool must always be loaded in the prompt, even when the
    /// loader would otherwise prune it.
    fn always_load(&self) -> bool {
        false
    }

    /// Whether strict JSON-schema validation is enforced on the input.
    fn strict(&self) -> bool {
        false
    }

    /// Maximum size (in characters) of the tool's serialized result before
    /// truncation kicks in.
    fn max_result_size_chars(&self) -> usize;

    /// Whether the tool is safe to run concurrently with other tools given
    /// this specific input. Read-only tools typically return `true`.
    fn is_concurrency_safe(&self, input: &Value) -> bool;

    /// Whether this invocation is read-only with respect to the workspace.
    fn is_read_only(&self, input: &Value) -> bool;

    /// Whether this invocation can destroy data irreversibly.
    fn is_destructive(&self, _input: &Value) -> bool {
        false
    }

    /// Whether the tool reaches outside the local workspace (network, etc.).
    fn is_open_world(&self, _input: &Value) -> bool {
        false
    }

    /// Classify this invocation as search / read / list, if applicable.
    /// Used by the loader to fold "Search&Read" tools into one slot.
    fn is_search_or_read(&self, _input: &Value) -> Option<SearchReadInfo> {
        None
    }

    /// How the tool should react when an in-flight call is interrupted.
    fn interrupt_behavior(&self, _input: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    /// Normalize the input to a form suitable for replay / observability.
    ///
    /// Tools may strip secrets, redact paths, or canonicalize ordering.
    fn backfill_observable_input(&self, _input: &mut Value) {}

    /// Validate the input before any side-effects.
    ///
    /// # Errors
    /// Returns [`ValidationError`] if the input fails tool-specific checks
    /// that go beyond the static JSON schema (e.g. path exists, regex
    /// compiles).
    async fn validate_input(
        &self,
        _input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }

    /// Compute the permission decision for this `(input, ctx)` pair.
    ///
    /// The dispatcher calls this before invoking [`Tool::call`]. Hooks may
    /// override the result (e.g. allow / deny via `PreToolUse`).
    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> PermissionResult;

    /// Filesystem path this invocation primarily targets, if any.
    ///
    /// Used by the permission engine and file-state cache.
    fn get_path(&self, _input: &Value) -> Option<PathBuf> {
        None
    }

    /// Human-readable description of what this invocation will do.
    async fn description(&self, input: &Value, opts: &DescriptionOptions) -> String;

    /// Long-form tool prompt used when assembling the system prompt.
    async fn prompt(&self, opts: &PromptOptions) -> String;

    /// Execute the tool.
    ///
    /// # Errors
    /// Returns [`ToolError`] on permission denial, validation failure,
    /// I/O issues, abort, or file-state cache violations.
    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError>;

    /// Short verb phrase for activity displays
    /// (e.g. `"Reading file"`, `"Running command"`).
    fn get_activity_description(&self, _input: &Value) -> Option<String> {
        None
    }

    /// User-facing display name for this `(input)` pair, overriding [`name`] in
    /// the UI when the tool wants a per-invocation label.
    ///
    /// Port of claude-code's `Tool.userFacingName(input)` (the `AgentTool`
    /// override lives at `UI.tsx:760-775` — it shows the subagent type, e.g.
    /// `"Explore"`, instead of the generic `"Agent"`). Defaults to `None` so
    /// every other `Tool` impl is unaffected (frozen-trait rule); a `None` means
    /// "use [`name`]".
    ///
    /// Distinct from the input-less [`user_facing_name`](Self::user_facing_name)
    /// (a static label used by tools like `StopTask`/`TaskOutput`); this
    /// per-invocation variant takes the call `input` so `AgentTool` can show the
    /// requested subagent type.
    ///
    /// [`name`]: Tool::name
    fn user_facing_name_for_input(&self, _input: &Value) -> Option<String> {
        None
    }

    /// Background color (a theme-color key / name string) for this tool's
    /// user-facing name badge, if any.
    ///
    /// Port of claude-code's `Tool.userFacingNameBackgroundColor(input)` (the
    /// `AgentTool` override lives at `UI.tsx:776-787` — it returns the agent's
    /// configured color via `getAgentColor(subagent_type)`). claude returns a
    /// `keyof Theme`; the Rust analog returns the color name string. Defaults to
    /// `None` so every other `Tool` impl is unaffected (frozen-trait rule).
    fn user_facing_name_background_color(&self, _input: &Value) -> Option<String> {
        None
    }
}

/// Static context passed to [`Tool::is_enabled`]: feature flags and other
/// host-level toggles that do not depend on a specific invocation.
#[derive(Debug, Clone, Default)]
pub struct ToolStaticContext {
    /// Boolean feature flags keyed by name.
    pub feature_flags: std::collections::HashMap<String, bool>,
}

/// Options controlling [`Tool::description`] output.
#[derive(Debug, Clone)]
pub struct DescriptionOptions {
    /// Whether the agent is running in a non-interactive session (CI, batch).
    pub is_non_interactive_session: bool,
}

/// Options controlling [`Tool::prompt`] output.
#[derive(Debug, Clone, Default)]
pub struct PromptOptions {
    /// Whether to include illustrative examples in the prompt.
    pub include_examples: bool,
    /// The session (or subagent) model id the wire `tools` array is being
    /// built for. claude-code calls `prompt({model})` per tool, and some
    /// prompts are model-gated — TodoWrite selects the short `FWd` vs. long
    /// `UWd` variant via the `Dh(model)` "simple system prompt" gate
    /// (`Xla(e)=Dh(e)?FWd:UWd`). `None` mirrors the binary's `Dh(undefined)`
    /// (returns `false` → the long prompt). Thread the real model from the
    /// turn-loop / subagent build sites so new models (e.g. `claude-opus-4-8`,
    /// which is outside `UWu`'s classic list) correctly pick `FWd`.
    pub model: Option<String>,
}

/// Information returned by [`Tool::is_search_or_read`] describing how an
/// invocation behaves with respect to the workspace.
#[derive(Debug, Clone)]
pub struct SearchReadInfo {
    /// True if this invocation searches across files (grep-like).
    pub is_search: bool,
    /// True if this invocation reads a specific file or resource.
    pub is_read: bool,
    /// True if this invocation lists directory contents.
    pub is_list: bool,
}

/// How a tool should react when an in-flight call is interrupted.
#[derive(Debug, Clone, Copy)]
pub enum InterruptBehavior {
    /// Cancel the in-flight call and return immediately.
    Cancel,
    /// Block until the in-flight call finishes naturally.
    Block,
}

/// A one-shot mutator a tool returns to adjust the turn's [`ToolUseContext`] —
/// the Rust twin of claude-code's `ToolResult.contextModifier`.
///
/// The turn loop folds a tool batch's modifiers POST-DISPATCH over a seed
/// context and reads the result back (SKILLEXEC.3, model scope: a Skill tool's
/// `model:` frontmatter switches the session's main-loop model for the rest of
/// the session). `Send` so the streaming concurrent dispatch can collect them
/// across `.await` points.
pub type ContextModifier = Box<dyn FnOnce(ToolUseContext) -> ToolUseContext + Send>;

/// Result of a successful [`Tool::call`].
///
/// `data` is the JSON payload returned to the model. `new_messages` carries
/// any extra conversation messages the tool wants to inject (rare).
/// `context_modifier` lets a tool mutate the [`ToolUseContext`] for
/// subsequent calls (e.g. record a side-effect). `mcp_meta` is opaque
/// metadata threaded through for MCP tools.
pub struct ToolCallResult {
    /// JSON payload returned to the model.
    pub data: Value,
    /// Extra conversation messages to inject after this call.
    pub new_messages: Vec<protocol::ConversationMessage>,
    /// Optional one-shot mutator for the [`ToolUseContext`].
    pub context_modifier: Option<ContextModifier>,
    /// Opaque per-call metadata (used by MCP tools).
    pub mcp_meta: Option<serde_json::Value>,
}

impl std::fmt::Debug for ToolCallResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `context_modifier` is a boxed `FnOnce` and cannot be `Debug`-printed.
        f.debug_struct("ToolCallResult")
            .field("data", &self.data)
            .field("new_messages", &self.new_messages)
            .field("mcp_meta", &self.mcp_meta)
            .finish_non_exhaustive()
    }
}

/// Error returned by [`Tool::validate_input`].
#[derive(Debug, Clone, Error)]
#[error("invalid tool input: {0}")]
pub struct ValidationError(
    /// Human-readable validation error message.
    pub String,
);

/// Error returned by [`Tool::call`] and related dispatcher operations.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum ToolError {
    /// No tool registered under the requested name.
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Input failed validation (schema or tool-specific checks).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Permission engine denied the call.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Underlying I/O failure (network, disk, etc.).
    #[error("io: {0}")]
    Io(String),
    /// The call was aborted (interrupt, cancellation).
    #[error("aborted")]
    Aborted,
    /// Internal error: tool implementation bug or unexpected state.
    #[error("internal: {0}")]
    Internal(String),
    /// Edit attempted before the file was read in this session.
    /// Wired in Plan 10 via the file-state cache.
    #[error("edit before read: {path}")]
    EditWithoutRead {
        /// Path of the file the agent tried to edit.
        path: String,
    },
    /// File changed on disk after the agent last read it.
    /// Wired in Plan 10 via the file-state cache.
    #[error("file modified externally since last read: {path}")]
    FileModifiedExternally {
        /// Path of the file that changed externally.
        path: String,
    },
    /// The cached view of this file is partial; the agent must re-read it.
    /// Wired in Plan 10 via the file-state cache.
    #[error("partial view — please re-read {path}")]
    PartialViewMustReread {
        /// Path that the agent must re-read in full.
        path: String,
    },
    /// File content hash does not match the cached value.
    /// Wired in Plan 10 via the file-state cache.
    #[error("file content hash mismatch: {path}")]
    FileContentMismatch {
        /// Path of the file whose hash mismatched.
        path: String,
    },
    /// M4-01: file exceeds the size limit configured for the read.
    #[error("file too large: {size} bytes exceeds limit {limit}")]
    FileTooLarge {
        /// Actual size encountered, in bytes.
        size: u64,
        /// Configured maximum, in bytes.
        limit: u64,
    },
    /// M4-01: requested path resolves outside the trusted-dirs whitelist.
    #[error("path not in trusted directory: {path:?}")]
    PathBlocked {
        /// The canonicalised path that was rejected.
        path: std::path::PathBuf,
    },
    /// M4-01: file looks like a binary blob (NUL bytes in the head window).
    #[error("binary file detected at {path:?} ({reason})")]
    BinaryFile {
        /// Path of the file that was rejected.
        path: std::path::PathBuf,
        /// Short reason ("NUL byte at offset N").
        reason: &'static str,
    },
    /// M4-02: subprocess hit the watchdog (placeholder variant; emitter lives in M4-02).
    #[error("command timed out after {timeout_ms}ms")]
    Timeout {
        /// Configured watchdog, in milliseconds.
        timeout_ms: u64,
    },
    /// M4-01..M4-03: tool output exceeded `MAX_TOOL_OUTPUT_LENGTH`.
    #[error("output truncated at {limit} chars")]
    OutputTruncated {
        /// Char limit that was hit.
        limit: usize,
    },
    /// M4-05: subagent loop crashed.
    #[error("subagent failed: {0}")]
    SubagentFailed(String),
    /// M4-07: MCP server reported a tool-call failure.
    #[error("MCP tool error: {server}/{tool}: {detail}")]
    McpFailure {
        /// MCP server identifier.
        server: String,
        /// MCP tool name.
        tool: String,
        /// Server-supplied detail string.
        detail: String,
    },
    /// M4-07: LSP request failed.
    #[error("LSP tool error: {0}")]
    LspFailure(String),
    /// M4-03: HTTP transport error (DNS, connect, TLS, status code).
    #[error("transport error: {0}")]
    Transport(String),
}

impl ToolError {
    /// The BARE, model-facing message — the string claude-code would have
    /// thrown as `error.message` and rendered verbatim into the `tool_result`
    /// content via `formatError` (`utils/toolErrors.ts` returns `error.message`
    /// unmodified; `services/tools/toolExecution.ts:1691` feeds it raw into the
    /// `is_error` tool_result block).
    ///
    /// This is DISTINCT from [`std::fmt::Display`] (used for internal logging),
    /// which prepends a per-variant prefix (`invalid input: `, `internal: `,
    /// …). Those prefixes are a LingXi-internal convenience and must NOT leak
    /// into the wire bytes the model sees — claude emits only the inner message
    /// (e.g. `Agent type 'x' not found. Available agents: …`).
    ///
    /// For the message-carrying variants, the inner string IS exactly what the
    /// equivalent claude-code tool throws, so we return it bare. The structured
    /// variants (file-state, size, path, timeout, …) keep their full `Display`
    /// rendering — that text IS the message claude would surface, and there is
    /// no spurious prefix to strip.
    #[must_use]
    pub fn model_facing_message(&self) -> String {
        match self {
            // Single-string variants whose `Display` prepends a non-meaningful
            // prefix: the inner string is the claude `error.message`.
            ToolError::NotFound(s)
            | ToolError::InvalidInput(s)
            | ToolError::PermissionDenied(s)
            | ToolError::Io(s)
            | ToolError::Internal(s)
            | ToolError::SubagentFailed(s)
            | ToolError::LspFailure(s)
            | ToolError::Transport(s) => s.clone(),
            // Structured / prefix-free variants: `Display` already renders the
            // exact model-facing message (no LingXi-only prefix to strip).
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod m4_01_error_variant_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn file_too_large_display_format() {
        let e = ToolError::FileTooLarge {
            size: 300_000,
            limit: 262_144,
        };
        assert_eq!(
            e.to_string(),
            "file too large: 300000 bytes exceeds limit 262144"
        );
    }

    #[test]
    fn path_blocked_display_format() {
        let e = ToolError::PathBlocked {
            path: PathBuf::from("/etc/passwd"),
        };
        assert_eq!(
            e.to_string(),
            r#"path not in trusted directory: "/etc/passwd""#
        );
    }

    #[test]
    fn binary_file_display_format() {
        let e = ToolError::BinaryFile {
            path: PathBuf::from("/tmp/x.bin"),
            reason: "NUL byte at offset 0",
        };
        assert_eq!(
            e.to_string(),
            r#"binary file detected at "/tmp/x.bin" (NUL byte at offset 0)"#
        );
    }

    #[test]
    fn timeout_display_format() {
        let e = ToolError::Timeout { timeout_ms: 5000 };
        assert_eq!(e.to_string(), "command timed out after 5000ms");
    }

    #[test]
    fn output_truncated_display_format() {
        let e = ToolError::OutputTruncated { limit: 30_000 };
        assert_eq!(e.to_string(), "output truncated at 30000 chars");
    }

    /// `model_facing_message()` returns the BARE inner message (claude's
    /// `error.message`) — NO `invalid input: ` / `internal: ` Display prefix.
    /// claude renders this verbatim into the `is_error` tool_result content
    /// (`formatError` → `toolExecution.ts:1691`); the prefix is logging-only.
    #[test]
    fn model_facing_message_strips_variant_prefix() {
        // The two prefixes the AgentTool errors actually use.
        let invalid = ToolError::InvalidInput(
            "Agent type 'x' not found. Available agents: general-purpose".into(),
        );
        assert_eq!(
            invalid.model_facing_message(),
            "Agent type 'x' not found. Available agents: general-purpose"
        );
        // Display keeps the prefix (logging path) — the divergence the fix targets.
        assert_eq!(
            invalid.to_string(),
            "invalid input: Agent type 'x' not found. Available agents: general-purpose"
        );

        let internal = ToolError::Internal(
            "Agent 'x' requires MCP servers matching: github. MCP servers with tools: none."
                .into(),
        );
        assert_eq!(
            internal.model_facing_message(),
            "Agent 'x' requires MCP servers matching: github. MCP servers with tools: none."
        );

        // Other single-string variants are bare too.
        assert_eq!(
            ToolError::SubagentFailed("boom".into()).model_facing_message(),
            "boom"
        );

        // Structured variants keep their full Display (no spurious prefix).
        let timeout = ToolError::Timeout { timeout_ms: 5000 };
        assert_eq!(
            timeout.model_facing_message(),
            "command timed out after 5000ms"
        );
    }
}

#[cfg(test)]
mod user_facing_name_default_tests {
    use super::*;
    use crate::progress::ToolProgressSender;

    /// A minimal `Tool` that overrides nothing — exercises the defaulted
    /// `user_facing_name` / `user_facing_name_background_color` (G9 frozen-trait
    /// rule: every non-`AgentTool` tool sees `None`).
    struct BareTool {
        schema: Value,
    }

    #[async_trait]
    impl Tool for BareTool {
        fn name(&self) -> &str {
            "Bare"
        }
        fn input_schema(&self) -> &Value {
            &self.schema
        }
        fn is_enabled(&self, _: &ToolStaticContext) -> bool {
            true
        }
        fn max_result_size_chars(&self) -> usize {
            0
        }
        fn is_concurrency_safe(&self, _: &Value) -> bool {
            true
        }
        fn is_read_only(&self, _: &Value) -> bool {
            true
        }
        async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
            unimplemented!("not exercised")
        }
        async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
            String::new()
        }
        async fn prompt(&self, _: &PromptOptions) -> String {
            String::new()
        }
        async fn call(
            &self,
            _: Value,
            _: ToolUseContext,
            _: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            unimplemented!("not exercised")
        }
    }

    #[test]
    fn user_facing_name_methods_default_to_none() {
        let t = BareTool {
            schema: serde_json::json!({"type": "object"}),
        };
        let input = serde_json::json!({});
        assert_eq!(t.user_facing_name_for_input(&input), None);
        assert_eq!(t.user_facing_name_background_color(&input), None);
    }
}
