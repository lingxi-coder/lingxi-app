//! Serde schema for a models.dev-shaped provider slice (`data/models-dev/*.json`).
//! OpenRouter is generated from its official Models API into this shared shape.
//!
//! Tolerant of unknown/added fields: upstream evolves, so only the fields we map
//! are declared and everything else is ignored. Optional fields default so a
//! missing key never fails the parse.

use serde::{Deserialize, Deserializer};

fn deserialize_flag_option<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(flag) => Some(flag),
        serde_json::Value::Number(number) => Some(number.as_i64().is_none_or(|n| n != 0)),
        serde_json::Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                None
            } else if matches!(normalized.as_str(), "false" | "0" | "no" | "off") {
                Some(false)
            } else {
                Some(true)
            }
        }
        serde_json::Value::Array(values) => Some(!values.is_empty()),
        serde_json::Value::Object(values) => Some(!values.is_empty()),
    }))
}

fn deserialize_experimental_flag<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::Bool(flag) => Some(flag),
        serde_json::Value::Number(number) => Some(number.as_i64().is_none_or(|n| n != 0)),
        serde_json::Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            }
        }
        // Some slices use an object here for experimental request modes. That
        // is not a model lifecycle status and must not label the whole model.
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }))
}

/// One provider slice: `{ api?, name, env[], id, models: { id -> Model } }`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSlice {
    /// Upstream API base (advisory; the routing table overrides it).
    #[serde(default)]
    pub api: Option<String>,
    /// Human provider name.
    pub name: String,
    /// Upstream credential env var names (advisory).
    #[serde(default)]
    pub env: Vec<String>,
    /// Provider id as keyed in api.json.
    pub id: String,
    /// Models keyed by model id.
    pub models: std::collections::BTreeMap<String, Model>,
}

/// One model entry. Only mapped fields are declared; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)]
pub struct Model {
    /// Wire model id (sent on the request; also the billing key).
    pub id: String,
    /// Human-facing label.
    pub name: String,
    /// Optional one-line description, surfaced as a dimmed sub-line in the
    /// `/model` picker. models.dev carries this for some models; absent ⇒ `None`.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub knowledge: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default, deserialize_with = "deserialize_flag_option")]
    pub open_weights: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_flag_option")]
    pub attachment: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_flag_option")]
    pub temperature: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_experimental_flag")]
    pub experimental: Option<bool>,
    /// Whether the model supports native tool calls.
    #[serde(default)]
    pub tool_call: bool,
    /// Whether the model supports reasoning.
    #[serde(default)]
    pub reasoning: bool,
    /// Provider-published toggle, effort, and token-budget controls.
    #[serde(default)]
    pub reasoning_options: Vec<ReasoningOption>,
    /// Whether the model supports structured output.
    #[serde(default)]
    pub structured_output: bool,
    /// Input/output modalities (absent → text-only).
    #[serde(default)]
    pub modalities: Option<Modalities>,
    /// Per-million-token costs (absent → unpriced).
    #[serde(default)]
    pub cost: Option<Cost>,
    /// Token limits (context window + max output). Registered into the
    /// `model::model_limits` registry at catalog assembly so non-Claude models
    /// report their real window / max-output instead of the Claude defaults.
    #[serde(default)]
    pub limit: Option<Limit>,
    /// Catalog status (`alpha`/`beta`/`deprecated`), when present.
    #[serde(default)]
    pub status: Option<String>,
}

/// Input/output modality lists.
#[derive(Debug, Clone, Deserialize)]
pub struct Modalities {
    /// Accepted input modalities (e.g. `text`, `image`, `pdf`).
    #[serde(default)]
    pub input: Vec<String>,
    /// Produced output modalities.
    #[serde(default)]
    pub output: Vec<String>,
}

/// Per-million-token costs (USD).
#[derive(Debug, Clone, Deserialize)]
pub struct Cost {
    /// Input price per million tokens.
    pub input: f64,
    /// Output price per million tokens.
    pub output: f64,
    /// Cache-read price per million tokens.
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Cache-write price per million tokens.
    #[serde(default)]
    pub cache_write: Option<f64>,
    /// Separately billed reasoning output, when published.
    #[serde(default)]
    pub reasoning: Option<f64>,
    /// Alternate rate sheets activated above a context threshold.
    #[serde(default)]
    pub tiers: Vec<CostTier>,
}

/// One models.dev pricing tier.
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)]
pub struct CostTier {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub reasoning: Option<f64>,
    pub tier: CostTierTrigger,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)]
pub struct CostTierTrigger {
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
}

/// Token limits.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Limit {
    /// Context-window size in tokens.
    pub context: u64,
    /// Maximum accepted input tokens, when separately published.
    #[serde(default)]
    pub input: Option<u64>,
    /// Max output tokens.
    pub output: u64,
}

/// One models.dev reasoning option. Unknown option kinds remain parseable and
/// are ignored by the normalized controls mapper.
#[derive(Debug, Clone, Deserialize)]
#[allow(missing_docs)]
pub struct ReasoningOption {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub min: Option<u64>,
    #[serde(default)]
    pub max: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");
    const OPENAI: &str = include_str!("../../data/models-dev/openai.json");

    #[test]
    fn parses_deepseek_slice() {
        let slice: ProviderSlice = serde_json::from_str(DEEPSEEK).expect("deepseek slice parses");
        assert_eq!(slice.name, "DeepSeek");
        assert_eq!(slice.models.len(), 3);
        // Every model carries an id + name.
        for (key, model) in &slice.models {
            assert_eq!(key, &model.id);
            assert!(!model.name.is_empty());
        }
    }

    #[test]
    fn parses_openai_slice_with_object_flags() {
        let slice: ProviderSlice = serde_json::from_str(OPENAI).expect("openai slice parses");
        assert!(slice.models.contains_key("gpt-5.6-sol"));
        let sol = slice
            .models
            .get("gpt-5.6-sol")
            .expect("gpt-5.6-sol present");
        assert_eq!(
            sol.experimental, None,
            "experimental request modes are not a model lifecycle status"
        );
    }
}
