//! `AgentProgressLine` (claude-code `AgentProgressLine.tsx`): a tree-prefixed
//! agent line with tool/token counts and a status line. Backgrounded agents
//! show only the tree line (counts + status hidden).

use crate::components::coordinator::format_num::format_token_count;

/// Tree branch for a mid-list agent (`├─`).
pub const BRANCH_MID: &str = "\u{251C}\u{2500}";
/// Tree branch for the last agent (`└─`).
pub const BRANCH_LAST: &str = "\u{2514}\u{2500}";
/// Status gutter (`⎿  ` = U+23BF + 2 spaces).
pub const STATUS_GUTTER: &str = "\u{23BF}  ";

/// Progress state → status text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProgressState {
    /// Spawned, not yet producing output → `Initializing…`.
    Initializing,
    /// Running with a current activity label.
    Running(String),
    /// Finished (sync) → `Done`.
    Done,
    /// Async + resolved → tree line only (counts + status hidden).
    Backgrounded,
}

/// Render the multi-line progress block for one agent.
/// `is_last` selects the tree branch + continuation indent.
#[must_use]
pub fn render_agent_progress_line(
    name: &str,
    is_last: bool,
    tool_use_count: u32,
    token_count: u64,
    state: &AgentProgressState,
) -> String {
    let branch = if is_last { BRANCH_LAST } else { BRANCH_MID };
    let mut out = format!("{branch} {name}");
    if matches!(state, AgentProgressState::Backgrounded) {
        return out;
    }
    let tool = if tool_use_count == 1 {
        "tool use"
    } else {
        "tool uses"
    };
    out.push_str(&format!(
        " \u{00B7} {tool_use_count} {tool} \u{00B7} {} tokens",
        format_token_count(token_count)
    ));
    let status = match state {
        AgentProgressState::Initializing => "Initializing\u{2026}",
        AgentProgressState::Running(activity) => activity.as_str(),
        AgentProgressState::Done => "Done",
        AgentProgressState::Backgrounded => unreachable!(),
    };
    let cont = if is_last { "   " } else { "\u{2502}  " };
    out.push_str(&format!("\n{cont}{STATUS_GUTTER}{status}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_glyph_bytes() {
        assert_eq!(BRANCH_MID.as_bytes(), &[0xE2, 0x94, 0x9C, 0xE2, 0x94, 0x80]); // ├─
        assert_eq!(
            BRANCH_LAST.as_bytes(),
            &[0xE2, 0x94, 0x94, 0xE2, 0x94, 0x80]
        ); // └─
    }

    #[test]
    fn mid_running() {
        let out = render_agent_progress_line(
            "explorer",
            false,
            2,
            5100,
            &AgentProgressState::Running("Reading files".into()),
        );
        assert_eq!(out, "\u{251C}\u{2500} explorer \u{00B7} 2 tool uses \u{00B7} 5.1k tokens\n\u{2502}  \u{23BF}  Reading files");
    }

    #[test]
    fn last_initializing_singular_tool() {
        let out =
            render_agent_progress_line("writer", true, 1, 512, &AgentProgressState::Initializing);
        assert_eq!(out, "\u{2514}\u{2500} writer \u{00B7} 1 tool use \u{00B7} 512 tokens\n   \u{23BF}  Initializing\u{2026}");
    }

    #[test]
    fn done() {
        let out = render_agent_progress_line("writer", true, 3, 2300, &AgentProgressState::Done);
        assert_eq!(
            out,
            "\u{2514}\u{2500} writer \u{00B7} 3 tool uses \u{00B7} 2.3k tokens\n   \u{23BF}  Done"
        );
    }

    #[test]
    fn backgrounded_tree_only() {
        let out =
            render_agent_progress_line("bg", true, 9, 9999, &AgentProgressState::Backgrounded);
        assert_eq!(out, "\u{2514}\u{2500} bg");
    }
}
