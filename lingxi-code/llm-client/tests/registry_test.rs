use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, ModelRegistry,
    PricingConfig, ProviderId, ProviderProfile, ProtocolFamily,
};

fn test_config() -> ClientConfig {
    ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAICompatible {
                name: "openrouter".to_string(),
            },
            profile_name: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::Env {
                var: "OPENROUTER_API_KEY".to_string(),
            },
            models: vec![ModelProfile {
                display_model: "Claude via OpenRouter".to_string(),
                request_model: "anthropic/claude-sonnet-4".to_string(),
                billing_model: "claude-sonnet-4".to_string(),
                aliases: vec!["or-sonnet".to_string()],
                capabilities: Capabilities {
                    streaming: true,
                    tools: true,
                    vision: false,
                    documents: false,
                    reasoning: false,
                    structured_output: true,
                },
            }],
            pricing: PricingConfig::default(),
        }],
    }
}

#[test]
fn available_models_lists_configured_profile_models_and_aliases() {
    let registry = ModelRegistry::from_config(test_config()).expect("registry");

    let listings = registry.available_models();

    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].profile_name, "openrouter");
    assert_eq!(listings[0].display_model, "Claude via OpenRouter");
    assert_eq!(listings[0].request_model, "anthropic/claude-sonnet-4");
    assert_eq!(listings[0].billing_model, "claude-sonnet-4");
    assert_eq!(listings[0].aliases, vec!["or-sonnet"]);
    assert!(listings[0].capabilities.streaming);
}

#[test]
fn resolve_rejects_ambiguous_model_references() {
    let mut config = test_config();
    let mut second = config.providers[0].clone();
    second.profile_name = "fallback-gateway".to_string();
    second.models[0].aliases = vec![];
    config.providers.push(second);
    let registry = ModelRegistry::from_config(config).expect("registry");

    let err = registry
        .resolve("anthropic/claude-sonnet-4")
        .expect_err("shared request_model must be ambiguous");
    assert!(matches!(
        err,
        llm_client::LlmError::InvalidRequest { message }
            if message.contains("openrouter") && message.contains("fallback-gateway")
    ));

    let route = registry.resolve("or-sonnet").expect("unique alias still resolves");
    assert_eq!(route.profile_name, "openrouter");
}

#[test]
fn resolve_uses_alias_without_model_string_provider_guessing() {
    let registry = ModelRegistry::from_config(test_config()).expect("registry");

    let route = registry.resolve("or-sonnet").expect("route");

    assert_eq!(route.profile_name, "openrouter");
    assert_eq!(route.request_model, "anthropic/claude-sonnet-4");
    assert_eq!(route.pricing_model.billing_model, "claude-sonnet-4");
    assert_eq!(
        route.pricing_model.pricing_provider_id,
        ProviderId::OpenAICompatible {
            name: "openrouter".to_string(),
        }
    );
}
