//! Dir-up walker: cwd → parents → user-home.

use std::path::{Path, PathBuf};

/// Filename of the project memory file (case-sensitive).
pub const FILE_NAME: &str = "CLAUDE.md";
/// Filename of the local-override memory file.
pub const LOCAL_OVERRIDE_NAME: &str = "CLAUDE.local.md";

/// One discovered CLAUDE.md (or local override) location, post-walk.
#[derive(Debug, Clone)]
pub struct HierarchyEntry {
    /// Absolute path to the file on disk.
    pub path: PathBuf,
    /// Whether this is a `CLAUDE.local.md` (true) or `CLAUDE.md` (false).
    pub is_local_override: bool,
    /// Whether the actual filename's bytes matched `FILE_NAME` exactly
    /// (false on case-insensitive filesystems that lowercased it).
    pub exact_case: bool,
}

/// Snapshot of discovered memory-file locations, innermost-first
/// (cwd's files, then each parent's, then User). Consumers reverse this
/// to reach claude-code's splice order. See [`walk`].
#[derive(Debug, Default)]
pub struct Hierarchy {
    /// Discovered entries in walk order.
    pub entries: Vec<HierarchyEntry>,
}

/// Directory name claude-code uses for nested config (`.claude/`).
const DOT_CLAUDE: &str = ".claude";
/// Subdirectory under `.claude/` holding `*.md` rule files.
const RULES_DIR: &str = "rules";

/// Discover the claude-code memory-file set, in `getMemoryFiles`
/// tier order (`utils/claudemd.ts:790-934`):
///   1. **User**:    `<home>/.claude/CLAUDE.md`, then `<home>/.claude/rules/**.md`.
///   2. **Project + Local**, from the filesystem root DOWN to `cwd`; per dir:
///      `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/**.md`, `CLAUDE.local.md`.
///
/// claude-code splices files in exactly that order (User first, the innermost
/// `cwd` last). The orchestrator (`prompt::memory_block`) **reverses** this
/// primitive's output to reach that splice order, so `walk` upholds an
/// innermost-first contract: it builds the list in claude-code splice order and
/// reverses it once at the end, yielding `cwd` … parents … User.
///
/// NB: claude-code's MANAGED tier (`getManagedFilePath` → `/etc/claude-code` etc.)
/// is intentionally NOT probed — this port has no managed-settings concept
/// (the settings loader models only user + project layers), and probing an
/// absolute system path would make the hermetic discovery machine-dependent.
/// Deferred until the port grows a managed-settings tier.
///
/// Every file is emitted at most once, mirroring claude-code's shared
/// `processedPaths` set. Missing dirs/files are silently skipped (NOT an error).
#[must_use]
pub fn walk(cwd: &Path, home: &Path) -> Hierarchy {
    let mut out = Vec::new();
    let mut processed = std::collections::HashSet::new();

    // (1) User tier — `<home>/.claude/CLAUDE.md` + `<home>/.claude/rules/**`.
    let user_dir = home.join(DOT_CLAUDE);
    emit_probe(&user_dir, FILE_NAME, false, &mut out, &mut processed);
    collect_rules(&user_dir.join(RULES_DIR), &mut out, &mut processed);

    // (3) Project + Local tier — filesystem root DOWN to cwd.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut cur: Option<&Path> = Some(cwd);
    while let Some(dir) = cur {
        dirs.push(dir.to_path_buf());
        cur = dir.parent();
    }
    dirs.reverse(); // root → cwd
    for dir in &dirs {
        // Project: `CLAUDE.md`, then `.claude/CLAUDE.md`, then `.claude/rules/**`.
        emit_probe(dir, FILE_NAME, false, &mut out, &mut processed);
        emit_probe(
            &dir.join(DOT_CLAUDE),
            FILE_NAME,
            false,
            &mut out,
            &mut processed,
        );
        collect_rules(
            &dir.join(DOT_CLAUDE).join(RULES_DIR),
            &mut out,
            &mut processed,
        );
        // Local override LAST within the directory (claude-code emits Local
        // after Project so it wins the model's recency attention).
        emit_probe(dir, LOCAL_OVERRIDE_NAME, true, &mut out, &mut processed);
    }

    // `out` is now in claude-code splice order (User → root … → cwd).
    // Reverse to the innermost-first contract this primitive promises; the
    // orchestrator reverses again to restore the splice order.
    out.reverse();
    Hierarchy { entries: out }
}

/// Probe `dir` for a file named `want` (case-insensitive) and, when found and
/// not already emitted, push a [`HierarchyEntry`]. Mirrors a single
/// `processMemoryFile` call guarded by the shared `processedPaths` set.
fn emit_probe(
    dir: &Path,
    want: &str,
    is_local: bool,
    out: &mut Vec<HierarchyEntry>,
    processed: &mut std::collections::HashSet<PathBuf>,
) {
    if let Some(entry) = probe(dir, want, is_local) {
        if processed.insert(entry.path.clone()) {
            out.push(entry);
        }
    }
}

