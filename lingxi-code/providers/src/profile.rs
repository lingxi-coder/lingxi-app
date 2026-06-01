//! Provider profile config — the typed form of a settings `providers` entry,
//! plus the built-in defaults.

use crate::error::CodecError;
use std::collections::BTreeMap;

/// Which wire format / codec a profile uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Anthropic Messages API (the only kind constructible in P2).
    Anthropic,
    /// `OpenAI` / OpenAI-compatible chat completions (codec lands in P3).
    OpenAi,
    /// Google Gemini generateContent (codec lands in P4).
    Gemini,
    /// Azure `OpenAI` (`OpenAI` chat wire over a deployment URL + `api-key` header).
    AzureOpenAi,
    /// Google Vertex AI (Gemini body over a regional URL + GCP bearer auth).
    Vertex,
    /// AWS Bedrock (Claude): Anthropic body over `InvokeModel` + `SigV4` auth.
    Bedrock,
}

impl ProviderKind {
    /// Parse the settings `type` string.
    ///
    /// # Errors
    /// Returns [`CodecError::Unsupported`] for an unknown type.
    pub fn parse(s: &str) -> Result<Self, CodecError> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "gemini" => Ok(Self::Gemini),
            "azureOpenAi" | "azure" => Ok(Self::AzureOpenAi),
            "vertex" => Ok(Self::Vertex),
            "bedrock" => Ok(Self::Bedrock),
            other => Err(CodecError::Unsupported(format!(
                "unknown provider type {other:?} (expected anthropic|openai|gemini|azureOpenAi|vertex|bedrock)"
            ))),
        }
    }
}

/// One provider profile: a wire format plus its endpoint + key source.
#[derive(Debug, Clone)]
pub struct ProviderProfile {
    /// Wire format / codec.
    pub kind: ProviderKind,
    /// Optional base URL override (for OpenAI-compatible / custom endpoints).
    pub base_url: Option<String>,
    /// Name of the env var holding the API key (e.g. `OPENAI_API_KEY`).
    /// `None` means no auth (e.g. a local Ollama endpoint).
    pub api_key_env: Option<String>,
    /// Reasoning effort override for `OpenAI` o-series models. When set, the
    /// `OpenAI` codec emits `reasoning_effort`, uses `max_completion_tokens`,
    /// and omits `temperature`.
    pub reasoning_effort: Option<crate::request::ReasoningEffort>,
    /// Thinking-token budget for Gemini 2.5 models. When set, the Gemini
    /// codec emits `generationConfig.thinkingConfig`.
    pub thinking_budget: Option<u32>,
    /// Azure deployment name (e.g. `gpt-4o`). Used only when `kind` is
    /// [`ProviderKind::AzureOpenAi`].
    pub azure_deployment: Option<String>,
    /// Azure REST API version query parameter (e.g. `2024-10-21`). Used only
    /// when `kind` is [`ProviderKind::AzureOpenAi`].
    pub azure_api_version: Option<String>,
    /// GCP project id (Vertex only).
    pub project: Option<String>,
    /// GCP region, e.g. `us-central1` (Vertex only).
    pub region: Option<String>,
    /// Provider-local model ids this profile declares (e.g.
    /// `["llama-3.3-70b"]`). Surfaced by `/model`'s list mode as
    /// `{profile}/{model}`. Empty by default.
    pub models: Vec<String>,
}

/// Built-in profiles, keyed by name. `anthropic` uses `anthropic_base` (the
/// CLI's resolved Anthropic base URL); `openai`/`gemini` use their first-party
/// defaults (their codecs are not available until P3/P4).
#[must_use]
pub fn builtin_profiles(anthropic_base: Option<String>) -> BTreeMap<String, ProviderProfile> {
    let mut m = BTreeMap::new();
    m.insert(
        "anthropic".to_string(),
        ProviderProfile {
            kind: ProviderKind::Anthropic,
            base_url: anthropic_base,
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            reasoning_effort: None,
            thinking_budget: None,
            azure_deployment: None,
            azure_api_version: None,
            project: None,
            region: None,
            models: Vec::new(),
        },
    );
    m.insert(
        "openai".to_string(),
        ProviderProfile {
            kind: ProviderKind::OpenAi,
            base_url: None,
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            reasoning_effort: None,
            thinking_budget: None,
            azure_deployment: None,
            azure_api_version: None,
            project: None,
            region: None,
            models: Vec::new(),
        },
    );
    m.insert(
        "gemini".to_string(),
        ProviderProfile {
            kind: ProviderKind::Gemini,
            base_url: None,
            api_key_env: Some("GEMINI_API_KEY".to_string()),
            reasoning_effort: None,
            thinking_budget: None,
            azure_deployment: None,
            azure_api_version: None,
            project: None,
            region: None,
            models: Vec::new(),
        },
    );
    m
}

