//! `UserToolResultMessage` — placeholder (real impl lands in Task 5).
//!
//! Task 3 needs this module to exist so `components/messages/mod.rs`
//! compiles; Task 5 replaces this body with the full renderer + truncation
//! + Bash ANSI hook.

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;

/// Props for [`UserToolResultMessage`].
#[derive(Debug, Clone, Default, Props)]
pub struct UserToolResultProps {
    /// Correlator matching the paired `AssistantToolUseMessage.id`.
    pub id: ToolUseId,
    /// Tool name (used to gate Bash → ANSI parser).
    pub tool: String,
    /// JSON result payload.
    pub result: serde_json::Value,
    /// `true` → render full body (line/byte-capped) + truncation footer.
    pub expanded: bool,
    /// `true` → render the `> ` focus prefix.
    pub focused: bool,
}

/// String renderer placeholder. Task 5 replaces with the full M6-04 form
/// (`└ first_line (+N lines)` collapsed; line+byte-bounded body expanded).
#[must_use]
pub fn render_user_tool_result_to_string(_props: UserToolResultProps) -> String {
    String::new()
}
