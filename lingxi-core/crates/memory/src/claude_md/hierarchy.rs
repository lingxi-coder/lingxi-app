//! Dir-up walker: cwd → parents → user-home. Filled in Task 2.

use std::path::PathBuf;

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
