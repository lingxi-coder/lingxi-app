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
pub const MAX_MEMORY_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Age penalty unit in days. `age_blocks = age_days / 30`.
pub const MEMORY_AGE_PENALTY_DAYS: u64 = 30;

/// Hard-drop threshold (365 days). Scan-time hygiene; the only drop in
/// the loader path. Age otherwise penalizes, never drops.
pub const MEMORY_AGE_HARD_DROP_DAYS: u64 = 365;

/// Floor on age weight (bps). Very old entries still reachable at 10%.
pub const MEMORY_MIN_AGE_WEIGHT_BPS: u32 = 1_000;

/// Default top-k for `find_relevant`. Caller-overridable.
pub const DEFAULT_RELEVANT_MEMORIES: usize = 5;
