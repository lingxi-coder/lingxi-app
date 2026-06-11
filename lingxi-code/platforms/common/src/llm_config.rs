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

/// Parsed routing overrides from the settings `routing` object.
///
/// Returned by [`parse_routing_overrides`].  Hosts pass these into the adapter
/// constructor so the retry/fallback machinery uses the settings-configured
/// values rather than the compile-time defaults.
///
/// ## Fields
///
/// - `fallback`: per-model fallback target. Key is the **display model** of
///   the primary model (alias-resolved); value is the display model of the
///   fallback target (must resolve in `cfg`). Only chain[0] is stored; longer
///   chains warn via `tracing::warn!`.
/// - `max_retries`: `routing.retry.maxAttempts` parsed as `u32`. When `None`,
///   `CLAUDE_CODE_MAX_RETRIES` env (then `DEFAULT_MAX_RETRIES`) applies.
/// - `backoff_ms`: `routing.retry.backoffMs` as the base-delay for the jitter
///   ladder's first rung (scales `DEFAULT_BASE_DELAYS_MS` proportionally).
///   When `None`, the default `[500, 1000, 2000]` ladder is used.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RoutingOverrides {
    /// Per-model fallback targets: display-model → display-model.
    pub fallback: BTreeMap<String, String>,
    /// `routing.retry.maxAttempts` override.
    pub max_retries: Option<u32>,
    /// `routing.retry.backoffMs` override (first rung of the jitter ladder).
    pub backoff_ms: Option<u64>,
}

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
    if base_url.is_empty() {
        return Err(LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: \"baseUrl\" is required and must not be empty"
            ),
        });
    }

    let api_key_env = entry
        .get("apiKeyEnv")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if api_key_env.is_empty() {
        return Err(LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: \"apiKeyEnv\" is required and must not be empty"
            ),
        });
    }

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

/// Resolve a display model from `cfg` using the `profile/model` target string.
///
/// Returns the `display_model` string of the resolved model (same as the
/// `model_id` part in most cases, but normalised via the registry).
fn resolve_display_model<'a>(
    cfg: &'a llm_client::ClientConfig,
    profile_part: &str,
    model_part: &str,
) -> Option<&'a str> {
    cfg.providers
        .iter()
        .find(|p| p.profile_name == profile_part)
        .and_then(|p| p.models.iter().find(|m| m.display_model == model_part || m.aliases.contains(&model_part.to_string())))
        .map(|m| m.display_model.as_str())
}

