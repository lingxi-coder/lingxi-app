//! Shared provider settings parsing and provider identity helpers.
//!
//! This module owns the multi-provider semantics that used to be duplicated in
//! host/platform crates: settings provider kinds, provider profile construction,
//! credential handling mode, model capability parsing, pricing overrides, and
//! model-string/provider identity helpers.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::{
    AuthStrategy, AzureConfig, Capabilities, CredentialConfig, LlmError, ModelProfile,
    PricingConfig, ProtocolFamily, ProviderId, ProviderProfile, SigningConfig, TokenPricing,
};

const SUPPORTED_PROVIDER_TYPES: &str =
    "openai, openai-responses, anthropic, gemini, azure-openai, bedrock-claude, vertex-claude, vertex-gemini";

/// Provider kinds accepted in `settings.providers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OpenAI-compatible Chat Completions wire.
    OpenAi,
    /// OpenAI Responses wire.
    OpenAiResponses,
    /// Anthropic Messages wire.
    Anthropic,
    /// Gemini first-party generateContent wire.
    Gemini,
    /// Azure OpenAI deployments wire.
    AzureOpenAi,
    /// Anthropic Claude on AWS Bedrock.
    BedrockClaude,
    /// Anthropic Claude on Vertex AI.
    VertexClaude,
    /// Gemini on Vertex AI.
    VertexGemini,
}

impl ProviderKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(Self::OpenAi),
            "openai-responses" => Some(Self::OpenAiResponses),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "azure-openai" => Some(Self::AzureOpenAi),
            "bedrock-claude" => Some(Self::BedrockClaude),
            "vertex-claude" => Some(Self::VertexClaude),
            "vertex-gemini" => Some(Self::VertexGemini),
            _ => None,
        }
    }

    fn protocol(self) -> ProtocolFamily {
        match self {
            Self::OpenAi => ProtocolFamily::OpenAiChat,
            Self::OpenAiResponses => ProtocolFamily::OpenAiResponses,
            Self::Anthropic => ProtocolFamily::AnthropicMessages,
            Self::Gemini => ProtocolFamily::GeminiGenerateContent,
            Self::AzureOpenAi => ProtocolFamily::AzureOpenAi,
            Self::BedrockClaude => ProtocolFamily::BedrockClaude,
            Self::VertexClaude => ProtocolFamily::VertexClaude,
            Self::VertexGemini => ProtocolFamily::VertexGemini,
        }
    }

    fn auth(self) -> AuthStrategy {
        match self {
            Self::AzureOpenAi => AuthStrategy::AzureToken,
            Self::BedrockClaude => AuthStrategy::AwsSigV4,
            Self::VertexClaude | Self::VertexGemini => AuthStrategy::GcpToken,
            Self::OpenAi | Self::OpenAiResponses | Self::Anthropic | Self::Gemini => {
                AuthStrategy::ApiKey
            }
        }
    }

    fn provider_id(self, profile_name: &str) -> ProviderId {
        match self {
            Self::OpenAi | Self::OpenAiResponses => ProviderId::OpenAICompatible {
                name: profile_name.to_string(),
            },
            Self::Anthropic => ProviderId::AnthropicFirstParty,
            Self::Gemini => ProviderId::Gemini,
            Self::AzureOpenAi => ProviderId::AzureOpenAI,
            Self::BedrockClaude => ProviderId::BedrockClaude,
            Self::VertexClaude => ProviderId::VertexClaude,
            Self::VertexGemini => ProviderId::VertexGemini,
        }
    }

    fn requires_base_url(self) -> bool {
        !matches!(self, Self::BedrockClaude)
    }

    fn requires_api_key_env(self) -> bool {
        !matches!(self, Self::BedrockClaude)
    }
}

/// Credential behavior for parsed user provider profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialMode {
    /// Resolve provider credentials from the configured environment variable.
    Env,
    /// Retain `apiKeyEnv` as metadata and leave the profile credential deferred.
    DeferredStatic,
}

