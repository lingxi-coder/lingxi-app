//! Cost subsystem: pricing catalog, token/non-token usage aggregation, a
//! saturating-arithmetic cost calculator, an in-process cost tracker, and a
//! budget enforcer.
//!
//! See design spec §7 (Cost) and the M1 plan for the canonical scope.
//!
//! Task 4 implements [`pricing`], [`usage`], and [`calculator`]. Task 5 fills
//! in [`budget`] and [`tracker`] (currently stubs so this crate compiles).
#![forbid(unsafe_code)]

pub mod budget;
pub mod calculator;
pub mod pricing;
pub mod tracker;
pub mod usage;

pub use budget::{BudgetCheckResult, BudgetConfig, BudgetEnforcer, BudgetExceedPolicy};
pub use calculator::CostCalculator;
pub use pricing::{
    CostError, ModelPricing, ModelRef, MoneyPerToken, NonTokenBillableUnit, PricingCatalog,
    PricingResolution, PricingSource, ProviderId, TokenClass,
};
pub use tracker::{CostState, CostTracker, ModelUsage};
pub use usage::{ApiSpeed, ServerToolUsage, TokenUsage, Usage};
