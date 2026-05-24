//! `tengu_cost_*` event schemas — 10 events. M3-05 emits 3; M3-06 locks all 10.
//!
//! Spec §7 line 730-745. The three M3-05-emitted events
//! (`tengu_cost_recorded`, `tengu_cost_budget_warning`,
//! `tengu_cost_budget_exceeded`) byte-match the wire shape locked by
//! `lingxi_cost::events::emit_cost_recorded` (§4 Flow B). The remaining 7
//! schemas are M4-staged but locked here so M4 can emit without bumping
//! the schema tree.

use crate::pii::Verified;
use serde::{Deserialize, Serialize};

/// `tengu_cost_recorded` — one entry per successful API call. M3-05 emitter.
pub const RECORDED: &str = "tengu_cost_recorded";
/// `tengu_cost_budget_warning` — 80% threshold crossed. M3-05 emitter.
pub const BUDGET_WARNING: &str = "tengu_cost_budget_warning";
/// `tengu_cost_budget_exceeded` — 100% threshold crossed. M3-05 emitter.
pub const BUDGET_EXCEEDED: &str = "tengu_cost_budget_exceeded";
/// `tengu_cost_summary_requested` — `/cost` slash-command invocation.
pub const SUMMARY_REQUESTED: &str = "tengu_cost_summary_requested";
/// `tengu_cost_summary_generated` — `/cost` summary payload produced.
pub const SUMMARY_GENERATED: &str = "tengu_cost_summary_generated";
/// `tengu_cost_unknown_model` — a model lacked a pricing-table entry.
pub const UNKNOWN_MODEL: &str = "tengu_cost_unknown_model";
/// `tengu_cost_batch_discount_applied` — M4-only: `/v1/messages/batches` discount.
pub const BATCH_DISCOUNT_APPLIED: &str = "tengu_cost_batch_discount_applied";
/// `tengu_cost_pricing_table_refreshed` — pricing table reloaded.
pub const PRICING_TABLE_REFRESHED: &str = "tengu_cost_pricing_table_refreshed";
/// `tengu_cost_session_total_updated` — per-session aggregate moved.
pub const SESSION_TOTAL_UPDATED: &str = "tengu_cost_session_total_updated";
/// `tengu_cost_persistence_failed` — cost-state flush to disk failed.
pub const PERSISTENCE_FAILED: &str = "tengu_cost_persistence_failed";

pub(crate) const NAMES: &[&str] = &[
    RECORDED,
    BUDGET_WARNING,
    BUDGET_EXCEEDED,
    SUMMARY_REQUESTED,
    SUMMARY_GENERATED,
    UNKNOWN_MODEL,
    BATCH_DISCOUNT_APPLIED,
    PRICING_TABLE_REFRESHED,
    SESSION_TOTAL_UPDATED,
    PERSISTENCE_FAILED,
];

/// `tengu_cost_recorded` payload — byte-locked against M3-05.
///
/// The `cost_usd` field name is preserved verbatim for M3-05 parity even
/// though the value is stored in nano-USD (`u64`, saturating arithmetic).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedPayload {
    /// Whitelisted model identifier (e.g. `claude-sonnet-4-5`).
    pub model: Verified,
    /// Anthropic `usage.input_tokens`.
    pub input_tokens: u64,
    /// Anthropic `usage.output_tokens`.
    pub output_tokens: u64,
    /// Prompt-caching: tokens read from cache.
    pub cache_read_input_tokens: u64,
    /// Prompt-caching: tokens written to a new cache block.
    pub cache_creation_input_tokens: u64,
    /// Value is nano-USD; name kept as `cost_usd` for downstream `BigQuery` parity.
    pub cost_usd: u64,
    /// Session identifier (`SessionId::to_string()` — non-PII).
    pub session_id: Verified,
    /// M4 batches endpoint flag; ALWAYS `false` in M3 emitters.
    pub is_batch_request: bool,
}

/// Payload for [`BUDGET_WARNING`] — fired once when usage crosses 80%.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetWarningPayload {
    /// Nano-USD limit.
    pub limit_usd: u64,
    /// Nano-USD current.
    pub current_usd: u64,
    /// Basis-points percentage (8000 = 80%).
    pub percent_bps: u64,
}

/// Payload for [`BUDGET_EXCEEDED`] — fired once when usage crosses 100%.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetExceededPayload {
    /// Nano-USD limit.
    pub limit_usd: u64,
    /// Nano-USD current.
    pub current_usd: u64,
}

/// Payload for [`SUMMARY_REQUESTED`] — `/cost` invocation entered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryRequestedPayload {
    /// Session identifier the summary was requested for.
    pub session_id: Verified,
}

/// Payload for [`SUMMARY_GENERATED`] — `/cost` payload built and rendered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryGeneratedPayload {
    /// Session identifier the summary was generated for.
    pub session_id: Verified,
    /// Nano-USD total for this session.
    pub session_total_usd: u64,
    /// Nano-USD total for the calendar day (rolling).
    pub day_total_usd: u64,
    /// Nano-USD total for the calendar month (rolling).
    pub month_total_usd: u64,
    /// Distinct models billed in this summary window.
    pub models_billed: u32,
}

/// Payload for [`UNKNOWN_MODEL`] — pricing-table miss, fell back to default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownModelPayload {
    /// Model identifier that lacked a pricing-table entry.
    pub model: Verified,
}

/// M4-only schema; M3 never emits. Locked here so M4 can plug in without bumping schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchDiscountAppliedPayload {
    /// Model identifier on the discounted batch.
    pub model: Verified,
    /// Nano-USD before the batch discount.
    pub original_cost_usd: u64,
    /// Nano-USD after the batch discount.
    pub discounted_cost_usd: u64,
    /// Basis-points discount applied (5000 = 50%).
    pub discount_bps: u64,
}

/// Payload for [`PRICING_TABLE_REFRESHED`] — pricing data reloaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingTableRefreshedPayload {
    /// Distinct model entries now resident in the table.
    pub entries: u32,
}

/// Payload for [`SESSION_TOTAL_UPDATED`] — per-session aggregate moved.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTotalUpdatedPayload {
    /// Session identifier whose total was updated.
    pub session_id: Verified,
    /// New nano-USD running total for this session.
    pub total_usd: u64,
}

/// Payload for [`PERSISTENCE_FAILED`] — cost-state flush to disk failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceFailedPayload {
    /// Session identifier whose state failed to persist.
    pub session_id: Verified,
    /// Whitelisted error kind (no raw IO details).
    pub error: Verified,
}
