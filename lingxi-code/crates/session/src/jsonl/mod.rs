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
//! - `loader` — M5-08 resume enumeration + chain validation + interactive picker.
//! - `title` — M5-08 first-user-message title extraction.

pub mod djb2;
pub mod loader;
pub mod path;
pub mod reader;
pub mod recover;
pub mod schema;
pub mod title;
pub mod uuid;
pub mod writer;

// Re-export pre-existing public surface so dependents keep their imports.
pub use recover::{read_recover, RecoveryResult, StorageError};

// New M5-07 public surface.
pub use path::{project_dir_name, session_path};
// `reader::SessionMetadata` (lite head-only struct) is intentionally NOT
// re-exported as `jsonl::SessionMetadata`; M5-08 introduces a different
// `SessionMetadata` for the resume picker (uuid + title + mtime + line
// count). Callers that need the M5-07 type reach it via
// `lingxi_session::JsonlSessionMetadata` (crate-root alias) or the
// fully-qualified `lingxi_session::jsonl::reader::SessionMetadata`.
pub use reader::JsonlReader;
pub use schema::JsonlMessage;
pub use uuid::validate_uuid;
pub use writer::JsonlWriter;

// New M5-08 public surface.
pub use loader::{
    list_recent_sessions, load_session, select_session_interactive, LoaderError, SessionMetadata,
};
pub use title::extract_title;

/// Size of the head buffer for lite metadata reads — 64 KiB.
/// Byte-locked to `claude-code/src/utils/sessionStoragePortable.ts:17`
/// (`LITE_READ_BUF_SIZE = 65536`).
pub const LITE_READ_BUF_SIZE: usize = 65_536;
