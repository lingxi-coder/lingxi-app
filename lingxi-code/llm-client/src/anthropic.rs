//! Anthropic protocol helpers.

use serde_json::Value;

use crate::{ServerToolUsage, TokenUsage, Usage};

/// Normalize Anthropic usage JSON into independent billable token buckets.
#[must_use]
pub fn normalize_anthropic_usage(value: &Value) -> Usage {
    Usage {
        billable_tokens: TokenUsage {
            input: u64_field(value, "input_tokens"),
            output: u64_field(value, "output_tokens"),
            cache_write: u64_field(value, "cache_creation_input_tokens"),
            cache_read: u64_field(value, "cache_read_input_tokens"),
            reasoning_output: u64_field(value, "reasoning_output_tokens"),
        },
        server_tool_use: server_tool_usage(value),
        provider_metadata: value.clone(),
        ..Usage::default()
    }
}

fn server_tool_usage(value: &Value) -> Option<ServerToolUsage> {
    let server_tool_use = value.get("server_tool_use")?;
    Some(ServerToolUsage {
        web_search_requests: u64_field(server_tool_use, "web_search_requests"),
    })
}

fn u64_field(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or_default()
}
