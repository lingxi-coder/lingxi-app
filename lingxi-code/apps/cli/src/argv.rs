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
    let amount: f64 = value
        .parse()
        .map_err(|_| "--max-budget-usd must be a positive number greater than 0".to_string())?;
    // JS `Number("inf")` is `NaN` (rejected) and `Number("1e400")` is `Infinity`,
    // and claude's guard is `isNaN(amount) || amount <= 0`. Rust's `f64::FromStr`
    // instead parses "inf"/"INF"/"Infinity"/"1e400" all to `f64::INFINITY`, which
    // is neither NaN nor `<= 0` — so reject any non-finite value to match the
    // oracle's parse-time rejection (and bound the downstream cost cap).
    if !amount.is_finite() || amount <= 0.0 {
        return Err("--max-budget-usd must be a positive number greater than 0".to_string());
    }
    Ok(amount)
}

/// AI coding assistant — runs a single turn or REPL
#[derive(Debug, Parser, Clone, Default)]
#[command(name = "lingxi-cli", version, about, long_about = None)]
#[allow(clippy::struct_excessive_bools, clippy::doc_markdown)]
pub struct Argv {
    /// Top-level subcommand (mcp, auth, plugin, project, setup-token, agents,
    /// install, update, doctor, auto-mode, ultrareview). Declared BEFORE the
    /// `prompt` positional so clap resolves a leading subcommand-name token as
    /// the subcommand (and an optional-value global flag like `-d`/`-r` before
    /// it can't swallow it) rather than as the chat `[prompt]`. `None` = the
    /// normal chat / REPL / print path.
    #[command(subcommand)]
    pub command: Option<crate::commands::Commands>,

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
    ///
    /// HIDDEN in claude-code 2.1.191 (`.hideHelp()`; absent from `claude
    /// --help`) — mirrored here with `hide = true`.
    #[arg(long = "max-turns", value_name = "turns", hide = true)]
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

    /// Additional directories to allow tool access to
    ///
    /// claude-code `--add-dir <directories...>`. Unioned into the permission
    /// policy's working-directory set (like `permissions.additionalDirectories`)
    /// so file tools (Read/Edit/Bash) may operate outside `cwd`.
    #[arg(long = "add-dir", value_name = "directories", num_args = 1..)]
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

    /// Minimal mode: skip hooks, LSP, plugin sync...Sets LINGXI_SIMPLE=1
    // TODO(bare): wire into LINGXI_SIMPLE env + skip hooks/LSP/plugin behavior
    #[arg(long = "bare")]
    pub bare: bool,

    /// Start with all customizations disabled — useful for troubleshooting
    // TODO(safe-mode): wire into safe mode initialization (no plugins, hooks, etc.)
    #[arg(long = "safe-mode")]
    pub safe_mode: bool,

    /// JSON object defining custom agents (e.g. '{"reviewer": {"description":
    /// "Reviews code", "prompt": "You are a code reviewer"}}')
    ///
    /// claude-code `--agents <json>` takes EXACTLY ONE value (a JSON object
    /// string parsed downstream), not a space-separated list.
    // TODO(agents): parse the JSON + wire into agent configuration
    #[arg(long = "agents", value_name = "json")]
    pub agents: Option<String>,

    /// Agent for the current session. Overrides the 'agent' setting.
    ///
    /// claude-code `--agent <agent>` takes EXACTLY ONE value.
    // TODO(agent): wire into agent configuration
    #[arg(long = "agent", value_name = "agent")]
    pub agent: Option<String>,

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
    // No `require_equals`: commander's `--from-pr [value]` consumes the next
    // SPACE-separated token as the value (`--from-pr 123`), so we must NOT force
    // the `--from-pr=123` form or `123` would be mis-parsed as the prompt.
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

