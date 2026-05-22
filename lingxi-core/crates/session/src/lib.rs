//! Session storage + crash-safe transcript reader + resumer.
//!
//! See spec §22 (`SessionMetadata`, append-only JSONL with fsync + flock,
//! crash-safe reader, `SessionResumer`).

#![forbid(unsafe_code)]

pub mod jsonl;
pub mod metadata;
pub mod resumer;
pub mod storage;
pub mod transcript;

pub use jsonl::{read_recover, RecoveryResult, StorageError};
pub use metadata::SessionMetadata;
pub use resumer::{ResumeError, ResumedSession, SessionResumer};
pub use storage::{LoadedSession, SessionStorage};
pub use transcript::TranscriptEntry;
