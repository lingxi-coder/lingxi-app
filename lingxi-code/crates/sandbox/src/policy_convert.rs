//! Port of `convertToSandboxRuntimeConfig` and `getLinuxGlobPatternWarnings`
//! from `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
//!
//! Walks `SettingsJson.permissions.allow/deny` for `Edit(...)`, `Read(...)`,
//! `WebFetch(domain:...)` rules and folds the extracted patterns into a
//! `SandboxRuntimeConfig`. `Bash(...)` rules are intentionally ignored here —
//! the bash decision (compound + env-var stripping + excludedCommands match)
//! lives in [`crate::decision::should_use_sandbox`].

use crate::path_pattern::resolve_path_pattern_for_sandbox;
use crate::runtime_config::{
    FilesystemRestrictionConfig, NetworkRestrictionConfig, SandboxRuntimeConfig, SettingsJson,
};
use std::path::{Path, PathBuf};

/// Tool-name prefix matchers. claude-code's permission rules carry the tool
/// name as `Edit`, `Read`, `Bash`, or `WebFetch` — we hard-code those.
const TOOL_EDIT: &str = "Edit";
const TOOL_READ: &str = "Read";
const TOOL_WEBFETCH: &str = "WebFetch";

/// Parse a `Tool(content)` permission rule string into `(tool, content)`.
///
/// Returns `None` if the rule has no parentheses (a bare `Tool` rule covers
/// all calls and doesn't carry a filesystem path).
fn parse_rule(rule: &str) -> Option<(&str, &str)> {
    let open = rule.find('(')?;
    if !rule.ends_with(')') {
        return None;
    }
    let tool = &rule[..open];
    let content = &rule[open + 1..rule.len() - 1];
    Some((tool, content))
}

/// Convert claude-code `SettingsJson` into `SandboxRuntimeConfig`.
///
/// Walk `permissions.allow` and `permissions.deny`:
/// - `Edit(path)` allow  → `filesystem.allow_write`
/// - `Edit(path)` deny   → `filesystem.deny_write`
/// - `Read(path)` allow  → `filesystem.allow_read`
/// - `Read(path)` deny   → `filesystem.deny_read`
/// - `WebFetch(domain:host)` allow → `network.allowed_domains`
///
/// `additional_directories` is appended to `allow_write` (additional dirs are
/// always writable inside the sandbox; this matches claude-code's behavior of
/// pushing `additionalDirectories` into the sandbox-runtime allow set).
///
/// Any user-supplied `sandbox` subsection values override the derived defaults.
#[must_use]
pub fn convert_settings_to_runtime_config(settings: &SettingsJson) -> SandboxRuntimeConfig {
    let settings_dir: PathBuf = settings
        .settings_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut filesystem = FilesystemRestrictionConfig::default();
    let mut network = NetworkRestrictionConfig::default();

    if let Some(perms) = &settings.permissions {
        for rule_string in &perms.allow {
            apply_rule(
                rule_string,
                &settings_dir,
                &mut filesystem,
                &mut network,
                true,
            );
        }
        for rule_string in &perms.deny {
            apply_rule(
                rule_string,
                &settings_dir,
                &mut filesystem,
                &mut network,
                false,
            );
        }
        // Additional directories are pushed verbatim into allowWrite — they
        // become writable inside the sandbox (matching claude-code).
        for dir in &perms.additional_directories {
            filesystem.allow_write.push(dir.clone());
        }
    }

    // Now apply any user-supplied `sandbox` subsection overrides.
    let mut cfg = SandboxRuntimeConfig {
        network,
        filesystem,
        ..Default::default()
    };

    if let Some(s) = &settings.sandbox {
        if let Some(v) = s.enabled {
            cfg.enabled = v;
        }
        if let Some(v) = s.fail_if_unavailable {
            cfg.fail_if_unavailable = v;
        }
        if let Some(v) = &s.enabled_platforms {
            cfg.enabled_platforms = Some(v.clone());
        }
        if let Some(v) = s.auto_allow_bash_if_sandboxed {
            cfg.auto_allow_bash_if_sandboxed = v;
        }
        if let Some(v) = &s.allow_unsandboxed_commands {
            cfg.allow_unsandboxed_commands.clone_from(v);
        }
        if let Some(v) = &s.network {
            // Merge: keep allowed_domains from WebFetch rules + values from
            // the sandbox.network.allowedDomains subsection.
            let mut merged = v.clone();
            merged.allowed_domains = {
                let mut combined = cfg.network.allowed_domains.clone();
                combined.extend(v.allowed_domains.iter().cloned());
                combined
            };
            cfg.network = merged;
        }
        if let Some(v) = &s.filesystem {
            // Merge: derived deny/allow paths + user-configured ones.
            let mut merged = v.clone();
            merged.allow_write = {
                let mut combined = cfg.filesystem.allow_write.clone();
                combined.extend(v.allow_write.iter().cloned());
                combined
            };
            merged.deny_write = {
                let mut combined = cfg.filesystem.deny_write.clone();
                combined.extend(v.deny_write.iter().cloned());
                combined
            };
            merged.deny_read = {
                let mut combined = cfg.filesystem.deny_read.clone();
                combined.extend(v.deny_read.iter().cloned());
                combined
            };
            merged.allow_read = {
                let mut combined = cfg.filesystem.allow_read.clone();
                combined.extend(v.allow_read.iter().cloned());
                combined
            };
            cfg.filesystem = merged;
        }
        if let Some(v) = &s.ignore_violations {
            cfg.ignore_violations.clone_from(v);
        }
        if let Some(v) = s.enable_weaker_nested_sandbox {
            cfg.enable_weaker_nested_sandbox = v;
        }
        if let Some(v) = s.enable_weaker_network_isolation {
            cfg.enable_weaker_network_isolation = v;
        }
        if let Some(v) = &s.excluded_commands {
            cfg.excluded_commands.clone_from(v);
        }
        if let Some(v) = &s.ripgrep {
            cfg.ripgrep = v.clone();
        }
    }

    cfg
}

