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
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Timeout fired before the command completed.
    #[error("timeout")]
    Timeout,
}
