//! `ExitPlanMode` dialog placeholder — full implementation lands in Task 4.
#![forbid(unsafe_code)]

use super::DialogFocus;

/// Mutable state for the ExitPlanMode dialog (Task 4).
#[derive(Debug, Clone, Default)]
pub struct ExitPlanModeState {
    /// Which button is currently highlighted.
    pub focus: DialogFocus,
}
