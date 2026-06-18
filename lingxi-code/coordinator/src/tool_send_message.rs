//! `SendMessageTool` — the coordinator-only `SendMessage` tool.
//!
//! 1:1 Rust port of the lingxi/claude-code `SendMessageTool`
//! (`src/tools/SendMessageTool/SendMessageTool.ts`, name constant
//! `SEND_MESSAGE_TOOL_NAME = "SendMessage"`).
//!
//! ## Addressing (`parse_address`, port of `utils/peerAddress.ts` + the
//! name/agent-id resolution in `SendMessageTool.ts:800-913`)
//!
//! The TS `to` field is a teammate **name**, `"*"` for broadcast, or a
//! `uds:` / `bridge:` URI. On the lingxi coordinator a worker is keyed by its
//! [`AgentId`]; this tool therefore resolves `to` into one of five forms:
//!
//! - [`Address::Broadcast`] (`"*"`): fan out to every other teammate
//!   (`handleBroadcast`, `SendMessageTool.ts:191-266`).
//! - [`Address::Name`]: a bare teammate name, resolved to an [`AgentId`] via
//!   [`TeamRegistry::find_by_name`].
//! - [`Address::AgentId`]: a raw UUID / `agent:<uuid>` display form (the
//!   coordinator's `<task-id>` surface — kept so the existing
//!   continue-a-worker-by-id flow still works).
//! - [`Address::Uds`] / [`Address::Bridge`]: the cross-session schemes. The
//!   entire bridge/UDS subsystem is unported on this build, so these return a
//!   clear "not supported on this build" error (DEFERRED).
//!
//! ## Deferred (noted, not attempted)
//!
//! - The pending-message queue + stopped-agent auto-resume
//!   (`SendMessageTool.ts:802-874`) — needs the live teammate task loop.
//! - The `uds:` / `bridge:` transports + the bridge `ask` permission
//!   escalation (`checkPermissions`, `SendMessageTool.ts:585-602`) — the bridge
//!   subsystem is unported.
//!
//! Telemetry is intentionally omitted: coordinator-side tools hold an
//! `Arc<TeamRegistry>` directly rather than a `BuiltinToolContext`, so there is
//! no `bus` to emit on (per the implementation brief, telemetry is optional for
//! these tools).

use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use protocol::AgentId;
use serde_json::{json, Value};
use uuid::Uuid;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use traits::team_spawn::TeamSpawnSeam;

use crate::mailbox::{MailboxError, MessageSender, TeammateMessage};
use crate::team_registry::TeamRegistry;

/// Canonical tool name — must match `SEND_MESSAGE_TOOL_NAME` in
/// `src/tools/SendMessageTool/constants.ts` exactly.
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// Short human-readable description — matches `DESCRIPTION` in
/// `src/tools/SendMessageTool/prompt.ts`.
const DESCRIPTION: &str = "Send a message to another agent";

/// The team lead's name (TS `TEAM_LEAD_NAME = "team-lead"`,
/// `utils/swarm/constants.ts:1`). `shutdown_response` must be addressed to it.
const TEAM_LEAD_NAME: &str = "team-lead";

/// `maxResultSizeChars` in the TS tool is `100_000`.
const MAX_RESULT_SIZE_CHARS: usize = 100_000;

/// Lazily-built input schema cache.
///
/// `std::sync::OnceLock` is used instead of `once_cell::sync::Lazy` so the
/// file stays self-contained: the coordinator crate does not directly depend
/// on `once_cell`.
static INPUT_SCHEMA: OnceLock<Value> = OnceLock::new();

/// A parsed `to` recipient address.
///
/// Replaces the former `parse_recipient` (UUID-only) with the full TS
/// addressing surface. See the module docs for the mapping onto the lingxi
/// coordinator's [`AgentId`]-keyed workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// `"*"` — broadcast to every other teammate.
    Broadcast,
    /// A bare teammate name (resolved via [`TeamRegistry::find_by_name`]).
    Name(String),
    /// A raw agent id (UUID or `agent:<uuid>`).
    AgentId(AgentId),
    /// `uds:<socket-path>` (or a legacy bare `/`-prefixed path) — cross-session,
    /// unported.
    Uds(String),
    /// `bridge:<session-id>` — Remote Control peer, unported.
    Bridge(String),
}

/// A parsed structured message — the TS `StructuredMessage` discriminated union
/// (`SendMessageTool.ts:46-65`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum StructuredMessage {
    /// `shutdown_request { reason? }`.
    ShutdownRequest { reason: Option<String> },
    /// `shutdown_response { request_id, approve, reason? }`.
    ShutdownResponse {
        request_id: String,
        approve: bool,
        reason: Option<String>,
    },
    /// `plan_approval_response { request_id, approve, feedback? }`.
    PlanApprovalResponse {
        request_id: String,
        approve: bool,
        feedback: Option<String>,
    },
}

