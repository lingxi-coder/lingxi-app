//! Trait-level contract test suites. Each `MockX` (or production) impl runs
//! through these to verify it satisfies the trait's invariants.
//!
//! M1.23 ships the filesystem contract; the remaining 12 traits use the same
//! pattern and land in M2 (`process`, `http`, `mcp`, `worktree`, `swarm`,
//! `secure_storage`, `sandbox`, `lsp`, `bridge`, `runtime`, `clock`,
//! `hook_broadcaster`).

pub mod filesystem;
