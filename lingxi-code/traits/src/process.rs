//! Process execution abstraction.
//!
//! `ProcessRunner` is the boundary between the engine's tool layer (Bash,
//! sub-shells, helper invocations) and platform-specific process spawning.
//! By accepting only [`SandboxedCommand`], the trait makes it impossible to
//! launch a child process that has not first passed through a
//! [`crate::sandbox::Sandbox`] decision (D2 / spec A1).
//!
//! See spec §24 (Sandbox) and D17 (Runtime boundary).

use crate::sandbox::SandboxedCommand;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Consumer for a command whose output must be observed while it is running.
///
/// The process runner invokes stdout one complete logical line at a time and
/// stderr in bounded byte chunks. Implementations should apply bounded
/// backpressure; returning an error aborts the command and lets the platform
/// runner tear down its process tree.
#[async_trait]
pub trait ProcessStreamSink: Send + Sync {
    /// Consume one stdout line, without its trailing line ending.
    async fn stdout_line(&self, line: String) -> Result<(), ProcessError>;

    /// Consume a bounded stderr chunk. Stderr is not an event stream, but must
    /// be drained concurrently so a noisy child cannot deadlock on a full pipe.
    async fn stderr_chunk(&self, chunk: Vec<u8>) -> Result<(), ProcessError>;
}

/// Runs sandbox-vetted commands on the host.
///
/// Implementations live in platform crates. Every method consumes a
/// [`SandboxedCommand`] so the type system enforces that the sandbox
/// pipeline has already been consulted.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// Run a sandboxed command to completion and collect its output.
    ///
    /// # Errors
    /// Returns [`ProcessError`] when the platform does not support process
    /// execution, when I/O fails, or when the command's timeout fires.
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError>;

    /// Run a sandboxed command while streaming stdout lines and stderr chunks.
    ///
    /// Platform runners with pipe support override this to deliver output live.
    /// The compatibility default preserves correctness for minimal/mobile test
    /// runners by executing through [`Self::run`] and replaying the captured
    /// output before returning.
    async fn run_streaming(
        &self,
        cmd: &SandboxedCommand,
        sink: std::sync::Arc<dyn ProcessStreamSink>,
    ) -> Result<ProcessOutput, ProcessError> {
        let output = self.run(cmd).await?;
        for line in output.stdout.lines() {
            sink.stdout_line(line.to_string()).await?;
        }
        if !output.stderr.is_empty() {
            sink.stderr_chunk(output.stderr.as_bytes().to_vec()).await?;
        }
        Ok(output)
    }

    /// Spawn a sandboxed command as a background process and return a
    /// handle that can later be passed to [`ProcessRunner::kill`].
    ///
    /// # Errors
    /// Returns [`ProcessError`] when the spawn fails or the platform does
    /// not support background processes.
    async fn spawn_background(&self, cmd: &SandboxedCommand)
        -> Result<ProcessHandle, ProcessError>;

    /// Kill a previously spawned background process.
    ///
    /// # Errors
    /// Returns [`ProcessError`] when the process cannot be terminated.
    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError>;

    /// Whether this runner can spawn processes on the current host.
    fn is_available(&self) -> bool;

    /// Run a hook command with runtime async detection (claude-code
    /// `hooks.ts:1117-1166`): the child's FIRST stdout line is inspected, and if
    /// it parses as `{"async": true, "asyncTimeout"?: <ms>}` the hook is
    /// backgrounded (left running, detached, bounded by `asyncTimeout` or
    /// `default_async_timeout`) and [`HookRunOutcome::Backgrounded`] is returned
    /// immediately, so the hook never blocks the turn. Otherwise the hook runs to
    /// completion and its buffered output is returned as
    /// [`HookRunOutcome::Completed`].
    ///
    /// The default implementation performs NO streaming: it defers to
    /// [`ProcessRunner::run`] and always reports `Completed`, so a platform
    /// without a streaming runner keeps its current (buffered, never-backgrounded)
    /// behavior. Only runners that can stream stdout override this.
    async fn run_hook_with_async_detection(
        &self,
        cmd: &SandboxedCommand,
        _default_async_timeout: std::time::Duration,
    ) -> Result<HookRunOutcome, ProcessError> {
        Ok(HookRunOutcome::Completed(self.run(cmd).await?))
    }

    /// Run a sandboxed FOREGROUND tool command, but — matching claude-code
    /// 2.1.210+ — when the command hits its timeout, hand the still-running child
    /// off to the background instead of killing it. Returns
    /// [`ForegroundOutcome::Completed`] when the command finished within its
    /// timeout, or [`ForegroundOutcome::MovedToBackground`] carrying the new
    /// background task handle when it timed out and was left running (its output
    /// keeps streaming to the task file so the model can Read it).
    ///
    /// The default implementation does NOT background: it defers to
    /// [`ProcessRunner::run`], mapping a normal finish to
    /// [`ForegroundOutcome::Completed`] and propagating the runner's timeout
    /// error unchanged, so a platform without the streaming handoff keeps its
    /// current kill-on-timeout behavior (the caller then falls back to the
    /// interrupted result). Only runners that can detach a timed-out child
    /// override this.
    ///
    /// # Errors
    /// Returns [`ProcessError`] on spawn/I/O failure. The default impl also
    /// surfaces [`ProcessError::Timeout`] on the timeout path.
    async fn run_foreground(
        &self,
        cmd: &SandboxedCommand,
    ) -> Result<ForegroundOutcome, ProcessError> {
        Ok(ForegroundOutcome::Completed(self.run(cmd).await?))
    }
}

