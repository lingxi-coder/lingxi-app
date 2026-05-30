//! Coordinator-only tool stubs.
//!
//! Returns the 4 coordinator-only tools (`TeamCreate` / `TeamDelete` /
//! `SendMessage` / `SyntheticOutput`). In §15 (Plugin) and §22 (cli-demo)
//! the host wires these into `ToolRegistry` only when
//! [`crate::CoordinatorMode::is_enabled`] is true.
//!
//! Plan 15 wires the real `call()` methods; M1 ships an empty Vec so the
//! registry can be assembled at startup.

use crate::team_registry::TeamRegistry;
use std::sync::Arc;
use tool_api::Tool;

/// Build the placeholder list of coordinator-only tools.
///
/// `_team` is taken by value to match the M2 signature where the real tool
/// constructors clone the `Arc` into their handler state.
#[must_use]
#[allow(clippy::needless_pass_by_value)]
pub fn coordinator_internal_tools(_team: Arc<TeamRegistry>) -> Vec<Arc<dyn Tool>> {
    // Full impls in production; M1.13 ships placeholder names so the registry
    // can advertise them. Plan 15 wires the real call() methods.
    Vec::new()
}
