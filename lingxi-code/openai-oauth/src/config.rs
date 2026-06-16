//! Static OpenAI OAuth endpoints + client_id + scopes + Codex backend URL.
//! Constants verified against codex `login/src/server.rs` and `model-provider-info`.

/// OpenAI OAuth configuration (issuer, client_id, endpoints, scopes, backend).
#[derive(Debug, Clone)]
pub struct OpenAiOAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub authorize_url: String,
    pub token_url: String,
    pub device_usercode_url: String,
    pub device_token_url: String,
    pub device_verify_url: String,
    pub scopes: String,
    pub codex_backend: String,
    /// Loopback redirect ports, in preference order (codex allowlist: 1455, then 1457).
    pub loopback_ports: [u16; 2],
}

impl Default for OpenAiOAuthConfig {
    fn default() -> Self {
        let issuer = "https://auth.openai.com".to_string();
        Self {
            authorize_url: format!("{issuer}/oauth/authorize"),
            token_url: format!("{issuer}/oauth/token"),
            device_usercode_url: format!("{issuer}/api/accounts/deviceauth/usercode"),
            device_token_url: format!("{issuer}/api/accounts/deviceauth/token"),
            device_verify_url: format!("{issuer}/codex/device"),
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_string(),
            scopes: "openid profile email offline_access api.connectors.read api.connectors.invoke"
                .to_string(),
            codex_backend: "https://chatgpt.com/backend-api/codex".to_string(),
            loopback_ports: [1455, 1457],
            issuer,
        }
    }
}

impl OpenAiOAuthConfig {
    /// The loopback redirect URI for a chosen port.
    #[must_use]
    pub fn redirect_uri(&self, port: u16) -> String {
        format!("http://localhost:{port}/auth/callback")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_has_codex_constants() {
        let c = OpenAiOAuthConfig::default();
        assert_eq!(c.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(c.token_url, "https://auth.openai.com/oauth/token");
        assert_eq!(c.authorize_url, "https://auth.openai.com/oauth/authorize");
        assert!(c.scopes.contains("offline_access"));
        assert_eq!(c.codex_backend, "https://chatgpt.com/backend-api/codex");
        assert_eq!(c.loopback_ports, [1455, 1457]);
        assert_eq!(c.redirect_uri(1455), "http://localhost:1455/auth/callback");
    }
}
