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
//! | claude-sonnet-4-20250514     | claude-sonnet-4      | true      |
//! | claude-sonnet-4-5-20250929   | claude-sonnet-4-5    | true      |
//! | claude-sonnet-4-6            | claude-sonnet-4-6    | true      |
//! | claude-opus-4-20250514       | claude-opus-4        | true      |
//! | claude-opus-4-1-20250805     | claude-opus-4-1      | true      |
//! | claude-opus-4-5-20251101     | claude-opus-4-5      | true      |
//! | claude-opus-4-6              | claude-opus-4-6      | true      |
//! | claude-opus-4-7              | claude-opus-4-7      | true      |  ← orchestrator DEFAULT_MODEL
//! | claude-haiku-4-20250307      | claude-haiku-4       | true      |
//! | claude-haiku-4-5             | claude-haiku-4-5     | true      |
//!
//! All Claude-4-generation models support the Anthropic `thinking` field
//! (`modelSupportsThinking` → true for every non-`claude-3-*` first-party
//! model), so every entry carries `reasoning: true` — the thinking field is sent
//! by default (adaptive where supported, fixed budget otherwise). This gates
//! `llm_client::validate_capabilities`'s reasoning check.
//!
//! ## Auth strategies
//!
//! `oauth_path = false` → `ApiKey` + `CredentialConfig::Env { var: "ANTHROPIC_API_KEY" }`
//! `oauth_path = true`  → `OAuthBearer` + `CredentialConfig::HostManaged { id: "anthropic_oauth" }`
//!
//! All models get streaming, tools, vision, documents, and `reasoning: true`
//! (every Claude-4-generation model supports the `thinking` field).

use std::collections::BTreeMap;

/// Parsed routing overrides from the settings `routing` object.
///
/// Returned by [`parse_routing_overrides`].  Hosts pass these into the adapter
/// constructor so the retry/fallback machinery uses the settings-configured
/// values rather than the compile-time defaults.
///
/// ## Fields
///
/// - `fallback`: per-model fallback chain. Key is the **display model** of
///   the primary model (alias-resolved); value is an ordered list of fallback
///   targets (display models, each validated against `cfg`). The adapter walks
///   the chain on consecutive overload events: chain[0] fires first, chain[1]
///   when chain[0] is also overloaded, and so on until exhausted.
/// - `max_retries`: `routing.retry.maxAttempts` parsed as `u32`. When `None`,
///   `CLAUDE_CODE_MAX_RETRIES` env (then `DEFAULT_MAX_RETRIES`) applies.
/// - `backoff_ms`: `routing.retry.backoffMs` as the base-delay for the
///   exponential backoff ladder's first rung (overrides `BASE_DELAY_MS` = 500,
///   keeping `min(b * 2^attempt, 32000)` growth + cap). When `None`, the
///   default ladder `min(500 * 2^attempt, 32000)` is used.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RoutingOverrides {
    /// Per-model fallback chains: display-model → ordered Vec of display-models.
    pub fallback: BTreeMap<String, Vec<String>>,
    /// `routing.retry.maxAttempts` override.
    pub max_retries: Option<u32>,
    /// `routing.retry.backoffMs` override (first rung of the jitter ladder).
    pub backoff_ms: Option<u64>,
}

