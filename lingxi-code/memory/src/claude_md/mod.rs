//! CLAUDE.md hierarchy loader.
//!
//! Discovers the full claude-code memory-file set (`getMemoryFiles`): the
//! Managed tier (`<managed>/CLAUDE.md` + `.claude/rules/**`), the User tier
//! (`~/.claude/CLAUDE.md` + `~/.claude/rules/**`), and, per directory from the
//! filesystem root down to cwd, `CLAUDE.md`, `.claude/CLAUDE.md`,
//! `.claude/rules/**`, and `CLAUDE.local.md`. Each file is loaded with a 10 MB
//! per-file cap. See [`hierarchy::walk`].

pub mod hierarchy;
pub mod loader;

pub use hierarchy::{Hierarchy, HierarchyEntry};
pub use loader::{LoadedFile, LoaderError};
