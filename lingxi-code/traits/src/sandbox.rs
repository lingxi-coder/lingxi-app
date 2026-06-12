//! Process sandbox abstraction.
//!
//! The `Sandbox` trait is the boundary between the engine's command
//! execution layer and platform-specific sandbox implementations. The
//! [`SandboxedCommand`] newtype is the only shape accepted by
//! [`crate::process::ProcessRunner`], so a command cannot reach the runner
//! without first passing through a [`Sandbox::prepare`] (or audited bypass)
//! call. This is the type-system enforcement behind decision D2 / spec A1.
//!
//! See spec §24 (Sandbox) and D17 (Runtime boundary).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Platform-specific sandbox backend.
///
/// Implementations live in platform crates. The engine receives an
/// `Arc<dyn Sandbox>` and never invokes a backend directly.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Whether this sandbox can actually wrap commands on the current host.
    fn is_available(&self) -> bool;

    /// Backend kind for telemetry / decision logic.
    fn backend(&self) -> SandboxBackend;

    /// The ONLY way to construct a [`SandboxedCommand`]: through this method,
    /// which applies the policy and tags the command as sandboxed.
    ///
    /// # Errors
    /// Returns [`SandboxError::Unavailable`] when the backend cannot wrap the
    /// command, [`SandboxError::PathCanonicalize`] / [`SandboxError::SymlinkEscape`]
    /// when policy paths cannot be safely resolved, and [`SandboxError::Io`]
    /// for transient platform errors.
    fn prepare(
        &self,
        cmd: ProcessCommand,
        policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError>;

    /// Audited bypass for cases that need it (e.g. user explicitly disabled
    /// sandbox). Records the reason so the audit log can later confirm intent.
    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand;

    /// Probe the live capabilities of the backend (what features actually
    /// work on this host, e.g. user-namespaces availability on Linux).
    async fn probe_capability(&self) -> SandboxCapability;
}

/// Concrete sandbox implementation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxBackend {
    /// Linux user / mount / network namespaces.
    LinuxNamespaces,
    /// Firejail-based wrapper on Linux.
    LinuxFirejail,
    /// macOS `sandbox-exec` profile.
    MacOsSandboxExec,
    /// Windows Job Object + restricted token.
    WindowsJobObject,
    /// Android in-engine Minijail (no_new_privs / rlimits / seccomp via
    /// libminijail linked into the engine .so). Spec r3 D6.
    AndroidMinijail,
    /// No sandbox enforcement (used for explicit bypass).
    None,
}

/// Declarative sandbox policy applied by [`Sandbox::prepare`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Outbound network access policy.
    pub network: NetworkPolicy,
    /// Paths the sandboxed process is allowed to write to.
    pub writable_paths: Vec<PathBuf>,
    /// Paths that must remain inaccessible.
    pub denied_paths: Vec<PathBuf>,
    /// Whether the wrapped process may spawn subprocesses.
    pub allow_subprocess: bool,
    /// Resource ceilings enforced by the backend.
    pub limits: ResourceLimits,
}

/// Coarse network isolation level for a [`SandboxPolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkPolicy {
    /// No outbound network at all.
    Disabled,
    /// Loopback (127.0.0.0/8, `::1`) only.
    LoopbackOnly,
    /// Full outbound network access.
    Allowed,
}

/// Resource ceilings for a sandboxed process.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum CPU time in seconds.
    pub max_cpu_seconds: Option<u32>,
    /// Maximum resident memory in megabytes.
    pub max_memory_mb: Option<u32>,
    /// Maximum number of child processes / threads.
    pub max_processes: Option<u32>,
    /// Maximum number of open file descriptors.
    pub max_open_files: Option<u32>,
}

/// Live probe result from [`Sandbox::probe_capability`].
#[derive(Debug, Clone)]
pub struct SandboxCapability {
    /// Whether the backend can wrap commands right now.
    pub available: bool,
    /// Human-readable explanation when unavailable.
    pub reason: Option<String>,
    /// Per-feature flags for the backend.
    pub features: SandboxFeatures,
}

/// Granular feature flags reported by [`Sandbox::probe_capability`].
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // mirrors the OS capability matrix verbatim
pub struct SandboxFeatures {
    /// Backend can isolate the network namespace.
    pub network_isolation: bool,
    /// Backend can mount the root filesystem read-only.
    pub fs_readonly: bool,
    /// Backend can selectively expose writable paths.
    pub fs_readwrite_paths: bool,
    /// Backend can cap the number of processes.
    pub process_limit: bool,
    /// Backend can apply `NO_NEW_PRIVS` or equivalent.
    pub no_new_privileges: bool,
}

