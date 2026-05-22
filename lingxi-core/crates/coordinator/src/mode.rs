//! Coordinator mode toggle.
//!
//! When enabled, the host wires coordinator-only tools (`team_create`,
//! `team_delete`, `send_message`, `synthetic_output`) into the tool registry.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// In-memory coordinator-mode flag.
#[derive(Debug, Default)]
pub struct CoordinatorMode {
    enabled: AtomicBool,
    /// True if the session was started directly in coordinator mode (vs.
    /// upgraded later via a mode-switch).
    pub session_started_as_coordinator: bool,
}

impl CoordinatorMode {
    /// Construct a disabled mode.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether coordinator mode is currently active.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    /// Switch into coordinator mode.
    pub fn enter(&self) {
        self.enabled.store(true, Ordering::Release);
    }
    /// Leave coordinator mode.
    pub fn exit(&self) {
        self.enabled.store(false, Ordering::Release);
    }
}

/// Result of a mode-switch tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModeSwitchResult {
    /// Successfully entered coordinator mode.
    EnteredCoordinator,
    /// Successfully exited coordinator mode.
    ExitedCoordinator,
}
