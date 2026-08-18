//! `ListAgents` (alias `ListPeers`) — 2.1.232 `zy` / `SDd`.
//!
//! Lists other local live sessions this process can `SendMessage` to.
//! In-process teammates are addressed by the name they were spawned with, not
//! this list. Cloud / Remote Control rows are omitted (carve-out).

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Model-facing name (2.1.232 `zy`).
pub const LIST_AGENTS_TOOL_NAME: &str = "ListAgents";
/// Legacy alias (2.1.232 `SDd`).
pub const LIST_PEERS_TOOL_NAME: &str = "ListPeers";

const DESCRIPTION: &str = concat!(
    "Lists other local Claude sessions on this machine you can SendMessage to. ",
    "In-process teammates are addressed by the name they were spawned with, not this list. ",
    "Names are the address: send with `SendMessage({to: \"<name>\", message: \"...\"})`, ",
    "copying the name exactly as a row prints it. Append a row's ` [ref]` only when the ",
    "bare name is not enough — two rows share it, or an error asks you to disambiguate."
);

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "channel": {
                "type": "string",
                "description": "Not available in this build; leave unset."
            },
            "q": {
                "type": "string",
                "maxLength": 256,
                "description": "Not available in this build; leave unset."
            }
        }
    })
});

static OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "listing": { "type": "string", "description": "Formatted list of reachable agents" }
        },
        "required": ["listing"]
    })
});

/// `ListAgents` tool.
pub struct ListAgentsTool {
    _ctx: BuiltinToolContext,
}

impl ListAgentsTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { _ctx: ctx }
    }
}

fn format_listing() -> String {
    let Some(dir) = traits::live_sessions::process_dir() else {
        return "No reachable agents.".into();
    };
    let self_id = traits::live_sessions::process_session_id();
    let Ok(live) = dir.list_live() else {
        return "No reachable agents.".into();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut rows = Vec::new();
    for rec in live {
        if self_id.as_deref().is_some_and(|id| rec.sid() == id) {
            continue;
        }
        let name = rec.display_name();
        let rref = session_ref(rec.sid(), rec.pid);
        let kind = rec.kind.as_deref().unwrap_or("session");
        let started = rec
            .started_at
            .map(|t| format!("started {} ago", rel_ms(now.saturating_sub(t))))
            .unwrap_or_default();
        let mut bits = vec![format!("{name} [{rref}]"), kind.to_string()];
        if !started.is_empty() {
            bits.push(started);
        }
        rows.push(format!("  {}", bits.join("  ·  ")));
    }
    if rows.is_empty() {
        return "No reachable agents.".into();
    }
    format!("Peer sessions ({}):\n{}", rows.len(), rows.join("\n"))
}

fn session_ref(session_id: &str, pid: u32) -> String {
    let hex: String = session_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect();
    if hex.len() >= 6 {
        hex.to_ascii_lowercase()
    } else {
        format!("{pid:08x}")
    }
}

fn rel_ms(ms: i64) -> String {
    if ms < 60_000 {
        format!("{}s", (ms / 1000).max(0))
    } else if ms < 3_600_000 {
        format!("{}m", ms / 60_000)
    } else {
        format!("{}h", ms / 3_600_000)
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &str {
        LIST_AGENTS_TOOL_NAME
    }

    fn aliases(&self) -> &[&str] {
        &[LIST_PEERS_TOOL_NAME]
    }

    fn search_hint(&self) -> Option<&str> {
        Some("list agents you can SendMessage to")
    }

    fn user_facing_name(&self) -> Option<&str> {
        Some("ListAgents")
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&OUTPUT_SCHEMA)
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        traits::live_sessions::cross_session_messaging_enabled()
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ListAgents is read-only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        DESCRIPTION.into()
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let listing = format_listing();
        Ok(ToolCallResult {
            data: json!({ "listing": listing }),
            model_content: Some(listing),
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_byte_exact() {
        assert_eq!(LIST_AGENTS_TOOL_NAME, "ListAgents");
        assert_eq!(LIST_PEERS_TOOL_NAME, "ListPeers");
    }

    #[test]
    fn empty_listing_without_process_dir() {
        assert_eq!(format_listing(), "No reachable agents.");
    }
}
