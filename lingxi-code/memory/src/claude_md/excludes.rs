//! `claudeMdExcludes` — exclude `CLAUDE.md` files matching glob patterns or
//! absolute paths from loading. 1:1 with claude-code `isClaudeMdExcluded`
//! (`claudemd.ts:547-573`) + `resolveExcludePatterns`. Only `User` / `Project` /
//! `Local` tier files are excludable; `Managed` (and AutoMem/TeamMem) are never
//! excluded.

use crate::claude_md::ClaudeMdTier;
use glob::{MatchOptions, Pattern};
use std::path::Path;

// picomatch `{ dot: true }`: `*` matches leading dots; `*` does NOT cross `/`
// (path separator), `**` does.
fn match_opts() -> MatchOptions {
    MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    }
}

/// Compiled `claudeMdExcludes` matcher. Build once from the merged settings
/// patterns; query per file via [`Self::is_excluded`].
#[derive(Debug, Default, Clone)]
pub struct ClaudeMdExcluder {
    patterns: Vec<Pattern>,
}

impl ClaudeMdExcluder {
    /// Build from the merged `claudeMdExcludes` patterns. Each ABSOLUTE pattern
    /// also contributes a realpath-resolved variant (resolving its longest
    /// existing directory prefix) so a pattern written against `/tmp/...` still
    /// matches a file the OS resolved to `/private/tmp/...` (claude-code
    /// `resolveExcludePatterns`). Unparseable patterns are skipped.
    #[must_use]
    pub fn new(raw_patterns: &[String]) -> Self {
        let mut patterns = Vec::new();
        for expanded in resolve_exclude_patterns(raw_patterns) {
            if let Ok(p) = Pattern::new(&expanded) {
                patterns.push(p);
            }
        }
        Self { patterns }
    }

    /// `true` when no patterns are configured (filtering is a no-op).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether the `CLAUDE.md` at `path` (tier `tier`) is excluded. Only
    /// `User` / `Project` / `Local` are excludable; `Managed` is never excluded
    /// (`isClaudeMdExcluded`, `claudemd.ts:548-550`).
    #[must_use]
    pub fn is_excluded(&self, path: &Path, tier: ClaudeMdTier) -> bool {
        if self.patterns.is_empty()
            || !matches!(
                tier,
                ClaudeMdTier::User | ClaudeMdTier::Project | ClaudeMdTier::Local
            )
        {
            return false;
        }
        let normalized = path.to_string_lossy().replace('\\', "/");
        let candidate = Path::new(&normalized);
        let opts = match_opts();
        self.patterns
            .iter()
            .any(|p| p.matches_path_with(candidate, opts))
    }
}

/// Expand exclude patterns by adding a realpath-resolved variant for each
/// ABSOLUTE pattern (resolving the longest existing directory prefix), so a
/// symlinked prefix (`/tmp` → `/private/tmp`) matches. Relative / glob-only
/// patterns are kept verbatim. 1:1 with claude-code `resolveExcludePatterns`.
fn resolve_exclude_patterns(patterns: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in patterns {
        let normalized = raw.replace('\\', "/");
        if normalized.is_empty() {
            continue;
        }
        out.push(normalized.clone());
        if normalized.starts_with('/') {
            let glob_start = normalized
                .find(|c| matches!(c, '*' | '?' | '{' | '['))
                .unwrap_or(normalized.len());
            let static_prefix = &normalized[..glob_start];
            if let Some(resolved) = resolve_existing_prefix(static_prefix) {
                let candidate = format!("{resolved}{}", &normalized[glob_start..]);
                if candidate != normalized {
                    out.push(candidate);
                }
            }
        }
    }
    out
}

/// Canonicalize the longest existing prefix of absolute `path`, re-appending the
/// non-existent tail. `None` when nothing in the path exists.
fn resolve_existing_prefix(path: &str) -> Option<String> {
    let mut dir = std::path::PathBuf::from(path);
    let mut tail = String::new();
    loop {
        if dir.exists() {
            let mut s = std::fs::canonicalize(&dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            s.push_str(&tail);
            return Some(s);
        }
        let comp = dir.file_name()?.to_string_lossy().into_owned();
        tail = format!("/{comp}{tail}");
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_excluder_excludes_nothing() {
        let ex = ClaudeMdExcluder::new(&[]);
        assert!(ex.is_empty());
        assert!(!ex.is_excluded(Path::new("/p/CLAUDE.md"), ClaudeMdTier::Project));
    }

    #[test]
    fn managed_tier_is_never_excluded() {
        let ex = ClaudeMdExcluder::new(&["**/CLAUDE.md".to_string()]);
        // A glob that DOES match the path, but Managed tier is exempt.
        assert!(!ex.is_excluded(Path::new("/mgr/CLAUDE.md"), ClaudeMdTier::Managed));
        assert!(ex.is_excluded(Path::new("/proj/CLAUDE.md"), ClaudeMdTier::Project));
    }

    #[test]
    fn glob_and_absolute_patterns_match() {
        let ex = ClaudeMdExcluder::new(&[
            "**/secret/CLAUDE.md".to_string(),
            "/etc/proj/CLAUDE.md".to_string(),
        ]);
        assert!(ex.is_excluded(Path::new("/a/secret/CLAUDE.md"), ClaudeMdTier::User));
        assert!(ex.is_excluded(Path::new("/etc/proj/CLAUDE.md"), ClaudeMdTier::Local));
        assert!(!ex.is_excluded(Path::new("/a/public/CLAUDE.md"), ClaudeMdTier::User));
        // `*` does not cross `/`.
        assert!(!ex.is_excluded(Path::new("/a/b/CLAUDE.md"), ClaudeMdTier::Project)
            && ClaudeMdExcluder::new(&["/a/*/CLAUDE.md".to_string()])
                .is_excluded(Path::new("/a/b/CLAUDE.md"), ClaudeMdTier::Project));
    }
}
