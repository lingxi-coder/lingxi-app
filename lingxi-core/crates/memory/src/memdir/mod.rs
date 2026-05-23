//! Memdir scanner + fixed-point u64 ranker.
//!
//! Enumerates `~/.claude/memdir/` (+ optional `~/.claude/team-mem/`),
//! drops entries older than 365 days, and ranks survivors by
//! `jaccard × age_weight × tier_weight × team_boost` in bps.

pub mod age;
pub mod find;
pub mod paths;
pub mod scan;
pub mod team_paths;
pub mod team_prompts;

pub use age::age_weight_bps;
pub use find::{find_relevant, RelevanceInputs};
pub use paths::{memdir_path, MemdirRoots, MEMDIR_SUBDIR, TEAM_MEM_SUBDIR};
pub use scan::{scan_memdir, MemdirSnapshot};
