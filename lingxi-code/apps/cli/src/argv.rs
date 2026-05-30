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
    #[arg(long = "resume", value_name = "ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Override the active model (e.g. claude-opus-4-7)
    #[arg(long = "model", value_name = "NAME")]
    pub model: Option<String>,

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

    /// True iff the binary should start the REPL.
    ///
    /// Rules: prompt is None or trimmed-empty, AND `--resume` is also absent
    /// (resume-without-prompt enters resumed-REPL handled in Task 8).
    #[must_use]
    pub fn is_repl_mode(&self) -> bool {
        let no_prompt = self.prompt.as_deref().map_or(true, |s| s.trim().is_empty());
        no_prompt && self.resume.is_none()
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
    fn model_flag() {
        let a = Argv::from_iter(["lingxi-cli", "--model", "claude-sonnet-4-6", "hi"]).unwrap();
        assert_eq!(a.model.as_deref(), Some("claude-sonnet-4-6"));
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
