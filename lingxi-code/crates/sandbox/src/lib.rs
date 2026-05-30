//! Sandbox decision engine + reusable policy helpers.
//!
//! `lingxi-sandbox` does not ship a backend (those live in platform crates).
//! It provides:
//!
//! - [`policy::default_policy`] — the conservative policy used when callers
//!   don't supply one explicitly.
//! - [`decision::should_use_sandbox`] — the M1 decision matrix used by the
//!   tool layer to pick between sandbox / no-sandbox / refusal.
//! - [`decision::should_use_sandbox_for_command`] — the M2 decision that
//!   consults `SandboxRuntimeConfig::excluded_commands` after
//!   compound-command splitting and safe-wrapper / env-var fixed-point
//!   stripping (port of claude-code's `shouldUseSandbox.ts`).
//! - [`dependency_check::check_dependencies`] — host probe for required
//!   sandbox binaries (`sandbox-exec`, `bwrap`, `socat`).
//! - [`dependency_check::sandbox_unavailable_reason`] — decoder for the
//!   five claude-code byte-for-byte error strings surfaced when sandbox
//!   can't run.
//! - [`violation_store::SandboxViolationStore`] — bounded ring buffer of
//!   sandbox violation events surfaced by the backend.
//! - [`canonicalize_safely`] — a symlink-escape-safe path canonicalization
//!   helper used by backend impls.
//!
//! See spec §24 (Sandbox).

#![forbid(unsafe_code)]

pub mod decision;
pub mod dependency_check;
pub mod path_pattern;
pub mod policy;
pub mod policy_convert;
pub mod runtime_config;
pub mod violation_store;
pub mod wrap;

pub use decision::{
    is_obviously_dangerous, should_use_sandbox, should_use_sandbox_for_command,
    split_compound_command, strip_env_and_wrappers_fixedpoint, ProjectTrustLevel, SandboxDecision,
    BINARY_HIJACK_VARS,
};
pub use dependency_check::{
    check_dependencies, sandbox_unavailable_reason, MissingDeps, SandboxDependencyCheck,
};
pub use lingxi_traits::{
    NetworkPolicy, ResourceLimits, Sandbox, SandboxBackend, SandboxError, SandboxPolicy,
    SandboxedCommand, SandboxedTag,
};
pub use path_pattern::resolve_path_pattern_for_sandbox;
pub use policy::default_policy;
pub use policy_convert::{convert_settings_to_runtime_config, linux_glob_pattern_warnings};
pub use runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, Platform, RipgrepConfig,
    SandboxRuntimeConfig, SandboxSettingsJson, SettingsJson, SettingsPermissions,
};
pub use violation_store::{
    SandboxViolationEvent, SandboxViolationKind, SandboxViolationStore, SANDBOX_VIOLATION_STORE_CAP,
};
pub use wrap::{wrap_with_sandbox, SandboxWrapError};

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
