//! `WaitForMcpServers` — 2.1.238 `Sdl` (`cc-238.js @229642127`), name `Qze`
//! (`cc-238.js @222784661`).
//!
//! Blocks for up to [`WAIT_TIMEOUT_MS`] while the named (or all pending) MCP
//! servers finish connecting, then reports each server's settled bucket. Ported
//! because without it the model has no way to wait out a still-connecting
//! server whose tools are not yet in the wire tool list.
//!
//! NOT new in 2.1.238 — 2.1.220 registers `WaitForMcpServers` too (`grep -acoF`
//! = 4 in both binaries). What 2.1.238 *added* is the `getTools` (`iJ`) tail
//! block that force-adds it when servers are pending and `ToolSearch` is absent:
//!
//! ```text
//! if(eZf()&&!l.some((c)=>il(c,y0))&&!l.some((c)=>il(c,Qze)))l=[...l,...Ohe([Sdl],e)];
//! ```
//!
//! The port splits the oracle's two-path enablement across `is_enabled` (the
//! "any pending server" leg, read through the registry's synchronous mirror)
//! and `ConversationOrchestrator::filtered_available_tools` (the "ToolSearch is
//! not in the list" leg). See the `is_enabled` doc below.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::BTreeSet;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Model-facing name (2.1.238 `Qze`, `cc-238.js @222784661`).
pub const WAIT_FOR_MCP_SERVERS_TOOL_NAME: &str = "WaitForMcpServers";

/// Oracle `C8v = 5000` (`cc-238.js @229641916`) — the wall-clock ceiling on the
/// pending-poll loop.
pub const WAIT_TIMEOUT_MS: u64 = 5_000;

/// Oracle `await Pr(50, r.signal)` — the poll interval inside the wait loop.
const POLL_INTERVAL_MS: u64 = 50;

/// Oracle `EAa()` (`cc-238.js @222783975`) — used verbatim for BOTH
/// `description()` and `prompt()` (`async description(){return EAa()},
/// async prompt(){return EAa()}`). The oracle builds it as an array of lines
/// `.join("\n")`, so there is no trailing newline.
const DESCRIPTION: &str = concat!(
    "Wait for MCP servers that are still connecting and whose tools are not\n",
    "yet in your tool list. Pass `servers` to wait for specific ones, or omit\n",
    "it to wait for all pending servers.\n",
    "\n",
    "If the user's request needs tools from a still-connecting server, call this\n",
    "tool to wait for it. Once it connects, its tools will be added to your tool\n",
    "list and you can use them directly. Returns ready=true when servers are\n",
    "ready, ready=false if they failed to connect, need authentication, or are\n",
    "disabled.\n",
    "\n",
    "You do not need to ask the user for confirmation to use this tool."
);

/// Oracle `k8v` (`cc-238.js @229641990`):
/// `be({servers:dt(H()).optional().describe("Server names to wait for (default: all pending)")})`.
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "servers": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Server names to wait for (default: all pending)"
            }
        }
    })
});

/// Oracle `A8v` (`cc-238.js @229642095`). `cached` and `unconfigured` are
/// `.optional()` there; the port's registry models neither a discovery-cache
/// bucket nor an "unconfigured remote" error code, so both keys are omitted
/// from every result rather than emitted empty.
static OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "ready":        { "type": "boolean" },
            "connected":    { "type": "array", "items": { "type": "string" } },
            "cached":       { "type": "array", "items": { "type": "string" } },
            "failed":       { "type": "array", "items": { "type": "string" } },
            "stillPending": { "type": "array", "items": { "type": "string" } },
            "needsAuth":    { "type": "array", "items": { "type": "string" } },
            "disabled":     { "type": "array", "items": { "type": "string" } },
            "unconfigured": { "type": "array", "items": { "type": "string" } },
            "unknown":      { "type": "array", "items": { "type": "string" } }
        },
        "required": [
            "ready", "connected", "failed", "stillPending",
            "needsAuth", "disabled", "unknown"
        ]
    })
});

/// `WaitForMcpServers` tool.
pub struct WaitForMcpServersTool {
    ctx: BuiltinToolContext,
}

impl WaitForMcpServersTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

/// The settled buckets the oracle's `call` produces.
#[derive(Default)]
struct Buckets {
    connected: Vec<String>,
    failed: Vec<String>,
    still_pending: Vec<String>,
    needs_auth: Vec<String>,
    disabled: Vec<String>,
    unknown: Vec<String>,
}

