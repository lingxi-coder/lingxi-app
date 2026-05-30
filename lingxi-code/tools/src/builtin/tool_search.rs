//! `ToolSearchTool` — token-overlap search over the registry.
//!
//! Wire identifiers locked in spec §7:
//! - Top-20 results.
//! - Token normalization: lowercase, split on `[^a-z0-9]+`.
//! - Score = `|query_tokens ∩ tool_tokens|`.
//! - Ties broken by tool name lexicographic order.
//!
//! Avoids holding `Arc<ToolRegistry>` directly (which would cycle) by
//! accepting a `ToolRegistryView` snapshot at construction time. The
//! dispatcher passes a freshly-snapshotted vec when registering this tool.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{TOOL_SEARCH_COMPLETED, TOOL_SEARCH_FAILED, TOOL_SEARCH_STARTED};
use telemetry::AnalyticsBus;

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock.
pub const TOOL_SEARCH_TOOL_NAME: &str = "ToolSearch";
/// Top-N cap (spec §7).
pub const TOOL_SEARCH_MAX_RESULTS: usize = 20;

/// One row in the searchable registry view.
#[derive(Debug, Clone)]
pub struct ToolSearchEntry {
    /// Tool name (used for ranking display + tie-breaks).
    pub name: String,
    /// Tool description (token source).
    pub description: String,
}

/// Read-only snapshot of registered tools fed to `ToolSearchTool` at
/// construction. Avoids the `Arc<ToolRegistry>` cycle that would arise from
/// `lingxi-tools::ToolSearchTool` holding a strong ref to its owner.
pub trait ToolRegistryView: Send + Sync {
    /// All registered tool entries (name + description).
    fn entries(&self) -> Vec<ToolSearchEntry>;
}

/// Static-vec implementation — `register_all_builtin_tools` builds one of
/// these from the registry it just populated.
pub struct StaticRegistryView {
    entries: Vec<ToolSearchEntry>,
}

impl StaticRegistryView {
    /// Construct from a vec of entries.
    #[must_use]
    pub fn new(entries: Vec<ToolSearchEntry>) -> Self {
        Self { entries }
    }
}

impl ToolRegistryView for StaticRegistryView {
    fn entries(&self) -> Vec<ToolSearchEntry> {
        self.entries.clone()
    }
}

/// `ToolSearchTool` — token-overlap search over the registry. Top-20 results.
pub struct ToolSearchTool {
    pub(crate) ctx: super::BuiltinToolContext,
    pub(crate) view: Arc<dyn ToolRegistryView>,
}

impl ToolSearchTool {
    /// Construct with an empty view (always returns no results — hermetic
    /// default for when no registry snapshot has been wired).
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self {
            ctx,
            view: Arc::new(StaticRegistryView::new(Vec::new())),
        }
    }

    /// Construct with a caller-supplied view (production use).
    #[must_use]
    pub fn with_view(ctx: super::BuiltinToolContext, view: Arc<dyn ToolRegistryView>) -> Self {
        Self { ctx, view }
    }
}

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "minLength": 1 }
        },
        "required": ["query"]
    })
});

/// Normalize a text blob into token set: lowercase + split on `[^a-z0-9]+`.
#[must_use]
pub(crate) fn tokenize(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            buf.push(ch.to_ascii_lowercase());
        } else if !buf.is_empty() {
            out.insert(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.insert(buf);
    }
    out
}

