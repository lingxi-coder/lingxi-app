//! Fusion hard-budget reservation.
//!
//! Peak dollars are quoted from token *rates* and
//! `panelReservedInputTokensPerTurn` — never 1 byte = 1 token. Settlement
//! (`orchestrator::price_realized_usage`) prices each component's own actual
//! usage through the same [`FusionPriceBook`]; a component with no rate marks
//! the run `estimated = true` rather than guessing a dollar figure — §4
//! forbids a conservative fallback estimate.

use crate::config::FusionRuntimeConfig;
use crate::model_resolver::{ModelSource, ResolvedPanel, ResolvedSet};
use platform_api::{
    BudgetEnforcerHandle, BudgetError, BudgetReservationId, FusionCostClass, FusionError,
    FusionRequest,
};
use std::sync::Arc;

/// 1 byte = 1 token is ONLY the missing-usage settlement fallback, never the
/// reservation quote. Codex's original peak used 256 KiB for that mistake.
// `budget` is a private module (see `fusion/src/lib.rs`), so this `pub` item
// is unreachable outside the crate; it exists as a named regression
// comparator for `mistaken_byte_input_tokens`'s tests, not dead API surface.
#[allow(dead_code)]
pub const CODEX_MISTAKEN_INPUT_BYTES_PER_TURN: u64 = 256 * 1024;

/// Per-token / per-request rates for one model. `None` means unpriced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRates {
    /// Nano-USD per input token.
    pub input_nano_usd_per_token: u64,
    /// Nano-USD per output token.
    pub output_nano_usd_per_token: u64,
    /// Flat nano-USD per provider request.
    pub per_request_nano_usd: u64,
    /// Nano-USD per prompt-cache-READ token. `0` for a model whose catalog
    /// entry has input/output rates but no cache rate (never `None` — a
    /// missing cache rate must not turn an otherwise-priced, token-billed
    /// model unpriced and hard-reject it under a session `--max-budget`).
    pub cache_read_nano_usd_per_token: u64,
    /// Nano-USD per prompt-cache-WRITE token. Same `0`-default rule as
    /// [`Self::cache_read_nano_usd_per_token`].
    pub cache_write_nano_usd_per_token: u64,
    /// Nano-USD per reasoning-output token. Same `0`-default rule as
    /// [`Self::cache_read_nano_usd_per_token`]: a model with real
    /// input/output rates but no separate `ReasoningOutput` catalog entry
    /// (the common case — only a minority of models.dev rows price
    /// reasoning separately) must stay token-priced, never flip to fully
    /// unpriced over a missing reasoning rate.
    pub reasoning_nano_usd_per_token: u64,
}

/// Injected price table. Production wraps [`cost::PricingCatalog`].
pub trait FusionPriceBook: Send + Sync {
    /// Rates for `(profile, model)`, if the model is token-priced.
    fn rates_for(&self, profile: &str, model: &str) -> Option<ModelRates>;
}

impl FusionPriceBook for () {
    fn rates_for(&self, _profile: &str, _model: &str) -> Option<ModelRates> {
        None
    }
}

/// Quoted peak plus the token counts used to produce it (for fixtures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionQuote {
    /// Peak nano-USD to reserve.
    pub reserved_nano_usd: u64,
    /// Input tokens in the corrected formula.
    pub reserved_input_tokens: u64,
    /// Output tokens in the corrected formula.
    pub reserved_output_tokens: u64,
    /// Provider calls in the peak (panel turns + analyst + synth).
    pub max_calls: u64,
}

/// Token counts the mistaken 1-byte-1-token formula would have reserved.
// `budget` is a private module (see `fusion/src/lib.rs`), so this `pub` item
// is unreachable outside the crate; it exists as a named regression
// comparator for the corrected-quote tests below, not dead API surface.
#[allow(dead_code)]
#[must_use]
pub fn mistaken_byte_input_tokens(panel_count: u8, panel_max_turns: u32) -> u64 {
    u64::from(panel_count)
        .saturating_mul(u64::from(panel_max_turns))
        .saturating_mul(CODEX_MISTAKEN_INPUT_BYTES_PER_TURN)
}