/// Options controlling provider settings parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderParseOptions {
    /// How API-key credentials should be represented in the returned profile.
    pub credential_mode: ProviderCredentialMode,
    /// Whether `models` must be present and non-empty.
    pub models_required: bool,
}

impl ProviderParseOptions {
    /// Strict platform parser behavior: credentials come from env and models are required.
    #[must_use]
    pub const fn strict_env() -> Self {
        Self {
            credential_mode: ProviderCredentialMode::Env,
            models_required: true,
        }
    }

    /// Lenient provider-config behavior: credentials are deferred and models may be absent.
    #[must_use]
    pub const fn lenient_deferred_static() -> Self {
        Self {
            credential_mode: ProviderCredentialMode::DeferredStatic,
            models_required: false,
        }
    }
}

/// A parsed user profile plus the original API-key env var, when one exists.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUserProvider {
    /// Built provider profile.
    pub profile: ProviderProfile,
    /// `apiKeyEnv` captured for host credential metadata.
    pub env_var: Option<String>,
}

/// Strictly parse `settings.providers` into provider profiles.
///
/// # Errors
///
/// Returns [`LlmError::InvalidRequest`] for the first invalid provider or model
/// entry.
pub fn parse_provider_profiles_strict(
    providers: &BTreeMap<String, Value>,
    options: ProviderParseOptions,
) -> Result<Vec<ParsedUserProvider>, LlmError> {
    let mut out = Vec::with_capacity(providers.len());
    for (name, value) in providers {
        let parsed = parse_one_provider(name, value, options, true)
            .map_err(|message| LlmError::InvalidRequest { message })?;
        out.push(parsed.provider);
    }
    Ok(out)
}

/// Leniently parse `settings.providers`, skipping invalid providers and
/// returning warning strings instead of errors.
#[must_use]
pub fn parse_provider_profiles_lenient(
    providers: &BTreeMap<String, Value>,
    options: ProviderParseOptions,
) -> (Vec<ParsedUserProvider>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (name, value) in providers {
        match parse_one_provider(name, value, options, false) {
            Ok(parsed) => {
                warnings.extend(parsed.warnings);
                out.push(parsed.provider);
            }
            Err(message) => warnings.push(format!("{message}; skipped")),
        }
    }
    (out, warnings)
}

#[derive(Debug)]
struct ProviderParseResult {
    provider: ParsedUserProvider,
    warnings: Vec<String>,
}

