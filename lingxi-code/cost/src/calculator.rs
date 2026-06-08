//! Cost calculator: converts a [`Usage`] record plus a [`ModelPricing`] entry
//! into a nano-USD total using saturating arithmetic everywhere.
//!
//! Saturating arithmetic means pathological inputs (e.g. `u64::MAX` tokens or
//! a `u64::MAX` rate) never panic — they pin to `u64::MAX`. Callers that need
//! to detect overflow can compare the result against `u64::MAX` or use
//! [`u64::checked_mul`] in their own code.

use crate::pricing::{
    first_party_name_to_canonical, ModelPricing, NonTokenBillableUnit, PricingCatalog, TokenClass,
};
use crate::usage::{ApiSpeed, Usage};

/// Stateless calculator that turns [`Usage`] + [`ModelPricing`] into money.
pub struct CostCalculator;

impl CostCalculator {
    /// Total cost in nano-USD. Saturating arithmetic — no panic on overflow.
    #[must_use]
    pub fn calculate_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        // COST.3 — Opus 4.6 fast-mode tier. claude-code `getModelCosts`
        // (`utils/modelCost.ts:144-153`) special-cases `CLAUDE_OPUS_4_6`:
        // `isFastMode = usage.speed === 'fast'` → `getOpus46CostTier(isFastMode)`
        // returns the $30/$150 `COST_TIER_30_150` instead of the catalog $5/$25
        // tier. The catalog is keyed only on `(provider, model)`, so the
        // speed-dependent tier is resolved here: when the usage record carries
        // `ApiSpeed::Fast` AND the (canonicalized) model is `claude-opus-4-6`,
        // swap in the fast-tier rates. (TS additionally ANDs `isFastModeEnabled()`,
        // a config-layer global that is unreachable from the cost crate — same
        // deferral as the prompt-caching model gates; the API only echoes
        // `speed: 'fast'` when fast mode was actually used for the request, so
        // `usage.speed == Fast` is the faithful per-request signal.)
        let fast_override = Self::opus_4_6_fast_override(usage, pricing);
        let pricing = fast_override.as_ref().unwrap_or(pricing);

        let mut total: u64 = 0;

        for (class, rate) in &pricing.token_rates {
            let tokens = usage.tokens_for(*class);
            total = total.saturating_add(tokens.saturating_mul(rate.nano_usd_per_token));
        }

        if let Some(s) = usage.server_tool_use {
            if let Some(per_req) = pricing
                .non_token_rates_nano_usd
                .get(&NonTokenBillableUnit::WebSearchRequest)
                .copied()
            {
                let requests = u64::from(s.web_search_requests);
                total = total.saturating_add(requests.saturating_mul(per_req));
            }
        }

