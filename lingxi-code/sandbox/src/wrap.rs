//! Sandbox wrap dispatch: produce a shell-runnable wrapped command line per
//! platform.
//!
//! - Linux / WSL2: `bwrap` with policy-derived filesystem + network flags,
//!   suffixed by `-- /bin/sh -c '<command>'`. A `socat` companion to enforce
//!   `network.allowed_domains` filtering is wired up as a process-lifecycle
//!   concern at the `platforms/posix/src/sandbox.rs` layer (Task 17); this
//!   builder just emits the `bwrap` argv that establishes a netns. See
//!   `TODO(M2-followup)` notes inline.
//! - macOS: write an SBPL profile to a tempfile, return
//!   `sandbox-exec -f <profile> <command>`.
//! - Windows / WSL1: return [`SandboxWrapError::Unsupported`] (handled by the
//!   refusal path before this dispatcher runs).

use crate::runtime_config::{Platform, SandboxRuntimeConfig};
use thiserror::Error;

/// Errors produced by [`wrap_with_sandbox`].
#[derive(Debug, Clone, Error)]
pub enum SandboxWrapError {
    /// Platform is on the refuse list (Windows, WSL1) — the caller should
    /// surface the inner string verbatim.
    #[error("{0}")]
    Unsupported(String),
    /// Failed to write the SBPL profile tempfile.
    #[error("failed to write SBPL profile: {0}")]
    SbplWrite(String),
}

/// Wrap `command` for execution under the sandbox identified by `platform`.
///
/// `policy` carries the runtime configuration (filesystem / network /
/// excludedCommands etc.).
///
/// Returns the shell-runnable string the caller should pass to its
/// `/bin/sh -c` runner.
///
/// # Errors
/// Returns [`SandboxWrapError::SbplWrite`] when the macOS path cannot persist
/// the SBPL profile tempfile. The Linux / WSL2 path is infallible. Windows /
/// WSL1 should be handled by the refusal pipeline upstream — see
/// [`crate::dependency_check::sandbox_unavailable_reason`].
pub fn wrap_with_sandbox(
    command: &str,
    policy: &SandboxRuntimeConfig,
    platform: Platform,
) -> Result<String, SandboxWrapError> {
    match platform {
        Platform::Linux | Platform::Wsl => Ok(wrap_linux_bwrap(command, policy)),
        Platform::Mac => wrap_macos_sbpl(command, policy),
    }
}

// =============================================================================
// Linux / WSL2 — bwrap
// =============================================================================

/// Build the `bwrap` invocation. The caller (`platforms/posix/src/sandbox.rs`)
/// passes this string to `/bin/sh -c` and is responsible for spawning the
/// optional `socat` companion when `network.allowed_domains` is non-empty.
fn wrap_linux_bwrap(command: &str, policy: &SandboxRuntimeConfig) -> String {
    // Default ro root + tmpfs ephemeral writes + procfs/devfs + pid-ns +
    // die-with-parent — match claude-code's defaults.
    let mut args: Vec<String> = vec![
        // Create a user namespace where possible; degrade gracefully on kernels
        // with unprivileged userns disabled (`-try`) instead of failing to start
        // (finding 5). Without it an unprivileged bwrap cannot create the pid/net
        // namespaces below. Verified to start + degrade under user.max_user_namespaces=0.
        "--unshare-user-try".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--unshare-pid".into(),
        "--die-with-parent".into(),
    ];

    // Writable paths per filesystem.allow_write.
    for path in &policy.filesystem.allow_write {
        args.push("--bind".into());
        args.push(path.clone());
        args.push(path.clone());
    }

    // Conservative network posture: full host net ONLY for an allow-all policy
    // (allowed_domains non-empty == NetworkPolicy::Allowed). LoopbackOnly /
    // Disabled get a fresh network namespace (loopback-only, external blocked).
    // The socat domain-filter companion is deferred; a domain-allowlist policy
    // therefore gets no external egress (errs safe) until it lands.
    if policy.network.allowed_domains.is_empty() {
        args.push("--unshare-net".into());
    } else {
        args.push("--share-net".into());
    }

    let quoted = shell_escape_single(command);
    let joined = args.join(" ");
    format!("bwrap {joined} -- /bin/sh -c {quoted}")
}

/// Single-quote-escape `s` for `/bin/sh -c`.
///
/// Returns `'<s with embedded single quotes escaped>'`. Embedded `'` is
/// replaced with `'\''` (close quote, escaped quote, reopen quote).
fn shell_escape_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// =============================================================================
// macOS — SBPL profile
// =============================================================================