#[allow(clippy::too_many_lines)]
fn parse_one_provider(
    name: &str,
    value: &Value,
    options: ProviderParseOptions,
    strict: bool,
) -> Result<ProviderParseResult, String> {
    let mut warnings = Vec::new();
    let obj = value
        .as_object()
        .ok_or_else(|| format!("provider {name:?}: entry is not an object"))?;

    let type_str = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("provider {name:?}: missing string \"type\""))?;
    let kind = ProviderKind::parse(type_str).ok_or_else(|| {
        format!(
            "provider {name:?}: unknown type {type_str:?} (supported: {SUPPORTED_PROVIDER_TYPES})"
        )
    })?;
    let protocol = kind.protocol();
    let auth = kind.auth();

    let supports_websockets =
        parse_optional_bool(obj, name, "supportsWebsockets")?.unwrap_or(false);
    let supports_websocket_compression =
        parse_optional_bool(obj, name, "supportsWebsocketCompression")?.unwrap_or(false);
    let websocket_connect_timeout_ms = parse_optional_u64(obj, name, "websocketConnectTimeoutMs")?;

    if supports_websockets && matches!(kind, ProviderKind::BedrockClaude) {
        return Err(format!(
            "provider {name:?}: supportsWebsockets is not supported for AWS SigV4/Bedrock providers"
        ));
    }
    if supports_websockets && !matches!(protocol, ProtocolFamily::OpenAiResponses) {
        return Err(format!(
            "provider {name:?}: supportsWebsockets is only valid for openai-responses providers"
        ));
    }
    if supports_websocket_compression {
        return Err(format!(
            "provider {name:?}: supportsWebsocketCompression is not supported by this build"
        ));
    }

    let (base_url, signing) = if matches!(kind, ProviderKind::BedrockClaude) {
        let region = required_non_empty_string(obj, name, "region", Some("bedrock-claude"))?;
        let base_url = obj
            .get("baseUrl")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(
                || format!("https://bedrock-runtime.{region}.amazonaws.com"),
                str::to_string,
            );
        (
            base_url,
            Some(SigningConfig {
                region,
                service: "bedrock".to_string(),
            }),
        )
    } else if kind.requires_base_url() {
        (required_non_empty_string(obj, name, "baseUrl", None)?, None)
    } else {
        unreachable!("all non-bedrock provider kinds require baseUrl")
    };

    let azure = if matches!(kind, ProviderKind::AzureOpenAi) {
        Some(AzureConfig {
            api_version: required_non_empty_string(obj, name, "apiVersion", Some("azure-openai"))?,
        })
    } else {
        None
    };

    let env_var = obj
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if kind.requires_api_key_env() && env_var.is_none() {
        return Err(format!(
            "provider {name:?}: \"apiKeyEnv\" is required and must not be empty"
        ));
    }

    let credential = if matches!(kind, ProviderKind::BedrockClaude) {
        CredentialConfig::HostManaged {
            id: "bedrock_sigv4".to_string(),
        }
    } else {
        match options.credential_mode {
            ProviderCredentialMode::Env => CredentialConfig::Env {
                var: env_var.clone().expect("apiKeyEnv was validated above"),
            },
            ProviderCredentialMode::DeferredStatic => CredentialConfig::None,
        }
    };

    let models = parse_models(
        obj.get("models"),
        name,
        options.models_required,
        strict,
        &mut warnings,
    )?;
    let pricing = if let Some(pricing_val) = obj.get("pricing") {
        parse_pricing_overrides(name, pricing_val, &models)?
    } else {
        PricingConfig::default()
    };

    Ok(ProviderParseResult {
        provider: ParsedUserProvider {
            profile: ProviderProfile {
                provider_id: kind.provider_id(name),
                profile_name: name.to_string(),
                base_url,
                protocol,
                auth,
                credential,
                models,
                pricing,
                signing,
                azure,
                supports_websockets,
                supports_websocket_compression,
                websocket_connect_timeout_ms,
            },
            env_var,
        },
        warnings,
    })
}

fn parse_optional_bool(
    obj: &Map<String, Value>,
    provider_name: &str,
    key: &str,
) -> Result<Option<bool>, String> {
    obj.get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("provider {provider_name:?}: \"{key}\" must be a boolean"))
        })
        .transpose()
}

fn parse_optional_u64(
    obj: &Map<String, Value>,
    provider_name: &str,
    key: &str,
) -> Result<Option<u64>, String> {
    obj.get(key)
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                format!("provider {provider_name:?}: \"{key}\" must be an unsigned integer")
            })
        })
        .transpose()
}

fn required_non_empty_string(
    obj: &Map<String, Value>,
    provider_name: &str,
    key: &str,
    kind_hint: Option<&str>,
) -> Result<String, String> {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| match kind_hint {
            Some(kind) => {
                format!("provider {provider_name:?}: missing non-empty \"{key}\" for {kind}")
            }
            None => {
                format!("provider {provider_name:?}: \"{key}\" is required and must not be empty")
            }
        })
}

