//! Compaction orchestrator — runs each layer in order, escalating only when
//! cheaper layers leave us over the autocompact threshold.
//!
//! Order mirrors the TS query pipeline (`query.ts:400-467`): **snip →
//! microcompact → autocompact**. The `contextCollapse` layer that TS runs
//! between microcompact and autocompact is feature-gated and absent from the
//! reference checkout, so it is **intentionally omitted here** (documented as a
//! known gap, not a divergence).
//!
//! Autocompact is gated through [`crate::threshold_calc::should_auto_compact`]
//! and guarded by the circuit breaker from `autoCompactIfNeeded`
//! (`autoCompact.ts:241-351`): after
//! [`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`](crate::thresholds::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES)
//! consecutive failures the layer short-circuits without calling the
//! summarizer.

use crate::autocompact::{Autocompactor, CompactionError};
use crate::microcompact::{Microcompactor, TimeBasedMCConfig};
use crate::snip::SnipCompactor;
use crate::thresholds::{
    AutoCompactTrackingState, CompactionLayer, MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES,
};
use protocol::ConversationMessage;
use std::time::SystemTime;

/// Result of one orchestrator pass.
#[derive(Debug, Clone)]
pub struct IterationCompactionResult {
    /// Compacted message list.
    pub messages: Vec<ConversationMessage>,
    /// Layers that actually fired this iteration, in order.
    pub layers_applied: Vec<CompactionLayer>,
    /// Approximate tokens freed across all layers.
    pub total_tokens_freed: u64,
    /// Consecutive autocompact-failure count after this pass. Mirrors the
    /// `consecutiveFailures` value `autoCompactIfNeeded` threads back to the
    /// caller (`autoCompact.ts:328-349`): reset to `0` on a successful
    /// autocompact, incremented on a failed one, and carried through unchanged
    /// when autocompact did not run. The caller persists this into its
    /// `AutoCompactTrackingState` so the next iteration's circuit breaker sees
    /// it.
    pub consecutive_failures: u32,
    /// Whether the autocompact layer actually ran and succeeded this pass.
    /// Mirrors `wasCompacted` from `autoCompactIfNeeded`. `false` when
    /// autocompact was skipped (under threshold or circuit-breaker tripped) or
    /// failed; snip/micro firing alone does **not** set this.
    pub was_compacted: bool,
}

/// Owns one instance of each layer + the autocompact threshold.
pub struct CompactionOrchestrator {
    /// Snip layer.
    pub snip: SnipCompactor,
    /// Microcompact layer.
    pub micro: Microcompactor,
    /// Autocompact layer.
    pub auto: Autocompactor,
    /// Token threshold above which autocompact fires.
    pub autocompact_threshold: u64,
}

impl CompactionOrchestrator {
    /// Build a fresh orchestrator with default per-layer config.
    #[must_use]
    pub fn new(autocompact_threshold: u64) -> Self {
        Self {
            snip: SnipCompactor,
            micro: Microcompactor {
                config: TimeBasedMCConfig::default(),
            },
            auto: Autocompactor::new(),
            autocompact_threshold,
        }
    }

    /// Build an orchestrator with a caller-provided [`Autocompactor`], keeping
    /// the default snip + microcompact layers.
    ///
    /// The composition root (In-Loop Compaction Batch 6) passes an autocompactor
    /// wired via [`Autocompactor::with_forked_runner`] so the autocompact layer
    /// issues a real forked summary call (sharing the parent's prompt cache)
    /// instead of the deterministic fallback that [`Self::new`]'s
    /// [`Autocompactor::new`] produces.
    #[must_use]
    pub fn with_autocompactor(auto: Autocompactor, autocompact_threshold: u64) -> Self {
        Self {
            snip: SnipCompactor,
            micro: Microcompactor {
                config: TimeBasedMCConfig::default(),
            },
            auto,
            autocompact_threshold,
        }
    }

    /// Run one full orchestrator pass with a **fresh** tracking state.
    ///
    /// Thin wrapper over [`Self::process_iteration_tracked`] that starts from a
    /// default [`AutoCompactTrackingState`] (zero consecutive failures), so the
    /// circuit breaker never short-circuits on this path. Used by the manual
    /// `/compact` entry point (`force_compact_with_cancel`), whose behavior is
    /// unchanged by Batch 3: snip runs first but targets the autocompact
    /// threshold, so a history that is over the threshold still escalates to
    /// autocompact exactly as before.
    ///
    /// `snip_tokens_freed_already` lets the caller report snip work done outside
    /// this entry point; it is folded into the freed total and the
    /// should-auto-compact subtraction.
    ///
    /// # Errors
    ///
    /// Propagates [`CompactionError`] from the autocompact layer when it fires
    /// and fails.
    pub async fn process_iteration(
        &self,
        messages: Vec<ConversationMessage>,
        snip_tokens_freed_already: u64,
    ) -> Result<IterationCompactionResult, CompactionError> {
        let mut tracking = AutoCompactTrackingState::default();
        self.process_iteration_tracked(messages, snip_tokens_freed_already, &mut tracking)
            .await
    }

