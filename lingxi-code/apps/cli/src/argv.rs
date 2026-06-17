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
#[derive(Debug, Parser, Clone)]
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
    #[arg(long = "json")]
    pub json: bool,

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
}
