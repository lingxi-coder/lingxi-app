//! Fusion hard-budget reservation.
//!
//! Peak dollars are quoted from token *rates* and
//! `panelReservedInputTokensPerTurn` — never 1 byte = 1 token. Missing usage
//! at settlement may fall back to that conservative byte heuristic, marked
//! `estimated`.

use crate::config::FusionRuntimeConfig;
use crate::model_resolver::{ModelSource, ResolvedPanel, ResolvedSet};
use crate::panel::PanelInternal;
use platform_api::{
    BudgetEnforcerHandle, BudgetError, BudgetReservationId, FusionCostClass, FusionError,
    FusionRequest, FusionUsage, PanelRunStatus,
};
use std::sync::Arc;

/// 1 byte = 1 token is ONLY the missing-usage settlement fallback, never the
/// reservation quote. Codex's original peak used 256 KiB for that mistake.
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

fn model_peak(
    model: &ResolvedPanel,
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    session_has_max: bool,
    input_tokens: u64,
    output_tokens: u64,
    calls: u64,
) -> Result<u64, FusionError> {
    let hints = catalog
        .list()
        .into_iter()
        .find(|row| row.profile == model.profile && row.model == model.model)
        .map(|row| row.hints);
    if hints.is_some_and(|h| h.cost_class == FusionCostClass::Subscription) {
        return Ok(0);
    }
    match prices.rates_for(&model.profile, &model.model) {
        Some(rates) => Ok(input_tokens
            .saturating_mul(rates.input_nano_usd_per_token)
            .saturating_add(output_tokens.saturating_mul(rates.output_nano_usd_per_token))
            .saturating_add(calls.saturating_mul(rates.per_request_nano_usd))),
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
    /// No session cap / zero quote.
    #[must_use]
    pub(crate) fn noop() -> Self {
        Self {
            budget: Arc::new(NoopBudget),
            id: BudgetReservationId::NOOP,
            quote: FusionQuote {
                reserved_nano_usd: 0,
                reserved_input_tokens: 0,
                reserved_output_tokens: 0,
                max_calls: 0,
            },
            disarmed: true,
        }
    }

    /// Quoted peak this lease holds.
    #[must_use]
    pub fn quote(&self) -> &FusionQuote {
        &self.quote
    }

    /// Release the hold after actual spend (already in the cost tracker).
    pub async fn commit(mut self, actual_nano_usd: u64) -> Result<(), FusionError> {
        self.disarmed = true;
        self.budget
            .commit_reservation(self.id, actual_nano_usd)
            .await
            .map_err(map_budget_err)
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

struct NoopBudget;

#[async_trait::async_trait]
impl BudgetEnforcerHandle for NoopBudget {
    async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
        Ok(())
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
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
        let _ = budget;
        let mut lease = ReservationLease::noop();
        lease.quote = quote;
        return Ok(lease);
    }
    match budget.reserve_nano_usd(quote.reserved_nano_usd).await {
        Ok(id) => Ok(ReservationLease {
            budget,
            id,
            quote,
            disarmed: false,
        }),
        Err(err) => Err(map_budget_err(err)),
    }
}

fn map_budget_err(err: BudgetError) -> FusionError {
    match err {
        BudgetError::Exceeded { .. } => FusionError::BudgetExceeded,
        BudgetError::Internal(_) => FusionError::BudgetReservationUnavailable,
    }
}

/// Settlement: use billed nano-USD when present, otherwise a conservative
/// per-panel share of the reservation, flagged estimated.
#[must_use]
pub fn settle_usage(panels: &[PanelInternal], quote: &FusionQuote, billed: u64) -> FusionUsage {
    let mut usage = FusionUsage {
        reserved_max_nano_usd: quote.reserved_nano_usd,
        realized_nano_usd: billed,
        ..FusionUsage::default()
    };
    let n = panels
        .iter()
        .filter(|p| p.status == PanelRunStatus::Completed)
        .count()
        .max(1) as u64;
    let share = quote.reserved_nano_usd / n;
    for panel in panels {
        if let Some(panel_usage) = &panel.usage {
            usage.input_tokens = usage.input_tokens.saturating_add(panel_usage.input_tokens);
            usage.output_tokens = usage
                .output_tokens
                .saturating_add(panel_usage.output_tokens);
            usage.provider_requests = usage
                .provider_requests
                .saturating_add(panel_usage.provider_requests);
            usage.estimated = usage.estimated || panel_usage.estimated;
        } else if panel.status == PanelRunStatus::Completed {
            usage.estimated = true;
            usage.realized_nano_usd = usage.realized_nano_usd.saturating_add(share);
        }
    }
    if billed == 0 && usage.realized_nano_usd == 0 {
        let no_tokens = panels
            .iter()
            .filter(|p| p.status == PanelRunStatus::Completed)
            .all(|p| {
                p.usage.as_ref().is_none_or(|u| {
                    u.input_tokens == 0 && u.output_tokens == 0 && u.realized_nano_usd == 0
                })
            });
        if no_tokens {
            usage.estimated = true;
            usage.realized_nano_usd = share;
        }
    }
    usage
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

    struct RecordingBudget {
        max: Option<u64>,
        held: AtomicU64,
        calls: AtomicU64,
        reserves: Mutex<Vec<u64>>,
        fail: bool,
    }

    impl RecordingBudget {
        fn capped(max: u64) -> Arc<Self> {
            Arc::new(Self {
                max: Some(max),
                held: AtomicU64::new(0),
                calls: AtomicU64::new(0),
                reserves: Mutex::new(Vec::new()),
                fail: false,
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
}
