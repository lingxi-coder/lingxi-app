//! File-path glob matching for permission rules (enforcement phase 3a).
//!
//! Ports the path-matching core of claude-code
//! `utils/permissions/filesystem.ts` so that CONTENT rules targeting file
//! tools — `Edit(src/**)`, `Read(./secrets/**)`, `Write(/tmp/out/**)` — match
//! against the tool input's path instead of the whole tool (the phase-2
//! stand-in matched such rules tool-wide). The two functions reproduced here
//! are `patternWithRoot` (resolve a rule pattern to a `(relativePattern, root)`
//! pair keyed off the rule's [`PermissionRuleSource`]) and the per-rule slice
//! of `matchingRuleForInput` (gitignore-test one pattern against one path).
//!
//! ## Tool grouping (claude-code `getPatternsByRoot` / `getRuleByContentsForToolName`)
//! claude-code routes file checks by TOOL TYPE, not the running tool's literal
//! name: every editing tool consults `Edit`-named rules (`FILE_EDIT_TOOL_NAME`)
//! and every reading tool consults `Read`-named rules (`FILE_READ_TOOL_NAME`).
//! [`file_tool_kind`] reproduces that split; the integration in
//! [`crate::policy::PermissionPolicy::authorize`] additionally honors
//! "edit-allow implies read-allow" (`checkReadPermissionForTool` step 5).
//!
//! ## Root resolution needs no per-rule plumbing
//! claude-code's `rootPathForSource` + `getSettingsRootPathForSource` switch
//! ONLY on the `source` enum (not the concrete settings-file path), so a single
//! [`FsRoots`] (`cwd` / `home` / `claude_home`) threaded in at policy
//! construction reproduces them exactly: `UserSettings`→`claude_home`,
//! `Project`/`Local`/`Policy`/`Flag`→`cwd`, `CliArg`/`Command`/`Session`→`cwd`.
//!
//! ## Documented divergences from `filesystem.ts` (forced / bounded, not gaps)
//! - **Per-rule single-pattern test** rather than claude-code's per-root
//!   batched `ignore().add(patterns)` + map-back. `authorize` evaluates rules
//!   one at a time, so each rule is tested in isolation. This is observably
//!   identical for "does THIS rule match THIS path?" except for cross-rule
//!   gitignore NEGATION (`!pattern`) interplay within one root — permission
//!   rule strings never carry `!`, so the case does not arise.
//! - **POSIX only.** The Windows POSIX-drive (`//c/Users/…`) conversion, the
//!   `windowsPathToPosixPath` step, and `hasSuspiciousWindowsPathPattern` /
//!   UNC defense-in-depth are omitted (the port's parity target is macOS/Linux,
//!   matching the orchestrator's `absolutize` divergence). The `//abs`→`/`-root
//!   and `~/`→home cases ARE ported.
//! - **Lexical, not `realpath`.** [`expand_path`] never touches disk (no
//!   symlink resolution / `getPathsForPermissionCheck`), matching the
//!   orchestrator's `expandPath` mirror. The wider
//!   `checkRead/checkWritePermissionForTool` flow (working-directory auto-allow,
//!   internal-path allowances, `.git`/`.claude` safety asks, suggestions) is
//!   NOT reproduced here — `authorize` keeps its deny→allow→mode shape; see its
//!   docs for what that elides.
//! - **No Unicode NFC** on `~`-expansion (`homedir().normalize('NFC')`): a no-op
//!   for ASCII paths and `OsStr` has no portable NFC primitive.

use crate::rule::PermissionRuleSource;
use std::path::{Component, Path, PathBuf};

/// The filesystem roots a [`PermissionRuleSource`] resolves against, supplied
/// once at policy construction. Mirrors the process-global `getOriginalCwd()` /
/// `homedir()` / `getClaudeConfigHomeDir()` that claude-code's
/// `rootPathForSource` consults.
#[derive(Debug, Clone)]
pub struct FsRoots {
    /// Original working directory — root for `CliArg`/`Command`/`Session` rules
    /// and for project/local/policy settings (`getOriginalCwd()`).
    pub cwd: PathBuf,
    /// Home directory, for `~/`-prefixed patterns. `None` disables `~/`
    /// expansion (the pattern then fails to resolve a root → never matches).
    pub home: Option<PathBuf>,
    /// Claude config home (`~/.claude`), the root for `UserSettings` rules
    /// (`getSettingsRootPathForSource('userSettings')` = `resolve(configHome)`).
    pub claude_home: PathBuf,
}

