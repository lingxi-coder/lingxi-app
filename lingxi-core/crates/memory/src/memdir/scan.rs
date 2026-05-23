//! Memdir enumeration + 365-day hard drop.

use crate::{MAX_MEMORY_FILE_SIZE, MEMORY_AGE_HARD_DROP_DAYS};
use lingxi_protocol::{MemoryEntry, MemoryEntryTier};
use std::path::Path;
use std::time::SystemTime;

/// Snapshot of memdir scan result.
#[derive(Debug, Default)]
pub struct MemdirSnapshot {
    /// Entries surviving the 365-day hygiene drop. Order is insertion
    /// order (user dir then team dir, lexicographic within each).
    pub entries: Vec<MemoryEntry>,
}

/// Scan with `now = SystemTime::now()`. Convenience wrapper around
/// [`scan_memdir_at`].
///
/// # Errors
///
/// Returns `std::io::Error` only for directory-iteration errors that
/// aren't `NotFound` (missing dirs are silently treated as empty).
pub fn scan_memdir(roots: &super::paths::MemdirRoots) -> std::io::Result<MemdirSnapshot> {
    scan_memdir_at(roots, SystemTime::now())
}

/// Scan with explicit `now` (testable). Files older than
/// `MEMORY_AGE_HARD_DROP_DAYS` are dropped; oversized files (>10 MB) are
/// dropped too (their event is emitted by the caller through
/// [`crate::claude_md::loader::emit_file_too_large`] for hierarchy files;
/// memdir files emit the same event via the loader path used by the
/// engine wrapper — see Task 13).
///
/// # Errors
///
/// Returns `std::io::Error` for directory-iteration errors other than
/// `NotFound`.
pub fn scan_memdir_at(
    roots: &super::paths::MemdirRoots,
    now: SystemTime,
) -> std::io::Result<MemdirSnapshot> {
    let mut entries = Vec::new();
    enumerate(&roots.user_memdir, MemoryEntryTier::User, now, &mut entries)?;
    if let Some(team) = roots.team_memdir.as_deref() {
        enumerate(team, MemoryEntryTier::Team, now, &mut entries)?;
    }
    Ok(MemdirSnapshot { entries })
}

fn enumerate(
    dir: &Path,
    tier: MemoryEntryTier,
    now: SystemTime,
    out: &mut Vec<MemoryEntry>,
) -> std::io::Result<()> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // Sort by filename for deterministic ordering.
    let mut paths: Vec<_> = read
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() as usize > MAX_MEMORY_FILE_SIZE {
            // Oversized — skip (caller emits tengu_memory_file_too_large
            // via the loader wrapper in Task 13).
            continue;
        }
        let mtime = meta.modified().unwrap_or(now);
        let age_days = now
            .duration_since(mtime)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0);
        if age_days > MEMORY_AGE_HARD_DROP_DAYS {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        out.push(MemoryEntry {
            path,
            tier,
            body,
            age_days,
            size_bytes: meta.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memdir::paths::{memdir_path, MemdirRoots};
    use crate::MEMORY_AGE_HARD_DROP_DAYS;
    use lingxi_protocol::MemoryEntryTier;
    use std::fs;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    fn write_dated(path: &std::path::Path, bytes: &[u8], age_days: u64) {
        fs::write(path, bytes).unwrap();
        let mtime = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(mtime)).unwrap();
    }

    #[test]
    fn scan_drops_entries_older_than_365_days() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let memdir = home.join(".claude").join("memdir");
        fs::create_dir_all(&memdir).unwrap();
        write_dated(&memdir.join("fresh.md"), b"fresh\n", 10);
        write_dated(&memdir.join("stale.md"), b"stale\n", MEMORY_AGE_HARD_DROP_DAYS + 1);

        let roots = memdir_path(home, false);
        let snap = scan_memdir_at(&roots, SystemTime::now()).unwrap();
        let names: Vec<_> = snap
            .entries
            .iter()
            .filter_map(|e| e.path.file_name().and_then(|n| n.to_str().map(String::from)))
            .collect();
        assert!(names.contains(&"fresh.md".into()));
        assert!(!names.contains(&"stale.md".into()), "stale dropped at scan");
    }

    #[test]
    fn user_tier_assigned_for_user_memdir() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let memdir = home.join(".claude").join("memdir");
        fs::create_dir_all(&memdir).unwrap();
        fs::write(memdir.join("u.md"), b"u\n").unwrap();
        let roots = memdir_path(home, false);
        let snap = scan_memdir_at(&roots, SystemTime::now()).unwrap();
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0].tier, MemoryEntryTier::User);
    }

    #[test]
    fn team_tier_assigned_for_team_memdir() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let team = home.join(".claude").join("team-mem");
        fs::create_dir_all(&team).unwrap();
        fs::write(team.join("t.md"), b"t\n").unwrap();
        let roots = memdir_path(home, true);
        let snap = scan_memdir_at(&roots, SystemTime::now()).unwrap();
        assert!(snap.entries.iter().any(|e| e.tier == MemoryEntryTier::Team));
    }

    #[test]
    fn missing_dirs_yield_empty_snapshot() {
        let tmp = TempDir::new().unwrap();
        let roots = MemdirRoots {
            user_memdir: tmp.path().join("does/not/exist"),
            team_memdir: None,
        };
        let snap = scan_memdir_at(&roots, SystemTime::now()).unwrap();
        assert!(snap.entries.is_empty());
    }
}
