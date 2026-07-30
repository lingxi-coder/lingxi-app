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
//! Delivery routes through the injected [`traits::mailbox::MailboxRouterHandle`]
//! seam. The desktop host wires the same name-aware router used by coordinator
//! teammates and background agents; its mailbox pump wakes a parked persistent
//! agent on delivery. Cross-session `uds:`/`bridge:` transports remain outside
//! this local tool (the richer coordinator tool rejects them explicitly).
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

/// Preview width for the `routing.content` field (claude-code `Us(t, 50)`).
///
/// 2.1.212 stopped duplicating the full message body in the sender's transcript:
/// the body rides the mailbox delivery, and the tool result only carries a
/// 50-char preview. See [`truncate_preview`].
const ROUTING_CONTENT_PREVIEW_CHARS: usize = 50;

/// Truncate `s` to `max_width` columns, appending `…` if it was longer.
///
/// Port of claude-code `utils/truncate.ts` `truncate(str, maxWidth)` (the
/// two-arg / non-single-line form, `Us(t, 50)`): return the string unchanged
/// when it fits, otherwise take complete grapheme clusters through
/// `max_width - 1` terminal cells and append the ellipsis. Reuse the TUI's
/// shared width primitive so CJK and multi-codepoint emoji follow the same
/// display-width contract everywhere.
fn truncate_preview(s: &str, max_width: usize) -> String {
    tui_core::render::truncate_to_width_ellipsis(s, max_width)
}