    /// Enable prompt suggestions. In print/SDK mode, emits a prompt_suggestion
    /// message after each turn with a predicted next user prompt
    ///
    /// VISIBLE in claude-code 2.1.191 with a fixed choices list and preset
    /// "true": bare `--prompt-suggestions` → "true"; an out-of-choices value
    /// (e.g. "banana") is HARD-REJECTED. The `value_parser` below mirrors that.
    // TODO(prompt-suggestions): wire into prompt suggestion emission
    #[arg(
        long = "prompt-suggestions",
        value_name = "value",
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = ["true", "false", "1", "0", "yes", "no", "on", "off"]
    )]
    pub prompt_suggestions: Option<String>,

    /// Validate the final result against this JSON Schema, forcing structured
    /// output (claude-code `--json-schema <schema>`; only works with `--print`).
    /// The model is compelled to call a `StructuredOutput` tool whose schema is
    /// this; the result is validated and retried up to
    /// `MAX_STRUCTURED_OUTPUT_RETRIES` times. Pass a JSON Schema as a string.
    #[arg(long = "json-schema", value_name = "schema")]
    pub json_schema: Option<String>,

    /// Enable verbose logging to stderr (debug mode) with optional category
    /// filtering (e.g. "api,hooks" or "!1p,!file")
    ///
    /// claude-code: `-d, --debug [filter]`. The value is OPTIONAL: bare `-d` /
    /// `--debug` enables debug mode unfiltered (empty sentinel); `--debug
    /// api,hooks` carries the category filter. Stored as `Option<String>`:
    /// `None` = off, `Some("")` = on/unfiltered, `Some(filter)` = on/filtered.
    /// `debug_enabled()` collapses it back to the old bool for callers that
    /// only need on/off (e.g. `logging::init`).
    /// No `require_equals`: commander's `-d, --debug [filter]` consumes the next
    /// SPACE-separated token as the optional filter (`--debug api,hooks`), and a
    /// bare `--debug` before a subcommand-name token binds that token as the
    /// filter exactly as the real binary does (it does NOT route to the
    /// subcommand and does NOT start a billable turn — the leading `command`
    /// subcommand resolution only triggers when the FIRST token is the command).
    #[arg(short = 'd', long = "debug", value_name = "filter", num_args = 0..=1, default_missing_value = "")]
    pub debug: Option<String>,

    /// Disable TUI; use stdio REPL (line-editing fallback)
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    /// SECURITY-SENSITIVE: bypass all permission prompts for the session
    /// (claude-code `--dangerously-skip-permissions`). Resolves to
    /// `PermissionMode::BypassPermissions` subject to the safety guards
    /// (root refusal; ant sandbox/no-internet) in `permission::bypass_guard`.
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Initial permission mode (`--permission-mode <mode>`). claude-code 2.1.191
    /// commander `.choices(['acceptEdits','auto','bypassPermissions','default',
    /// 'dontAsk','plan'])` — an out-of-choices value is HARD-REJECTED at parse
    /// time (exit 1 with an allowed-choices message), so the `value_parser`
    /// below mirrors that. `auto` is a real choice and resolves to
    /// `PermissionMode::Auto` downstream.
    #[arg(
        long = "permission-mode",
        // lowercase placeholder so the help line and the commander-style
        // invalid-value error read `--permission-mode <mode>` (not `<MODE>`).
        value_name = "mode",
        value_parser = ["acceptEdits", "auto", "bypassPermissions", "default", "dontAsk", "plan"]
    )]
    pub permission_mode: Option<String>,

    // ── v2.1.191 parity: previously-missing public top-level flags ───────────
    // These are accepted by clap so `lingxi-cli` no longer HARD-ERRORS on a
    // visible `claude --help` flag. Behavioral wiring for the not-yet-ported
    // ones is tracked in task "Wire un-ported flag backing features"; until
    // then they parse-and-carry (accepted, inert) so scripts/SDK callers stop
    // breaking on contact. claude-code source: main.tsx flag registration.
    /// Use a specific session ID for the conversation (must be a valid UUID)
    ///
    /// claude-code `--session-id <uuid>`. Overrides the generated session id.
    #[arg(long = "session-id", value_name = "uuid")]
    pub session_id: Option<String>,

    /// Set a display name for this session (shown in the prompt box, /resume
    /// picker, and terminal title)
    #[arg(short = 'n', long = "name", value_name = "name")]
    pub name: Option<String>,

    /// Comma-separated list of setting sources to load (user, project, local).
    #[arg(long = "setting-sources", value_name = "sources")]
    pub setting_sources: Option<String>,

    /// Only use MCP servers from --mcp-config, ignoring all other MCP
    /// configurations
    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,

    /// Move per-machine sections (cwd, env info, memory paths, git status) from
    /// the system prompt into the first user message. Improves cross-user
    /// prompt-cache reuse. Only applies with the default system prompt.
    #[arg(long = "exclude-dynamic-system-prompt-sections")]
    pub exclude_dynamic_system_prompt_sections: bool,

    /// [DEPRECATED. Use --debug instead] Enable MCP debug mode (shows MCP
    /// server errors)
    #[arg(long = "mcp-debug")]
    pub mcp_debug: bool,

    /// Automatically connect to IDE on startup if exactly one valid IDE is
    /// available
    #[arg(long = "ide")]
    pub ide: bool,

    /// MCP tool to use for permission prompts (only works with --print). Hidden
    /// SDK flag (claude-code registers it with `.hideHelp()`).
    #[arg(long = "permission-prompt-tool", value_name = "tool", hide = true)]
    pub permission_prompt_tool: Option<String>,

    /// Enable bypassing all permission checks as an option, without it being
    /// enabled by default. Recommended only for sandboxes with no internet
    /// access.
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub allow_dangerously_skip_permissions: bool,

    /// Disable all skills
    #[arg(long = "disable-slash-commands")]
    pub disable_slash_commands: bool,

    /// Enable Claude in Chrome integration
    #[arg(long = "chrome")]
    pub chrome: bool,

    /// Disable Claude in Chrome integration
    #[arg(long = "no-chrome")]
    pub no_chrome: bool,

    /// Render screen-reader friendly output (flat text, no decorative borders
    /// or animations).
    #[arg(long = "ax-screen-reader")]
    pub ax_screen_reader: bool,

    /// File resources to download at startup. Format: file_id:relative_path
    /// (e.g. --file file_abc:doc.txt file_def:img.png)
    #[arg(long = "file", value_name = "specs", num_args = 1..)]
    pub file: Option<Vec<String>>,

    /// Create a new git worktree for this session (optionally specify a name)
    ///
    /// claude-code `-w, --worktree [name]`. Value is OPTIONAL: bare `-w` mints
    /// an auto-named worktree (empty sentinel); `-w name` names it.
    #[arg(short = 'w', long = "worktree", value_name = "name", num_args = 0..=1, default_missing_value = "")]
    pub worktree: Option<String>,

    /// Create a tmux session for the worktree (requires --worktree). Uses iTerm2
    /// native panes when available; use --tmux=classic for traditional tmux.
    ///
    /// `--tmux` alone = native (empty sentinel); `--tmux=classic` = classic.
    /// `require_equals` so the value only binds via `=` (a bare `--tmux` won't
    /// swallow a following prompt token), matching commander's boolean-ish flag.
    #[arg(long = "tmux", value_name = "mode", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub tmux: Option<String>,

    /// Start the session as a background agent and return immediately (manage
    /// with `lingxi-cli agents`)
    #[arg(long = "background", visible_alias = "bg")]
    pub background: bool,
}

