//! Shared built-in Anthropic `ClientConfig` for `DefaultLlmClient`.
//!
//! Both `engine-desktop` and `engine-mobile` need an identical 10-entry
//! Claude-4-generation model table. Keeping it here (a crate both hosts already
//! depend on) eliminates the duplicate that previously lived in each host and
//! ensures they can never silently drift.
//!
//! ## Model table (`display_model` → `billing_model`)
//!
//! | `display_model`              | `billing_model`      | reasoning |
//! |------------------------------|----------------------|-----------|
//! | claude-sonnet-4-20250514     | claude-sonnet-4      | false     |
//! | claude-sonnet-4-5-20250929   | claude-sonnet-4-5    | false     |
//! | claude-sonnet-4-6            | claude-sonnet-4-6    | false     |
//! | claude-opus-4-20250514       | claude-opus-4        | true      |
//! | claude-opus-4-1-20250805     | claude-opus-4-1      | true      |
//! | claude-opus-4-5-20251101     | claude-opus-4-5      | true      |
//! | claude-opus-4-6              | claude-opus-4-6      | true      |
//! | claude-opus-4-7              | claude-opus-4-7      | true      |  ← orchestrator DEFAULT_MODEL
//! | claude-haiku-4-20250307      | claude-haiku-4       | false     |
//! | claude-haiku-4-5             | claude-haiku-4-5     | false     |
//!
//! ## Auth strategies
//!
//! `oauth_path = false` → `ApiKey` + `CredentialConfig::Env { var: "ANTHROPIC_API_KEY" }`
//! `oauth_path = true`  → `OAuthBearer` + `CredentialConfig::HostManaged { id: "anthropic_oauth" }`
//!
//! All models get streaming, tools, vision, and documents; Opus variants
//! additionally get `reasoning: true`.

use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, ModelProfile, PricingConfig,
    ProtocolFamily, ProviderId, ProviderProfile,
};

