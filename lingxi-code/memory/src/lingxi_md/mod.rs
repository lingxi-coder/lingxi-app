//! LINGXI.md hierarchy loader.
//!
//! Discovers the full claude-code memory-file set (`getMemoryFiles`): the
//! Managed tier (`<managed>/LINGXI.md` + `.lingxi/rules/**`), the User tier
//! (`~/.lingxi/LINGXI.md` + `~/.lingxi/rules/**`), and, per directory from the
//! filesystem root down to cwd, `LINGXI.md`, `.lingxi/LINGXI.md`,
//! `.lingxi/rules/**`, and `LINGXI.local.md`. Each file is read whole (no size
//! cap — parity with claude-code `readFile`). See [`hierarchy::walk`].

pub mod excludes;
pub mod hierarchy;
pub mod loader;

pub use excludes::LingxiMdExcluder;
pub use hierarchy::{user_config_dir, Hierarchy, HierarchyEntry};
pub use loader::{LoadedFile, LoaderError};

/// Which LINGXI.md tier a discovered file belongs to.
///
/// 1:1 with claude-code `MemoryType` (`utils/memory/types.ts`) restricted to
/// the four instruction tiers this port loads — the separate memdir tiers
/// (`AutoMem` / `TeamMem`) are out of scope. The tier drives two things:
///
/// - **splice / discovery order** (`getMemoryFiles`, claudemd.ts:803-934):
///   Managed first, then User, then Project, then Local.
/// - **the injection description** (`getLingxiMds`, claudemd.ts:1168-1186):
///   Managed and User share the "private global instructions" wording; Project
///   and Local each have their own.
///
/// DISTINCT from [`crate::tier::MemoryTier`], which models the separate memdir
/// subsystem (Project/User/Session/Team). Do NOT overload one for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LingxiMdTier {
    /// Enterprise / managed policy memory (`<managed>/LINGXI.md` +
    /// `<managed>/.lingxi/rules/**`). Always loaded, never settings-gated,
    /// never excludable; lowest priority (spliced first).
    Managed,
    /// User-global memory (`~/.lingxi/LINGXI.md` + `~/.lingxi/rules/**`).
    User,
    /// Project memory checked into the codebase (`LINGXI.md`,
    /// `.lingxi/LINGXI.md`, `.lingxi/rules/**`).
    Project,
    /// Private project-local override (`LINGXI.local.md`), not checked in.
    Local,
}
