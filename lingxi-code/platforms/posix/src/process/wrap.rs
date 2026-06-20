//! Command wrapping helpers for cwd tracking + extglob disable + env vars.
//!
//! Reproduces claude-code's `bashProvider.ts:156-187` wrap idiom so the
//! `BashShell` tool can:
//!
//!   1. Disable extended globs (bash `extglob`, zsh `EXTENDED_GLOB`) so
//!      user-typed glob patterns behave consistently across shells.
//!   2. Run the user command.
//!   3. Tail-record the post-exec working directory to `<cwd_file>` so the
//!      next invocation can resume from the same directory.
//!
//! The cwd file path is shell-single-quoted to defeat path-injection from
//! filenames with apostrophes.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 30-minute default timeout matches claude-code's
/// `DEFAULT_TIMEOUT = 30 * 60 * 1000` in `Shell.ts:44`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// `(name, value)` tuples for the env-vars claude-code injects into every
/// `Shell.execute` spawn. The runtime impl in [`super::runner`] applies
/// these on top of the caller-supplied env.
pub const ENV_CLAUDECODE: (&str, &str) = ("CLAUDECODE", "1");
/// Marks every spawned process as running inside a claude-code CHILD session —
/// claude-code `Uot` sets `CLAUDE_CODE_CHILD_SESSION:"1"` UNCONDITIONALLY (#7).
/// (The conditional `AI_AGENT`/`CLAUDE_EFFORT`/`TRACEPARENT` vars need a
/// command source / effort / tracing context not threaded into the runner — a
/// separate follow-up.)
pub const ENV_CLAUDE_CODE_CHILD_SESSION: (&str, &str) = ("CLAUDE_CODE_CHILD_SESSION", "1");
/// Forces `git`'s editor to a no-op so interactive git commands cannot
/// block the shell.
pub const ENV_GIT_EDITOR: (&str, &str) = ("GIT_EDITOR", "true");
/// Name of the `SHELL` env var inherited from the user's environment.
pub const ENV_SHELL: &str = "SHELL";
/// Name of the session-id env var the IDE bridge / hooks consume.
pub const ENV_CLAUDE_CODE_SESSION_ID: &str = "CLAUDE_CODE_SESSION_ID";

/// Stable per-task output file path.
///
/// `<temp>/lingxi-task-output/<task_id>.out`. The directory is created on
/// first use by [`super::runner`].
#[must_use]
pub fn task_output_path(task_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("lingxi-task-output")
        .join(format!("{task_id}.out"))
}

/// Wrap a user command for the bash-tool spawn so we can track cwd
/// changes and disable extended globs.
///
/// Layout per claude-code's `bashProvider.ts:156-187`:
/// ```text
/// <extglob_disable> && <command> && pwd -P >| '<cwd_file>'
/// ```
///
/// `shell_path` is the absolute path of the spawn binary (`/bin/bash`,
/// `/bin/zsh`, …) so we pick the right idiom. When
/// `claude_code_shell_prefix_set` is true (caller's
/// `CLAUDE_CODE_SHELL_PREFIX` env is non-empty), the combined bash+zsh
/// idiom is used because the wrapper may pick a different shell than
/// `shell_path`.
#[must_use]
pub fn wrap_command_for_cwd_tracking(
    command: &str,
    cwd_file: &Path,
    shell_path: &str,
    claude_code_shell_prefix_set: bool,
) -> String {
    let extglob = if claude_code_shell_prefix_set {
        Some("{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true".to_string())
    } else if shell_path.contains("bash") {
        Some("shopt -u extglob 2>/dev/null || true".to_string())
    } else if shell_path.contains("zsh") {
        Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true".to_string())
    } else {
        None
    };

    let escaped_cwd_file = shell_single_quote(&cwd_file.to_string_lossy());
    let pwd_tail = format!("pwd -P >| {escaped_cwd_file}");

    match extglob {
        Some(disable) => format!("{disable} && {command} && {pwd_tail}"),
        None => format!("{command} && {pwd_tail}"),
    }
}

/// Single-quote a string for bash. Replaces interior `'` with `'\''`.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_timeout_is_30_minutes() {
        assert_eq!(DEFAULT_TIMEOUT.as_secs(), 30 * 60);
    }

    #[test]
    fn env_constants_match_claude_code() {
        assert_eq!(ENV_CLAUDECODE, ("CLAUDECODE", "1"));
        assert_eq!(
            ENV_CLAUDE_CODE_CHILD_SESSION,
            ("CLAUDE_CODE_CHILD_SESSION", "1")
        );
        assert_eq!(ENV_GIT_EDITOR, ("GIT_EDITOR", "true"));
        assert_eq!(ENV_SHELL, "SHELL");
        assert_eq!(ENV_CLAUDE_CODE_SESSION_ID, "CLAUDE_CODE_SESSION_ID");
    }

    #[test]
    fn shell_single_quote_handles_apostrophes() {
        assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
    }

    #[test]
    fn unknown_shell_omits_extglob_disable() {
        let wrapped =
            wrap_command_for_cwd_tracking("ls", Path::new("/tmp/cwd"), "/bin/fish", false);
        assert!(!wrapped.contains("shopt"), "fish must not include shopt");
        assert!(!wrapped.contains("setopt"), "fish must not include setopt");
        assert!(wrapped.contains("pwd -P >| '/tmp/cwd'"));
    }
}
