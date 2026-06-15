//! Synchronous GitHub Copilot request authenticator + redacting token wrapper.
//!
//! opencode parity: the GitHub OAuth-App token is used DIRECTLY as the bearer
//! credential (no `copilot_internal/v2/token` exchange), so authentication is
//! pure header injection and fits the synchronous [`Authenticator`] trait.

use crate::{Authenticator, LlmError, ProviderRequest};

/// `X-GitHub-Api-Version` header value sent to GitHub Copilot.
pub const COPILOT_API_VERSION: &str = "2026-06-01";
/// `User-Agent` sent to GitHub Copilot.
pub const COPILOT_USER_AGENT: &str = "LingXi-Code";

/// GitHub OAuth token used directly as the Copilot bearer credential.
///
/// The `Debug` impl is redacting so the token never reaches logs or errors.
#[derive(Clone)]
pub struct CopilotSecret(String);

impl CopilotSecret {
    /// Wrap a GitHub OAuth token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// **Plan 3c frozen-crate (§10) EXCEPTION — documented deviation.** Expose the
    /// raw GitHub OAuth token so the host `/connect` device-flow driver can persist
    /// it to the keychain under `github-copilot`. This is the ONLY reader; the
    /// `Debug` impl stays redacting. `#[doc(hidden)]` so it is not part of the
    /// public surface and is only reachable by the engine that already drives the
    /// Copilot login. Do not use for logging or display.
    #[doc(hidden)]
    #[must_use]
    pub fn token_for_storage(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CopilotSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CopilotSecret(<redacted>)")
    }
}

/// Authenticator for GitHub Copilot's OpenAI-compatible endpoint. Injects the
/// Copilot header set and uses the GitHub OAuth token directly as the bearer.
#[derive(Debug, Clone)]
pub struct CopilotAuthenticator {
    secret: CopilotSecret,
}

impl CopilotAuthenticator {
    /// Create an authenticator from a GitHub OAuth token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            secret: CopilotSecret::new(token),
        }
    }
}

impl Authenticator for CopilotAuthenticator {
    fn apply(&self, mut request: ProviderRequest) -> Result<ProviderRequest, LlmError> {
        let headers = &mut request.headers;
        // Defensive parity with opencode: never let an x-api-key ride along.
        headers.remove("x-api-key");
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", self.secret.0),
        );
        headers.insert("User-Agent".to_string(), COPILOT_USER_AGENT.to_string());
        headers.insert("Openai-Intent".to_string(), "conversation-edits".to_string());
        headers.insert(
            "X-GitHub-Api-Version".to_string(),
            COPILOT_API_VERSION.to_string(),
        );
        // LingXi is an agentic client; per-request user/agent refinement is deferred.
        headers.insert("x-initiator".to_string(), "agent".to_string());
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn injects_copilot_headers_and_strips_x_api_key() {
        let mut request = ProviderRequest::post_json(
            "https://api.githubcopilot.com/chat/completions",
            json!({ "model": "gpt-5.4-nano" }),
        );
        request
            .headers
            .insert("x-api-key".to_string(), "leftover".to_string());

        let signed = CopilotAuthenticator::new("ght_token")
            .apply(request)
            .expect("applies");

        assert_eq!(
            signed.headers.get("Authorization"),
            Some(&"Bearer ght_token".to_string())
        );
        assert_eq!(
            signed.headers.get("X-GitHub-Api-Version"),
            Some(&"2026-06-01".to_string())
        );
        assert_eq!(
            signed.headers.get("Openai-Intent"),
            Some(&"conversation-edits".to_string())
        );
        assert_eq!(
            signed.headers.get("User-Agent"),
            Some(&"LingXi-Code".to_string())
        );
        assert_eq!(signed.headers.get("x-initiator"), Some(&"agent".to_string()));
        assert!(!signed.headers.contains_key("x-api-key"));
    }

    #[test]
    fn token_for_storage_returns_raw_token_for_persistence() {
        // Frozen-crate (§10) exception: the /connect device-flow MUST persist the
        // GitHub token under `github-copilot`. The Debug stays redacting; only
        // this explicit, doc-hidden accessor exposes the raw token.
        let s = CopilotSecret::new("ght_live_token");
        assert_eq!(s.token_for_storage(), "ght_live_token");
        // Debug is still redacting (no regression).
        assert!(!format!("{s:?}").contains("ght_live_token"));
    }

    #[test]
    fn debug_does_not_leak_token() {
        let dbg_secret = format!("{:?}", CopilotSecret::new("supersecret"));
        assert!(!dbg_secret.contains("supersecret"));
        let dbg_auth = format!("{:?}", CopilotAuthenticator::new("supersecret"));
        assert!(!dbg_auth.contains("supersecret"));
    }
}
