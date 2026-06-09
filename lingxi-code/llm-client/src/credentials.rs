//! Credential lookup traits and lightweight providers.

use std::fmt;

use crate::{LlmError, ProviderId};

/// Scope used to resolve provider credentials.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialScope {
    /// Provider identity requesting credentials.
    pub provider_id: ProviderId,
    /// Provider profile name requesting credentials.
    pub profile_name: String,
}

impl CredentialScope {
    /// Create a credential lookup scope.
    #[must_use]
    pub fn new(provider_id: ProviderId, profile_name: impl Into<String>) -> Self {
        Self {
            provider_id,
            profile_name: profile_name.into(),
        }
    }
}

/// Secret material loaded by a credential provider.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// Provider API key.
    ApiKey(String),
    /// Bearer or OAuth access token.
    BearerToken(String),
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => formatter.debug_tuple("ApiKey").field(&"[REDACTED]").finish(),
            Self::BearerToken(_) => formatter
                .debug_tuple("BearerToken")
                .field(&"[REDACTED]")
                .finish(),
        }
    }
}

/// Loads credentials for a provider/profile scope.
pub trait CredentialProvider {
    /// Load credential material for a scope.
    fn load(&self, scope: &CredentialScope) -> Result<Credential, LlmError>;
}

/// Credential provider backed by one static credential.
#[derive(Debug, Clone)]
pub struct StaticCredentialProvider {
    credential: Credential,
}

impl StaticCredentialProvider {
    /// Create a provider returning the same credential for every scope.
    #[must_use]
    pub fn new(credential: Credential) -> Self {
        Self { credential }
    }
}

impl CredentialProvider for StaticCredentialProvider {
    fn load(&self, _scope: &CredentialScope) -> Result<Credential, LlmError> {
        Ok(self.credential.clone())
    }
}

/// Credential provider backed by an environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCredentialProvider {
    variable_name: String,
}

impl EnvCredentialProvider {
    /// Create a provider that reads an API key from an environment variable.
    #[must_use]
    pub fn new(variable_name: impl Into<String>) -> Self {
        Self {
            variable_name: variable_name.into(),
        }
    }
}

impl CredentialProvider for EnvCredentialProvider {
    fn load(&self, _scope: &CredentialScope) -> Result<Credential, LlmError> {
        std::env::var(&self.variable_name)
            .map(Credential::ApiKey)
            .map_err(|_| LlmError::Authentication)
    }
}
