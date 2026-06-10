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