/// Failure modes shared by every [`Sandbox`] method.
#[derive(Debug, Clone, Error)]
pub enum SandboxError {
    /// Backend cannot wrap commands on this host.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// Backend cannot wrap commands on this platform at all (e.g. Windows).
    /// Distinct from [`Unavailable`] — `Unavailable` means the backend exists
    /// but dependencies are missing; `Unsupported` means the backend itself
    /// is absent from claude-code on this OS.
    ///
    /// [`Unavailable`]: SandboxError::Unavailable
    #[error("sandbox not supported on this platform")]
    Unsupported,
    /// `realpath`-style canonicalization failed for a policy path.
    #[error("path canonicalization failed: {0}")]
    PathCanonicalize(String),
    /// A canonicalized policy path resolved outside the allowed root
    /// (symlink escape).
    #[error("symlink escape detected for {0}")]
    SymlinkEscape(String),
    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(String),
}

/// Raw, un-sandboxed command description.
///
/// This is the shape callers build; it can be fed into [`Sandbox::prepare`]
/// or [`Sandbox::bypass_with_audit`] to obtain a [`SandboxedCommand`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCommand {
    /// Executable to spawn.
    pub command: String,
    /// CLI arguments.
    pub args: Vec<String>,
    /// Working directory for the child process.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: std::collections::HashMap<String, String>,
    /// Optional timeout — backends should kill the process after this.
    pub timeout: Option<std::time::Duration>,
    /// Optional stdin payload to feed the child process.
    pub stdin: Option<String>,
}

/// Opaque newtype — only constructible via [`Sandbox::prepare`],
/// [`Sandbox::bypass_with_audit`], or the `__new_sandboxed` constructor
/// (which is the seam external [`Sandbox`] impls call into).
///
/// [`crate::process::ProcessRunner::run`] accepts only this type, making it
/// impossible to bypass the sandbox decision (D2 / spec A1).
#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    inner: ProcessCommand,
    tag: SandboxedTag,
}

/// Provenance of a [`SandboxedCommand`].
#[derive(Debug, Clone)]
pub enum SandboxedTag {
    /// Command was wrapped by a real sandbox backend.
    Wrapped {
        /// Which backend wrapped the command.
        backend: SandboxBackend,
    },
    /// Command was admitted via [`Sandbox::bypass_with_audit`]; the reason is
    /// recorded for the audit log.
    BypassAuditedWithReason {
        /// Human-readable justification.
        reason: String,
    },
}

impl SandboxedCommand {
    /// INTERNAL constructor for [`Sandbox`] implementations.
    ///
    /// Only [`Sandbox::prepare`] and [`Sandbox::bypass_with_audit`] impls
    /// should call this. The leading double-underscore is a convention to
    /// signal "do not call this from regular code". Regular code must go
    /// through the [`Sandbox`] trait so the policy / audit hook actually
    /// runs.
    #[must_use]
    pub fn __new_sandboxed(inner: ProcessCommand, tag: SandboxedTag) -> Self {
        Self { inner, tag }
    }

    /// Access the underlying [`ProcessCommand`] (for the runner to actually
    /// exec).
    #[must_use]
    pub fn inner(&self) -> &ProcessCommand {
        &self.inner
    }

    /// Read the provenance tag for audit / telemetry.
    #[must_use]
    pub fn tag(&self) -> &SandboxedTag {
        &self.tag
    }
}

#[cfg(test)]
mod m2_01_tests {
    use super::*;

    #[test]
    fn sandbox_error_unsupported_displays() {
        let e = SandboxError::Unsupported;
        assert_eq!(format!("{e}"), "sandbox not supported on this platform");
    }

    #[test]
    fn sandbox_error_unsupported_distinct_from_unavailable() {
        let u = SandboxError::Unsupported;
        let a = SandboxError::Unavailable("bwrap missing".into());
        assert!(!matches!(u, SandboxError::Unavailable(_)));
        assert!(matches!(a, SandboxError::Unavailable(_)));
    }

    #[test]
    fn android_minijail_backend_serde_roundtrip() {
        let b = SandboxBackend::AndroidMinijail;
        let json = serde_json::to_string(&b).expect("serialize");
        assert_eq!(json, "\"AndroidMinijail\"");
        let back: SandboxBackend = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, SandboxBackend::AndroidMinijail);
    }
}
