//! `BypassPermissionsMode` dialog placeholder — full implementation lands in Task 5.
#![forbid(unsafe_code)]

/// Mutable state for the BypassPermissionsMode dialog (Task 5).
#[derive(Debug, Clone, Default)]
pub struct BypassPermissionsState {
    /// Letters typed so far (lowercased internally).
    pub typed: String,
}
