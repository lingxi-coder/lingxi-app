//! `/cost` data API: `CostTracker::summary() -> CostSummary { session, day, month, by_model }`.
//!
//! M5 will route this through the `/cost` slash command; M3-05 only provides
//! the data shape and the read-side aggregation. See spec §8 M3-05 phase 5
//! (line 899).

#![forbid(unsafe_code)]

// Task 5 fills in the body. Empty stub for the module-skeleton test.

/// Read-side aggregate of one tracker's cost state. Returned by
/// [`CostTracker::summary`](crate::CostTracker::summary).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CostSummary {
    /// Session-scope totals (placeholder; Task 5 expands).
    pub session_total_nano_usd: u64,
}

/// Session-scope cost summary. Task 5 fills in the body.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionCostSummary {
    /// Placeholder for total nano-USD spent in this session.
    pub total_nano_usd: u64,
}

/// Period-scope (day/month) cost summary. Task 5 fills in the body.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PeriodCostSummary {
    /// Placeholder for the period label (e.g. `"2026-05-23"` or `"2026-05"`).
    pub label: String,
    /// Placeholder for total nano-USD spent in this period.
    pub total_nano_usd: u64,
}

/// Per-model cost breakdown summary. Task 5 fills in the body.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelCostSummary {
    /// Placeholder for total nano-USD spent on this model.
    pub total_nano_usd: u64,
}