/// Which file-permission group a running tool belongs to (claude-code routes
/// file checks by tool TYPE — `FILE_EDIT_TOOL_NAME='Edit'` /
/// `FILE_READ_TOOL_NAME='Read'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileToolKind {
    /// Editing tools (`checkWritePermissionForTool`, type `'edit'`): consult
    /// `Edit`-named content rules.
    Editor,
    /// Reading tools (`checkReadPermissionForTool`, type `'read'`): consult
    /// `Read`-named content rules, plus `Edit`-ALLOW rules (edit⇒read).
    Reader,
    /// Not a file tool — content matching does not apply (phase-2 tool-wide
    /// behavior is preserved by the caller; Bash/WebFetch content matching is
    /// the separate 3a-bash deferral).
    NonFile,
}

/// Classify a running tool. Editor set = `Edit`/`Write`/`MultiEdit`/
/// `NotebookEdit` (the tools wired to `checkWritePermissionForTool`); reader
/// set = `Read`/`Glob`/`Grep`/`LSP` (wired to `checkReadPermissionForTool`).
/// `MultiEdit` is listed for forward-compatibility (no such tool exists in the
/// port yet; harmless).
#[must_use]
pub fn file_tool_kind(tool_name: &str) -> FileToolKind {
    match tool_name {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => FileToolKind::Editor,
        "Read" | "Glob" | "Grep" | "LSP" => FileToolKind::Reader,
        _ => FileToolKind::NonFile,
    }
}

/// Extract the path a file tool operates on from its input (claude-code
/// `tool.getPath(input)`). `NotebookEdit` uses `notebook_path`; `Glob`/`Grep`
/// use `path` (the search root, defaulting to `cwd` when absent, as
/// `GlobTool.getPath`/`GrepTool.getPath` do); the rest use `file_path`.
/// Returns `None` only when a `file_path`/`notebook_path` field is absent or
/// non-string (the tool itself would already have failed input validation).
///
/// NOTE on `LSP`: claude-code's `LSPTool.getPath` reads `filePath` (camel-case),
/// but THIS port's `LSPTool` declares its field as `file_path` (snake-case,
/// `tools/lsp/src/lsp_tool.rs`). We deliberately track the port's own schema —
/// so `file_path` here is correct for the port. If the LSP tool is ever
/// re-aligned to `filePath`, this mapping must follow it (else `Read(...)`
/// rules silently stop covering `LSP`).
#[must_use]
pub fn input_path_for_tool<'a>(
    tool_name: &str,
    input: &'a serde_json::Value,
    roots: &FsRoots,
) -> Option<std::borrow::Cow<'a, str>> {
    let field = match tool_name {
        "NotebookEdit" => "notebook_path",
        "Glob" | "Grep" => "path",
        // Read / Edit / Write / MultiEdit / LSP
        _ => "file_path",
    };
    match input.get(field).and_then(serde_json::Value::as_str) {
        Some(p) => Some(std::borrow::Cow::Borrowed(p)),
        // Glob/Grep default their search root to cwd when `path` is omitted.
        None if matches!(tool_name, "Glob" | "Grep") => {
            Some(std::borrow::Cow::Owned(roots.cwd.to_string_lossy().into_owned()))
        }
        None => None,
    }
}

/// Settings root for a rule source — 1:1 with claude-code `rootPathForSource`
/// (`filesystem.ts`) composed with `getSettingsRootPathForSource`
/// (`settings/settings.ts`): user settings resolve against the Claude config
/// home; every project-scoped / runtime source resolves against the original
/// cwd. `FlagSettings` approximates to `cwd` (the flag-settings file path is
/// not plumbed into the port — documented).
fn root_path_for_source(source: PermissionRuleSource, roots: &FsRoots) -> PathBuf {
    match source {
        PermissionRuleSource::UserSettings => roots.claude_home.clone(),
        PermissionRuleSource::ProjectSettings
        | PermissionRuleSource::LocalSettings
        | PermissionRuleSource::PolicySettings
        | PermissionRuleSource::FlagSettings
        | PermissionRuleSource::CliArg
        | PermissionRuleSource::Command
        | PermissionRuleSource::Session => roots.cwd.clone(),
    }
}

