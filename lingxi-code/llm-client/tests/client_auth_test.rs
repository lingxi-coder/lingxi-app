use std::sync::Arc;

use llm_client::client::DefaultLlmClient;
use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, Credential, CredentialConfig, CredentialProvider,
    CredentialScope, LlmError, LlmRequest, ModelProfile, PricingConfig, ProtocolFamily,
    ProviderId, ProviderProfile,
};
use llm_client::BoxFuture;

fn profile(
    provider_id: ProviderId,
    protocol: ProtocolFamily,
    base_url: &str,
    auth: AuthStrategy,
    credential: CredentialConfig,
) -> ProviderProfile {
    ProviderProfile {
        provider_id,
        profile_name: "p".to_string(),
        base_url: base_url.to_string(),
        protocol,
        auth,
        credential,
        models: vec![ModelProfile {
            display_model: "p-model".to_string(),
            request_model: "p-model".to_string(),
            billing_model: "p-model".to_string(),
            aliases: vec![],
            capabilities: Capabilities {
                streaming: true,
                tools: true,
                ..Default::default()
            },
        }],
        pricing: PricingConfig::default(),
    }
}

fn client_with(
    provider_id: ProviderId,
    protocol: ProtocolFamily,
    base_url: &str,
    auth: AuthStrategy,
    credential: CredentialConfig,
) -> DefaultLlmClient {
    DefaultLlmClient::from_config(ClientConfig {
        providers: vec![profile(provider_id, protocol, base_url, auth, credential)],
    })
    .expect("client")
}

#[tokio::test]
async fn api_key_strategy_uses_provider_specific_headers() {
    std::env::set_var("LLM_CLIENT_AUTH_TEST_ANTHROPIC", "anthropic-secret");
    let client = client_with(
        ProviderId::AnthropicFirstParty,
        ProtocolFamily::AnthropicMessages,
        "https://api.anthropic.com",
        AuthStrategy::ApiKey,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_ANTHROPIC".to_string() },
    );
    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");
    assert_eq!(
        prepared.provider_request.headers.get("x-api-key").map(String::as_str),
        Some("anthropic-secret")
    );

    std::env::set_var("LLM_CLIENT_AUTH_TEST_GEMINI", "gemini-secret");
    let client = client_with(
        ProviderId::Gemini,
        ProtocolFamily::GeminiGenerateContent,
        "https://generativelanguage.googleapis.com/v1beta",
        AuthStrategy::ApiKey,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_GEMINI".to_string() },
    );
    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");
    assert_eq!(
        prepared.provider_request.headers.get("x-goog-api-key").map(String::as_str),
        Some("gemini-secret")
    );

    std::env::set_var("LLM_CLIENT_AUTH_TEST_OPENAI", "openai-secret");
    let client = client_with(
        ProviderId::OpenAI,
        ProtocolFamily::OpenAiChat,
        "https://api.openai.com/v1",
        AuthStrategy::ApiKey,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_OPENAI".to_string() },
    );
    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");
    assert_eq!(
        prepared.provider_request.headers.get("Authorization").map(String::as_str),
        Some("Bearer openai-secret")
    );
}

#[tokio::test]
async fn oauth_bearer_on_anthropic_adds_oauth_beta_header() {
    std::env::set_var("LLM_CLIENT_AUTH_TEST_OAUTH", "oauth-token");
    let client = client_with(
        ProviderId::AnthropicFirstParty,
        ProtocolFamily::AnthropicMessages,
        "https://api.anthropic.com",
        AuthStrategy::OAuthBearer,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_OAUTH".to_string() },
    );

    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");

    assert_eq!(
        prepared.provider_request.headers.get("Authorization").map(String::as_str),
        Some("Bearer oauth-token")
    );
    assert_eq!(
        prepared.provider_request.headers.get("anthropic-beta").map(String::as_str),
        Some("oauth-2025-04-20")
    );
}