/// Bucket the current action states of the requested servers, plus the
/// requested names that no configured server matches (oracle `_`, "unknown").
fn bucket(states: &[(String, traits::McpActionState)], requested: &[String]) -> Buckets {
    use traits::McpActionState as S;
    let mut b = Buckets::default();
    for (name, state) in states {
        match state {
            S::Connected => b.connected.push(name.clone()),
            S::Pending => b.still_pending.push(name.clone()),
            S::NeedsAuth => b.needs_auth.push(name.clone()),
            S::Disabled => b.disabled.push(name.clone()),
            S::Failed => b.failed.push(name.clone()),
            // Oracle `default:` — the `switch(v.type)` has no `needs-approval`
            // arm, so such a client falls through and lands in NO bucket.
            S::NeedsApproval => {}
        }
    }
    // Oracle: `let y=new Set(c.map((v)=>au(v.name))),_=n.filter((v)=>!y.has(au(v)))`
    // — requested names with no matching client, compared on the NORMALIZED
    // name (`au` = `normalizeNameForMCP`).
    let seen: BTreeSet<String> = states
        .iter()
        .map(|(n, _)| mcp::normalization::normalize_name_for_mcp(n))
        .collect();
    for name in requested {
        if !seen.contains(&mcp::normalization::normalize_name_for_mcp(name)) {
            b.unknown.push(name.clone());
        }
    }
    b
}

/// Restrict a full action-state listing to the requested server names, matching
/// on the raw name OR its normalized form (oracle
/// `i=()=>Zpr(t).filter((v)=>n.includes(v.name)||o.has(au(v.name)))`).
fn select(
    states: Vec<(String, traits::McpActionState)>,
    requested: &[String],
) -> Vec<(String, traits::McpActionState)> {
    let normalized: BTreeSet<String> = requested
        .iter()
        .map(|s| mcp::normalization::normalize_name_for_mcp(s))
        .collect();
    states
        .into_iter()
        .filter(|(name, _)| {
            requested.iter().any(|r| r == name)
                || normalized.contains(&mcp::normalization::normalize_name_for_mcp(name))
        })
        .collect()
}

/// Oracle `mapToolResultToToolResultBlockParam` (`cc-238.js @229644106`): a
/// `\n`-joined list of non-empty lines, `is_error` = `!ready`.
fn render_for_model(b: &Buckets, ready: bool) -> String {
    let mut lines = vec![format!("ready: {ready}")];
    if !b.connected.is_empty() {
        lines.push(format!(
            "Connected (their tools are now available \u{2014} call them directly): {}",
            b.connected.join(", ")
        ));
    }
    if !b.failed.is_empty() {
        lines.push(format!("Failed to connect: {}", b.failed.join(", ")));
    }
    if !b.still_pending.is_empty() {
        lines.push(format!(
            "Still connecting (try again or proceed without): {}",
            b.still_pending.join(", ")
        ));
    }
    if !b.needs_auth.is_empty() {
        lines.push(format!(
            "Needs authentication (ask the user to run /mcp): {}",
            b.needs_auth.join(", ")
        ));
    }
    if !b.disabled.is_empty() {
        lines.push(format!(
            "Disabled (ask the user to enable via /mcp): {}",
            b.disabled.join(", ")
        ));
    }
    if !b.unknown.is_empty() {
        lines.push(format!(
            "Unknown (no MCP server with this name is configured): {}",
            b.unknown.join(", ")
        ));
    }
    lines.join("\n")
}

#[async_trait]
impl Tool for WaitForMcpServersTool {
    fn name(&self) -> &str {
        WAIT_FOR_MCP_SERVERS_TOOL_NAME
    }

