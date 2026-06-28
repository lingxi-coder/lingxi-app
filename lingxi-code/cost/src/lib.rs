//! Cost subsystem: pricing catalog, token/non-token usage aggregation, a
//! saturating-arithmetic cost calculator, an in-process cost tracker, a
//! budget enforcer, and (M3-05) cost-event emission + budget alarms +
//! `/cost` data API.
//!
//! See design spec §7 (Cost), the M1 plan for the M1 scope, and the M3
//! engine-completion design (`docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md`)
//! for the M3-05 extensions.
#![forbid(unsafe_code)]

pub mod budget;
pub mod calculator;
pub mod events;
pub mod handle;
pub mod pricing;
pub mod summary;
pub mod token_usage_replay;
pub mod tracker;
pub mod usage;

pub use budget::{
    BudgetCheckResult, BudgetConfig, BudgetEnforcer, BudgetExceedPolicy,
    BUDGET_EXCEEDED_THRESHOLD_BPS, BUDGET_WARNING_THRESHOLD_BPS,
};
pub use calculator::CostCalculator;
pub use events::{emit_api_success, ApiSuccessFields, EVENT_NAME_API_SUCCESS};
pub use pricing::{
    nano_usd_to_dollars_format, CostError, ModelPricing, ModelRef, MoneyPerToken,
    NonTokenBillableUnit, PricingCatalog, PricingResolution, PricingSource, ProviderId, TokenClass,
    BATCH_DISCOUNT_BPS,
};
pub use summary::{CostSummary, ModelCostSummary, PeriodCostSummary, SessionCostSummary};
pub use token_usage_replay::{
    latest_token_usage_turn_id, latest_token_usage_turn_id_from_events, ReplayTurn,
    ReplayTurnStatus,
};
pub use tracker::{CostState, CostTracker, ModelUsage};
pub use usage::{ApiSpeed, ServerToolUsage, TokenUsage, Usage};