/// Build the built-in Anthropic [`ClientConfig`] for [`llm_client::DefaultLlmClient`].
///
/// One [`ProviderProfile`] with the full Claude-4-generation model table (10
/// entries). See module-level docs for the complete table and auth strategy
/// description.
///
/// **3c note:** `modelProviders` settings merging (multi-provider routing) is
/// deferred to Plan 3c; this profile covers the default Anthropic-only path.
#[must_use]
pub fn builtin_anthropic_config(api_base: &str, oauth_path: bool) -> ClientConfig {
    fn model(display: &str, billing: &str, aliases: &[&str], reasoning: bool) -> ModelProfile {
        ModelProfile {
            display_model: display.to_string(),
            request_model: display.to_string(),
            billing_model: billing.to_string(),
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            capabilities: Capabilities {
                streaming: true,
                tools: true,
                vision: true,
                documents: true,
                reasoning,
                structured_output: false,
            },
        }
    }

    let (auth, credential) = if oauth_path {
        (
            AuthStrategy::OAuthBearer,
            CredentialConfig::HostManaged { id: "anthropic_oauth".to_string() },
        )
    } else {
        (
            AuthStrategy::ApiKey,
            CredentialConfig::Env { var: "ANTHROPIC_API_KEY".to_string() },
        )
    };

    ClientConfig {
        providers: vec![ProviderProfile {
            provider_id: ProviderId::AnthropicFirstParty,
            profile_name: "anthropic".to_string(),
            base_url: api_base.to_string(),
            protocol: ProtocolFamily::AnthropicMessages,
            auth,
            credential,
            pricing: PricingConfig::default(),
            models: vec![
                // — Claude Sonnet 4 (default engine model) —
                model(
                    "claude-sonnet-4-20250514",
                    "claude-sonnet-4",
                    &["claude-sonnet-4", "claude-sonnet", "claude"],
                    false,
                ),
                // — Claude Sonnet 4.5 —
                model(
                    "claude-sonnet-4-5-20250929",
                    "claude-sonnet-4-5",
                    &["claude-sonnet-4-5"],
                    false,
                ),
                // — Claude Sonnet 4.6 —
                model("claude-sonnet-4-6", "claude-sonnet-4-6", &[], false),
                // — Claude Opus 4 (Opus-fallback gate target) —
                model(
                    "claude-opus-4-20250514",
                    "claude-opus-4",
                    &["claude-opus-4", "claude-opus"],
                    true,
                ),
                // — Claude Opus 4.1 —
                model(
                    "claude-opus-4-1-20250805",
                    "claude-opus-4-1",
                    &["claude-opus-4-1"],
                    true,
                ),
                // — Claude Opus 4.5 —
                model(
                    "claude-opus-4-5-20251101",
                    "claude-opus-4-5",
                    &["claude-opus-4-5"],
                    true,
                ),
                // — Claude Opus 4.6 —
                model("claude-opus-4-6", "claude-opus-4-6", &[], true),
                // — Claude Opus 4.7 (orchestrator DEFAULT_MODEL; orchestrator/src/config.rs:18) —
                model("claude-opus-4-7", "claude-opus-4-7", &[], true),
                // — Claude Haiku 4 —
                model(
                    "claude-haiku-4-20250307",
                    "claude-haiku-4",
                    &["claude-haiku-4", "claude-haiku"],
                    false,
                ),
                // — Claude Haiku 4.5 —
                model("claude-haiku-4-5", "claude-haiku-4-5", &[], false),
            ],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::{DefaultLlmClient, LlmError};

    /// Build a test config with `AuthStrategy::None` + `CredentialConfig::None`
    /// so `prepare()` never attempts a credential lookup (no env var needed).
    fn test_config(api_base: &str) -> ClientConfig {
        let mut cfg = builtin_anthropic_config(api_base, false);
        // Swap auth+credential to None so prepare() is credential-free in tests.
        for p in &mut cfg.providers {
            p.auth = AuthStrategy::None;
            p.credential = CredentialConfig::None;
        }
        cfg
    }

    /// Every `display_model` in the builtin table must be resolvable via
    /// `available_models()`. This pins the complete 10-entry table so a
    /// future edit that accidentally drops a model is caught immediately.
    #[test]
    fn all_table_entries_resolvable() {
        let cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let client = DefaultLlmClient::from_config(cfg).expect("config must be valid");
        let available: Vec<String> = client
            .available_models()
            .into_iter()
            .map(|m| m.display_model)
            .collect();

        let expected = [
            "claude-sonnet-4-20250514",
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-6",
            "claude-opus-4-20250514",
            "claude-opus-4-1-20250805",
            "claude-opus-4-5-20251101",
            "claude-opus-4-6",
            // orchestrator DEFAULT_MODEL (config.rs:18) — was absent before this fix
            "claude-opus-4-7",
            "claude-haiku-4-20250307",
            "claude-haiku-4-5",
        ];
        for model_id in &expected {
            assert!(
                available.iter().any(|m| m == model_id),
                "model {model_id:?} missing from available_models(); got: {available:?}",
            );
        }
    }

    /// `claude-opus-4-7` (orchestrator DEFAULT_MODEL, config.rs:18) must survive
    /// `prepare()` so the orchestrator's default model actually routes at runtime.
    #[tokio::test]
    async fn default_model_resolves() {
        let cfg = test_config("https://api.anthropic.com");
        let client =
            DefaultLlmClient::from_config(cfg).expect("config must be valid");

        // `prepare()` exercises registry resolution + codec encoding + auth —
        // with AuthStrategy::None it short-circuits before any network call.
        let req = llm_client::LlmRequest::new("claude-opus-4-7");
        let result = client.prepare(&req).await;
        assert!(
            result.is_ok(),
            "prepare(claude-opus-4-7) failed: {:?} — orchestrator DEFAULT_MODEL must be in the table",
            result.err(),
        );
    }

    /// An unknown model id must yield `LlmError::ModelUnavailable`, not a panic
    /// or a misleading error variant.
    #[tokio::test]
    async fn unknown_model_yields_model_unavailable() {
        let cfg = test_config("https://api.anthropic.com");
        let client = DefaultLlmClient::from_config(cfg).expect("config must be valid");

        let req = llm_client::LlmRequest::new("claude-unknown-999");
        let err = client
            .prepare(&req)
            .await
            .expect_err("unknown model must not resolve");
        assert!(
            matches!(err, LlmError::ModelUnavailable),
            "expected ModelUnavailable, got: {err:?}",
        );
    }
}
