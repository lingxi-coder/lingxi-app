//! Per-agent transcript writer.
//!
//! [`AgentTranscriptWriter`] appends one JSON line per
//! [`ConversationMessage`] to a transcript file under the agent's transcript
//! subdir. The full append-only [`lingxi_traits::FileSystem::append_file`]
//! lands in Plan 10; today we read-then-rewrite to keep the public surface
//! stable.

use lingxi_protocol::{AgentId, ConversationMessage};
use lingxi_traits::FileSystem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

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
///
/// Today this is a thin wrapper over [`FileSystem`]; Plan 10 replaces the
/// read-then-write hack with a real append API.
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
    pub async fn record(&self, message: &ConversationMessage) -> Result<(), lingxi_traits::FsError> {
        let entry = TranscriptEntry {
            agent_id: self.agent_id,
            timestamp: SystemTime::now(),
            message: message.clone(),
        };
        let line = format!("{}\n", serde_json::to_string(&entry).expect("transcript serialization"));
        // FileSystem.append_file added in Plan 10. For now use write+read concat.
        let path_str = self.transcript_path.to_str().expect("utf-8 transcript path");
        let existing = self
            .fs
            .read_file(path_str, None, None)
            .await
            .map(|fc| fc.content)
            .unwrap_or_default();
        self.fs.write_file(path_str, &(existing + &line)).await
    }
}