/// Resolve a rule pattern to `(relativePattern, root)` — 1:1 with claude-code
/// `patternWithRoot`. `root == None` means "anywhere" (matched relative to cwd
/// by the caller, mirroring `root ?? getCwd()`).
///
/// - `//abs…` → root `/`, pattern keeps a single leading slash (`pattern[1..]`).
/// - `~/x` → root = home, pattern = `/x` (`pattern[1..]`); `None` home → no root.
/// - `/x` (single slash) → root = [`root_path_for_source`], pattern unchanged.
/// - otherwise → root `None`; a leading `./` is stripped (`./.env` → `.env`).
fn pattern_with_root(
    pattern: &str,
    source: PermissionRuleSource,
    roots: &FsRoots,
) -> (String, Option<PathBuf>) {
    if let Some(after_first) = pattern.strip_prefix("//") {
        // `//etc/**` → relativePattern `/etc/**`, root `/` (pattern.slice(1)).
        (format!("/{after_first}"), Some(PathBuf::from("/")))
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        // `~/x` → relativePattern `/x` (pattern.slice(1)), root = home.
        match &roots.home {
            Some(h) => (format!("/{rest}"), Some(h.clone())),
            // No home configured: cannot resolve `~` → leave unrooted so it
            // resolves against cwd and effectively never matches a `~` file.
            None => (pattern.to_string(), None),
        }
    } else if pattern.starts_with('/') {
        // Single leading slash → settings-dir-relative.
        (pattern.to_string(), Some(root_path_for_source(source, roots)))
    } else {
        // No root: strip a leading `./` so `./.env` matches `.env`.
        let normalized = pattern.strip_prefix("./").unwrap_or(pattern);
        (normalized.to_string(), None)
    }
}

/// Lexically expand a tool path to an absolute, normalized path — same
/// `expandPath` mirror as the orchestrator's `absolutize` (trim; bare `~` /
/// `~/…` against home; absolute kept; relative joined to cwd; then `.`/`..`
/// collapsed without touching disk).
fn expand_path(raw: &str, roots: &FsRoots) -> PathBuf {
    let trimmed = raw.trim();
    let expanded: PathBuf = if trimmed == "~" {
        roots.home.clone().unwrap_or_else(|| PathBuf::from(trimmed))
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        roots
            .home
            .as_ref()
            .map_or_else(|| PathBuf::from(trimmed), |h| h.join(rest))
    } else {
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            roots.cwd.join(p)
        }
    };
    normalize_lexically(&expanded)
}