    /// Run one full orchestrator pass, threading `tracking` for the autocompact
    /// circuit breaker.
    ///
    /// Order (TS `query.ts:400-467`):
    /// 1. **Snip** — drop oldest messages targeting the autocompact threshold
    ///    (cheapest, no LLM). Records [`CompactionLayer::Snip`] when it removed
    ///    at least one message.
    /// 2. **Microcompact** — clear large tool results. Records
    ///    [`CompactionLayer::Microcompact`] when it cleared anything.
    /// 3. **Autocompact** — only when still over threshold per
    ///    [`should_auto_compact`](crate::threshold_calc::should_auto_compact)
    ///    **and** the circuit breaker has not tripped
    ///    (`tracking.consecutive_failures < MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`).
    ///    On success resets `tracking.consecutive_failures` to `0` and sets
    ///    `was_compacted = true`; on error increments it and propagates the
    ///    error.
    ///
    /// `snip_tokens_freed_already` accounts for snip work the caller already did
    /// before this entry point (TS `snipTokensFreed`); it is added to this
    /// pass's snip savings for the threshold subtraction.
    ///
    /// # Errors
    ///
    /// Propagates [`CompactionError`] from the autocompact layer when it fires
    /// and fails. On error `tracking.consecutive_failures` has already been
    /// incremented.
    pub async fn process_iteration_tracked(
        &self,
        mut messages: Vec<ConversationMessage>,
        snip_tokens_freed_already: u64,
        tracking: &mut AutoCompactTrackingState,
    ) -> Result<IterationCompactionResult, CompactionError> {
        let mut layers = Vec::new();
        let mut freed = snip_tokens_freed_already;
        if snip_tokens_freed_already > 0 {
            layers.push(CompactionLayer::Snip);
        }

        // --- Layer 1: snip (cheapest, no LLM) ----------------------------- //
        // COMPACT.4: TS gates the snip pass behind `feature('HISTORY_SNIP')`
        // (`query.ts:401`), which resolves to `envBool('CLAUDE_CODE_HISTORY_SNIP',
        // false)` (`shims/bun-bundle.ts:20`) — OFF by default in the reference
        // checkout. The prior code ran the snip pass UNCONDITIONALLY, shedding
        // the oldest messages on every iteration even with the feature off; gate
        // it so the default matches TS.
        //
        // Target the autocompact threshold as the snip budget: snip is the
        // cheap first escalation, so we let it shed the oldest messages toward
        // the same ceiling autocompact defends. The protected-tail floor in
        // `SnipCompactor::snip` means snip cannot drive a genuinely-oversized
        // history below the threshold on its own, so autocompact still
        // escalates when warranted (this preserves the manual `/compact`
        // outcome — see `process_iteration`).
        if history_snip_enabled() {
            let current_tokens = crate::grouping::estimate_tokens_for_range(&messages);
            let snip = SnipCompactor::snip(messages, current_tokens, self.autocompact_threshold);
            if snip.removed_count > 0 {
                layers.push(CompactionLayer::Snip);
                freed = freed.saturating_add(snip.tokens_freed);
            }
            messages = snip.messages;
        }

        // --- Layer 2: microcompact (time-gap gated) ----------------------- //
        // CSM.3: TS `maybeTimeBasedMicrocompact` only clears old tool results
        // when the idle-gap trigger fires; with `config.enabled == false` (the
        // default) it is a no-op. The prior code ran `micro.compact`
        // UNCONDITIONALLY, over-clearing tool results on every iteration even
        // when time-based micro is disabled. PARITY-GAP: `ConversationMessage`
        // carries no per-message timestamp, so when ENABLED we run on the
        // `keep_recent` count alone (the SPECS-noted fallback) rather than the
        // exact since-last-assistant idle gap.
        if self.micro.config.enabled {
            let micro = self.micro.compact(messages, SystemTime::now());
            if micro.cleared_count > 0 {
                layers.push(CompactionLayer::Microcompact);
                freed = freed.saturating_add(micro.tokens_saved);
                // COMPACT.3: TS `maybeTimeBasedMicrocompact` suppresses the
                // "context left until autocompact" warning once it has actually
                // cleared tool results (`microCompact.ts:511`, reached only when
                // `tokensSaved > 0` — it returns null at :494-496 otherwise).
                // The token counts are stale until the next API response, so the
                // warning would be misleading. The prior code never suppressed.
                crate::warning_state::suppress_compact_warning();
            }
            messages = micro.messages;
        }

        // --- (collapse layer intentionally omitted — known gap) ----------- //

        // --- Layer 3: autocompact (threshold + circuit-breaker gated) ----- //
        let mut was_compacted = false;
        let estimate_after_micro = crate::grouping::estimate_tokens_for_range(&messages);
        let over_threshold = crate::threshold_calc::should_auto_compact(
            estimate_after_micro,
            // Snip removed messages but the surviving usage estimate above
            // already reflects the post-snip set, so no extra subtraction is
            // applied here (snip_freed = 0). The freed total still carries the
            // savings for the caller's accounting.
            0,
            self.autocompact_threshold,
        );
        // Circuit breaker: after N consecutive failures, stop trying so a
        // hopelessly-over-limit session does not hammer the summarizer every
        // turn (`autoCompact.ts:260-265`).
        let breaker_tripped =
            tracking.consecutive_failures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES;

        if over_threshold && !breaker_tripped {
            match self.auto.compact(messages.clone()).await {
                Ok(result) => {
                    messages.clone_from(&result.summary_messages);
                    freed = freed.saturating_add(
                        result
                            .pre_compact_token_count
                            .saturating_sub(result.post_compact_token_count),
                    );
                    layers.push(CompactionLayer::Autocompact);
                    was_compacted = true;
                    // Reset the failure count on success.
                    tracking.consecutive_failures = 0;
                }
                Err(e) => {
                    // Increment for the circuit breaker, then propagate.
                    tracking.consecutive_failures =
                        tracking.consecutive_failures.saturating_add(1);
                    return Err(e);
                }
            }
        }

        Ok(IterationCompactionResult {
            messages,
            layers_applied: layers,
            total_tokens_freed: freed,
            consecutive_failures: tracking.consecutive_failures,
            was_compacted,
        })
    }
}

