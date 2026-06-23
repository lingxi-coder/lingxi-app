//! clap-derive argv struct for `lingxi-cli`. See plan M5-12 Task 0 step 1 +
//! step 5 for the locked flag names and help-text first lines.
//!
//! The help-text first lines are **byte-locked** by the plan; the
//! `clippy::doc_markdown` allow below is required to keep `end_turn`,
//! `messages_create`, etc. rendered exactly as documented (no backticks).
//! `clippy::struct_excessive_bools` is allowed because the flag set is a
//! direct projection of the locked CLI surface — refactoring into enums
//! would diverge from the spec table.

use clap::Parser;
use std::path::PathBuf;

/// clap value-parser for `--max-budget-usd`. Mirrors claude-code's arg parser
/// (`main.tsx`): `Number(value)` then reject `isNaN(amount) || amount <= 0`
/// with the byte-identical error message. A non-numeric argument (which JS would
/// coerce to `NaN`) and a non-positive number both fail the same way here.
fn parse_positive_budget_usd(value: &str) -> Result<f64, String> {
    let amount: f64 = value.parse().map_err(|_| {
        "--max-budget-usd must be a positive number greater than 0".to_string()
    })?;
    if amount.is_nan() || amount <= 0.0 {
        return Err("--max-budget-usd must be a positive number greater than 0".to_string());
    }
    Ok(amount)
}

/// AI coding assistant — runs a single turn or REPL
#[derive(Debug, Parser, Clone, Default)]
#[command(name = "lingxi-cli", version, about, long_about = None)]
#[allow(clippy::struct_excessive_bools, clippy::doc_markdown)]
pub struct Argv {
    /// The user prompt for this one-shot conversation
    ///
    /// When absent (and `--resume` is not set), enters REPL mode (M5-13).
    pub prompt: Option<String>,

    /// Print mode: exit after first end_turn
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// Resume a previous session by UUID (or interactive picker if absent)
    ///
    /// claude-code: `-r, --resume [value]` — "Resume a conversation by session
    /// ID, or open interactive picker with optional search term" (`main.tsx:988`).
    /// The value is OPTIONAL (`[value]`): `-r`/`--resume` with no argument yields
    /// the empty-string picker sentinel; with an argument it carries the id /
    /// search term. (The user-facing help first line is byte-locked by plan
    /// M5-12 / `cli_help.rs`, so it is kept as the original wording above.)
    #[arg(short = 'r', long = "resume", value_name = "ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Continue the most recent conversation in the current directory
    ///
    /// claude-code: `-c, --continue` (`main.tsx:988`). FLAG PARSE ONLY here — the
    /// continue runtime (load-most-recent-in-cwd) is wired by the CLI entrypoint
    /// (`lib.rs` / `run.rs`), not this struct.
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// When resuming, create a new session ID instead of reusing the original (use with --resume or --continue)
    ///
    /// claude-code: `--fork-session` (`main.tsx:988`). FLAG PARSE ONLY here — the
    /// fork runtime (mint a fresh session id on resume) is wired downstream.
    #[arg(long = "fork-session")]
    pub fork_session: bool,

    /// Override the active model (e.g. claude-opus-4-7)
    #[arg(long = "model", value_name = "NAME")]
    pub model: Option<String>,

    /// Enable automatic fallback to specified model when default model is overloaded (only works with --print)
    ///
    /// Maps to `OrchestratorConfig::fallback_model`. claude-code accepts this
    /// flag unconditionally but only HONORS it in `--print`/non-interactive mode
    /// (`main.tsx:1000` documents "only works with --print"; it is consumed only
    /// on the print/query path). We mirror that SOFT restriction: parse it always
    /// (no parse-time `requires` error, matching claude-code), and the honoring is
    /// gated to print mode by the consumer. When the primary model hits the
    /// consecutive-529 Opus gate, the turn loop switches to this model
    /// (`query.ts:894-948`).
    #[arg(long = "fallback-model", value_name = "MODEL")]
    pub fallback_model: Option<String>,

