//! CLAUDE.md hierarchy loader.
//!
//! Walks cwd → parents → user home, loading `CLAUDE.md` and
//! `CLAUDE.local.md` at each level with a 10 MB per-file cap.

pub mod hierarchy;
pub mod loader;

pub use hierarchy::{Hierarchy, HierarchyEntry};
pub use loader::{LoadedFile, LoaderError};
