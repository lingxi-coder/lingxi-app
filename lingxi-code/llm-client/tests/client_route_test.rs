use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, LlmError, LlmRequest,
    ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, ResponseFormat,
};
use llm_client::client::DefaultLlmClient;

#[tokio::test]
async fn client_builds_routes_from_config_and_lists_models() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
        }],
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    assert_eq!(client.available_models().len(), 1);
    assert!(client.prepare(&LlmRequest::new("fast")).await.is_ok());
}

#[tokio::test]
async fn prepare_returns_route_identity_and_encodes_resolved_request_model() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
        }],
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    let prepared = client.prepare(&LlmRequest::new("fast")).await.unwrap();

    assert_eq!(prepared.route.resolved_route.profile_name, "openai");
    assert_eq!(prepared.provider_request.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(prepared.provider_request.body_json["model"], "gpt-4o");
}

#[test]
fn duplicate_profile_names_are_rejected_during_client_construction() {
    let config = ClientConfig {
        providers: vec![
            ProviderProfile {
                provider_id: ProviderId::OpenAI,
                profile_name: "shared".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                protocol: ProtocolFamily::OpenAiChat,
                auth: AuthStrategy::Bearer,
                credential: CredentialConfig::None,
                models: vec![ModelProfile {
                    display_model: "GPT-4o".to_string(),
                    request_model: "gpt-4o".to_string(),
                    billing_model: "gpt-4o".to_string(),
                    aliases: vec!["fast".to_string()],
                    capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
                }],
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
            },
            ProviderProfile {
                provider_id: ProviderId::AnthropicFirstParty,
                profile_name: "shared".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
                protocol: ProtocolFamily::AnthropicMessages,
                auth: AuthStrategy::Bearer,
                credential: CredentialConfig::None,
                models: vec![ModelProfile {
                    display_model: "Claude".to_string(),
                    request_model: "claude-sonnet-4-20250514".to_string(),
                    billing_model: "claude-sonnet-4-20250514".to_string(),
                    aliases: vec![],
                    capabilities: Capabilities { streaming: true, tools: true, ..Default::default() },
                }],
                pricing: PricingConfig::default(),
                signing: None,
                azure: None,
            },
        ],
    };

    let err = DefaultLlmClient::from_config(config).unwrap_err();
    assert!(matches!(err, LlmError::InvalidRequest { .. }));
}

#[test]
fn unsupported_protocol_family_yields_actionable_config_error() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::BedrockClaude,
            profile_name: "bedrock-us".to_string(),
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
            protocol: ProtocolFamily::BedrockClaude,
            auth: AuthStrategy::AwsSigV4,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "Claude".to_string(),
                request_model: "anthropic.claude-sonnet-4".to_string(),
                billing_model: "claude-sonnet-4".to_string(),
                aliases: vec![],
                capabilities: Capabilities::default(),
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
        }],
    };

    let err = DefaultLlmClient::from_config(config).unwrap_err();

    assert!(matches!(
        err,
        LlmError::InvalidRequest { message }
            if message.contains("bedrock-us") && message.contains("BedrockClaude")
    ));
}

#[tokio::test]
async fn response_format_is_rejected_when_selected_model_lacks_structured_output() {
    let config = ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::OpenAI,
            profile_name: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            protocol: ProtocolFamily::OpenAiChat,
            auth: AuthStrategy::Bearer,
            credential: CredentialConfig::None,
            models: vec![ModelProfile {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                billing_model: "gpt-4o".to_string(),
                aliases: vec!["fast".to_string()],
                capabilities: Capabilities { streaming: true, tools: true, structured_output: false, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
            signing: None,
            azure: None,
        }],
    };

    let client = DefaultLlmClient::from_config(config).unwrap();
    let mut request = LlmRequest::new("fast");
    request.response_format = Some(ResponseFormat::JsonObject);

    let err = client.prepare(&request).await.unwrap_err();
    assert!(matches!(err, LlmError::UnsupportedCapability { capability } if capability == "structured_output"));
}