/// Recursively graft a hidden trailing catch-all positional onto every LEAF
/// subcommand (a node with no further subcommands) so surplus positional args
/// are silently ignored like commander's `allowExcessArguments` default. Groups
/// (incl. the top-level) are only recursed into — they must keep rejecting an
/// unknown first token as an unknown subcommand. A leaf that already has a
/// variadic positional (e.g. `mcp add <name> <commandOrUrl> [args...]`) is left
/// alone, since it already absorbs the surplus and a second catch-all would
/// conflict.
fn with_excess_catchall(mut cmd: clap::Command) -> clap::Command {
    let sub_names: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    if sub_names.is_empty() {
        if !has_variadic_positional(&cmd) {
            cmd = cmd.arg(
                clap::Arg::new("__excess_ignored")
                    .num_args(0..)
                    .hide(true)
                    .help("Excess positional arguments are ignored"),
            );
        }
    } else {
        for name in sub_names {
            cmd = cmd.mut_subcommand(name, with_excess_catchall);
        }
    }
    cmd
}

/// True iff `cmd` already has a positional that accepts more than one value (a
/// `Vec`/trailing-var-arg positional), which would conflict with a second
/// catch-all positional.
fn has_variadic_positional(cmd: &clap::Command) -> bool {
    cmd.get_positionals().any(|a| {
        a.get_num_args()
            .map(|r| r.max_values() > 1)
            .unwrap_or(false)
    })
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
        use clap::{CommandFactory, FromArgMatches};
        // Commander's `allowExcessArguments` defaults to true: surplus POSITIONAL
        // args are silently ignored, not errored (e.g. `mcp get foo extra` runs
        // `get foo`). clap rejects them, so we graft a hidden trailing catch-all
        // positional onto every LEAF subcommand that lacks a variadic positional,
        // then deserialize via `from_arg_matches` (which ignores the catch-all).
        // Unknown FLAGS and unknown SUBCOMMANDS still error — only excess
        // positionals are absorbed.
        let cmd = with_excess_catchall(Self::command());
        let matches = cmd.try_get_matches_from(iter)?;
        Self::from_arg_matches(&matches)
    }

    /// True iff debug mode is on (`-d`/`--debug` in any form, the deprecated
    /// `--mcp-debug` alias, or `--debug-file` which implicitly enables it).
    /// Collapses the new `Option<String>` filter back to the old on/off bool.
    #[must_use]
    pub fn debug_enabled(&self) -> bool {
        self.debug.is_some() || self.mcp_debug || self.debug_file.is_some()
    }

    /// The `--debug` category filter (e.g. "api,hooks" or "!1p,!file"), or
    /// `None` when debug is off or enabled without a filter. An empty string
    /// (bare `-d`/`--debug`) means "on, unfiltered" → `None` filter.
    #[must_use]
    pub fn debug_filter(&self) -> Option<&str> {
        match self.debug.as_deref() {
            Some(f) if !f.is_empty() => Some(f),
            _ => None,
        }
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
    /// NDJSON output. `--output-format stream-json` is handled separately via
    /// [`Self::is_stream_json`] and does NOT fall through here.
    #[must_use]
    pub fn is_json_output(&self) -> bool {
        self.json || matches!(self.output_format.as_deref(), Some("json"))
    }

    /// True iff `--output-format stream-json` is set (the realtime SDK output
    /// protocol). This is distinct from `--output-format json` (which emits a
    /// single result object) and from `--json` (LingXi legacy NDJSON).
    ///
    /// Note: the spec requires `--verbose` to accompany `--print +
    /// stream-json`; that validation is enforced in `run_cli` / `run_oneshot`.
    #[must_use]
    pub fn is_stream_json(&self) -> bool {
        matches!(self.output_format.as_deref(), Some("stream-json"))
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

    /// True iff `--input-format stream-json` is set.
    #[must_use]
    pub fn is_stream_json_input(&self) -> bool {
        matches!(self.input_format.as_deref(), Some("stream-json"))
    }

    /// Validate `--input-format=stream-json` cross-flag constraints.
    ///
    /// Checks (in binary order per §4.1 of SPEC-inferred.md):
    /// 1. `--input-format=stream-json` requires `--output-format=stream-json`
    ///    → `Error: --input-format=stream-json requires output-format=stream-json.`
    /// 2. `--input-format=stream-json` requires `--print`
    ///    → `Error: --input-format=stream-json requires --print.`
    /// 3. `--replay-user-messages` requires both `--input-format=stream-json`
    ///    and `--output-format=stream-json`
    ///    → `Error: --replay-user-messages requires both --input-format=stream-json and --output-format=stream-json.`
    ///
    /// Returns `Ok(())` when the combination is valid. The exact error strings
    /// are byte-locked to the binary (§4.1 SPEC-inferred.md).
    pub fn validate_stream_json_input_args(&self) -> Result<(), String> {
        if self.is_stream_json_input() {
            if !self.is_stream_json() {
                return Err(
                    "--input-format=stream-json requires output-format=stream-json.".to_string(),
                );
            }
            if !self.print {
                return Err("--input-format=stream-json requires --print.".to_string());
            }
        }
        if self.replay_user_messages {
            if !self.is_stream_json_input() || !self.is_stream_json() {
                return Err(
                    "--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json."
                        .to_string(),
                );
            }
        }
        Ok(())
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
#[path = "argv_test.rs"]
mod argv_test;