#[tokio::test]
async fn host_managed_credentials_resolve_through_injected_provider() {
    #[derive(Debug)]
    struct RecordingStore;
    impl CredentialProvider for RecordingStore {
        fn load<'a>(
            &'a self,
            scope: &'a CredentialScope,
        ) -> BoxFuture<'a, Result<Credential, LlmError>> {
            let id = scope.credential_id.as_deref().unwrap_or("missing").to_string();
            Box::pin(async move { Ok(Credential::BearerToken(format!("token-for-{id}"))) })
        }
    }

    let client = DefaultLlmClient::from_config(ClientConfig {
        providers: vec![profile(
            ProviderId::OpenAI,
            ProtocolFamily::OpenAiChat,
            "https://api.openai.com/v1",
            AuthStrategy::Bearer,
            CredentialConfig::HostManaged { id: "team-key".to_string() },
        )],
    })
    .expect("client")
    .with_credential_provider(Arc::new(RecordingStore));

    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");

    assert_eq!(
        prepared.provider_request.headers.get("Authorization").map(String::as_str),
        Some("Bearer token-for-team-key")
    );
}

#[tokio::test]
async fn host_managed_credentials_without_provider_fail_authentication() {
    let client = client_with(
        ProviderId::OpenAI,
        ProtocolFamily::OpenAiChat,
        "https://api.openai.com/v1",
        AuthStrategy::Bearer,
        CredentialConfig::Static { id: "k".to_string() },
    );

    assert!(matches!(
        client.prepare(&LlmRequest::new("p-model")).await.unwrap_err(),
        LlmError::Authentication
    ));
}

#[tokio::test]
async fn unimplemented_signing_strategies_fail_at_prepare() {
    let client = client_with(
        ProviderId::AnthropicFirstParty,
        ProtocolFamily::AnthropicMessages,
        "https://api.anthropic.com",
        AuthStrategy::AwsSigV4,
        CredentialConfig::None,
    );

    assert!(matches!(
        client.prepare(&LlmRequest::new("p-model")).await.unwrap_err(),
        LlmError::InvalidRequest { message } if message.contains("AwsSigV4")
    ));
}

#[tokio::test]
async fn missing_credential_config_sends_request_without_client_auth() {
    let client = client_with(
        ProviderId::OpenAI,
        ProtocolFamily::OpenAiChat,
        "https://api.openai.com/v1",
        AuthStrategy::Bearer,
        CredentialConfig::None,
    );

    let prepared = client.prepare(&LlmRequest::new("p-model")).await.expect("prepare");

    assert!(!prepared.provider_request.headers.contains_key("Authorization"));
    assert!(!prepared.provider_request.headers.contains_key("x-api-key"));
}

#[tokio::test]
async fn prepare_count_tokens_is_authenticated_for_anthropic_routes() {
    std::env::set_var("LLM_CLIENT_AUTH_TEST_CT", "ct-key");
    let client = client_with(
        ProviderId::AnthropicFirstParty,
        ProtocolFamily::AnthropicMessages,
        "https://api.anthropic.com",
        AuthStrategy::ApiKey,
        CredentialConfig::Env { var: "LLM_CLIENT_AUTH_TEST_CT".to_string() },
    );

    let prepared = client
        .prepare_count_tokens(&LlmRequest::new("p-model").with_user_text("hi"))
        .await
        .expect("prepared");

    assert!(prepared.url.ends_with("/v1/messages/count_tokens"));
    assert_eq!(prepared.headers.get("x-api-key").map(String::as_str), Some("ct-key"));
    assert!(prepared.body_json.get("max_tokens").is_none());
}

#[tokio::test]
async fn prepare_count_tokens_rejects_non_anthropic_routes() {
    let client = client_with(
        ProviderId::OpenAI,
        ProtocolFamily::OpenAiChat,
        "https://api.openai.com/v1",
        AuthStrategy::None,
        CredentialConfig::None,
    );

    let err = client
        .prepare_count_tokens(&LlmRequest::new("p-model").with_user_text("hi"))
        .await
        .unwrap_err();

    assert!(matches!(err, LlmError::InvalidRequest { message } if message.contains("count_tokens")));
}
