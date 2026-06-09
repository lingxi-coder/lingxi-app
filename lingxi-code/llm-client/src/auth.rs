//! Request-aware authenticators.

use crate::{LlmError, PreparedRequest};

/// Applies credentials to a fully prepared request.
pub trait Authenticator {
    /// Return the request with authentication applied.
    fn apply(&self, request: PreparedRequest) -> Result<PreparedRequest, LlmError>;
}

/// Authenticator that inserts an `x-api-key` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAuthenticator {
    api_key: String,
}

impl ApiKeyAuthenticator {
    /// Create an API-key authenticator.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

impl Authenticator for ApiKeyAuthenticator {
    fn apply(&self, mut request: PreparedRequest) -> Result<PreparedRequest, LlmError> {
        request
            .headers
            .insert("x-api-key".to_string(), self.api_key.clone());
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
    fn apply(&self, mut request: PreparedRequest) -> Result<PreparedRequest, LlmError> {
        request.headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", self.token),
        );
        Ok(request)
    }
}