/// Parse the `to` field into an [`Address`].
///
/// Mirrors `parseAddress` (`utils/peerAddress.ts`) for the scheme detection,
/// then layers the coordinator's name-vs-agent-id resolution on the `other`
/// scheme. Pure (no registry access) — name resolution happens in `call`.
#[must_use]
pub fn parse_address(to: &str) -> Address {
    let trimmed = to.trim();
    if trimmed == "*" {
        return Address::Broadcast;
    }
    if let Some(rest) = trimmed.strip_prefix("uds:") {
        return Address::Uds(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("bridge:") {
        return Address::Bridge(rest.to_string());
    }
    // Legacy: bare socket paths (`/...`) route through the UDS branch
    // (peerAddress.ts:19).
    if trimmed.starts_with('/') {
        return Address::Uds(trimmed.to_string());
    }
    // `other` scheme: a raw agent id (UUID / `agent:<uuid>`) or a teammate name.
    let candidate = trimmed.strip_prefix("agent:").unwrap_or(trimmed);
    if let Ok(uuid) = Uuid::parse_str(candidate) {
        return Address::AgentId(AgentId::from_uuid(uuid));
    }
    Address::Name(trimmed.to_string())
}

/// Coerce a JSON value to `bool` the way TS `semanticBoolean()`
/// (`utils/semanticBoolean.ts`) does: a real bool, or the exact strings
/// `"true"`/`"false"`. Anything else is `None` (a validation error).
fn semantic_boolean(v: Option<&Value>) -> Option<bool> {
    match v {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) if s == "true" => Some(true),
        Some(Value::String(s)) if s == "false" => Some(false),
        _ => None,
    }
}

/// Build the JSON Schema for `SendMessage` inputs.
fn build_input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["to", "message"],
        "properties": {
            "to": {
                "type": "string",
                "description": "Recipient: teammate name, \"*\" for broadcast, \"uds:<socket-path>\" local peer, \"bridge:<session-id>\" Remote Control peer"
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

/// Coordinator-only `SendMessage` tool.
///
/// Holds an `Arc<TeamRegistry>` so it can resolve the recipient worker's
/// [`AgentId`] and route through the registry's `mailbox_router`, plus an
/// optional [`TeamSpawnSeam`] so an approved in-process shutdown can signal the
/// responding worker's backing task to cancel (TS `handleShutdownApproval`'s
/// `abortController.abort()`, `SendMessageTool.ts:348-366`).
pub struct SendMessageTool {
    team: Arc<TeamRegistry>,
    spawn_seam: Option<Arc<dyn TeamSpawnSeam>>,
}

impl SendMessageTool {
    /// Construct a new tool wired to the shared team registry (no spawn seam, so
    /// an approved shutdown routes its response but does not signal cancellation
    /// of the backing task — used by tests that don't exercise that path).
    #[must_use]
    pub fn new(team: Arc<TeamRegistry>) -> Self {
        Self {
            team,
            spawn_seam: None,
        }
    }

    /// Attach the [`TeamSpawnSeam`] so an approved `shutdown_response` cancels
    /// the responding worker's backing task (`TeamSpawnSeam::kill`).
    #[must_use]
    pub fn with_spawn_seam(mut self, spawn_seam: Arc<dyn TeamSpawnSeam>) -> Self {
        self.spawn_seam = Some(spawn_seam);
        self
    }

    /// Determine the sender attribution for a routed message.
    fn sender_from(ctx: &ToolUseContext) -> MessageSender {
        ctx.agent_id
            .map_or(MessageSender::Coordinator, MessageSender::Teammate)
    }

    /// Resolve a non-broadcast [`Address`] to a concrete recipient [`AgentId`].
    ///
    /// `Name` → registry lookup; `AgentId` → as-is. `Uds`/`Bridge`/`Broadcast`
    /// are not single-recipient and must be handled by the caller before this.
    async fn resolve_recipient(&self, addr: &Address) -> Result<AgentId, ToolError> {
        match addr {
            Address::AgentId(id) => Ok(*id),
            Address::Name(name) => self
                .team
                .find_by_name(name)
                .await
                .map(|w| w.agent_id)
                .ok_or_else(|| {
                    ToolError::InvalidInput(format!("SendMessage: no such teammate: {name}"))
                }),
            Address::Broadcast | Address::Uds(_) | Address::Bridge(_) => Err(ToolError::Internal(
                "SendMessage: resolve_recipient called on a non-single recipient".into(),
            )),
        }
    }

    /// Route a single [`TeammateMessage`] to `to`, mapping the mailbox error
    /// space onto [`ToolError`].
    async fn route(&self, to: &AgentId, msg: TeammateMessage) -> Result<(), ToolError> {
        self.team
            .mailbox_router
            .route(to, msg)
            .await
            .map_err(|e| match e {
                MailboxError::NotFound(id) => {
                    ToolError::InvalidInput(format!("SendMessage: no such worker: {}", id.as_uuid()))
                }
                other => ToolError::Internal(format!("SendMessage: {other}")),
            })
    }

    /// `handleBroadcast` (`SendMessageTool.ts:191-266`): enumerate the team and
    /// route to every member except the sender (case-insensitive self-skip).
    async fn handle_broadcast(
        &self,
        content: String,
        ctx: &ToolUseContext,
    ) -> Result<ToolCallResult, ToolError> {
        let sender_id = ctx.agent_id;
        let workers = self.team.list().await;

        let recipients: Vec<crate::team_registry::WorkerAgent> = workers
            .into_iter()
            // Skip self: the calling teammate must not receive its own broadcast.
            .filter(|w| Some(w.agent_id) != sender_id)
            .collect();

        if recipients.is_empty() {
            return Ok(Self::ok(json!({
                "success": true,
                "message": "No teammates to broadcast to (you are the only team member)",
                "recipients": Vec::<String>::new(),
            })));
        }

        let from = Self::sender_from(ctx);
        let mut delivered: Vec<String> = Vec::with_capacity(recipients.len());
        for w in &recipients {
            let msg = TeammateMessage {
                from: from.clone(),
                content: content.clone(),
                message_id: tool_api::util::ids::ulid_or_uuid(),
                timestamp: SystemTime::now(),
                request_id: None,
            };
            // Best-effort fan-out: a single unreachable mailbox doesn't abort the
            // broadcast (mirrors the TS loop, which writes each inbox in turn).
            if self.route(&w.agent_id, msg).await.is_ok() {
                delivered.push(w.name.clone());
            }
        }

        Ok(Self::ok(json!({
            "success": true,
            "message": format!(
                "Message broadcast to {} teammate(s): {}",
                delivered.len(),
                delivered.join(", ")
            ),
            "recipients": delivered,
        })))
    }

    /// `handleShutdownRequest` (`SendMessageTool.ts:268-303`): mint a request id,
    /// deliver the request to the target, return the id.
    async fn handle_shutdown_request(
        &self,
        addr: &Address,
        to_label: &str,
        reason: Option<String>,
        ctx: &ToolUseContext,
    ) -> Result<ToolCallResult, ToolError> {
        let to_id = self.resolve_recipient(addr).await?;
        // generateRequestId('shutdown', target) (utils/agentId.ts:62-68).
        let request_id = generate_request_id("shutdown", to_label);

        let content = serde_json::to_string(&json!({
            "type": "shutdown_request",
            "requestId": request_id,
            "reason": reason,
        }))
        .map_err(|e| ToolError::Internal(format!("SendMessage: {e}")))?;

        let msg = TeammateMessage {
            from: Self::sender_from(ctx),
            content,
            message_id: tool_api::util::ids::ulid_or_uuid(),
            timestamp: SystemTime::now(),
            request_id: Some(request_id.clone()),
        };
        self.route(&to_id, msg).await?;

        Ok(Self::ok(json!({
            "success": true,
            "message": format!("Shutdown request sent to {to_label}. Request ID: {request_id}"),
            "request_id": request_id,
            "target": to_label,
        })))
    }

    /// `handleShutdownApproval` / `handleShutdownRejection`
    /// (`SendMessageTool.ts:305-432`): route the response to the team lead
    /// (the coordinator), and on approval signal the responding in-process
    /// worker's backing task to cancel.
    async fn handle_shutdown_response(
        &self,
        request_id: String,
        approve: bool,
        reason: Option<String>,
        ctx: &ToolUseContext,
    ) -> Result<ToolCallResult, ToolError> {
        let leader = self.team.coordinator_id;
        let payload = if approve {
            json!({ "type": "shutdown_approved", "requestId": request_id })
        } else {
            json!({
                "type": "shutdown_rejected",
                "requestId": request_id,
                "reason": reason,
            })
        };
        let content = serde_json::to_string(&payload)
            .map_err(|e| ToolError::Internal(format!("SendMessage: {e}")))?;

        let msg = TeammateMessage {
            from: Self::sender_from(ctx),
            content,
            message_id: tool_api::util::ids::ulid_or_uuid(),
            timestamp: SystemTime::now(),
            request_id: Some(request_id.clone()),
        };
        self.route(&leader, msg).await?;

        // On approval, signal the responding (calling) worker's backing task to
        // cancel — the in-process analog of `task.abortController.abort()`.
        if approve {
            if let (Some(seam), Some(agent_id)) = (&self.spawn_seam, ctx.agent_id) {
                if let Some(worker) = self.team.list().await.into_iter().find(|w| {
                    w.agent_id == agent_id && !w.task_id.is_empty()
                }) {
                    // Best-effort: a kill failure does not fail the response send.
                    let _ = seam.kill(&worker.task_id).await;
                }
            }
        }

        let message = if approve {
            "Shutdown approved. Sent confirmation to team-lead.".to_string()
        } else {
            format!(
                "Shutdown rejected. Reason: \"{}\". Continuing to work.",
                reason.unwrap_or_default()
            )
        };
        Ok(Self::ok(json!({
            "success": true,
            "message": message,
            "request_id": request_id,
        })))
    }

    /// `handlePlanApproval` / `handlePlanRejection`
    /// (`SendMessageTool.ts:434-518`): route the plan-approval response to the
    /// requesting worker (`to`).
    async fn handle_plan_approval(
        &self,
        addr: &Address,
        to_label: &str,
        request_id: String,
        approve: bool,
        feedback: Option<String>,
        ctx: &ToolUseContext,
    ) -> Result<ToolCallResult, ToolError> {
        let to_id = self.resolve_recipient(addr).await?;
        let payload = if approve {
            json!({
                "type": "plan_approval_response",
                "requestId": request_id,
                "approved": true,
            })
        } else {
            json!({
                "type": "plan_approval_response",
                "requestId": request_id,
                "approved": false,
                "feedback": feedback.clone().unwrap_or_else(|| "Plan needs revision".into()),
            })
        };
        let content = serde_json::to_string(&payload)
            .map_err(|e| ToolError::Internal(format!("SendMessage: {e}")))?;

        let msg = TeammateMessage {
            from: Self::sender_from(ctx),
            content,
            message_id: tool_api::util::ids::ulid_or_uuid(),
            timestamp: SystemTime::now(),
            request_id: Some(request_id.clone()),
        };
        self.route(&to_id, msg).await?;

        let message = if approve {
            format!(
                "Plan approved for {to_label}. They will receive the approval and can proceed with implementation."
            )
        } else {
            format!(
                "Plan rejected for {to_label} with feedback: \"{}\"",
                feedback.unwrap_or_else(|| "Plan needs revision".into())
            )
        };
        Ok(Self::ok(json!({
            "success": true,
            "message": message,
            "request_id": request_id,
        })))
    }

    /// Wrap a JSON `data` blob in a successful [`ToolCallResult`].
    fn ok(data: Value) -> ToolCallResult {
        ToolCallResult {
            data,
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        }
    }
}

/// `generateRequestId(requestType, agentId)` (`utils/agentId.ts:62-68`):
/// `{requestType}-{timestamp}@{agentId}`.
fn generate_request_id(request_type: &str, agent_id: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{request_type}-{timestamp}@{agent_id}")
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        SEND_MESSAGE_TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        INPUT_SCHEMA.get_or_init(build_input_schema)
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // TS `isEnabled()` = `isAgentSwarmsEnabled()`. In coordinator mode the
        // swarm surface is always live (the registry exists), so this is `true`.
        true
    }

    fn max_result_size_chars(&self) -> usize {
        MAX_RESULT_SIZE_CHARS
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        // The registry/mailbox are `RwLock`/`Mutex`-guarded, so concurrent
        // sends are safe.
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

    fn search_hint(&self) -> Option<&str> {
        Some("send messages to agent teammates (swarm protocol)")
    }

    fn should_defer(&self) -> bool {
        // TS `shouldDefer: true`.
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        // The TS tool only escalates to `ask` for the cross-machine `bridge:`
        // scheme, which is unported on the coordinator side (those addresses are
        // rejected up front in `call`). All routable coordinator sends are
        // allowed. (DEFERRED: the bridge `ask` escalation,
        // `SendMessageTool.ts:585-602`.)
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "SendMessage routes to a local worker mailbox".into(),
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
        // Condensed from `src/tools/SendMessageTool/prompt.ts` getPrompt().
        concat!(
            "# SendMessage\n\n",
            "Continue an existing worker by sending a follow-up to its agent id ",
            "or teammate name, or broadcast to all with \"*\".\n\n",
            "```json\n",
            "{\"to\": \"scout\", \"summary\": \"fix npe\", \"message\": \"Fix the null pointer...\"}\n",
            "```\n\n",
            "Your plain text output is NOT visible to other agents — to communicate, ",
            "you MUST call this tool."
        )
        .into()
    }

    // A faithful 1:1 port of the long TS `SendMessageTool.call`
    // (validateInput + the address/structured dispatch). Kept as one method to
    // preserve the line-by-line parity mapping (same allow as `tool_team_create`).
    #[allow(clippy::too_many_lines)]
    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // --- Parse + validate `to` (validateInput, SendMessageTool.ts:604-718) ---
        let to = input
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("SendMessage: missing 'to'".into()))?;

        if to.trim().is_empty() {
            return Err(ToolError::InvalidInput("to must not be empty".into()));
        }
        let addr = parse_address(to);
        // address target must not be empty (uds/bridge with empty target).
        match &addr {
            Address::Uds(t) | Address::Bridge(t) if t.trim().is_empty() => {
                return Err(ToolError::InvalidInput(
                    "address target must not be empty".into(),
                ));
            }
            _ => {}
        }
        // `@` is reserved (one team per session) — bare name / "*" only.
        if to.contains('@') {
            return Err(ToolError::InvalidInput(
                "to must be a bare teammate name or \"*\" — there is only one team per session"
                    .into(),
            ));
        }
        // DEFERRED: the entire bridge/UDS subsystem is unported on this build.
        match &addr {
            Address::Uds(_) => {
                return Err(ToolError::InvalidInput(
                    "SendMessage: 'uds:' addresses are not supported on this build".into(),
                ));
            }
            Address::Bridge(_) => {
                return Err(ToolError::InvalidInput(
                    "SendMessage: 'bridge:' addresses are not supported on this build".into(),
                ));
            }
            _ => {}
        }

        // --- Parse `message`: plain string or structured object -------------
        let message = input
            .get("message")
            .ok_or_else(|| ToolError::InvalidInput("SendMessage: missing 'message'".into()))?;

        // String message: requires a non-empty summary; broadcast or DM.
        if let Value::String(s) = message {
            let summary = input.get("summary").and_then(Value::as_str);
            if summary.is_none_or(|s| s.trim().is_empty()) {
                return Err(ToolError::InvalidInput(
                    "summary is required when message is a string".into(),
                ));
            }
            return match addr {
                Address::Broadcast => self.handle_broadcast(s.clone(), &ctx).await,
                ref a => {
                    let to_id = self.resolve_recipient(a).await?;
                    let msg = TeammateMessage {
                        from: Self::sender_from(&ctx),
                        content: s.clone(),
                        message_id: tool_api::util::ids::ulid_or_uuid(),
                        timestamp: SystemTime::now(),
                        request_id: None,
                    };
                    self.route(&to_id, msg).await?;
                    Ok(Self::ok(json!({
                        "success": true,
                        "message": format!("Message sent to {}'s inbox", display_target(to, a)),
                    })))
                }
            };
        }

        // Structured message.
        let Value::Object(obj) = message else {
            return Err(ToolError::InvalidInput(
                "SendMessage: 'message' must be a string or a structured object".into(),
            ));
        };

        // structured messages cannot be broadcast (to: "*").
        if addr == Address::Broadcast {
            return Err(ToolError::InvalidInput(
                "structured messages cannot be broadcast (to: \"*\")".into(),
            ));
        }

        let structured = parse_structured(obj)?;

        match structured {
            StructuredMessage::ShutdownRequest { reason } => {
                self.handle_shutdown_request(&addr, to, reason, &ctx).await
            }
            StructuredMessage::ShutdownResponse {
                request_id,
                approve,
                reason,
            } => {
                // shutdown_response must be sent to "team-lead".
                if !to.eq_ignore_ascii_case(TEAM_LEAD_NAME) {
                    return Err(ToolError::InvalidInput(format!(
                        "shutdown_response must be sent to \"{TEAM_LEAD_NAME}\""
                    )));
                }
                // reason required when rejecting.
                if !approve && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                    return Err(ToolError::InvalidInput(
                        "reason is required when rejecting a shutdown request".into(),
                    ));
                }
                self.handle_shutdown_response(request_id, approve, reason, &ctx)
                    .await
            }
            StructuredMessage::PlanApprovalResponse {
                request_id,
                approve,
                feedback,
            } => {
                self.handle_plan_approval(&addr, to, request_id, approve, feedback, &ctx)
                    .await
            }
        }
    }
}