/// Whether the `HISTORY_SNIP` snip pass runs (COMPACT.4).
///
/// Mirrors TS `feature('HISTORY_SNIP')`, which resolves to
/// `envBool('CLAUDE_CODE_HISTORY_SNIP', false)` (`shims/bun-bundle.ts:20,33-37`):
/// the env var must be exactly `"1"` or `"true"` (byte-exact — `envBool` does NOT
/// trim or case-fold, unlike `isEnvTruthy`). Absent or any other value → `false`,
/// matching the reference checkout's default-off so the snip layer is a no-op
/// unless explicitly enabled.
fn history_snip_enabled() -> bool {
    match std::env::var("CLAUDE_CODE_HISTORY_SNIP") {
        Ok(v) => v == "1" || v == "true",
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autocompact::CompactionResult;
    use protocol::MessageId;

    fn long_user(i: usize) -> ConversationMessage {
        // ~80 chars → ~20 tokens each, so a handful clears any small threshold.
        ConversationMessage::user(
            MessageId::new(),
            format!("turn-{i} padding text to push the token estimate over a small threshold value"),
        )
    }

    /// An `Autocompactor` whose `compact` always returns `Err`, to exercise the
    /// circuit breaker without a network call. Built by swapping the orchestrator
    /// field after construction.
    fn failing_autocompactor() -> Autocompactor {
        // The default (unwired) autocompactor SUCCEEDS via the deterministic
        // fallback, so to force failures we wire a runner with an empty slot:
        // `compact` then returns `CompactionError::Internal("no cache-safe
        // params")` on every call.
        use sidequery::{CacheSafeParamsSlot, ForkedAgentRunner, SubagentSlotProvider};
        use std::sync::Arc;
        struct NoopProvider;
        impl SubagentSlotProvider for NoopProvider {}
        // No `with_side_query_client` → run() would fail too, but the empty slot
        // is consulted first and short-circuits with an error before any call.
        let runner = Arc::new(ForkedAgentRunner::new(Arc::new(NoopProvider)));
        let slot = Arc::new(CacheSafeParamsSlot::new()); // never saved → empty
        Autocompactor::with_forked_runner(runner, slot)
    }

    /// A panicking autocompactor: any call to `compact` would panic. Used to
    /// prove the snip-only-under-threshold path never reaches autocompact.
    ///
    /// We cannot make `Autocompactor::compact` itself panic (it is a concrete
    /// type), so instead we assert via the threshold gate: with a threshold
    /// above the post-snip estimate, `should_auto_compact` is false and the
    /// `auto.compact` arm is never entered. The default autocompactor here would
    /// otherwise succeed; the test asserts it was NOT invoked by checking that
    /// `was_compacted` is false and `Autocompact` is absent from the layers.
    fn order_orchestrator(threshold: u64) -> CompactionOrchestrator {
        CompactionOrchestrator::new(threshold)
    }

    #[tokio::test]
    async fn circuit_breaker_short_circuits_after_three_failures() {
        // Threshold 0 so `should_auto_compact` is always true: every pass tries
        // autocompact. The failing autocompactor errors each time, so the first
        // three passes return Err and bump consecutive_failures to 3; the fourth
        // short-circuits (was_compacted=false) WITHOUT calling the summarizer.
        let mut orch = order_orchestrator(0);
        orch.auto = failing_autocompactor();

        let mut tracking = AutoCompactTrackingState::default();
        let msgs = vec![long_user(0), long_user(1), long_user(2)];

        // 3 consecutive Err from the failing mock summarizer.
        for expected in 1..=3u32 {
            let r = orch
                .process_iteration_tracked(msgs.clone(), 0, &mut tracking)
                .await;
            assert!(r.is_err(), "attempt {expected} should fail");
            assert_eq!(
                tracking.consecutive_failures, expected,
                "failure count should increment to {expected}"
            );
        }

        // 4th call: breaker tripped → short-circuit, no summarizer call, Ok with
        // was_compacted=false.
        let res = orch
            .process_iteration_tracked(msgs.clone(), 0, &mut tracking)
            .await
            .expect("breaker short-circuits to Ok, not Err");
        assert!(
            !res.was_compacted,
            "4th call must NOT compact (circuit breaker)"
        );
        assert!(
            !res.layers_applied.contains(&CompactionLayer::Autocompact),
            "autocompact layer must not fire when breaker is tripped"
        );
        assert_eq!(res.consecutive_failures, 3);
    }

    #[tokio::test]
    async fn snip_alone_under_threshold_never_calls_summarizer() {
        // A huge threshold means even the full (un-snipped) history is under it,
        // so `should_auto_compact` is false and the autocompact arm is never
        // entered. We assert no Autocompact layer and was_compacted=false. If the
        // gate were broken and autocompact ran, the default autocompactor would
        // succeed and the layer would appear — so its ABSENCE proves the
        // summarizer was not invoked.
        let orch = order_orchestrator(1_000_000);
        let mut tracking = AutoCompactTrackingState::default();
        let msgs = vec![long_user(0), long_user(1), long_user(2)];

        let res = orch
            .process_iteration_tracked(msgs, 0, &mut tracking)
            .await
            .expect("under threshold → Ok");

        assert!(!res.was_compacted, "must not compact under threshold");
        assert!(
            !res.layers_applied.contains(&CompactionLayer::Autocompact),
            "autocompact must not fire under threshold"
        );
        // No snip either (under budget) and no micro (no tool results).
        assert!(res.layers_applied.is_empty(), "no layers should fire");
        assert_eq!(tracking.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn success_resets_consecutive_failures_and_sets_was_compacted() {
        // Threshold 0 → always over threshold; default (unwired) autocompactor
        // SUCCEEDS via the deterministic fallback. Seed a non-zero failure count
        // (below the breaker limit) and confirm a successful pass resets it and
        // reports was_compacted=true.
        let orch = order_orchestrator(0);
        let mut tracking = AutoCompactTrackingState {
            consecutive_failures: 2,
            ..Default::default()
        };
        let msgs = vec![long_user(0), long_user(1)];

        let res = orch
            .process_iteration_tracked(msgs, 0, &mut tracking)
            .await
            .expect("default autocompactor succeeds");

        assert!(res.was_compacted, "autocompact should have run");
        assert!(res.layers_applied.contains(&CompactionLayer::Autocompact));
        assert_eq!(
            tracking.consecutive_failures, 0,
            "success resets the failure count"
        );
        assert_eq!(res.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn process_iteration_wrapper_uses_fresh_tracking() {
        // The thin wrapper starts from a fresh tracking state, so even after
        // (hypothetical) prior failures elsewhere it always attempts autocompact
        // when over threshold. Threshold 0 + default autocompactor → success.
        let orch = order_orchestrator(0);
        let msgs = vec![long_user(0), long_user(1)];
        let res = orch
            .process_iteration(msgs, 0)
            .await
            .expect("wrapper succeeds");
        assert!(res.was_compacted);
        assert!(res.layers_applied.contains(&CompactionLayer::Autocompact));
    }

    // --- COMPACT.4: HISTORY_SNIP gating ---------------------------------- //

    /// Serializes the env-mutating COMPACT.4 tests so the shared
    /// `CLAUDE_CODE_HISTORY_SNIP` process var isn't raced. The body runs with the
    /// var set to `value` (or removed when `None`); the prior value is always
    /// restored. The body is sync (drives async work via a local runtime) so the
    /// guard is never held across an `.await`.
    static SNIP_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_snip_env<R>(value: Option<&str>, body: impl FnOnce() -> R) -> R {
        let _guard = SNIP_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = std::env::var("CLAUDE_CODE_HISTORY_SNIP").ok();
        match value {
            Some(v) => std::env::set_var("CLAUDE_CODE_HISTORY_SNIP", v),
            None => std::env::remove_var("CLAUDE_CODE_HISTORY_SNIP"),
        }
        let out = body();
        match saved {
            Some(v) => std::env::set_var("CLAUDE_CODE_HISTORY_SNIP", v),
            None => std::env::remove_var("CLAUDE_CODE_HISTORY_SNIP"),
        }
        out
    }

    /// Run `fut` to completion on a fresh current-thread runtime, so an
    /// env-serialized (sync) test can drive the async orchestrator without
    /// holding `SNIP_ENV_LOCK` across an `.await`.
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    #[test]
    fn snip_disabled_by_default_does_not_fire_over_budget() {
        // COMPACT.4: with HISTORY_SNIP OFF (the default), the snip pass is a
        // no-op even with a history far over budget — no Snip layer is recorded.
        // Autocompact still escalates because the (un-snipped) history is over
        // threshold.
        with_snip_env(None, || {
            let orch = order_orchestrator(50);
            let mut tracking = AutoCompactTrackingState::default();
            let msgs: Vec<_> = (0..40).map(long_user).collect();

            let res = block_on(orch.process_iteration_tracked(msgs, 0, &mut tracking))
                .expect("over threshold → autocompact succeeds");

            assert!(
                !res.layers_applied.contains(&CompactionLayer::Snip),
                "snip must NOT fire when HISTORY_SNIP is off (default)"
            );
            assert!(
                res.layers_applied.contains(&CompactionLayer::Autocompact),
                "autocompact still escalates on the un-snipped, over-threshold history"
            );
        });
    }

    #[test]
    fn history_snip_env_flag_enables_snip() {
        // COMPACT.4: with CLAUDE_CODE_HISTORY_SNIP=1 the snip pass runs again,
        // sheds the oldest messages down to the protected tail, and records the
        // Snip layer (mirroring TS `feature('HISTORY_SNIP')` on).
        with_snip_env(Some("1"), || {
            let orch = order_orchestrator(50);
            let mut tracking = AutoCompactTrackingState::default();
            let msgs: Vec<_> = (0..40).map(long_user).collect();

            let res = block_on(orch.process_iteration_tracked(msgs, 0, &mut tracking))
                .expect("over threshold → autocompact succeeds");

            assert!(
                res.layers_applied.contains(&CompactionLayer::Snip),
                "snip should fire when HISTORY_SNIP is enabled and over budget"
            );
            assert!(res.total_tokens_freed > 0, "snip should free tokens");
        });
    }

    #[test]
    fn history_snip_enabled_parses_envbool() {
        // Byte-faithful `envBool('CLAUDE_CODE_HISTORY_SNIP', false)`: only the
        // exact strings "1" and "true" enable it (no trim / case-fold), absent or
        // anything else → false.
        with_snip_env(None, || assert!(!history_snip_enabled()));
        with_snip_env(Some("1"), || assert!(history_snip_enabled()));
        with_snip_env(Some("true"), || assert!(history_snip_enabled()));
        with_snip_env(Some("0"), || assert!(!history_snip_enabled()));
        with_snip_env(Some("yes"), || assert!(!history_snip_enabled()));
        with_snip_env(Some("TRUE"), || assert!(!history_snip_enabled()));
    }

    // Keep an explicit reference to CompactionResult's shape so the test module
    // documents the success contract it relies on (pre/post token counts feed
    // total_tokens_freed).
    #[allow(dead_code)]
    fn _result_shape(r: &CompactionResult) -> u64 {
        r.pre_compact_token_count
            .saturating_sub(r.post_compact_token_count)
    }
}
