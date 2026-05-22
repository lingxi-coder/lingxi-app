//! Cost calculator: converts a [`Usage`] record plus a [`ModelPricing`] entry
//! into a nano-USD total using saturating arithmetic everywhere.
//!
//! Saturating arithmetic means pathological inputs (e.g. `u64::MAX` tokens or
//! a `u64::MAX` rate) never panic — they pin to `u64::MAX`. Callers that need
//! to detect overflow can compare the result against `u64::MAX` or use
//! [`u64::checked_mul`] in their own code.

use crate::pricing::{ModelPricing, NonTokenBillableUnit, TokenClass};
use crate::usage::Usage;

/// Stateless calculator that turns [`Usage`] + [`ModelPricing`] into money.
pub struct CostCalculator;

impl CostCalculator {
    /// Total cost in nano-USD. Saturating arithmetic — no panic on overflow.
    #[must_use]
    pub fn calculate_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
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
    use crate::usage::TokenUsage;
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
