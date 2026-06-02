//! Coordinator-only tool assembly.
//!
//! Builds the 4 coordinator-only tools (`TeamCreate` / `TeamDelete` /
//! `SendMessage` / `StructuredOutput`), each wired to the shared
//! [`TeamRegistry`]. In §15 (Plugin) and §22 (cli-demo) the host wires these
//! into `ToolRegistry` only when [`crate::CoordinatorMode::is_enabled`] is true.

use crate::team_registry::TeamRegistry;
use crate::tool_send_message::SendMessageTool;
use crate::tool_synthetic_output::SyntheticOutputTool;
use crate::tool_team_create::TeamCreateTool;
use crate::tool_team_delete::TeamDeleteTool;
use std::sync::Arc;
use tool_api::Tool;

/// Build the list of coordinator-only tools, each sharing `team`.
///
/// Returns the four coordinator-mode tools as `Arc<dyn Tool>` trait objects so
/// the host can register them directly. Each constructor clones the shared
/// [`TeamRegistry`] `Arc` into its handler state.
#[must_use]
pub fn coordinator_internal_tools(team: Arc<TeamRegistry>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TeamCreateTool::new(team.clone())) as Arc<dyn Tool>,
        Arc::new(TeamDeleteTool::new(team.clone())) as Arc<dyn Tool>,
        Arc::new(SendMessageTool::new(team.clone())) as Arc<dyn Tool>,
        Arc::new(SyntheticOutputTool::new(team)) as Arc<dyn Tool>,
    ]
}