/// Outcome of [`ProcessRunner::run_foreground`].
#[derive(Debug)]
pub enum ForegroundOutcome {
    /// The command finished within its timeout; carries its collected output.
    Completed(ProcessOutput),
    /// The command exceeded its timeout and was moved to the background
    /// (claude-code 2.1.210+). The child keeps running detached with its output
    /// streaming to the task file; the handle names the task so the caller can
    /// build the "moved to the background" note and the model can Read the file.
    MovedToBackground(ProcessHandle),
}

/// Outcome of [`ProcessRunner::run_hook_with_async_detection`].
///
/// Not `Clone` — the `Backgrounded` variant carries a single-consumer
/// [`tokio::sync::oneshot::Receiver`] for the hook's eventual output.
#[derive(Debug)]
pub enum HookRunOutcome {
    /// The hook ran to completion; carries its buffered output.
    Completed(ProcessOutput),
    /// The hook's first stdout line was `{"async": true, …}`; it has been
    /// backgrounded (detached, bounded by `async_timeout`). The eventual drained
    /// output is delivered once through `output` when the detached process
    /// finishes (or is killed at `async_timeout`), so the caller can fold it
    /// back as an `async_hook_response` (claude-code `registerPendingAsyncHook`).
    Backgrounded {
        /// Effective background timeout: the marker's `asyncTimeout` when
        /// present (and `> 0`), else the caller's `default_async_timeout`
        /// (claude-code `asyncTimeout || 15000`). The fold-back registration
        /// bounds itself by this same value.
        async_timeout: std::time::Duration,
        /// Delivers the hook's eventual drained [`ProcessOutput`] exactly once
        /// when the detached process finishes or is killed at `async_timeout`.
        /// `None` when the runner cannot retain the output (e.g. the default,
        /// non-streaming trait impl, which never backgrounds at all).
        output: Option<tokio::sync::oneshot::Receiver<ProcessOutput>>,
    },
}

/// Collected output of a completed process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOutput {
    /// Captured stdout (UTF-8 lossy decoded).
    pub stdout: String,
    /// Captured stderr (UTF-8 lossy decoded).
    pub stderr: String,
    /// Exit code (use `-1` to indicate "signalled" if the platform cannot
    /// report a real status).
    pub exit_code: i32,
    /// True when the runner had to kill the process for exceeding its
    /// timeout.
    pub timed_out: bool,
}

/// Handle to a background process previously spawned by
/// [`ProcessRunner::spawn_background`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessHandle {
    /// Engine-side task identifier.
    pub task_id: String,
    /// OS-level process id.
    pub pid: u32,
}

/// Failure modes shared by every [`ProcessRunner`] method.
#[derive(Debug, Clone, Error)]
pub enum ProcessError {
    /// Platform does not implement process execution.
    #[error("unsupported on this platform")]
    Unsupported,
    /// A requested policy guarantee cannot be enforced on this platform.
    /// The payload names the guarantee (spec: "errors must name the
    /// unenforceable guarantee").
    #[error("policy unsupported: {0}")]
    PolicyUnsupported(String),
    /// The runner received a [`SandboxedCommand`] whose backend plan is
    /// missing, malformed, or minted for a different backend.
    #[error("malformed sandbox plan: {0}")]
    MalformedSandboxPlan(String),
    /// Jail setup failed at runtime (after prepare admitted the command).
    #[error("sandbox enforcement failed: {0}")]
    SandboxEnforcementFailed(String),
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Timeout fired before the command completed.
    #[error("timeout")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::ProcessError;

    #[test]
    fn structured_variants_name_the_guarantee() {
        assert_eq!(
            ProcessError::PolicyUnsupported("networked shell is not supported".into()).to_string(),
            "policy unsupported: networked shell is not supported"
        );
        assert_eq!(
            ProcessError::MalformedSandboxPlan("missing android plan".into()).to_string(),
            "malformed sandbox plan: missing android plan"
        );
        assert_eq!(
            ProcessError::SandboxEnforcementFailed("seccomp load failed".into()).to_string(),
            "sandbox enforcement failed: seccomp load failed"
        );
    }
}
