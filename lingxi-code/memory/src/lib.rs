//! 4-tier memory subsystem for `LingXi` Core.
//!
//! v0.4.0 (M3-02) extends the v0.3.0 scaffold with the production
//! `claude_md` hierarchy loader, `memdir` scanner + fixed-point u64
//! ranker, real `team_paths` resolution, and a thin secret-scan adapter
//! over the v3 §16.5 `lingxi-secret` rule set.

#![forbid(unsafe_code)]

pub mod claude_md;
pub mod file;
pub mod memdir;
pub mod prefetch;
pub mod secret_scan;
pub mod selector;
pub mod session_memory;
pub mod snapshot;
pub mod surfacing;
pub mod team_memory;
pub mod tier;

pub use file::{
    parse_markdown_with_frontmatter, MemoryError, MemoryFile, MemoryFrontmatter,
    MAX_ENTRYPOINT_BYTES, MAX_ENTRYPOINT_LINES,
};
pub use tier::MemoryTier;

// ------ M3-02 wire-identifier constants ------

/// Per-file cap (10 MB). Files larger than this are skipped with
/// `tengu_memory_file_too_large`.
///
/// NB: this is the **memdir** scanner cap, NOT the LINGXI.md hierarchy loader,
/// which reads every file whole (no size drop — parity with claude-code
/// `safelyReadMemoryFileAsync`). See [`get_large_memory_files`] for the
/// non-blocking 40k-char *warning* the LINGXI.md path surfaces instead.
pub const MAX_MEMORY_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Recommended maximum character count for a single memory file
/// (claude-code `MAX_MEMORY_CHARACTER_COUNT`, claudemd.ts:91-92).
///
/// This is a soft, non-blocking recommendation: files over this size are still
/// loaded in full. [`get_large_memory_files`] flags them so a caller can warn
/// the user, exactly as claude-code does — it never drops the file.
pub const MAX_MEMORY_CHARACTER_COUNT: usize = 40_000;

/// Return the subset of `files` whose body exceeds
/// [`MAX_MEMORY_CHARACTER_COUNT`] characters.
///
/// 1:1 with claude-code `getLargeMemoryFiles` (claudemd.ts:1132-1134):
/// `files.filter(f => f.content.length > MAX_MEMORY_CHARACTER_COUNT)`. This is
/// a **warning** list — the returned files are NOT removed from the memory set;
/// every file is still loaded whole. The character count uses Unicode scalar
/// values (`chars().count()`), matching the JS `String.length`-style intent of
/// "characters" closely enough for the human-facing warning.
#[must_use]
pub fn get_large_memory_files(files: &[MemoryFile]) -> Vec<&MemoryFile> {
    files
        .iter()
        .filter(|f| f.content.chars().count() > MAX_MEMORY_CHARACTER_COUNT)
        .collect()
}

/// Age penalty unit in days. `age_blocks = age_days / 30`.
pub const MEMORY_AGE_PENALTY_DAYS: u64 = 30;

/// Hard-drop threshold (365 days). Scan-time hygiene; the only drop in
/// the loader path. Age otherwise penalizes, never drops.
pub const MEMORY_AGE_HARD_DROP_DAYS: u64 = 365;

/// Floor on age weight (bps). Very old entries still reachable at 10%.
pub const MEMORY_MIN_AGE_WEIGHT_BPS: u32 = 1_000;

/// Default top-k for `find_relevant`. Caller-overridable.
pub const DEFAULT_RELEVANT_MEMORIES: usize = 5;