/// Corrected reservation quote. Subscription models contribute $0.
///
/// # Errors
///
/// [`FusionError::InvalidConfiguration`] when a token-billed model has no
/// price and the session has a max budget.
pub fn quote(
    config: &FusionRuntimeConfig,
    resolved: &ResolvedSet,
    request: &FusionRequest,
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    session_has_max: bool,
) -> Result<FusionQuote, FusionError> {
    let panel_count = u64::from(u8::try_from(resolved.panels.len()).unwrap_or(u8::MAX));
    let turns = u64::from(config.panel_max_turns);
    let reserved_input_tokens = panel_count
        .saturating_mul(turns)
        .saturating_mul(u64::from(config.panel_reserved_input_tokens_per_turn));
    let panel_output = panel_count
        .saturating_mul(turns)
        .saturating_mul(u64::from(config.panel_max_output_tokens_per_turn));
    let analyst_output = u64::from(config.analyst_max_output_tokens)
        .saturating_mul(1 + u64::from(config.analysis_protocol_retries));
    let synth_output = u64::from(config.synthesizer_max_output_tokens);
    let reserved_output_tokens = panel_output
        .saturating_add(analyst_output)
        .saturating_add(synth_output);
    let panel_calls = panel_count.saturating_mul(turns);
    let analyst_calls = 1 + u64::from(config.analysis_protocol_retries);
    let max_calls = panel_calls.saturating_add(analyst_calls).saturating_add(1);

    let mut reserved_usd = 0_u64;
    for panel in &resolved.panels {
        reserved_usd = reserved_usd.saturating_add(model_peak(
            panel,
            catalog,
            prices,
            session_has_max,
            u64::from(config.panel_max_turns)
                .saturating_mul(u64::from(config.panel_reserved_input_tokens_per_turn)),
            u64::from(config.panel_max_turns)
                .saturating_mul(u64::from(config.panel_max_output_tokens_per_turn)),
            u64::from(config.panel_max_turns),
        )?);
    }
    reserved_usd = reserved_usd.saturating_add(model_peak(
        &resolved.analyst,
        catalog,
        prices,
        session_has_max,
        0,
        analyst_output,
        analyst_calls,
    )?);
    let parent = ResolvedPanel {
        profile: request.parent_profile.clone(),
        model: request.parent_model.clone(),
    };
    reserved_usd = reserved_usd.saturating_add(model_peak(
        &parent,
        catalog,
        prices,
        session_has_max,
        0,
        synth_output,
        1,
    )?);

    if let Some(cap) = config.max_reserved_nano_usd {
        reserved_usd = reserved_usd.min(cap);
    }

    Ok(FusionQuote {
        reserved_nano_usd: reserved_usd,
        reserved_input_tokens,
        reserved_output_tokens,
        max_calls,
    })
}

/// Price one component (a panel, the analyst, or the synthesizer/parent) from
/// its own token usage. `None` means the model has no price in `prices` and
/// is not a `Subscription`-class hint — the caller decides whether that is a
/// hard failure (reservation quote, when the session has a max budget) or an
/// `estimated = true` component (settlement).
///
/// `cache_read_tokens`/`cache_write_tokens` are priced through
/// [`ModelRates::cache_read_nano_usd_per_token`] /
/// [`ModelRates::cache_write_nano_usd_per_token`] the same way input/output
/// are — omitting them here would under-bill every Fusion run that uses
/// prompt caching against the SAME catalog the main turn loop's
/// `CostCalculator` bills the identical usage from in full.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub(crate) fn price_component(
    profile: &str,
    model: &str,
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    calls: u64,
) -> Option<u64> {
    let hints = catalog
        .list()
        .into_iter()
        .find(|row| row.profile == profile && row.model == model)
        .map(|row| row.hints);
    if hints.is_some_and(|h| h.cost_class == FusionCostClass::Subscription) {
        return Some(0);
    }
    prices.rates_for(profile, model).map(|rates| {
        input_tokens
            .saturating_mul(rates.input_nano_usd_per_token)
            .saturating_add(output_tokens.saturating_mul(rates.output_nano_usd_per_token))
            .saturating_add(
                cache_read_tokens.saturating_mul(rates.cache_read_nano_usd_per_token),
            )
            .saturating_add(
                cache_write_tokens.saturating_mul(rates.cache_write_nano_usd_per_token),
            )
            .saturating_add(
                reasoning_tokens.saturating_mul(rates.reasoning_nano_usd_per_token),
            )
            .saturating_add(calls.saturating_mul(rates.per_request_nano_usd))
    })
}