/// Parse `routing.fallback`, `routing.retry.maxAttempts`, and
/// `routing.retry.backoffMs` from the settings `routing` object.
///
/// ## Fallback shape
///
/// ```json
/// "fallback": { "<primary-model-or-alias>": ["<profile/model>", ...] }
/// ```
///
/// - Key is normalised to the display model via registry-style resolution
///   against `cfg` (same as how `apply_routing_aliases` resolves targets).
///   When the key doesn't resolve as a `profile/model` it is tried as a bare
///   display model or alias across all providers.
/// - Only chain[0] is used.  Longer chains emit a `tracing::warn!` and the
///   extra entries are discarded.
/// - The target (`chain[0]`) must resolve to a known `profile/model`; an
///   unknown target is an [`LlmError::InvalidRequest`].
///
/// ## Retry shape
///
/// ```json
/// "retry": { "maxAttempts": 5, "backoffMs": 1000 }
/// ```
///
/// - `maxAttempts` (u32) → [`RoutingOverrides::max_retries`].
/// - `backoffMs` (u64) → [`RoutingOverrides::backoff_ms`].
///
/// ## Precedence (adapter)
///
/// `CLAUDE_CODE_MAX_RETRIES` env > `routing.retry.maxAttempts` > `DEFAULT_MAX_RETRIES` (10).
/// Per-model `routing.fallback` entry wins over the adapter's global `fallback_model`.
///
/// # Errors
///
/// Returns [`LlmError::InvalidRequest`] when a fallback target (`chain[0]`)
/// cannot be resolved in `cfg`.
pub fn parse_routing_overrides(
    routing: &serde_json::Value,
    cfg: &llm_client::ClientConfig,
) -> Result<RoutingOverrides, llm_client::LlmError> {
    let mut overrides = RoutingOverrides::default();

    // ── fallback ──────────────────────────────────────────────────────────────
    if let Some(fallback_map) = routing.get("fallback").and_then(serde_json::Value::as_object) {
        for (key, chain_val) in fallback_map {
            // Resolve the KEY to a display model.
            // The key may be "profile/model" or a bare alias/display model.
            let key_display = if let Some((profile_part, model_part)) = key.split_once('/') {
                resolve_display_model(cfg, profile_part, model_part)
                    .map_or_else(|| key.clone(), str::to_string)
            } else {
                // Bare name: search all providers for an alias or display match.
                cfg.providers
                    .iter()
                    .flat_map(|p| p.models.iter())
                    .find(|m| m.display_model == *key || m.aliases.contains(key))
                    .map_or_else(|| key.clone(), |m| m.display_model.clone())
            };

            // chain_val must be an array; we only use chain[0].
            let chain = chain_val.as_array().ok_or_else(|| llm_client::LlmError::InvalidRequest {
                message: format!(
                    "routing.fallback[{key:?}]: value must be an array of \"profile/model\" strings"
                ),
            })?;
            if chain.is_empty() {
                return Err(llm_client::LlmError::InvalidRequest {
                    message: format!(
                        "routing.fallback[{key:?}]: chain must have at least one entry"
                    ),
                });
            }
            if chain.len() > 1 {
                tracing::warn!(
                    "routing.fallback[{key:?}]: fallback chains beyond the first entry are not yet supported; using chain[0] only"
                );
            }
            let target = chain[0].as_str().ok_or_else(|| llm_client::LlmError::InvalidRequest {
                message: format!(
                    "routing.fallback[{key:?}]: chain[0] must be a \"profile/model\" string"
                ),
            })?;
            // Validate the target resolves.
            let (profile_part, model_part) = target.split_once('/').ok_or_else(|| {
                llm_client::LlmError::InvalidRequest {
                    message: format!(
                        "routing.fallback[{key:?}]: target {target:?} must be \"profile/model\""
                    ),
                }
            })?;
            let target_display = resolve_display_model(cfg, profile_part, model_part)
                .ok_or_else(|| llm_client::LlmError::InvalidRequest {
                    message: format!(
                        "routing.fallback[{key:?}]: target {target:?} not found in any configured profile"
                    ),
                })?
                .to_string();

            overrides.fallback.insert(key_display, target_display);
        }
    }

    // ── retry ────────────────────────────────────────────────────────────────
    if let Some(retry_obj) = routing.get("retry").and_then(serde_json::Value::as_object) {
        if let Some(max_attempts_val) = retry_obj.get("maxAttempts") {
            let n = max_attempts_val.as_u64().ok_or_else(|| llm_client::LlmError::InvalidRequest {
                message: "routing.retry.maxAttempts must be a non-negative integer".to_string(),
            })?;
            overrides.max_retries = Some(u32::try_from(n).unwrap_or(u32::MAX));
        }
        if let Some(backoff_val) = retry_obj.get("backoffMs") {
            let n = backoff_val.as_u64().ok_or_else(|| llm_client::LlmError::InvalidRequest {
                message: "routing.retry.backoffMs must be a non-negative integer".to_string(),
            })?;
            if n == 0 {
                // 0 would collapse the jitter ladder to zero-delay retries (a
                // tight retry loop hammering the provider) — reject up front.
                return Err(llm_client::LlmError::InvalidRequest {
                    message: "routing.retry.backoffMs must be >= 1 (0 would disable backoff entirely)"
                        .to_string(),
                });
            }
            overrides.backoff_ms = Some(n);
        }
    }

    Ok(overrides)
}