use llm_client::{
    AuthStrategy, AzureConfig, Capabilities, ClientConfig, CredentialConfig, LlmError,
    ModelProfile, PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, SigningConfig,
    TokenPricing,
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
            signing: None,
            azure: None,
            models: vec![
                // — Claude Sonnet 4 (default engine model) —
                model(
                    "claude-sonnet-4-20250514",
                    "claude-sonnet-4",
                    &["claude-sonnet-4", "claude-sonnet", "claude"],
                    true,
                ),
                // — Claude Sonnet 4.5 —
                model(
                    "claude-sonnet-4-5-20250929",
                    "claude-sonnet-4-5",
                    &["claude-sonnet-4-5"],
                    true,
                ),
                // — Claude Sonnet 4.6 —
                model("claude-sonnet-4-6", "claude-sonnet-4-6", &[], true),
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
                    true,
                ),
                // — Claude Haiku 4.5 —
                model("claude-haiku-4-5", "claude-haiku-4-5", &[], true),
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
///     "type": "openai" | "openai-responses" | "anthropic" | "gemini"
///           | "azure-openai" | "bedrock-claude" | "vertex-claude" | "vertex-gemini",
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
// Each provider type is a large self-contained arm; the line count is inherent.
#[allow(clippy::too_many_lines)]
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
        // OpenAI Responses API (`POST {baseUrl}/responses`); same baseUrl /
        // apiKeyEnv requirements and ApiKey (Bearer) auth as "openai".
        "openai-responses" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::OpenAiResponses,
        ),
        "anthropic" => (ProviderId::AnthropicFirstParty, ProtocolFamily::AnthropicMessages),
        "gemini" => (ProviderId::Gemini, ProtocolFamily::GeminiGenerateContent),
        "azure-openai" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::AzureOpenAi,
        ),
        "bedrock-claude" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::BedrockClaude,
        ),
        // Vertex AI: Claude on Vertex (rawPredict/streamRawPredict SSE)
        "vertex-claude" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::VertexClaude,
        ),
        // Vertex AI: Gemini on Vertex (generateContent/streamGenerateContent SSE)
        "vertex-gemini" => (
            ProviderId::OpenAICompatible { name: profile_name.to_string() },
            ProtocolFamily::VertexGemini,
        ),
        other => {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: unknown type {other:?} (supported: openai, openai-responses, anthropic, gemini, azure-openai, bedrock-claude, vertex-claude, vertex-gemini)"
                ),
            });
        }
    };

    // For bedrock-claude, `region` is required and `baseUrl` may be omitted
    // (defaults to the Bedrock runtime endpoint for the region).
    // For all other types, `baseUrl` is required.
    let (base_url, bedrock_signing) = if type_str == "bedrock-claude" {
        let region = entry
            .get("region")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if region.is_empty() {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: \"region\" is required for bedrock-claude type"
                ),
            });
        }
        let base_url = entry
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || format!("https://bedrock-runtime.{region}.amazonaws.com"),
                str::to_string,
            );
        let signing = SigningConfig { region, service: "bedrock".to_string() };
        (base_url, Some(signing))
    } else {
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
        (base_url, None)
    };

    // For bedrock-claude, apiKeyEnv is NOT required — SigV4 credentials are
    // host-managed or loaded via StaticCredentialProvider (three-field AWS
    // credentials cannot be expressed through a single environment variable).
    // `CredentialConfig::HostManaged { id: "bedrock_sigv4" }` is used so that
    // a StaticCredentialProvider (or host-managed store) can supply the three-
    // field Credential::AwsSigV4 at request time.
    // Credential errors (missing access key / secret / session token) surface
    // at request time via LlmError::Authentication.
    //
    // For all other types, apiKeyEnv is required.
    let credential_config = if type_str == "bedrock-claude" {
        // Use HostManaged so the client's injected CredentialProvider is consulted.
        CredentialConfig::HostManaged { id: "bedrock_sigv4".to_string() }
    } else {
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
        CredentialConfig::Env { var: api_key_env }
    };

    // For azure-openai, apiVersion is required.
    let azure_config = if type_str == "azure-openai" {
        let api_version = entry
            .get("apiVersion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if api_version.is_empty() {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: \"apiVersion\" is required for azure-openai type"
                ),
            });
        }
        Some(AzureConfig { api_version })
    } else {
        None
    };

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

    // Parse optional "pricing" block → per-model price overrides.
    let pricing =
        if let Some(pricing_val) = entry.get("pricing") {
            parse_pricing_overrides(profile_name, pricing_val, &model_profiles)?
        } else {
            PricingConfig::default()
        };

    // Auth strategy by type:
    // - azure-openai: AzureToken (injects `api-key:` header rather than `Authorization: Bearer`)
    // - bedrock-claude: AwsSigV4 (SigV4 request signing; credentials via StaticCredentialProvider)
    // - vertex-claude / vertex-gemini: GcpToken (Bearer token; `apiKeyEnv` holds the bearer token
    //   env var; `EnvCredentialProvider` loads it as `Credential::ApiKey(value)`, and the
    //   `GcpToken` authenticate arm accepts both `ApiKey` and `BearerToken` via `load_secret`)
    // - all others: standard ApiKey
    let auth = if type_str == "azure-openai" {
        AuthStrategy::AzureToken
    } else if type_str == "bedrock-claude" {
        AuthStrategy::AwsSigV4
    } else if type_str == "vertex-claude" || type_str == "vertex-gemini" {
        AuthStrategy::GcpToken
    } else {
        AuthStrategy::ApiKey
    };

    cfg.providers.push(ProviderProfile {
        provider_id,
        profile_name: profile_name.to_string(),
        base_url,
        protocol,
        auth,
        credential: credential_config,
        models: model_profiles,
        pricing,
        signing: bedrock_signing,
        azure: azure_config,
    });
    Ok(())
}

/// Parse a `providers.<name>.pricing` JSON object into [`PricingConfig::overrides`].
///
/// ## Settings shape
///
/// ```json
/// "pricing": {
///   "<model-id>": {
///     "inputPerMtok": 1.5,
///     "outputPerMtok": 6.0,
///     "cacheWritePerMtok": 1.875,
///     "cacheReadPerMtok": 0.15,
///     "reasoningPerMtok": 6.0
///   }
/// }
/// ```
///
/// `model-id` is the display model `id` from the `models` array (e.g.
/// `"gpt-4o"` in `"models": [{"id": "gpt-4o"}]`).  Unknown model ids (not
/// present in `model_profiles`) are rejected as config bugs.  Negative prices
/// and non-number values are also rejected.  Unknown keys inside a model's
/// pricing object are rejected (strict — typos in field names could silently
/// produce wrong pricing).
///
/// # Errors
///
/// Returns [`LlmError::InvalidRequest`] for any of the above violations.
fn parse_pricing_overrides(
    profile_name: &str,
    pricing_val: &serde_json::Value,
    model_profiles: &[ModelProfile],
) -> Result<PricingConfig, LlmError> {
    const KNOWN_PRICING_KEYS: &[&str] = &[
        "inputPerMtok",
        "outputPerMtok",
        "cacheWritePerMtok",
        "cacheReadPerMtok",
        "reasoningPerMtok",
    ];

    let pricing_obj = pricing_val.as_object().ok_or_else(|| LlmError::InvalidRequest {
        message: format!(
            "provider {profile_name:?}: \"pricing\" must be an object (got {})",
            match pricing_val {
                serde_json::Value::Array(_) => "array",
                serde_json::Value::Bool(_) => "bool",
                serde_json::Value::Number(_) => "number",
                serde_json::Value::String(_) => "string",
                serde_json::Value::Null => "null",
                serde_json::Value::Object(_) => "object",
            }
        ),
    })?;

    let mut overrides = Vec::new();

    for (model_id, model_pricing_val) in pricing_obj {
        // Verify the model id is known in this profile's models list.
        let is_known = model_profiles.iter().any(|m| m.display_model == *model_id);
        if !is_known {
            return Err(LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: pricing key {model_id:?} is not in the models list — add the model first or remove the override"
                ),
            });
        }

        let model_pricing_obj = model_pricing_val.as_object().ok_or_else(|| LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: pricing[{model_id:?}] must be an object"
            ),
        })?;

        // Validate: no unknown keys.
        for key in model_pricing_obj.keys() {
            if !KNOWN_PRICING_KEYS.contains(&key.as_str()) {
                return Err(LlmError::InvalidRequest {
                    message: format!(
                        "provider {profile_name:?}: pricing[{model_id:?}] unknown key {key:?} (known: inputPerMtok, outputPerMtok, cacheWritePerMtok, cacheReadPerMtok, reasoningPerMtok)"
                    ),
                });
            }
        }

        // Parse required fields.
        let input_per_million = parse_price_field(profile_name, model_id, model_pricing_obj, "inputPerMtok", true)?
            .unwrap_or(0.0);
        let output_per_million = parse_price_field(profile_name, model_id, model_pricing_obj, "outputPerMtok", true)?
            .unwrap_or(0.0);
        let cache_write_per_million = parse_price_field(profile_name, model_id, model_pricing_obj, "cacheWritePerMtok", false)?
            .unwrap_or(0.0);
        let cache_read_per_million = parse_price_field(profile_name, model_id, model_pricing_obj, "cacheReadPerMtok", false)?
            .unwrap_or(0.0);
        let reasoning_per_million = parse_price_field(profile_name, model_id, model_pricing_obj, "reasoningPerMtok", false)?
            .unwrap_or(0.0);

        let pricing = TokenPricing {
            input_per_million,
            output_per_million,
            cache_write_per_million,
            cache_read_per_million,
            reasoning_per_million,
        };
        overrides.push((model_id.clone(), pricing));
    }

    Ok(PricingConfig { require_priced: false, overrides })
}

