//! Parse `settings.providers` into `llm_client::ProviderProfile`s (spec §5.1).
//! Supports the same provider kinds as `platform-common` while preserving this
//! crate's skip+warn tolerant behavior for bad entries.

use std::collections::BTreeMap;

use llm_client::{
    AuthStrategy, AzureConfig, Capabilities, CredentialConfig, ModelProfile, PricingConfig,
    ProtocolFamily, ProviderId, ProviderProfile, SigningConfig,
};
use serde_json::Value;

/// A parsed user profile plus the `apiKeyEnv` recorded for its credential source.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUserProvider {
    /// The built provider profile. API-key providers start with
    /// [`CredentialConfig::None`] and `assemble` rewrites them to `Static{id}`;
    /// host-managed providers such as Bedrock keep their credential marker.
    pub profile: ProviderProfile,
    /// The `apiKeyEnv` env fallback recorded by `assemble` into `CredentialSource`.
    pub env_var: Option<String>,
}

/// Parse `settings.providers` into profiles + warnings (spec §5.1). Unknown
/// `type` / missing required fields skips the entry with a warning. A model-less
/// entry parses with no models; `assemble` decides routability (it drops + warns).
#[must_use]
pub fn parse_user_providers(
    providers: &BTreeMap<String, Value>,
) -> (Vec<ParsedUserProvider>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for (name, value) in providers {
        let Some(obj) = value.as_object() else {
            warnings.push(format!(
                "provider {name:?}: entry is not an object; skipped"
            ));
            continue;
        };
        let Some(type_str) = obj.get("type").and_then(Value::as_str) else {
            warnings.push(format!(
                "provider {name:?}: missing string \"type\"; skipped"
            ));
            continue;
        };
        let (protocol, auth, provider_id) = match type_str {
            "openai" => (
                ProtocolFamily::OpenAiChat,
                AuthStrategy::ApiKey,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "openai-responses" => (
                ProtocolFamily::OpenAiResponses,
                AuthStrategy::ApiKey,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "anthropic" => (
                ProtocolFamily::AnthropicMessages,
                AuthStrategy::ApiKey,
                ProviderId::AnthropicFirstParty,
            ),
            "gemini" => (
                ProtocolFamily::GeminiGenerateContent,
                AuthStrategy::ApiKey,
                ProviderId::Gemini,
            ),
            "azure-openai" => (
                ProtocolFamily::AzureOpenAi,
                AuthStrategy::AzureToken,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "bedrock-claude" => (
                ProtocolFamily::BedrockClaude,
                AuthStrategy::AwsSigV4,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "vertex-claude" => (
                ProtocolFamily::VertexClaude,
                AuthStrategy::GcpToken,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "vertex-gemini" => (
                ProtocolFamily::VertexGemini,
                AuthStrategy::GcpToken,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            other => {
                warnings.push(format!(
                    "provider {name:?}: unknown type {other:?} (expected openai|openai-responses|anthropic|gemini|azure-openai|bedrock-claude|vertex-claude|vertex-gemini); skipped"
                ));
                continue;
            }
        };

        let supports_websockets = match obj.get("supportsWebsockets") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                warnings.push(format!(
                    "provider {name:?}: \"supportsWebsockets\" must be a boolean; skipped"
                ));
                continue;
            }
        };
        let supports_websocket_compression = match obj.get("supportsWebsocketCompression") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                warnings.push(format!(
                    "provider {name:?}: \"supportsWebsocketCompression\" must be a boolean; skipped"
                ));
                continue;
            }
        };
        let websocket_connect_timeout_ms = match obj.get("websocketConnectTimeoutMs") {
            None => None,
            Some(value) => match value.as_u64() {
                Some(ms) => Some(ms),
                None => {
                    warnings.push(format!(
                        "provider {name:?}: \"websocketConnectTimeoutMs\" must be an unsigned integer; skipped"
                    ));
                    continue;
                }
            },
        };
        if supports_websockets && matches!(auth, AuthStrategy::AwsSigV4) {
            warnings.push(format!(
                "provider {name:?}: supportsWebsockets is not supported for AWS SigV4/Bedrock providers; skipped"
            ));
            continue;
        }
        if supports_websockets && !matches!(protocol, ProtocolFamily::OpenAiResponses) {
            warnings.push(format!(
                "provider {name:?}: supportsWebsockets is only valid for openai-responses providers; skipped"
            ));
            continue;
        }
        if supports_websocket_compression {
            warnings.push(format!(
                "provider {name:?}: supportsWebsocketCompression is not supported by this build; skipped"
            ));
            continue;
        }

        let (base_url, signing) = if type_str == "bedrock-claude" {
            let Some(region) = obj
                .get("region")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                warnings.push(format!(
                    "provider {name:?}: missing non-empty \"region\" for bedrock-claude; skipped"
                ));
                continue;
            };
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
                    region: region.to_string(),
                    service: "bedrock".to_string(),
                }),
            )
        } else {
            let Some(base_url) = obj
                .get("baseUrl")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                warnings.push(format!(
                    "provider {name:?}: missing non-empty \"baseUrl\"; skipped"
                ));
                continue;
            };
            (base_url.to_string(), None)
        };

        let azure = if type_str == "azure-openai" {
            let Some(api_version) = obj
                .get("apiVersion")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                warnings.push(format!(
                    "provider {name:?}: missing non-empty \"apiVersion\" for azure-openai; skipped"
                ));
                continue;
            };
            Some(AzureConfig {
                api_version: api_version.to_string(),
            })
        } else {
            None
        };

        let env_var = obj
            .get("apiKeyEnv")
            .and_then(Value::as_str)
            .map(str::to_string);
        if type_str != "bedrock-claude" && env_var.as_deref().unwrap_or("").is_empty() {
            warnings.push(format!(
                "provider {name:?}: missing non-empty \"apiKeyEnv\"; skipped"
            ));
            continue;
        }

        let credential = if type_str == "bedrock-claude" {
            CredentialConfig::HostManaged {
                id: "bedrock_sigv4".to_string(),
            }
        } else {
            CredentialConfig::None
        };

        let models = obj
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| parse_models(name, arr, &mut warnings))
            .unwrap_or_default();

        out.push(ParsedUserProvider {
            profile: ProviderProfile {
                provider_id,
                profile_name: name.clone(),
                base_url,
                protocol,
                auth,
                credential,
                models,
                pricing: PricingConfig::default(),
                signing,
                azure,
                supports_websockets,
                supports_websocket_compression,
                websocket_connect_timeout_ms,
            },
            env_var,
        });
    }

    (out, warnings)
}