fn apply_rule(
    rule_string: &str,
    settings_dir: &Path,
    filesystem: &mut FilesystemRestrictionConfig,
    network: &mut NetworkRestrictionConfig,
    is_allow: bool,
) {
    let Some((tool, content)) = parse_rule(rule_string) else {
        return;
    };
    match tool {
        TOOL_EDIT => {
            let resolved = resolve_path_pattern_for_sandbox(content, settings_dir);
            if is_allow {
                filesystem.allow_write.push(resolved);
            } else {
                filesystem.deny_write.push(resolved);
            }
        }
        TOOL_READ => {
            let resolved = resolve_path_pattern_for_sandbox(content, settings_dir);
            if is_allow {
                filesystem.allow_read.push(resolved);
            } else {
                filesystem.deny_read.push(resolved);
            }
        }
        TOOL_WEBFETCH => {
            if let Some(domain) = content.strip_prefix("domain:") {
                if is_allow {
                    network.allowed_domains.push(domain.to_string());
                }
                // Denied domains: claude-code stores these for telemetry but
                // does not emit them on the wire; we match that and ignore.
            }
        }
        _ => {
            // Bash, Task, etc. are not part of the sandbox filesystem map.
        }
    }
}

/// Port of `getLinuxGlobPatternWarnings` from
/// `claude-code/src/utils/sandbox/sandbox-adapter.ts`.
///
/// Returns the verbatim permission-rule strings whose path content contains
/// glob characters `* ? [ ]` (excluding a trailing `/**`). bubblewrap cannot
/// resolve globs, so claude-code surfaces these as user-facing warnings on
/// Linux/WSL.
///
/// Caller is expected to only invoke this on Linux/WSL hosts where sandbox is
/// enabled; this function does not check the platform itself (that's the
/// caller's responsibility — keeps this function purely string-driven).
#[must_use]
pub fn linux_glob_pattern_warnings(settings: &SettingsJson) -> Vec<String> {
    let Some(perms) = &settings.permissions else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for rule_string in perms.allow.iter().chain(perms.deny.iter()) {
        let Some((tool, content)) = parse_rule(rule_string) else {
            continue;
        };
        if tool != TOOL_EDIT && tool != TOOL_READ {
            continue;
        }
        if has_globs_excluding_trailing_double_star(content) {
            warnings.push(rule_string.clone());
        }
    }
    warnings
}

/// `true` iff `path` contains `*`, `?`, `[`, or `]` anywhere outside a
/// trailing `/**`. Mirrors the JS regex `/[*?\[\]]/.test(stripped)` after
/// `path.replace(/\/\*\*$/, '')`.
fn has_globs_excluding_trailing_double_star(path: &str) -> bool {
    let stripped = path.strip_suffix("/**").unwrap_or(path);
    stripped.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}
