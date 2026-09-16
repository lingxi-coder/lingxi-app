//! Top-of-loop guards, and the per-entry order they run in (§3.5).
//!
//! The three entries do not agree, and the plan keeps all three rather than
//! picking one:
//!
//! ```text
//! Batched            drain → max_turns → budget → increment
//! BatchedCancelable  cancel → max_turns → budget → increment   (NO drain)
//! Streaming          drain → max_turns → budget → increment → cancel
//! ```
//!
//! What is shared here is the drain/limit/increment sequence. CANCEL is not:
//! its position differs (first on one path, last on another, absent on the
//! third) and so does its handling — the cancelable entry injects an interrupt
//! and returns, streaming emits `aborted_streaming` and breaks so its epilogue
//! still runs. Folding those together would erase §3.2 rows that
//! `tests/turn_epilogue_boundary_test.rs` and `tests/turn_cancel_mapping_test.rs`
//! pin. Each caller keeps its own cancel handling, at its own position.

use super::*;

/// Which entry's guard order to run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LoopGuardOrder {
    /// `run_turn`: drains pending input before the limits, so a queued message
    /// is not dropped by one.
    Batched,
    /// `run_turn_with_cancel`: no drain at all. A turn the user is cancelling
    /// must not pull a queued message in on its way out.
    BatchedCancelable,
    /// `run_turn_streaming`: drains like `Batched`; its cancel guard runs after
    /// the increment, which the caller does itself.
    Streaming,
}

impl LoopGuardOrder {
    /// Whether this entry drains pending input at the top of its loop.
    const fn drains(self) -> bool {
        match self {
            Self::Batched | Self::Streaming => true,
            Self::BatchedCancelable => false,
        }
    }
}

/// What the shared guards decided.
///
/// Deliberately not an outcome: the three entries map these to different public
/// types — batched returns `Ok(TurnOutcome::MaxTurns)`, streaming returns
/// `Err(OrchestratorError::MaxTurnsReached)` — and §3.2 keeps that difference.
pub(crate) enum GuardVerdict {
    /// Run the model step.
    Proceed,
    /// `max_turns` reached before this step.
    MaxTurns,
    /// The configured budget is spent.
    OverBudget,
}

impl ConversationOrchestrator {
    /// Run the drain/limit/increment guards in `order`, returning what they
    /// decided.
    ///
    /// The turn counter is incremented ONLY on `Proceed`, matching all three
    /// entries today: a turn stopped by a limit does not count the step it
    /// never ran.
    pub(crate) async fn run_turn_loop_guards(
        &self,
        order: LoopGuardOrder,
        state: &mut TurnLoopState,
    ) -> GuardVerdict {
        if order.drains() {
            // Before the limits on purpose, so a message that arrived during the
            // previous step is not dropped by `max_turns` or the budget.
            self.drain_mid_turn_input().await;
        }
        if self.config.max_turns != 0 && state.turn_count >= self.config.max_turns {
            return GuardVerdict::MaxTurns;
        }
        if self.over_budget().await {
            return GuardVerdict::OverBudget;
        }
        state.turn_count = state.turn_count.saturating_add(1);
        GuardVerdict::Proceed
    }
}