    /// Maximum number of agentic turns before the loop early-exits (claude-code
    /// `--max-turns <turns>`, "only works with --print"). Maps to
    /// `OrchestratorConfig::max_turns`; unset (or `0`) = unbounded.
    #[arg(long = "max-turns", value_name = "turns")]
    pub max_turns: Option<u32>,

    /// Maximum dollar amount to spend on API calls (claude-code
    /// `--max-budget-usd <amount>`, "only works with --print"). Maps to
    /// `OrchestratorConfig::max_budget_nano_usd` (× 1e9); unset = no cap. Must be
    /// a positive number greater than 0 (parity with claude-code's arg parser).
    #[arg(long = "max-budget-usd", value_name = "amount", value_parser = parse_positive_budget_usd)]
    pub max_budget_usd: Option<f64>,

    /// Change to this directory before initialising
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Disable streaming SSE; use batched messages_create instead
    #[arg(long = "no-stream")]
    pub no_stream: bool,

    /// Emit machine-readable NDJSON to stdout (one event per line)
    ///
    /// LingXi-specific alias for `--output-format json`. The binary equivalent is
    /// `--output-format json`; `--json` keeps backward compatibility with callers
    /// that used the LingXi-specific flag before `--output-format` was added.
    #[arg(long = "json")]
    pub json: bool,

    /// Output format (only works with --print): "text" (default), "json" (single result), or "stream-json" (realtime streaming)
    ///
    /// `text` = plain stdout (default). `json` = same as `--json` (NDJSON per event).
    /// `stream-json` = realtime bidirectional NDJSON I/O protocol (SDK consumers).
    ///
    /// NOTE(stream-json): the full bidirectional stream-json I/O subsystem is not
    /// yet implemented. Parsing succeeds but `stream-json` behaves like `text`
    /// until the realtime NDJSON I/O subsystem is wired.
    // TODO(stream-json): realtime NDJSON I/O subsystem
    #[arg(long = "output-format", value_name = "format", value_parser = ["text", "json", "stream-json"])]
    pub output_format: Option<String>,

    /// Input format (only works with --print): "text" (default), or "stream-json" (realtime streaming input)
    ///
    /// NOTE(stream-json): the full bidirectional stream-json input subsystem is not
    /// yet implemented. Parsing succeeds but `stream-json` behaves like `text`
    /// until the realtime NDJSON I/O subsystem is wired.
    // TODO(stream-json): realtime NDJSON I/O subsystem
    #[arg(long = "input-format", value_name = "format", value_parser = ["text", "stream-json"])]
    pub input_format: Option<String>,

    /// System prompt to use for the session
    ///
    /// When set, replaces the default assembled system prompt for the session.
    /// Only works with --print (non-interactive mode).
    #[arg(long = "system-prompt", value_name = "prompt")]
    pub system_prompt: Option<String>,

    /// Append a system prompt to the default system prompt
    ///
    /// When set, appended to the end of the default system prompt for the session.
    /// Only works with --print (non-interactive mode).
    #[arg(long = "append-system-prompt", value_name = "prompt")]
    pub append_system_prompt: Option<String>,

    /// System prompt file to use for the session (hidden flag)
    ///
    /// Like `--system-prompt` but reads the prompt from a file.
    #[arg(long = "system-prompt-file", value_name = "file", hide = true)]
    pub system_prompt_file: Option<PathBuf>,

    /// Append a system prompt from a file to the default system prompt (hidden flag)
    ///
    /// Like `--append-system-prompt` but reads the prompt to append from a file.
    #[arg(long = "append-system-prompt-file", value_name = "file", hide = true)]
    pub append_system_prompt_file: Option<PathBuf>,

    /// Comma or space-separated list of tool names to allow (e.g. "Bash(git *) Edit")
    #[arg(long = "allowed-tools", visible_alias = "allowedTools", value_name = "tools", num_args = 1..)]
    pub allowed_tools: Option<Vec<String>>,

    /// Comma or space-separated list of tool names to deny (e.g. "Bash(git *) Edit")
    #[arg(long = "disallowed-tools", visible_alias = "disallowedTools", value_name = "tools", num_args = 1..)]
    pub disallowed_tools: Option<Vec<String>>,

