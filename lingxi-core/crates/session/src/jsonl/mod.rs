//! On-disk JSONL transcript format — byte-equivalent to claude-code's
//! `~/.claude/projects/<sanitized-cwd>[-<djb2>]/<session-uuid>.jsonl`.
//!
//! Submodules:
//! - `djb2` — modified-djb2 hash (1:1 port of `claude-code/src/utils/hash.ts`).
//! - `uuid` — UUID v4 validation regex from `sessionStoragePortable.ts:23-24`.
//! - `path` — project-dir resolver (sanitize-path + djb2-suffix fallback).
//! - `schema` — `JsonlMessage` struct with the locked outer field set.
//! - `writer` — append-only `JsonlWriter` (one line = one JSON object + `\n`).
//! - `reader` — full `read_all` + 64 KB-head `read_lite` byte-byte algorithms.
//! - `recover` — pre-existing crash-recovery reader (M1/M3 surface, unchanged).

pub mod djb2;
pub mod path;
pub mod reader;
pub mod recover;
pub mod schema;
pub mod uuid;
pub mod writer;

// Re-export pre-existing public surface so dependents keep their imports.
pub use recover::{read_recover, RecoveryResult, StorageError};

// New M5-07 public surface.
pub use path::{project_dir_name, session_path};
pub use reader::{JsonlReader, SessionMetadata};
pub use schema::JsonlMessage;
pub use uuid::validate_uuid;
pub use writer::JsonlWriter;

/// Size of the head buffer for lite metadata reads — 64 KiB.
/// Byte-locked to `claude-code/src/utils/sessionStoragePortable.ts:17`
/// (`LITE_READ_BUF_SIZE = 65536`).
pub const LITE_READ_BUF_SIZE: usize = 65_536;
