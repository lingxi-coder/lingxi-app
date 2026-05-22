//! Sandbox decision engine + reusable policy helpers.
//!
//! `lingxi-sandbox` does not ship a backend (those live in platform crates).
//! It provides:
//!
//! - [`policy::default_policy`] — the conservative policy used when callers
//!   don't supply one explicitly.
//! - [`decision::should_use_sandbox`] — the decision matrix used by the
//!   tool layer to pick between sandbox / no-sandbox / refusal.
//! - [`canonicalize_safely`] — a symlink-escape-safe path canonicalization
//!   helper used by backend impls.
//!
//! See spec §24 (Sandbox).

#![forbid(unsafe_code)]

pub mod decision;
pub mod policy;

pub use decision::{
    is_obviously_dangerous, should_use_sandbox, ProjectTrustLevel, SandboxDecision,
};
pub use lingxi_traits::{
    NetworkPolicy, ResourceLimits, Sandbox, SandboxBackend, SandboxError, SandboxPolicy,
    SandboxedCommand, SandboxedTag,
};
pub use policy::default_policy;

/// Canonicalize `path` and verify the result stays inside `workspace`.
///
/// Used by platform sandbox impls (spec A2) before mounting writable paths
/// into a sandbox: the kernel resolves symlinks at `realpath` time, so any
/// link that escapes the workspace would otherwise allow a sandboxed
/// process to write outside the project.
///
/// # Errors
/// Returns [`lingxi_traits::SandboxError::PathCanonicalize`] if the kernel
/// canonicalization itself fails, or
/// [`lingxi_traits::SandboxError::SymlinkEscape`] when the canonical path
/// resolves outside `workspace`.
pub fn canonicalize_safely(
    path: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<std::path::PathBuf, lingxi_traits::SandboxError> {
    let canon = path
        .canonicalize()
        .map_err(|e| lingxi_traits::SandboxError::PathCanonicalize(e.to_string()))?;
    if !canon.starts_with(workspace) {
        return Err(lingxi_traits::SandboxError::SymlinkEscape(
            path.display().to_string(),
        ));
    }
    Ok(canon)
}
