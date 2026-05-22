//! IDE bridge wire messages.
//!
//! The 9 variants in [`BridgeMessage`] are the full vocabulary the engine and
//! an IDE plugin exchange over [`crate::transport::IdeBridge`]. The first five
//! flow IDE → CLI, the next four flow CLI → IDE, and `Heartbeat` is
//! bidirectional. See spec §29.2.

use lingxi_protocol::ToolUseId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Tagged enum carrying every payload type that crosses the bridge.
///
/// Wire format is JSON with `#[serde(tag = "type", rename_all = "snake_case")]`
/// so each message looks like `{"type": "user_prompt", "text": ..., ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeMessage {
    // ---------- IDE → CLI ----------
    /// User typed a prompt in the IDE's chat panel.
    UserPrompt {
        /// Raw prompt text.
        text: String,
        /// Optional structured attachments (images, file refs, ...).
        attachments: Vec<serde_json::Value>,
    },
    /// IDE asks the CLI to open a file (typically from a deep-link).
    OpenFileRequest {
        /// Absolute or workspace-relative path.
        path: PathBuf,
        /// Optional 1-based line number to scroll to.
        line: Option<u32>,
    },
    /// IDE's reply to a CLI-initiated current-file query.
    GetCurrentFileResponse {
        /// Path of the file currently focused in the IDE.
        path: PathBuf,
        /// Full text content of that file.
        content: String,
    },
    /// User answered a permission prompt in the IDE UI.
    PermissionDecision {
        /// Which tool call this decision belongs to.
        tool_use_id: ToolUseId,
        /// Free-form decision string (e.g. `allow`, `deny`, `allow_always`).
        decision: String,
    },
    /// User asked the IDE to abort the in-flight turn.
    AbortRequest,

    // ---------- CLI → IDE ----------
    /// CLI asks the IDE to render a diff between old and new content.
    ShowDiff {
        /// Path being edited.
        path: PathBuf,
        /// Pre-edit file content.
        old_content: String,
        /// Post-edit file content.
        new_content: String,
    },
    /// CLI asks the IDE to show a permission prompt for a tool call.
    ShowPermissionPrompt {
        /// Tool call awaiting a decision.
        tool_use_id: ToolUseId,
        /// Short human-readable action description.
        action: String,
        /// Risk level string (e.g. `low`, `medium`, `high`).
        risk: String,
    },
    /// CLI streams an assistant message to the IDE chat panel.
    AssistantMessage {
        /// Message body (may contain markdown).
        text: String,
        /// Role label, typically `assistant`.
        role: String,
    },
    /// CLI notifies the IDE that a tool is starting / progressing / done.
    ToolExecution {
        /// Tool call this status update refers to.
        tool_use_id: ToolUseId,
        /// Tool name (e.g. `Read`, `Bash`).
        tool_name: String,
        /// Status string (e.g. `started`, `completed`, `failed`).
        status: String,
    },

    /// Bidirectional liveness ping.
    Heartbeat {
        /// Wall-clock time when the heartbeat was emitted.
        timestamp: std::time::SystemTime,
    },
}
