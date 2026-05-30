//! Denial tracking state — used by `policy` to gate `Auto` mode fallback.
//!
//! M1 ships types only; the policy reads but never mutates these in M1.3.

use crate::result::PermissionDecisionReason;
use std::collections::HashMap;
use std::time::SystemTime;

/// Tunables. Public for tests.
pub mod limits {
    /// Per-tool consecutive-denial threshold before policy escalates.
    pub const PER_TOOL_FALLBACK: u32 = 5;
    /// Cross-tool consecutive-denial threshold.
    pub const GLOBAL_FALLBACK: u32 = 10;
    /// How long a denial record stays "consecutive" before it ages out.
    pub const RECORD_TTL: std::time::Duration = std::time::Duration::from_secs(300);
}

/// Aggregated denial state across all tools.
#[derive(Debug, Clone, Default)]
pub struct DenialTrackingState {
    /// Per-tool denial records keyed by tool name.
    pub per_tool_denials: HashMap<String, DenialRecord>,
    /// Cross-tool consecutive denial counter (reset on first non-deny).
    pub total_consecutive: u32,
    /// Wall-clock timestamp of the last non-deny outcome.
    pub last_success_at: Option<SystemTime>,
}

/// One tool's denial record.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    /// Consecutive deny count since last allow/ask-allowed.
    pub consecutive_count: u32,
    /// Wall-clock timestamp of the most recent denial.
    pub last_denial_at: SystemTime,
    /// The reason carried by the most recent denial — for diagnostics.
    pub last_reason: PermissionDecisionReason,
}