fn parse_models(
    value: Option<&Value>,
    provider_name: &str,
    models_required: bool,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<ModelProfile>, String> {
    let Some(value) = value else {
        if models_required {
            return Err(format!(
                "provider {provider_name:?}: \"models\" is required (no auto-discovery)"
            ));
        }
        return Ok(Vec::new());
    };

    let Some(models) = value.as_array() else {
        if strict {
            return Err(format!(
                "provider {provider_name:?}: \"models\" must be an array"
            ));
        }
        warnings.push(format!(
            "provider {provider_name:?}: \"models\" must be an array; treating as empty"
        ));
        return Ok(Vec::new());
    };

    if models_required && models.is_empty() {
        return Err(format!(
            "provider {provider_name:?}: \"models\" must have at least one entry"
        ));
    }

    let mut parsed = Vec::with_capacity(models.len());
    for model in models {
        match parse_model_entry(provider_name, model) {
            Ok(profile) => parsed.push(profile),
            Err(message) if strict => return Err(message),
            Err(message) => warnings.push(format!("{message}; skipped")),
        }
    }

    if models_required && parsed.is_empty() {
        return Err(format!(
            "provider {provider_name:?}: \"models\" must have at least one entry"
        ));
    }

    Ok(parsed)
}

fn parse_model_entry(provider_name: &str, value: &Value) -> Result<ModelProfile, String> {
    match value {
        Value::String(id) => Ok(ModelProfile {
            display_model: id.clone(),
            request_model: id.clone(),
            billing_model: id.clone(),
            aliases: Vec::new(),
            description: None,
            capabilities: permissive_caps(),
        }),
        Value::Object(obj) => {
            let id = obj.get("id").and_then(Value::as_str).ok_or_else(|| {
                format!("provider {provider_name:?}: model entry missing string \"id\"")
            })?;
            let aliases = obj
                .get("aliases")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|alias| alias.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Ok(ModelProfile {
                display_model: id.to_string(),
                request_model: id.to_string(),
                billing_model: id.to_string(),
                aliases,
                description: None,
                capabilities: parse_capabilities(obj.get("capabilities")),
            })
        }
        _ => Err(format!(
            "provider {provider_name:?}: model entry is not a string or object"
        )),
    }
}

fn permissive_caps() -> Capabilities {
    Capabilities {
        streaming: true,
        tools: true,
        vision: true,
        documents: true,
        reasoning: true,
        structured_output: true,
    }
}

fn parse_capabilities(caps_val: Option<&Value>) -> Capabilities {
    let Some(caps_val) = caps_val else {
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
            .and_then(Value::as_bool)
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

fn parse_pricing_overrides(
    profile_name: &str,
    pricing_val: &Value,
    model_profiles: &[ModelProfile],
) -> Result<PricingConfig, String> {
    const KNOWN_PRICING_KEYS: &[&str] = &[
        "inputPerMtok",
        "outputPerMtok",
        "cacheWritePerMtok",
        "cacheReadPerMtok",
        "reasoningPerMtok",
    ];

    let pricing_obj = pricing_val.as_object().ok_or_else(|| {
        format!(
            "provider {profile_name:?}: \"pricing\" must be an object (got {})",
            json_type_name(pricing_val)
        )
    })?;

    let known_models: BTreeSet<&str> = model_profiles
        .iter()
        .map(|model| model.display_model.as_str())
        .collect();
    let mut overrides = Vec::with_capacity(pricing_obj.len());

    for (model_id, model_pricing_val) in pricing_obj {
        if !known_models.contains(model_id.as_str()) {
            return Err(format!(
                "provider {profile_name:?}: pricing key {model_id:?} is not in the models list - add the model first or remove the override"
            ));
        }

        let model_pricing_obj = model_pricing_val.as_object().ok_or_else(|| {
            format!("provider {profile_name:?}: pricing[{model_id:?}] must be an object")
        })?;

        for key in model_pricing_obj.keys() {
            if !KNOWN_PRICING_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "provider {profile_name:?}: pricing[{model_id:?}] unknown key {key:?} (known: inputPerMtok, outputPerMtok, cacheWritePerMtok, cacheReadPerMtok, reasoningPerMtok)"
                ));
            }
        }

        let input_per_million = parse_price_field(
            profile_name,
            model_id,
            model_pricing_obj,
            "inputPerMtok",
            true,
        )?
        .unwrap_or(0.0);
        let output_per_million = parse_price_field(
            profile_name,
            model_id,
            model_pricing_obj,
            "outputPerMtok",
            true,
        )?
        .unwrap_or(0.0);
        let cache_write_per_million = parse_price_field(
            profile_name,
            model_id,
            model_pricing_obj,
            "cacheWritePerMtok",
            false,
        )?
        .unwrap_or(0.0);
        let cache_read_per_million = parse_price_field(
            profile_name,
            model_id,
            model_pricing_obj,
            "cacheReadPerMtok",
            false,
        )?
        .unwrap_or(0.0);
        let reasoning_per_million = parse_price_field(
            profile_name,
            model_id,
            model_pricing_obj,
            "reasoningPerMtok",
            false,
        )?
        .unwrap_or(0.0);

        overrides.push((
            model_id.clone(),
            TokenPricing {
                input_per_million,
                output_per_million,
                cache_write_per_million,
                cache_read_per_million,
                reasoning_per_million,
            },
        ));
    }

    Ok(PricingConfig {
        require_priced: false,
        overrides,
    })
}