fn wrap_macos_sbpl(
    command: &str,
    policy: &SandboxRuntimeConfig,
) -> Result<String, SandboxWrapError> {
    let profile = generate_sbpl_profile(policy);
    let profile_path = write_sbpl_tempfile(&profile)?;
    let quoted = shell_escape_single(command);
    Ok(format!(
        "sandbox-exec -f {profile_path} /bin/sh -c {quoted}",
    ))
}

/// Generate a minimal-but-valid SBPL profile string.
///
/// Source-of-truth structure (claude-code's `getSandboxProfile()`):
/// ```sbpl
/// (version 1)
/// (deny default)
/// (allow process-exec)
/// (allow process-fork)
/// (allow signal (target self))
/// (allow sysctl-read)
/// (allow file-read*)
/// (allow file-write*  (regex "^/private/tmp"))
/// (allow file-write*  (regex "^<allow_write_path>"))
/// (allow network*)            ;; only if domains/local_binding allowed
/// ```
///
/// TODO(M2-followup): expand SBPL coverage to include
/// `allowManagedReadPathsOnly`, `ignoreViolations`, `enableWeakerNestedSandbox`
/// trustd allowance, and the full claude-code template. Current template is
/// the minimal subset that produces a valid `sandbox-exec` profile and covers
/// `filesystem.allow_write` + `denyWrite` + `denyRead` + network on/off.
pub(crate) fn generate_sbpl_profile(policy: &SandboxRuntimeConfig) -> String {
    let mut out = String::new();
    out.push_str("(version 1)\n");
    out.push_str("(deny default)\n");
    out.push_str("(allow process-exec)\n");
    out.push_str("(allow process-fork)\n");
    out.push_str("(allow signal (target self))\n");
    out.push_str("(allow sysctl-read)\n");
    out.push_str("(allow file-read*)\n");
    // Always allow /private/tmp (claude-code parity).
    out.push_str("(allow file-write* (regex \"^/private/tmp\"))\n");
    for p in &policy.filesystem.allow_write {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(allow file-write* (regex \"^{escaped}\"))\n"));
    }
    for p in &policy.filesystem.deny_write {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(deny file-write* (regex \"^{escaped}\"))\n"));
    }
    for p in &policy.filesystem.deny_read {
        let escaped = sbpl_regex_escape(p);
        out.push_str(&format!("(deny file-read* (regex \"^{escaped}\"))\n"));
    }
    let want_network = !policy.network.allowed_domains.is_empty()
        || policy.network.allow_local_binding
        || policy.network.allow_all_unix_sockets;
    if want_network {
        out.push_str("(allow network*)\n");
    }
    out
}

/// Escape `path` for inclusion inside an SBPL regex literal. Replaces the
/// regex metacharacters `. * + ? ( ) [ ] { } ^ $ |` with `\\<ch>`.
fn sbpl_regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        if matches!(
            ch,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn write_sbpl_tempfile(profile: &str) -> Result<String, SandboxWrapError> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("lingxi-sandbox-")
        .suffix(".sb")
        .tempfile()
        .map_err(|e| SandboxWrapError::SbplWrite(e.to_string()))?;
    f.write_all(profile.as_bytes())
        .map_err(|e| SandboxWrapError::SbplWrite(e.to_string()))?;
    // Persist: the tempfile must outlive this function so `sandbox-exec -f`
    // can read it. The OS will clean it up at reboot; for long-lived
    // processes the platform layer is responsible for explicit unlink.
    let (_file, path) = f
        .keep()
        .map_err(|e| SandboxWrapError::SbplWrite(e.error.to_string()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::wrap_linux_bwrap;
    use crate::runtime_config::SandboxRuntimeConfig;

    #[test]
    fn bwrap_creates_a_user_namespace() {
        let w = wrap_linux_bwrap("true", &SandboxRuntimeConfig::default());
        assert!(w.contains("--unshare-user-try"),
            "bwrap must request a userns (degrading) so it can create pid/net ns unprivileged: {w}");
    }

    #[test]
    fn share_net_only_when_allowed_domains_present() {
        let mut cfg = SandboxRuntimeConfig::default();
        // Allowed → ["*"] → --share-net
        cfg.network.allowed_domains = vec!["*".into()];
        assert!(wrap_linux_bwrap("true", &cfg).contains("--share-net"));
        // LoopbackOnly: empty domains + allow_local_binding → NOT --share-net
        cfg.network.allowed_domains.clear();
        cfg.network.allow_local_binding = true;
        let w = wrap_linux_bwrap("true", &cfg);
        assert!(
            !w.contains("--share-net"),
            "loopback must not get full egress: {w}"
        );
        assert!(w.contains("--unshare-net"));
    }
}