/// Compute `(score, name, description)` for every entry, then return top
/// [`TOOL_SEARCH_MAX_RESULTS`] sorted by `(-score, name)`.
#[must_use]
pub(crate) fn rank(query: &str, entries: &[ToolSearchEntry]) -> Vec<ToolSearchEntry> {
    let q_tokens = tokenize(query);
    let mut scored: Vec<(usize, &ToolSearchEntry)> = entries
        .iter()
        .map(|e| {
            let mut text = String::with_capacity(e.name.len() + e.description.len() + 1);
            text.push_str(&e.name);
            text.push(' ');
            text.push_str(&e.description);
            let toks = tokenize(&text);
            let score = q_tokens.intersection(&toks).count();
            (score, e)
        })
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    scored
        .into_iter()
        .take(TOOL_SEARCH_MAX_RESULTS)
        .map(|(_, e)| e.clone())
        .collect()
}

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(TOOL_SEARCH_FAILED, md).await;
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        TOOL_SEARCH_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ToolSearch is a read-only registry query".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Token-overlap search over the registered tool list (top-20 results).".into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "ToolSearch: rank registered tools by token overlap with the query.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let q = input
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("ToolSearch: missing or non-string query".into()))?;
        if q.is_empty() {
            return Err(ValidationError("ToolSearch: query is empty".into()));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();
        let query = match input.get("query").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_query", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "ToolSearch: missing or non-string query".into(),
                ));
            }
        };
        if query.is_empty() {
            emit_failed(&bus, "empty_query", started.elapsed().as_millis() as u64).await;
            return Err(ToolError::InvalidInput("ToolSearch: query is empty".into()));
        }

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("_PROTO_query".into(), pii_str(&query));
        bus.log_event(TOOL_SEARCH_STARTED, md).await;

        let entries = self.view.entries();
        let top = rank(&query, &entries);

        let results: Vec<Value> = top
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "description": e.description,
                })
            })
            .collect();

        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("result_count".into(), AnalyticsValue::Int(top.len() as i64));
        md.insert(
            "registry_size".into(),
            AnalyticsValue::Int(entries.len() as i64),
        );
        bus.log_event(TOOL_SEARCH_COMPLETED, md).await;

        Ok(ToolCallResult {
            data: json!({
                "query": query,
                "results": results,
                "result_count": top.len(),
                "max_results": TOOL_SEARCH_MAX_RESULTS,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    fn mk_view(entries: Vec<(&str, &str)>) -> Arc<dyn ToolRegistryView> {
        Arc::new(StaticRegistryView::new(
            entries
                .into_iter()
                .map(|(n, d)| ToolSearchEntry {
                    name: n.into(),
                    description: d.into(),
                })
                .collect(),
        ))
    }

    #[test]
    fn constants_locked() {
        assert_eq!(TOOL_SEARCH_TOOL_NAME, "ToolSearch");
        assert_eq!(TOOL_SEARCH_MAX_RESULTS, 20);
    }

    #[test]
    fn tokenize_lowercases_and_splits_on_non_alnum() {
        let toks = tokenize("Read-File! ABC123");
        assert!(toks.contains("read"));
        assert!(toks.contains("file"));
        assert!(toks.contains("abc123"));
        assert_eq!(toks.len(), 3);
    }

    #[test]
    fn rank_orders_by_overlap_score_then_name() {
        let entries = vec![
            ToolSearchEntry {
                name: "B".into(),
                description: "read file write".into(),
            },
            ToolSearchEntry {
                name: "A".into(),
                description: "read file write".into(),
            },
            ToolSearchEntry {
                name: "C".into(),
                description: "read".into(),
            },
        ];
        let r = rank("read file", &entries);
        // A and B both match {read, file} (score 2); A < B by name.
        // C only matches {read} (score 1).
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].name, "A");
        assert_eq!(r[1].name, "B");
        assert_eq!(r[2].name, "C");
    }

    #[test]
    fn rank_caps_at_20() {
        let entries: Vec<ToolSearchEntry> = (0..50)
            .map(|i| ToolSearchEntry {
                name: format!("t{i:02}"),
                description: "match me please".into(),
            })
            .collect();
        let r = rank("match", &entries);
        assert_eq!(r.len(), TOOL_SEARCH_MAX_RESULTS);
    }

    #[test]
    fn rank_excludes_zero_score() {
        let entries = vec![ToolSearchEntry {
            name: "x".into(),
            description: "nothing matches here".into(),
        }];
        assert!(rank("foo", &entries).is_empty());
    }

    #[tokio::test]
    async fn returns_ranked_results() {
        let tool = ToolSearchTool::with_view(
            shell_test_ctx(dummy_out()),
            mk_view(vec![("Read", "read a file"), ("Write", "write a file")]),
        );
        let out = tool
            .call(json!({"query": "read"}), fresh_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(out.data["result_count"], json!(1));
        assert_eq!(out.data["results"][0]["name"], json!("Read"));
    }

    #[tokio::test]
    async fn rejects_empty_query() {
        let tool = ToolSearchTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(json!({"query": ""}), fresh_ctx(), fresh_tx())
            .await
            .expect_err("empty");
        assert!(format!("{err}").contains("query is empty"));
    }
}
