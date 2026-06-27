//! `tokio::process`-backed [`ProcessRunner`] for Windows hosts.
//!
//! Mirrors the POSIX runner's spawn-env contract (`LINGXI=1`,
//! `GIT_EDITOR=true`, `SHELL=<inner.command>`) and the 30-minute default
//! timeout. `spawn_background` writes per-task file-mode stdio to
//! `<temp>/lingxi-task-output/<task_id>.out` and `kill` delegates to
//! [`super::kill_tree::kill_tree_windows`], which shells out to
//! `taskkill /T /F /PID <pid>`.

use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use traits::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand};

/// 30-minute default timeout matches the POSIX runner and claude-code's
/// `DEFAULT_TIMEOUT` (`Shell.ts:44`).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const ENV_LINGXI_MARKER: (&str, &str) = ("LINGXI", "1");
const ENV_GIT_EDITOR: (&str, &str) = ("GIT_EDITOR", "true");
const ENV_SHELL: &str = "SHELL";
const ENV_LINGXI_SESSION_ID: &str = "LINGXI_SESSION_ID";

/// Production [`ProcessRunner`] using `tokio::process`.
#[derive(Default)]
pub struct WindowsProcess;

impl WindowsProcess {
    /// Construct a new `WindowsProcess` runner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn build_command(cmd: &SandboxedCommand) -> Command {
        let inner = cmd.inner();
        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
        // Spawn-env contract (overrides caller env to mirror posix).
        tcmd.env(ENV_LINGXI_MARKER.0, ENV_LINGXI_MARKER.1);
        tcmd.env(ENV_GIT_EDITOR.0, ENV_GIT_EDITOR.1);
        tcmd.env(ENV_SHELL, &inner.command);
        if let Some(sess) = inner.env.get(ENV_LINGXI_SESSION_ID) {
            tcmd.env(ENV_LINGXI_SESSION_ID, sess);
        }
        tcmd
    }
}

#[async_trait]
impl ProcessRunner for WindowsProcess {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = tcmd.spawn().map_err(|e| ProcessError::Io(e.to_string()))?;
        if let Some(stdin_text) = &inner.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(stdin_text.as_bytes())
                    .await
                    .map_err(|e| ProcessError::Io(e.to_string()))?;
            }
        }

        let timeout = inner.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(r) => r.map_err(|e| ProcessError::Io(e.to_string()))?,
            Err(_) => return Err(ProcessError::Timeout),
        };

        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out: false,
        })
    }

    async fn spawn_background(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        // Minimal Windows implementation: spawn with file-mode stdio.
        // Unlike POSIX we do not call setsid — Windows tree-kill works off
        // the PID via taskkill's `/T` flag, which walks the parent-child
        // table regardless of session/group membership. The cli-demo only
        // exercises foreground today; this path is for symmetry with posix
        // so callers can use the same engine-side handle plumbing.
        let task_id = generate_task_id();
        let out_path = std::env::temp_dir()
            .join("lingxi-task-output")
            .join(format!("{task_id}.out"));
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }
        // `.append(true)` already implies write access — clippy flags a
        // redundant `.write(true)` with `ineffective_open_options`.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open {out_path:?}: {e}")))?;
        let stderr_file = file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;

        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr_file));

        let child = tcmd
            .spawn()
            .map_err(|e| ProcessError::Io(format!("spawn_background: {e}")))?;
        let pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("spawn_background: child has no pid".into()))?;
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });
        Ok(ProcessHandle { task_id, pid })
    }

    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
        super::kill_tree::kill_tree_windows(handle.pid).await
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Generate a unique task id of the form `local_bash_<nanos-hex>` —
/// matches the POSIX runner's scheme so engine-side log inspection looks
/// the same on either host.
fn generate_task_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("local_bash_{nanos:x}")
}
