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

use std::collections::BTreeMap;

use llm_client::{
    AuthStrategy, Capabilities, ClientConfig, CredentialConfig, LlmError, ModelProfile,
    PricingConfig, ProtocolFamily, ProviderId, ProviderProfile,
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

/// Parse the settings `providers` and `routing` objects into `cfg`, appending
/// custom [`ProviderProfile`]s and injecting model aliases from
/// `routing.aliases`.
///
/// ## Provider entry shape
///
/// ```json
/// {
///   "<profile_name>": {
///     "type": "openai" | "anthropic" | "gemini",
///     "baseUrl": "<url>",
///     "apiKeyEnv": "<ENV_VAR>",
///     "models": [
///       { "id": "<display_model>", "aliases": ["<alias>", …]?, "capabilities": {…}? }
///     ]
///   }
/// }
/// ```
///
/// `models` is **required**; an absent or empty list is an error.
///
/// ## Routing shape
///
/// ```json
/// {
///   "aliases": { "<alias>": "<profile_name>/<model_id>", … }
/// }
/// ```
///
/// - `aliases`: wired — each alias is pushed onto the target
///   [`ModelProfile`], resolved across all providers in `cfg` (including the
///   builtin Anthropic profile). An unknown `profile_name/model_id` target
///   is an [`LlmError::InvalidRequest`].
/// - `fallback` / `retry`: **parsed but inert** — `fallback_model` comes from
///   `DesktopConfig`/argv today; retry policy comes from
///   `CLAUDE_CODE_MAX_RETRIES`. Wiring them is future work and is documented
///   in `engine/src/settings/schema.rs`.
///
/// ## Errors
///
/// - Unknown `"type"` → [`LlmError::InvalidRequest`] naming the type string.
/// - Missing / empty `"models"` → [`LlmError::InvalidRequest`].
/// - Duplicate `profile_name` → [`LlmError::InvalidRequest`].
/// - Unknown alias target (`profile/model`) → [`LlmError::InvalidRequest`].
///
/// # Errors
///
/// Returns [`LlmError::InvalidRequest`] for any validation failure described above.
#[allow(clippy::too_many_lines)]
pub fn apply_settings_providers(
    cfg: &mut ClientConfig,
    providers: &BTreeMap<String, serde_json::Value>,
    routing: Option<&serde_json::Value>,
) -> Result<(), LlmError> {
    for (profile_name, entry) in providers {
        apply_one_provider(cfg, profile_name, entry)?;
    }
    apply_routing_aliases(cfg, routing)?;
    Ok(())
}

/// Parse and append one settings provider entry to `cfg`.
fn apply_one_provider(
    cfg: &mut ClientConfig,
    profile_name: &str,
    entry: &serde_json::Value,
) -> Result<(), LlmError> {
    // Duplicate check
    if cfg.providers.iter().any(|p| p.profile_name == profile_name) {
        return Err(LlmError::InvalidRequest {
            message: format!("duplicate provider profile name: {profile_name:?}"),
        });
    }

    let type_str = entry
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!("provider {profile_name:?}: missing or non-string \"type\""),
        })?;

    let (provider_id, protocol) = match type_str {
        "openai" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::OpenAiChat,
        ),
        "anthropic" => (ProviderId::AnthropicFirstParty, ProtocolFamily::AnthropicMessages),
        "gemini" => (ProviderId::Gemini, ProtocolFamily::GeminiGenerateContent),
        other => {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: unknown type {other:?} (supported: openai, anthropic, gemini)"
                ),
            });
        }
    };

    let base_url = entry
        .get("baseUrl")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    let api_key_env = entry
        .get("apiKeyEnv")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();

    // models is REQUIRED; absent or empty → error.
    let models_val = entry.get("models").ok_or_else(|| LlmError::InvalidRequest {
        message: format!(
            "provider {profile_name:?}: \"models\" is required (no auto-discovery)"
        ),
    })?;
    let models_arr = models_val.as_array().ok_or_else(|| LlmError::InvalidRequest {
        message: format!("provider {profile_name:?}: \"models\" must be an array"),
    })?;
    if models_arr.is_empty() {
        return Err(LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: \"models\" must have at least one entry"
            ),
        });
    }

    let mut model_profiles = Vec::new();
    for m in models_arr {
        model_profiles.push(parse_model_entry(profile_name, m)?);
    }

    cfg.providers.push(ProviderProfile {
        provider_id,
        profile_name: profile_name.to_string(),
        base_url,
        protocol,
        auth: AuthStrategy::ApiKey,
        credential: CredentialConfig::Env { var: api_key_env },
        models: model_profiles,
        pricing: PricingConfig::default(),
    });
    Ok(())
}