/// Wire `routing.aliases` into the target model profiles already in `cfg`.
///
/// `routing.fallback` and `routing.retry` are parsed by [`parse_routing_overrides`]
/// separately and threaded into the adapter constructor.
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

    /// A provider entry with a missing `baseUrl` is rejected.
    #[test]
    fn missing_base_url_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "no-url": {
                "type": "openai",
                "apiKeyEnv": "SOME_KEY",
                "models": [{ "id": "some-model" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("baseUrl")),
            "expected InvalidRequest about missing baseUrl, got: {err:?}"
        );
    }

    /// A provider entry with an empty `baseUrl` is rejected.
    #[test]
    fn empty_base_url_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "empty-url": {
                "type": "openai",
                "baseUrl": "",
                "apiKeyEnv": "SOME_KEY",
                "models": [{ "id": "some-model" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("baseUrl")),
            "expected InvalidRequest about empty baseUrl, got: {err:?}"
        );
    }

    /// A provider entry with a missing `apiKeyEnv` is rejected.
    #[test]
    fn missing_api_key_env_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "no-key-env": {
                "type": "openai",
                "baseUrl": "https://example.com",
                "models": [{ "id": "some-model" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("apiKeyEnv")),
            "expected InvalidRequest about missing apiKeyEnv, got: {err:?}"
        );
    }

    /// A provider entry with an empty `apiKeyEnv` is rejected.
    #[test]
    fn empty_api_key_env_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "empty-key-env": {
                "type": "openai",
                "baseUrl": "https://example.com",
                "apiKeyEnv": "",
                "models": [{ "id": "some-model" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("apiKeyEnv")),
            "expected InvalidRequest about empty apiKeyEnv, got: {err:?}"
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

    // ── parse_routing_overrides tests ─────────────────────────────────────────

    fn routing_test_cfg() -> ClientConfig {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        // Add a second provider so we can test cross-profile fallback.
        cfg.providers.push(llm_client::ProviderProfile {
            provider_id: llm_client::ProviderId::OpenAICompatible { name: "groq".to_string() },
            profile_name: "groq".to_string(),
            base_url: "https://api.groq.com".to_string(),
            protocol: llm_client::ProtocolFamily::OpenAiChat,
            auth: llm_client::AuthStrategy::ApiKey,
            credential: llm_client::CredentialConfig::Env { var: "GROQ_KEY".to_string() },
            models: vec![llm_client::ModelProfile {
                display_model: "llama-3.3-70b".to_string(),
                request_model: "llama-3.3-70b".to_string(),
                billing_model: "llama-3.3-70b".to_string(),
                aliases: vec!["llama".to_string()],
                capabilities: llm_client::Capabilities { streaming: true, tools: true, ..Default::default() },
            }],
            pricing: PricingConfig::default(),
        });
        cfg
    }

    /// Happy path: fallback + retry numbers parsed correctly.
    #[test]
    fn parse_routing_overrides_happy() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "anthropic/claude-opus-4-7": ["anthropic/claude-sonnet-4-20250514"]
            },
            "retry": { "maxAttempts": 5, "backoffMs": 1000 }
        }"#).unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed");
        assert_eq!(
            overrides.fallback.get("claude-opus-4-7"),
            Some(&"claude-sonnet-4-20250514".to_string()),
            "fallback key must normalize to display model"
        );
        assert_eq!(overrides.max_retries, Some(5));
        assert_eq!(overrides.backoff_ms, Some(1000));
    }

    /// Unknown fallback target errors with not found.
    #[test]
    fn parse_routing_overrides_unknown_fallback_target_error() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "anthropic/claude-opus-4-7": ["nonexistent/model"]
            }
        }"#).unwrap();

        let err = parse_routing_overrides(&routing, &cfg).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("not found")),
            "expected not found error, got: {err:?}"
        );
    }

    /// Chain >1 uses first entry and logs a warning (no error).
    #[test]
    fn parse_routing_overrides_chain_gt1_uses_first_entry() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "anthropic/claude-opus-4-7": [
                    "anthropic/claude-sonnet-4-20250514",
                    "groq/llama-3.3-70b"
                ]
            }
        }"#).unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed with chain>1");
        // chain[0] must be used
        assert_eq!(
            overrides.fallback.get("claude-opus-4-7"),
            Some(&"claude-sonnet-4-20250514".to_string()),
            "chain[0] must be used when chain length > 1"
        );
    }

    /// Retry numbers parsed: maxAttempts and backoffMs.
    #[test]
    fn parse_routing_overrides_retry_numbers() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "retry": { "maxAttempts": 3, "backoffMs": 2000 }
        }"#).unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed");
        assert_eq!(overrides.max_retries, Some(3));
        assert_eq!(overrides.backoff_ms, Some(2000));
        assert!(overrides.fallback.is_empty());
    }

    /// `backoffMs: 0` is rejected at parse time (zero-delay retries are a
    /// tight loop hammering the provider).
    #[test]
    fn parse_routing_overrides_backoff_zero_rejected() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "retry": { "backoffMs": 0 }
        }"#).unwrap();

        let err = parse_routing_overrides(&routing, &cfg).expect_err("backoffMs=0 must error");
        let llm_client::LlmError::InvalidRequest { message } = err else {
            panic!("expected InvalidRequest, got {err:?}");
        };
        assert!(message.contains("backoffMs must be >= 1"), "got: {message}");
    }

    /// Absent routing → defaults (no overrides).
    #[test]
    fn parse_routing_overrides_absent_gives_defaults() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str("{}").unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed");
        assert!(overrides.fallback.is_empty());
        assert!(overrides.max_retries.is_none());
        assert!(overrides.backoff_ms.is_none());
    }

    /// Alias in key is resolved to display model.
    #[test]
    fn parse_routing_overrides_key_alias_resolves_to_display_model() {
        let cfg = routing_test_cfg();
        // "llama" is an alias for "llama-3.3-70b" in the groq profile.
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "llama": ["anthropic/claude-sonnet-4-20250514"]
            }
        }"#).unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed");
        assert_eq!(
            overrides.fallback.get("llama-3.3-70b"),
            Some(&"claude-sonnet-4-20250514".to_string()),
            "alias key 'llama' must resolve to display model 'llama-3.3-70b'"
        );
    }
}
