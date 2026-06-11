//! Credential lookup traits and lightweight providers.

use std::fmt;

use crate::{BoxFuture, LlmError, ProviderId};

/// Scope used to resolve provider credentials.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialScope {
    /// Provider identity requesting credentials.
    pub provider_id: ProviderId,
    /// Provider profile name requesting credentials.
    pub profile_name: String,
    /// Host-defined credential id from `CredentialConfig::Static` or
    /// `CredentialConfig::HostManaged`, when one was configured.
    pub credential_id: Option<String>,
}

impl CredentialScope {
    /// Create a credential lookup scope.
    #[must_use]
    pub fn new(provider_id: ProviderId, profile_name: impl Into<String>) -> Self {
        Self {
            provider_id,
            profile_name: profile_name.into(),
            credential_id: None,
        }
    }

    /// Attach the host-defined credential id to the scope.
    #[must_use]
    pub fn with_credential_id(mut self, credential_id: impl Into<String>) -> Self {
        self.credential_id = Some(credential_id.into());
        self
    }
}

/// Secret material loaded by a credential provider.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// Provider API key.
    ApiKey(String),
    /// Bearer or OAuth access token.
    BearerToken(String),
    /// AWS `SigV4` signing credentials.
    ///
    /// For `EnvCredentialProvider`, `SigV4` is not supported — use a
    /// `StaticCredentialProvider` or a host-managed credential store instead
    /// (loading them requires knowing the three fields together, which the
    /// single-env-variable model cannot express).
    AwsSigV4 {
        /// AWS access key ID.
        access_key_id: String,
        /// AWS secret access key.
        secret_access_key: String,
        /// Optional session token (for temporary credentials / STS).
        session_token: Option<String>,
    },
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => formatter.debug_tuple("ApiKey").field(&"[REDACTED]").finish(),
            Self::BearerToken(_) => formatter
                .debug_tuple("BearerToken")
                .field(&"[REDACTED]")
                .finish(),
            Self::AwsSigV4 { .. } => formatter
                .debug_struct("AwsSigV4")
                .field("access_key_id", &"[REDACTED]")
                .field("secret_access_key", &"[REDACTED]")
                .field("session_token", &"[REDACTED]")
                .finish(),
        }
    }
}

/// Loads credentials for a provider/profile scope.
///
/// `load` is async so implementations can refresh expiring material
/// (e.g. OAuth) inside the lookup.
pub trait CredentialProvider: fmt::Debug + Send + Sync {
    /// Load credential material for a scope.
    fn load<'a>(
        &'a self,
        scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>>;
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
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let credential = self.credential.clone();
        Box::pin(async move { Ok(credential) })
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
    fn load<'a>(
        &'a self,
        _scope: &'a CredentialScope,
    ) -> BoxFuture<'a, Result<Credential, LlmError>> {
        let result = std::env::var(&self.variable_name)
            .map(Credential::ApiKey)
            .map_err(|_| LlmError::Authentication);
        Box::pin(async move { result })
    }
}
