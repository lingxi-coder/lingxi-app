//! Session storage + crash-safe transcript reader + resumer.
//!
//! See spec §22 (`SessionMetadata`, append-only JSONL with fsync + flock,
//! crash-safe reader, `SessionResumer`).

#![forbid(unsafe_code)]

pub mod agent_color;
pub mod agent_rows;
pub mod branch;
pub mod file_history;
pub mod filestate;
pub mod forked_skill;
pub mod jsonl;
pub mod metadata;
pub mod prompt_history;
pub mod rate_limit_checkpoint;
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
// Rate-limit resume checkpoint (SC-02): the `RESUME.md` document, the git
// executor (`q4v`) and the once-per-session latch (`z4v`). The `rate_limited`
// trigger lives in `orchestrator::turn_loop`; see the module docs for the
// `near_limit` twin that still needs a caller.
pub use rate_limit_checkpoint::{
    checkpoint_ref, clear_last_checkpoint_result, last_checkpoint_result,
    local_checkpoint_commit_allowed, perform_rate_limit_checkpoint, render_resume_md,
    sanitize_todo_line, CheckpointGates,
    CheckpointRequest, CheckpointResult, CheckpointSkipReason, CheckpointTrigger, ResumeDoc,
    ALLOW_LOCAL_CHECKPOINT_COMMIT_ENV, CHECKPOINT_REF_PREFIX, MAX_CHECKPOINT_FILE_COUNT,
    MAX_CHECKPOINT_TOTAL_BYTES, RESUME_MD_REPO_PATH,
};
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
// SC-08 — transcript-file compaction (`performCompactTranscript`). The trigger
// lives on `JsonlWriter::maybe_compact_transcript`; these are the surface an
// embedder needs to inspect or drive one directly.
pub use jsonl::{
    perform_compact_transcript, CompactFailure, CompactOutcome, CompactStats,
    COMPACT_BACKSTOP_BYTES, MIN_COMPACT_FILE_BYTES, TRANSCRIPT_LOCAL_GC_ENV,
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