/// Recursively collect every `*.md` file under `rules_dir`, mirroring
/// claude-code `processMdRules` (`utils/claudemd.ts:697-788`): descend into
/// subdirectories depth-first, guard against symlink cycles with a visited-dir
/// set, and silently ignore a missing / non-dir / permission-denied directory.
///
/// claude-code iterates entries in raw `readdir` order; that OS order is
/// unstable and cannot be pinned in a fixture, so the Rust port sorts entries
/// by file name for reproducibility. Only files whose name ends in `.md`
/// (case-sensitive, matching `endsWith('.md')`) are emitted, each at most once
/// via the shared `processed` set.
fn collect_rules(
    rules_dir: &Path,
    out: &mut Vec<HierarchyEntry>,
    processed: &mut std::collections::HashSet<PathBuf>,
) {
    let mut visited = std::collections::HashSet::new();
    collect_rules_inner(rules_dir, out, processed, &mut visited);
}

fn collect_rules_inner(
    rules_dir: &Path,
    out: &mut Vec<HierarchyEntry>,
    processed: &mut std::collections::HashSet<PathBuf>,
    visited: &mut std::collections::HashSet<PathBuf>,
) {
    // visitedDirs cycle guard: key on the canonical (symlink-resolved) path so
    // a symlink loop (A → B → A) terminates. Fall back to the raw path when it
    // can't be canonicalized (e.g. missing — read_dir below then bails).
    let key = std::fs::canonicalize(rules_dir).unwrap_or_else(|_| rules_dir.to_path_buf());
    if !visited.insert(key) {
        return;
    }

    // Missing / non-dir / permission-denied → silently ignored.
    let Ok(read) = std::fs::read_dir(rules_dir) else {
        return;
    };

    // Sort by file name for deterministic ordering (see fn docs).
    let mut items: Vec<std::fs::DirEntry> = read.flatten().collect();
    items.sort_by_key(std::fs::DirEntry::file_name);

    for ent in items {
        let path = ent.path();
        let Ok(ft) = ent.file_type() else { continue };
        // Resolve symlinks via stat() to classify the target, matching
        // processMdRules' safeResolvePath handling.
        let (is_dir, is_file) = if ft.is_symlink() {
            match std::fs::metadata(&path) {
                Ok(m) => (m.is_dir(), m.is_file()),
                Err(_) => continue,
            }
        } else {
            (ft.is_dir(), ft.is_file())
        };

        if is_dir {
            collect_rules_inner(&path, out, processed, visited);
        } else if is_file
            && ent.file_name().to_string_lossy().ends_with(".md")
            && processed.insert(path.clone())
        {
            out.push(HierarchyEntry {
                path,
                is_local_override: false,
                exact_case: true,
            });
        }
    }
}

fn probe(dir: &Path, want: &str, is_local: bool) -> Option<HierarchyEntry> {
    // Scan the directory and compare names case-insensitively. We rely on
    // the dirent listing (NOT `Path::is_file`) so that case-insensitive
    // filesystems (macOS APFS-default, Windows NTFS) still produce a
    // `exact_case == false` signal when the on-disk name differs from
    // `want` in case. The event hook lives in [`emit_case_mismatch`].
    let want_lc = want.to_ascii_lowercase();
    let read = std::fs::read_dir(dir).ok()?;
    for ent in read.flatten() {
        let name = ent.file_name();
        let s = name.to_string_lossy();
        if s.to_ascii_lowercase() == want_lc && ent.file_type().ok()?.is_file() {
            return Some(HierarchyEntry {
                path: ent.path(),
                is_local_override: is_local,
                exact_case: s == want,
            });
        }
    }
    None
}

use std::sync::Arc;

/// Telemetry event name emitted when a CLAUDE.md is found under a
/// different case (e.g. `claude.md` on macOS APFS).
pub const TENGU_MEMORY_CASE_MISMATCH: &str = "tengu_memory_case_mismatch";

