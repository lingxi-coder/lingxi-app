//! Anthropic auth-source resolver.
//!
//! See spec §30.2 / A6. Inspects environment, stored credentials, and
//! settings to choose exactly one of the documented auth sources. Order is
//! deterministic and intentionally favours managed-context overrides.

#![allow(clippy::struct_excessive_bools)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One of the nine documented Anthropic auth sources (plus [`AuthSource::None`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthSource {
    /// Highest priority: managed contexts force OAuth.
    OAuthClaudeAi,
    /// Environment variable `ANTHROPIC_AUTH_TOKEN` (bearer).
    EnvAuthToken,
    /// Environment variable `ANTHROPIC_API_KEY`.
    EnvApiKey,
    /// File-descriptor inheritance (managed launch).
    FileDescriptor,
    /// API key stored via `SecureStorage`.
    StoredApiKey,
    /// Legacy: `settings.json` `apiKey` field.
    SettingsApiKey,
    /// External script that returns an API key on stdout.
    ApiKeyHelper {
        /// Path to the helper script invoked to fetch the key.
        script_path: PathBuf,
    },
    /// AWS Bedrock credentials in the ambient environment.
    AwsBedrock,
    /// No usable source found.
    None,
}

/// Inputs the resolver inspects to pick an [`AuthSource`].
///
/// Built once during platform init; passed by reference to [`resolve`].
pub struct ResolverContext {
    /// `true` when running under a managed context (CCR / Claude Desktop)
    /// that must force OAuth even if other sources are present.
    pub managed_oauth_only: bool,
    /// Captured value of `ANTHROPIC_AUTH_TOKEN`, if set.
    pub env_auth_token: Option<String>,
    /// Captured value of `ANTHROPIC_API_KEY`, if set.
    pub env_api_key: Option<String>,
    /// `true` when the launcher inherited an API key via a file descriptor.
    pub fd_present: bool,
    /// `true` when an OAuth token is present in `SecureStorage`.
    pub has_stored_oauth: bool,
    /// `true` when a raw API key is present in `SecureStorage`.
    pub has_stored_api_key: bool,
    /// API key surfaced by `settings.json` (legacy path).
    pub settings_api_key: Option<String>,
    /// Path to a configured `apiKeyHelper` script, if any.
    pub api_key_helper_script: Option<PathBuf>,
    /// `true` when AWS Bedrock credentials are present.
    pub aws_present: bool,
}

/// Resolve the highest-priority auth source from the supplied context.
///
/// Order: managed OAuth → env auth token → env API key → FD → stored OAuth →
/// stored API key → settings → helper script → AWS Bedrock → [`AuthSource::None`].
#[must_use]
pub fn resolve(ctx: &ResolverContext) -> AuthSource {
    if ctx.managed_oauth_only && ctx.has_stored_oauth {
        return AuthSource::OAuthClaudeAi;
    }
    if ctx.env_auth_token.is_some() {
        return AuthSource::EnvAuthToken;
    }
    if ctx.env_api_key.is_some() {
        return AuthSource::EnvApiKey;
    }
    if ctx.fd_present {
        return AuthSource::FileDescriptor;
    }
    if ctx.has_stored_oauth {
        return AuthSource::OAuthClaudeAi;
    }
    if ctx.has_stored_api_key {
        return AuthSource::StoredApiKey;
    }
    if ctx.settings_api_key.is_some() {
        return AuthSource::SettingsApiKey;
    }
    if let Some(p) = &ctx.api_key_helper_script {
        return AuthSource::ApiKeyHelper {
            script_path: p.clone(),
        };
    }
    if ctx.aws_present {
        return AuthSource::AwsBedrock;
    }
    AuthSource::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_oauth_wins() {
        let ctx = ResolverContext {
            managed_oauth_only: true,
            env_auth_token: Some("x".into()),
            env_api_key: None,
            fd_present: false,
            has_stored_oauth: true,
            has_stored_api_key: false,
            settings_api_key: None,
            api_key_helper_script: None,
            aws_present: false,
        };
        assert!(matches!(resolve(&ctx), AuthSource::OAuthClaudeAi));
    }

    #[test]
    fn none_when_no_source() {
        let ctx = ResolverContext {
            managed_oauth_only: false,
            env_auth_token: None,
            env_api_key: None,
            fd_present: false,
            has_stored_oauth: false,
            has_stored_api_key: false,
            settings_api_key: None,
            api_key_helper_script: None,
            aws_present: false,
        };
        assert!(matches!(resolve(&ctx), AuthSource::None));
    }
}