#[allow(clippy::too_many_arguments)]
fn model_peak(
    model: &ResolvedPanel,
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    session_has_max: bool,
    input_tokens: u64,
    output_tokens: u64,
    calls: u64,
) -> Result<u64, FusionError> {
    // The reservation quote is a peak estimate built from configured turn
    // caps, not real usage — it has no cache token counts to price, and
    // deliberately does not invent one (0 in, 0 out of the cache terms);
    // the cache premium is priced for real at settlement
    // (`orchestrator::price_realized_usage`), where the actual counts exist.
    match price_component(
        &model.profile,
        &model.model,
        catalog,
        prices,
        input_tokens,
        output_tokens,
        0,
        0,
        0,
        calls,
    ) {
        Some(v) => Ok(v),
        None if !session_has_max => Ok(0),
        None => Err(FusionError::InvalidConfiguration(format!(
            "token-billed model `{}/{}` has no price",
            model.profile, model.model
        ))),
    }
}

/// RAII hold. Drop spawns an async release so a forgotten path cannot leak.
pub struct ReservationLease {
    budget: Arc<dyn BudgetEnforcerHandle>,
    id: BudgetReservationId,
    quote: FusionQuote,
    disarmed: bool,
}

impl ReservationLease {
    /// Quoted peak this lease holds.
    #[must_use]
    pub fn quote(&self) -> &FusionQuote {
        &self.quote
    }

    /// Release the hold after actual spend and record it into the cost
    /// tracker. `disarmed` is set only AFTER `commit_reservation` succeeds —
    /// a cancel/timeout/abort parked on this await must not make `Drop` treat
    /// the hold as already settled (it would spawn a second release racing
    /// this call, or worse, leak the hold if this future is dropped between
    /// the two statements). On error the hold is still armed, so `Drop`'s
    /// spawned release is the fallback that reclaims capacity.
    pub async fn commit(mut self, actual_nano_usd: u64) -> Result<(), FusionError> {
        self.budget
            .commit_reservation(self.id, actual_nano_usd)
            .await
            .map_err(|err| map_budget_err(&err))?;
        self.disarmed = true;
        Ok(())
    }
}

impl Drop for ReservationLease {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        self.disarmed = true;
        let budget = Arc::clone(&self.budget);
        let id = self.id;
        tokio::spawn(async move {
            budget.release_reservation(id).await;
        });
    }
}

/// Quote + reserve. Zero provider calls happen before this returns.
///
/// # Errors
///
/// Preflight [`FusionError`] variants.
pub async fn acquire(
    config: &FusionRuntimeConfig,
    resolved: &ResolvedSet,
    request: &FusionRequest,
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    budget: Arc<dyn BudgetEnforcerHandle>,
) -> Result<ReservationLease, FusionError> {
    let session_has_max = budget.max_session_nano_usd().is_some();
    let quote = quote(config, resolved, request, catalog, prices, session_has_max)?;
    if !session_has_max || quote.reserved_nano_usd == 0 {
        // Nothing to hold (uncapped session, or a $0 quote), but `commit`
        // must still reach the REAL budget handle: settlement
        // (`commit_reservation`) is the only place Fusion's priced spend
        // reaches the session's CostTracker, so an uncapped session must not
        // make that spend invisible to `/cost` by routing commit through a
        // throwaway `NoopBudget`. `disarmed: true` because nothing was
        // reserved — `Drop` must not spawn a release for a hold never taken.
        return Ok(ReservationLease {
            budget,
            id: BudgetReservationId::NOOP,
            quote,
            disarmed: true,
        });
    }
    match budget.reserve_nano_usd(quote.reserved_nano_usd).await {
        Ok(id) => Ok(ReservationLease {
            budget,
            id,
            quote,
            disarmed: false,
        }),
        Err(err) => Err(map_budget_err(&err)),
    }
}

