//! Builds + runs the `tmux new-session` invocation used to give a freshly
//! created worktree its own detached tmux session.
//!
//! Mirrors claude-code 2.1.206's worktree-tmux creation call: `Ur("tmux",
//! ["new-session","-d","-s",name,"-c",path])`, which returns `{created:
//! false, error}` on a non-zero exit and success otherwise (binary
//! @216347493). This module is intentionally ISOLATED from
//! `swarm/tmux.rs` (which is pane-oriented, for the multi-agent swarm
//! feature) — worktree tmux sessions are a separate, session-oriented
//! concern. It is not yet wired into boot or `ExitWorktree`; see
//! `docs/superpowers/plans/2026-07-14-worktree-tmux-launch.md` Tasks 3-5.

use std::collections::HashMap;
use std::path::Path;

use traits::{ProcessCommand, ProcessRunner, SandboxedCommand, SandboxedTag};

/// Audit reason stamped on the `tmux new-session` command handed to the
/// [`ProcessRunner`]. This is an internal infra invocation (not a
/// user/agent-issued Bash command), so it bypasses the normal sandbox
/// pipeline the same way hook commands do (see
/// `platforms/posix/src/process/runner.rs`'s `HOOK_COMMAND_AUDIT_REASON`).
const WORKTREE_TMUX_AUDIT_REASON: &str = "worktree_tmux_new_session";

/// Build the argv (excluding the `tmux` program name itself) for creating a
/// detached tmux session rooted at `worktree_path`:
/// `new-session -d -s <session_name> -c <worktree_path>`.
#[must_use]
pub fn build_worktree_tmux_argv(session_name: &str, worktree_path: &Path) -> Vec<String> {
    vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session_name.to_string(),
        "-c".to_string(),
        worktree_path.to_string_lossy().into_owned(),
    ]
}

/// Run `tmux new-session -d -s <session_name> -c <worktree_path>` through
/// the given [`ProcessRunner`] seam. Maps a non-zero exit to
/// `Err(stderr)` (mirrors 206's `{created:false,error}`); a zero exit
/// (or any output) maps to `Ok(())`. A runner-level [`traits::ProcessError`]
/// is stringified into the `Err`.
pub async fn create_worktree_tmux_session(
    runner: &dyn ProcessRunner,
    session_name: &str,
    worktree_path: &Path,
) -> Result<(), String> {
    let pcmd = ProcessCommand {
        command: "tmux".to_string(),
        args: build_worktree_tmux_argv(session_name, worktree_path),
        cwd: None,
        env: HashMap::new(),
        timeout: None,
        stdin: None,
    };
    let sandboxed = SandboxedCommand::__new_sandboxed(
        pcmd,
        SandboxedTag::BypassAuditedWithReason {
            reason: WORKTREE_TMUX_AUDIT_REASON.to_string(),
        },
    );
    let output = runner
        .run(&sandboxed)
        .await
        .map_err(|e| e.to_string())?;
    if output.exit_code != 0 {
        return Err(output.stderr);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use traits::{ProcessError, ProcessHandle, ProcessOutput};

    #[test]
    fn build_worktree_tmux_argv_shape_is_exact() {
        let argv = build_worktree_tmux_argv(
            "wt-feat",
            Path::new("/repo/.lingxi/worktrees/feat"),
        );
        assert_eq!(
            argv,
            vec![
                "new-session".to_string(),
                "-d".to_string(),
                "-s".to_string(),
                "wt-feat".to_string(),
                "-c".to_string(),
                "/repo/.lingxi/worktrees/feat".to_string(),
            ]
        );
    }

    /// Mock `ProcessRunner` that returns a canned exit code + stderr and
    /// records the `tmux` argv it was handed, mirroring the `MockRunner`
    /// pattern used in `hooks/src/executor_test.rs`.
    struct MockRunner {
        exit_code: i32,
        stderr: String,
        recorded_command: Mutex<Option<String>>,
        recorded_args: Mutex<Option<Vec<String>>>,
    }

    impl MockRunner {
        fn new(exit_code: i32, stderr: &str) -> Self {
            Self {
                exit_code,
                stderr: stderr.to_string(),
                recorded_command: Mutex::new(None),
                recorded_args: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl ProcessRunner for MockRunner {
        async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            *self.recorded_command.lock().unwrap() = Some(cmd.inner().command.clone());
            *self.recorded_args.lock().unwrap() = Some(cmd.inner().args.clone());
            Ok(ProcessOutput {
                stdout: String::new(),
                stderr: self.stderr.clone(),
                exit_code: self.exit_code,
                timed_out: false,
            })
        }

        async fn spawn_background(
            &self,
            _cmd: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Err(ProcessError::Unsupported)
        }

        async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn zero_exit_maps_to_ok_and_uses_the_argv() {
        let runner = MockRunner::new(0, "");
        let path = PathBuf::from("/tmp/wt-x");
        let result = create_worktree_tmux_session(&runner, "wt-x", &path).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(
            *runner.recorded_command.lock().unwrap(),
            Some("tmux".to_string())
        );
        assert_eq!(
            *runner.recorded_args.lock().unwrap(),
            Some(build_worktree_tmux_argv("wt-x", &path))
        );
    }

    #[tokio::test]
    async fn nonzero_exit_maps_to_err_with_stderr() {
        let runner = MockRunner::new(1, "boom");
        let path = PathBuf::from("/tmp/wt-y");
        let result = create_worktree_tmux_session(&runner, "wt-y", &path).await;
        assert_eq!(result, Err("boom".to_string()));
    }
}