/// Emit `tengu_memory_case_mismatch` for entries where `exact_case == false`.
///
/// `actual` is the filename component (NOT the full path); the full path is
/// PII-tagged via `_PROTO_path`. No-op when `bus` is `None`.
pub async fn emit_case_mismatch(
    bus: Option<&Arc<telemetry::AnalyticsBus>>,
    path: &std::path::Path,
    actual: &str,
) {
    let Some(bus) = bus else {
        return;
    };
    let mut md = telemetry::sink::LogEventMetadata::new();
    md.insert(
        "_PROTO_path".into(),
        telemetry::sink::AnalyticsValue::String(
            telemetry::pii::PiiTagged::assert_pii_tagged_column(path.display().to_string())
                .into_inner(),
        ),
    );
    md.insert(
        "actual".into(),
        telemetry::sink::AnalyticsValue::String(
            telemetry::pii::Verified::assert_safe(actual.to_string()).into_inner(),
        ),
    );
    bus.log_event(TENGU_MEMORY_CASE_MISMATCH, md).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(dir: &std::path::Path, name: &str) {
        fs::write(dir.join(name), b"# notes\n").unwrap();
    }

    #[test]
    fn walks_cwd_then_parents_then_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        touch(&home.join(".claude"), "CLAUDE.md");
        let outer = tmp.path().join("repo");
        let inner = outer.join("pkg");
        fs::create_dir_all(&inner).unwrap();
        touch(&outer, "CLAUDE.md");
        touch(&inner, "CLAUDE.md");

        let h = walk(&inner, &home);
        let paths: Vec<_> = h.entries.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                inner.join("CLAUDE.md"),
                outer.join("CLAUDE.md"),
                home.join(".claude").join("CLAUDE.md"),
            ],
            "walk order must be cwd → parents → home"
        );
        assert!(h.entries.iter().all(|e| !e.is_local_override));
    }

    #[test]
    fn surfaces_local_override_alongside_canonical() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        touch(&cwd, "CLAUDE.md");
        touch(&cwd, "CLAUDE.local.md");

        let h = walk(&cwd, &home);
        let kinds: Vec<_> = h.entries.iter().map(|e| e.is_local_override).collect();
        // CLAUDE.local.md MUST come before CLAUDE.md at the same level
        // so it can shadow the canonical entry.
        assert_eq!(kinds, vec![true, false]);
    }

    #[test]
    fn stops_at_filesystem_root_no_panic() {
        // walk(&Path::new("/"), &home) must not loop or panic
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let _h = walk(std::path::Path::new("/"), &home);
    }

    #[test]
    fn missing_files_produce_empty_hierarchy() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("empty");
        fs::create_dir_all(&cwd).unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap(); // .claude does NOT exist
        let h = walk(&cwd, &home);
        assert!(h.entries.is_empty());
    }

    /// Helper: relative paths (under `root`) of the entries, in the
    /// orchestrator's *splice* order (reverse of the innermost-first walk).
    fn splice_order(h: &Hierarchy, root: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = h
            .entries
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(root)
                    .unwrap_or(&e.path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.reverse();
        v
    }

    #[test]
    fn discovers_dot_claude_claude_md() {
        // `.claude/CLAUDE.md` (Project) must be found right after `CLAUDE.md`.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(repo.join(".claude")).unwrap();
        touch(&repo, "CLAUDE.md");
        touch(&repo.join(".claude"), "CLAUDE.md");

        let h = walk(&repo, &home);
        assert_eq!(
            splice_order(&h, tmp.path()),
            vec!["repo/CLAUDE.md", "repo/.claude/CLAUDE.md"],
            "splice order: CLAUDE.md then .claude/CLAUDE.md"
        );
    }

    #[test]
    fn discovers_dot_claude_rules_including_nested_in_order() {
        // `.claude/rules/**/*.md` (Project), recursive, sorted by name.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(rules.join("sub")).unwrap();
        touch(&repo, "CLAUDE.md");
        touch(&rules, "b.md");
        touch(&rules, "a.md");
        touch(&rules, "ignored.txt"); // non-.md skipped
        touch(&rules.join("sub"), "z.md");

        let h = walk(&repo, &home);
        // Sorted readdir: `a.md`, `b.md`, then the `sub/` directory (recursed).
        assert_eq!(
            splice_order(&h, tmp.path()),
            vec![
                "repo/CLAUDE.md",
                "repo/.claude/rules/a.md",
                "repo/.claude/rules/b.md",
                "repo/.claude/rules/sub/z.md",
            ],
            "rules discovered recursively, sorted, .md only"
        );
    }

    #[test]
    fn discovers_user_rules_tier() {
        // `~/.claude/rules/**/*.md` (User) come right after `~/.claude/CLAUDE.md`.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let user = home.join(".claude");
        fs::create_dir_all(user.join("rules")).unwrap();
        touch(&user, "CLAUDE.md");
        touch(&user.join("rules"), "u1.md");
        touch(&user.join("rules"), "u2.md");
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        touch(&cwd, "CLAUDE.md");

        let h = walk(&cwd, &home);
        assert_eq!(
            splice_order(&h, tmp.path()),
            vec![
                "home/.claude/CLAUDE.md",
                "home/.claude/rules/u1.md",
                "home/.claude/rules/u2.md",
                "repo/CLAUDE.md",
            ],
            "User CLAUDE.md + rules precede the project tier"
        );
    }

    #[test]
    fn full_tier_order_user_project_local() {
        // The MANAGED tier is intentionally not probed (no Rust managed-settings
        // concept). Assert the User → Project → Local ordering with
        // `.claude/CLAUDE.md`, rules, and the local override.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let user = home.join(".claude");
        fs::create_dir_all(user.join("rules")).unwrap();
        touch(&user, "CLAUDE.md");
        touch(&user.join("rules"), "ur.md");

        let repo = tmp.path().join("repo");
        let pkg = repo.join("pkg");
        fs::create_dir_all(pkg.join(".claude").join("rules")).unwrap();
        touch(&repo, "CLAUDE.md");
        touch(&pkg, "CLAUDE.md");
        touch(&pkg.join(".claude"), "CLAUDE.md");
        touch(&pkg.join(".claude").join("rules"), "pr.md");
        touch(&pkg, "CLAUDE.local.md");

        let h = walk(&pkg, &home);
        assert_eq!(
            splice_order(&h, tmp.path()),
            vec![
                "home/.claude/CLAUDE.md",
                "home/.claude/rules/ur.md",
                "repo/CLAUDE.md",
                "repo/pkg/CLAUDE.md",
                "repo/pkg/.claude/CLAUDE.md",
                "repo/pkg/.claude/rules/pr.md",
                "repo/pkg/CLAUDE.local.md",
            ],
            "tier order: User → Project(root→cwd: CLAUDE, .claude/CLAUDE, rules) → Local"
        );
    }

    #[test]
    fn missing_dot_claude_and_rules_dirs_silently_ignored() {
        // No `.claude/` dir at all: walk yields only the plain CLAUDE.md, no panic.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        touch(&repo, "CLAUDE.md");

        let h = walk(&repo, &home);
        assert_eq!(splice_order(&h, tmp.path()), vec!["repo/CLAUDE.md"]);
    }

    #[test]
    fn rules_recursion_visited_guard_breaks_symlink_cycle() {
        // A symlink loop inside the rules tree must not recurse forever.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let repo = tmp.path().join("repo");
        let rules = repo.join(".claude").join("rules");
        fs::create_dir_all(&rules).unwrap();
        touch(&repo, "CLAUDE.md");
        touch(&rules, "r.md");

        // rules/loop -> rules (cycle). Skip the test if symlinks are unsupported.
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&rules, rules.join("loop")).is_ok();
        #[cfg(not(unix))]
        let made = false;

        let h = walk(&repo, &home);
        let order = splice_order(&h, tmp.path());
        // Must terminate and still surface r.md (exactly once).
        assert!(order.contains(&"repo/.claude/rules/r.md".to_string()));
        assert_eq!(
            order.iter().filter(|p| p.ends_with("r.md")).count(),
            1,
            "r.md emitted exactly once despite the cycle (made_symlink={made})"
        );
    }

    #[test]
    fn case_mismatch_flag_set_on_lowercased_filename() {
        // On a case-sensitive filesystem we simulate a mismatch by writing
        // the file as `claude.md` (all-lowercase) and asserting exact_case=false.
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("repo");
        fs::create_dir_all(&cwd).unwrap();
        touch(&cwd, "claude.md");
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();

        let h = walk(&cwd, &home);
        assert_eq!(h.entries.len(), 1, "lowercased file must still be found");
        assert!(!h.entries[0].exact_case, "exact_case must be false");
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use telemetry::{sink::LogEventMetadata, AnalyticsBus, AnalyticsSink, AnalyticsValue};

    struct CapturingSink {
        events: Mutex<Vec<(String, LogEventMetadata)>>,
    }

    #[async_trait::async_trait]
    impl AnalyticsSink for CapturingSink {
        async fn log_event(&self, name: &str, metadata: LogEventMetadata) {
            self.events.lock().unwrap().push((name.into(), metadata));
        }
        async fn log_event_async(&self, name: &str, metadata: LogEventMetadata) {
            self.log_event(name, metadata).await;
        }
        fn name(&self) -> &str {
            "capturing"
        }
    }

    #[tokio::test]
    async fn emit_case_mismatch_writes_event_with_pii_tagged_path() {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(CapturingSink {
            events: Mutex::new(Vec::new()),
        });
        bus.attach_sink(sink.clone()).await;

        emit_case_mismatch(
            Some(&bus),
            std::path::Path::new("/Users/u/proj/claude.md"),
            "claude.md",
        )
        .await;

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let (name, md) = &events[0];
        assert_eq!(name, "tengu_memory_case_mismatch");
        assert!(md.contains_key("_PROTO_path"));
        match md.get("actual") {
            Some(AnalyticsValue::String(s)) => assert_eq!(s, "claude.md"),
            _ => panic!("actual must be a string"),
        }
    }
}