fn map_budget_err(err: &BudgetError) -> FusionError {
    match err {
        BudgetError::Exceeded { .. } => FusionError::BudgetExceeded,
        BudgetError::Internal(_) => FusionError::BudgetReservationUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FusionRuntimeConfig;
    use crate::model_resolver::CatalogModel;
    use platform_api::{
        FusionModelHints, FusionOrigin, FusionPreset, FusionRequest, DEFAULT_FUSION_DIMENSIONS,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::sync::Mutex;

    struct MapPrices(HashMap<(String, String), ModelRates>);
    impl FusionPriceBook for MapPrices {
        fn rates_for(&self, profile: &str, model: &str) -> Option<ModelRates> {
            self.0
                .get(&(profile.to_string(), model.to_string()))
                .copied()
        }
    }

    fn hinted(profile: &str, model: &str, sub: bool) -> CatalogModel {
        CatalogModel {
            profile: profile.into(),
            model: model.into(),
            hints: FusionModelHints {
                eligible: true,
                quality_rank: 90,
                judge_eligible: true,
                cost_class: if sub {
                    FusionCostClass::Subscription
                } else {
                    FusionCostClass::Medium
                },
                ..FusionModelHints::default()
            },
            structured_output: true,
        }
    }

    fn request() -> FusionRequest {
        FusionRequest {
            schema_version: 1,
            origin: FusionOrigin::Slash,
            prompt: "task".into(),
            preset: FusionPreset::Quality,
            models: None,
            dimensions: DEFAULT_FUSION_DIMENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            partial_ok: true,
            max_panel: None,
            cross_provider: true,
            parent_profile: "anthropic".into(),
            parent_model: "sonnet".into(),
            conversation_id: None,
            workflow_run_id: None,
        }
    }

    fn resolved_three() -> ResolvedSet {
        ResolvedSet {
            panels: vec![
                ResolvedPanel {
                    profile: "anthropic".into(),
                    model: "sonnet".into(),
                },
                ResolvedPanel {
                    profile: "openai".into(),
                    model: "terra".into(),
                },
                ResolvedPanel {
                    profile: "deepseek".into(),
                    model: "pro".into(),
                },
            ],
            analyst: ResolvedPanel {
                profile: "anthropic".into(),
                model: "sonnet".into(),
            },
        }
    }

    fn unit_prices() -> MapPrices {
        let rate = ModelRates {
            input_nano_usd_per_token: 1,
            output_nano_usd_per_token: 1,
            per_request_nano_usd: 0,
            cache_read_nano_usd_per_token: 1,
            cache_write_nano_usd_per_token: 1,
            reasoning_nano_usd_per_token: 1,
        };
        let mut map = HashMap::new();
        for (p, m) in [
            ("anthropic", "sonnet"),
            ("openai", "terra"),
            ("deepseek", "pro"),
        ] {
            map.insert((p.into(), m.into()), rate);
        }
        MapPrices(map)
    }

    #[test]
    fn corrected_formula_fits_where_byte_equals_token_would_not() {
        let config = FusionRuntimeConfig::defaults();
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let q = quote(
            &config,
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            true,
        )
        .unwrap();
        let wrong_input = mistaken_byte_input_tokens(3, config.panel_max_turns);
        assert_eq!(
            q.reserved_input_tokens,
            3 * u64::from(config.panel_max_turns)
                * u64::from(config.panel_reserved_input_tokens_per_turn)
        );
        assert!(
            q.reserved_input_tokens < wrong_input,
            "corrected input {} must be below Codex byte-as-token {} ",
            q.reserved_input_tokens,
            wrong_input
        );
        // Session remaining sits between the two peaks (input-only, $1/token).
        let remaining = (q.reserved_input_tokens + wrong_input) / 2;
        assert!(q.reserved_nano_usd <= remaining);
        assert!(wrong_input > remaining);
    }

    #[test]
    fn subscription_models_quote_zero_dollars() {
        let config = FusionRuntimeConfig::defaults();
        let catalog = vec![
            hinted("anthropic", "sonnet", true),
            hinted("openai", "terra", true),
            hinted("deepseek", "pro", true),
        ];
        let q = quote(&config, &resolved_three(), &request(), &catalog, &(), true).unwrap();
        assert_eq!(q.reserved_nano_usd, 0);
    }

    #[test]
    fn unpriced_token_model_rejected_when_session_has_max() {
        let config = FusionRuntimeConfig::defaults();
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let err = quote(&config, &resolved_three(), &request(), &catalog, &(), true).unwrap_err();
        assert!(matches!(err, FusionError::InvalidConfiguration(_)));
    }

    /// G003-cache follow-up: `price_component` must charge cache-read and
    /// cache-write tokens the same way it charges input/output — a panel or
    /// analyst/synth call that used prompt caching is still real spend, and
    /// the main turn loop's `CostCalculator` (over the SAME catalog) already
    /// bills all four classes in full.
    #[test]
    fn price_component_bills_cache_read_and_cache_write_tokens() {
        let catalog = vec![hinted("anthropic", "sonnet", false)];
        let rates = unit_prices(); // 1 nano-USD/token on every class
        let priced = price_component(
            "anthropic", "sonnet", &catalog, &rates, 5, 8, // input, output
            200, 40, // cache_read, cache_write
            0,  // reasoning
            0,
        )
        .expect("anthropic/sonnet has a unit rate");
        assert_eq!(
            priced,
            5 + 8 + 200 + 40,
            "cache-read and cache-write tokens must be priced, not silently dropped \
(200 cache-read + 40 cache-write tokens went unbilled before this fix)"
        );
    }

    /// Finding [1]: reasoning-output tokens must be priced the same way
    /// input/output/cache tokens are — a reasoning-heavy panel's largest
    /// cost bucket (e.g. `gemini-3.1-pro-preview` at $12/Mtok reasoning)
    /// otherwise reaches neither `realized_nano_usd` nor the session budget,
    /// even though the provider bills it in full.
    #[test]
    fn price_component_bills_reasoning_output_tokens() {
        let catalog = vec![hinted("anthropic", "sonnet", false)];
        let rates = unit_prices(); // 1 nano-USD/token on every class
        let priced = price_component(
            "anthropic", "sonnet", &catalog, &rates, 5, 8, // input, output
            0, 0, // cache_read, cache_write
            50_000, // reasoning
            0,
        )
        .expect("anthropic/sonnet has a unit rate");
        assert_eq!(
            priced,
            5 + 8 + 50_000,
            "reasoning-output tokens must be priced, not silently dropped \
(50,000 reasoning tokens went unbilled before this fix)"
        );
    }

    struct RecordingBudget {
        max: Option<u64>,
        held: AtomicU64,
        calls: AtomicU64,
        reserves: Mutex<Vec<u64>>,
        fail: bool,
        /// `commit_reservation` call count — 0 in tests below unless noted.
        commit_calls: AtomicU64,
        /// `actual_nano_usd` argument recorded by every `commit_reservation` call.
        committed: Mutex<Vec<u64>>,
        /// When `Some`, `commit_reservation` returns this error instead of
        /// recording — used to prove `ReservationLease::commit` leaves the hold
        /// ARMED (so `Drop` still releases it) when the commit itself fails.
        fail_commit: Option<BudgetError>,
    }

    impl RecordingBudget {
        fn capped(max: u64) -> Arc<Self> {
            Arc::new(Self {
                max: Some(max),
                held: AtomicU64::new(0),
                calls: AtomicU64::new(0),
                reserves: Mutex::new(Vec::new()),
                fail: false,
                commit_calls: AtomicU64::new(0),
                committed: Mutex::new(Vec::new()),
                fail_commit: None,
            })
        }

        fn uncapped() -> Arc<Self> {
            Arc::new(Self {
                max: None,
                held: AtomicU64::new(0),
                calls: AtomicU64::new(0),
                reserves: Mutex::new(Vec::new()),
                fail: false,
                commit_calls: AtomicU64::new(0),
                committed: Mutex::new(Vec::new()),
                fail_commit: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl BudgetEnforcerHandle for RecordingBudget {
        async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
        fn max_session_nano_usd(&self) -> Option<u64> {
            self.max
        }
        async fn active_reservation_nano_usd(&self) -> u64 {
            self.held.load(Ordering::SeqCst)
        }
        async fn reserve_nano_usd(
            &self,
            nano_usd: u64,
        ) -> Result<BudgetReservationId, BudgetError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.reserves.lock().await.push(nano_usd);
            if self.fail {
                return Err(BudgetError::Exceeded {
                    current_nano_usd: 0,
                });
            }
            let new = self.held.load(Ordering::SeqCst).saturating_add(nano_usd);
            if let Some(max) = self.max {
                if new > max {
                    return Err(BudgetError::Exceeded {
                        current_nano_usd: self.held.load(Ordering::SeqCst),
                    });
                }
            }
            self.held.store(new, Ordering::SeqCst);
            Ok(BudgetReservationId::from_raw(new.max(1)))
        }
        async fn commit_reservation(
            &self,
            id: BudgetReservationId,
            actual_nano_usd: u64,
        ) -> Result<(), BudgetError> {
            self.commit_calls.fetch_add(1, Ordering::SeqCst);
            self.committed.lock().await.push(actual_nano_usd);
            if let Some(err) = &self.fail_commit {
                return Err(match err {
                    BudgetError::Exceeded { current_nano_usd } => BudgetError::Exceeded {
                        current_nano_usd: *current_nano_usd,
                    },
                    BudgetError::Internal(s) => BudgetError::Internal(s.clone()),
                });
            }
            self.release_reservation(id).await;
            Ok(())
        }
        async fn release_reservation(&self, _id: BudgetReservationId) {
            self.held.store(0, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn acquire_failure_does_not_hold_capacity() {
        let budget = RecordingBudget {
            max: Some(1),
            held: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            reserves: Mutex::new(Vec::new()),
            fail: true,
            commit_calls: AtomicU64::new(0),
            committed: Mutex::new(Vec::new()),
            fail_commit: None,
        };
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let err = match acquire(
            &FusionRuntimeConfig::defaults(),
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            Arc::new(budget),
        )
        .await
        {
            Ok(_) => panic!("expected reserve failure"),
            Err(e) => e,
        };
        assert!(matches!(err, FusionError::BudgetExceeded));
    }

    #[tokio::test]
    async fn two_acquires_cannot_both_cross_the_cap() {
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let config = FusionRuntimeConfig::defaults();
        let q = quote(
            &config,
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            true,
        )
        .unwrap();
        let budget = RecordingBudget::capped(q.reserved_nano_usd.saturating_mul(3) / 2);
        let a = acquire(
            &config,
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            budget.clone(),
        )
        .await;
        let b = acquire(
            &config,
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            budget.clone(),
        )
        .await;
        let wins = u8::from(a.is_ok()) + u8::from(b.is_ok());
        assert_eq!(wins, 1, "exactly one Fusion hold on a 1.5×-quote cap");
    }

    #[tokio::test]
    async fn drop_releases_hold() {
        let budget = RecordingBudget::capped(u64::MAX);
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        {
            let _lease = acquire(
                &FusionRuntimeConfig::defaults(),
                &resolved_three(),
                &request(),
                &catalog,
                &unit_prices(),
                budget.clone(),
            )
            .await
            .unwrap();
            assert!(budget.held.load(Ordering::SeqCst) > 0);
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(budget.held.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn acquire_succeeds_when_session_has_max_and_price_book_has_rates() {
        // The composition root's failure mode before the price book was wired:
        // `--max-budget` made every token-billed panel hit `InvalidConfiguration`
        // at quote() because `prices` was the `()` book (`rates_for` always
        // `None`). With a real price book present, a capped session must reach
        // `reserve_nano_usd` and succeed.
        let budget = RecordingBudget::capped(u64::MAX);
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let lease = acquire(
            &FusionRuntimeConfig::defaults(),
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            budget.clone(),
        )
        .await
        .expect("priced catalog under a capped session must reserve, not reject");
        assert!(lease.quote().reserved_nano_usd > 0, "unit-priced quote is non-zero");
        assert_eq!(budget.calls.load(Ordering::SeqCst), 1, "exactly one reserve call");
    }

    #[tokio::test]
    async fn acquire_with_uncapped_session_still_commits_to_the_real_budget() {
        // F001 fix round 1, finding #1: an uncapped session (no
        // `--max-budget`, `max_session_nano_usd() == None`) took the early
        // `!session_has_max` return in `acquire`, which used to hand back a
        // lease backed by a throwaway `NoopBudget` — `commit` on that lease
        // never reached the real budget's `commit_reservation`, so Fusion
        // spend on an uncapped session never reached the session's
        // CostTracker / `/cost`. Prove `commit` reaches the REAL handle: no
        // reservation is taken (uncapped ⇒ `reserve_nano_usd` is never
        // called) but `commit_reservation` still records the actual amount.
        let budget = RecordingBudget::uncapped();
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let lease = acquire(
            &FusionRuntimeConfig::defaults(),
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            budget.clone(),
        )
        .await
        .expect("an uncapped session must not reject at quote/reserve");
        assert_eq!(
            budget.calls.load(Ordering::SeqCst),
            0,
            "an uncapped session never calls reserve_nano_usd"
        );
        lease.commit(777).await.expect("commit reaches the real budget handle");
        assert_eq!(
            budget.commit_calls.load(Ordering::SeqCst),
            1,
            "commit_reservation must be called exactly once even on the noop-lease path"
        );
        assert_eq!(
            budget.committed.lock().await.clone(),
            vec![777],
            "the real budget must record the actual realized spend, not discard it"
        );
    }

    #[tokio::test]
    async fn commit_stays_armed_on_reservation_error_so_drop_still_releases() {
        // `ReservationLease::commit` must await `commit_reservation` BEFORE
        // setting `disarmed = true`. Prove it from the failure side: when the
        // budget's `commit_reservation` itself errors, the lease must still be
        // armed when it is dropped, so `Drop`'s spawned release is the
        // fallback that reclaims the hold instead of leaking it for the rest
        // of the session.
        let budget = Arc::new(RecordingBudget {
            max: Some(u64::MAX),
            held: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            reserves: Mutex::new(Vec::new()),
            fail: false,
            commit_calls: AtomicU64::new(0),
            committed: Mutex::new(Vec::new()),
            fail_commit: Some(BudgetError::Internal("commit backend down".into())),
        });
        let catalog = vec![
            hinted("anthropic", "sonnet", false),
            hinted("openai", "terra", false),
            hinted("deepseek", "pro", false),
        ];
        let lease = acquire(
            &FusionRuntimeConfig::defaults(),
            &resolved_three(),
            &request(),
            &catalog,
            &unit_prices(),
            budget.clone(),
        )
        .await
        .unwrap();
        assert!(budget.held.load(Ordering::SeqCst) > 0, "hold in place before commit");
        let err = lease.commit(999).await.unwrap_err();
        assert!(
            matches!(err, FusionError::BudgetReservationUnavailable),
            "commit surfaces the mapped error, got {err:?}"
        );
        assert_eq!(
            budget.commit_calls.load(Ordering::SeqCst),
            1,
            "commit_reservation was attempted exactly once"
        );
        // The lease (moved into `commit`, which took `self`) is dropped here —
        // if `commit` had disarmed before the `?`, this held amount would
        // never clear because Drop only spawns a release when armed.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            budget.held.load(Ordering::SeqCst),
            0,
            "Drop released the hold because commit left it armed on failure"
        );
    }
}
