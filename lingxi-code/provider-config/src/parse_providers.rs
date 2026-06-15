//! Parse `settings.providers` into `llm_client::ProviderProfile`s (spec §5.1).
//! Narrowed to openai|anthropic|gemini with skip+warn (non-fatal) on bad entries.

use std::collections::BTreeMap;

use llm_client::{
    AuthStrategy, Capabilities, CredentialConfig, ModelProfile, PricingConfig, ProtocolFamily,
    ProviderId, ProviderProfile,
};
use serde_json::Value;

/// A parsed user profile plus the `apiKeyEnv` recorded for its credential source.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedUserProvider {
    /// The built provider profile (credential is a placeholder `None`; `assemble`
    /// rewrites it to `Static{id}` per spec §5.4).
    pub profile: ProviderProfile,
    /// The `apiKeyEnv` env fallback recorded by `assemble` into `CredentialSource`.
    pub env_var: Option<String>,
}

/// Parse `settings.providers` into profiles + warnings (spec §5.1). Unknown
/// `type` / missing `baseUrl` skips the entry with a warning. A model-less entry
/// parses with no models; `assemble` decides routability (it drops + warns).
#[must_use]
pub fn parse_user_providers(
    providers: &BTreeMap<String, Value>,
) -> (Vec<ParsedUserProvider>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();

    for (name, value) in providers {
        let Some(obj) = value.as_object() else {
            warnings.push(format!("provider {name:?}: entry is not an object; skipped"));
            continue;
        };
        let Some(type_str) = obj.get("type").and_then(Value::as_str) else {
            warnings.push(format!("provider {name:?}: missing string \"type\"; skipped"));
            continue;
        };
        let Some(base_url) = obj.get("baseUrl").and_then(Value::as_str) else {
            warnings.push(format!("provider {name:?}: missing \"baseUrl\"; skipped"));
            continue;
        };

        let (protocol, auth, provider_id) = match type_str {
            "openai" => (
                ProtocolFamily::OpenAiChat,
                AuthStrategy::Bearer,
                ProviderId::OpenAICompatible { name: name.clone() },
            ),
            "anthropic" => (
                ProtocolFamily::AnthropicMessages,
                AuthStrategy::ApiKey,
                ProviderId::Custom { name: name.clone() },
            ),
            "gemini" => (
                ProtocolFamily::GeminiGenerateContent,
                AuthStrategy::ApiKey,
                ProviderId::Custom { name: name.clone() },
            ),
            other => {
                warnings.push(format!(
                    "provider {name:?}: unknown type {other:?} (expected openai|anthropic|gemini); skipped"
                ));
                continue;
            }
        };

        let env_var = obj.get("apiKeyEnv").and_then(Value::as_str).map(str::to_string);

        let models = obj
            .get("models")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.as_str())
                    .map(|id| ModelProfile {
                        display_model: id.to_string(),
                        request_model: id.to_string(),
                        billing_model: id.to_string(),
                        aliases: Vec::new(),
                        capabilities: permissive_caps(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(ParsedUserProvider {
            profile: ProviderProfile {
                provider_id,
                profile_name: name.clone(),
                base_url: base_url.to_string(),
                protocol,
                auth,
                credential: CredentialConfig::None,
                models,
                pricing: PricingConfig::default(),
            },
            env_var,
        });
    }

    (out, warnings)
}

/// Permissive capabilities for a user-declared model (spec §5.1) so preflight
/// does not reject.
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
            json!({ "type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY", "models": ["llama-3.3-70b"] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.profile.profile_name, "groq");
        assert_eq!(p.profile.provider_id, ProviderId::OpenAICompatible { name: "groq".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::OpenAiChat);
        assert_eq!(p.profile.auth, AuthStrategy::Bearer);
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
            json!({ "type": "anthropic", "baseUrl": "https://proxy.example/anthropic", "models": ["claude-x"] }),
        );
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::Custom { name: "myclaude".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::AnthropicMessages);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
        assert_eq!(p.env_var, None);
    }

    #[test]
    fn gemini_maps_to_gemini_apikey_custom() {
        let raw = one(
            "g",
            json!({ "type": "gemini", "baseUrl": "https://generativelanguage.googleapis.com/v1beta", "models": ["gemini-2.5-pro"] }),
        );
        let (parsed, _warns) = parse_user_providers(&raw);
        let p = &parsed[0];
        assert_eq!(p.profile.provider_id, ProviderId::Custom { name: "g".to_string() });
        assert_eq!(p.profile.protocol, ProtocolFamily::GeminiGenerateContent);
        assert_eq!(p.profile.auth, AuthStrategy::ApiKey);
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
        let raw = one("listingonly", json!({ "type": "openai", "baseUrl": "https://x" }));
        let (parsed, warns) = parse_user_providers(&raw);
        assert!(warns.is_empty());
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].profile.models.is_empty());
    }
}