/// Render the recipient label for the success message: the original `to` for a
/// name, or the resolved agent uuid for an id address.
fn display_target(raw: &str, addr: &Address) -> String {
    match addr {
        Address::AgentId(id) => id.as_uuid().to_string(),
        _ => raw.to_string(),
    }
}

/// Parse a structured message object into [`StructuredMessage`], mirroring the
/// TS discriminated union + `semanticBoolean` coercion (`SendMessageTool.ts:
/// 46-65`). `request_id` / `approve` are required on the response variants.
fn parse_structured(obj: &serde_json::Map<String, Value>) -> Result<StructuredMessage, ToolError> {
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("SendMessage: structured message missing 'type'".into()))?;

    let request_id = || -> Result<String, ToolError> {
        obj.get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ToolError::InvalidInput("SendMessage: structured message missing 'request_id'".into())
            })
    };
    let approve = || -> Result<bool, ToolError> {
        semantic_boolean(obj.get("approve")).ok_or_else(|| {
            ToolError::InvalidInput("SendMessage: 'approve' must be a boolean".into())
        })
    };
    let opt_str = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);

    match ty {
        "shutdown_request" => Ok(StructuredMessage::ShutdownRequest {
            reason: opt_str("reason"),
        }),
        "shutdown_response" => Ok(StructuredMessage::ShutdownResponse {
            request_id: request_id()?,
            approve: approve()?,
            reason: opt_str("reason"),
        }),
        "plan_approval_response" => Ok(StructuredMessage::PlanApprovalResponse {
            request_id: request_id()?,
            approve: approve()?,
            feedback: opt_str("feedback"),
        }),
        other => Err(ToolError::InvalidInput(format!(
            "SendMessage: unknown structured message type '{other}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::context::{ToolUseContext, ToolUseOptions};
    use tool_api::progress::progress_channel;

    fn fresh_tx() -> ToolProgressSender {
        let (tx, _rx) = progress_channel();
        tx
    }

    fn fresh_ctx() -> ToolUseContext {
        ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: "test".into(),
                max_budget_nano_usd: None,
                mcp_clients: vec![],
                is_non_interactive_session: false,
                custom_system_prompt: None,
                append_system_prompt: None,
            },
            messages: vec![],
            tool_use_id: None,
            agent_id: None,
            agent_name: None,
            team_name: None,
            content_replacement_state: None,
            session: None,
            subagent_registry: None,
            cancel: None,
        }
    }

    fn ctx_as(agent: AgentId) -> ToolUseContext {
        let mut c = fresh_ctx();
        c.agent_id = Some(agent);
        c
    }

    fn make_registry() -> Arc<TeamRegistry> {
        Arc::new(TeamRegistry::new(AgentId::new()))
    }

    /// Register a known mailbox for `agent_id` so `drain()` is observable.
    async fn observable_mailbox(
        registry: &Arc<TeamRegistry>,
        agent_id: AgentId,
    ) -> Arc<crate::mailbox::TeammateMailbox> {
        let mailbox = Arc::new(crate::mailbox::TeammateMailbox::new(agent_id));
        registry
            .mailbox_router
            .register(agent_id, mailbox.clone())
            .await;
        mailbox
    }

    #[test]
    fn name_matches_ts_constant() {
        let tool = SendMessageTool::new(make_registry());
        assert_eq!(tool.name(), "SendMessage");
        assert_eq!(SEND_MESSAGE_TOOL_NAME, "SendMessage");
    }

    #[test]
    fn schema_shape_matches_ts() {
        let tool = SendMessageTool::new(make_registry());
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["to", "message"]));
        assert!(schema["properties"]["to"].is_object());
        assert!(schema["properties"]["summary"].is_object());
        assert!(schema["properties"]["message"]["oneOf"].is_array());
    }

    #[test]
    fn parse_address_covers_all_schemes() {
        assert_eq!(parse_address("*"), Address::Broadcast);
        assert_eq!(parse_address("uds:/tmp/s.sock"), Address::Uds("/tmp/s.sock".into()));
        assert_eq!(parse_address("/tmp/s.sock"), Address::Uds("/tmp/s.sock".into()));
        assert_eq!(parse_address("bridge:abc"), Address::Bridge("abc".into()));
        assert_eq!(parse_address("scout"), Address::Name("scout".into()));
        let id = AgentId::new();
        assert_eq!(parse_address(&id.as_uuid().to_string()), Address::AgentId(id));
        assert_eq!(parse_address(&format!("agent:{}", id.as_uuid())), Address::AgentId(id));
    }

    #[test]
    fn semantic_boolean_matches_ts() {
        assert_eq!(semantic_boolean(Some(&json!(true))), Some(true));
        assert_eq!(semantic_boolean(Some(&json!(false))), Some(false));
        assert_eq!(semantic_boolean(Some(&json!("true"))), Some(true));
        assert_eq!(semantic_boolean(Some(&json!("false"))), Some(false));
        assert_eq!(semantic_boolean(Some(&json!("yes"))), None);
        assert_eq!(semantic_boolean(None), None);
    }

    #[test]
    fn is_read_only_true_only_for_string_message() {
        let tool = SendMessageTool::new(make_registry());
        assert!(tool.is_read_only(&json!({ "to": "x", "message": "hi" })));
        assert!(!tool.is_read_only(
            &json!({ "to": "x", "message": { "type": "shutdown_request" } })
        ));
    }

    #[tokio::test]
    async fn delivers_string_message_to_recipient_mailbox() {
        let registry = make_registry();
        let agent_id = registry
            .spawn_worker("explorer".into(), "scout".into(), "task-1".into())
            .await
            .expect("spawn_worker must succeed");
        let mailbox = observable_mailbox(&registry, agent_id).await;

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": agent_id.as_uuid().to_string(),
            "summary": "assign first task",
            "message": "start on task #1"
        });

        let res = tool
            .call(input, fresh_ctx(), fresh_tx())
            .await
            .expect("send must succeed");
        assert_eq!(res.data["success"], true);

        let drained = mailbox.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].content, "start on task #1");
        assert!(matches!(drained[0].from, MessageSender::Coordinator));
    }

    #[tokio::test]
    async fn delivers_string_message_by_teammate_name() {
        let registry = make_registry();
        let agent_id = registry
            .spawn_worker("explorer".into(), "Scout".into(), "task-1".into())
            .await
            .unwrap();
        let mailbox = observable_mailbox(&registry, agent_id).await;

        let tool = SendMessageTool::new(registry);
        // Case-insensitive name resolution.
        let input = json!({ "to": "scout", "summary": "go", "message": "begin" });
        let res = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);

        let drained = mailbox.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].content, "begin");
    }

    #[tokio::test]
    async fn unknown_teammate_name_is_invalid_input() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry);
        let input = json!({ "to": "nobody", "summary": "x", "message": "y" });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn broadcast_fans_out_to_all_but_self() {
        let registry = make_registry();
        let a = registry.spawn_worker("e".into(), "alpha".into(), "t-a".into()).await.unwrap();
        let b = registry.spawn_worker("e".into(), "beta".into(), "t-b".into()).await.unwrap();
        let c = registry.spawn_worker("e".into(), "gamma".into(), "t-c".into()).await.unwrap();
        let mb_a = observable_mailbox(&registry, a).await;
        let mb_b = observable_mailbox(&registry, b).await;
        let mb_c = observable_mailbox(&registry, c).await;

        let tool = SendMessageTool::new(registry);
        // alpha broadcasts → beta + gamma receive, alpha does not.
        let input = json!({ "to": "*", "summary": "all hands", "message": "sync up" });
        let res = tool.call(input, ctx_as(a), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        let recipients = res.data["recipients"].as_array().unwrap();
        assert_eq!(recipients.len(), 2, "self excluded: {recipients:?}");

        assert!(mb_a.drain().is_empty(), "sender must not receive its own broadcast");
        assert_eq!(mb_b.drain().len(), 1);
        assert_eq!(mb_c.drain().len(), 1);
    }

    #[tokio::test]
    async fn broadcast_alone_reports_no_teammates() {
        let registry = make_registry();
        let solo = registry.spawn_worker("e".into(), "solo".into(), "t".into()).await.unwrap();
        let tool = SendMessageTool::new(registry);
        let input = json!({ "to": "*", "summary": "hi", "message": "anyone?" });
        let res = tool.call(input, ctx_as(solo), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["recipients"].as_array().unwrap().len(), 0);
        assert_eq!(
            res.data["message"],
            "No teammates to broadcast to (you are the only team member)"
        );
    }

    #[tokio::test]
    async fn shutdown_request_mints_request_id_and_delivers() {
        let registry = make_registry();
        let target = registry
            .spawn_worker("e".into(), "victim".into(), "t".into())
            .await
            .unwrap();
        let mailbox = observable_mailbox(&registry, target).await;

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "victim",
            "message": { "type": "shutdown_request", "reason": "done" }
        });
        let res = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        let rid = res.data["request_id"].as_str().unwrap();
        assert!(rid.starts_with("shutdown-"), "minted id: {rid}");
        assert!(rid.contains("@victim"), "id embeds target: {rid}");

        let drained = mailbox.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].request_id.as_deref(), Some(rid));
        let body: Value = serde_json::from_str(&drained[0].content).unwrap();
        assert_eq!(body["type"], "shutdown_request");
        assert_eq!(body["requestId"], rid);
    }

    #[tokio::test]
    async fn shutdown_response_routes_to_leader() {
        let coordinator = AgentId::new();
        let registry = Arc::new(TeamRegistry::new(coordinator));
        // The leader/coordinator must have a mailbox to receive the response.
        let leader_mb = observable_mailbox(&registry, coordinator).await;
        let worker = registry
            .spawn_worker("e".into(), "worker".into(), "t".into())
            .await
            .unwrap();

        let tool = SendMessageTool::new(registry);
        // Approve. Must be addressed to "team-lead".
        let input = json!({
            "to": "team-lead",
            "message": { "type": "shutdown_response", "request_id": "shutdown-1@worker", "approve": true }
        });
        let res = tool.call(input, ctx_as(worker), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["request_id"], "shutdown-1@worker");

        let drained = leader_mb.drain();
        assert_eq!(drained.len(), 1, "leader received the response");
        let body: Value = serde_json::from_str(&drained[0].content).unwrap();
        assert_eq!(body["type"], "shutdown_approved");
        assert_eq!(body["requestId"], "shutdown-1@worker");
    }

    #[tokio::test]
    async fn shutdown_response_to_wrong_target_is_rejected() {
        let registry = make_registry();
        let _w = registry.spawn_worker("e".into(), "worker".into(), "t".into()).await.unwrap();
        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "worker",
            "message": { "type": "shutdown_response", "request_id": "r1", "approve": true }
        });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert_eq!(m, "shutdown_response must be sent to \"team-lead\""),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutdown_response_reject_without_reason_is_rejected() {
        let coordinator = AgentId::new();
        let registry = Arc::new(TeamRegistry::new(coordinator));
        observable_mailbox(&registry, coordinator).await;
        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "team-lead",
            "message": { "type": "shutdown_response", "request_id": "r1", "approve": false }
        });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => {
                assert_eq!(m, "reason is required when rejecting a shutdown request");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approved_shutdown_signals_kill_via_seam() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct RecordingSeam {
            killed: AtomicBool,
            killed_task: std::sync::Mutex<Option<String>>,
        }
        #[async_trait]
        impl TeamSpawnSeam for RecordingSeam {
            async fn spawn_teammate(
                &self,
                _agent_id: AgentId,
                _name: String,
                _team_name: String,
                _description: String,
            ) -> Result<String, traits::team_spawn::TeamSpawnError> {
                Ok(String::new())
            }
            async fn kill(&self, task_id: &str) -> Result<(), traits::team_spawn::TeamSpawnError> {
                self.killed.store(true, Ordering::SeqCst);
                *self.killed_task.lock().unwrap() = Some(task_id.to_string());
                Ok(())
            }
        }

        let coordinator = AgentId::new();
        let registry = Arc::new(TeamRegistry::new(coordinator));
        observable_mailbox(&registry, coordinator).await;
        let worker = registry
            .spawn_worker("e".into(), "worker".into(), "task-worker".into())
            .await
            .unwrap();

        let seam = Arc::new(RecordingSeam {
            killed: AtomicBool::new(false),
            killed_task: std::sync::Mutex::new(None),
        });
        let tool = SendMessageTool::new(registry).with_spawn_seam(seam.clone());
        let input = json!({
            "to": "team-lead",
            "message": { "type": "shutdown_response", "request_id": "r1", "approve": true }
        });
        tool.call(input, ctx_as(worker), fresh_tx()).await.unwrap();

        assert!(seam.killed.load(Ordering::SeqCst), "approved shutdown must signal kill");
        assert_eq!(seam.killed_task.lock().unwrap().as_deref(), Some("task-worker"));
    }

    #[tokio::test]
    async fn plan_approval_routes_to_requesting_worker() {
        let registry = make_registry();
        let worker = registry
            .spawn_worker("e".into(), "planner".into(), "t".into())
            .await
            .unwrap();
        let mailbox = observable_mailbox(&registry, worker).await;

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "planner",
            "message": { "type": "plan_approval_response", "request_id": "plan-1@planner", "approve": true }
        });
        let res = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        assert_eq!(res.data["request_id"], "plan-1@planner");

        let drained = mailbox.drain();
        assert_eq!(drained.len(), 1);
        let body: Value = serde_json::from_str(&drained[0].content).unwrap();
        assert_eq!(body["type"], "plan_approval_response");
        assert_eq!(body["approved"], true);
    }

    #[tokio::test]
    async fn plan_rejection_includes_feedback() {
        let registry = make_registry();
        let worker = registry
            .spawn_worker("e".into(), "planner".into(), "t".into())
            .await
            .unwrap();
        let mailbox = observable_mailbox(&registry, worker).await;

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "planner",
            "message": {
                "type": "plan_approval_response",
                "request_id": "plan-1@planner",
                "approve": false,
                "feedback": "tighten the scope"
            }
        });
        let res = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap();
        assert_eq!(res.data["success"], true);
        let drained = mailbox.drain();
        let body: Value = serde_json::from_str(&drained[0].content).unwrap();
        assert_eq!(body["approved"], false);
        assert_eq!(body["feedback"], "tighten the scope");
    }

    #[tokio::test]
    async fn structured_broadcast_is_rejected() {
        let tool = SendMessageTool::new(make_registry());
        let input = json!({ "to": "*", "message": { "type": "shutdown_request" } });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => {
                assert_eq!(m, "structured messages cannot be broadcast (to: \"*\")");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uds_address_is_not_supported() {
        let tool = SendMessageTool::new(make_registry());
        let input = json!({ "to": "uds:/tmp/x.sock", "summary": "s", "message": "hi" });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("'uds:' addresses are not supported on this build"),
                "got {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn bridge_address_is_not_supported() {
        let tool = SendMessageTool::new(make_registry());
        let input = json!({ "to": "bridge:sess-1", "summary": "s", "message": "hi" });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("'bridge:' addresses are not supported on this build"),
                "got {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn string_message_without_summary_is_rejected() {
        let registry = make_registry();
        let agent_id = registry
            .spawn_worker("explorer".into(), "scout".into(), "task-1".into())
            .await
            .expect("spawn_worker must succeed");

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": agent_id.as_uuid().to_string(),
            "message": "no summary here"
        });

        let err = tool
            .call(input, fresh_ctx(), fresh_tx())
            .await
            .expect_err("string message without summary must be rejected");
        match err {
            ToolError::InvalidInput(m) => {
                assert_eq!(m, "summary is required when message is a string");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn at_bearing_recipient_is_invalid_input() {
        let tool = SendMessageTool::new(make_registry());
        let input = json!({
            "to": "team-lead@my-team",
            "summary": "hi",
            "message": "yo"
        });
        let err = tool
            .call(input, fresh_ctx(), fresh_tx())
            .await
            .expect_err("an '@'-bearing recipient must be rejected");
        match err {
            ToolError::InvalidInput(m) => assert!(m.contains("there is only one team per session"), "got {m}"),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_agent_id_recipient_is_invalid_input() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry);
        let stranger = AgentId::new();
        let input = json!({
            "to": stranger.as_uuid().to_string(),
            "summary": "hello there",
            "message": "anyone home?"
        });
        let err = tool
            .call(input, fresh_ctx(), fresh_tx())
            .await
            .expect_err("routing to an unknown agent must fail");
        assert!(matches!(err, ToolError::InvalidInput(_)), "got {err:?}");
    }
}