    fn user_facing_name(&self) -> Option<&str> {
        Some("MCP Wait For Servers")
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&OUTPUT_SCHEMA)
    }

    /// Oracle `maxResultSizeChars:1e4`.
    fn max_result_size_chars(&self) -> usize {
        10_000
    }

    /// Oracle `isEnabled(){return R8v(Gi(),b7e()??[])}` where
    ///
    /// ```text
    /// function R8v(e,t){if(KU()&&e1e(e)&&!QLe(Fo(e)))return!1;return bdl(t).length>0}
    /// ```
    ///
    /// (`cc-238.js @229641674`). The `bdl(t).length>0` leg — "at least one MCP
    /// client is `type === "pending"`" — is read here through
    /// [`mcp::McpRegistry::has_pending_servers`], the synchronous mirror the
    /// tool-list seam refreshes before assembling a request (the state itself
    /// sits behind an async lock, which this synchronous trait method cannot
    /// take).
    ///
    /// The `KU()` leg — "ToolSearch is already covering deferred discovery for
    /// this model, so do not also advertise a waiter" — is applied by the
    /// caller, exactly as the oracle's own `getTools` (`iJ`) tail block applies
    /// it (`!l.some((c)=>il(c,y0))`, `y0="ToolSearch"`): composed over both
    /// paths, upstream advertises this tool iff there are pending servers AND
    /// `ToolSearch` is not in the final list. See
    /// `ConversationOrchestrator::filtered_available_tools`.
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        self.ctx
            .mcp_registry
            .as_ref()
            .is_some_and(|r| r.has_pending_servers())
    }

    /// Oracle `isReadOnly(){return!0}`.
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    /// Oracle `isConcurrencySafe(){return!1}`.
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }

    async fn check_permissions(&self, input: &Value, _: &ToolUseContext) -> PermissionResult {
        // Oracle: `async checkPermissions(e){return{behavior:"allow",updatedInput:e}}`.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "WaitForMcpServers is read-only".into(),
            },
            updated_input: Some(input.clone()),
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
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let Some(registry) = self.ctx.mcp_registry.as_ref() else {
            return Err(ToolError::Io(
                "WaitForMcpServers: MCP registry not configured".into(),
            ));
        };

        // Oracle: `n = e.servers?.length ? e.servers : bdl(Zpr(t))` — the
        // explicit list when non-empty, otherwise every currently-pending
        // server.
        let explicit: Vec<String> = input
            .get("servers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let requested: Vec<String> = if explicit.is_empty() {
            registry
                .action_states()
                .await
                .into_iter()
                .filter(|(_, s)| *s == traits::McpActionState::Pending)
                .map(|(n, _)| n)
                .collect()
        } else {
            explicit
        };

        // Oracle wait loop: poll every 50ms while any selected client is still
        // `pending` and the 5s budget has not elapsed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(WAIT_TIMEOUT_MS);
        loop {
            let now = select(registry.action_states().await, &requested);
            let any_pending = now
                .iter()
                .any(|(_, s)| *s == traits::McpActionState::Pending);
            if !any_pending || std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }

        let settled = select(registry.action_states().await, &requested);
        let buckets = bucket(&settled, &requested);
        // Oracle `S`: ready when nothing is still pending, failed, needing auth,
        // disabled or unknown. (`cached` and `unconfigured` do NOT block.)
        let ready = buckets.still_pending.is_empty()
            && buckets.failed.is_empty()
            && buckets.needs_auth.is_empty()
            && buckets.disabled.is_empty()
            && buckets.unknown.is_empty();

        let model_content = render_for_model(&buckets, ready);
        Ok(ToolCallResult {
            data: json!({
                "ready": ready,
                "connected": buckets.connected,
                "failed": buckets.failed,
                "stillPending": buckets.still_pending,
                "needsAuth": buckets.needs_auth,
                "disabled": buckets.disabled,
                "unknown": buckets.unknown,
            }),
            model_content: Some(model_content),
            new_messages: vec![],
            context_modifier: None,
            // Oracle: `is_error:!e.ready`.
            is_error: !ready,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_byte_exact() {
        assert_eq!(WAIT_FOR_MCP_SERVERS_TOOL_NAME, "WaitForMcpServers");
    }

    /// 2.1.238 `EAa()` (`cc-238.js @222783975`) joined with `\n`. Both the
    /// description and the prompt are this exact string.
    #[test]
    fn description_matches_the_oracle_join() {
        assert!(DESCRIPTION.starts_with(
            "Wait for MCP servers that are still connecting and whose tools are not\nyet in your tool list."
        ));
        assert!(DESCRIPTION
            .ends_with("You do not need to ask the user for confirmation to use this tool."));
        assert!(
            !DESCRIPTION.ends_with('\n'),
            "the oracle joins lines with \\n and does not append a trailing newline"
        );
        assert!(
            DESCRIPTION.contains("\n\nIf the user's request needs tools"),
            "the oracle's array carries an empty line before the second paragraph"
        );
    }

    /// The `mapToolResultToToolResultBlockParam` line set, including the em dash
    /// in the Connected line (`—`, NOT a hyphen).
    #[test]
    fn result_renderer_matches_the_oracle_lines() {
        let b = Buckets {
            connected: vec!["a".into()],
            needs_auth: vec!["b".into()],
            ..Buckets::default()
        };
        assert_eq!(
            render_for_model(&b, false),
            "ready: false\n\
             Connected (their tools are now available \u{2014} call them directly): a\n\
             Needs authentication (ask the user to run /mcp): b"
        );
        assert_eq!(
            render_for_model(&Buckets::default(), true),
            "ready: true",
            "every empty bucket drops its line (oracle `.filter(Boolean)`)"
        );
    }

    /// A requested name with no configured server lands in `unknown`, compared
    /// on the NORMALIZED name (oracle `au`).
    #[test]
    fn unknown_bucket_compares_normalized_names() {
        let states = vec![("my.server".to_string(), traits::McpActionState::Connected)];
        let b = bucket(&states, &["my_server".into(), "nope".into()]);
        assert_eq!(b.connected, vec!["my.server".to_string()]);
        assert_eq!(
            b.unknown,
            vec!["nope".to_string()],
            "`my_server` normalizes onto the configured `my.server`"
        );
    }

    /// Readiness ignores `connected` but is blocked by every other bucket.
    #[test]
    fn ready_requires_every_blocking_bucket_empty() {
        let states = vec![("s".to_string(), traits::McpActionState::Pending)];
        let b = bucket(&states, &["s".into()]);
        assert_eq!(b.still_pending, vec!["s".to_string()]);
        assert!(b.unknown.is_empty());
    }
}
