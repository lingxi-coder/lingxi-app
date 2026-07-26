//! Session storage + crash-safe transcript reader + resumer.
//!
//! See spec §22 (`SessionMetadata`, append-only JSONL with fsync + flock,
//! crash-safe reader, `SessionResumer`).

#![forbid(unsafe_code)]

pub mod agent_color;
pub mod agent_rows;
pub mod branch;
pub mod file_history;
pub mod forked_skill;
pub mod filestate;
pub mod jsonl;
pub mod metadata;
pub mod resumer;
pub mod rewind;
pub mod rollout;
pub mod storage;
pub mod transcript;

// Pre-existing surface — preserved bit-for-bit so M1-M4 downstream compiles.
pub use jsonl::{read_recover, RecoveryResult, StorageError};

// `/color` persistence (claude-code `saveAgentColor`).
pub use agent_color::{agent_color_entry, last_agent_color, save_agent_color};
pub use branch::{create_branch, BranchError, BranchResult};
pub use file_history::{FileHistory, FileHistoryBackup, SnapshotRecord};
pub use metadata::SessionMetadata;
pub use resumer::{ResumeError, ResumedRollout, ResumedSession, SessionResumer};
pub use rewind::rewind_conversation;
pub use storage::{LoadedSession, SessionStorage};
pub use transcript::TranscriptEntry;

// New M5-07 surface — distinct name (`JsonlSessionMetadata`) so it does NOT
// collide with the pre-existing `metadata::SessionMetadata` re-export.
pub use jsonl::reader::SessionMetadata as JsonlSessionMetadata;
pub use jsonl::{
    project_dir_name, session_path, validate_uuid, JsonlMessage, JsonlReader, JsonlWriter,
    LITE_READ_BUF_SIZE,
};

// Rollout RECORD capability (codex `rollout` crate recorder semantics merged
// into the `session` crate). Re-exported under the `rollout` module name to
// avoid colliding with the byte-locked claude-code transcript surface above.
pub use rollout::{
    append_rollout_item_to_path, append_thread_name, builder_from_items, find_thread_name_by_id,
    find_thread_names_by_ids, is_persisted_rollout_item, parse_timestamp_uuid_from_filename,
    persisted_rollout_items, plain_rollout_path, remove_thread_name_entries, rollout_date_parts,
    GitInfo, InitialHistory, ResumedHistory, RolloutItem, RolloutLine, RolloutRecorder,
    RolloutRecorderParams, SessionMeta as RolloutSessionMeta, SessionMetaLine, SessionSource,
    ThreadMetadataBuilder, ARCHIVED_SESSIONS_SUBDIR, SESSIONS_SUBDIR,
};
