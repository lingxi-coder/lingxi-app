//! Serde schema for a single models.dev provider slice (`data/models-dev/*.json`).
//!
//! Tolerant of unknown/added fields: upstream evolves, so only the fields we map
//! are declared and everything else is ignored. Optional fields default so a
//! missing key never fails the parse.

use serde::Deserialize;

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
pub struct Model {
    /// Wire model id (sent on the request; also the billing key).
    pub id: String,
    /// Human-facing label.
    pub name: String,
    /// Whether the model supports native tool calls.
    #[serde(default)]
    pub tool_call: bool,
    /// Whether the model supports reasoning.
    #[serde(default)]
    pub reasoning: bool,
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
#[derive(Debug, Clone, Copy, Deserialize)]
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
}

/// Token limits.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Limit {
    /// Context-window size in tokens.
    pub context: u64,
    /// Max output tokens.
    pub output: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEEPSEEK: &str = include_str!("../../data/models-dev/deepseek.json");

    #[test]
    fn parses_deepseek_slice() {
        let slice: ProviderSlice = serde_json::from_str(DEEPSEEK).expect("deepseek slice parses");
        assert_eq!(slice.name, "DeepSeek");
        assert_eq!(slice.models.len(), 4);
        // Every model carries an id + name.
        for (key, model) in &slice.models {
            assert_eq!(key, &model.id);
            assert!(!model.name.is_empty());
        }
    }
}
