//! `Sandbox` trait impl — POSIX policy-validating no-op.
//!
//! M2.02 ships type-safe sandbox plumbing only. Real isolation (Linux user
//! namespaces, macOS `sandbox-exec`) is a follow-up. Production code MUST
//! still route through [`Sandbox::prepare`] so the [`SandboxedCommand`] type
//! invariant from D2 is preserved.

use async_trait::async_trait;
use lingxi_traits::{
    ProcessCommand, Sandbox, SandboxBackend, SandboxCapability, SandboxError, SandboxFeatures,
    SandboxPolicy, SandboxedCommand, SandboxedTag,
};

/// Policy-validating no-op sandbox for POSIX desktops.
///
/// Validates `SandboxPolicy.denied_paths` against the requested cwd and
/// flags symlink escape under the workspace, but does NOT yet wrap the
/// child process in any OS isolation primitive.
#[derive(Default)]
pub struct PosixSandbox;

impl PosixSandbox {
    /// Construct a new `PosixSandbox`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Sandbox for PosixSandbox {
    fn is_available(&self) -> bool {
        true
    }

    fn backend(&self) -> SandboxBackend {
        SandboxBackend::None
    }

    fn prepare(
        &self,
        cmd: ProcessCommand,
        policy: &SandboxPolicy,
    ) -> Result<SandboxedCommand, SandboxError> {
        // Validate cwd is not inside a denied path. Real backends will also
        // canonicalize and apply the network / writable_paths policies.
        if let Some(cwd) = &cmd.cwd {
            for denied in &policy.denied_paths {
                if cwd.starts_with(denied) {
                    return Err(SandboxError::SymlinkEscape(cwd.display().to_string()));
                }
            }
        }
        Ok(SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::Wrapped {
                backend: SandboxBackend::None,
            },
        ))
    }

    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
        SandboxedCommand::__new_sandboxed(
            cmd,
            SandboxedTag::BypassAuditedWithReason {
                reason: reason.to_string(),
            },
        )
    }

    async fn probe_capability(&self) -> SandboxCapability {
        SandboxCapability {
            available: false,
            reason: Some(
                "M2.02 posix sandbox is a policy-validating no-op; OS isolation lands in a follow-up"
                    .into(),
            ),
            features: SandboxFeatures::default(),
        }
    }
}
