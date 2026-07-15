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
}

/// Outcome of [`ProcessRunner::run_hook_with_async_detection`].
#[derive(Debug, Clone)]
pub enum HookRunOutcome {
    /// The hook ran to completion; carries its buffered output.
    Completed(ProcessOutput),
    /// The hook's first stdout line was `{"async": true, …}`; it has been
    /// backgrounded (detached, bounded by its async timeout). Platform runners
    /// may retain eventual stdout/stderr in `output_path`; the file can still be
    /// growing when this outcome is returned.
    Backgrounded {
        /// Path of the eventual stdout/stderr capture, when retained.
        output_path: Option<String>,
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