/// Parse one `models[n]` entry from the settings JSON into a [`ModelProfile`].
fn parse_model_entry(
    profile_name: &str,
    m: &serde_json::Value,
) -> Result<ModelProfile, LlmError> {
    let id = m
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: each model entry must have an \"id\""
            ),
        })?
        .to_string();

    let aliases: Vec<String> = m
        .get("aliases")
        .and_then(serde_json::Value::as_array)
        .map(|arr| arr.iter().filter_map(|a| a.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let caps = parse_capabilities(m.get("capabilities"));

    Ok(ModelProfile {
        display_model: id.clone(),
        request_model: id.clone(),
        billing_model: id,
        aliases,
        capabilities: caps,
    })
}

/// Parse a `capabilities` JSON object into a [`Capabilities`] value.
///
/// When the object is absent, returns sensible defaults: streaming + tools
/// enabled; vision, documents, reasoning, and structured-output disabled.
fn parse_capabilities(caps_val: Option<&serde_json::Value>) -> Capabilities {
    let Some(caps_val) = caps_val else {
        // Default: streaming + tools, no vision/documents/reasoning/structured_output.
        return Capabilities {
            streaming: true,
            tools: true,
            vision: false,
            documents: false,
            reasoning: false,
            structured_output: false,
        };
    };

    let flag = |key: &str, default: bool| -> bool {
        caps_val
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    };

    Capabilities {
        streaming: flag("streaming", true),
        tools: flag("tools", true),
        vision: flag("vision", false),
        documents: flag("documents", false),
        reasoning: flag("reasoning", false),
        structured_output: flag("structuredOutput", false),
    }
}

/// Wire `routing.aliases` into the target model profiles already in `cfg`.
///
/// `routing.fallback` and `routing.retry` are intentionally ignored here —
/// they are future work documented in the schema comment.
fn apply_routing_aliases(
    cfg: &mut ClientConfig,
    routing: Option<&serde_json::Value>,
) -> Result<(), LlmError> {
    let Some(routing) = routing else {
        return Ok(());
    };
    let Some(aliases_map) = routing.get("aliases").and_then(serde_json::Value::as_object) else {
        return Ok(());
    };

    for (alias, target_val) in aliases_map {
        let target = target_val.as_str().ok_or_else(|| LlmError::InvalidRequest {
            message: format!("routing.aliases[{alias:?}]: value must be a string"),
        })?;

        // target is "profile_name/model_id"
        let (profile_part, model_part) = target.split_once('/').ok_or_else(|| {
            LlmError::InvalidRequest {
                message: format!(
                    "routing.aliases[{alias:?}]: target {target:?} must be \"profile/model\""
                ),
            }
        })?;

        // Find the matching model in cfg.providers (including builtin).
        let model = cfg
            .providers
            .iter_mut()
            .find(|p| p.profile_name == profile_part)
            .and_then(|p| p.models.iter_mut().find(|m| m.display_model == model_part))
            .ok_or_else(|| LlmError::InvalidRequest {
                message: format!(
                    "routing.aliases[{alias:?}]: target {target:?} not found in any configured profile"
                ),
            })?;

        if !model.aliases.contains(&alias.to_string()) {
            model.aliases.push(alias.clone());
        }
    }
    Ok(())
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

    // ---- apply_settings_providers tests ------------------------------------

    /// A groq-style OpenAI-compatible profile parses correctly and is appended
    /// to the builtin Anthropic profile, so both profiles are available.
    #[test]
    fn openai_profile_appended() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "groq": {
                "type": "openai",
                "baseUrl": "https://api.groq.com/openai/v1",
                "apiKeyEnv": "GROQ_API_KEY",
                "models": [
                    { "id": "llama-3.3-70b-versatile" }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        assert_eq!(cfg.providers.len(), 2, "builtin + groq");
        let groq = cfg.providers.iter().find(|p| p.profile_name == "groq").unwrap();
        assert_eq!(groq.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(groq.protocol, ProtocolFamily::OpenAiChat);
        assert!(
            matches!(&groq.provider_id, ProviderId::OpenAICompatible { name } if name == "groq"),
            "provider_id must be OpenAICompatible with name=groq"
        );
        assert_eq!(
            groq.credential,
            CredentialConfig::Env { var: "GROQ_API_KEY".to_string() }
        );
        assert_eq!(groq.models.len(), 1);
        assert_eq!(groq.models[0].display_model, "llama-3.3-70b-versatile");
        // Default capabilities: streaming + tools, no vision/docs/reasoning.
        assert!(groq.models[0].capabilities.streaming);
        assert!(groq.models[0].capabilities.tools);
        assert!(!groq.models[0].capabilities.vision);
        assert!(!groq.models[0].capabilities.reasoning);
    }

    /// A gemini profile parses with the correct protocol family and provider id.
    #[test]
    fn gemini_profile_parses() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-gemini": {
                "type": "gemini",
                "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
                "apiKeyEnv": "GEMINI_API_KEY",
                "models": [
                    { "id": "gemini-2.0-flash" }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let gemini = cfg.providers.iter().find(|p| p.profile_name == "my-gemini").unwrap();
        assert_eq!(gemini.protocol, ProtocolFamily::GeminiGenerateContent);
        assert_eq!(gemini.provider_id, ProviderId::Gemini);
        assert_eq!(
            gemini.credential,
            CredentialConfig::Env { var: "GEMINI_API_KEY".to_string() }
        );
    }

    /// An alias in `routing.aliases` pointing at a custom profile's model
    /// gets pushed onto that model's alias list.
    #[test]
    fn alias_injected_into_custom_profile() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "groq": {
                "type": "openai",
                "baseUrl": "https://api.groq.com/openai/v1",
                "apiKeyEnv": "GROQ_API_KEY",
                "models": [{ "id": "llama-3.3-70b-versatile" }]
            }
        }"#).unwrap();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "aliases": { "llama": "groq/llama-3.3-70b-versatile" }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, Some(&routing)).expect("must succeed");

        let groq = cfg.providers.iter().find(|p| p.profile_name == "groq").unwrap();
        assert!(
            groq.models[0].aliases.contains(&"llama".to_string()),
            "alias 'llama' must be injected into the model's aliases"
        );
    }

    /// An alias in `routing.aliases` pointing at a builtin profile's model
    /// (e.g. "anthropic/claude-sonnet-4-20250514") is also wired.
    #[test]
    fn alias_injected_into_builtin_profile() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "aliases": { "my-sonnet": "anthropic/claude-sonnet-4-20250514" }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &BTreeMap::new(), Some(&routing)).expect("must succeed");

        let anthropic = cfg.providers.iter().find(|p| p.profile_name == "anthropic").unwrap();
        let sonnet = anthropic
            .models
            .iter()
            .find(|m| m.display_model == "claude-sonnet-4-20250514")
            .unwrap();
        assert!(
            sonnet.aliases.contains(&"my-sonnet".to_string()),
            "alias 'my-sonnet' must be injected into the builtin sonnet model's aliases"
        );
    }

    /// A provider entry with no `models` key is rejected with [`LlmError::InvalidRequest`].
    #[test]
    fn missing_models_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "bad-provider": {
                "type": "openai",
                "baseUrl": "https://example.com",
                "apiKeyEnv": "SOME_KEY"
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("required")),
            "expected InvalidRequest about missing models, got: {err:?}"
        );
    }

    /// A provider entry with an empty `models` array is rejected.
    #[test]
    fn empty_models_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "empty-provider": {
                "type": "openai",
                "baseUrl": "https://example.com",
                "apiKeyEnv": "SOME_KEY",
                "models": []
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("at least one")),
            "expected InvalidRequest about empty models, got: {err:?}"
        );
    }

    /// An unknown `type` value is rejected with a message naming the type.
    #[test]
    fn unknown_type_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "weird": {
                "type": "cohere",
                "baseUrl": "https://api.cohere.ai",
                "apiKeyEnv": "COHERE_KEY",
                "models": [{ "id": "command-r" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("cohere")),
            "expected InvalidRequest naming the unknown type, got: {err:?}"
        );
    }

    /// A duplicate profile name is rejected with a clear error.
    #[test]
    fn duplicate_profile_name_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        // "anthropic" is already the builtin profile name.
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "anthropic": {
                "type": "openai",
                "baseUrl": "https://example.com",
                "apiKeyEnv": "KEY",
                "models": [{ "id": "some-model" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("duplicate")),
            "expected InvalidRequest about duplicate profile, got: {err:?}"
        );
    }

    /// An alias targeting an unknown profile/model is rejected.
    #[test]
    fn alias_unknown_target_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "aliases": { "fast": "nonexistent-profile/some-model" }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &BTreeMap::new(), Some(&routing)).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("not found")),
            "expected InvalidRequest about unknown alias target, got: {err:?}"
        );
    }
}
