//! `tokio::process`-backed [`ProcessRunner`] for desktop hosts.
//!
//! Foreground `run` applies the claude-code spawn-env contract
//! (`CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<bin>`) plus a 30-minute
//! default timeout. `spawn_background` lands a real child with
//! file-mode stdio (POSIX `O_NOFOLLOW`) wired to a per-task output file
//! and a setsid call so [`super::kill_tree::kill_tree_unix`] can later
//! terminate the descendant process group. `kill(handle)` simply
//! delegates to `kill_tree_unix(handle.pid)`.

use crate::process::kill_tree::kill_tree_unix;
use crate::process::spawn_unsafe::attach_setsid;
use crate::process::wrap::{
    task_output_path, DEFAULT_TIMEOUT, ENV_CLAUDECODE, ENV_CLAUDE_CODE_SESSION_ID, ENV_GIT_EDITOR,
    ENV_SHELL,
};
use async_trait::async_trait;
use lingxi_traits::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand};
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Production [`ProcessRunner`] using `tokio::process`.
#[derive(Default)]
pub struct PosixProcess;

impl PosixProcess {
    /// Construct a new `PosixProcess` runner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Build a `tokio::process::Command` from a sandboxed command, applying
    /// the claude-code spawn-env contract.
    ///
    /// Env-var precedence (matches claude-code `Shell.ts:317-328`):
    /// 1. Caller-supplied env vars on the [`SandboxedCommand`].
    /// 2. `CLAUDECODE=1`, `GIT_EDITOR=true`, `SHELL=<inner.command>` —
    ///    overwritten on top so callers cannot accidentally clobber them.
    /// 3. `CLAUDE_CODE_SESSION_ID` is propagated only when the caller has
    ///    explicitly injected it through the env map (the engine layer
    ///    decides whether to set it).
    fn build_command(cmd: &SandboxedCommand) -> Command {
        let inner = cmd.inner();
        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }

        // 1. Caller env first.
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
        // 2. Spawn-env contract (overrides anything the caller set).
        tcmd.env(ENV_CLAUDECODE.0, ENV_CLAUDECODE.1);
        tcmd.env(ENV_GIT_EDITOR.0, ENV_GIT_EDITOR.1);
        tcmd.env(ENV_SHELL, &inner.command);
        // 3. CLAUDE_CODE_SESSION_ID propagated only if explicitly provided.
        if let Some(sess) = inner.env.get(ENV_CLAUDE_CODE_SESSION_ID) {
            tcmd.env(ENV_CLAUDE_CODE_SESSION_ID, sess);
        }
        tcmd
    }
}

#[async_trait]
impl ProcessRunner for PosixProcess {
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
        let task_id = generate_task_id();
        let out_path = task_output_path(&task_id);
        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ProcessError::Io(format!("mkdir task-output: {e}")))?;
        }

        // Open the file with O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW
        // to match claude-code's symlink-attack guard in Shell.ts:299-312.
        // `.append(true)` already implies write access on POSIX — clippy's
        // `ineffective_open_options` would flag a redundant `.write(true)`.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&out_path)
            .map_err(|e| ProcessError::Io(format!("open task-output {out_path:?}: {e}")))?;
        // Duplicate the fd for stderr so both streams interleave atomically.
        let stderr_file = file
            .try_clone()
            .map_err(|e| ProcessError::Io(format!("clone fd: {e}")))?;

        let mut tcmd = Self::build_command(cmd);
        tcmd.stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr_file));
        attach_setsid(&mut tcmd);

        let child = tcmd
            .spawn()
            .map_err(|e| ProcessError::Io(format!("spawn_background: {e}")))?;
        let pid = child
            .id()
            .ok_or_else(|| ProcessError::Io("spawn_background: child has no pid".into()))?;

        // Detach the JoinHandle — the child runs on its own; kill_tree
        // terminates it later. `tokio::process::Child` requires `.wait()` to
        // be called; spawn a small reaper to avoid zombies.
        tokio::spawn(async move {
            let mut child = child;
            let _ = child.wait().await;
        });

        Ok(ProcessHandle { task_id, pid })
    }

    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError> {
        kill_tree_unix(handle.pid).await
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Generate a unique task id of the form `local_bash_<nanos-hex>`.
fn generate_task_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("local_bash_{nanos:x}")
}
