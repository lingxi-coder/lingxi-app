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
    /// Terminal agent status for lifecycle records. Ordinary conversation
    /// entries omit this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Terminal failure detail. Kept out of ordinary message entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional display name of the spawned agent. Older transcripts omit
    /// this field; hosts fall back to the parked task row or agent id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Resolved agent type (for example `general-purpose`). Persisting this
    /// beside messages lets a live tail expose metadata before a parked task
    /// row exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Concrete provider-local model used by this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider profile paired with [`Self::model`], when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,
}

/// Appends [`TranscriptEntry`] lines to a per-agent transcript file.
pub struct AgentTranscriptWriter {
    /// Absolute path the transcript is written to.
    pub transcript_path: PathBuf,
    /// Owning agent id; stamped on every entry.
    pub agent_id: AgentId,
    /// Sandboxed filesystem used to read/write the transcript.
    fs: Arc<dyn FileSystem>,
    agent_name: Option<String>,
    agent_type: Option<String>,
    model: Option<String>,
    model_profile: Option<String>,
}

impl AgentTranscriptWriter {
    /// Construct a new writer that targets `transcript_path` for `agent_id`.
    #[must_use]
    pub fn new(transcript_path: PathBuf, agent_id: AgentId, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            transcript_path,
            agent_id,
            fs,
            agent_name: None,
            agent_type: None,
            model: None,
            model_profile: None,
        }
    }

    /// Attach display metadata that is copied onto each entry. This remains a
    /// builder so existing minimal/test callers can keep the old constructor.
    #[must_use]
    pub fn with_metadata(
        mut self,
        agent_name: Option<String>,
        agent_type: Option<String>,
        model: Option<String>,
        model_profile: Option<String>,
    ) -> Self {
        self.agent_name = agent_name;
        self.agent_type = agent_type;
        self.model = model;
        self.model_profile = model_profile;
        self
    }

    /// Append one [`TranscriptEntry`] for `message`.
    pub async fn record(&self, message: &ConversationMessage) -> Result<(), traits::FsError> {
        let entry = TranscriptEntry {
            agent_id: self.agent_id,
            timestamp: SystemTime::now(),
            message: message.clone(),
            status: None,
            error: None,
            agent_name: self.agent_name.clone(),
            agent_type: self.agent_type.clone(),
            model: self.model.clone(),
            model_profile: self.model_profile.clone(),
        };
        self.append_entry(&entry).await
    }

    /// Append a terminal lifecycle entry while retaining a normal transcript
    /// message shape for readers that replay only `message` values.
    pub async fn record_terminal(
        &self,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), traits::FsError> {
        let detail = error.unwrap_or(status);
        let entry = TranscriptEntry {
            agent_id: self.agent_id,
            timestamp: SystemTime::now(),
            message: ConversationMessage::System {
                id: protocol::MessageId::new(),
                content: detail.to_string(),
                subtype: Some(format!("agent_{status}")),
                compact_metadata: None,
            },
            status: Some(status.to_string()),
            error: error.map(str::to_string),
            agent_name: self.agent_name.clone(),
            agent_type: self.agent_type.clone(),
            model: self.model.clone(),
            model_profile: self.model_profile.clone(),
        };
        self.append_entry(&entry).await
    }

    async fn append_entry(&self, entry: &TranscriptEntry) -> Result<(), traits::FsError> {
        let line = format!(
            "{}\n",
            serde_json::to_string(entry).expect("transcript serialization")
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
