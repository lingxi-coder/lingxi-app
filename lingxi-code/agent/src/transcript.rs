//! Per-agent transcript writer.
//!
//! [`AgentTranscriptWriter`] appends one JSON line per
//! [`ConversationMessage`] to a transcript file under the agent's transcript
//! subdir.

use protocol::{AgentId, ConversationMessage};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use traits::FileSystem;

/// One line in the agent transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Owning agent.
    pub agent_id: AgentId,
    /// Wall-clock timestamp the line was recorded.
    pub timestamp: SystemTime,
    /// The conversation message itself.
    pub message: ConversationMessage,
}

/// Appends [`TranscriptEntry`] lines to a per-agent transcript file.
pub struct AgentTranscriptWriter {
    /// Absolute path the transcript is written to.
    pub transcript_path: PathBuf,
    /// Owning agent id; stamped on every entry.
    pub agent_id: AgentId,
    /// Sandboxed filesystem used to read/write the transcript.
    fs: Arc<dyn FileSystem>,
}

impl AgentTranscriptWriter {
    /// Construct a new writer that targets `transcript_path` for `agent_id`.
    #[must_use]
    pub fn new(transcript_path: PathBuf, agent_id: AgentId, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            transcript_path,
            agent_id,
            fs,
        }
    }

    /// Append one [`TranscriptEntry`] for `message`.
    pub async fn record(&self, message: &ConversationMessage) -> Result<(), traits::FsError> {
        let entry = TranscriptEntry {
            agent_id: self.agent_id,
            timestamp: SystemTime::now(),
            message: message.clone(),
        };
        let line = format!(
            "{}\n",
            serde_json::to_string(&entry).expect("transcript serialization")
        );
        let path_str = self
            .transcript_path
            .to_str()
            .expect("utf-8 transcript path");
        // A real APPEND, not read-then-rewrite. The old hack round-tripped the
        // whole file through `read_file`, whose returned view is not
        // guaranteed byte-identical to the file — concatenating onto it
        // corrupted the JSONL — and rewrote every prior line on each message,
        // making a long conversation quadratic.
        self.fs.append_file(path_str, &line).await
    }
}