fn parse_price_field(
    profile_name: &str,
    model_id: &str,
    obj: &Map<String, Value>,
    key: &str,
    required: bool,
) -> Result<Option<f64>, String> {
    match obj.get(key) {
        None if required => Err(format!(
            "provider {profile_name:?}: pricing[{model_id:?}] missing required field {key:?}"
        )),
        None => Ok(None),
        Some(val) => {
            let v = val.as_f64().ok_or_else(|| {
                format!(
                    "provider {profile_name:?}: pricing[{model_id:?}].{key} must be a number, got {val}"
                )
            })?;
            if v < 0.0 {
                return Err(format!(
                    "provider {profile_name:?}: pricing[{model_id:?}].{key} must be >= 0 (got {v})"
                ));
            }
            Ok(Some(v))
        }
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "array",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Null => "null",
        Value::Object(_) => "object",
    }
}

/// Built-in Anthropic Claude model profiles used by desktop/mobile hosts.
#[must_use]
pub fn anthropic_model_profiles() -> Vec<ModelProfile> {
    fn model(display: &str, billing: &str, aliases: &[&str], reasoning: bool) -> ModelProfile {
        ModelProfile {
            display_model: display.to_string(),
            request_model: display.to_string(),
            billing_model: billing.to_string(),
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            description: None,
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

    vec![
        model(
            "claude-sonnet-4-20250514",
            "claude-sonnet-4",
            &["claude-sonnet-4", "claude-sonnet", "claude"],
            true,
        ),
        model(
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-5",
            &["claude-sonnet-4-5"],
            true,
        ),
        model("claude-sonnet-4-6", "claude-sonnet-4-6", &[], true),
        model(
            "claude-opus-4-20250514",
            "claude-opus-4",
            &["claude-opus-4", "claude-opus"],
            true,
        ),
        model(
            "claude-opus-4-1-20250805",
            "claude-opus-4-1",
            &["claude-opus-4-1"],
            true,
        ),
        model(
            "claude-opus-4-5-20251101",
            "claude-opus-4-5",
            &["claude-opus-4-5"],
            true,
        ),
        model("claude-opus-4-6", "claude-opus-4-6", &[], true),
        model("claude-opus-4-7", "claude-opus-4-7", &[], true),
        model(
            "claude-haiku-4-20250307",
            "claude-haiku-4",
            &["claude-haiku-4", "claude-haiku"],
            true,
        ),
        model("claude-haiku-4-5", "claude-haiku-4-5", &[], true),
    ]
}

/// Construct the built-in Anthropic provider profile with caller-selected auth.
#[must_use]
pub fn anthropic_provider_profile(
    api_base: &str,
    auth: AuthStrategy,
    credential: CredentialConfig,
) -> ProviderProfile {
    ProviderProfile {
        provider_id: ProviderId::AnthropicFirstParty,
        profile_name: "anthropic".to_string(),
        base_url: api_base.to_string(),
        protocol: ProtocolFamily::AnthropicMessages,
        auth,
        credential,
        models: anthropic_model_profiles(),
        pricing: PricingConfig::default(),
        signing: None,
        azure: None,
        supports_websockets: false,
        supports_websocket_compression: false,
        websocket_connect_timeout_ms: None,
    }
}

/// Split a user model reference into `(profile, model)` with Claude Code
/// back-compat semantics.
#[must_use]
pub fn split_profile_model(model_ref: &str) -> (String, String) {
    if model_ref.starts_with("claude-") {
        return ("anthropic".to_string(), model_ref.to_string());
    }
    match model_ref.split_once('/') {
        Some((profile, bare)) if !profile.is_empty() && !bare.is_empty() => {
            (profile.to_string(), bare.to_string())
        }
        _ => ("anthropic".to_string(), model_ref.to_string()),
    }
}

/// Normalize a resolved provider profile to the pricing provider identity used
/// by shared pricing catalogs.
#[must_use]
pub fn pricing_provider_id_for_profile(profile_name: &str, provider_id: &ProviderId) -> ProviderId {
    match profile_name {
        "anthropic" => ProviderId::AnthropicFirstParty,
        "openai" | "azure" => ProviderId::OpenAI,
        "gemini" | "vertex" => ProviderId::Gemini,
        "bedrock" => ProviderId::BedrockClaude,
        _ => match provider_id {
            ProviderId::AzureOpenAI | ProviderId::OpenAI => ProviderId::OpenAI,
            ProviderId::VertexGemini | ProviderId::VertexClaude | ProviderId::Gemini => {
                ProviderId::Gemini
            }
            ProviderId::BedrockClaude => ProviderId::BedrockClaude,
            ProviderId::AnthropicFirstParty => ProviderId::AnthropicFirstParty,
            ProviderId::OpenAICompatible { name } => {
                ProviderId::OpenAICompatible { name: name.clone() }
            }
            ProviderId::Custom { name } => ProviderId::Custom { name: name.clone() },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(name: &str, value: Value) -> BTreeMap<String, Value> {
        let mut providers = BTreeMap::new();
        providers.insert(name.to_string(), value);
        providers
    }

    #[test]
    fn strict_parser_covers_all_provider_kinds_with_explicit_identities() {
        let providers: BTreeMap<String, Value> = serde_json::from_value(json!({
            "openai_chat": {
                "type": "openai",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "models": [{"id": "gpt-4o"}]
            },
            "openai_responses": {
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "models": [{"id": "gpt-5"}]
            },
            "anthropic_user": {
                "type": "anthropic",
                "baseUrl": "https://api.anthropic.com",
                "apiKeyEnv": "ANTHROPIC_API_KEY",
                "models": [{"id": "claude-sonnet-4-20250514"}]
            },
            "gemini_user": {
                "type": "gemini",
                "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
                "apiKeyEnv": "GEMINI_API_KEY",
                "models": [{"id": "gemini-2.5-pro"}]
            },
            "azure_user": {
                "type": "azure-openai",
                "baseUrl": "https://example.openai.azure.com",
                "apiKeyEnv": "AZURE_OPENAI_API_KEY",
                "apiVersion": "2024-02-01",
                "models": [{"id": "gpt-4o"}]
            },
            "bedrock_user": {
                "type": "bedrock-claude",
                "region": "us-east-1",
                "models": [{"id": "anthropic.claude-3-5-sonnet-20241022-v2:0"}]
            },
            "vertex_claude_user": {
                "type": "vertex-claude",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1",
                "apiKeyEnv": "GOOGLE_OAUTH_TOKEN",
                "models": [{"id": "claude-sonnet-4@20250514"}]
            },
            "vertex_gemini_user": {
                "type": "vertex-gemini",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1",
                "apiKeyEnv": "GOOGLE_OAUTH_TOKEN",
                "models": [{"id": "gemini-2.5-pro"}]
            }
        }))
        .unwrap();

        let parsed = parse_provider_profiles_strict(&providers, ProviderParseOptions::strict_env())
            .expect("providers parse");
        let by_name: BTreeMap<_, _> = parsed
            .iter()
            .map(|p| (p.profile.profile_name.as_str(), &p.profile))
            .collect();

        assert_eq!(
            by_name["openai_chat"].provider_id,
            ProviderId::OpenAICompatible {
                name: "openai_chat".to_string()
            }
        );
        assert_eq!(
            by_name["openai_responses"].protocol,
            ProtocolFamily::OpenAiResponses
        );
        assert_eq!(
            by_name["anthropic_user"].provider_id,
            ProviderId::AnthropicFirstParty
        );
        assert_eq!(by_name["gemini_user"].provider_id, ProviderId::Gemini);
        assert_eq!(by_name["azure_user"].provider_id, ProviderId::AzureOpenAI);
        assert_eq!(
            by_name["bedrock_user"].provider_id,
            ProviderId::BedrockClaude
        );
        assert_eq!(
            by_name["vertex_claude_user"].provider_id,
            ProviderId::VertexClaude
        );
        assert_eq!(
            by_name["vertex_gemini_user"].provider_id,
            ProviderId::VertexGemini
        );
        assert_eq!(by_name["azure_user"].auth, AuthStrategy::AzureToken);
        assert_eq!(by_name["bedrock_user"].auth, AuthStrategy::AwsSigV4);
        assert_eq!(
            by_name["bedrock_user"].base_url,
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        );
        assert_eq!(
            by_name["bedrock_user"]
                .signing
                .as_ref()
                .map(|s| s.service.as_str()),
            Some("bedrock")
        );
    }

    #[test]
    fn lenient_parser_skips_invalid_provider_with_warning() {
        let providers = one("bad", json!({ "type": "cohere", "baseUrl": "https://x" }));

        let (parsed, warnings) = parse_provider_profiles_lenient(
            &providers,
            ProviderParseOptions::lenient_deferred_static(),
        );

        assert!(parsed.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown type"));
        assert!(warnings[0].contains("skipped"));
    }

    #[test]
    fn strict_parser_returns_invalid_request() {
        let providers = one("bad", json!({ "type": "openai", "baseUrl": "https://x" }));

        let err = parse_provider_profiles_strict(&providers, ProviderParseOptions::strict_env())
            .expect_err("missing apiKeyEnv should error");

        assert!(
            matches!(err, LlmError::InvalidRequest { ref message } if message.contains("apiKeyEnv")),
            "got {err:?}"
        );
    }

    #[test]
    fn credential_modes_switch_between_env_and_deferred() {
        let providers = one(
            "groq",
            json!({
                "type": "openai",
                "baseUrl": "https://api.groq.com/openai/v1",
                "apiKeyEnv": "GROQ_API_KEY",
                "models": [{"id": "llama-3.3-70b"}]
            }),
        );

        let strict = parse_provider_profiles_strict(&providers, ProviderParseOptions::strict_env())
            .expect("strict parse");
        assert_eq!(
            strict[0].profile.credential,
            CredentialConfig::Env {
                var: "GROQ_API_KEY".to_string()
            }
        );

        let (lenient, warnings) = parse_provider_profiles_lenient(
            &providers,
            ProviderParseOptions::lenient_deferred_static(),
        );
        assert!(warnings.is_empty());
        assert_eq!(lenient[0].profile.credential, CredentialConfig::None);
        assert_eq!(lenient[0].env_var.as_deref(), Some("GROQ_API_KEY"));
    }

    #[test]
    fn model_less_lenient_entry_is_preserved_for_assemble_to_drop() {
        let providers = one(
            "listing",
            json!({
                "type": "openai",
                "baseUrl": "https://x",
                "apiKeyEnv": "OPENAI_API_KEY"
            }),
        );

        let (parsed, warnings) = parse_provider_profiles_lenient(
            &providers,
            ProviderParseOptions::lenient_deferred_static(),
        );

        assert!(warnings.is_empty());
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].profile.models.is_empty());
    }

    #[test]
    fn websocket_flags_only_parse_for_openai_responses() {
        let providers = one(
            "responses",
            json!({
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "websocketConnectTimeoutMs": 1500,
                "models": [{"id": "gpt-5"}]
            }),
        );

        let parsed = parse_provider_profiles_strict(&providers, ProviderParseOptions::strict_env())
            .expect("providers parse");

        assert!(parsed[0].profile.supports_websockets);
        assert_eq!(parsed[0].profile.websocket_connect_timeout_ms, Some(1500));
    }

    #[test]
    fn websocket_flags_reject_invalid_protocols_and_compression() {
        let chat = one(
            "chat",
            json!({
                "type": "openai",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "models": [{"id": "gpt-4o"}]
            }),
        );
        let err = parse_provider_profiles_strict(&chat, ProviderParseOptions::strict_env())
            .expect_err("chat websocket should reject");
        assert!(
            matches!(err, LlmError::InvalidRequest { ref message } if message.contains("openai-responses"))
        );

        let compression = one(
            "responses",
            json!({
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "supportsWebsocketCompression": true,
                "models": [{"id": "gpt-5"}]
            }),
        );
        let err = parse_provider_profiles_strict(&compression, ProviderParseOptions::strict_env())
            .expect_err("compression should reject");
        assert!(
            matches!(err, LlmError::InvalidRequest { ref message } if message.contains("not supported"))
        );
    }

    #[test]
    fn pricing_overrides_parse_and_validate() {
        let providers = one(
            "groq",
            json!({
                "type": "openai",
                "baseUrl": "https://api.groq.com/openai/v1",
                "apiKeyEnv": "GROQ_API_KEY",
                "models": [{"id": "llama"}],
                "pricing": {
                    "llama": {
                        "inputPerMtok": 1.0,
                        "outputPerMtok": 2.0,
                        "cacheReadPerMtok": 0.25
                    }
                }
            }),
        );

        let parsed = parse_provider_profiles_strict(&providers, ProviderParseOptions::strict_env())
            .expect("providers parse");

        assert_eq!(parsed[0].profile.pricing.overrides.len(), 1);
        let (_, pricing) = &parsed[0].profile.pricing.overrides[0];
        assert_eq!(pricing.input_per_million, 1.0);
        assert_eq!(pricing.output_per_million, 2.0);
        assert_eq!(pricing.cache_read_per_million, 0.25);
    }

    #[test]
    fn anthropic_profile_helper_preserves_model_table() {
        let profile = anthropic_provider_profile(
            "https://api.anthropic.com",
            AuthStrategy::ApiKey,
            CredentialConfig::Env {
                var: "ANTHROPIC_API_KEY".to_string(),
            },
        );

        assert_eq!(profile.profile_name, "anthropic");
        assert_eq!(profile.models.len(), 10);
        assert!(profile
            .models
            .iter()
            .any(|m| m.display_model == "claude-opus-4-7"));
        assert!(profile.models.iter().all(|m| m.capabilities.reasoning));
    }

    #[test]
    fn split_profile_model_matches_claude_code_backcompat() {
        assert_eq!(
            split_profile_model("openai/gpt-4o"),
            ("openai".to_string(), "gpt-4o".to_string())
        );
        assert_eq!(
            split_profile_model("claude-3-5/sonnet"),
            ("anthropic".to_string(), "claude-3-5/sonnet".to_string())
        );
        assert_eq!(
            split_profile_model("some-model"),
            ("anthropic".to_string(), "some-model".to_string())
        );
    }

    #[test]
    fn pricing_provider_helper_normalizes_managed_clouds() {
        assert_eq!(
            pricing_provider_id_for_profile("azure-user", &ProviderId::AzureOpenAI),
            ProviderId::OpenAI
        );
        assert_eq!(
            pricing_provider_id_for_profile("vertex-claude-user", &ProviderId::VertexClaude),
            ProviderId::Gemini
        );
        assert_eq!(
            pricing_provider_id_for_profile("bedrock-user", &ProviderId::BedrockClaude),
            ProviderId::BedrockClaude
        );
        assert_eq!(
            pricing_provider_id_for_profile(
                "groq",
                &ProviderId::OpenAICompatible {
                    name: "groq".to_string()
                }
            ),
            ProviderId::OpenAICompatible {
                name: "groq".to_string()
            }
        );
    }
}