fn parse_models(
    provider_name: &str,
    models: &[Value],
    warnings: &mut Vec<String>,
) -> Vec<ModelProfile> {
    models
        .iter()
        .filter_map(|model| match model {
            Value::String(id) => Some(ModelProfile {
                display_model: id.clone(),
                request_model: id.clone(),
                billing_model: id.clone(),
                aliases: Vec::new(),
                capabilities: permissive_caps(),
            }),
            Value::Object(obj) => {
                let Some(id) = obj.get("id").and_then(Value::as_str) else {
                    warnings.push(format!(
                        "provider {provider_name:?}: model entry missing string \"id\"; skipped"
                    ));
                    return None;
                };
                let aliases = obj
                    .get("aliases")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|alias| alias.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(ModelProfile {
                    display_model: id.to_string(),
                    request_model: id.to_string(),
                    billing_model: id.to_string(),
                    aliases,
                    capabilities: parse_capabilities(obj.get("capabilities")),
                })
            }
            _ => {
                warnings.push(format!(
                    "provider {provider_name:?}: model entry is not a string or object; skipped"
                ));
                None
            }
        })
        .collect()
}

/// Backward-compatible permissive capabilities for legacy string model entries.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(name: &str, v: Value) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        m.insert(name.to_string(), v);
        m
    }

    #[test]
    fn openai_maps_to_openai_chat_bearer_compatible() {
        let raw = one(
            "groq",
            json!({ "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY", "models": [{"id": "llama-3.3-70b"}] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.profile.profile_name, "groq");
        assert_eq!(
            p.profile.provider_id,
            ProviderId::OpenAICompatible {
                name: "groq".to_string()
            }
        );
        assert_eq!(p.profile.protocol, ProtocolFamily::OpenAiChat);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
        assert_eq!(p.profile.base_url, "https://api.groq.com/openai/v1");
        assert_eq!(p.env_var.as_deref(), Some("GROQ_API_KEY"));
        assert_eq!(p.profile.models.len(), 1);
        assert_eq!(p.profile.models[0].request_model, "llama-3.3-70b");
        assert_eq!(p.profile.models[0].billing_model, "llama-3.3-70b");
        assert_eq!(p.profile.models[0].display_model, "llama-3.3-70b");
        assert!(p.profile.models[0].capabilities.tools);
    }

    #[test]
    fn anthropic_maps_to_anthropic_messages_apikey_custom() {
        let raw = one(
            "myclaude",
            json!({ "type": "anthropic", "baseUrl": "https://proxy.example/anthropic", "apiKeyEnv": "ANTHROPIC_API_KEY", "models": ["claude-x"] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::AnthropicFirstParty);
        assert_eq!(p.profile.protocol, ProtocolFamily::AnthropicMessages);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
        assert_eq!(p.env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn gemini_maps_to_gemini_apikey_custom() {
        let raw = one(
            "g",
            json!({ "type": "gemini", "baseUrl": "https://generativelanguage.googleapis.com/v1beta", "apiKeyEnv": "GEMINI_API_KEY", "models": ["gemini-2.5-pro"] }),
        );
        let (parsed, _warns) = parse_user_providers(&raw);
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::Gemini);
        assert_eq!(p.profile.protocol, ProtocolFamily::GeminiGenerateContent);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
    }

    #[test]
    fn supported_provider_kinds_match_platform_parser_semantics() {
        let raw: BTreeMap<String, Value> = serde_json::from_value(json!({
            "openai_chat": {
                "type": "openai",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "models": [{"id": "gpt-4o", "capabilities": {"reasoning": true}}]
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
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/anthropic/models",
                "apiKeyEnv": "GOOGLE_OAUTH_TOKEN",
                "models": [{"id": "claude-sonnet-4@20250514"}]
            },
            "vertex_gemini_user": {
                "type": "vertex-gemini",
                "baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models",
                "apiKeyEnv": "GOOGLE_OAUTH_TOKEN",
                "models": [{"id": "gemini-2.5-pro"}]
            }
        }))
        .unwrap();

        let (parsed, warns) = parse_user_providers(&raw);

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(parsed.len(), 8);
        let by_name: BTreeMap<_, _> = parsed
            .iter()
            .map(|p| (p.profile.profile_name.as_str(), &p.profile))
            .collect();
        assert_eq!(by_name["openai_chat"].protocol, ProtocolFamily::OpenAiChat);
        assert_eq!(
            by_name["openai_responses"].protocol,
            ProtocolFamily::OpenAiResponses
        );
        assert_eq!(
            by_name["anthropic_user"].protocol,
            ProtocolFamily::AnthropicMessages
        );
        assert_eq!(
            by_name["gemini_user"].protocol,
            ProtocolFamily::GeminiGenerateContent
        );
        assert_eq!(by_name["azure_user"].protocol, ProtocolFamily::AzureOpenAi);
        assert_eq!(by_name["azure_user"].auth, AuthStrategy::AzureToken);
        assert_eq!(
            by_name["azure_user"].azure.as_ref().unwrap().api_version,
            "2024-02-01"
        );
        assert_eq!(
            by_name["bedrock_user"].protocol,
            ProtocolFamily::BedrockClaude
        );
        assert_eq!(by_name["bedrock_user"].auth, AuthStrategy::AwsSigV4);
        assert_eq!(
            by_name["bedrock_user"].base_url,
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        );
        assert_eq!(
            by_name["bedrock_user"].signing.as_ref().unwrap().service,
            "bedrock"
        );
        assert_eq!(
            by_name["bedrock_user"].credential,
            CredentialConfig::HostManaged {
                id: "bedrock_sigv4".to_string(),
            }
        );
        assert_eq!(
            by_name["vertex_claude_user"].protocol,
            ProtocolFamily::VertexClaude
        );
        assert_eq!(by_name["vertex_claude_user"].auth, AuthStrategy::GcpToken);
        assert_eq!(
            by_name["vertex_gemini_user"].protocol,
            ProtocolFamily::VertexGemini
        );
        assert_eq!(by_name["vertex_gemini_user"].auth, AuthStrategy::GcpToken);
        assert!(by_name["openai_chat"].models[0].capabilities.reasoning);
        assert!(!by_name["openai_responses"].supports_websockets);
        assert_eq!(
            by_name["openai_responses"].websocket_connect_timeout_ms,
            None
        );
    }

    #[test]
    fn openai_responses_websocket_fields_parse() {
        let raw = one(
            "openai_responses",
            json!({
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "websocketConnectTimeoutMs": 1500,
                "models": [{"id": "gpt-5"}],
            }),
        );

        let (parsed, warns) = parse_user_providers(&raw);

        assert!(warns.is_empty(), "unexpected warnings: {warns:?}");
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].profile.supports_websockets);
        assert_eq!(parsed[0].profile.websocket_connect_timeout_ms, Some(1500));
    }

    #[test]
    fn websocket_fields_warn_and_skip_non_responses_provider() {
        let raw = one(
            "openai_chat",
            json!({
                "type": "openai",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "models": [{"id": "gpt-4o"}],
            }),
        );

        let (parsed, warns) = parse_user_providers(&raw);

        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("openai-responses"), "warnings: {warns:?}");
    }

    #[test]
    fn websocket_fields_warn_and_skip_bedrock_sigv4_provider() {
        let raw = one(
            "bedrock",
            json!({
                "type": "bedrock-claude",
                "region": "us-east-1",
                "supportsWebsockets": true,
                "models": [{"id": "anthropic.claude-3-5-sonnet-20241022-v2:0"}],
            }),
        );

        let (parsed, warns) = parse_user_providers(&raw);

        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("AWS SigV4"), "warnings: {warns:?}");
    }

    #[test]
    fn websocket_compression_warns_and_skips_until_transport_supports_it() {
        let raw = one(
            "openai_responses",
            json!({
                "type": "openai-responses",
                "baseUrl": "https://api.openai.com/v1",
                "apiKeyEnv": "OPENAI_API_KEY",
                "supportsWebsockets": true,
                "supportsWebsocketCompression": true,
                "models": [{"id": "gpt-5"}],
            }),
        );

        let (parsed, warns) = parse_user_providers(&raw);

        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("not supported"), "warnings: {warns:?}");
    }

    #[test]
    fn unknown_type_is_skipped_with_warning() {
        let raw = one("weird", json!({ "type": "cohere", "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("unknown type"));
        assert!(warns[0].contains("weird"));
    }

    #[test]
    fn missing_base_url_is_skipped_with_warning() {
        let raw = one("nb", json!({ "type": "openai", "models": ["m"] }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("baseUrl"));
    }

    #[test]
    fn non_object_entry_is_skipped_with_warning() {
        let raw = one("bad", json!("just-a-string"));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("not an object"));
    }

    #[test]
    fn missing_type_is_skipped_with_warning() {
        let raw = one("nt", json!({ "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(parsed.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("type"));
    }

    #[test]
    fn model_less_entry_parses_with_no_models() {
        let raw = one(
            "listingonly",
            json!({ "type": "openai", "baseUrl": "https://x", "apiKeyEnv": "OPENAI_API_KEY" }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].profile.models.is_empty());
    }
}
