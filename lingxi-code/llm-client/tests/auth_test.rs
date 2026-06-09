use std::collections::BTreeMap;

use llm_client::{
    ApiKeyAuthenticator, Authenticator, BearerAuthenticator, Credential, CredentialProvider,
    CredentialScope, EnvCredentialProvider, PreparedRequest, ProviderId, StaticCredentialProvider,
};

#[test]
fn env_credential_provider_loads_secret_for_scope() {
    std::env::set_var("LLM_CLIENT_TEST_API_KEY", "test-key");
    let provider = EnvCredentialProvider::new("LLM_CLIENT_TEST_API_KEY");

    let credential = provider
        .load(&CredentialScope::new(
            ProviderId::AnthropicFirstParty,
            "anthropic",
        ))
        .expect("credential");

    assert_eq!(credential, Credential::ApiKey("test-key".to_string()));
}

#[test]
fn static_credential_debug_is_redacted() {
    let provider = StaticCredentialProvider::new(Credential::BearerToken("secret-token".to_string()));

    let debug = format!("{:?}", provider.load(&CredentialScope::new(ProviderId::OpenAI, "openai")));

    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("secret-token"));
}

#[test]
fn api_key_authenticator_applies_header_to_prepared_request() {
    let request = PreparedRequest::new("https://api.anthropic.com/v1/messages")
        .with_body(br#"{"model":"claude"}"#.to_vec());
    let auth = ApiKeyAuthenticator::new("test-key");

    let signed = auth.apply(request).expect("signed request");

    assert_eq!(signed.headers.get("x-api-key"), Some(&"test-key".to_string()));
    assert_eq!(signed.body, br#"{"model":"claude"}"#.to_vec());
}

#[test]
fn bearer_authenticator_applies_authorization_header_without_removing_existing_headers() {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    let request = PreparedRequest {
        url: "https://api.openai.com/v1/chat/completions".to_string(),
        headers,
        body: Vec::new(),
    };
    let auth = BearerAuthenticator::new("test-token");

    let signed = auth.apply(request).expect("signed request");

    assert_eq!(
        signed.headers.get("Authorization"),
        Some(&"Bearer test-token".to_string())
    );
    assert_eq!(
        signed.headers.get("content-type"),
        Some(&"application/json".to_string())
    );
}
