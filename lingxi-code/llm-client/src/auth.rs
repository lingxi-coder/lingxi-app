//! Request-aware authenticators.

use crate::{LlmError, ProviderRequest};

/// Applies credentials to an encoded provider request.
pub trait Authenticator {
    /// Return the request with authentication applied.
    fn apply(&self, request: ProviderRequest) -> Result<ProviderRequest, LlmError>;
}

/// Authenticator that inserts an API-key header (`x-api-key` by default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAuthenticator {
    header_name: String,
    api_key: String,
}

impl ApiKeyAuthenticator {
    /// Create an API-key authenticator using the `x-api-key` header.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_header_name("x-api-key", api_key)
    }

    /// Create an API-key authenticator for a provider-specific header name
    /// (e.g. Gemini's `x-goog-api-key`).
    #[must_use]
    pub fn with_header_name(header_name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            header_name: header_name.into(),
            api_key: api_key.into(),
        }
    }
}

impl Authenticator for ApiKeyAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        request
            .headers
            .insert(self.header_name.clone(), self.api_key.clone());
        Ok(request)
    }
}

/// Authenticator that inserts an `Authorization: Bearer` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerAuthenticator {
    token: String,
}

impl BearerAuthenticator {
    /// Create a bearer-token authenticator.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

impl Authenticator for BearerAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        request.headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", self.token),
        );
        Ok(request)
    }
}

/// Authenticator for ChatGPT-account OAuth: `Authorization: Bearer` plus the
/// `ChatGPT-Account-ID` header (and `X-OpenAI-Fedramp` when set). Mirrors codex
/// `model-provider/src/bearer_auth_provider.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptAuthenticator {
    token: String,
    account_id: Option<String>,
    fedramp: bool,
}

impl ChatGptAuthenticator {
    /// Create a ChatGPT OAuth authenticator.
    #[must_use]
    pub fn new(token: impl Into<String>, account_id: Option<String>, fedramp: bool) -> Self {
        Self { token: token.into(), account_id, fedramp }
    }
}

impl Authenticator for ChatGptAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        let headers = &mut request.headers;
        headers.remove("x-api-key");
        headers.insert("Authorization".to_string(), format!("Bearer {}", self.token));
        if let Some(acc) = &self.account_id {
            headers.insert("ChatGPT-Account-ID".to_string(), acc.clone());
        }
        if self.fedramp {
            headers.insert("X-OpenAI-Fedramp".to_string(), "true".to_string());
        }
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatgpt_authenticator_sets_bearer_and_account_header() {
        let auth = ChatGptAuthenticator::new("tok-123", Some("acc_9".to_string()), false);
        let req = auth.apply(ProviderRequest::post_json("https://x/responses", serde_json::json!({}))).unwrap();
        assert_eq!(req.headers.get("Authorization").map(String::as_str), Some("Bearer tok-123"));
        assert_eq!(req.headers.get("ChatGPT-Account-ID").map(String::as_str), Some("acc_9"));
        assert!(!req.headers.contains_key("X-OpenAI-Fedramp"));
    }

    #[test]
    fn chatgpt_authenticator_sets_fedramp_when_flagged() {
        let auth = ChatGptAuthenticator::new("t", None, true);
        let req = auth.apply(ProviderRequest::post_json("https://x/responses", serde_json::json!({}))).unwrap();
        assert_eq!(req.headers.get("X-OpenAI-Fedramp").map(String::as_str), Some("true"));
        assert!(!req.headers.contains_key("ChatGPT-Account-ID"));
    }
}
