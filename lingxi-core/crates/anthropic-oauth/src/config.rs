//! Static configuration for the Claude.ai OAuth client.
//!
//! See spec §30.1. Endpoints mirror the canonical claude-code reference; the
//! redirect URI is a `http://127.0.0.1:{port}/callback` loopback that the
//! caller binds before opening the browser.

use serde::{Deserialize, Serialize};

/// Endpoint and identifier set for the Authorization Code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeAiOAuthConfig {
    /// Browser-facing authorization endpoint.
    pub authorization_endpoint: String,
    /// Backchannel token-exchange / refresh endpoint.
    pub token_endpoint: String,
    /// Backchannel revocation endpoint.
    pub revocation_endpoint: String,
    /// Profile endpoint used to fetch the authenticated user record.
    pub profile_endpoint: String,
    /// OAuth client ID registered with login.claude.ai.
    pub client_id: String,
    /// Loopback redirect URI registered for the local CLI flow.
    pub redirect_uri: String,
    /// Scopes requested in the authorize URL.
    pub scopes: Vec<String>,
}

/// Spec §7 line 720 — claude-code's authoritative scope list.
///
/// Order is preserved when joined into the `scope=` parameter; tests assert
/// the exact `read:user write:messages read:projects` byte sequence.
pub const CLAUDE_CODE_OAUTH_SCOPES: &[&str] = &["read:user", "write:messages", "read:projects"];

/// Spec §7 line 715 — refresh `grant_type`. Locked byte-for-byte.
pub const REFRESH_GRANT_TYPE: &str = "refresh_token";

impl ClaudeAiOAuthConfig {
    /// Default config — endpoints per claude-code reference, redirect URI
    /// targeted at a caller-supplied loopback `port`.
    #[must_use]
    pub fn default_with_port(port: u16) -> Self {
        Self {
            authorization_endpoint: "https://claude.ai/oauth/authorize".into(),
            token_endpoint: "https://console.anthropic.com/v1/oauth/token".into(),
            revocation_endpoint: "https://console.anthropic.com/v1/oauth/revoke".into(),
            profile_endpoint: "https://api.claude.ai/v1/me".into(),
            client_id: "lingxi-core".into(),
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
            scopes: CLAUDE_CODE_OAUTH_SCOPES.iter().map(|s| (*s).into()).collect(),
        }
    }
}
