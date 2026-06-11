use llm_client::{
    CostEstimator, LlmError, PricingCatalog, PricingConfig, PricingModelRef, PricingPolicy,
    ProviderId, TokenPricing, TokenUsage, Usage,
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

// ── Task 2: add_override + PricingConfig serde tests ─────────────────────────

/// `add_override` (mutable builder) produces the same result as the existing
/// `with_override` (consuming builder) — both override wins over builtin.
#[test]
fn add_override_wins_over_builtin_price() {
    let mut catalog = PricingCatalog::empty().with_price(
        ProviderId::AnthropicFirstParty,
        "claude-sonnet-4",
        TokenPricing::input_output(3.0, 15.0),
    );
    // add_override applied after construction — should shadow the builtin.
    catalog.add_override(
        ProviderId::AnthropicFirstParty,
        "claude-sonnet-4",
        TokenPricing::input_output(1.5, 7.0),
    );
    let estimator = CostEstimator::new(catalog, PricingPolicy::MarkUnestimated);

    let usage_1m = Usage {
        billable_tokens: TokenUsage { input: 1_000_000, output: 1_000_000, ..Default::default() },
        ..Default::default()
    };
    let estimate = estimator.estimate(pricing_model(), &usage_1m).expect("estimate");

    assert_eq!(estimate.pricing_source.as_deref(), Some("override"));
    // 1M input × $1.5/M = $1.5 + 1M output × $7.0/M = $7.0 → $8.5
    let total = estimate.total_cost_usd.expect("total must be Some");
    assert!((total - 8.5).abs() < 1e-9, "total must be $8.5, got ${total}");
    let input = estimate.input_cost_usd.expect("input must be Some");
    assert!((input - 1.5).abs() < 1e-9, "input must be $1.5/M, got ${input}");
}

/// `PricingConfig` with overrides round-trips through JSON serde.
#[test]
fn pricing_config_with_overrides_serde_roundtrip() {
    let tp = TokenPricing {
        input_per_million: 1.5,
        output_per_million: 6.0,
        cache_write_per_million: 1.875,
        cache_read_per_million: 0.15,
        reasoning_per_million: 6.0,
    };
    let cfg = PricingConfig {
        require_priced: false,
        overrides: vec![("my-model".to_string(), tp)],
    };

    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: PricingConfig = serde_json::from_str(&json).expect("deserialize");

    assert!(!back.require_priced);
    assert_eq!(back.overrides.len(), 1);
    let (model_id, tp_back) = &back.overrides[0];
    assert_eq!(model_id, "my-model");
    assert!((tp_back.input_per_million - 1.5).abs() < 1e-12);
    assert!((tp_back.output_per_million - 6.0).abs() < 1e-12);
    assert!((tp_back.cache_write_per_million - 1.875).abs() < 1e-12);
    assert!((tp_back.cache_read_per_million - 0.15).abs() < 1e-12);
    assert!((tp_back.reasoning_per_million - 6.0).abs() < 1e-12);
}

/// `PricingConfig` with absent overrides field (e.g. from old configs) deserializes
/// with an empty overrides vec — no serde errors, no regression.
#[test]
fn pricing_config_absent_overrides_deserializes_to_empty() {
    // The old config shape only had `require_priced`.
    let json = r#"{"require_priced": false}"#;
    let cfg: PricingConfig = serde_json::from_str(json).expect("must deserialize");
    assert!(cfg.overrides.is_empty(), "absent overrides must default to empty");

    // Also the default-derived shape must have an empty overrides.
    let default_cfg = PricingConfig::default();
    assert!(default_cfg.overrides.is_empty());
}

/// `TokenPricing` JSON round-trip via camelCase serde names.
#[test]
fn token_pricing_serde_roundtrip_camel_case() {
    let json = r#"{
        "inputPerMtok": 3.0,
        "outputPerMtok": 15.0,
        "cacheWritePerMtok": 3.75,
        "cacheReadPerMtok": 0.30,
        "reasoningPerMtok": 1.5
    }"#;
    let tp: TokenPricing = serde_json::from_str(json).expect("deserialize");
    assert!((tp.input_per_million - 3.0).abs() < 1e-12);
    assert!((tp.output_per_million - 15.0).abs() < 1e-12);
    assert!((tp.cache_write_per_million - 3.75).abs() < 1e-12);
    assert!((tp.cache_read_per_million - 0.30).abs() < 1e-12);
    assert!((tp.reasoning_per_million - 1.5).abs() < 1e-12);

    // Serialized form must use camelCase keys.
    let back = serde_json::to_string(&tp).expect("serialize");
    assert!(back.contains("inputPerMtok"), "must serialize to camelCase: {back}");
    assert!(back.contains("outputPerMtok"), "must serialize to camelCase: {back}");
}

/// Optional `TokenPricing` fields default to 0.0 when absent.
#[test]
fn token_pricing_optional_fields_default_to_zero() {
    let json = r#"{"inputPerMtok": 2.0, "outputPerMtok": 8.0}"#;
    let tp: TokenPricing = serde_json::from_str(json).expect("deserialize");
    assert!((tp.cache_write_per_million - 0.0).abs() < 1e-12);
    assert!((tp.cache_read_per_million - 0.0).abs() < 1e-12);
    assert!((tp.reasoning_per_million - 0.0).abs() < 1e-12);
}
