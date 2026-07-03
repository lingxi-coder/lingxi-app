//! Sandbox auto-allow decision for the bash permission gate — a faithful,
//! crate-local mirror of the parts of claude-code
//! `src/tools/BashTool/shouldUseSandbox.ts` that `bashToolHasPermission`'s
//! sandbox-auto-allow branch needs (`SandboxManager.isSandboxingEnabled()` +
//! `isAutoAllowBashIfSandboxedEnabled()` + `shouldUseSandbox(input)`).
//!
//! # Why a crate-local mirror (not a `sandbox` dep)
//! The real config + decision live in the `sandbox` crate
//! (`sandbox::runtime_config::SandboxRuntimeConfig` /
//! `sandbox::decision::should_use_sandbox_for_command`). But `sandbox` already
//! depends on `permission` (for `PermissionMode`), so `permission` cannot depend
//! on `sandbox` without a dependency CYCLE — and the parity effort forbids
//! adding a new external/internal edge here. So this module re-implements the
//! SAME decision over a MINIMAL config carrying only the three fields the
//! auto-allow branch reads (`enabled`, `auto_allow_bash_if_sandboxed`,
//! `excluded_commands`). The logic is byte-for-byte the same as
//! `sandbox::decision::should_use_sandbox_for_command` because BOTH call the
//! SAME shared `excludedCommands` core in `crate::shell_command`
//! (`split_command`, `strip_env_and_wrappers_fixedpoint`,
//! `strip_all_leading_env_vars`/`is_binary_hijack_var`, `strip_safe_wrappers`)
//! plus `crate::shell_rule_matching` rule dispatch, so the two cannot drift.
//!
//! # Population
//! [`SandboxAutoAllowConfig`] is attached to a [`crate::policy::PermissionPolicy`]
//! via `with_sandbox_runtime`. When ABSENT (the default), the auto-allow layer
//! is a no-op — behavior is unchanged, preserving the opt-in posture of the
//! whole enforcement path. When present and `enabled`, a command that WOULD be
//! sandboxed ([`SandboxAutoAllowConfig::would_sandbox`]) and that matched no
//! explicit deny/ask rule is auto-allowed (the sandbox is the safety boundary).

/// Minimal sandbox-runtime config the bash auto-allow branch consults — the
/// three fields of `sandbox::runtime_config::SandboxRuntimeConfig` that
/// `bashToolHasPermission`'s sandbox-auto-allow guard reads. Construct one at
/// the engine boot site from the real `SandboxRuntimeConfig` and attach it via
/// [`crate::policy::PermissionPolicy::with_sandbox_runtime`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxAutoAllowConfig {
    /// `SandboxRuntimeConfig.enabled` — master sandbox toggle
    /// (`SandboxManager.isSandboxingEnabled()`). When `false`, the auto-allow
    /// branch never fires.
    pub enabled: bool,
    /// `SandboxRuntimeConfig.autoAllowBashIfSandboxed`
    /// (`SandboxManager.isAutoAllowBashIfSandboxedEnabled()`, default `true` in
    /// claude-code). When `false`, a sandboxable command is NOT auto-allowed.
    pub auto_allow_bash_if_sandboxed: bool,
    /// `SandboxRuntimeConfig.excludedCommands` — commands that run OUTSIDE the
    /// sandbox even when enabled (e.g. `bazel`, `make`). A command matching any
    /// of these is NOT sandboxed, so it is NOT auto-allowed.
    pub excluded_commands: Vec<String>,
}

impl SandboxAutoAllowConfig {
    /// Build the minimal config from the three relevant fields. Mirrors copying
    /// them out of a `sandbox::runtime_config::SandboxRuntimeConfig`.
    #[must_use]
    pub fn new(
        enabled: bool,
        auto_allow_bash_if_sandboxed: bool,
        excluded_commands: Vec<String>,
    ) -> Self {
        Self {
            enabled,
            auto_allow_bash_if_sandboxed,
            excluded_commands,
        }
    }

