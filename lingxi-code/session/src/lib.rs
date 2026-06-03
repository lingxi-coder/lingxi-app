//! Session storage + crash-safe transcript reader + resumer.
//!
//! See spec §22 (`SessionMetadata`, append-only JSONL with fsync + flock,
//! crash-safe reader, `SessionResumer`).

#![forbid(unsafe_code)]

pub mod agent_color;
pub mod jsonl;
pub mod metadata;
pub mod resumer;
pub mod storage;
pub mod transcript;

// Pre-existing surface — preserved bit-for-bit so M1-M4 downstream compiles.
pub use jsonl::{read_recover, RecoveryResult, StorageError};

// `/color` persistence (claude-code `saveAgentColor`).
pub use agent_color::{agent_color_entry, last_agent_color, save_agent_color};
pub use metadata::SessionMetadata;
pub use resumer::{ResumeError, ResumedSession, SessionResumer};
pub use storage::{LoadedSession, SessionStorage};
pub use transcript::TranscriptEntry;

// New M5-07 surface — distinct name (`JsonlSessionMetadata`) so it does NOT
// collide with the pre-existing `metadata::SessionMetadata` re-export.
pub use jsonl::reader::SessionMetadata as JsonlSessionMetadata;
pub use jsonl::{
    project_dir_name, session_path, validate_uuid, JsonlMessage, JsonlReader, JsonlWriter,
    LITE_READ_BUF_SIZE,
};
