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
//!
//! ## Working-directory containment (`AcceptEdits` auto-allow, Batch 1)
//! [`path_in_working_path`] / [`path_in_allowed_working_path`] port claude-code's
//! `pathInWorkingPath` / `pathInAllowedWorkingPath` (`filesystem.ts:683-744`),
//! which gate the `mode === 'acceptEdits' && isInWorkingDir` write auto-allow
//! (`:1360-1375`). They expand both the target and each working dir, apply the
//! macOS `/private/var`→`/var` & `/private/tmp`→`/tmp` symlink rewrites, lowercase
//! case-fold (so `.cLaUdE/…` cannot bypass a case-insensitive filesystem), and
//! accept iff the working-dir-relative path neither escapes upward (`..`-segment)
//! nor is absolute.
//!
//! **Divergence (forced, pre-accepted): lexical only — no on-disk symlink
//! resolution.** claude-code's `pathInAllowedWorkingPath` checks BOTH the
//! original path AND its `realpathSync`-resolved form (`getPathsForPermissionCheck`
//! / `getResolvedWorkingDirPaths`) against EVERY resolved working dir, to defeat
//! symlink-escape. This port resolves nothing on disk (consistent with
//! [`expand_path`]'s "Lexical, not `realpath`" decision): it compares the single
//! lexically-expanded target against the single lexically-expanded working dir.
//! Symlink-escape hardening is a known, already-accepted divergence in this crate
//! (the `/private/var`↔`/var` rewrite still handles the one common macOS case).

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
pub(crate) fn pattern_with_root(
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
///
/// `pub(crate)` so the auto-edit safety guard ([`crate::auto_edit_safety`]) can
/// reuse the SAME lexical expansion claude-code's `expandPath` performs before
/// the danger-segment scan — keeping the two paths byte-identical.
pub(crate) fn expand_path(raw: &str, roots: &FsRoots) -> PathBuf {
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

/// Normalize a path string for case-insensitive comparison — port of
/// `normalizeCaseForComparison` (`filesystem.ts:90-92`). Always lowercases
/// regardless of platform so a mixed-case `.cLauDe/CoMmAnDs` cannot bypass the
/// security checks on a case-insensitive filesystem (macOS / Windows).
///
/// (A sibling [`crate::auto_edit_safety::normalize_case_for_comparison`] exists
/// for the auto-edit safety guard; both are intentionally the same `to_lowercase`
/// — kept per-module to mirror the two TS call sites without a cross-module dep.)
#[must_use]
pub fn normalize_case_for_comparison(s: &str) -> String {
    s.to_lowercase()
}

/// Apply the macOS symlink rewrites claude-code performs before comparing paths
/// for working-dir containment (`filesystem.ts:716-721`):
/// `/private/var/` → `/var/` and `/private/tmp` (followed by `/` or end) →
/// `/tmp`. Operates on the already-absolute, lexically-expanded path string.
///
/// Only the leading `/private/var/` and `/private/tmp` forms are rewritten (the
/// TS regexes are anchored with `^`); an interior `/private/var` is untouched.
fn rewrite_private_symlinks(abs: &str) -> String {
    if let Some(rest) = abs.strip_prefix("/private/var/") {
        return format!("/var/{rest}");
    }
    // `/private/tmp$` or `/private/tmp/…` (the `(\/|$)` capture is preserved).
    if abs == "/private/tmp" {
        return "/tmp".to_string();
    }
    if let Some(rest) = abs.strip_prefix("/private/tmp/") {
        return format!("/tmp/{rest}");
    }
    abs.to_string()
}

/// Is `path` inside (or equal to) the single working directory `working`? — port
/// of `pathInWorkingPath` (`filesystem.ts:709-744`).
///
/// 1. Lexically expand both `path` and `working` to absolute, normalized paths.
/// 2. Apply the macOS `/private/var`→`/var` & `/private/tmp`→`/tmp` rewrites.
/// 3. Lowercase case-fold both for case-insensitive comparison.
/// 4. Compute the working-dir-relative path; accept iff it is the same path
///    (`""`), does NOT contain a `..` traversal segment, and is NOT absolute.
///
/// Lexical only — see the module-header divergence note (no on-disk `realpath`).
#[must_use]
pub fn path_in_working_path(path: &Path, working: &Path, roots: &FsRoots) -> bool {
    let absolute_path = expand_path(&path.to_string_lossy(), roots);
    let absolute_working_path = expand_path(&working.to_string_lossy(), roots);

    // macOS symlink rewrites (`/private/var`→`/var`, `/private/tmp`→`/tmp`).
    let normalized_path = rewrite_private_symlinks(&absolute_path.to_string_lossy());
    let normalized_working = rewrite_private_symlinks(&absolute_working_path.to_string_lossy());

    // Case-fold for case-insensitive filesystems.
    let case_path = normalize_case_for_comparison(&normalized_path);
    let case_working = normalize_case_for_comparison(&normalized_working);

    // POSIX relative path from working dir to target.
    let relative = posix_relative(Path::new(&case_working), Path::new(&case_path));

    // Same path.
    if relative.is_empty() {
        return true;
    }

    // `containsPathTraversal` (`path.ts:133-135`): `(?:^|[\\/])\.\.(?:[\\/]|$)` —
    // a `..` segment bounded by separators / string ends. `posix_relative` only
    // ever emits `..` at the START (or as the whole string), so this rejects an
    // escaping target. We mirror the TS predicate's full segment semantics.
    if contains_path_traversal(&relative) {
        return false;
    }

    // Inside iff the relative path is not itself absolute (`posix.isAbsolute`).
    !Path::new(&relative).is_absolute()
}

/// Port of `containsPathTraversal` (`path.ts:133-135`):
/// JS `/(?:^|[\\/])\.\.(?:[\\/]|$)/` — true when a `..` appears as a whole path
/// segment (bounded by `/`, `\`, or a string boundary on each side). A `..`
/// embedded in a longer name (`..beta`, `x..y`) is NOT a traversal.
fn contains_path_traversal(path: &str) -> bool {
    let bytes = path.as_bytes();
    let len = bytes.len();
    let is_sep = |b: u8| b == b'/' || b == b'\\';
    let mut i = 0;
    while i + 1 < len {
        if bytes[i] == b'.' && bytes[i + 1] == b'.' {
            let left_ok = i == 0 || is_sep(bytes[i - 1]);
            let right_idx = i + 2;
            let right_ok = right_idx == len || is_sep(bytes[right_idx]);
            if left_ok && right_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Is `path` inside ANY of the allowed working directories? — port of
/// `pathInAllowedWorkingPath` (`filesystem.ts:683-707`), reduced to the lexical
/// path set (one expanded path vs. one expanded working dir; no `realpath`).
///
/// claude-code's "every resolved input path must be within SOME working path"
/// collapses, with a single lexical path, to "the path is within some working
/// dir". Returns `false` for an empty `working_dirs` list (no allowance).
#[must_use]
pub fn path_in_allowed_working_path(path: &Path, working_dirs: &[PathBuf], roots: &FsRoots) -> bool {
    working_dirs
        .iter()
        .any(|wd| path_in_working_path(path, wd, roots))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> FsRoots {
        FsRoots {
            cwd: PathBuf::from("/proj"),
            home: Some(PathBuf::from("/home/u")),
            claude_home: PathBuf::from("/home/u/.lingxi"),
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
            "/home/u/.lingxi/sub/x",
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

    // ── Batch 1: working-dir containment ──────────────────────────────────

    fn in_working(path: &str, working: &str) -> bool {
        path_in_working_path(Path::new(path), Path::new(working), &roots())
    }

    #[test]
    fn normalize_case_lowercases_in_filesystem() {
        assert_eq!(normalize_case_for_comparison(".LINGXI"), ".lingxi");
        assert_eq!(normalize_case_for_comparison("Foo/Bar.RS"), "foo/bar.rs");
    }

    #[test]
    fn path_inside_working_dir_is_contained() {
        assert!(in_working("/proj/src/main.rs", "/proj"));
        // Nested deeper.
        assert!(in_working("/proj/a/b/c.rs", "/proj"));
        // The working dir itself (same path) is contained.
        assert!(in_working("/proj", "/proj"));
        // A relative input path resolves against cwd (= /proj) → inside.
        assert!(in_working("src/main.rs", "/proj"));
    }

    #[test]
    fn path_outside_working_dir_is_not_contained() {
        assert!(!in_working("/other/x.rs", "/proj"));
        // A sibling that shares a prefix but is not under the dir.
        assert!(!in_working("/projector/x.rs", "/proj"));
    }

    #[test]
    fn dotdot_escape_is_rejected() {
        // `..`-escaping the working dir must be rejected even though the lexical
        // expansion of the working dir vs target could otherwise look adjacent.
        assert!(!in_working("/proj/../etc/passwd", "/proj"));
        // A `..` that stays inside is fine (`/proj/a/../b` == `/proj/b`).
        assert!(in_working("/proj/a/../b.rs", "/proj"));
    }

    #[test]
    fn private_var_rewrite_makes_paths_match() {
        // `/private/var/...` target vs `/var/...` working dir → both rewrite to
        // `/var/...` and compare equal-prefix → contained.
        assert!(in_working("/private/var/folders/x/file.rs", "/var/folders/x"));
        // And the reverse: `/var/...` target vs `/private/var/...` working dir.
        assert!(in_working("/var/folders/x/file.rs", "/private/var/folders/x"));
    }

    #[test]
    fn private_tmp_rewrite_makes_paths_match() {
        assert!(in_working("/private/tmp/work/out.rs", "/tmp/work"));
        assert!(in_working("/tmp/work/out.rs", "/private/tmp/work"));
        // Bare `/private/tmp` rewrites to `/tmp` (the `$`-anchored branch).
        assert!(in_working("/private/tmp", "/tmp"));
    }

    #[test]
    fn comparison_is_case_insensitive() {
        // A mixed-case `.LINGXI` segment in the target still resolves under a
        // lowercase working dir (case-folded comparison).
        assert!(in_working("/Proj/SRC/Main.RS", "/proj/src"));
        assert!(in_working("/proj/.LiNgXi/x", "/proj/.lingxi"));
    }

    #[test]
    fn contains_path_traversal_segment_semantics() {
        // Whole-segment `..` (bounded by separators / boundaries) → true.
        assert!(contains_path_traversal(".."));
        assert!(contains_path_traversal("../x"));
        assert!(contains_path_traversal("a/../b"));
        assert!(contains_path_traversal("a/.."));
        assert!(contains_path_traversal("a\\..\\b"));
        // `..` embedded in a longer name is NOT a traversal.
        assert!(!contains_path_traversal("..beta"));
        assert!(!contains_path_traversal("x..y"));
        assert!(!contains_path_traversal("v2..beta/x"));
        assert!(!contains_path_traversal("a/b/c"));
        assert!(!contains_path_traversal(""));
    }

    #[test]
    fn allowed_working_path_iterates_dirs() {
        let r = roots();
        let dirs = vec![PathBuf::from("/proj"), PathBuf::from("/extra/work")];
        // Inside the first dir.
        assert!(path_in_allowed_working_path(
            Path::new("/proj/src/x.rs"),
            &dirs,
            &r
        ));
        // Inside the second (additional) dir.
        assert!(path_in_allowed_working_path(
            Path::new("/extra/work/y.rs"),
            &dirs,
            &r
        ));
        // Inside neither.
        assert!(!path_in_allowed_working_path(
            Path::new("/nope/z.rs"),
            &dirs,
            &r
        ));
        // Empty working-dir list → never contained.
        assert!(!path_in_allowed_working_path(
            Path::new("/proj/src/x.rs"),
            &[],
            &r
        ));
    }
}