/// Parse and validate one price field from a model's pricing object.
///
/// Returns `Ok(None)` when `required = false` and the key is absent;
/// `Ok(Some(v))` when present and valid; `Err` on missing-required, non-number,
/// or negative.
fn parse_price_field(
    profile_name: &str,
    model_id: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    required: bool,
) -> Result<Option<f64>, LlmError> {
    match obj.get(key) {
        None if required => Err(LlmError::InvalidRequest {
            message: format!(
                "provider {profile_name:?}: pricing[{model_id:?}] missing required field {key:?}"
            ),
        }),
        None => Ok(None),
        Some(val) => {
            let v = val.as_f64().ok_or_else(|| LlmError::InvalidRequest {
                message: format!(
                    "provider {profile_name:?}: pricing[{model_id:?}].{key} must be a number, got {val}"
                ),
            })?;
            if v < 0.0 {
                return Err(LlmError::InvalidRequest {
                    message: format!(
                        "provider {profile_name:?}: pricing[{model_id:?}].{key} must be >= 0 (got {v})"
                    ),
                });
            }
            Ok(Some(v))
        }
    }
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
/// - The **full chain** is validated and stored.  Every entry must resolve to
///   a known `profile/model`; an unknown entry is an [`LlmError::InvalidRequest`].
///   The adapter walks the chain in order: chain[0] fires first on the initial
///   overload fallback, chain[1] when chain[0] is also overloaded, and so on
///   until exhausted.
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
/// Returns [`LlmError::InvalidRequest`] when any fallback target in the chain
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

            // chain_val must be a non-empty array; every entry is validated.
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
            // Validate every entry in the chain and collect display models.
            let mut resolved_chain: Vec<String> = Vec::with_capacity(chain.len());
            for (i, entry_val) in chain.iter().enumerate() {
                let target = entry_val.as_str().ok_or_else(|| llm_client::LlmError::InvalidRequest {
                    message: format!(
                        "routing.fallback[{key:?}]: chain[{i}] must be a \"profile/model\" string"
                    ),
                })?;
                let (profile_part, model_part) = target.split_once('/').ok_or_else(|| {
                    llm_client::LlmError::InvalidRequest {
                        message: format!(
                            "routing.fallback[{key:?}]: chain[{i}] target {target:?} must be \"profile/model\""
                        ),
                    }
                })?;
                let target_display = resolve_display_model(cfg, profile_part, model_part)
                    .ok_or_else(|| llm_client::LlmError::InvalidRequest {
                        message: format!(
                            "routing.fallback[{key:?}]: chain[{i}] target {target:?} not found in any configured profile"
                        ),
                    })?
                    .to_string();
                resolved_chain.push(target_display);
            }

            overrides.fallback.insert(key_display, resolved_chain);
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
            signing: None,
            azure: None,
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
            Some(&vec!["claude-sonnet-4-20250514".to_string()]),
            "fallback key must normalize to display model; chain stored as Vec"
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

    /// Unknown fallback target in chain[1] also errors (every entry validated).
    #[test]
    fn parse_routing_overrides_unknown_chain1_target_error() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "anthropic/claude-opus-4-7": [
                    "anthropic/claude-sonnet-4-20250514",
                    "nonexistent/model-two"
                ]
            }
        }"#).unwrap();

        let err = parse_routing_overrides(&routing, &cfg).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("not found")),
            "chain[1] unknown target must error with not found, got: {err:?}"
        );
    }

    /// Multi-entry chain: all entries validated and stored in order (no warn, no truncation).
    #[test]
    fn parse_routing_overrides_full_chain_stored() {
        let cfg = routing_test_cfg();
        let routing: serde_json::Value = serde_json::from_str(r#"{
            "fallback": {
                "anthropic/claude-opus-4-7": [
                    "anthropic/claude-sonnet-4-20250514",
                    "groq/llama-3.3-70b"
                ]
            }
        }"#).unwrap();

        let overrides = parse_routing_overrides(&routing, &cfg).expect("must succeed with multi-entry chain");
        // Full chain must be stored in order.
        assert_eq!(
            overrides.fallback.get("claude-opus-4-7"),
            Some(&vec![
                "claude-sonnet-4-20250514".to_string(),
                "llama-3.3-70b".to_string(),
            ]),
            "full chain must be stored with all entries in order"
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
            Some(&vec!["claude-sonnet-4-20250514".to_string()]),
            "alias key 'llama' must resolve to display model 'llama-3.3-70b'"
        );
    }

    // ── parse_pricing_overrides (Task 2) tests ─────────────────────────────────

    /// Happy path: a provider with a pricing block parses correctly and
    /// `PricingConfig::overrides` carries the right `TokenPricing` values.
    #[test]
    fn pricing_overrides_happy_path() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "gpt-custom": {
                        "inputPerMtok": 1.5,
                        "outputPerMtok": 6.0,
                        "cacheWritePerMtok": 1.875,
                        "cacheReadPerMtok": 0.15,
                        "reasoningPerMtok": 6.0
                    }
                }
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let p = cfg.providers.iter().find(|p| p.profile_name == "myprovider").unwrap();
        assert_eq!(p.pricing.overrides.len(), 1);
        let (model_id, tp) = &p.pricing.overrides[0];
        assert_eq!(model_id, "gpt-custom");
        assert!((tp.input_per_million - 1.5).abs() < 1e-12, "input_per_million");
        assert!((tp.output_per_million - 6.0).abs() < 1e-12, "output_per_million");
        assert!((tp.cache_write_per_million - 1.875).abs() < 1e-12, "cache_write_per_million");
        assert!((tp.cache_read_per_million - 0.15).abs() < 1e-12, "cache_read_per_million");
        assert!((tp.reasoning_per_million - 6.0).abs() < 1e-12, "reasoning_per_million");
    }

    /// Absent pricing block → empty overrides (existing behavior preserved).
    #[test]
    fn pricing_overrides_absent_gives_empty() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let p = cfg.providers.iter().find(|p| p.profile_name == "myprovider").unwrap();
        assert!(
            p.pricing.overrides.is_empty(),
            "absent pricing must produce empty overrides"
        );
    }

    /// Unknown model id in the pricing block → `LlmError::InvalidRequest`.
    #[test]
    fn pricing_overrides_unknown_model_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "nonexistent-model": { "inputPerMtok": 1.0, "outputPerMtok": 2.0 }
                }
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("nonexistent-model")),
            "expected InvalidRequest naming the unknown model, got: {err:?}"
        );
    }

    /// Negative price → `LlmError::InvalidRequest`.
    #[test]
    fn pricing_overrides_negative_price_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "gpt-custom": { "inputPerMtok": -1.0, "outputPerMtok": 2.0 }
                }
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("inputPerMtok") && message.contains(">= 0")),
            "expected InvalidRequest about negative price, got: {err:?}"
        );
    }

    /// Unknown key in a model's pricing object → `LlmError::InvalidRequest` naming it.
    #[test]
    fn pricing_overrides_unknown_key_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "gpt-custom": {
                        "inputPerMtok": 1.0,
                        "outputPerMtok": 2.0,
                        "typoKey": 3.0
                    }
                }
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("typoKey")),
            "expected InvalidRequest naming the unknown key, got: {err:?}"
        );
    }

    /// Non-number price value → `LlmError::InvalidRequest`.
    #[test]
    fn pricing_overrides_non_number_price_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "gpt-custom": { "inputPerMtok": "not-a-number", "outputPerMtok": 2.0 }
                }
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("inputPerMtok") && message.contains("number")),
            "expected InvalidRequest about non-number, got: {err:?}"
        );
    }

    /// Optional fields (cacheWrite/cacheRead/reasoning) may be omitted.
    #[test]
    fn pricing_overrides_optional_fields_may_be_absent() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "myprovider": {
                "type": "openai",
                "baseUrl": "https://api.example.com/v1",
                "apiKeyEnv": "MY_API_KEY",
                "models": [{ "id": "gpt-custom" }],
                "pricing": {
                    "gpt-custom": { "inputPerMtok": 2.0, "outputPerMtok": 8.0 }
                }
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed with minimal pricing");

        let p = cfg.providers.iter().find(|p| p.profile_name == "myprovider").unwrap();
        let (_, tp) = &p.pricing.overrides[0];
        assert!((tp.input_per_million - 2.0).abs() < 1e-12);
        assert!((tp.output_per_million - 8.0).abs() < 1e-12);
        assert!((tp.cache_write_per_million - 0.0).abs() < 1e-12, "cache_write defaults to 0");
        assert!((tp.cache_read_per_million - 0.0).abs() < 1e-12, "cache_read defaults to 0");
        assert!((tp.reasoning_per_million - 0.0).abs() < 1e-12, "reasoning defaults to 0");
    }

    // ── azure-openai settings type (Task 5) tests ─────────────────────────────

    /// E2E: an azure-openai profile parses correctly, is built into a
    /// `DefaultLlmClient`, and `prepare()` produces:
    /// - A URL with `/openai/deployments/<model>/chat/completions?api-version=...`
    /// - An `api-key` header (AzureToken auth)
    /// - No `model` key in the request body
    ///
    /// Settings E2E test name: `azure_profile_prepare_url_and_api_key_header`
    #[tokio::test]
    async fn azure_profile_prepare_url_and_api_key_header() {
        std::env::set_var("PLATFORM_COMMON_TEST_AZURE_KEY", "my-azure-api-key");

        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-azure": {
                "type": "azure-openai",
                "baseUrl": "https://myresource.openai.azure.com",
                "apiKeyEnv": "PLATFORM_COMMON_TEST_AZURE_KEY",
                "apiVersion": "2024-02-01",
                "models": [
                    { "id": "gpt-4o-deployment" }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        // Verify the profile was parsed correctly.
        let azure_profile = cfg.providers.iter().find(|p| p.profile_name == "my-azure")
            .expect("my-azure profile must be present");
        assert_eq!(azure_profile.protocol, llm_client::ProtocolFamily::AzureOpenAi);
        assert_eq!(azure_profile.auth, llm_client::AuthStrategy::AzureToken);
        assert!(
            azure_profile.azure.as_ref().map(|a| a.api_version.as_str()) == Some("2024-02-01"),
            "azure config must have apiVersion=2024-02-01"
        );

        // Build a client and prepare a request.
        let client = DefaultLlmClient::from_config(cfg).expect("client must build");
        let req = llm_client::LlmRequest::new("gpt-4o-deployment");
        let prepared = client.prepare(&req).await.expect("prepare must succeed");

        // URL check: deployment pattern.
        assert!(
            prepared.provider_request.url.contains("/openai/deployments/gpt-4o-deployment/chat/completions"),
            "URL must include deployment path; got: {}",
            prepared.provider_request.url
        );
        assert!(
            prepared.provider_request.url.contains("api-version=2024-02-01"),
            "URL must include api-version; got: {}",
            prepared.provider_request.url
        );

        // Auth check: api-key header present.
        assert_eq!(
            prepared.provider_request.headers.get("api-key").map(String::as_str),
            Some("my-azure-api-key"),
            "api-key header must be injected by AzureToken auth"
        );

        // No Authorization header (Azure uses api-key, not Bearer).
        assert!(
            !prepared.provider_request.headers.contains_key("Authorization"),
            "AzureToken must NOT inject Authorization header"
        );

        // Model key must be absent from body (deployment is in the URL).
        assert!(
            prepared.provider_request.body_json.get("model").is_none(),
            "Azure request body must not include model key; got: {}",
            prepared.provider_request.body_json
        );
    }

    /// azure-openai with missing apiVersion → error naming apiVersion.
    #[test]
    fn azure_openai_missing_api_version_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-azure": {
                "type": "azure-openai",
                "baseUrl": "https://myresource.openai.azure.com",
                "apiKeyEnv": "SOME_KEY",
                "models": [{ "id": "gpt-4o" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("apiVersion")),
            "expected InvalidRequest about missing apiVersion, got: {err:?}"
        );
    }

    /// azure-openai with empty apiVersion → error.
    #[test]
    fn azure_openai_empty_api_version_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-azure": {
                "type": "azure-openai",
                "baseUrl": "https://myresource.openai.azure.com",
                "apiKeyEnv": "SOME_KEY",
                "apiVersion": "",
                "models": [{ "id": "gpt-4o" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("apiVersion")),
            "expected InvalidRequest about empty apiVersion, got: {err:?}"
        );
    }

    /// The error message for unknown type now includes both "azure-openai" and "bedrock-claude".
    #[test]
    fn unknown_type_mentions_azure_openai_in_supported_list() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "weird": {
                "type": "cohere-v2",
                "baseUrl": "https://api.cohere.ai",
                "apiKeyEnv": "COHERE_KEY",
                "models": [{ "id": "command-r" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("azure-openai")),
            "error for unknown type must list azure-openai as supported, got: {err:?}"
        );
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("bedrock-claude")),
            "error for unknown type must list bedrock-claude as supported, got: {err:?}"
        );
    }

    // ── bedrock-claude settings type tests ────────────────────────────────────

    /// A bedrock-claude profile with `region` and no `baseUrl` defaults the base
    /// URL to `https://bedrock-runtime.<region>.amazonaws.com`.
    #[test]
    fn bedrock_claude_default_base_url_from_region() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-bedrock": {
                "type": "bedrock-claude",
                "region": "us-east-1",
                "models": [{ "id": "anthropic.claude-3-5-sonnet-20241022-v2:0" }]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let bedrock = cfg.providers.iter().find(|p| p.profile_name == "my-bedrock").unwrap();
        assert_eq!(
            bedrock.base_url,
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            "base_url must default to region-derived endpoint"
        );
        assert_eq!(bedrock.protocol, ProtocolFamily::BedrockClaude);
        assert_eq!(bedrock.auth, AuthStrategy::AwsSigV4);
        assert!(
            bedrock.signing.as_ref().map(|s| (s.region.as_str(), s.service.as_str())) == Some(("us-east-1", "bedrock")),
            "signing config must have region=us-east-1 and service=bedrock; got {:?}", bedrock.signing
        );
        assert_eq!(
            bedrock.credential,
            CredentialConfig::HostManaged { id: "bedrock_sigv4".to_string() },
            "bedrock-claude must use CredentialConfig::HostManaged so the injected provider is consulted"
        );
    }

    /// A bedrock-claude profile with an explicit `baseUrl` must use that URL
    /// instead of the default region-derived endpoint.
    #[test]
    fn bedrock_claude_explicit_base_url_overrides_default() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-bedrock": {
                "type": "bedrock-claude",
                "region": "eu-west-1",
                "baseUrl": "https://custom-bedrock.example.com",
                "models": [{ "id": "anthropic.claude-3-haiku-20240307-v1:0" }]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let bedrock = cfg.providers.iter().find(|p| p.profile_name == "my-bedrock").unwrap();
        assert_eq!(
            bedrock.base_url,
            "https://custom-bedrock.example.com",
            "explicit baseUrl must override the region-derived default"
        );
        assert!(
            bedrock.signing.as_ref().map(|s| s.region.as_str()) == Some("eu-west-1"),
            "region in signing config must still come from \"region\" key"
        );
    }

    /// A bedrock-claude profile without a `region` key is rejected.
    #[test]
    fn bedrock_claude_missing_region_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-bedrock": {
                "type": "bedrock-claude",
                "models": [{ "id": "anthropic.claude-3-5-sonnet-20241022-v2:0" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("region")),
            "expected InvalidRequest about missing region, got: {err:?}"
        );
    }

    /// E2E: a `bedrock-claude` profile parsed from settings builds a
    /// [`DefaultLlmClient`], and `prepare_at` with a fixed clock produces:
    /// - URL: `{base_url}/model/{model_id}/invoke`
    /// - `x-amz-date` header present
    /// - `x-amz-content-sha256` header present
    /// - `Authorization` header starting with `AWS4-HMAC-SHA256`
    /// - No `model` key in the request body
    /// - `anthropic_version: "bedrock-2023-05-31"` in body
    ///
    /// Test name: `bedrock_claude_prepare_e2e_sigv4_headers`
    #[tokio::test]
    async fn bedrock_claude_prepare_e2e_sigv4_headers() {
        use std::sync::Arc;
        use std::time::{Duration, UNIX_EPOCH};
        use llm_client::{
            Credential, DefaultLlmClient, StaticCredentialProvider,
        };

        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-bedrock": {
                "type": "bedrock-claude",
                "region": "us-east-1",
                "models": [
                    { "id": "anthropic.claude-3-5-sonnet-20241022-v2:0", "capabilities": {"streaming": true, "tools": true} }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        // Inject static SigV4 credentials so prepare_at succeeds without
        // requiring real AWS environment variables.
        let credentials = Arc::new(StaticCredentialProvider::new(Credential::AwsSigV4 {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        }));

        let client = DefaultLlmClient::from_config(cfg)
            .expect("client must build")
            .with_credential_provider(credentials);

        // Fixed clock: 2024-01-15T12:34:56Z (Unix epoch 1705322096)
        let fixed_now = UNIX_EPOCH + Duration::from_secs(1_705_322_096);
        let req = llm_client::LlmRequest::new("anthropic.claude-3-5-sonnet-20241022-v2:0");
        let prepared = client
            .prepare_at(&req, fixed_now)
            .await
            .expect("prepare_at must succeed");

        // URL: non-streaming must use /invoke.
        assert!(
            prepared.provider_request.url.ends_with("/invoke"),
            "URL must end with /invoke; got: {}",
            prepared.provider_request.url
        );
        assert!(
            prepared.provider_request.url.contains("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            "URL must contain model id with raw ':'; got: {}",
            prepared.provider_request.url
        );

        // x-amz-date must be present and match the fixed clock.
        let amz_date = prepared.provider_request.headers
            .get("x-amz-date")
            .expect("x-amz-date header must be present");
        assert_eq!(amz_date, "20240115T123456Z", "x-amz-date must match the fixed clock");

        // x-amz-content-sha256 must be present.
        assert!(
            prepared.provider_request.headers.contains_key("x-amz-content-sha256"),
            "x-amz-content-sha256 header must be present"
        );

        // Authorization header must use AWS4-HMAC-SHA256.
        let auth = prepared.provider_request.headers
            .get("Authorization")
            .expect("Authorization header must be present");
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256"),
            "Authorization must start with AWS4-HMAC-SHA256; got: {auth}"
        );
        assert!(
            auth.contains("20240115"),
            "Authorization must contain the signing date 20240115; got: {auth}"
        );
        assert!(
            auth.contains("us-east-1/bedrock/aws4_request"),
            "Authorization must contain the credential scope; got: {auth}"
        );

        // Model key must be absent from body.
        assert!(
            prepared.provider_request.body_json.get("model").is_none(),
            "body must not contain model key; got: {}",
            prepared.provider_request.body_json
        );

        // anthropic_version must be in body.
        assert_eq!(
            prepared.provider_request.body_json.get("anthropic_version").and_then(serde_json::Value::as_str),
            Some("bedrock-2023-05-31"),
            "body must contain anthropic_version=bedrock-2023-05-31"
        );

        // No anthropic-version header.
        assert!(
            !prepared.provider_request.headers.contains_key("anthropic-version"),
            "anthropic-version header must NOT be present for Bedrock"
        );
    }

    // ── vertex-claude settings type tests ─────────────────────────────────────

    /// A `vertex-claude` profile parses correctly: `VertexClaude` protocol,
    /// `GcpToken` auth, `CredentialConfig::Env` from `apiKeyEnv`, `baseUrl` REQUIRED.
    #[test]
    fn vertex_claude_profile_parses() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-vertex-claude": {
                "type": "vertex-claude",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1",
                "apiKeyEnv": "VERTEX_BEARER_TOKEN",
                "models": [{ "id": "claude-sonnet-4@20250514" }]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let p = cfg.providers.iter().find(|p| p.profile_name == "my-vertex-claude").unwrap();
        assert_eq!(p.protocol, ProtocolFamily::VertexClaude);
        assert_eq!(p.auth, AuthStrategy::GcpToken);
        assert_eq!(
            p.credential,
            CredentialConfig::Env { var: "VERTEX_BEARER_TOKEN".to_string() },
            "vertex-claude must use CredentialConfig::Env so the token env var is consulted"
        );
        assert_eq!(
            p.base_url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1"
        );
    }

    /// `vertex-claude` with missing `baseUrl` is rejected (baseUrl is REQUIRED).
    #[test]
    fn vertex_claude_missing_base_url_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-vertex-claude": {
                "type": "vertex-claude",
                "apiKeyEnv": "VERTEX_BEARER_TOKEN",
                "models": [{ "id": "claude-sonnet-4@20250514" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("baseUrl")),
            "expected InvalidRequest about missing baseUrl, got: {err:?}"
        );
    }

    /// E2E: a `vertex-claude` profile parsed from settings builds a
    /// [`DefaultLlmClient`], and `prepare()` with a token env var produces:
    /// - URL: `{base_url}/publishers/anthropic/models/{model}:rawPredict`
    /// - `Authorization: Bearer <token>` header
    /// - No `model` key in body
    /// - `anthropic_version: "vertex-2023-10-16"` in body
    /// - No `anthropic-version` header
    ///
    /// Credential-loading choice: `CredentialConfig::Env { var }` causes
    /// `EnvCredentialProvider` to load the env var as `Credential::ApiKey(value)`.
    /// The `GcpToken` authenticate arm calls `load_secret()`, which accepts both
    /// `Credential::ApiKey` and `Credential::BearerToken` as a plain string and
    /// passes it to `BearerAuthenticator` → `Authorization: Bearer <value>`.
    #[tokio::test]
    async fn vertex_claude_prepare_e2e_bearer_header() {
        std::env::set_var("PLATFORM_COMMON_TEST_VERTEX_CLAUDE_TOKEN", "my-gcp-bearer-token");

        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-vertex-claude": {
                "type": "vertex-claude",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1",
                "apiKeyEnv": "PLATFORM_COMMON_TEST_VERTEX_CLAUDE_TOKEN",
                "models": [
                    { "id": "claude-sonnet-4@20250514", "capabilities": {"streaming": true, "tools": true} }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let client = DefaultLlmClient::from_config(cfg).expect("client must build");
        let req = llm_client::LlmRequest::new("claude-sonnet-4@20250514");
        let prepared = client.prepare(&req).await.expect("prepare must succeed");

        // URL: non-streaming must use :rawPredict.
        assert!(
            prepared.provider_request.url.ends_with(":rawPredict"),
            "URL must end with :rawPredict; got: {}",
            prepared.provider_request.url
        );
        assert!(
            prepared.provider_request.url.contains("/publishers/anthropic/models/"),
            "URL must contain /publishers/anthropic/models/; got: {}",
            prepared.provider_request.url
        );

        // Authorization: Bearer <token> must be present.
        let auth_header = prepared.provider_request.headers
            .get("Authorization")
            .expect("Authorization header must be present for GcpToken");
        assert_eq!(
            auth_header, "Bearer my-gcp-bearer-token",
            "Authorization must be Bearer <token>"
        );

        // No model key in body.
        assert!(
            prepared.provider_request.body_json.get("model").is_none(),
            "body must not contain model key; got: {}",
            prepared.provider_request.body_json
        );

        // anthropic_version: vertex-2023-10-16 in body.
        assert_eq!(
            prepared.provider_request.body_json
                .get("anthropic_version")
                .and_then(serde_json::Value::as_str),
            Some("vertex-2023-10-16"),
            "body must contain anthropic_version=vertex-2023-10-16"
        );

        // No anthropic-version header.
        assert!(
            !prepared.provider_request.headers.contains_key("anthropic-version"),
            "anthropic-version header must NOT be present for VertexClaude"
        );
    }

    // ── vertex-gemini settings type tests ─────────────────────────────────────

    /// A `vertex-gemini` profile parses correctly: `VertexGemini` protocol,
    /// `GcpToken` auth, `CredentialConfig::Env` from `apiKeyEnv`, `baseUrl` REQUIRED.
    #[test]
    fn vertex_gemini_profile_parses() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-vertex-gemini": {
                "type": "vertex-gemini",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1",
                "apiKeyEnv": "VERTEX_BEARER_TOKEN",
                "models": [{ "id": "gemini-2.0-flash" }]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let p = cfg.providers.iter().find(|p| p.profile_name == "my-vertex-gemini").unwrap();
        assert_eq!(p.protocol, ProtocolFamily::VertexGemini);
        assert_eq!(p.auth, AuthStrategy::GcpToken);
        assert_eq!(
            p.credential,
            CredentialConfig::Env { var: "VERTEX_BEARER_TOKEN".to_string() },
            "vertex-gemini must use CredentialConfig::Env so the token env var is consulted"
        );
    }

    /// E2E: a `vertex-gemini` profile parsed from settings builds a
    /// [`DefaultLlmClient`], and `prepare()` with a token env var produces:
    /// - URL: `{base_url}/publishers/google/models/{model}:generateContent`
    /// - `Authorization: Bearer <token>` header
    /// - `contents` array in body (Gemini format)
    #[tokio::test]
    async fn vertex_gemini_prepare_e2e_bearer_header() {
        std::env::set_var("PLATFORM_COMMON_TEST_VERTEX_GEMINI_TOKEN", "my-vertex-gemini-token");

        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-vertex-gemini": {
                "type": "vertex-gemini",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1",
                "apiKeyEnv": "PLATFORM_COMMON_TEST_VERTEX_GEMINI_TOKEN",
                "models": [
                    { "id": "gemini-2.0-flash", "capabilities": {"streaming": true, "tools": true} }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let client = DefaultLlmClient::from_config(cfg).expect("client must build");
        let req = llm_client::LlmRequest::new("gemini-2.0-flash");
        let prepared = client.prepare(&req).await.expect("prepare must succeed");

        // URL: non-streaming must use :generateContent.
        assert!(
            prepared.provider_request.url.ends_with(":generateContent"),
            "URL must end with :generateContent; got: {}",
            prepared.provider_request.url
        );
        assert!(
            prepared.provider_request.url.contains("/publishers/google/models/"),
            "URL must contain /publishers/google/models/; got: {}",
            prepared.provider_request.url
        );

        // Authorization: Bearer <token> must be present.
        let auth_header = prepared.provider_request.headers
            .get("Authorization")
            .expect("Authorization header must be present for GcpToken");
        assert_eq!(
            auth_header, "Bearer my-vertex-gemini-token",
            "Authorization must be Bearer <token>"
        );

        // Body must have contents array (Gemini format).
        assert!(
            prepared.provider_request.body_json.get("contents").is_some(),
            "body must have 'contents' (Gemini format); got: {}",
            prepared.provider_request.body_json
        );

        // No x-goog-api-key header (Vertex uses Bearer, not api-key).
        assert!(
            !prepared.provider_request.headers.contains_key("x-goog-api-key"),
            "x-goog-api-key must NOT be present for Vertex Gemini"
        );
    }

    /// The error message for unknown type includes `vertex-claude` and `vertex-gemini`.
    #[test]
    fn unknown_type_mentions_vertex_types_in_supported_list() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "weird": {
                "type": "cohere-v3",
                "baseUrl": "https://api.cohere.ai",
                "apiKeyEnv": "COHERE_KEY",
                "models": [{ "id": "command-r" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        let LlmError::InvalidRequest { message } = err else {
            panic!("expected InvalidRequest, got something else");
        };
        assert!(message.contains("vertex-claude"), "must list vertex-claude; got: {message}");
        assert!(message.contains("vertex-gemini"), "must list vertex-gemini; got: {message}");
    }

    // ── openai-responses settings type tests ──────────────────────────────────

    /// An `openai-responses` profile parses correctly: `OpenAiResponses`
    /// protocol, `ApiKey` auth, `CredentialConfig::Env` from `apiKeyEnv`,
    /// `baseUrl` REQUIRED (mirrors the `"openai"` type exactly).
    #[test]
    fn openai_responses_profile_parses() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-responses": {
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "models": [
                    { "id": "gpt-4o" }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        assert_eq!(cfg.providers.len(), 2, "builtin + my-responses");
        let p = cfg.providers.iter().find(|p| p.profile_name == "my-responses").unwrap();
        assert_eq!(p.base_url, "https://api.openai.com/v1");
        assert_eq!(p.protocol, ProtocolFamily::OpenAiResponses);
        assert_eq!(p.auth, AuthStrategy::ApiKey);
        assert!(
            matches!(&p.provider_id, ProviderId::OpenAICompatible { name } if name == "my-responses"),
            "provider_id must be OpenAICompatible with name=my-responses"
        );
        assert_eq!(
            p.credential,
            CredentialConfig::Env { var: "OPENAI_API_KEY".to_string() }
        );
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].display_model, "gpt-4o");
        // Default capabilities: streaming + tools, no vision/docs/reasoning.
        assert!(p.models[0].capabilities.streaming);
        assert!(p.models[0].capabilities.tools);
        assert!(!p.models[0].capabilities.vision);
        assert!(!p.models[0].capabilities.reasoning);
    }

    /// openai-responses with no baseUrl → error (same rule as `"openai"`).
    #[test]
    fn openai_responses_missing_base_url_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-responses": {
                "type": "openai-responses",
                "apiKeyEnv": "OPENAI_API_KEY",
                "models": [{ "id": "gpt-4o" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("baseUrl")),
            "expected InvalidRequest about missing baseUrl, got: {err:?}"
        );
    }

    /// openai-responses with no apiKeyEnv → error (same rule as `"openai"`).
    #[test]
    fn openai_responses_missing_api_key_env_is_error() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-responses": {
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "models": [{ "id": "gpt-4o" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        assert!(
            matches!(&err, LlmError::InvalidRequest { message } if message.contains("apiKeyEnv")),
            "expected InvalidRequest about missing apiKeyEnv, got: {err:?}"
        );
    }

    /// E2E: an `openai-responses` profile parsed from settings builds a
    /// [`DefaultLlmClient`], and `prepare()` with a key env var produces:
    /// - URL: `{base_url}/responses`, method POST
    /// - `Authorization: Bearer <key>` header (OpenAI family ApiKey auth)
    #[tokio::test]
    async fn openai_responses_prepare_e2e_responses_url_and_bearer_header() {
        std::env::set_var("PLATFORM_COMMON_TEST_OPENAI_RESPONSES_KEY", "my-responses-key");

        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "my-responses": {
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "PLATFORM_COMMON_TEST_OPENAI_RESPONSES_KEY",
                "models": [
                    { "id": "gpt-4o", "capabilities": {"streaming": true, "tools": true} }
                ]
            }
        }"#).unwrap();

        apply_settings_providers(&mut cfg, &providers, None).expect("must succeed");

        let client = DefaultLlmClient::from_config(cfg).expect("client must build");
        let req = llm_client::LlmRequest::new("gpt-4o");
        let prepared = client.prepare(&req).await.expect("prepare must succeed");

        assert_eq!(prepared.provider_request.method, "POST");
        assert_eq!(
            prepared.provider_request.url, "https://api.openai.com/v1/responses",
            "URL must be {{baseUrl}}/responses"
        );

        let auth_header = prepared.provider_request.headers
            .get("Authorization")
            .expect("Authorization header must be present for ApiKey auth");
        assert_eq!(
            auth_header, "Bearer my-responses-key",
            "Authorization must be Bearer <key>"
        );
    }

    /// The error message for unknown type includes `openai-responses`.
    #[test]
    fn unknown_type_mentions_openai_responses_in_supported_list() {
        let mut cfg = builtin_anthropic_config("https://api.anthropic.com", false);
        let providers: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{
            "weird": {
                "type": "cohere-v4",
                "baseUrl": "https://api.cohere.ai",
                "apiKeyEnv": "COHERE_KEY",
                "models": [{ "id": "command-r" }]
            }
        }"#).unwrap();

        let err = apply_settings_providers(&mut cfg, &providers, None).unwrap_err();
        let LlmError::InvalidRequest { message } = err else {
            panic!("expected InvalidRequest, got something else");
        };
        assert!(
            message.contains("openai-responses"),
            "must list openai-responses; got: {message}"
        );
    }
}
