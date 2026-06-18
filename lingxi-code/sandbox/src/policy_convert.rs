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

/// Session/host-side seeds that claude-code's `convertToSandboxRuntimeConfig`
/// reads from module-level state (`getClaudeTempDir`, `SETTING_SOURCES`,
/// `getManagedSettingsDropInDir`, `getCwdState`, `getAdditionalDirectoriesForClaudeMd`,
/// `worktreeMainRepoPath`, …). In Rust we have no global session object, so the
/// caller threads these in explicitly. All fields default to empty/`None`, so a
/// minimal caller passes `&SandboxConvertContext::default()` and gets the same
/// result as the no-seed path (only `.` is seeded into `allow_write`).
///
/// Mirrors the seed application in sandbox-adapter.ts:225-299.
#[derive(Debug, Clone, Default)]
pub struct SandboxConvertContext {
    /// Claude temp dir (`getClaudeTempDir()`), seeded into `allow_write` right
    /// after `.` (sandbox-adapter.ts:225). Needed for Shell.ts cwd tracking
    /// files. `None` => not seeded (only `.`).
    pub claude_temp_dir: Option<String>,
    /// Resolved settings-file paths across all sources
    /// (`SETTING_SOURCES.map(getSettingsFilePathForSource)`), each unconditionally
    /// added to `deny_write` to prevent sandbox escape (sandbox-adapter.ts:232-235).
    pub settings_file_paths: Vec<String>,
    /// Managed-settings drop-in dir (`getManagedSettingsDropInDir()`), added to
    /// `deny_write` (sandbox-adapter.ts:236).
    pub managed_drop_in_dir: Option<String>,
    /// `.claude/settings*.json` + `.claude/skills` paths derived from the
    /// current working directory when it differs from the original cwd, added to
    /// `deny_write` (sandbox-adapter.ts:238-255). Threaded pre-resolved.
    pub cwd_settings_paths: Vec<String>,
    /// `.claude/skills` paths for the original (and current) cwd, added to
    /// `deny_write` (sandbox-adapter.ts:247-255). Threaded pre-resolved.
    pub skills_dirs: Vec<String>,
    /// Cached git-worktree main repo path (`worktreeMainRepoPath`), added to
    /// `allow_write` when present and `!= cwd` (sandbox-adapter.ts:286-288).
    pub worktree_main_repo_path: Option<String>,
    /// Session-only `--add-dir` / `/add-dir` directories
    /// (`getAdditionalDirectoriesForClaudeMd()`), unioned with
    /// `permissions.additionalDirectories` into `allow_write`
    /// (sandbox-adapter.ts:295-299).
    pub additional_md_dirs: Vec<String>,
}

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
/// - `Read(path)` deny   → `filesystem.deny_read`
/// - `WebFetch(domain:host)` allow → `network.allowed_domains`
///
/// `additional_directories` (settings) is UNIONED with `ctx.additional_md_dirs`
/// (session) into `allow_write` with insertion-order dedup (TS `Set` semantics,
/// sandbox-adapter.ts:295-299).
///
/// Seeds (sandbox-adapter.ts:225-299, applied BEFORE the rule walk):
/// - `allow_write` starts with `["."]`, then `ctx.claude_temp_dir` if `Some`;
/// - `deny_write` is extended with `ctx.settings_file_paths`, then
///   `ctx.managed_drop_in_dir` if `Some`, then `ctx.cwd_settings_paths` and
///   `ctx.skills_dirs`;
/// - `ctx.worktree_main_repo_path` is pushed into `allow_write` if `Some`.
///
/// Bare-git paths (`HEAD`/`objects`/`refs`/`hooks`/`config`) are intentionally
/// NOT seeded here — they are owned by the posix `prepare` layer.
///
/// Any user-supplied `sandbox` subsection values override the derived defaults.
#[must_use]
pub fn convert_settings_to_runtime_config(
    settings: &SettingsJson,
    ctx: &SandboxConvertContext,
) -> SandboxRuntimeConfig {
    // Gap C (allowManagedDomainsOnly / allowManagedReadPathsOnly): claude-code
    // enforces these by knowing WHICH settings source each rule came from
    // (`getSettingsForSource('policySettings')`). lingxi-core's conversion
    // pipeline takes a single merged `SettingsJson` with no per-source model, so
    // we CANNOT faithfully restrict to managed-only here. We surface the flags
    // on the wire (they round-trip) but do NOT silently honor them as if they
    // restricted the source — doing so would be MORE permissive-looking than the
    // real (source-aware) behavior. See sandbox-adapter.ts:181-210, 343-347.
    debug_assert!(
        settings
            .sandbox
            .as_ref()
            .and_then(|s| s.network.as_ref())
            .map_or(true, |n| !n.allow_managed_domains_only),
        "Gap C: allowManagedDomainsOnly cannot be source-restricted without a \
         per-source settings model; flag is surfaced but not enforced here"
    );
    debug_assert!(
        settings
            .sandbox
            .as_ref()
            .and_then(|s| s.filesystem.as_ref())
            .map_or(true, |f| !f.allow_managed_read_paths_only),
        "Gap C: allowManagedReadPathsOnly cannot be source-restricted without a \
         per-source settings model; flag is surfaced but not enforced here"
    );

    let settings_dir: PathBuf = settings
        .settings_dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut filesystem = FilesystemRestrictionConfig::default();
    let mut network = NetworkRestrictionConfig::default();

    // --- Seeds (sandbox-adapter.ts:225-299), applied BEFORE the rule walk. ---
    // Always include current directory and Claude temp directory as writable.
    filesystem.allow_write.push(".".to_string());
    if let Some(tmp) = &ctx.claude_temp_dir {
        filesystem.allow_write.push(tmp.clone());
    }
    // Always deny writes to settings.json files / managed drop-in dir / cwd
    // settings / skills dirs to prevent sandbox escape.
    filesystem
        .deny_write
        .extend(ctx.settings_file_paths.iter().cloned());
    if let Some(dir) = &ctx.managed_drop_in_dir {
        filesystem.deny_write.push(dir.clone());
    }
    filesystem
        .deny_write
        .extend(ctx.cwd_settings_paths.iter().cloned());
    filesystem.deny_write.extend(ctx.skills_dirs.iter().cloned());
    // Git worktree main repo path needs write access for index.lock etc.
    if let Some(main_repo) = &ctx.worktree_main_repo_path {
        filesystem.allow_write.push(main_repo.clone());
    }

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
    }

    // Additional directories: insertion-order-dedup union of
    // `permissions.additionalDirectories` (settings) and `ctx.additional_md_dirs`
    // (session). TS `new Set([...settings, ...session])` semantics
    // (sandbox-adapter.ts:295-299) — first occurrence wins, later dups dropped.
    {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let settings_dirs = settings
            .permissions
            .as_ref()
            .map_or(&[][..], |p| p.additional_directories.as_slice());
        for dir in settings_dirs.iter().chain(ctx.additional_md_dirs.iter()) {
            if seen.insert(dir.as_str()) {
                filesystem.allow_write.push(dir.clone());
            }
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
        if let Some(v) = s.allow_unsandboxed_commands {
            cfg.allow_unsandboxed_commands = v;
        }
        if let Some(v) = &s.network {
            // Merge: keep allowed_domains from WebFetch rules + values from
            // the sandbox.network.allowedDomains subsection. Likewise preserve
            // the DERIVED denied_domains (from WebFetch deny rules) and combine
            // with any sandbox.network.deniedDomains — mirror the allowed combine
            // so a user network override never clobbers derived deny entries.
            let mut merged = v.clone();
            merged.allowed_domains = {
                // claude-code order: [sandbox.network.allowedDomains ...,
                // WebFetch(domain:) allow ...] (sandbox-adapter.ts:198-209).
                // The subsection domains come FIRST, then the WebFetch-derived ones.
                let mut combined = v.allowed_domains.clone();
                combined.extend(cfg.network.allowed_domains.iter().cloned());
                combined
            };
            merged.denied_domains = {
                let mut combined = cfg.network.denied_domains.clone();
                combined.extend(v.denied_domains.iter().cloned());
                combined
            };
            cfg.network = merged;
        }
        if let Some(v) = &s.filesystem {
            // Merge: derived deny/allow paths + user-configured ones.
            let mut merged = v.clone();
            // sandbox.filesystem.* paths use STANDARD semantics, resolved via
            // resolveSandboxFilesystemPath (sandbox-adapter.ts:334-345, #30067) —
            // NOT the permission-rule `/path = settings-relative` convention.
            let resolve = |p: &str| resolve_sandbox_filesystem_path(p, &settings_dir);
            merged.allow_write = {
                let mut combined = cfg.filesystem.allow_write.clone();
                combined.extend(v.allow_write.iter().map(|p| resolve(p)));
                combined
            };
            merged.deny_write = {
                let mut combined = cfg.filesystem.deny_write.clone();
                combined.extend(v.deny_write.iter().map(|p| resolve(p)));
                combined
            };
            merged.deny_read = {
                let mut combined = cfg.filesystem.deny_read.clone();
                combined.extend(v.deny_read.iter().map(|p| resolve(p)));
                combined
            };
            merged.allow_read = {
                let mut combined = cfg.filesystem.allow_read.clone();
                combined.extend(v.allow_read.iter().map(|p| resolve(p)));
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

/// Port of `resolveSandboxFilesystemPath` (sandbox-adapter.ts:138-146 + the
/// #30067 fix). UNLIKE permission-rule paths (`resolve_path_pattern_for_sandbox`),
/// `sandbox.filesystem.*` uses STANDARD path semantics:
/// - `//path` → `/path` (legacy permission-rule escape, kept for compat);
/// - everything else → `expandPath(pattern, settings_dir)`:
///   - `~`        → `home_dir`
///   - `~/rest`   → `home_dir/rest`
///   - absolute   → as-is (NOT settings-relative — this is the #30067 fix)
///   - relative   → resolved against `settings_dir`
///   - empty/ws   → `settings_dir`
///
/// `home_dir` is injected (the `sandbox` crate carries no `dirs` dependency);
/// see [`resolve_sandbox_filesystem_path`] for the wrapper that reads `$HOME`.
#[must_use]
pub(crate) fn resolve_sandbox_filesystem_path_with(
    pattern: &str,
    settings_dir: &Path,
    home_dir: &str,
) -> String {
    // Legacy escape: //path → /path.
    if let Some(stripped) = pattern.strip_prefix("//") {
        return format!("/{stripped}");
    }
    // expandPath(pattern, settings_dir):
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return settings_dir.to_string_lossy().into_owned();
    }
    if trimmed == "~" {
        return home_dir.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        // expandPath: join(homedir(), slice(2)) — home + "/" + rest. Node `join`
        // lexically collapses `.`/`..` (path.ts:64), so we normalize when the
        // result is absolute (home_dir normally is).
        let joined = format!("{home_dir}/{rest}");
        if joined.starts_with('/') {
            return crate::path_pattern::lexically_normalize_absolute(&joined);
        }
        return joined;
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        // expandPath returns absolute paths via Node `normalize()` (path.ts:80),
        // which lexically collapses `.`/`..` — NOT settings-relative, NOT
        // filesystem-canonicalized.
        return crate::path_pattern::lexically_normalize_absolute(trimmed);
    }
    // Relative → Node `resolve(settings_dir, pattern)` (path.ts:84), which
    // lexically collapses `.`/`..`. Normalize after the join when absolute.
    let joined = settings_dir.join(trimmed).to_string_lossy().into_owned();
    if joined.starts_with('/') {
        return crate::path_pattern::lexically_normalize_absolute(&joined);
    }
    joined
}

/// Wrapper reading the real home dir from `$HOME` (POSIX). `expandPath` uses
/// `homedir()`; on the posix sandbox target that is `$HOME`. Falls back to an
/// empty string when unset (matching `path_utils.rs`'s `unwrap_or_default`).
#[must_use]
fn resolve_sandbox_filesystem_path(pattern: &str, settings_dir: &Path) -> String {
    let home_dir = std::env::var("HOME").unwrap_or_default();
    resolve_sandbox_filesystem_path_with(pattern, settings_dir, &home_dir)
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
            // claude-code extracts ONLY Read DENY rules → denyRead
            // (sandbox-adapter.ts:323-325). There is NO Read-allow → allowRead
            // branch; allowRead is sourced exclusively from
            // sandbox.filesystem.allowRead (sandbox-adapter.ts:343-347).
            if !is_allow {
                let resolved = resolve_path_pattern_for_sandbox(content, settings_dir);
                filesystem.deny_read.push(resolved);
            }
            // Read(allow) contributes nothing here.
        }
        TOOL_WEBFETCH => {
            if let Some(domain) = content.strip_prefix("domain:") {
                if is_allow {
                    network.allowed_domains.push(domain.to_string());
                } else {
                    // Deny rules feed the computed denylist, checked before the
                    // allowlist (sandbox-adapter.ts:212-220, 362).
                    network.denied_domains.push(domain.to_string());
                }
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

#[cfg(test)]
mod resolve_fs_path_tests {
    use super::resolve_sandbox_filesystem_path_with;
    use std::path::Path;
    #[test]
    fn tilde_expands_to_injected_home() {
        let sd = Path::new("/home/u/.claude");
        assert_eq!(
            resolve_sandbox_filesystem_path_with("~", sd, "/home/u"),
            "/home/u"
        );
        assert_eq!(
            resolve_sandbox_filesystem_path_with("~/.cargo", sd, "/home/u"),
            "/home/u/.cargo"
        );
    }

    #[test]
    fn relative_dot_prefix_resolves_against_settings_dir() {
        // `./src` is relative → resolve(settings_dir, "./src") collapses `.`.
        let sd = Path::new("/proj/.claude");
        assert_eq!(
            resolve_sandbox_filesystem_path_with("./src", sd, "/home/u"),
            "/proj/.claude/src"
        );
    }

    #[test]
    fn absolute_collapses_dot_dot() {
        // Absolute → Node normalize(): `/a/../b` → `/b`.
        let sd = Path::new("/proj/.claude");
        assert_eq!(
            resolve_sandbox_filesystem_path_with("/a/../b", sd, "/home/u"),
            "/b"
        );
    }

    #[test]
    fn relative_dot_dot_collapses_after_join() {
        // Relative `x/../y` resolved under settings_dir → `<dir>/y`.
        let sd = Path::new("/proj/.claude");
        assert_eq!(
            resolve_sandbox_filesystem_path_with("x/../y", sd, "/home/u"),
            "/proj/.claude/y"
        );
    }

    #[test]
    fn tilde_rest_collapses_dot_dot() {
        // `~/a/../b` → join(home, "a/../b") collapses → `/home/u/b`.
        let sd = Path::new("/proj/.claude");
        assert_eq!(
            resolve_sandbox_filesystem_path_with("~/a/../b", sd, "/home/u"),
            "/home/u/b"
        );
    }
}
