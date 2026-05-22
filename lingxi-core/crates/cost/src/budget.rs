//! Budget enforcement — filled in Task 5.
//!
//! Currently provides placeholder type stubs so `lib.rs` re-exports compile.

use serde::{Deserialize, Serialize};

/// Placeholder — replaced in Task 5.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BudgetConfig {}

/// Placeholder — replaced in Task 5.
#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    /// Within budget.
    Allowed,
}

/// Placeholder — replaced in Task 5.
pub struct BudgetEnforcer {}

/// Placeholder — replaced in Task 5.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BudgetExceedPolicy {
    /// Hard halt on budget exceeded.
    Halt,
}
