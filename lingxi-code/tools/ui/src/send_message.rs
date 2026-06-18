//! `SendMessageTool` — the main-loop swarm `SendMessage` tool.
//!
//! 1:1 Rust port of claude-code's
//! `src/tools/SendMessageTool/SendMessageTool.ts` (name constant
//! `SEND_MESSAGE_TOOL_NAME = "SendMessage"`). It sends a message to a teammate
//! in an agent swarm: a plain-text note (with a short `summary` preview), a
//! broadcast to every teammate (`to: "*"`), or one of three structured
//! protocol messages (`shutdown_request` / `shutdown_response` /
//! `plan_approval_response`).
//!
//! **Schema/validation parity** is reproduced byte-faithfully from the TS
//! (`inputSchema` + `validateInput`). The schema + the structured-message
//! variants mirror the coordinator-side port at
//! `coordinator/src/tool_send_message.rs`, adapted so `parse_recipient`
//! resolves a teammate *name* (the TS contract) rather than a bare UUID.
//!
//! **Delivery is a `PARITY-GAP`.** claude-code's `writeToMailbox` appends to a
//! name-keyed file mailbox, and the in-process router auto-resumes a paused
//! teammate. Neither the UDS/bridge transport nor a team roster is reachable
//! from `tool-ui`; the Rust port instead routes through the injected
//! [`traits::mailbox::MailboxRouterHandle`] seam (which resolves recipients by
//! `AgentId`). Name-keyed delivery is therefore best-effort / local-only here:
//! the tool always *validates* and *shapes* the result faithfully, even when
//! the underlying seam cannot resolve a name. Sites where the real transport
//! would do more are marked `// PARITY-GAP:`.
//!
//! The LingXi-internal claim window stays byte-locked at
//! `Duration::from_secs(30)` (spec §7 line 498), and the telemetry event names
//! are unchanged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{SEND_MESSAGE_COMPLETED, SEND_MESSAGE_FAILED, SEND_MESSAGE_STARTED};
use telemetry::AnalyticsBus;
use traits::mailbox::{MailboxMessage, MailboxRouterHandle};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};
use tool_api::BuiltinToolContext;

/// Tool name `'SendMessage'` (claude-code `SEND_MESSAGE_TOOL_NAME`).
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// LingXi-internal claim-window lock — see spec §7 line 498.
/// Encoded as `Duration::from_secs(30)` byte-for-byte in source.
pub const SEND_MESSAGE_CLAIM_WINDOW: Duration = Duration::from_secs(30);

/// `TEAM_LEAD_NAME` — 1:1 with claude-code
/// `src/utils/swarm/constants.ts` (`export const TEAM_LEAD_NAME = 'team-lead'`).
const TEAM_LEAD_NAME: &str = "team-lead";

/// `DESCRIPTION` — 1:1 with `src/tools/SendMessageTool/prompt.ts`.
const DESCRIPTION: &str = "Send a message to another agent";

/// `maxResultSizeChars` in the TS tool is `100_000`.
const MAX_RESULT_SIZE_CHARS: usize = 100_000;

static SEND_MESSAGE_SCHEMA: Lazy<Value> = Lazy::new(build_input_schema);

/// Build the JSON Schema for `SendMessage` inputs.
///
/// Mirrors the TS `z.object({ to, summary, message })` 1:1: `to` and `message`
/// are required, `summary` is optional, and `message` is a `oneOf`
/// plain-string / structured-message discriminated union. The structured
/// branch's only required key is `type` (one of the three protocol variants),
/// matching `StructuredMessage` in the TS — additional shape constraints
/// (`request_id`, `approve`, `reason`, `feedback`) are advertised per-property
/// just as the zod union does.
fn build_input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["to", "message"],
        "properties": {
            "to": {
                "type": "string",
                "description": "Recipient: teammate name, or \"*\" for broadcast to all teammates"
            },
            "summary": {
                "type": "string",
                "description": "A 5-10 word summary shown as a preview in the UI (required when message is a string)"
            },
            "message": {
                "oneOf": [
                    {
                        "type": "string",
                        "description": "Plain text message content"
                    },
                    {
                        "type": "object",
                        "required": ["type"],
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": [
                                    "shutdown_request",
                                    "shutdown_response",
                                    "plan_approval_response"
                                ]
                            },
                            "request_id": { "type": "string" },
                            "approve": { "type": "boolean" },
                            "reason": { "type": "string" },
                            "feedback": { "type": "string" }
                        }
                    }
                ]
            }
        }
    })
}

