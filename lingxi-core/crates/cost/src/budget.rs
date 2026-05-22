//! Budget enforcement — pre-call gate + post-call latch.
//!
//! [`BudgetEnforcer`] is consulted before every API call (with an estimated
//! cost) and again after the call returns (with the realized cost). The
//! pre-call gate may emit warnings, ask the host to confirm, or block;
//! the post-call latch sets a one-way "realized exceeded" flag that causes
//! every subsequent pre-call gate to halt regardless of the per-call estimate.

use crate::tracker::CostTracker;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration controlling [`BudgetEnforcer`] behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Hard limit on total session cost, in nano-USD. `None` disables.
    pub max_session_nano_usd: Option<u64>,
    /// Hard limit on a single turn's cost, in nano-USD. `None` disables.
    pub max_turn_nano_usd: Option<u64>,
    /// Hard limit on a single turn's total tokens. `None` disables.
    pub max_turn_tokens: Option<u64>,
    /// Fractional thresholds (e.g. `[0.5, 0.8, 0.95]`) at which a one-shot
    /// warning event is emitted as the session approaches its budget.
    pub warning_thresholds: Vec<f64>,
    /// What to do when the budget is exceeded (or projected to be).
    pub on_exceed: BudgetExceedPolicy,
}

/// Policy for handling a budget exceedance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetExceedPolicy {
    /// Stop immediately; no further API calls.
    Halt,
    /// Ask the host (and via the host, the user) whether to continue.
    AskUser,
    /// Emit a warning but allow the call to proceed.
    WarnOnly,
}

/// Cost tracker + atomic latch for realized-exceeded budget enforcement.
pub struct BudgetEnforcer {
    config: BudgetConfig,
    cost_tracker: Arc<CostTracker>,
    warnings_fired: RwLock<HashSet<u32>>,
    realized_exceeded: AtomicBool,
}

/// Result of a [`BudgetEnforcer::check_pre_api_call`].
#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    /// Call is within budget; proceed without surfacing anything.
    Ok,
    /// Crossed a fractional warning threshold (e.g. 50%) for the first time.
    ThresholdWarning {
        /// Threshold percent that fired (e.g. `50`, `80`, `95`).
        pct: u32,
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget but policy is [`BudgetExceedPolicy::WarnOnly`].
    Warn {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget; policy is [`BudgetExceedPolicy::AskUser`] — host must
    /// prompt the user before continuing.
    AskUser {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
    /// Over-budget; policy is [`BudgetExceedPolicy::Halt`] — stop now.
    Halt {
        /// Current cumulative cost in nano-USD.
        current: u64,
        /// Configured maximum session cost in nano-USD.
        limit: u64,
    },
}

impl BudgetEnforcer {
    /// Construct a new enforcer bound to `cost_tracker`.
    #[must_use]
    pub fn new(config: BudgetConfig, cost_tracker: Arc<CostTracker>) -> Self {
        Self {
            config,
            cost_tracker,
            warnings_fired: RwLock::new(HashSet::new()),
            realized_exceeded: AtomicBool::new(false),
        }
    }

    /// Pre-API call gate. After the call returns, call
    /// [`Self::check_post_api_call`] to latch the realized-exceeded flag if
    /// the actual cost overran.
    pub async fn check_pre_api_call(&self, estimated_cost_nano_usd: u64) -> BudgetCheckResult {
        if self.realized_exceeded.load(Ordering::Acquire) {
            let current = self.cost_tracker.total_nano_usd().await;
            let limit = self.config.max_session_nano_usd.unwrap_or(0);
            return BudgetCheckResult::Halt { current, limit };
        }
        let current = self.cost_tracker.total_nano_usd().await;
        let after = current.saturating_add(estimated_cost_nano_usd);
        if let Some(max) = self.config.max_session_nano_usd {
            if after > max {
                return match self.config.on_exceed {
                    BudgetExceedPolicy::Halt => BudgetCheckResult::Halt {
                        current,
                        limit: max,
                    },
                    BudgetExceedPolicy::AskUser => BudgetCheckResult::AskUser {
                        current,
                        limit: max,
                    },
                    BudgetExceedPolicy::WarnOnly => BudgetCheckResult::Warn {
                        current,
                        limit: max,
                    },
                };
            }
            // Threshold warning — cast to f64 only for the ratio comparison.
            #[allow(clippy::cast_precision_loss)]
            let ratio = after as f64 / max as f64;
            for &threshold in &self.config.warning_thresholds {
                if ratio >= threshold {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let pct = (threshold * 100.0) as u32;
                    let mut fired = self.warnings_fired.write().await;
                    if fired.insert(pct) {
                        return BudgetCheckResult::ThresholdWarning {
                            pct,
                            current,
                            limit: max,
                        };
                    }
                }
            }
        }
        BudgetCheckResult::Ok
    }

    /// Latch the realized-exceeded flag if cumulative cost now exceeds the
    /// configured session limit. Subsequent [`Self::check_pre_api_call`]
    /// returns [`BudgetCheckResult::Halt`].
    pub async fn check_post_api_call(&self, _realized_cost: u64) {
        let total = self.cost_tracker.total_nano_usd().await;
        if let Some(max) = self.config.max_session_nano_usd {
            if total > max {
                self.realized_exceeded.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::PricingCatalog;
    use lingxi_protocol::SessionId;
    use tokio::sync::mpsc;

    fn make_tracker() -> Arc<CostTracker> {
        let (tx, _rx) = mpsc::channel(8);
        Arc::new(CostTracker::new(
            SessionId::nil(),
            Arc::new(PricingCatalog::builtin_reference()),
            tx,
        ))
    }

    #[tokio::test]
    async fn under_budget_returns_ok() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000_000_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![0.5, 0.8, 0.95],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(
            e.check_pre_api_call(10_000_000).await,
            BudgetCheckResult::Ok
        ));
    }

    #[tokio::test]
    async fn over_budget_halts_with_halt_policy() {
        let cfg = BudgetConfig {
            max_session_nano_usd: Some(1_000),
            max_turn_nano_usd: None,
            max_turn_tokens: None,
            warning_thresholds: vec![],
            on_exceed: BudgetExceedPolicy::Halt,
        };
        let e = BudgetEnforcer::new(cfg, make_tracker());
        assert!(matches!(
            e.check_pre_api_call(10_000).await,
            BudgetCheckResult::Halt { .. }
        ));
    }
}