    /// Specify the list of available tools from the built-in set. Use "" to disable all tools
    #[arg(long = "tools", value_name = "tools", num_args = 1..)]
    pub tools: Option<Vec<String>>,

    /// Add a directory to search for CLAUDE.md files
    ///
    /// Additional directories (beyond cwd and ~/.claude) to search for CLAUDE.md
    /// files and include in the system prompt context.
    #[arg(long = "add-dir", value_name = "directory", num_args = 1..)]
    pub add_dir: Option<Vec<PathBuf>>,

    /// Load settings from a JSON file or JSON string
    #[arg(long = "settings", value_name = "file-or-json")]
    pub settings: Option<String>,

    /// Load MCP servers from JSON files or strings (space-separated)
    // TODO(mcp-config): wire into mcp server loading from CLI flag
    #[arg(long = "mcp-config", value_name = "configs", num_args = 1..)]
    pub mcp_config: Option<Vec<String>>,

    /// Override verbose mode setting from config
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Minimal mode: skip hooks, LSP, plugin sync...Sets CLAUDE_CODE_SIMPLE=1
    // TODO(bare): wire into CLAUDE_CODE_SIMPLE env + skip hooks/LSP/plugin behavior
    #[arg(long = "bare")]
    pub bare: bool,

    /// Start with all customizations disabled — useful for troubleshooting
    // TODO(safe-mode): wire into safe mode initialization (no plugins, hooks, etc.)
    #[arg(long = "safe-mode")]
    pub safe_mode: bool,

    /// Configure agent behaviors (space-separated)
    // TODO(agents): wire into agent configuration
    #[arg(long = "agents", value_name = "agents", num_args = 1..)]
    pub agents: Option<Vec<String>>,

    /// Configure a single agent
    // TODO(agent): wire into agent configuration
    #[arg(long = "agent", value_name = "agent", num_args = 1..)]
    pub agent: Option<Vec<String>>,

    /// Directory to load plugins from
    // TODO(plugin-dir): wire into plugin loading path
    #[arg(long = "plugin-dir", value_name = "dir")]
    pub plugin_dir: Option<PathBuf>,

    /// Disable session persistence - sessions will not be saved to disk and cannot be resumed (only works with --print)
    // TODO(no-session-persistence): wire into session-save path
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,

    /// Resume a session linked to a PR by PR number/URL, or open interactive picker
    // TODO(from-pr): wire into PR-linked session resume
    #[arg(long = "from-pr", value_name = "value", num_args = 0..=1, default_missing_value = "")]
    pub from_pr: Option<String>,

    /// Effort level for the current session
    // TODO(effort): wire into model/orchestrator effort configuration
    #[arg(long = "effort", value_name = "level")]
    pub effort: Option<String>,

    /// Enable beta features (Warning: Custom betas are only available for API key users)
    // TODO(betas): wire into beta feature activation
    #[arg(long = "betas", value_name = "betas", num_args = 1..)]
    pub betas: Option<Vec<String>>,

    /// Write debug logs to a specific file path (implicitly enables debug mode)
    #[arg(long = "debug-file", value_name = "path")]
    pub debug_file: Option<PathBuf>,

    /// Include partial message chunks as they arrive (only works with --print and --output-format=stream-json)
    // TODO(stream-json): realtime NDJSON I/O subsystem
    #[arg(long = "include-partial-messages")]
    pub include_partial_messages: bool,

    /// Include all hook lifecycle events in the output stream (only works with --output-format=stream-json)
    // TODO(stream-json): realtime NDJSON I/O subsystem
    #[arg(long = "include-hook-events")]
    pub include_hook_events: bool,

    /// Re-emit user messages from stdin back on stdout for acknowledgment (only works with --input-format=stream-json and --output-format=stream-json)
    // TODO(stream-json): realtime NDJSON I/O subsystem
    #[arg(long = "replay-user-messages")]
    pub replay_user_messages: bool,

    /// Thinking mode: enabled (equivalent to adaptive), disabled (hidden flag)
    // TODO(thinking): wire into model thinking budget configuration
    #[arg(long = "thinking", value_name = "mode", value_parser = ["enabled", "adaptive", "disabled"], hide = true)]
    pub thinking: Option<String>,