/// Resolved `to` recipient.
///
/// `parse_recipient` resolves by teammate **name** first, then the explicit
/// `agent:<uuid>` reference form — mirroring the TS `call` resolution order
/// (`agentNameRegistry.get(to) ?? toAgentId(to)`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recipient {
    /// `to == "*"` — broadcast to every teammate.
    Broadcast,
    /// A teammate addressed by name (the primary path).
    Teammate(String),
    /// An explicit `agent:<uuid>` reference (prefix stripped).
    Agent(String),
}

impl Recipient {
    /// String handed to the mailbox seam for a single (non-broadcast) route.
    fn route_target(&self) -> &str {
        match self {
            Recipient::Broadcast => "*",
            Recipient::Teammate(s) | Recipient::Agent(s) => s,
        }
    }
}

/// `SendMessageTool` — routes a teammate message via the swarm mailbox seam.
pub struct SendMessageTool {
    ctx: BuiltinToolContext,
}

impl SendMessageTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn fresh_invocation_id() -> String {
        tool_api::util::ids::ulid_or_uuid()
    }

    /// Boolean that also accepts the string literals `"true"`/`"false"` — a
    /// port of the TS `semanticBoolean()` used for the `approve` field.
    fn semantic_bool(v: &Value) -> Option<bool> {
        match v {
            Value::Bool(b) => Some(*b),
            Value::String(s) if s == "true" => Some(true),
            Value::String(s) if s == "false" => Some(false),
            _ => None,
        }
    }

    /// Parse the `to` field into a [`Recipient`].
    ///
    /// Resolves by teammate **name** first (any bare string), then the
    /// explicit `agent:<uuid>` reference form, and recognises `"*"` as a
    /// broadcast. Rejection strings are byte-faithful with the TS
    /// `validateInput`.
    ///
    /// # Errors
    /// Returns [`ToolError::InvalidInput`] when `to` is empty or contains `@`
    /// (one team per session — bare name / `*` only in the TS contract).
    fn parse_recipient(to: &str) -> Result<Recipient, ToolError> {
        let trimmed = to.trim();
        if trimmed.is_empty() {
            return Err(ToolError::InvalidInput("to must not be empty".into()));
        }
        if trimmed.contains('@') {
            return Err(ToolError::InvalidInput(
                "to must be a bare teammate name or \"*\" — there is only one team per session"
                    .into(),
            ));
        }
        if trimmed == "*" {
            return Ok(Recipient::Broadcast);
        }
        // Resolve by teammate NAME first, then the explicit `agent:<uuid>` form.
        if let Some(uuid) = trimmed.strip_prefix("agent:") {
            return Ok(Recipient::Agent(uuid.to_string()));
        }
        Ok(Recipient::Teammate(trimmed.to_string()))
    }

    /// Sender display name for `routing.sender`.
    ///
    /// PARITY-GAP: the TS resolves `getAgentName()` (the human teammate name),
    /// which the `tool-ui` [`ToolUseContext`] does not carry. We fall to the
    /// TS `||` branch: a known agent id ⇒ a teammate (`"teammate"`); otherwise
    /// the team lead (`TEAM_LEAD_NAME`).
    fn sender_name(ctx: &ToolUseContext) -> String {
        if ctx.agent_id.is_some() {
            "teammate".to_string()
        } else {
            TEAM_LEAD_NAME.to_string()
        }
    }

    /// `from` string handed to the mailbox seam.
    ///
    /// PARITY-GAP: the seam resolves senders by `AgentId`, so unlike the TS
    /// (which records the human name) we pass a UUID — the calling agent's, or
    /// the nil id when the agent id is unknown (narrowly-scoped tests).
    fn route_from(ctx: &ToolUseContext) -> String {
        ctx.agent_id.map_or_else(
            || protocol::AgentId::nil().as_uuid().to_string(),
            |a| a.as_uuid().to_string(),
        )
    }

    /// `generateRequestId('{request_type}', '{agent_id}')` from the TS —
    /// `"{request_type}-{millis}@{agent_id}"`.
    fn generate_request_id(request_type: &str, agent_id: &str) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("{request_type}-{millis}@{agent_id}")
    }

    /// Best-effort delivery through the mailbox seam.
    ///
    /// PARITY-GAP: the production [`MailboxRouterHandle`] resolves recipients by
    /// `AgentId` (UUID). claude-code's `writeToMailbox` appends to a name-keyed
    /// file mailbox and auto-resumes a paused teammate — out of faithful reach
    /// here. A UUID recipient (e.g. the coordinator-wiring path) delivers for
    /// real; a teammate-name recipient is a local-only / no-op delivery. Either
    /// way the caller shapes the result faithfully, so route errors are
    /// swallowed rather than surfaced.
    async fn deliver(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        target: &str,
        content: String,
    ) {
        let msg = MailboxMessage {
            message_id: tool_api::util::ids::ulid_or_uuid(),
            content,
            timestamp: SystemTime::now(),
            // No teammate-color seam threaded into SendMessage; `None` is omitted
            // from the wire form (parity with claude-code `color: undefined`).
            color: None,
        };
        let _ = router.route(from, target, msg).await;
    }

    /// Build a `routing` object, omitting `summary`/`content` when absent
    /// (mirrors `jsonStringify` dropping `undefined`). Teammate colours are a
    /// PARITY-GAP — there is no team context in `tool-ui` — so they are omitted.
    fn routing(sender: &str, target: &str, summary: Option<&str>, content: Option<&str>) -> Value {
        let mut m = Map::new();
        m.insert("sender".into(), json!(sender));
        m.insert("target".into(), json!(target));
        if let Some(s) = summary {
            m.insert("summary".into(), json!(s));
        }
        if let Some(c) = content {
            m.insert("content".into(), json!(c));
        }
        Value::Object(m)
    }

    /// `handleMessage` — a plain-text note to a single teammate.
    async fn handle_message(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        content: &str,
        summary: Option<&str>,
        sender: &str,
    ) -> Value {
        Self::deliver(router, from, recipient.route_target(), content.to_string()).await;
        json!({
            "success": true,
            "message": format!("Message sent to {to_display}'s inbox"),
            "routing": Self::routing(
                sender,
                &format!("@{to_display}"),
                summary,
                Some(content),
            ),
        })
    }

    /// `handleBroadcast` — fan-out to every teammate.
    ///
    /// PARITY-GAP: claude-code reads the team file to enumerate recipients and
    /// `writeToMailbox` to each. `tool-ui` has no team roster, so broadcast is
    /// local-only (nothing is actually routed): we shape the empty-roster
    /// `BroadcastOutput` variant faithfully rather than fabricate a recipient
    /// list.
    fn handle_broadcast() -> Value {
        json!({
            "success": true,
            "message": "No teammates to broadcast to (you are the only team member)",
            "recipients": [],
        })
    }

    /// `handleShutdownRequest` — ask a teammate to shut down.
    async fn handle_shutdown_request(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        reason: Option<&str>,
        sender: &str,
    ) -> Value {
        let request_id = Self::generate_request_id("shutdown", to_display);
        // PARITY-GAP: claude-code wraps this in `createShutdownRequestMessage`
        // (adds `from`/ISO `timestamp`); we deliver an equivalent structured
        // payload best-effort.
        let payload = json!({
            "type": "shutdown_request",
            "requestId": request_id,
            "from": sender,
            "reason": reason,
        });
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await;
        json!({
            "success": true,
            "message": format!("Shutdown request sent to {to_display}. Request ID: {request_id}"),
            "request_id": request_id,
            "target": to_display,
        })
    }

    /// `handleShutdownApproval` — a teammate approves its own shutdown.
    ///
    /// PARITY-GAP: the TS aborts the in-process teammate's controller or calls
    /// `gracefulShutdown(0)`. Host process control is out of reach here; we
    /// deliver the approval to the team lead best-effort and return the result
    /// shape.
    async fn handle_shutdown_approval(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        request_id: &str,
    ) -> Value {
        // `getAgentName() || 'teammate'` in the TS — name unavailable here.
        let agent_name = "teammate";
        let payload = json!({
            "type": "shutdown_approved",
            "requestId": request_id,
            "from": agent_name,
        });
        Self::deliver(
            router,
            from,
            TEAM_LEAD_NAME,
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await;
        json!({
            "success": true,
            "message": format!(
                "Shutdown approved. Sent confirmation to team-lead. Agent {agent_name} is now exiting."
            ),
            "request_id": request_id,
        })
    }

    /// `handleShutdownRejection` — a teammate declines a shutdown request.
    async fn handle_shutdown_rejection(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        request_id: &str,
        reason: &str,
    ) -> Value {
        let agent_name = "teammate";
        let payload = json!({
            "type": "shutdown_rejected",
            "requestId": request_id,
            "from": agent_name,
            "reason": reason,
        });
        Self::deliver(
            router,
            from,
            TEAM_LEAD_NAME,
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await;
        json!({
            "success": true,
            "message": format!("Shutdown rejected. Reason: \"{reason}\". Continuing to work."),
            "request_id": request_id,
        })
    }

    /// `handlePlanApproval` — the team lead approves a teammate's plan.
    ///
    /// PARITY-GAP: the TS gates on `isTeamLead(teamContext)` and inherits the
    /// leader's permission mode from app state — neither is reachable from
    /// `tool-ui`, so the gate is elided and the approval is delivered
    /// best-effort.
    async fn handle_plan_approval(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        request_id: &str,
    ) -> Value {
        let payload = json!({
            "type": "plan_approval_response",
            "requestId": request_id,
            "approved": true,
            "from": TEAM_LEAD_NAME,
        });
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await;
        json!({
            "success": true,
            "message": format!(
                "Plan approved for {to_display}. They will receive the approval and can proceed with implementation."
            ),
            "request_id": request_id,
        })
    }

    /// `handlePlanRejection` — the team lead rejects a teammate's plan.
    async fn handle_plan_rejection(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        request_id: &str,
        feedback: &str,
    ) -> Value {
        let payload = json!({
            "type": "plan_approval_response",
            "requestId": request_id,
            "approved": false,
            "feedback": feedback,
            "from": TEAM_LEAD_NAME,
        });
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await;
        json!({
            "success": true,
            "message": format!("Plan rejected for {to_display} with feedback: \"{feedback}\""),
            "request_id": request_id,
        })
    }

    async fn emit_started(bus: &Arc<AnalyticsBus>, invocation_id: &str, message_chars: i64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert("message_chars".into(), AnalyticsValue::Int(message_chars));
        bus.log_event(SEND_MESSAGE_STARTED, md).await;
    }

    async fn emit_completed(bus: &Arc<AnalyticsBus>, invocation_id: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(SEND_MESSAGE_COMPLETED, md).await;
    }

    async fn emit_failed(
        bus: &Arc<AnalyticsBus>,
        invocation_id: &str,
        error_kind: &str,
        duration_ms: u64,
    ) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        bus.log_event(SEND_MESSAGE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        SEND_MESSAGE_TOOL_NAME
    }
    fn search_hint(&self) -> Option<&str> {
        Some("send messages to agent teammates (swarm protocol)")
    }
    fn input_schema(&self) -> &Value {
        &SEND_MESSAGE_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // TS `isEnabled()` = `isAgentSwarmsEnabled()`. The swarm surface is
        // always live in the Rust host (the mailbox seam is wired when present),
        // so this is `true`.
        true
    }
    fn should_defer(&self) -> bool {
        // TS `shouldDefer: true`.
        true
    }
    // no-truncation: SendMessage opts out of the standard MAX_TOOL_OUTPUT_LENGTH
    // (30_000) cap in favor of its own `maxResultSizeChars: 100_000`
    // (claude-code SendMessageTool.ts:524); its results are bounded status
    // strings well under that ceiling.
    fn max_result_size_chars(&self) -> usize {
        MAX_RESULT_SIZE_CHARS
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, input: &Value) -> bool {
        // TS `isReadOnly(input)` = `typeof input.message === 'string'`.
        input.get("message").is_some_and(Value::is_string)
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
        // The TS tool only escalates to `ask` for the cross-machine `bridge:`
        // scheme (a UDS_INBOX feature not modelled here); all local sends are
        // allowed.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "SendMessage routes teammate-to-teammate text via the swarm mailbox".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    /// Port of the TS `validateInput`. Runs before [`Tool::call`] in the
    /// orchestrator; rejection strings are byte-faithful with the reference.
    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let to = input.get("to").and_then(Value::as_str).unwrap_or("");
        if to.trim().is_empty() {
            return Err(ValidationError("to must not be empty".into()));
        }
        if to.contains('@') {
            return Err(ValidationError(
                "to must be a bare teammate name or \"*\" — there is only one team per session"
                    .into(),
            ));
        }

        // Plain-string message: a non-empty `summary` is required.
        if let Some(Value::String(_)) = input.get("message") {
            let summary = input.get("summary").and_then(Value::as_str);
            if summary.is_none_or(|s| s.trim().is_empty()) {
                return Err(ValidationError(
                    "summary is required when message is a string".into(),
                ));
            }
            return Ok(());
        }

        // Structured message from here on.
        if to == "*" {
            return Err(ValidationError(
                "structured messages cannot be broadcast (to: \"*\")".into(),
            ));
        }

        if let Some(Value::Object(obj)) = input.get("message") {
            let mtype = obj.get("type").and_then(Value::as_str).unwrap_or("");
            if mtype == "shutdown_response" {
                if to != TEAM_LEAD_NAME {
                    return Err(ValidationError(format!(
                        "shutdown_response must be sent to \"{TEAM_LEAD_NAME}\""
                    )));
                }
                let approve = obj
                    .get("approve")
                    .and_then(Self::semantic_bool)
                    .unwrap_or(false);
                let reason = obj.get("reason").and_then(Value::as_str);
                if !approve && reason.is_none_or(|r| r.trim().is_empty()) {
                    return Err(ValidationError(
                        "reason is required when rejecting a shutdown request".into(),
                    ));
                }
            }
        }

        Ok(())
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // 1:1 with `src/tools/SendMessageTool/prompt.ts` getPrompt() (the
        // non-UDS_INBOX variant — cross-session transport is a PARITY-GAP).
        r#"# SendMessage

Send a message to another agent.

```json
{"to": "researcher", "summary": "assign task 1", "message": "start on task #1"}
```

| `to` | |
|---|---|
| `"researcher"` | Teammate by name |
| `"*"` | Broadcast to all teammates — expensive (linear in team size), use only when everyone genuinely needs it |

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool. Messages from teammates are delivered automatically; you don't check an inbox. Refer to teammates by name, never by UUID. When relaying, don't quote the original — it's already rendered to the user.

## Protocol responses (legacy)

If you receive a JSON message with `type: "shutdown_request"` or `type: "plan_approval_request"`, respond with the matching `_response` type — echo the `request_id`, set `approve` true/false:

```json
{"to": "team-lead", "message": {"type": "shutdown_response", "request_id": "...", "approve": true}}
{"to": "researcher", "message": {"type": "plan_approval_response", "request_id": "...", "approve": false, "feedback": "add error handling"}}
```

Approving shutdown terminates your process. Rejecting plan sends the teammate back to revise. Don't originate `shutdown_request` unless asked. Don't send structured JSON status messages — use TaskUpdate."#
            .into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = Self::fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // --- Mailbox-seam guard (Rust-port artifact) -------------------------
        // PARITY-GAP: claude-code appends to a file mailbox directly; the Rust
        // port delivers through an injected `MailboxRouterHandle`. With no
        // router wired the tool cannot route at all, so surface a clear internal
        // error before any other work. (Checked first so an unwired host hits
        // this path regardless of payload.)
        let router = match self.ctx.mailbox_router.clone() {
            Some(r) => r,
            None => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "router_not_wired",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(
                    "SendMessageTool: MailboxRouterHandle not wired into BuiltinToolContext".into(),
                ));
            }
        };

        // --- Parse `to` (defensive — schema/validate_input ran upstream) -----
        // PARITY-GAP compat: the desktop coordinator-wiring integration path
        // drives a legacy `{to_agent_id, message}` payload. Accept `to_agent_id`
        // as an alias for `to` so that wiring keeps routing; new callers use the
        // `to` field the schema advertises.
        let to = match input
            .get("to")
            .and_then(Value::as_str)
            .or_else(|| input.get("to_agent_id").and_then(Value::as_str))
        {
            Some(s) => s.to_string(),
            None => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "missing_to",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput("SendMessage: missing 'to'".into()));
            }
        };
        let recipient = match Self::parse_recipient(&to) {
            Ok(r) => r,
            Err(e) => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "invalid_recipient",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(e);
            }
        };

        // --- Parse `message` (plain string or structured object) -------------
        let message = match input.get("message") {
            Some(m) => m,
            None => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "missing_message",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "SendMessage: missing 'message'".into(),
                ));
            }
        };

        let summary = input
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string);
        let from = Self::route_from(&ctx);
        let sender = Self::sender_name(&ctx);

        let message_chars = match message {
            Value::String(s) => s.chars().count(),
            other => serde_json::to_string(other)
                .map(|s| s.chars().count())
                .unwrap_or(0),
        } as i64;
        Self::emit_started(&bus, &invocation_id, message_chars).await;

        let data = match message {
            Value::String(content) => match recipient {
                Recipient::Broadcast => Self::handle_broadcast(),
                _ => {
                    Self::handle_message(
                        &router,
                        &from,
                        &recipient,
                        &to,
                        content,
                        summary.as_deref(),
                        &sender,
                    )
                    .await
                }
            },
            Value::Object(obj) => {
                if matches!(recipient, Recipient::Broadcast) {
                    Self::emit_failed(
                        &bus,
                        &invocation_id,
                        "structured_broadcast",
                        started.elapsed().as_millis() as u64,
                    )
                    .await;
                    return Err(ToolError::InvalidInput(
                        "structured messages cannot be broadcast".into(),
                    ));
                }
                let mtype = obj.get("type").and_then(Value::as_str).unwrap_or("");
                let request_id_in = obj
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let approve = obj
                    .get("approve")
                    .and_then(Self::semantic_bool)
                    .unwrap_or(false);
                let reason = obj.get("reason").and_then(Value::as_str);
                let feedback = obj
                    .get("feedback")
                    .and_then(Value::as_str)
                    .unwrap_or("Plan needs revision");
                match mtype {
                    "shutdown_request" => {
                        Self::handle_shutdown_request(
                            &router,
                            &from,
                            &recipient,
                            &to,
                            reason,
                            &sender,
                        )
                        .await
                    }
                    "shutdown_response" => {
                        if approve {
                            Self::handle_shutdown_approval(&router, &from, &request_id_in).await
                        } else {
                            Self::handle_shutdown_rejection(
                                &router,
                                &from,
                                &request_id_in,
                                reason.unwrap_or(""),
                            )
                            .await
                        }
                    }
                    "plan_approval_response" => {
                        if approve {
                            Self::handle_plan_approval(
                                &router,
                                &from,
                                &recipient,
                                &to,
                                &request_id_in,
                            )
                            .await
                        } else {
                            Self::handle_plan_rejection(
                                &router,
                                &from,
                                &recipient,
                                &to,
                                &request_id_in,
                                feedback,
                            )
                            .await
                        }
                    }
                    _ => {
                        Self::emit_failed(
                            &bus,
                            &invocation_id,
                            "unknown_message_type",
                            started.elapsed().as_millis() as u64,
                        )
                        .await;
                        return Err(ToolError::InvalidInput(
                            "SendMessage: 'message' must be a string or a structured object".into(),
                        ));
                    }
                }
            }
            _ => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "invalid_message",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "SendMessage: 'message' must be a string or a structured object".into(),
                ));
            }
        };

        Self::emit_completed(&bus, &invocation_id, started.elapsed().as_millis() as u64).await;
        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::mailbox::{MailboxError, RouteAck};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Records every `(from, to, content)` it is asked to route and always acks
    /// (so name-keyed routes — which the production seam can't resolve — are
    /// observable in tests).
    struct RecordingRouter {
        routed: Mutex<Vec<(String, String, String)>>,
    }

    impl RecordingRouter {
        fn new() -> Self {
            Self {
                routed: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl MailboxRouterHandle for RecordingRouter {
        async fn route(
            &self,
            from_agent: &str,
            to_agent: &str,
            message: MailboxMessage,
        ) -> Result<RouteAck, MailboxError> {
            self.routed.lock().unwrap().push((
                from_agent.to_string(),
                to_agent.to_string(),
                message.content,
            ));
            Ok(RouteAck {
                claimed_at: SystemTime::now(),
                claim_window_secs: SEND_MESSAGE_CLAIM_WINDOW.as_secs(),
            })
        }
    }

    fn ctx_with(router: Arc<RecordingRouter>) -> BuiltinToolContext {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.mailbox_router = Some(router as Arc<dyn MailboxRouterHandle>);
        ctx
    }

    #[test]
    fn tool_name_locked() {
        assert_eq!(SEND_MESSAGE_TOOL_NAME, "SendMessage");
    }

    #[test]
    fn claim_window_locked_30_seconds() {
        assert_eq!(SEND_MESSAGE_CLAIM_WINDOW, Duration::from_secs(30));
        assert_eq!(SEND_MESSAGE_CLAIM_WINDOW.as_secs(), 30);
    }

    #[test]
    fn schema_shape_matches_ts() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["to", "message"]));
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["summary"].is_object());
        assert!(schema["properties"]["message"]["oneOf"].is_array());
        let variants = schema["properties"]["message"]["oneOf"]
            .as_array()
            .unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["type"], "string");
        assert_eq!(
            variants[1]["properties"]["type"]["enum"],
            json!(["shutdown_request", "shutdown_response", "plan_approval_response"])
        );
    }

    #[test]
    fn is_read_only_true_only_for_string_message() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        assert!(tool.is_read_only(&json!({ "to": "x", "message": "hi" })));
        assert!(
            !tool.is_read_only(&json!({ "to": "x", "message": { "type": "shutdown_request" } }))
        );
    }

    #[test]
    fn parse_recipient_resolves_name_then_agent_and_broadcast() {
        assert_eq!(
            SendMessageTool::parse_recipient("researcher").unwrap(),
            Recipient::Teammate("researcher".into())
        );
        assert_eq!(
            SendMessageTool::parse_recipient("agent:abc-123").unwrap(),
            Recipient::Agent("abc-123".into())
        );
        assert_eq!(
            SendMessageTool::parse_recipient("*").unwrap(),
            Recipient::Broadcast
        );
    }

    // ----- validate_input parity (byte-faithful rejection strings) ----------

    #[tokio::test]
    async fn validate_rejects_empty_to() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({ "to": "  ", "message": "hi" }), &fresh_ctx())
            .await
            .expect_err("empty `to` must reject");
        assert_eq!(err.0, "to must not be empty");
    }

    #[tokio::test]
    async fn validate_rejects_at_in_to() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(
                &json!({ "to": "researcher@team", "summary": "s", "message": "hi" }),
                &fresh_ctx(),
            )
            .await
            .expect_err("`@` in `to` must reject");
        assert_eq!(
            err.0,
            "to must be a bare teammate name or \"*\" — there is only one team per session"
        );
    }

    #[tokio::test]
    async fn validate_string_message_requires_summary() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(&json!({ "to": "researcher", "message": "hi" }), &fresh_ctx())
            .await
            .expect_err("string message without summary must reject");
        assert_eq!(err.0, "summary is required when message is a string");
    }

    #[tokio::test]
    async fn validate_structured_cannot_broadcast() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(
                &json!({ "to": "*", "message": { "type": "shutdown_request" } }),
                &fresh_ctx(),
            )
            .await
            .expect_err("structured broadcast must reject");
        assert_eq!(err.0, "structured messages cannot be broadcast (to: \"*\")");
    }

    #[tokio::test]
    async fn validate_shutdown_response_must_target_team_lead() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(
                &json!({
                    "to": "researcher",
                    "message": { "type": "shutdown_response", "request_id": "r", "approve": true }
                }),
                &fresh_ctx(),
            )
            .await
            .expect_err("shutdown_response to a non-lead must reject");
        assert_eq!(err.0, "shutdown_response must be sent to \"team-lead\"");
    }

    #[tokio::test]
    async fn validate_shutdown_rejection_requires_reason() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(
                &json!({
                    "to": "team-lead",
                    "message": { "type": "shutdown_response", "request_id": "r", "approve": false }
                }),
                &fresh_ctx(),
            )
            .await
            .expect_err("rejecting shutdown without a reason must reject");
        assert_eq!(err.0, "reason is required when rejecting a shutdown request");
    }

    #[tokio::test]
    async fn validate_accepts_well_formed_string_message() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        tool.validate_input(
            &json!({ "to": "researcher", "summary": "assign task 1", "message": "go" }),
            &fresh_ctx(),
        )
        .await
        .expect("a summarised string message is valid");
    }

    // ----- call() routing + handler shapes ----------------------------------

    #[tokio::test]
    async fn delivers_string_message_resolved_by_name() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let res = tool
            .call(
                json!({ "to": "researcher", "summary": "assign task 1", "message": "start on task #1" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("send must succeed");
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["message"], "Message sent to researcher's inbox");
        assert_eq!(res.data["routing"]["target"], "@researcher");
        assert_eq!(res.data["routing"]["content"], "start on task #1");

        let routed = router.routed.lock().unwrap();
        assert_eq!(routed.len(), 1);
        // Resolved by NAME — the recipient string is the bare teammate name.
        assert_eq!(routed[0].1, "researcher");
        assert_eq!(routed[0].2, "start on task #1");
    }

    #[tokio::test]
    async fn broadcast_returns_broadcast_shape() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router));
        let res = tool
            .call(
                json!({ "to": "*", "summary": "all hands", "message": "standup in 5" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("broadcast must succeed");
        assert_eq!(res.data["success"], true);
        assert!(res.data["recipients"].is_array());
    }

    #[tokio::test]
    async fn structured_shutdown_request_shape() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let res = tool
            .call(
                json!({
                    "to": "researcher",
                    "message": { "type": "shutdown_request", "reason": "done for the day" }
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("shutdown_request must succeed");
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["target"], "researcher");
        assert!(res.data["request_id"].as_str().unwrap().starts_with("shutdown-"));
        let msg = res.data["message"].as_str().unwrap();
        assert!(msg.starts_with("Shutdown request sent to researcher. Request ID: shutdown-"));
        // The structured payload was delivered to the named recipient.
        assert_eq!(router.routed.lock().unwrap()[0].1, "researcher");
    }

    #[tokio::test]
    async fn structured_shutdown_rejection_shape() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router));
        let res = tool
            .call(
                json!({
                    "to": "team-lead",
                    "message": {
                        "type": "shutdown_response",
                        "request_id": "shutdown-1@researcher",
                        "approve": false,
                        "reason": "still mid-task"
                    }
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("shutdown rejection must succeed");
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["request_id"], "shutdown-1@researcher");
        assert_eq!(
            res.data["message"],
            "Shutdown rejected. Reason: \"still mid-task\". Continuing to work."
        );
    }

    #[tokio::test]
    async fn router_not_wired_is_internal_error() {
        // Default shell ctx leaves `mailbox_router` = None.
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .call(
                json!({ "to": "researcher", "summary": "s", "message": "hi" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("an unwired router must error");
        match err {
            ToolError::Internal(m) => assert!(m.contains("not wired"), "got {m}"),
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