        total
    }

    /// COST.3 — return the Opus 4.6 fast-mode pricing override when this usage
    /// record should be billed at the $30/$150 tier, else `None`.
    ///
    /// Fires only when `usage.speed == Some(`[`ApiSpeed::Fast`]`)` and the
    /// resolved model canonicalizes to `claude-opus-4-6` (mirrors TS
    /// `getCanonicalName(model) === CLAUDE_OPUS_4_6`, `modelCost.ts:148-152`).
    fn opus_4_6_fast_override(usage: &Usage, pricing: &ModelPricing) -> Option<ModelPricing> {
        if usage.speed != Some(ApiSpeed::Fast) {
            return None;
        }
        if first_party_name_to_canonical(&pricing.model_ref.model) != "claude-opus-4-6" {
            return None;
        }
        Some(PricingCatalog::opus_4_6_fast_pricing(&pricing.model_ref))
    }

    /// Estimated cache-read savings in nano-USD: the difference between the
    /// (counterfactual) input-rate cost of the cache-read tokens and what was
    /// actually paid at the cache-read rate. Returns `0` when either rate is
    /// missing, or when the cache rate is not cheaper than input.
    #[must_use]
    pub fn cache_savings_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        let Some(input_rate) = pricing.token_rates.get(&TokenClass::Input) else {
            return 0;
        };
        let Some(cache_rate) = pricing.token_rates.get(&TokenClass::CacheRead) else {
            return 0;
        };
        let would_have_paid = usage
            .tokens
            .cache_read
            .saturating_mul(input_rate.nano_usd_per_token);
        let actually_paid = usage
            .tokens
            .cache_read
            .saturating_mul(cache_rate.nano_usd_per_token);
        would_have_paid.saturating_sub(actually_paid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{ModelRef, MoneyPerToken, PricingCatalog, PricingSource, ProviderId};
    use crate::usage::{ApiSpeed, ServerToolUsage, TokenUsage};
    use std::collections::HashMap;

    #[test]
    fn opus_4_6_input_only_correct() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            ..Usage::default()
        };
        // 1M tokens × 5000 nano-USD/tok = 5e9 nano-USD = $5
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            5_000_000_000
        );
    }

    #[test]
    fn cache_savings_positive_when_read_rate_lower() {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage {
                cache_read: 1_000_000,
                ..TokenUsage::default()
            },
            ..Usage::default()
        };
        let savings = CostCalculator::cache_savings_nano_usd(&usage, &p);
        assert!(savings > 0);
    }

    // ----- COST.3: Opus 4.6 fast-mode tier -----

    fn opus_4_6_pricing() -> ModelPricing {
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6".into(),
        };
        c.resolve(&mr).unwrap().0
    }

    #[test]
    fn opus_4_6_fast_mode_bills_30_150() {
        let p = opus_4_6_pricing();
        // 1M input tokens at the fast tier: $30/Mtok = 30e9 nano-USD.
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            speed: Some(ApiSpeed::Fast),
            ..Usage::default()
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            30_000_000_000
        );
        // 1M output tokens at the fast tier: $150/Mtok = 150e9 nano-USD.
        let usage = Usage {
            tokens: TokenUsage {
                output: 1_000_000,
                ..TokenUsage::default()
            },
            speed: Some(ApiSpeed::Fast),
            ..Usage::default()
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            150_000_000_000
        );
    }

    #[test]
    fn opus_4_6_non_fast_bills_5_25() {
        let p = opus_4_6_pricing();
        // speed = None (the wire default today) → catalog $5/$25 tier, no regression.
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            ..Usage::default()
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            5_000_000_000
        );
        // speed = Standard is explicitly NOT fast → still $5/$25.
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            speed: Some(ApiSpeed::Standard),
            ..Usage::default()
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            5_000_000_000
        );
    }

    #[test]
    fn fast_mode_only_applies_to_opus_4_6() {
        // A Sonnet ($3/$15) usage record carrying speed=Fast is NOT rebilled at
        // the Opus fast tier — the override is gated on the canonical model name.
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-sonnet-4-6".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            speed: Some(ApiSpeed::Fast),
            ..Usage::default()
        };
        // Sonnet $3/Mtok = 3e9, unaffected by the fast flag.
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            3_000_000_000
        );
    }

    #[test]
    fn opus_4_6_fast_mode_resolves_via_date_suffixed_id() {
        // The override canonicalizes the model name, so a date-suffixed resolve
        // (whose pricing.model_ref is already canonical) still bills the fast tier.
        let c = PricingCatalog::builtin_reference();
        let mr = ModelRef {
            provider: ProviderId::Anthropic,
            model: "claude-opus-4-6-20251101".into(),
        };
        let (p, _) = c.resolve(&mr).unwrap();
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            speed: Some(ApiSpeed::Fast),
            ..Usage::default()
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            30_000_000_000
        );
    }

    // ----- COST.5: server-side web-search billing -----

    #[test]
    fn web_search_requests_billed_one_cent_each() {
        let p = opus_4_6_pricing();
        // 3 web-search requests × $0.01 = $0.03 = 30_000_000 nano-USD; no tokens.
        let usage = Usage {
            server_tool_use: Some(ServerToolUsage {
                web_search_requests: 3,
            }),
            ..Usage::default()
        };
        assert_eq!(CostCalculator::calculate_nano_usd(&usage, &p), 30_000_000);
    }

    #[test]
    fn web_search_billed_on_top_of_fast_tokens() {
        let p = opus_4_6_pricing();
        // 1M input fast = 30e9 nano + 2 web searches = 20_000_000 nano.
        let usage = Usage {
            tokens: TokenUsage {
                input: 1_000_000,
                ..TokenUsage::default()
            },
            server_tool_use: Some(ServerToolUsage {
                web_search_requests: 2,
            }),
            speed: Some(ApiSpeed::Fast),
        };
        assert_eq!(
            CostCalculator::calculate_nano_usd(&usage, &p),
            30_000_000_000 + 20_000_000
        );
    }

    #[test]
    fn defaults_no_server_tool_or_speed_bills_tokens_only() {
        let p = opus_4_6_pricing();
        // Default usage (server_tool_use None, speed None) → only token cost,
        // no web-search surcharge and no fast-mode escalation (regression guard).
        let usage = Usage {
            tokens: TokenUsage {
                input: 100,
                output: 50,
                ..TokenUsage::default()
            },
            ..Usage::default()
        };
        assert!(usage.server_tool_use.is_none());
        assert!(usage.speed.is_none());
        // 100×5_000 + 50×25_000 = 500_000 + 1_250_000 = 1_750_000 nano-USD.
        assert_eq!(CostCalculator::calculate_nano_usd(&usage, &p), 1_750_000);
    }

    #[test]
    fn saturating_no_overflow() {
        let mut p = ModelPricing {
            model_ref: ModelRef {
                provider: ProviderId::Anthropic,
                model: "x".into(),
            },
            token_rates: HashMap::default(),
            non_token_rates_nano_usd: HashMap::default(),
            effective_from: None,
            source: PricingSource::BuiltInReference {
                provider: ProviderId::Anthropic,
            },
        };
        p.token_rates.insert(
            TokenClass::Input,
            MoneyPerToken {
                nano_usd_per_token: u64::MAX,
            },
        );
        let usage = Usage {
            tokens: TokenUsage {
                input: u64::MAX,
                ..TokenUsage::default()
            },
            ..Usage::default()
        };
        // Should not panic.
        let _ = CostCalculator::calculate_nano_usd(&usage, &p);
    }
}