    /// How thinking content appears in the response (hidden flag)
    // TODO(thinking-display): wire into thinking display configuration
    #[arg(long = "thinking-display", value_name = "display", value_parser = ["summarized", "omitted"], hide = true)]
    pub thinking_display: Option<String>,

    /// [DEPRECATED. Use --thinking instead for newer models] (hidden flag)
    // TODO(max-thinking-tokens): wire into thinking token budget (deprecated, use --thinking)
    #[arg(long = "max-thinking-tokens", value_name = "tokens", hide = true)]
    pub max_thinking_tokens: Option<u32>,

    /// Enable prompt suggestions. In print/SDK mode, emits a prompt_suggestion message after each turn (hidden flag)
    // TODO(prompt-suggestions): wire into prompt suggestion emission
    #[arg(long = "prompt-suggestions", value_name = "value", num_args = 0..=1, default_missing_value = "true", hide = true)]
    pub prompt_suggestions: Option<String>,

    /// Validate the final result against this JSON Schema, forcing structured
    /// output (claude-code `--json-schema <schema>`; only works with `--print`).
    /// The model is compelled to call a `StructuredOutput` tool whose schema is
    /// this; the result is validated and retried up to
    /// `MAX_STRUCTURED_OUTPUT_RETRIES` times. Pass a JSON Schema as a string.
    #[arg(long = "json-schema", value_name = "schema")]
    pub json_schema: Option<String>,

    /// Enable verbose logging to stderr
    #[arg(long = "debug")]
    pub debug: bool,

    /// Disable TUI; use stdio REPL (line-editing fallback)
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    /// SECURITY-SENSITIVE: bypass all permission prompts for the session
    /// (claude-code `--dangerously-skip-permissions`). Resolves to
    /// `PermissionMode::BypassPermissions` subject to the safety guards
    /// (root refusal; ant sandbox/no-internet) in `permission::bypass_guard`.
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Initial permission mode (`--permission-mode <mode>`): one of
    /// `default`/`plan`/`acceptEdits`/`bypassPermissions`/`dontAsk`. Unknown
    /// values resolve to `default` (claude-code `permissionModeFromString`).
    #[arg(long = "permission-mode", value_name = "MODE")]
    pub permission_mode: Option<String>,
}

impl Argv {
    /// Parse from any iterable of `OsString` (used by integration tests).
    ///
    /// Named `from_iter` for ergonomic parity with `Vec::from_iter`-style
    /// constructors; clippy's `should_implement_trait` is silenced because
    /// the canonical `std::iter::FromIterator` is the wrong shape (no
    /// `Result` return).
    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, T>(iter: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        Self::try_parse_from(iter)
    }

    /// True iff the binary should start a FRESH REPL.
    ///
    /// Rules: prompt is None or trimmed-empty, AND neither `--resume` nor
    /// `--continue` is set. Resume-without-prompt enters a resumed-REPL (Task 8);
    /// `--continue` likewise reopens the most-recent conversation rather than a
    /// fresh session, so it is excluded here too.
    #[must_use]
    pub fn is_repl_mode(&self) -> bool {
        let no_prompt = self.prompt.as_deref().map_or(true, |s| s.trim().is_empty());
        no_prompt && self.resume.is_none() && !self.continue_session
    }

    /// Resolve whether the JSON output mode is active.
    ///
    /// `--json` (LingXi-specific) OR `--output-format json` both activate JSON
    /// NDJSON output. `--output-format stream-json` is parsed but the full
    /// stream-json I/O subsystem is not yet implemented; it falls back to `json`
    /// for now (same NDJSON format, missing bidirectional protocol).
    #[must_use]
    pub fn is_json_output(&self) -> bool {
        self.json
            || matches!(
                self.output_format.as_deref(),
                Some("json") | Some("stream-json")
            )
    }

