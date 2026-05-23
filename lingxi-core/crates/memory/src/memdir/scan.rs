//! Memdir enumeration + 365-day hard drop. Filled in Task 6.

use lingxi_protocol::MemoryEntry;

/// Snapshot of memdir scan result.
#[derive(Debug, Default)]
pub struct MemdirSnapshot {
    /// Entries surviving the 365-day hygiene drop. Order is insertion
    /// order; consumers do their own ranking.
    pub entries: Vec<MemoryEntry>,
}

/// Placeholder; real impl in Task 6.
pub fn scan_memdir(_roots: &super::paths::MemdirRoots) -> std::io::Result<MemdirSnapshot> {
    unimplemented!("filled in Task 6")
}