    /// Would `command` be sandbox-wrapped under this config? — 1:1 with
    /// claude-code `shouldUseSandbox` / the `sandbox` crate's
    /// `should_use_sandbox_for_command`: `true` iff `enabled` AND no subcommand
    /// (after compound split + env/wrapper fixed-point stripping) matches any
    /// `excluded_commands` entry. An empty `excluded_commands` ⇒ everything is
    /// sandboxed (returns `true` whenever enabled).
    #[must_use]
    pub fn would_sandbox(&self, command: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if self.excluded_commands.is_empty() {
            return true;
        }
        for subcommand in crate::shell_command::split_command(command) {
            for cand in crate::shell_command::strip_env_and_wrappers_fixedpoint(&subcommand) {
                for pattern in &self.excluded_commands {
                    if crate::shell_command::matches_excluded_pattern(pattern, &cand) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// The full auto-allow predicate guard: sandboxing enabled AND
    /// auto-allow-if-sandboxed AND the command would be sandboxed. Mirrors the
    /// `isSandboxingEnabled() && isAutoAllowBashIfSandboxedEnabled() &&
    /// shouldUseSandbox(input)` conjunction at the top of `bashToolHasPermission`.
    #[must_use]
    pub fn auto_allows(&self, command: &str) -> bool {
        self.enabled && self.auto_allow_bash_if_sandboxed && self.would_sandbox(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(excluded: &[&str]) -> SandboxAutoAllowConfig {
        SandboxAutoAllowConfig::new(
            true,
            true,
            excluded.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    #[test]
    fn disabled_never_sandboxes() {
        let c = SandboxAutoAllowConfig::new(false, true, vec![]);
        assert!(!c.would_sandbox("echo hi"));
        assert!(!c.auto_allows("echo hi"));
    }

    #[test]
    fn enabled_no_excludes_sandboxes_everything() {
        let c = cfg(&[]);
        assert!(c.would_sandbox("echo hi"));
        assert!(c.would_sandbox("rm -rf /tmp/x"));
        assert!(c.auto_allows("echo hi"));
    }

    #[test]
    fn excluded_command_is_not_sandboxed() {
        // `make:*` is Prefix (excludes `make all`); a bare `make` Exact would only
        // exclude the literal `make` (strict — no longer first-token), so the
        // arg-bearing form below uses the prefix rule.
        let c = cfg(&["bazel:*", "make:*"]);
        assert!(!c.would_sandbox("bazel build //..."));
        assert!(!c.would_sandbox("make all"));
        assert!(!c.auto_allows("bazel build //..."));
        // a non-excluded command IS sandboxed
        assert!(c.would_sandbox("echo hi"));
        assert!(c.auto_allows("echo hi"));
    }

    #[test]
    fn auto_allow_exact_strict_and_wildcard() {
        // Strict Exact: a bare `bazel` rule no longer excludes `bazel build`
        // (first-token over-match removed), so it WOULD be sandboxed now.
        assert!(cfg(&["bazel"]).would_sandbox("bazel build"));
        // Prefix `bazel:*` still excludes `bazel build` (NOT sandboxed).
        assert!(!cfg(&["bazel:*"]).would_sandbox("bazel build"));
        // Wildcard `make *` (trailing ` *` optional) excludes bare `make`.
        assert!(!cfg(&["make *"]).would_sandbox("make"));
        assert!(!cfg(&["make *"]).would_sandbox("make all"));
    }

    #[test]
    fn exclusion_through_safe_wrapper_and_non_hijack_env() {
        // Faithful claude-code semantics (stripAllLeadingEnvVars with
        // BINARY_HIJACK_VARS blocklist + stripSafeWrappers):
        let c = cfg(&["bazel:*"]);
        // A non-hijack env prefix IS stripped → recognized as excluded → NOT sandboxed.
        assert!(!c.would_sandbox("FOO=bar bazel build"));
        // A SAFE_ENV_VARS prefix is stripped by stripSafeWrappers phase-1 → excluded.
        assert!(!c.would_sandbox("GOOS=linux bazel build"));
        // A safe wrapper is stripped → excluded → NOT sandboxed.
        assert!(!c.would_sandbox("timeout 5 bazel build"));
        assert!(!c.would_sandbox("nohup bazel build"));
        // A binary-hijack env prefix (PATH) makes stripAllLeadingEnvVars BREAK,
        // so `bazel build` is never exposed → still SANDBOXED.
        assert!(c.would_sandbox("PATH=/evil bazel build"));
        // LD_* matches /^LD_/ → still SANDBOXED.
        assert!(c.would_sandbox("LD_AUDIT=x bazel build"));
        // sudo/env are NOT safe wrappers (not in SAFE_WRAPPER_PATTERNS) → still SANDBOXED.
        assert!(c.would_sandbox("sudo bazel build"));
        assert!(c.would_sandbox("env bazel build"));
    }

    #[test]
    fn compound_with_excluded_subcommand_is_not_sandboxed() {
        // If ANY subcommand is excluded, the whole compound is not sandboxed.
        // (`make:*` Prefix excludes `make all`; a bare `make` Exact would not,
        // now that first-token over-match is removed.)
        let c = cfg(&["make:*"]);
        assert!(!c.would_sandbox("echo ok && make all"));
    }

    #[test]
    fn auto_allow_flag_gates() {
        // enabled but auto-allow OFF → would_sandbox true, but auto_allows false.
        let c = SandboxAutoAllowConfig::new(true, false, vec![]);
        assert!(c.would_sandbox("echo hi"));
        assert!(!c.auto_allows("echo hi"));
    }
}
