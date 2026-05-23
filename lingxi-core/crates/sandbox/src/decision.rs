//! Sandbox decision logic.
//!
//! [`should_use_sandbox`] is the single source of truth that decides
//! whether a command should be wrapped, executed unsandboxed, or refused
//! outright when the host has no sandbox available. It consults the
//! permission mode, project trust, classifier verdict, and host capability.
//!
//! [`should_use_sandbox_for_command`] is the M2 addition: a
//! `SandboxRuntimeConfig`-aware function that consults the
//! `excludedCommands` list after compound-command splitting and
//! safe-wrapper / env-var fixed-point stripping. Ports the logic from
//! claude-code `src/tools/BashTool/shouldUseSandbox.ts`.
//!
//! See spec §24.3 (sandbox decision matrix).

use crate::runtime_config::SandboxRuntimeConfig;
use lingxi_permission::PermissionMode;
use lingxi_traits::SandboxPolicy;
use std::collections::BTreeSet;

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

// =============================================================================
// Compound-command splitting + env-var / safe-wrapper fixed-point stripping.
//
// Ports the logic from `claude-code/src/tools/BashTool/shouldUseSandbox.ts` and
// `bashPermissions.ts` (`BINARY_HIJACK_VARS`, `stripAllLeadingEnvVars`,
// `stripSafeWrappers`).
// =============================================================================

/// Env-vars an attacker could use to redirect binary lookup. Matches the
/// claude-code constant `BINARY_HIJACK_VARS` exactly.
pub const BINARY_HIJACK_VARS: &[&str] = &[
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
];

/// Wrappers safe to strip when matching against excludedCommands patterns.
/// Each entry is a prefix + an arity hint:
/// - `"sudo -E --"` exact: strip leading 3 tokens.
/// - `"sudo --"` exact: strip leading 2 tokens.
/// - `"env --"` exact: strip leading 2 tokens.
/// - `"timeout <N>"`: 2 tokens (the wrapper + its single numeric arg).
/// - `"nice -n <N>"`: 3 tokens.
fn strip_safe_wrappers(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // sudo -E [KEY=VAL ...] --
    // Accepts env-var assignments between `-E` and `--` (claude-code's
    // safe-wrapper stripper skips `KEY=val` tokens here so the fixed-point
    // outer loop can then strip the env-vars separately).
    if tokens.len() >= 4 && tokens[0] == "sudo" && tokens[1] == "-E" {
        // Find the `--` separator after `-E`.
        let mut i = 2;
        while i < tokens.len() && tokens[i] != "--" {
            // Stop at first non-KEY=val token (defensive: don't skip arbitrary args).
            if !tokens[i].contains('=') {
                break;
            }
            i += 1;
        }
        if i < tokens.len() && tokens[i] == "--" && i + 1 < tokens.len() {
            // Emit `sudo -E` removed but env-vars retained so the env-var
            // stripper can take a second pass on the result.
            let mut rest: Vec<&str> = Vec::with_capacity(tokens.len() - (i + 1) + (i - 2));
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

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Strip leading `KEY=value` tokens where `KEY` is a `BINARY_HIJACK_VARS`
/// entry. Non-binary-hijack env-vars (`FOO=bar`) are left in place.
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

/// Iteratively apply `strip_safe_wrappers` and `strip_binary_hijack_env_vars`
/// until no new candidate is produced (fixed point).
///
/// Returns the deduped list of candidates (the original `cmd` is included).
#[must_use]
pub fn strip_env_and_wrappers_fixedpoint(cmd: &str) -> Vec<String> {
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

/// Split `command` on `&&`, `||`, and `;` into one entry per subcommand.
///
/// Pure delimiter-based split. Does NOT handle quoting (claude-code's
/// `splitCommand_DEPRECATED` likewise is quote-naive; that's why it's marked
/// `_DEPRECATED`). Sufficient for the excludedCommands match heuristic.
#[must_use]
pub fn split_compound_command(command: &str) -> Vec<String> {
    // Replace operators with a single delimiter sentinel, then split.
    // Order matters: `&&` and `||` are two-char ops; `;` is one-char.
    let normalized = command
        .replace("&&", "\x01")
        .replace("||", "\x01")
        .replace(';', "\x01");
    normalized
        .split('\x01')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Decide whether `command` should be sandbox-wrapped according to `config`.
///
/// Returns `true` iff:
/// - `config.enabled` is true, AND
/// - no subcommand (after compound split) — after iterative env-var +
///   safe-wrapper fixed-point stripping — matches any entry in
///   `config.excluded_commands`.
///
/// Pattern semantics for `excluded_commands` (claude-code):
/// - `bazel` matches exact command (or any candidate first token = `bazel`).
/// - `bazel:*` matches any command starting with `bazel ` (including `bazel`
///   on its own).
#[must_use]
pub fn should_use_sandbox_for_command(command: &str, config: &SandboxRuntimeConfig) -> bool {
    if !config.enabled {
        return false;
    }
    if config.excluded_commands.is_empty() {
        return true;
    }
    for subcommand in split_compound_command(command) {
        let candidates = strip_env_and_wrappers_fixedpoint(&subcommand);
        for cand in &candidates {
            for pattern in &config.excluded_commands {
                if matches_excluded(pattern, cand) {
                    return false;
                }
            }
        }
    }
    true
}

fn matches_excluded(pattern: &str, candidate: &str) -> bool {
    let trimmed = candidate.trim();
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return trimmed == prefix || trimmed.starts_with(&format!("{prefix} "));
    }
    // Exact OR first-token match.
    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    trimmed == pattern || first_token == pattern
}
