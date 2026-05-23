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

/// Snapshot of discovered CLAUDE.md locations, in walk order
/// (innermost first: cwd, then each parent, then user home).
#[derive(Debug, Default)]
pub struct Hierarchy {
    /// Discovered entries in walk order.
    pub entries: Vec<HierarchyEntry>,
}

/// Walk cwd → parents → `<home>/.claude` collecting CLAUDE.md files.
///
/// At each directory, `CLAUDE.local.md` (if present) is emitted FIRST,
/// then `CLAUDE.md`. The walk visits each directory at most once.
/// Returns an empty hierarchy when no files exist (NOT an error).
#[must_use]
pub fn walk(cwd: &Path, home: &Path) -> Hierarchy {
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut walker: Option<&Path> = Some(cwd);
    while let Some(dir) = walker {
        if seen.insert(dir.to_path_buf()) {
            collect_in_dir(dir, &mut entries);
        }
        walker = dir.parent();
    }

    let user_dir = home.join(".claude");
    if seen.insert(user_dir.clone()) {
        collect_in_dir(&user_dir, &mut entries);
    }

    Hierarchy { entries }
}

fn collect_in_dir(dir: &Path, out: &mut Vec<HierarchyEntry>) {
    // CLAUDE.local.md first (so it shadows the canonical entry).
    if let Some(entry) = probe(dir, LOCAL_OVERRIDE_NAME, true) {
        out.push(entry);
    }
    if let Some(entry) = probe(dir, FILE_NAME, false) {
        out.push(entry);
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
    bus: Option<&Arc<lingxi_telemetry::AnalyticsBus>>,
    path: &std::path::Path,
    actual: &str,
) {
    let Some(bus) = bus else {
        return;
    };
    let mut md = lingxi_telemetry::sink::LogEventMetadata::new();
    md.insert(
        "_PROTO_path".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::pii::PiiTagged::assert_pii_tagged_column(
                path.display().to_string(),
            )
            .into_inner(),
        ),
    );
    md.insert(
        "actual".into(),
        lingxi_telemetry::sink::AnalyticsValue::String(
            lingxi_telemetry::pii::Verified::assert_safe(actual.to_string()).into_inner(),
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
    use lingxi_telemetry::{sink::LogEventMetadata, AnalyticsBus, AnalyticsSink, AnalyticsValue};
    use std::sync::{Arc, Mutex};

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