/// `isAgentSwarmsEnabled()` gate: Anthropic-internal runs are on by default,
/// while external runs require the experimental env opt-in. `SendMessage`
/// mirrors that runtime gate and additionally requires a live mailbox router.
fn agent_swarms_enabled() -> bool {
    std::env::var("USER_TYPE").is_ok_and(|v| v == "ant")
        || traits::env::is_env_truthy(
            std::env::var("LINGXI_EXPERIMENTAL_AGENT_TEAMS")
                .ok()
                .as_deref(),
        )
}

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
                "description": "Recipient: teammate name"
            },
            "summary": {
                "type": "string",
                "maxLength": 200,
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
                "to must be a bare teammate name — there is only one team per session".into(),
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
    /// Uses the human teammate name threaded on [`ToolUseContext`], falling to
    /// the same `"teammate"` / `"team-lead"` labels as Claude when no display
    /// name is available.
    fn sender_name(ctx: &ToolUseContext) -> String {
        ctx.agent_name.clone().unwrap_or_else(|| {
            if ctx.agent_id.is_some() {
                "teammate".to_string()
            } else {
                TEAM_LEAD_NAME.to_string()
            }
        })
    }

    /// `from` string handed to the mailbox seam.
    ///
    /// Prefer the display name because the mailbox router and Claude's mailbox
    /// both key teammate attribution on it. An unnamed agent falls back to its
    /// id so the concrete router can still retain teammate attribution.
    fn route_from(ctx: &ToolUseContext) -> String {
        ctx.agent_name.clone().unwrap_or_else(|| {
            ctx.agent_id
                .map_or_else(|| TEAM_LEAD_NAME.to_string(), |a| a.to_string())
        })
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

    fn protocol_timestamp() -> String {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let seconds = duration.as_secs();
        let millis = duration.subsec_millis();
        let days = (seconds / 86_400) as i64;
        let seconds_of_day = seconds % 86_400;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let day_of_era = z - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let mut year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_prime = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
        let month = month_prime + if month_prime < 10 { 3 } else { -9 };
        year += i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
            seconds_of_day / 3_600,
            (seconds_of_day % 3_600) / 60,
            seconds_of_day % 60
        )
    }

    fn require_team_lead(ctx: &ToolUseContext, action: &str) -> Result<(), ToolError> {
        if ctx.agent_id.is_none() && ctx.agent_name.is_none() {
            return Ok(());
        }
        Err(ToolError::InvalidInput(format!(
            "Only the team lead can {action} plans. Teammates cannot {action} their own or other plans."
        )))
    }

    fn approval_permission_mode(&self) -> &'static str {
        let raw = self
            .ctx
            .permission_gate
            .as_ref()
            .and_then(|gate| gate.permission_mode())
            .unwrap_or_else(|| self.ctx.permission_mode.wire_str().to_string());
        let mode = permission::permission_mode_from_cli_string(&raw);
        if mode == permission::PermissionMode::Plan {
            permission::PermissionMode::Default.wire_str()
        } else {
            mode.wire_str()
        }
    }

    /// Deliver through the live mailbox seam. Delivery failures are surfaced;
    /// returning a success result after `NotFound`/`Full` would falsely tell the
    /// model that a background agent received a course correction.
    async fn deliver(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        target: &str,
        content: String,
    ) -> Result<(), ToolError> {
        let msg = MailboxMessage {
            message_id: tool_api::util::ids::ulid_or_uuid(),
            content,
            timestamp: SystemTime::now(),
            // No teammate-color seam threaded into SendMessage; `None` is omitted
            // from the wire form (parity with claude-code `color: undefined`).
            color: None,
        };
        router
            .route(from, target, msg)
            .await
            .map(|_| ())
            .map_err(|e| {
                ToolError::Internal(format!("SendMessage: failed to deliver to '{target}': {e}"))
            })
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
    ///
    /// The full body rides the mailbox delivery ([`Self::deliver`]); the tool
    /// result only carries a 50-char `routing.content` preview
    /// (claude-code 2.1.212 `content: Us(t, 50)`), so the body is not duplicated
    /// in the sender's transcript.
    async fn handle_message(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        content: &str,
        summary: Option<&str>,
        sender: &str,
    ) -> Result<Value, ToolError> {
        Self::deliver(router, from, recipient.route_target(), content.to_string()).await?;
        let preview = truncate_preview(content, ROUTING_CONTENT_PREVIEW_CHARS);
        Ok(json!({
            "success": true,
            "message": format!("Message sent to {to_display}'s inbox"),
            "routing": Self::routing(
                sender,
                &format!("@{to_display}"),
                summary,
                Some(&preview),
            ),
        }))
    }

    /// `handleBroadcast` — fan-out to every live teammate except the sender.
    async fn handle_broadcast(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        content: &str,
    ) -> Result<Value, ToolError> {
        let recipients = router
            .broadcast(
                from,
                MailboxMessage {
                    message_id: tool_api::util::ids::ulid_or_uuid(),
                    content: content.to_string(),
                    timestamp: SystemTime::now(),
                    color: None,
                },
            )
            .await
            .map_err(|e| ToolError::Internal(format!("SendMessage: failed to broadcast: {e}")))?;
        let message = if recipients.is_empty() {
            "No teammates to broadcast to (you are the only team member)".to_string()
        } else {
            format!(
                "Message broadcast to {} teammate(s): {}",
                recipients.len(),
                recipients.join(", ")
            )
        };
        Ok(json!({
            "success": true,
            "message": message,
            "recipients": recipients,
        }))
    }

    /// `handleShutdownRequest` — ask a teammate to shut down.
    async fn handle_shutdown_request(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        reason: Option<&str>,
        sender: &str,
    ) -> Result<Value, ToolError> {
        let request_id = Self::generate_request_id("shutdown", to_display);
        let payload = json!({
            "type": "shutdown_request",
            "requestId": request_id,
            "from": sender,
            "reason": reason,
            "timestamp": Self::protocol_timestamp(),
        });
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await?;
        Ok(json!({
            "success": true,
            "message": format!("Shutdown request sent to {to_display}. Request ID: {request_id}"),
            "request_id": request_id,
            "target": to_display,
        }))
    }

    /// `handleShutdownApproval` — a teammate approves its own shutdown.
    ///
    async fn handle_shutdown_approval(
        &self,
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        request_id: &str,
        agent_name: &str,
        ctx: &ToolUseContext,
    ) -> Result<Value, ToolError> {
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
        .await?;
        let task_address = ctx
            .agent_id
            .map(|id| id.to_string())
            .or_else(|| ctx.agent_name.clone())
            .ok_or_else(|| {
                ToolError::Internal(
                    "SendMessage: shutdown approval requires teammate context".into(),
                )
            })?;
        let registry = self.ctx.task_registry.as_ref().ok_or_else(|| {
            ToolError::Internal("SendMessage: task registry is not configured".into())
        })?;
        // The team LEAD (an agent) approved this shutdown, so the stop is
        // attributed to Claude, not the user (claude `killedBy:"parent"`).
        registry
            .kill_with_reason(&task_address, "parent")
            .await
            .map_err(|e| ToolError::Internal(format!("SendMessage: shutdown failed: {e}")))?;
        Ok(json!({
            "success": true,
            "message": format!(
                "Shutdown approved. Sent confirmation to team-lead. Agent {agent_name} is now exiting."
            ),
            "request_id": request_id,
        }))
    }

    /// `handleShutdownRejection` — a teammate declines a shutdown request.
    async fn handle_shutdown_rejection(
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        request_id: &str,
        reason: &str,
        agent_name: &str,
    ) -> Result<Value, ToolError> {
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
        .await?;
        Ok(json!({
            "success": true,
            "message": format!("Shutdown rejected. Reason: \"{reason}\". Continuing to work."),
            "request_id": request_id,
        }))
    }

    /// `handlePlanApproval` — the team lead approves a teammate's plan.
    async fn handle_plan_approval(
        &self,
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        request_id: &str,
        feedback: Option<&str>,
        ctx: &ToolUseContext,
    ) -> Result<Value, ToolError> {
        Self::require_team_lead(ctx, "approve")?;
        let mut payload = json!({
            "type": "plan_approval_response",
            "requestId": request_id,
            "approved": true,
            "timestamp": Self::protocol_timestamp(),
            "permissionMode": self.approval_permission_mode(),
        });
        if let Some(feedback) = feedback {
            payload["feedback"] = json!(feedback);
        }
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await?;
        Ok(json!({
            "success": true,
            "message": format!(
                "Plan approved for {to_display}. They will receive the approval and can proceed with implementation."
            ),
            "request_id": request_id,
        }))
    }

    /// `handlePlanRejection` — the team lead rejects a teammate's plan.
    async fn handle_plan_rejection(
        &self,
        router: &Arc<dyn MailboxRouterHandle>,
        from: &str,
        recipient: &Recipient,
        to_display: &str,
        request_id: &str,
        feedback: &str,
        ctx: &ToolUseContext,
    ) -> Result<Value, ToolError> {
        Self::require_team_lead(ctx, "reject")?;
        let payload = json!({
            "type": "plan_approval_response",
            "requestId": request_id,
            "approved": false,
            "feedback": feedback,
            "timestamp": Self::protocol_timestamp(),
        });
        Self::deliver(
            router,
            from,
            recipient.route_target(),
            serde_json::to_string(&payload).unwrap_or_default(),
        )
        .await?;
        let feedback_preview = truncate_preview(feedback, ROUTING_CONTENT_PREVIEW_CHARS);
        Ok(json!({
            "success": true,
            "message": format!(
                "Plan rejected for {to_display} with feedback: \"{feedback_preview}\""
            ),
            "request_id": request_id,
        }))
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
        // TS `isEnabled()` = `isAgentSwarmsEnabled()`, which binds the tool to a
        // live team/teammate context.
        //
        // The port additionally requires a wired `MailboxRouterHandle`:
        // without one, `call()` cannot route at all. Both conditions must hold
        // before the tool is advertised.
        self.ctx.mailbox_router.is_some() && agent_swarms_enabled()
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
        // The 2.1.218 oracle's SendMessage validateInput STILL hard-rejects a
        // broadcast unconditionally (`if(e.to==="*")return{result:!1,message:'…',
        // errorCode:9}`). The internal fan-out plumbing (handle_broadcast /
        // MailboxRouterHandle::broadcast) stays wired but DORMANT — unreachable
        // through this gate — until the oracle re-adds broadcast; the model is
        // not granted a fan-out capability the oracle denies.
        if to == "*" {
            return Err(ValidationError(
                "broadcast (to: \"*\") is no longer supported — send a message per recipient"
                    .into(),
            ));
        }
        if to.trim().is_empty() {
            return Err(ValidationError("to must not be empty".into()));
        }
        if to.contains('@') {
            return Err(ValidationError(
                "to must be a bare teammate name — there is only one team per session".into(),
            ));
        }

        // Plain-string message: guard against protocol frames embedded in text,
        // then require a non-empty `summary`.
        if let Some(Value::String(msg_str)) = input.get("message") {
            // Plaintext protocol-frame guard: a JSON string that parses to an
            // object with a `type` field that looks like a protocol message type
            // must not be sent as a plain-string message.
            if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(msg_str.trim()) {
                let mtype = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(
                    mtype,
                    "shutdown_request" | "shutdown_response" | "plan_approval_response"
                ) {
                    return Err(ValidationError(
                        "message text must not be a teammate protocol frame — use the structured message object form instead".into(),
                    ));
                }
            }
            let summary = input.get("summary").and_then(Value::as_str);
            if summary.is_none_or(|s| s.trim().is_empty()) {
                return Err(ValidationError(
                    "summary is required when message is a string".into(),
                ));
            }
            return Ok(());
        }

        // Structured message from here on. Runtime gating is enforced by
        // `is_enabled()`, so validation only checks the message shape.

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
                // Reason-on-approval guard: `reason` is only delivered on
                // rejections; providing it on an approval is invalid.
                let reason = obj.get("reason").and_then(Value::as_str);
                if approve && reason.is_some() {
                    return Err(ValidationError(
                        "reason is only delivered on rejections (approve: false) — remove it from an approval response".into(),
                    ));
                }
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

        let data_result: Result<Value, ToolError> = match message {
            Value::String(content) => match recipient {
                Recipient::Broadcast => Self::handle_broadcast(&router, &from, content).await,
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
                let feedback = obj.get("feedback").and_then(Value::as_str);
                match mtype {
                    "shutdown_request" => {
                        Self::handle_shutdown_request(
                            &router, &from, &recipient, &to, reason, &sender,
                        )
                        .await
                    }
                    "shutdown_response" => {
                        if approve {
                            self.handle_shutdown_approval(
                                &router,
                                &from,
                                &request_id_in,
                                &sender,
                                &ctx,
                            )
                            .await
                        } else {
                            Self::handle_shutdown_rejection(
                                &router,
                                &from,
                                &request_id_in,
                                reason.unwrap_or(""),
                                &sender,
                            )
                            .await
                        }
                    }
                    "plan_approval_response" => {
                        if approve {
                            self.handle_plan_approval(
                                &router,
                                &from,
                                &recipient,
                                &to,
                                &request_id_in,
                                feedback,
                                &ctx,
                            )
                            .await
                        } else {
                            self.handle_plan_rejection(
                                &router,
                                &from,
                                &recipient,
                                &to,
                                &request_id_in,
                                feedback.unwrap_or("Plan needs revision"),
                                &ctx,
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

        let data = match data_result {
            Ok(data) => data,
            Err(error) => {
                Self::emit_failed(
                    &bus,
                    &invocation_id,
                    "delivery_failed",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(error);
            }
        };

        Self::emit_completed(&bus, &invocation_id, started.elapsed().as_millis() as u64).await;
        // Model-facing content is the brief status string (`data["message"]`),
        // not the full routing/recipients JSON dump. claude-code surfaces the
        // `Message sent…`/shutdown/plan status line to the model; the routing
        // metadata stays in `data` for the UI only. Without this the dispatch
        // falls back to serialising `data` (the JSON dump) for the model.
        let model_content = data
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(ToolCallResult {
            data,
            model_content,
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
    use std::sync::{Mutex, OnceLock};
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::mailbox::{MailboxError, RouteAck};
    use traits::process::ProcessOutput;
    use traits::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
        TaskRegistryHandle, TaskUpdatePatch,
    };

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Records every `(from, to, content)` it is asked to route and always acks
    /// (so name-keyed routes — which the production seam can't resolve — are
    /// observable in tests).
    struct RecordingRouter {
        routed: Mutex<Vec<(String, String, String)>>,
        broadcasted: Mutex<Vec<(String, String)>>,
    }

    struct RejectingRouter;

    #[derive(Default)]
    struct RecordingTaskRegistry {
        killed: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TaskRegistryHandle for RecordingTaskRegistry {
        async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by SendMessage")
        }

        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }

        async fn list(
            &self,
            _filter: TaskListFilter,
        ) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(Vec::new())
        }

        async fn update(
            &self,
            _id: &str,
            _patch: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by SendMessage")
        }

        async fn set_status(
            &self,
            _id: &str,
            _status: &str,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by SendMessage")
        }

        async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
            self.killed.lock().unwrap().push(id.to_string());
            Ok(TaskRecord {
                task_id: "t12345678".into(),
                status: "killed".into(),
                ..Default::default()
            })
        }

        async fn output(
            &self,
            _id: &str,
            _offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unreachable!("not used by SendMessage")
        }
    }

    #[async_trait]
    impl MailboxRouterHandle for RejectingRouter {
        async fn route(
            &self,
            _from_agent: &str,
            to_agent: &str,
            _message: MailboxMessage,
        ) -> Result<RouteAck, MailboxError> {
            Err(MailboxError::NotFound(to_agent.to_string()))
        }
    }

    impl RecordingRouter {
        fn new() -> Self {
            Self {
                routed: Mutex::new(Vec::new()),
                broadcasted: Mutex::new(Vec::new()),
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

        async fn broadcast(
            &self,
            from_agent: &str,
            message: MailboxMessage,
        ) -> Result<Vec<String>, MailboxError> {
            self.broadcasted
                .lock()
                .unwrap()
                .push((from_agent.to_string(), message.content));
            Ok(vec!["alpha".to_string(), "beta".to_string()])
        }
    }

    fn ctx_with(router: Arc<RecordingRouter>) -> BuiltinToolContext {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.mailbox_router = Some(router as Arc<dyn MailboxRouterHandle>);
        ctx
    }

    fn ctx_with_shutdown(
        router: Arc<RecordingRouter>,
        registry: Arc<RecordingTaskRegistry>,
    ) -> BuiltinToolContext {
        let mut ctx = ctx_with(router);
        ctx.task_registry = Some(registry);
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
        let variants = schema["properties"]["message"]["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["type"], "string");
        assert_eq!(
            variants[1]["properties"]["type"]["enum"],
            json!([
                "shutdown_request",
                "shutdown_response",
                "plan_approval_response"
            ])
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
            "to must be a bare teammate name — there is only one team per session"
        );
    }

    #[tokio::test]
    async fn validate_string_message_requires_summary() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        let err = tool
            .validate_input(
                &json!({ "to": "researcher", "message": "hi" }),
                &fresh_ctx(),
            )
            .await
            .expect_err("string message without summary must reject");
        assert_eq!(err.0, "summary is required when message is a string");
    }

    #[tokio::test]
    async fn validate_broadcast_rejected_unconditionally() {
        let tool = SendMessageTool::new(shell_test_ctx(dummy_out()));
        // The oracle's validateInput hard-rejects `to: "*"` with a byte-exact
        // message; broadcast is no longer a supported model-facing capability.
        let err = tool
            .validate_input(
                &json!({ "to": "*", "summary": "all hands", "message": "standup in 5" }),
                &fresh_ctx(),
            )
            .await
            .expect_err("broadcast (to: \"*\") must be rejected");
        assert_eq!(
            err.0,
            "broadcast (to: \"*\") is no longer supported — send a message per recipient"
        );
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
        assert_eq!(
            err.0,
            "reason is required when rejecting a shutdown request"
        );
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
        // The model sees only the brief status string, never the routing JSON dump.
        assert_eq!(
            res.model_content.as_deref(),
            Some("Message sent to researcher's inbox")
        );

        let routed = router.routed.lock().unwrap();
        assert_eq!(routed.len(), 1);
        // Resolved by NAME — the recipient string is the bare teammate name.
        assert_eq!(routed[0].1, "researcher");
        assert_eq!(routed[0].2, "start on task #1");
    }

    #[tokio::test]
    async fn uses_spawned_agents_display_name_for_sender_identity() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let mut call_ctx = fresh_ctx();
        call_ctx.agent_id = Some(protocol::AgentId::new());
        call_ctx.agent_name = Some("researcher".to_string());

        let res = tool
            .call(
                json!({ "to": "reviewer", "summary": "share findings", "message": "done" }),
                call_ctx,
                fresh_tx(),
            )
            .await
            .expect("send must succeed");

        assert_eq!(res.data["routing"]["sender"], "researcher");
        assert_eq!(router.routed.lock().unwrap()[0].0, "researcher");
    }

    #[tokio::test]
    async fn delivery_failure_is_not_reported_as_success() {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.mailbox_router = Some(Arc::new(RejectingRouter));
        let tool = SendMessageTool::new(ctx);

        let err = tool
            .call(
                json!({ "to": "missing", "summary": "send update", "message": "hello" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("unknown recipient must fail");

        assert!(format!("{err}").contains("failed to deliver to 'missing'"));
    }

    #[tokio::test]
    async fn routing_content_is_truncated_to_50_char_preview() {
        // 2.1.212: the sender's tool result carries only a 50-char preview of
        // the body (`Us(t, 50)`); the full body rides the mailbox delivery and
        // is not duplicated in the transcript.
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let body = "a".repeat(100);
        let res = tool
            .call(
                json!({ "to": "researcher", "summary": "long note", "message": body.clone() }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("send must succeed");

        // routing.content = first 49 chars + ellipsis (width-50 truncation).
        let expected_preview = format!("{}\u{2026}", "a".repeat(49));
        assert_eq!(res.data["routing"]["content"], expected_preview);
        assert_eq!(
            res.data["routing"]["content"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            50
        );

        // The mailbox still receives the FULL body — nothing is truncated on the
        // delivery path.
        let routed = router.routed.lock().unwrap();
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].2, body);
    }

    #[test]
    fn truncate_preview_leaves_short_bodies_intact() {
        // At or under the width the body is returned verbatim (no ellipsis).
        assert_eq!(truncate_preview("start on task #1", 50), "start on task #1");
        assert_eq!(truncate_preview(&"a".repeat(50), 50), "a".repeat(50));
        assert_eq!(
            truncate_preview(&"a".repeat(51), 50),
            format!("{}\u{2026}", "a".repeat(49))
        );
    }

    #[test]
    fn truncate_preview_counts_terminal_cells_and_keeps_graphemes_intact() {
        assert_eq!(truncate_preview("你好世界", 7), "你好世…");
        assert_eq!(truncate_preview("👨‍👩‍👧‍👦abc", 4), "👨‍👩‍👧‍👦a…");
    }

    #[tokio::test]
    async fn broadcast_returns_broadcast_shape() {
        // DORMANT PLUMBING: the model can never reach this — validateInput now
        // hard-rejects `to: "*"` (see validate_broadcast_rejected_unconditionally).
        // This exercises the internal fan-out mechanism directly (bypassing the
        // gate) so it stays functional for the day the oracle re-adds broadcast.
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let res = tool
            .call(
                json!({ "to": "*", "summary": "all hands", "message": "standup in 5" }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("broadcast must succeed");
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["recipients"], json!(["alpha", "beta"]));
        // Model-facing text is the brief broadcast status string, not the data dump.
        assert_eq!(
            res.model_content.as_deref(),
            Some("Message broadcast to 2 teammate(s): alpha, beta")
        );
        assert_eq!(
            router.broadcasted.lock().unwrap().as_slice(),
            &[("team-lead".to_string(), "standup in 5".to_string())]
        );
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
        assert!(res.data["request_id"]
            .as_str()
            .unwrap()
            .starts_with("shutdown-"));
        let msg = res.data["message"].as_str().unwrap();
        assert!(msg.starts_with("Shutdown request sent to researcher. Request ID: shutdown-"));
        // Model-facing text mirrors the brief status string, not the data dump.
        assert_eq!(res.model_content.as_deref(), Some(msg));
        // The structured payload was delivered to the named recipient.
        let routed = router.routed.lock().unwrap();
        assert_eq!(routed[0].1, "researcher");
        let frame: Value = serde_json::from_str(&routed[0].2).unwrap();
        assert_eq!(frame["type"], "shutdown_request");
        assert_eq!(frame["from"], "team-lead");
        assert_eq!(frame["reason"], "done for the day");
        assert!(frame["timestamp"].as_str().is_some_and(|timestamp| {
            timestamp.len() == 24 && timestamp.as_bytes()[19] == b'.' && timestamp.ends_with('Z')
        }));
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
    async fn plan_approval_is_lead_only_and_inherits_non_plan_mode() {
        let router = Arc::new(RecordingRouter::new());
        let mut tool_ctx = ctx_with(router.clone());
        tool_ctx.permission_mode = permission::PermissionMode::AcceptEdits;
        let tool = SendMessageTool::new(tool_ctx);

        tool.call(
            json!({
                "to": "researcher",
                "message": {
                    "type": "plan_approval_response",
                    "request_id": "plan-1@researcher",
                    "approve": true
                }
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("team lead may approve");

        let routed = router.routed.lock().unwrap();
        let frame: Value = serde_json::from_str(&routed[0].2).unwrap();
        assert_eq!(frame["type"], "plan_approval_response");
        assert_eq!(frame["permissionMode"], "acceptEdits");
        assert!(frame.get("from").is_none());
        assert!(frame["timestamp"].as_str().is_some_and(|timestamp| {
            timestamp.len() == 24 && timestamp.as_bytes()[19] == b'.' && timestamp.ends_with('Z')
        }));
    }

    #[tokio::test]
    async fn plan_approval_downgrades_leader_plan_mode_to_default() {
        let router = Arc::new(RecordingRouter::new());
        let mut tool_ctx = ctx_with(router.clone());
        tool_ctx.permission_mode = permission::PermissionMode::Plan;
        let tool = SendMessageTool::new(tool_ctx);

        tool.call(
            json!({
                "to": "researcher",
                "message": {
                    "type": "plan_approval_response",
                    "request_id": "plan-2@researcher",
                    "approve": true
                }
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("team lead may approve");

        let frame: Value = serde_json::from_str(&router.routed.lock().unwrap()[0].2).unwrap();
        assert_eq!(frame["permissionMode"], "default");
    }

    #[tokio::test]
    async fn plan_approval_preserves_optional_feedback_without_sender_field() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));

        tool.call(
            json!({
                "to": "researcher",
                "message": {
                    "type": "plan_approval_response",
                    "request_id": "plan-feedback@researcher",
                    "approve": true,
                    "feedback": "Proceed with the smaller diff"
                }
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("team lead may approve with feedback");

        let frame: Value = serde_json::from_str(&router.routed.lock().unwrap()[0].2).unwrap();
        assert_eq!(frame["feedback"], "Proceed with the smaller diff");
        assert!(frame.get("from").is_none());
    }

    #[tokio::test]
    async fn plan_rejection_truncates_only_the_model_facing_feedback_preview() {
        let router = Arc::new(RecordingRouter::new());
        let tool = SendMessageTool::new(ctx_with(router.clone()));
        let feedback = "x".repeat(80);

        let result = tool
            .call(
                json!({
                    "to": "researcher",
                    "message": {
                        "type": "plan_approval_response",
                        "request_id": "plan-reject@researcher",
                        "approve": false,
                        "feedback": feedback
                    }
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("team lead may reject");

        let frame: Value = serde_json::from_str(&router.routed.lock().unwrap()[0].2).unwrap();
        assert_eq!(frame["feedback"], "x".repeat(80));
        assert!(frame.get("from").is_none());
        assert_eq!(
            result.data["message"],
            format!(
                "Plan rejected for researcher with feedback: \"{}\"",
                truncate_preview(&"x".repeat(80), ROUTING_CONTENT_PREVIEW_CHARS)
            )
        );
    }

    #[tokio::test]
    async fn teammate_cannot_approve_or_reject_plans() {
        for (approve, action) in [(true, "approve"), (false, "reject")] {
            let router = Arc::new(RecordingRouter::new());
            let tool = SendMessageTool::new(ctx_with(router.clone()));
            let mut call_ctx = fresh_ctx();
            call_ctx.agent_id = Some(protocol::AgentId::new());
            call_ctx.agent_name = Some("reviewer".to_string());
            let error = tool
                .call(
                    json!({
                        "to": "researcher",
                        "message": {
                            "type": "plan_approval_response",
                            "request_id": "plan-3@researcher",
                            "approve": approve
                        }
                    }),
                    call_ctx,
                    fresh_tx(),
                )
                .await
                .expect_err("teammate plan decisions must fail closed");
            assert!(error
                .model_facing_message()
                .contains(&format!("Only the team lead can {action} plans")));
            assert!(router.routed.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn shutdown_approval_delivers_then_kills_the_calling_worker() {
        let router = Arc::new(RecordingRouter::new());
        let registry = Arc::new(RecordingTaskRegistry::default());
        let tool = SendMessageTool::new(ctx_with_shutdown(router.clone(), registry.clone()));
        let mut call_ctx = fresh_ctx();
        let agent_id = protocol::AgentId::new();
        call_ctx.agent_id = Some(agent_id);
        call_ctx.agent_name = Some("researcher".into());

        let result = tool
            .call(
                json!({
                    "to": "team-lead",
                    "message": {
                        "type": "shutdown_response",
                        "request_id": "shutdown-1@researcher",
                        "approve": true
                    }
                }),
                call_ctx,
                fresh_tx(),
            )
            .await
            .expect("approval is delivered and worker is stopped");

        assert_eq!(result.data["success"], true);
        assert_eq!(router.routed.lock().unwrap()[0].1, "team-lead");
        assert_eq!(
            registry.killed.lock().unwrap().as_slice(),
            &[agent_id.to_string()]
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

    /// M-02: a session with NO mailbox router must not advertise `SendMessage`.
    /// Without a router every call returns `router_not_wired`, so offering it
    /// costs a tool slot, prompt budget and one wasted call to find that out.
    #[tokio::test]
    async fn send_message_is_disabled_without_a_mailbox_router() {
        let ctx = shell_test_ctx(dummy_out());
        assert!(
            ctx.mailbox_router.is_none(),
            "fixture precondition: no router wired"
        );
        let tool = SendMessageTool::new(ctx);
        assert!(!tool.is_enabled(&ToolStaticContext::default()));
    }

    #[test]
    fn send_message_is_disabled_when_agent_teams_gate_is_off() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("USER_TYPE");
        std::env::remove_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS");

        let tool = SendMessageTool::new(ctx_with(Arc::new(RecordingRouter::new())));
        assert!(
            !tool.is_enabled(&ToolStaticContext::default()),
            "external runs without the experimental gate must not advertise SendMessage"
        );
    }

    #[test]
    fn send_message_is_enabled_when_agent_teams_gate_is_on_and_router_wired() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("USER_TYPE");
        std::env::set_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS", "1");

        let tool = SendMessageTool::new(ctx_with(Arc::new(RecordingRouter::new())));
        assert!(tool.is_enabled(&ToolStaticContext::default()));

        std::env::remove_var("LINGXI_EXPERIMENTAL_AGENT_TEAMS");
    }
}
