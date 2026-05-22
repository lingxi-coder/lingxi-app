//! Sandbox decision logic.
//!
//! [`should_use_sandbox`] is the single source of truth that decides
//! whether a command should be wrapped, executed unsandboxed, or refused
//! outright when the host has no sandbox available. It consults the
//! permission mode, project trust, classifier verdict, and host capability.
//!
//! See spec §24.3 (sandbox decision matrix).

use lingxi_permission::PermissionMode;
use lingxi_traits::SandboxPolicy;

/// Outcome of [`should_use_sandbox`].
#[derive(Debug, Clone)]
pub enum SandboxDecision {
    /// Run the command directly with no sandbox wrapping.
    NoSandbox,
    /// Wrap the command using the supplied policy.
    Sandbox {
        /// Policy to apply when calling [`lingxi_traits::Sandbox::prepare`].
        policy: SandboxPolicy,
    },
    /// Host cannot provide a sandbox but the command is dangerous — refuse.
    RefuseBecauseSandboxUnavailable {
        /// Human-readable explanation for the refusal.
        reason: String,
    },
}

/// Whether the current project has been explicitly trusted by the user.
///
/// Trusted projects skip the sandbox for classifier-safe commands; untrusted
/// projects always go through the sandbox when available.
#[derive(Debug, Clone, Copy)]
pub enum ProjectTrustLevel {
    /// User has marked this workspace as trusted.
    Trusted,
    /// Workspace is unfamiliar or explicitly untrusted.
    Untrusted,
}

/// Decide whether to sandbox a command, run it directly, or refuse it.
///
/// Inputs:
/// - `cmd`: full command line (used for dangerous-pattern checks).
/// - `mode`: active permission mode.
/// - `trust`: project trust level.
/// - `classifier_safe`: optional verdict from the safety classifier
///   (`Some(true)` = safe, `Some(false)` = unsafe, `None` = unknown).
/// - `sandbox_available`: whether the host actually has a working sandbox.
/// - `workspace`: project workspace path, used to build a default policy.
#[must_use]
pub fn should_use_sandbox(
    cmd: &str,
    mode: PermissionMode,
    trust: ProjectTrustLevel,
    classifier_safe: Option<bool>,
    sandbox_available: bool,
    workspace: std::path::PathBuf,
) -> SandboxDecision {
    if matches!(mode, PermissionMode::BypassPermissions) {
        return SandboxDecision::NoSandbox;
    }
    if matches!(mode, PermissionMode::Plan) {
        return SandboxDecision::NoSandbox;
    }
    let dangerous = is_obviously_dangerous(cmd);
    if dangerous && !sandbox_available {
        return SandboxDecision::RefuseBecauseSandboxUnavailable {
            reason: "command flagged dangerous and no sandbox backend available".into(),
        };
    }
    if matches!(trust, ProjectTrustLevel::Trusted) && classifier_safe == Some(true) && !dangerous {
        return SandboxDecision::NoSandbox;
    }
    if !sandbox_available {
        return SandboxDecision::NoSandbox;
    }
    SandboxDecision::Sandbox {
        policy: crate::policy::default_policy(workspace),
    }
}

/// Coarse pattern check for obviously destructive commands.
///
/// This is intentionally a deny-list and not a substitute for the
/// classifier — it only catches the worst offenders (`rm -rf /`, `sudo`,
/// `chmod 777`, naked `curl`, fork bombs) so the decision matrix can
/// trip the "refuse" branch when no sandbox is available.
#[must_use]
pub fn is_obviously_dangerous(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    ["rm -rf /", "sudo ", "chmod 777", "curl", "fork bomb"]
        .iter()
        .any(|p| lower.contains(p))
}