    /// Resolve the effective system-prompt override from `--system-prompt` /
    /// `--system-prompt-file` (with file taking precedence when both are set).
    ///
    /// Returns `None` when neither flag is set. File read failures are silently
    /// ignored (same as TS `fs.readFileSync` error → fall back to None).
    #[must_use]
    pub fn resolve_system_prompt(&self) -> Option<String> {
        if let Some(ref path) = self.system_prompt_file {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Some(content);
            }
        }
        self.system_prompt.clone()
    }

    /// Resolve the effective append-system-prompt from `--append-system-prompt` /
    /// `--append-system-prompt-file` (with file taking precedence when both are
    /// set). Returns `None` when neither flag is set.
    #[must_use]
    pub fn resolve_append_system_prompt(&self) -> Option<String> {
        if let Some(ref path) = self.append_system_prompt_file {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Some(content);
            }
        }
        self.append_system_prompt.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_is_repl_mode() {
        let a = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(a.is_repl_mode());
        assert!(a.prompt.is_none());
    }

    #[test]
    fn positional_prompt_is_oneshot() {
        let a = Argv::from_iter(["lingxi-cli", "fix the bug"]).unwrap();
        assert_eq!(a.prompt.as_deref(), Some("fix the bug"));
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn print_flag_short_form() {
        let a = Argv::from_iter(["lingxi-cli", "-p", "hi"]).unwrap();
        assert!(a.print);
        assert_eq!(a.prompt.as_deref(), Some("hi"));
    }

    #[test]
    fn print_flag_long_form() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "hi"]).unwrap();
        assert!(a.print);
    }

    #[test]
    fn resume_with_uuid() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--resume",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap();
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn resume_without_value_enters_picker_mode() {
        let a = Argv::from_iter(["lingxi-cli", "--resume"]).unwrap();
        // Empty sentinel = picker.
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn resume_short_alias_with_value() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "-r",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap();
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn resume_short_alias_without_value_enters_picker() {
        // `-r` value is OPTIONAL (`[value]`): bare `-r` → empty picker sentinel.
        let a = Argv::from_iter(["lingxi-cli", "-r"]).unwrap();
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn continue_long_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--continue"]).unwrap();
        assert!(a.continue_session);
        // `--continue` reopens the most-recent conversation, not a fresh REPL.
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn continue_short_flag() {
        let a = Argv::from_iter(["lingxi-cli", "-c"]).unwrap();
        assert!(a.continue_session);
    }

    #[test]
    fn continue_default_false() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.continue_session);
    }

    #[test]
    fn fork_session_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--resume", "--fork-session"]).unwrap();
        assert!(a.fork_session);
        assert_eq!(a.resume.as_deref(), Some(""));
    }

    #[test]
    fn fork_session_default_false() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.fork_session);
    }

    #[test]
    fn continue_and_fork_together() {
        let a = Argv::from_iter(["lingxi-cli", "-c", "--fork-session"]).unwrap();
        assert!(a.continue_session && a.fork_session);
        assert!(!a.is_repl_mode());
    }

    #[test]
    fn model_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--model", "claude-sonnet-4-6", "hi"]).unwrap();
        assert_eq!(a.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn fallback_model_flag() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--fallback-model",
            "claude-sonnet-4-6",
            "hi",
        ])
        .unwrap();
        assert_eq!(a.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn fallback_model_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.fallback_model.is_none());
    }

    #[test]
    fn fallback_model_accepted_without_print() {
        // Soft restriction (parity with claude-code): the flag PARSES regardless
        // of --print; honoring is deferred to the print/non-interactive consumer.
        let a = Argv::from_iter(["lingxi-cli", "--fallback-model", "claude-sonnet-4-6", "hi"])
            .unwrap();
        assert_eq!(a.fallback_model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn max_turns_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--max-turns", "5", "hi"]).unwrap();
        assert_eq!(a.max_turns, Some(5));
    }

    #[test]
    fn max_turns_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.max_turns.is_none());
    }

    #[test]
    fn max_budget_usd_flag_parses() {
        let a =
            Argv::from_iter(["lingxi-cli", "--print", "--max-budget-usd", "2.5", "hi"]).unwrap();
        assert_eq!(a.max_budget_usd, Some(2.5));
    }

    #[test]
    fn max_budget_usd_rejects_zero_and_negative() {
        // Parity with claude-code: the arg parser rejects `amount <= 0`.
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "0", "hi"]).is_err());
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "-1", "hi"]).is_err());
    }

    #[test]
    fn max_budget_usd_rejects_non_numeric() {
        // A non-numeric value (JS `Number(...)` → `NaN`) is rejected too.
        assert!(Argv::from_iter(["lingxi-cli", "--max-budget-usd", "abc", "hi"]).is_err());
    }

    #[test]
    fn json_schema_flag_parses() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--json-schema",
            r#"{"type":"object"}"#,
            "hi",
        ])
        .unwrap();
        assert_eq!(a.json_schema.as_deref(), Some(r#"{"type":"object"}"#));
    }

    #[test]
    fn json_schema_default_none() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.json_schema.is_none());
    }

    #[test]
    fn cwd_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--cwd", "/tmp", "hi"]).unwrap();
        assert_eq!(a.cwd, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn no_stream_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--no-stream", "hi"]).unwrap();
        assert!(a.no_stream);
    }

    #[test]
    fn json_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--json", "hi"]).unwrap();
        assert!(a.json);
    }

    #[test]
    fn debug_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--debug", "hi"]).unwrap();
        assert!(a.debug);
    }

    #[test]
    fn no_tui_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--no-tui"]).unwrap();
        assert!(a.no_tui);
    }

    #[test]
    fn no_tui_flag_default_false() {
        let a = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(!a.no_tui);
    }

    #[test]
    fn dangerously_skip_permissions_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--dangerously-skip-permissions"]).unwrap();
        assert!(a.dangerously_skip_permissions);
    }

    #[test]
    fn permission_mode_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--permission-mode", "plan"]).unwrap();
        assert_eq!(a.permission_mode.as_deref(), Some("plan"));
        let b = Argv::from_iter(["lingxi-cli"]).unwrap();
        assert!(b.permission_mode.is_none());
        assert!(!b.dangerously_skip_permissions);
    }

    #[test]
    fn unknown_flag_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--nonexistent"]);
        assert!(r.is_err());
    }

    #[test]
    fn all_flags_together() {
        let a = Argv::from_iter([
            "lingxi-cli",
            "--print",
            "--no-stream",
            "--json",
            "--debug",
            "--no-tui",
            "--cwd",
            "/r",
            "--model",
            "claude-opus-4-7",
            "--resume",
            "00000000-0000-0000-0000-000000000001",
            "fix it",
        ])
        .unwrap();
        assert!(a.print && a.no_stream && a.json && a.debug && a.no_tui);
        assert_eq!(a.cwd, Some(PathBuf::from("/r")));
        assert_eq!(a.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(
            a.resume.as_deref(),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(a.prompt.as_deref(), Some("fix it"));
    }

    // ── New flag parse tests (v2.1.186 parity) ───────────────────────────────

    #[test]
    fn output_format_text_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--output-format", "text", "hi"]).unwrap();
        assert_eq!(a.output_format.as_deref(), Some("text"));
        assert!(!a.is_json_output());
    }

    #[test]
    fn output_format_json_parses_and_activates_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--output-format", "json", "hi"]).unwrap();
        assert_eq!(a.output_format.as_deref(), Some("json"));
        assert!(a.is_json_output());
    }

    #[test]
    fn output_format_stream_json_parses_and_activates_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--output-format", "stream-json", "hi"]).unwrap();
        assert_eq!(a.output_format.as_deref(), Some("stream-json"));
        // stream-json maps to json output for now (TODO: bidirectional subsystem)
        assert!(a.is_json_output());
    }

    #[test]
    fn output_format_invalid_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--output-format", "xml"]);
        assert!(r.is_err());
    }

    #[test]
    fn json_flag_activates_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "--json", "hi"]).unwrap();
        assert!(a.is_json_output());
    }

    #[test]
    fn no_output_format_or_json_is_not_json_output() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(!a.is_json_output());
    }

    #[test]
    fn input_format_text_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--input-format", "text", "hi"]).unwrap();
        assert_eq!(a.input_format.as_deref(), Some("text"));
    }

    #[test]
    fn input_format_stream_json_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--input-format", "stream-json", "hi"]).unwrap();
        assert_eq!(a.input_format.as_deref(), Some("stream-json"));
    }

    #[test]
    fn system_prompt_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--system-prompt", "You are helpful.", "hi"]).unwrap();
        assert_eq!(a.system_prompt.as_deref(), Some("You are helpful."));
    }

    #[test]
    fn append_system_prompt_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--print", "--append-system-prompt", "Also be concise.", "hi"]).unwrap();
        assert_eq!(a.append_system_prompt.as_deref(), Some("Also be concise."));
    }

    #[test]
    fn system_prompt_file_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--system-prompt-file", "/tmp/prompt.txt", "hi"]).unwrap();
        assert_eq!(a.system_prompt_file, Some(PathBuf::from("/tmp/prompt.txt")));
    }

    #[test]
    fn append_system_prompt_file_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--append-system-prompt-file", "/tmp/append.txt", "hi"]).unwrap();
        assert_eq!(a.append_system_prompt_file, Some(PathBuf::from("/tmp/append.txt")));
    }

    #[test]
    fn allowed_tools_parses() {
        // Positional prompt before multi-value flag to avoid greedy consumption.
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--allowed-tools", "Bash", "Edit"]).unwrap();
        let tools = a.allowed_tools.unwrap();
        assert_eq!(tools, &["Bash", "Edit"]);
    }

    #[test]
    fn allowed_tools_alias_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--allowedTools", "Bash"]).unwrap();
        let tools = a.allowed_tools.unwrap();
        assert_eq!(tools, &["Bash"]);
    }

    #[test]
    fn disallowed_tools_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--disallowed-tools", "Bash"]).unwrap();
        let tools = a.disallowed_tools.unwrap();
        assert_eq!(tools, &["Bash"]);
    }

    #[test]
    fn disallowed_tools_alias_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--disallowedTools", "Edit"]).unwrap();
        let tools = a.disallowed_tools.unwrap();
        assert_eq!(tools, &["Edit"]);
    }

    #[test]
    fn tools_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--tools", "Bash", "Read"]).unwrap();
        let tools = a.tools.unwrap();
        assert_eq!(tools, &["Bash", "Read"]);
    }

    #[test]
    fn add_dir_single_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--add-dir", "/extra/docs"]).unwrap();
        let dirs = a.add_dir.unwrap();
        assert_eq!(dirs, &[PathBuf::from("/extra/docs")]);
    }

    #[test]
    fn add_dir_multiple_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--add-dir", "/a", "/b"]).unwrap();
        let dirs = a.add_dir.unwrap();
        assert_eq!(dirs, &[PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn settings_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--settings", r#"{"verbose":true}"#, "hi"]).unwrap();
        assert_eq!(a.settings.as_deref(), Some(r#"{"verbose":true}"#));
    }

    #[test]
    fn mcp_config_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--mcp-config", "/path/mcp.json"]).unwrap();
        let cfg = a.mcp_config.unwrap();
        assert_eq!(cfg, &["/path/mcp.json"]);
    }

    #[test]
    fn verbose_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--verbose", "hi"]).unwrap();
        assert!(a.verbose);
    }

    #[test]
    fn bare_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--bare", "hi"]).unwrap();
        assert!(a.bare);
    }

    #[test]
    fn safe_mode_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--safe-mode", "hi"]).unwrap();
        assert!(a.safe_mode);
    }

    #[test]
    fn agents_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--agents", "coder", "reviewer"]).unwrap();
        let agents = a.agents.unwrap();
        assert_eq!(agents, &["coder", "reviewer"]);
    }

    #[test]
    fn agent_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--agent", "coder"]).unwrap();
        let agent = a.agent.unwrap();
        assert_eq!(agent, &["coder"]);
    }

    #[test]
    fn plugin_dir_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--plugin-dir", "/my/plugins", "hi"]).unwrap();
        assert_eq!(a.plugin_dir, Some(PathBuf::from("/my/plugins")));
    }

    #[test]
    fn no_session_persistence_flag_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--no-session-persistence", "hi"]).unwrap();
        assert!(a.no_session_persistence);
    }

    #[test]
    fn from_pr_with_value_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--from-pr", "123", "hi"]).unwrap();
        assert_eq!(a.from_pr.as_deref(), Some("123"));
    }

    #[test]
    fn from_pr_without_value_uses_empty_sentinel() {
        let a = Argv::from_iter(["lingxi-cli", "--from-pr"]).unwrap();
        assert_eq!(a.from_pr.as_deref(), Some(""));
    }

    #[test]
    fn effort_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--effort", "high", "hi"]).unwrap();
        assert_eq!(a.effort.as_deref(), Some("high"));
    }

    #[test]
    fn betas_parses() {
        let a = Argv::from_iter(["lingxi-cli", "fix it", "--betas", "beta1", "beta2"]).unwrap();
        let betas = a.betas.unwrap();
        assert_eq!(betas, &["beta1", "beta2"]);
    }

    #[test]
    fn debug_file_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--debug-file", "/tmp/debug.log", "hi"]).unwrap();
        assert_eq!(a.debug_file, Some(PathBuf::from("/tmp/debug.log")));
    }

    #[test]
    fn include_partial_messages_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--include-partial-messages", "hi"]).unwrap();
        assert!(a.include_partial_messages);
    }

    #[test]
    fn include_hook_events_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--include-hook-events", "hi"]).unwrap();
        assert!(a.include_hook_events);
    }

    #[test]
    fn replay_user_messages_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--replay-user-messages", "hi"]).unwrap();
        assert!(a.replay_user_messages);
    }

    #[test]
    fn thinking_enabled_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "enabled", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("enabled"));
    }

    #[test]
    fn thinking_adaptive_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "adaptive", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("adaptive"));
    }

    #[test]
    fn thinking_disabled_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking", "disabled", "hi"]).unwrap();
        assert_eq!(a.thinking.as_deref(), Some("disabled"));
    }

    #[test]
    fn thinking_invalid_errors() {
        let r = Argv::from_iter(["lingxi-cli", "--thinking", "full"]);
        assert!(r.is_err());
    }

    #[test]
    fn thinking_display_summarized_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking-display", "summarized", "hi"]).unwrap();
        assert_eq!(a.thinking_display.as_deref(), Some("summarized"));
    }

    #[test]
    fn thinking_display_omitted_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--thinking-display", "omitted", "hi"]).unwrap();
        assert_eq!(a.thinking_display.as_deref(), Some("omitted"));
    }

    #[test]
    fn max_thinking_tokens_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--max-thinking-tokens", "1000", "hi"]).unwrap();
        assert_eq!(a.max_thinking_tokens, Some(1000));
    }

    #[test]
    fn prompt_suggestions_with_value_parses() {
        let a = Argv::from_iter(["lingxi-cli", "--prompt-suggestions", "false", "hi"]).unwrap();
        assert_eq!(a.prompt_suggestions.as_deref(), Some("false"));
    }

    #[test]
    fn prompt_suggestions_without_value_uses_true_sentinel() {
        let a = Argv::from_iter(["lingxi-cli", "--prompt-suggestions"]).unwrap();
        assert_eq!(a.prompt_suggestions.as_deref(), Some("true"));
    }

    #[test]
    fn resolve_system_prompt_from_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--system-prompt", "Be helpful.", "hi"]).unwrap();
        assert_eq!(a.resolve_system_prompt().as_deref(), Some("Be helpful."));
    }

    #[test]
    fn resolve_system_prompt_none_when_not_set() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.resolve_system_prompt().is_none());
    }

    #[test]
    fn resolve_append_system_prompt_from_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--append-system-prompt", "Be concise.", "hi"]).unwrap();
        assert_eq!(a.resolve_append_system_prompt().as_deref(), Some("Be concise."));
    }

    #[test]
    fn resolve_append_system_prompt_none_when_not_set() {
        let a = Argv::from_iter(["lingxi-cli", "hi"]).unwrap();
        assert!(a.resolve_append_system_prompt().is_none());
    }
}