/// Collapse `.`/`..` segments without touching the filesystem (Node
/// `path.normalize`/`resolve`). A `..` pops the previous NORMAL component;
/// a leading `..` with nothing to pop is kept.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// POSIX-style lexical relative path from `base` to `target` (both absolute,
/// already normalized) — mirrors `posix.relative`. Returns the `/`-joined
/// relative string; `""` when equal; a `..`-leading string when `target` is
/// outside `base`.
fn posix_relative(base: &Path, target: &Path) -> String {
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let mut i = 0;
    while i < b.len() && i < t.len() && b[i] == t[i] {
        i += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in i..b.len() {
        parts.push("..".to_string());
    }
    for c in &t[i..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// Test whether `input_path` (a tool's raw path arg) matches a single rule
/// `pattern` tagged with `source` — the per-rule slice of claude-code
/// `matchingRuleForInput`.
///
/// 1. Expand the input path to an absolute, normalized path.
/// 2. Resolve the pattern's `(relativePattern, root)` via [`pattern_with_root`]
///    (`None` root ⇒ cwd, mirroring `root ?? getCwd()`).
/// 3. Compute the path relative to that root; bail if it escapes the root
///    (`..`-prefixed) or is empty (claude-code skips both).
/// 4. Strip a trailing `/**` (the `ignore` lib treats `path` as matching the
///    path AND everything inside it) and gitignore-test the relative path.
#[must_use]
pub fn path_matches_rule_pattern(
    input_path: &str,
    pattern: &str,
    source: PermissionRuleSource,
    roots: &FsRoots,
) -> bool {
    let file_abs = expand_path(input_path, roots);
    let (rel_pattern, root_opt) = pattern_with_root(pattern, source, roots);
    let effective_root = root_opt.unwrap_or_else(|| roots.cwd.clone());

    let rel_str = posix_relative(&effective_root, &file_abs);
    if rel_str.is_empty() || rel_str == ".." || rel_str.starts_with("../") {
        // Path is outside the pattern's root (or equals it) → no match.
        // claude-code skips only `""` and a `"../"`-prefix; the extra bare
        // `".."` guard is a deliberate defensive superset — it keeps a synthetic
        // `"/.."` (which carries a `ParentDir` component) from reaching the
        // builder, and the outcome is unchanged (TS's `ig.test("..")` also
        // yields no match for any real glob).
        return false;
    }

    // `ignore` treats `dir` as matching `dir` and everything under it, so the
    // `/**` suffix is redundant and must be stripped (matchingRuleForInput).
    let stripped = rel_pattern.strip_suffix("/**").unwrap_or(&rel_pattern);
    if stripped.is_empty() {
        // A bare root-anchored `/**` (also `//**` / `~/**`) strips to "".
        // claude-code feeds that "" to `ignore().add([""])`, which DROPS the
        // blank line (gitignore: an empty line matches no files) — so such a
        // rule matches NOTHING in the reference. Mirror that (matching nothing
        // is also the safe direction for ALLOW rules).
        return false;
    }

    // Match the relative path as if rooted at `/`: build a one-pattern matcher
    // anchored at `/` and test `/<relative>`. Computing the relative path
    // ourselves (above) — rather than handing the absolute path to the builder
    // — avoids the `ignore` crate's prefix-strip mis-matching paths that sit
    // OUTSIDE the root (it would otherwise glob-test the unstripped absolute).
    let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
    if let Err(e) = builder.add_line(None, stripped) {
        // An unbuildable glob (e.g. an unbalanced char class) matches nothing.
        // That is fail-CLOSED for an allow rule (safe) but fail-OPEN for a deny
        // rule (the deny silently does nothing), and it diverges from npm
        // `ignore`, which is more lenient — so surface it rather than swallow.
        tracing::warn!(
            pattern = %stripped,
            error = %e,
            "permission rule has an unparseable glob; the rule will not match"
        );
        return false;
    }
    let Ok(gitignore) = builder.build() else {
        return false;
    };
    let target = Path::new("/").join(&rel_str);
    gitignore
        .matched_path_or_any_parents(&target, false)
        .is_ignore()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> FsRoots {
        FsRoots {
            cwd: PathBuf::from("/proj"),
            home: Some(PathBuf::from("/home/u")),
            claude_home: PathBuf::from("/home/u/.claude"),
        }
    }

    fn matches(input: &str, pattern: &str, source: PermissionRuleSource) -> bool {
        path_matches_rule_pattern(input, pattern, source, &roots())
    }

    #[test]
    fn relative_glob_matches_under_cwd() {
        // `Edit(src/**)` in project settings → root = cwd.
        assert!(matches(
            "/proj/src/main.rs",
            "src/**",
            PermissionRuleSource::ProjectSettings
        ));
        assert!(matches(
            "src/lib.rs", // relative input → resolved against cwd
            "src/**",
            PermissionRuleSource::ProjectSettings
        ));
        // Outside src → no match.
        assert!(!matches(
            "/proj/tests/x.rs",
            "src/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn dot_slash_prefix_is_stripped() {
        // `./secrets/**` behaves like `secrets/**`.
        assert!(matches(
            "/proj/secrets/key.pem",
            "./secrets/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn bare_dotfile_pattern_matches_basename() {
        assert!(matches(
            "/proj/.env",
            ".env",
            PermissionRuleSource::ProjectSettings
        ));
        // gitignore basename rule matches at any depth.
        assert!(matches(
            "/proj/nested/.env",
            ".env",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn leading_slash_anchors_to_settings_root() {
        // `/src/**` is anchored at the settings root (cwd for project).
        assert!(matches(
            "/proj/src/a.rs",
            "/src/**",
            PermissionRuleSource::ProjectSettings
        ));
        // anchored: a nested `src` does NOT match.
        assert!(!matches(
            "/proj/a/src/b.rs",
            "/src/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn no_leading_slash_is_unanchored() {
        // `src/**` (no leading slash) matches `src` at ANY depth (gitignore
        // semantics, faithful to claude-code stripping `/**`→`src`).
        assert!(matches(
            "/proj/a/src/b.rs",
            "src/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn user_settings_root_is_claude_home() {
        // `/sub/**` in USER settings resolves against ~/.claude, not cwd.
        assert!(matches(
            "/home/u/.claude/sub/x",
            "/sub/**",
            PermissionRuleSource::UserSettings
        ));
        // A cwd path is OUTSIDE the user-settings root → no match.
        assert!(!matches(
            "/proj/sub/x",
            "/sub/**",
            PermissionRuleSource::UserSettings
        ));
    }

    #[test]
    fn tilde_resolves_against_home() {
        assert!(matches(
            "/home/u/.ssh/id_rsa",
            "~/.ssh/**",
            PermissionRuleSource::UserSettings
        ));
        // `~`-expanded input path also resolves to home.
        assert!(matches(
            "~/.ssh/id_rsa",
            "~/.ssh/**",
            PermissionRuleSource::UserSettings
        ));
    }

    #[test]
    fn double_slash_resolves_against_filesystem_root() {
        assert!(matches(
            "/etc/passwd",
            "//etc/**",
            PermissionRuleSource::ProjectSettings
        ));
        assert!(!matches(
            "/proj/etc/passwd",
            "//etc/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn path_outside_root_does_not_match() {
        // file above the root → posix_relative starts with `..` → skip.
        assert!(!matches(
            "/other/x.rs",
            "src/**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn glob_extension_pattern() {
        assert!(matches(
            "/proj/a/b.test.ts",
            "**/*.test.ts",
            PermissionRuleSource::ProjectSettings
        ));
        assert!(!matches(
            "/proj/a/b.ts",
            "**/*.test.ts",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn bare_root_double_star_matches_nothing() {
        // `/**` strips to "" → claude-code drops the blank `ignore` line, so
        // such a rule matches NOTHING (faithful + safe). Same for `//**`/`~/**`.
        assert!(!matches(
            "/proj/anything/deep.rs",
            "/**",
            PermissionRuleSource::ProjectSettings
        ));
        assert!(!matches(
            "/etc/x",
            "//**",
            PermissionRuleSource::ProjectSettings
        ));
        // …but a NON-empty unanchored `**` still matches everything (no strip).
        assert!(matches(
            "/proj/anything/deep.rs",
            "**",
            PermissionRuleSource::ProjectSettings
        ));
    }

    #[test]
    fn tool_classification() {
        assert_eq!(file_tool_kind("Edit"), FileToolKind::Editor);
        assert_eq!(file_tool_kind("Write"), FileToolKind::Editor);
        assert_eq!(file_tool_kind("NotebookEdit"), FileToolKind::Editor);
        assert_eq!(file_tool_kind("Read"), FileToolKind::Reader);
        assert_eq!(file_tool_kind("Glob"), FileToolKind::Reader);
        assert_eq!(file_tool_kind("Grep"), FileToolKind::Reader);
        assert_eq!(file_tool_kind("LSP"), FileToolKind::Reader);
        assert_eq!(file_tool_kind("Bash"), FileToolKind::NonFile);
    }

    #[test]
    fn input_path_extraction_per_tool() {
        let r = roots();
        let notebook = serde_json::json!({ "notebook_path": "/proj/a.ipynb" });
        assert_eq!(
            input_path_for_tool("NotebookEdit", &notebook, &r).as_deref(),
            Some("/proj/a.ipynb")
        );
        let edit = serde_json::json!({ "file_path": "/proj/x.rs" });
        assert_eq!(
            input_path_for_tool("Edit", &edit, &r).as_deref(),
            Some("/proj/x.rs")
        );
        let grep = serde_json::json!({ "path": "/proj/sub" });
        assert_eq!(
            input_path_for_tool("Grep", &grep, &r).as_deref(),
            Some("/proj/sub")
        );
        // Glob/Grep default to cwd when `path` is absent.
        let grep_no_path = serde_json::json!({ "pattern": "x" });
        assert_eq!(
            input_path_for_tool("Grep", &grep_no_path, &r).as_deref(),
            Some("/proj")
        );
        // Missing required path → None.
        let empty = serde_json::json!({});
        assert!(input_path_for_tool("Edit", &empty, &r).is_none());
    }
}
