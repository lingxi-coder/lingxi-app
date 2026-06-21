//! CLAUDE.md hierarchy loader.
//!
//! Discovers the full claude-code memory-file set (`getMemoryFiles`): the
//! Managed tier (`<managed>/CLAUDE.md` + `.claude/rules/**`), the User tier
//! (`~/.claude/CLAUDE.md` + `~/.claude/rules/**`), and, per directory from the
//! filesystem root down to cwd, `CLAUDE.md`, `.claude/CLAUDE.md`,
//! `.claude/rules/**`, and `CLAUDE.local.md`. Each file is read whole (no size
//! cap — parity with claude-code `readFile`). See [`hierarchy::walk`].

pub mod excludes;
pub mod hierarchy;
pub mod loader;

pub use excludes::ClaudeMdExcluder;
pub use hierarchy::{user_config_dir, Hierarchy, HierarchyEntry};
pub use loader::{LoadedFile, LoaderError};

/// Which CLAUDE.md tier a discovered file belongs to.
///
/// 1:1 with claude-code `MemoryType` (`utils/memory/types.ts`) restricted to
/// the four instruction tiers this port loads — the separate memdir tiers
/// (`AutoMem` / `TeamMem`) are out of scope. The tier drives two things:
///
/// - **splice / discovery order** (`getMemoryFiles`, claudemd.ts:803-934):
///   Managed first, then User, then Project, then Local.
/// - **the injection description** (`getClaudeMds`, claudemd.ts:1168-1186):
///   Managed and User share the "private global instructions" wording; Project
///   and Local each have their own.
///
/// DISTINCT from [`crate::tier::MemoryTier`], which models the separate memdir
/// subsystem (Project/User/Session/Team). Do NOT overload one for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeMdTier {
    /// Enterprise / managed policy memory (`<managed>/CLAUDE.md` +
    /// `<managed>/.claude/rules/**`). Always loaded, never settings-gated,
    /// never excludable; lowest priority (spliced first).
    Managed,
    /// User-global memory (`~/.claude/CLAUDE.md` + `~/.claude/rules/**`).
    User,
    /// Project memory checked into the codebase (`CLAUDE.md`,
    /// `.claude/CLAUDE.md`, `.claude/rules/**`).
    Project,
    /// Private project-local override (`CLAUDE.local.md`), not checked in.
    Local,
}