/// Parse the settings `providers` object into typed profiles. Each entry is
/// `{ "type": "openai", "baseUrl"?: "...", "apiKeyEnv"?: "..."|null }`.
///
/// # Errors
/// Returns [`CodecError::Unsupported`] if an entry is not an object, lacks a
/// string `type`, or has an unknown `type`.
pub fn parse_profiles(
    raw: Option<&BTreeMap<String, serde_json::Value>>,
) -> Result<BTreeMap<String, ProviderProfile>, CodecError> {
    let mut out = BTreeMap::new();
    let Some(raw) = raw else {
        return Ok(out);
    };
    for (name, value) in raw {
        let obj = value.as_object().ok_or_else(|| {
            CodecError::Unsupported(format!("provider profile {name:?} must be an object"))
        })?;
        let type_str = obj
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                CodecError::Unsupported(format!(
                    "provider profile {name:?} is missing a string \"type\""
                ))
            })?;
        let kind = ProviderKind::parse(type_str)?;
        let base_url = obj
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // `apiKeyEnv` may be a string or explicit null (no auth).
        let api_key_env = match obj.get("apiKeyEnv") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let reasoning_effort = obj
            .get("reasoningEffort")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::request::ReasoningEffort::parse);
        let thinking_budget = obj
            .get("thinkingBudget")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        let azure_deployment = obj
            .get("azureDeployment")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let azure_api_version = obj
            .get("azureApiVersion")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let project = obj
            .get("project")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let region = obj
            .get("region")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let models = obj
            .get("models")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            name.clone(),
            ProviderProfile {
                kind,
                base_url,
                api_key_env,
                reasoning_effort,
                thinking_budget,
                azure_deployment,
                azure_api_version,
                project,
                region,
                models,
            },
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtins_present() {
        let b = builtin_profiles(Some("https://api.anthropic.com".to_string()));
        assert_eq!(b["anthropic"].kind, ProviderKind::Anthropic);
        assert_eq!(b["openai"].kind, ProviderKind::OpenAi);
        assert_eq!(b["gemini"].kind, ProviderKind::Gemini);
        assert_eq!(
            b["anthropic"].base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
    }

    #[test]
    fn parse_none_is_empty() {
        assert!(parse_profiles(None).unwrap().is_empty());
    }

    #[test]
    fn parse_openai_compatible_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "groq".to_string(),
            json!({"type": "openai", "baseUrl": "https://api.groq.com/openai/v1", "apiKeyEnv": "GROQ_API_KEY"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["groq"].kind, ProviderKind::OpenAi);
        assert_eq!(
            p["groq"].base_url.as_deref(),
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(p["groq"].api_key_env.as_deref(), Some("GROQ_API_KEY"));
    }

    #[test]
    fn parse_null_api_key_env_means_no_auth() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "ollama".to_string(),
            json!({"type": "openai", "baseUrl": "http://localhost:11434/v1", "apiKeyEnv": null}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert!(p["ollama"].api_key_env.is_none());
    }

    #[test]
    fn parse_unknown_type_errors() {
        let mut raw = BTreeMap::new();
        raw.insert("x".to_string(), json!({"type": "mistral"}));
        assert!(parse_profiles(Some(&raw)).is_err());
    }

    #[test]
    fn parse_missing_type_errors() {
        let mut raw = BTreeMap::new();
        raw.insert("x".to_string(), json!({"baseUrl": "http://x"}));
        assert!(parse_profiles(Some(&raw)).is_err());
    }

    #[test]
    fn parse_azure_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "azure".to_string(),
            json!({"type":"azureOpenAi","baseUrl":"https://r.openai.azure.com",
                   "azureDeployment":"gpt-4o","azureApiVersion":"2024-10-21","apiKeyEnv":"AZURE_OPENAI_KEY"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["azure"].kind, ProviderKind::AzureOpenAi);
        assert_eq!(p["azure"].azure_deployment.as_deref(), Some("gpt-4o"));
        assert_eq!(p["azure"].azure_api_version.as_deref(), Some("2024-10-21"));
    }

    #[test]
    fn parse_vertex_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "vertex".to_string(),
            json!({"type": "vertex", "project": "p", "region": "us-central1"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["vertex"].kind, ProviderKind::Vertex);
        assert_eq!(p["vertex"].project.as_deref(), Some("p"));
        assert_eq!(p["vertex"].region.as_deref(), Some("us-central1"));
    }

    #[test]
    fn parse_bedrock_profile() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "bedrock".to_string(),
            json!({"type": "bedrock", "region": "us-east-1"}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["bedrock"].kind, ProviderKind::Bedrock);
        assert_eq!(p["bedrock"].region.as_deref(), Some("us-east-1"));
    }

    #[test]
    fn parse_models_array() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "groq".to_string(),
            json!({"type": "openai", "models": ["llama-3.3-70b", "mixtral-8x7b"]}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["groq"].models, vec!["llama-3.3-70b", "mixtral-8x7b"]);
        // Absent `models` defaults to empty.
        let mut raw2 = BTreeMap::new();
        raw2.insert("openai".to_string(), json!({"type": "openai"}));
        assert!(parse_profiles(Some(&raw2)).unwrap()["openai"]
            .models
            .is_empty());
    }

    #[test]
    fn parse_reasoning_fields() {
        use crate::request::ReasoningEffort;
        let mut raw = BTreeMap::new();
        raw.insert(
            "o3".to_string(),
            json!({"type": "openai", "reasoningEffort": "high", "thinkingBudget": 2048}),
        );
        let p = parse_profiles(Some(&raw)).unwrap();
        assert_eq!(p["o3"].reasoning_effort, Some(ReasoningEffort::High));
        assert_eq!(p["o3"].thinking_budget, Some(2048));
    }
}
