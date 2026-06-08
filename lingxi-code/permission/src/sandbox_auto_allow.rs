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
//! `sandbox::decision::should_use_sandbox_for_command` + its helpers
//! (`split_compound_command`, `strip_env_and_wrappers_fixedpoint`,
//! `matches_excluded`, `BINARY_HIJACK_VARS`), so the two stay in lockstep.
//!
//! # Population
//! [`SandboxAutoAllowConfig`] is attached to a [`crate::policy::PermissionPolicy`]
//! via `with_sandbox_runtime`. When ABSENT (the default), the auto-allow layer
//! is a no-op — behavior is unchanged, preserving the opt-in posture of the
//! whole enforcement path. When present and `enabled`, a command that WOULD be
//! sandboxed ([`SandboxAutoAllowConfig::would_sandbox`]) and that matched no
//! explicit deny/ask rule is auto-allowed (the sandbox is the safety boundary).

use std::collections::BTreeSet;

/// Env-vars an attacker could use to redirect binary lookup — 1:1 with
/// claude-code `BINARY_HIJACK_VARS` and the `sandbox` crate's copy.
const BINARY_HIJACK_VARS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
];

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
        for subcommand in split_compound_command(command) {
            for cand in strip_env_and_wrappers_fixedpoint(&subcommand) {
                for pattern in &self.excluded_commands {
                    if matches_excluded(pattern, &cand) {
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

/// Split `command` on `&&`, `||`, and `;` — 1:1 with the `sandbox` crate's
/// `split_compound_command` (claude-code `splitCommand_DEPRECATED`, quote-naive
/// for the excludedCommands heuristic).
fn split_compound_command(command: &str) -> Vec<String> {
    let normalized = command
        .replace("&&", "\u{1}")
        .replace("||", "\u{1}")
        .replace(';', "\u{1}");
    normalized
        .split('\u{1}')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// One pass of safe-wrapper stripping — 1:1 with the `sandbox` crate's
/// `strip_safe_wrappers`.
fn strip_safe_wrappers(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // sudo -E [KEY=VAL ...] --
    if tokens.len() >= 4 && tokens[0] == "sudo" && tokens[1] == "-E" {
        let mut i = 2;
        while i < tokens.len() && tokens[i] != "--" {
            if !tokens[i].contains('=') {
                break;
            }
            i += 1;
        }
        if i < tokens.len() && tokens[i] == "--" && i + 1 < tokens.len() {
            let mut rest: Vec<&str> = Vec::with_capacity(tokens.len());
            rest.extend_from_slice(&tokens[2..i]);
            rest.extend_from_slice(&tokens[i + 1..]);
            return Some(rest.join(" "));
        }
    }
    // sudo --
    if tokens.len() >= 3 && tokens[0] == "sudo" && tokens[1] == "--" {
        return Some(tokens[2..].join(" "));
    }
    // env --
    if tokens.len() >= 3 && tokens[0] == "env" && tokens[1] == "--" {
        return Some(tokens[2..].join(" "));
    }
    // timeout <N>
    if tokens.len() >= 3 && tokens[0] == "timeout" && is_numeric(tokens[1]) {
        return Some(tokens[2..].join(" "));
    }
    // nice -n <N>
    if tokens.len() >= 4 && tokens[0] == "nice" && tokens[1] == "-n" && is_numeric(tokens[2]) {
        return Some(tokens[3..].join(" "));
    }
    None
}

/// Strip leading `KEY=value` tokens where `KEY` is a `BINARY_HIJACK_VARS` entry
/// — 1:1 with the `sandbox` crate's `strip_binary_hijack_env_vars`.
fn strip_binary_hijack_env_vars(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut start = 0;
    while start < tokens.len() {
        let tok = tokens[start];
        if let Some(eq) = tok.find('=') {
            let key = &tok[..eq];
            if BINARY_HIJACK_VARS.contains(&key) {
                start += 1;
                continue;
            }
        }
        break;
    }
    if start == 0 {
        None
    } else {
        Some(tokens[start..].join(" "))
    }
}

/// Iteratively apply the wrapper + env strippers to a fixed point — 1:1 with the
/// `sandbox` crate's `strip_env_and_wrappers_fixedpoint`.
fn strip_env_and_wrappers_fixedpoint(cmd: &str) -> Vec<String> {
    let mut candidates: Vec<String> = vec![cmd.trim().to_string()];
    let mut seen: BTreeSet<String> = candidates.iter().cloned().collect();
    let mut start = 0;
    while start < candidates.len() {
        let end = candidates.len();
        for i in start..end {
            let c = candidates[i].clone();
            if let Some(env_stripped) = strip_binary_hijack_env_vars(&c) {
                if seen.insert(env_stripped.clone()) {
                    candidates.push(env_stripped);
                }
            }
            if let Some(wrap_stripped) = strip_safe_wrappers(&c) {
                if seen.insert(wrap_stripped.clone()) {
                    candidates.push(wrap_stripped);
                }
            }
        }
        start = end;
    }
    candidates
}

/// `excluded_commands` pattern match — 1:1 with the `sandbox` crate's
/// `matches_excluded`: `bazel:*` matches `bazel` / `bazel …`; a bare `bazel`
/// matches exact or first-token.
fn matches_excluded(pattern: &str, candidate: &str) -> bool {
    let trimmed = candidate.trim();
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return trimmed == prefix || trimmed.starts_with(&format!("{prefix} "));
    }
    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    trimmed == pattern || first_token == pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(excluded: &[&str]) -> SandboxAutoAllowConfig {
        SandboxAutoAllowConfig::new(true, true, excluded.iter().map(|s| (*s).to_string()).collect())
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
        let c = cfg(&["bazel:*", "make"]);
        assert!(!c.would_sandbox("bazel build //..."));
        assert!(!c.would_sandbox("make all"));
        assert!(!c.auto_allows("bazel build //..."));
        // a non-excluded command IS sandboxed
        assert!(c.would_sandbox("echo hi"));
        assert!(c.auto_allows("echo hi"));
    }

    #[test]
    fn exclusion_survives_env_and_wrapper_wrapping() {
        // An excluded command hidden behind a hijack env-var or a safe wrapper is
        // still recognized as excluded (so NOT sandboxed / NOT auto-allowed).
        let c = cfg(&["bazel:*"]);
        assert!(!c.would_sandbox("PATH=/evil bazel build"));
        assert!(!c.would_sandbox("timeout 5 bazel build"));
    }

    #[test]
    fn compound_with_excluded_subcommand_is_not_sandboxed() {
        // If ANY subcommand is excluded, the whole compound is not sandboxed.
        let c = cfg(&["make"]);
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
