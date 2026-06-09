use llm_client::{
    CostEstimator, LlmError, PricingCatalog, PricingModelRef, PricingPolicy, ProviderId,
    TokenPricing, TokenUsage, Usage,
};

fn pricing_model() -> PricingModelRef {
    PricingModelRef {
        pricing_provider_id: ProviderId::AnthropicFirstParty,
        billing_model: "claude-sonnet-4".to_string(),
        request_model: "claude-sonnet-4-20250514".to_string(),
        display_model: "Claude Sonnet 4".to_string(),
    }
}

fn usage() -> Usage {
    Usage {
        billable_tokens: TokenUsage {
            input: 1_000_000,
            output: 2_000_000,
            cache_write: 500_000,
            cache_read: 250_000,
            reasoning_output: 100_000,
        },
        ..Usage::default()
    }
}

#[test]
fn exact_price_match_computes_independent_token_bucket_costs() {
    let catalog = PricingCatalog::empty().with_price(
        ProviderId::AnthropicFirstParty,
        "claude-sonnet-4",
        TokenPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_write_per_million: 3.75,
            cache_read_per_million: 0.30,
            reasoning_per_million: 15.0,
        },
    );
    let estimator = CostEstimator::new(catalog, PricingPolicy::MarkUnestimated);

    let estimate = estimator.estimate(pricing_model(), &usage()).expect("estimate");

    assert!(estimate.estimated);
    assert_eq!(estimate.input_cost_usd, Some(3.0));
    assert_eq!(estimate.output_cost_usd, Some(30.0));
    assert_eq!(estimate.cache_write_cost_usd, Some(1.875));
    assert_eq!(estimate.cache_read_cost_usd, Some(0.075));
    assert_eq!(estimate.reasoning_cost_usd, Some(1.5));
    assert_eq!(estimate.total_cost_usd, Some(36.45));
}

#[test]
fn external_override_wins_over_builtin_price() {
    let catalog = PricingCatalog::empty()
        .with_price(
            ProviderId::AnthropicFirstParty,
            "claude-sonnet-4",
            TokenPricing::input_output(3.0, 15.0),
        )
        .with_override(
            ProviderId::AnthropicFirstParty,
            "claude-sonnet-4",
            TokenPricing::input_output(1.0, 2.0),
        );
    let estimator = CostEstimator::new(catalog, PricingPolicy::MarkUnestimated);

    let estimate = estimator.estimate(pricing_model(), &usage()).expect("estimate");

    assert_eq!(estimate.input_cost_usd, Some(1.0));
    assert_eq!(estimate.output_cost_usd, Some(4.0));
    assert_eq!(estimate.pricing_source.as_deref(), Some("override"));
}

#[test]
fn mark_unestimated_policy_returns_unestimated_cost_for_unknown_pricing() {
    let estimator = CostEstimator::new(PricingCatalog::empty(), PricingPolicy::MarkUnestimated);

    let estimate = estimator.estimate(pricing_model(), &usage()).expect("estimate");

    assert!(!estimate.estimated);
    assert_eq!(estimate.total_cost_usd, None);
    assert_eq!(estimate.pricing_model.billing_model, "claude-sonnet-4");
}

#[test]
fn require_priced_policy_returns_cost_unavailable_for_unknown_pricing() {
    let estimator = CostEstimator::new(PricingCatalog::empty(), PricingPolicy::RequirePriced);

    let error = estimator
        .estimate(pricing_model(), &usage())
        .expect_err("missing pricing should fail");

    assert!(matches!(error, LlmError::CostUnavailable { .. }));
}
