//! LLM-driven memory selector — routes through [`SideQueryClient`] (Plan 08).
//!
//! Closes spec gap **C1**: §6.3 memory selector now issues a real side
//! query (Haiku-class model, JSON structured output) instead of the
//! deterministic stub shipped in Plan 04. The selector is invoked once per
//! turn (in parallel with the main API call) and returns a small set of
//! memory file paths to surface in the next prompt.

use crate::file::{MemoryError, MemoryFile};
use lingxi_sidequery::{QuerySource, SideQueryClient, SideQueryRequest};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Selects which available memory files are relevant to the current turn.
pub struct MemorySelector {
    /// Model identifier used for the side query.
    pub selector_model: String,
    /// Hard cap on how many files are returned per call.
    pub max_selected: usize,
    client: Arc<dyn SideQueryClient>,
}

impl MemorySelector {
    /// Build a selector that issues side queries through `client`.
    ///
    /// Engine defaults: Haiku-class model, 5 files per turn.
    #[must_use]
    pub fn new(client: Arc<dyn SideQueryClient>) -> Self {
        Self {
            selector_model: "claude-haiku-4-5".into(),
            max_selected: 5,
            client,
        }
    }

    /// Pick a subset of `available` files relevant to `query`.
    ///
    /// The selector issues a single side query asking the model to return a
    /// JSON object `{ "filenames": ["..."] }` of basenames. The result is
    /// intersected with `available`, deduplicated against `already`, and
    /// truncated to [`Self::max_selected`].
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::SelectorUnavailable`] when the side-query
    /// call fails.
    pub async fn select_relevant(
        &self,
        query: &str,
        available: &[MemoryFile],
        recent_tools: &[String],
        already: &HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, MemoryError> {
        let candidates: Vec<&MemoryFile> = available
            .iter()
            .filter(|m| !already.contains(&m.path))
            .collect();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let prompt = build_selector_prompt(query, &candidates, recent_tools);
        let req = SideQueryRequest {
            model: self.selector_model.clone(),
            system_prompt: Some("You select memory files relevant to the query.".into()),
            messages: vec![lingxi_protocol::ConversationMessage::user(
                lingxi_protocol::MessageId::new(),
                prompt,
            )],
            tools: vec![],
            tool_choice: None,
            output_format: Some(serde_json::json!({
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "filenames": { "type": "array", "items": { "type": "string" } }
                    }
                }
            })),
            max_tokens: 1024,
            max_retries: 2,
            temperature: Some(0.0),
            thinking_budget: None,
            stop_sequences: vec![],
            query_source: QuerySource::MemorySelector,
            skip_system_prompt_prefix: false,
        };

        let resp = self
            .client
            .query(req)
            .await
            .map_err(|e| MemoryError::SelectorUnavailable(e.to_string()))?;
        let names = parse_filenames(resp.structured.as_ref());
        Ok(candidates
            .iter()
            .filter(|m| {
                m.path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| names.contains(&s.to_string()))
            })
            .map(|m| m.path.clone())
            .take(self.max_selected)
            .collect())
    }
}

fn build_selector_prompt(
    query: &str,
    candidates: &[&MemoryFile],
    recent_tools: &[String],
) -> String {
    let mut s = format!("Query: {query}\n\nAvailable memory files:\n");
    for m in candidates {
        s.push_str(&format!(
            "- {}: {}\n",
            m.path.display(),
            m.frontmatter.description
        ));
    }
    if !recent_tools.is_empty() {
        s.push_str(&format!(
            "\nRecently used tools: {}\n",
            recent_tools.join(", ")
        ));
    }
    s
}

fn parse_filenames(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(v) = value else {
        return Vec::new();
    };
    v.get("filenames")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(std::string::ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}
