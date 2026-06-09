//! Pricing catalog and per-call cost estimation.

use std::collections::HashMap;

use crate::{CostEstimate, LlmError, PricingModelRef, ProviderId, Usage};

/// Unknown-pricing policy for cost estimation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricingPolicy {
    /// Return an unestimated cost when pricing is unknown.
    MarkUnestimated,
    /// Apply a configured fallback tier when pricing is unknown.
    ApplyFallbackTier,
    /// Return an error when pricing is unknown.
    RequirePriced,
}

/// Per-million-token prices for independent billable buckets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenPricing {
    /// Input-token price per million tokens.
    pub input_per_million: f64,
    /// Output-token price per million tokens.
    pub output_per_million: f64,
    /// Cache-write price per million tokens.
    pub cache_write_per_million: f64,
    /// Cache-read price per million tokens.
    pub cache_read_per_million: f64,
    /// Reasoning-token price per million tokens.
    pub reasoning_per_million: f64,
}

impl TokenPricing {
    /// Create pricing with input and output buckets only.
    #[must_use]
    pub fn input_output(input_per_million: f64, output_per_million: f64) -> Self {
        Self {
            input_per_million,
            output_per_million,
            cache_write_per_million: 0.0,
            cache_read_per_million: 0.0,
            reasoning_per_million: 0.0,
        }
    }
}

/// Pricing catalog with built-in prices and external overrides.
#[derive(Debug, Clone, Default)]
pub struct PricingCatalog {
    prices: HashMap<PricingKey, TokenPricing>,
    overrides: HashMap<PricingKey, TokenPricing>,
}

impl PricingCatalog {
    /// Create an empty pricing catalog.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a built-in price entry.
    #[must_use]
    pub fn with_price(
        mut self,
        provider_id: ProviderId,
        billing_model: impl Into<String>,
        pricing: TokenPricing,
    ) -> Self {
        self.prices
            .insert(PricingKey::new(provider_id, billing_model), pricing);
        self
    }

    /// Add an external override price entry.
    #[must_use]
    pub fn with_override(
        mut self,
        provider_id: ProviderId,
        billing_model: impl Into<String>,
        pricing: TokenPricing,
    ) -> Self {
        self.overrides
            .insert(PricingKey::new(provider_id, billing_model), pricing);
        self
    }

    fn lookup(&self, pricing_model: &PricingModelRef) -> Option<(TokenPricing, &'static str)> {
        let key = PricingKey::new(
            pricing_model.pricing_provider_id.clone(),
            pricing_model.billing_model.clone(),
        );

        self.overrides
            .get(&key)
            .copied()
            .map(|pricing| (pricing, "override"))
            .or_else(|| {
                self.prices
                    .get(&key)
                    .copied()
                    .map(|pricing| (pricing, "builtin"))
            })
    }
}

/// Cost estimator using a pricing catalog and unknown-pricing policy.
#[derive(Debug, Clone)]
pub struct CostEstimator {
    catalog: PricingCatalog,
    policy: PricingPolicy,
}

impl CostEstimator {
    /// Create a cost estimator.
    #[must_use]
    pub fn new(catalog: PricingCatalog, policy: PricingPolicy) -> Self {
        Self { catalog, policy }
    }

    /// Estimate cost from resolved pricing identity and normalized usage.
    pub fn estimate(
        &self,
        pricing_model: PricingModelRef,
        usage: &Usage,
    ) -> Result<CostEstimate, LlmError> {
        let Some((pricing, source)) = self.catalog.lookup(&pricing_model) else {
            return match self.policy {
                PricingPolicy::MarkUnestimated | PricingPolicy::ApplyFallbackTier => {
                    Ok(CostEstimate::unestimated(pricing_model))
                }
                PricingPolicy::RequirePriced => Err(LlmError::CostUnavailable {
                    message: format!(
                        "missing pricing for {:?}/{}",
                        pricing_model.pricing_provider_id, pricing_model.billing_model
                    ),
                }),
            };
        };

        let tokens = usage.billable_tokens;
        let input_cost = price(tokens.input, pricing.input_per_million);
        let output_cost = price(tokens.output, pricing.output_per_million);
        let cache_write_cost = price(tokens.cache_write, pricing.cache_write_per_million);
        let cache_read_cost = price(tokens.cache_read, pricing.cache_read_per_million);
        let reasoning_cost = price(tokens.reasoning_output, pricing.reasoning_per_million);
        let total_cost = input_cost + output_cost + cache_write_cost + cache_read_cost + reasoning_cost;

        Ok(CostEstimate {
            pricing_model,
            total_cost_usd: Some(total_cost),
            input_cost_usd: Some(input_cost),
            output_cost_usd: Some(output_cost),
            cache_read_cost_usd: Some(cache_read_cost),
            cache_write_cost_usd: Some(cache_write_cost),
            reasoning_cost_usd: Some(reasoning_cost),
            estimated: true,
            pricing_source: Some(source.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PricingKey {
    provider_id: ProviderId,
    billing_model: String,
}

impl PricingKey {
    fn new(provider_id: ProviderId, billing_model: impl Into<String>) -> Self {
        Self {
            provider_id,
            billing_model: billing_model.into(),
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn price(tokens: u64, per_million: f64) -> f64 {
    (tokens as f64 / 1_000_000.0) * per_million
}
