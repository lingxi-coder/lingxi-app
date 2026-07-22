//! LLM-driven memory selector — routes through [`SideQueryClient`] (Plan 08).
//!
//! Closes spec gap **C1**: §6.3 memory selector now issues a real side
//! query (Haiku-class model, JSON structured output) instead of the
//! deterministic stub shipped in Plan 04. The selector is invoked once per
//! turn (in parallel with the main API call) and returns a small set of
//! memory file paths to surface in the next prompt.

use crate::file::{MemoryError, MemoryFile};
use sidequery::{QuerySource, SideQueryClient, SideQueryRequest};
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
            profile: None,
            system_prompt: Some("You select memory files relevant to the query.".into()),
            messages: vec![protocol::ConversationMessage::user(
                protocol::MessageId::new(),
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
            thinking: None,
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

/// Convert a `scan_memdir`-produced [`protocol::MemoryEntry`] into the
/// selector-input [`MemoryFile`] shape, so the prefetch path (and any future
/// consumer) can feed memdir entries straight into [`MemorySelector`] /
/// surfacing without re-walking disk.
///
/// Field mapping:
/// - `path` ← `entry.path` (verbatim).
/// - `content` ← `entry.body`. The memdir scanner already strips frontmatter +
///   redacts secrets, so the body is the renderable markdown.
/// - `frontmatter` ← re-parsed from `entry.body` via
///   [`crate::parse_markdown_with_frontmatter`]. In the normal case the body is
///   already frontmatter-stripped, so this yields the default frontmatter (empty
///   `description`); we re-parse defensively so a body that *does* carry an
///   inline `---` block still surfaces its `description` to the selector LLM.
/// - `mtime` ← reconstructed from `entry.age_days` relative to `SystemTime::now`
///   (`now - age_days * 86_400s`). The memdir scanner only retains whole-day age
///   (it drops the raw mtime), so this is the faithful day-granular
///   reconstruction; consumers that need exact mtime read the file directly.
///
/// Lives in the memory crate (not the orchestrator) so prefetch + the surfacing
/// channel + any future consumer share ONE converter (shared-helper contract).
#[must_use]
pub fn memory_entry_to_memory_file(entry: &protocol::MemoryEntry) -> MemoryFile {
    let (frontmatter, content) = crate::parse_markdown_with_frontmatter(&entry.body)
        .unwrap_or_else(|_| (crate::MemoryFrontmatter::default(), entry.body.clone()));
    let mtime = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            entry.age_days.saturating_mul(86_400),
        ))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    MemoryFile {
        path: entry.path.clone(),
        mtime,
        frontmatter,
        content,
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

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{MemoryEntry, MemoryEntryTier};
    use std::path::PathBuf;

    fn entry(path: &str, body: &str, age_days: u64) -> MemoryEntry {
        MemoryEntry {
            path: PathBuf::from(path),
            tier: MemoryEntryTier::Project,
            body: body.into(),
            age_days,
            size_bytes: body.len() as u64,
        }
    }

    #[test]
    fn converter_maps_path_and_body() {
        // A frontmatter-less body passes through verbatim (no trim — the render
        // path uses `content` as-is, matching the TS `${r.content}`).
        let f = memory_entry_to_memory_file(&entry("/m/a.md", "USE FD\n", 0));
        assert_eq!(f.path, PathBuf::from("/m/a.md"));
        assert_eq!(f.content, "USE FD\n");
        // Frontmatter-stripped body => default (empty) frontmatter.
        assert!(f.frontmatter.description.is_empty());
    }

    #[test]
    fn converter_reparses_inline_frontmatter_description() {
        // Defensive path: a body that still carries an inline `---` block
        // surfaces its `description` to the selector. The post-frontmatter body
        // is `trim_start`-ed by the parser (its documented contract) but keeps
        // its trailing newline.
        let body = "---\ndescription: bash tips\n---\nuse fd not find\n";
        let f = memory_entry_to_memory_file(&entry("/m/b.md", body, 0));
        assert_eq!(f.frontmatter.description, "bash tips");
        assert_eq!(f.content, "use fd not find\n");
    }

    #[test]
    fn converter_reconstructs_mtime_from_age_days() {
        // A 10-day-old entry => mtime is ~10 days before now (day-granular).
        let f = memory_entry_to_memory_file(&entry("/m/c.md", "X", 10));
        let now = std::time::SystemTime::now();
        let age = now.duration_since(f.mtime).expect("mtime is in the past");
        let days = age.as_secs() / 86_400;
        // Allow a 1-day slack for the test's wall-clock drift across the calls.
        assert!((9..=10).contains(&days), "expected ~10 days, got {days}");
    }
}
