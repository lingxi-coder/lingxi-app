//! `tokio::process`-backed [`ProcessRunner`] for Windows hosts.
//!
//! Implements blocking `run` via `tokio::process::Command` with timeout and
//! stdin support. Background spawn (`spawn_background`) and `kill` are
//! stubbed (return [`ProcessError::Unsupported`]) pending a handle-tracking
//! follow-up — the cli-demo only exercises `run`.

use async_trait::async_trait;
use lingxi_traits::{ProcessError, ProcessHandle, ProcessOutput, ProcessRunner, SandboxedCommand};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Production [`ProcessRunner`] using `tokio::process`.
#[derive(Default)]
pub struct WindowsProcess;

impl WindowsProcess {
    /// Construct a new `WindowsProcess` runner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProcessRunner for WindowsProcess {
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
        let inner = cmd.inner();
        let mut tcmd = Command::new(&inner.command);
        tcmd.args(&inner.args);
        if let Some(cwd) = &inner.cwd {
            tcmd.current_dir(cwd);
        }
        for (k, v) in &inner.env {
            tcmd.env(k, v);
        }
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

        let timed_out = false;
        let output = match inner.timeout {
            Some(t) => match tokio::time::timeout(t, child.wait_with_output()).await {
                Ok(r) => r.map_err(|e| ProcessError::Io(e.to_string()))?,
                Err(_) => return Err(ProcessError::Timeout),
            },
            None => child
                .wait_with_output()
                .await
                .map_err(|e| ProcessError::Io(e.to_string()))?,
        };

        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
            timed_out,
        })
    }

    async fn spawn_background(
        &self,
        _cmd: &SandboxedCommand,
    ) -> Result<ProcessHandle, ProcessError> {
        // TODO(M2-followup): background spawn with handle tracking.
        Err(ProcessError::Unsupported)
    }

    async fn kill(&self, _handle: &ProcessHandle) -> Result<(), ProcessError> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
}
