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
//! - [`Address::SessionId`]: the canonical local cross-session UUID address.
//! - [`Address::Uds`] / [`Address::Bridge`]: legacy/cross-machine schemes; direct
//!   forms remain unsupported while canonical local sessions route through the
//!   live registry.
//!
//! ## Deferred (noted, not attempted)
//!
//! - The pending-message queue + stopped-agent auto-resume
//!   (`SendMessageTool.ts:802-874`) — needs the live teammate task loop.
//! - User-supplied raw `uds:` / remote `bridge:` addresses and the bridge `ask`
//!   permission escalation (`checkPermissions`, `SendMessageTool.ts:585-602`).
//!   Canonical `session:<uuid>` already uses the registered UDS endpoint with a
//!   JSONL fallback.
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
    /// A live local session id (`session:<uuid>`).
    SessionId(String),
    /// A malformed canonical session address. Keep this distinct from a
    /// teammate name so invalid `session:` input can never be silently routed
    /// through the team mailbox.
    InvalidSession(String),
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
    if let Some(rest) = trimmed.strip_prefix("session:") {
        if let Some(id) = protocol::SessionId::parse_prefixed(rest) {
            return Address::SessionId(id.as_uuid().to_string());
        }
        return Address::InvalidSession(trimmed.to_string());
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
                "description": "Recipient: teammate name, session:<uuid>, \"*\" for broadcast, \"uds:<socket-path>\" local peer, \"bridge:<session-id>\" Remote Control peer"
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

    /// Resolve the model-visible sender label carried by the teammate envelope.
    async fn sender_name(&self, ctx: &ToolUseContext) -> String {
        let Some(agent_id) = ctx.agent_id else {
            return TEAM_LEAD_NAME.to_string();
        };
        self.team
            .find_by_agent_id(&agent_id)
            .await
            .map_or_else(|| agent_id.to_string(), |worker| worker.name)
    }

    /// Deliver directly to the live-session registry using the stable session
    /// UUID. UDS is preferred; the existing JSONL inbox is the fallback.
    async fn route_live_session(
        &self,
        session_id: &str,
        content: &str,
        summary: Option<&str>,
        ctx: &ToolUseContext,
    ) -> Result<(), ToolError> {
        if traits::live_sessions::process_session_id().as_deref() == Some(session_id) {
            return Err(ToolError::InvalidInput(
                "SendMessage: cannot send a message to the current session".into(),
            ));
        }
        let dir = traits::live_sessions::process_dir()
            .unwrap_or_else(traits::live_sessions::LiveSessionDir::process_default);
        let peer = dir
            .find_by_session_id(
                session_id,
                traits::live_sessions::process_session_id().as_deref(),
            )
            .ok_or_else(|| ToolError::InvalidInput("SendMessage: no such live session".into()))?;
        let from_name = self.sender_name(ctx).await;
        let from_sid = traits::live_sessions::process_session_id().unwrap_or_default();
        let message =
            traits::live_sessions::outbound_peer_message(&from_name, &from_sid, content, summary);
        let socket = peer
            .messaging_socket_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .filter(|path| traits::uds_inbox::is_canonical_inbox_sock(path));
        let uds_error = socket
            .as_deref()
            .map(|path| traits::uds_inbox::send_peer_message(path, &message))
            .and_then(Result::err);
        if socket.is_none() || uds_error.is_some() {
            dir.send_inbox(peer.sid(), &message).map_err(|error| {
                let transport = uds_error
                    .as_ref()
                    .map_or_else(String::new, |uds| format!(" (UDS failed first: {uds})"));
                ToolError::Internal(format!("SendMessage: {error}{transport}"))
            })?;
        }
        Ok(())
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
            Address::Broadcast
            | Address::SessionId(_)
            | Address::InvalidSession(_)
            | Address::Uds(_)
            | Address::Bridge(_) => Err(ToolError::Internal(
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
                MailboxError::NotFound(id) => ToolError::InvalidInput(format!(
                    "SendMessage: no such worker: {}",
                    id.as_uuid()
                )),
                other => ToolError::Internal(format!("SendMessage: {other}")),
            })
    }

    /// `handleBroadcast` (`SendMessageTool.ts:191-266`): enumerate the team and
    /// route to every member except the sender (case-insensitive self-skip).
    async fn handle_broadcast(
        &self,
        content: String,
        summary: String,
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
        let from_name = self.sender_name(ctx).await;
        let mut delivered: Vec<String> = Vec::with_capacity(recipients.len());
        for w in &recipients {
            let msg = TeammateMessage {
                from: from.clone(),
                from_name: from_name.clone(),
                content: content.clone(),
                summary: Some(summary.clone()),
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
            from_name: self.sender_name(ctx).await,
            content,
            summary: None,
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
            from_name: self.sender_name(ctx).await,
            content,
            summary: None,
            message_id: tool_api::util::ids::ulid_or_uuid(),
            timestamp: SystemTime::now(),
            request_id: Some(request_id.clone()),
        };
        self.route(&leader, msg).await?;

        // On approval, signal the responding (calling) worker's backing task to
        // cancel — the in-process analog of `task.abortController.abort()` —
        // then run the oracle's departure sequence (2.1.223 `Urv` @261175890 /
        // print.ts shutdown_approved @262044791): remove the member from the
        // team file (`jqt`), unassign its tasks (`RSr`), and deliver the
        // notification to the LEAD's mailbox as a `teammate_terminated` frame
        // (`Qyt` schema @248040794: `{type, message}`).
        //
        // PLACEMENT NOTE (topology adaptation, not a byte-mapped call site):
        // the oracle runs this inside the lead's inbox-processing loop when
        // the `shutdown_approved` frame arrives; the port has no lead-side
        // poll loop yet, and this approval handler is the earliest point that
        // knows the teammate is going away — same sequence, different host.
        if approve {
            if let Some(agent_id) = ctx.agent_id {
                let worker = self.team.find_by_agent_id(&agent_id).await;
                if let (Some(seam), Some(worker)) = (&self.spawn_seam, &worker) {
                    if !worker.task_id.is_empty() {
                        // Best-effort: a kill failure does not fail the response send.
                        let _ = seam.kill(&worker.task_id).await;
                    }
                }
                if let (Some(team_name), Some(worker)) = (self.team.team_name().await, &worker) {
                    let agent_id_str = agent_id.to_string();
                    // jqt: drop the member from config.json first (oracle order:
                    // team file → RSr → lead notification). Best-effort — a
                    // missing/corrupt team file must not fail the response.
                    if let Some(home) = crate::team_file::lingxi_home() {
                        let _ = crate::team_file::remove_team_member(
                            &home,
                            &team_name,
                            &agent_id_str,
                            &worker.name,
                        );
                    }
                    // RSr over the shared task list. List-id resolution matches
                    // the teammate auto-claim: env override, else the team name
                    // (the tools' resolve_task_list_id first two levels).
                    let list_id = std::env::var("LINGXI_TASK_LIST_ID")
                        .ok()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| team_name.clone());
                    let outcome = task_store::TodoStore::for_list(&list_id)
                        .unassign_tasks_for_teammate(
                            &agent_id_str,
                            &worker.name,
                            task_store::TeammateEndReason::Shutdown,
                        )
                        .await;
                    // Lead notification frame (Qyt): `{type:"teammate_terminated",
                    // message}` routed into the leader's mailbox like any other
                    // inter-agent frame. Best-effort.
                    let frame = serde_json::to_string(&json!({
                        "type": "teammate_terminated",
                        "message": outcome.notification_message,
                    }))
                    .unwrap_or_default();
                    let _ = self
                        .route(
                            &leader,
                            TeammateMessage {
                                from: Self::sender_from(ctx),
                                from_name: self.sender_name(ctx).await,
                                content: frame,
                                summary: None,
                                message_id: tool_api::util::ids::ulid_or_uuid(),
                                timestamp: SystemTime::now(),
                                request_id: Some(request_id.clone()),
                            },
                        )
                        .await;
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
            from_name: self.sender_name(ctx).await,
            content,
            summary: None,
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
            model_content: None,
            new_messages: Vec::new(),
            context_modifier: None,
            is_error: false,
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
        // scheme; direct bridge addresses are rejected up front in `call`.
        // Canonical local session sends and teammate sends are
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
        if let Address::InvalidSession(raw) = &addr {
            return Err(ToolError::InvalidInput(format!(
                "SendMessage: invalid session address '{raw}'; expected session:<uuid>"
            )));
        }
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
        if let Address::SessionId(session_id) = &addr {
            if traits::live_sessions::process_session_id().as_deref() == Some(session_id) {
                return Err(ToolError::InvalidInput(
                    "SendMessage: cannot send a message to the current session".into(),
                ));
            }
        }
        // Raw transport addresses remain private implementation details. Models
        // address a local peer through canonical `session:<uuid>` instead.
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
            let summary = summary.expect("validated above").trim().to_string();
            return match addr {
                Address::Broadcast => self.handle_broadcast(s.clone(), summary, &ctx).await,
                Address::SessionId(session_id) => {
                    self.route_live_session(&session_id, s, Some(&summary), &ctx)
                        .await?;
                    Ok(Self::ok(json!({
                        "success": true,
                        "message": format!("Message sent to session:{session_id}'s inbox"),
                    })))
                }
                ref a => {
                    let to_id = self.resolve_recipient(a).await?;
                    let msg = TeammateMessage {
                        from: Self::sender_from(&ctx),
                        from_name: self.sender_name(&ctx).await,
                        content: s.clone(),
                        summary: Some(summary),
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

        if matches!(addr, Address::SessionId(_)) {
            return Err(ToolError::InvalidInput(
                "SendMessage: structured protocol messages cannot target session:<uuid>; use a team teammate address"
                    .into(),
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
    let ty = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
        ToolError::InvalidInput("SendMessage: structured message missing 'type'".into())
    })?;

    let request_id = || -> Result<String, ToolError> {
        obj.get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ToolError::InvalidInput(
                    "SendMessage: structured message missing 'request_id'".into(),
                )
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

    static LIVE_SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
                model_profile: None,
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
            observer: None,
            team_name: None,
            content_replacement_state: None,
            session: None,
            subagent_registry: None,
            cancel: None,
            fork_parent_system_prompt: None,
            cwd: None,
            depth: 0,
            file_history: None,
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
        assert_eq!(
            parse_address("uds:/tmp/s.sock"),
            Address::Uds("/tmp/s.sock".into())
        );
        assert_eq!(
            parse_address("/tmp/s.sock"),
            Address::Uds("/tmp/s.sock".into())
        );
        assert_eq!(parse_address("bridge:abc"), Address::Bridge("abc".into()));
        assert_eq!(
            parse_address("session:not-a-uuid"),
            Address::InvalidSession("session:not-a-uuid".into())
        );
        assert_eq!(parse_address("scout"), Address::Name("scout".into()));
        assert_eq!(
            parse_address("session:11111111-2222-3333-4444-555555555555"),
            Address::SessionId("11111111-2222-3333-4444-555555555555".into())
        );
        let id = AgentId::new();
        assert_eq!(
            parse_address(&id.as_uuid().to_string()),
            Address::AgentId(id)
        );
        assert_eq!(
            parse_address(&format!("agent:{}", id.as_uuid())),
            Address::AgentId(id)
        );
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
        assert!(
            !tool.is_read_only(&json!({ "to": "x", "message": { "type": "shutdown_request" } }))
        );
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
        let a = registry
            .spawn_worker("e".into(), "alpha".into(), "t-a".into())
            .await
            .unwrap();
        let b = registry
            .spawn_worker("e".into(), "beta".into(), "t-b".into())
            .await
            .unwrap();
        let c = registry
            .spawn_worker("e".into(), "gamma".into(), "t-c".into())
            .await
            .unwrap();
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

        assert!(
            mb_a.drain().is_empty(),
            "sender must not receive its own broadcast"
        );
        assert_eq!(mb_b.drain().len(), 1);
        assert_eq!(mb_c.drain().len(), 1);
    }

    #[tokio::test]
    async fn broadcast_alone_reports_no_teammates() {
        let registry = make_registry();
        let solo = registry
            .spawn_worker("e".into(), "solo".into(), "t".into())
            .await
            .unwrap();
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
    async fn malformed_session_address_is_not_routed_as_a_teammate_name() {
        let registry = make_registry();
        registry
            .spawn_worker("e".into(), "session:not-a-uuid".into(), "t".into())
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry);
        let err = tool
            .call(
                json!({"to": "session:not-a-uuid", "message": "hello", "summary": "hi"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("malformed session address must be invalid input");
        match err {
            ToolError::InvalidInput(message) => {
                assert!(message.contains("invalid session address"), "{message}");
                assert!(message.contains("expected session:<uuid>"), "{message}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn structured_protocol_message_rejects_session_target() {
        let session_id = "11111111-2222-4333-8444-555555555555";
        let tool = SendMessageTool::new(make_registry());
        let err = tool
            .call(
                json!({
                    "to": format!("session:{session_id}"),
                    "message": {"type": "shutdown_request", "reason": "done"}
                }),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("team protocol messages must not target a session");
        match err {
            ToolError::InvalidInput(message) => assert_eq!(
                message,
                "SendMessage: structured protocol messages cannot target session:<uuid>; use a team teammate address"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn canonical_session_send_falls_back_when_the_advertised_socket_is_stale() {
        let _guard = LIVE_SESSION_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::TempDir::new().unwrap();
        let dir = traits::live_sessions::LiveSessionDir::at(temp.path().join("sessions"));
        let self_session = "22222222-3333-4444-8555-666666666666";
        let target_session = "11111111-2222-4333-8444-555555555555";
        traits::live_sessions::set_process_dir(dir.clone());
        traits::live_sessions::set_process_session_id(self_session);
        traits::live_sessions::set_process_name("team-lead");
        let stale_socket = traits::uds_inbox::default_socket_path(424_242);
        dir.upsert_identity(
            424_242,
            target_session,
            Some("peer"),
            None,
            Some(&stale_socket),
            Some("prompting"),
        )
        .unwrap();

        let tool = SendMessageTool::new(make_registry());
        tool.call(
            json!({
                "to": format!("session:{target_session}"),
                "summary": "fallback route",
                "message": "hello after stale UDS"
            }),
            fresh_ctx(),
            fresh_tx(),
        )
        .await
        .expect("file fallback must preserve delivery");

        let messages = dir.drain_inbox(target_session).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            traits::live_sessions::extract_cross_session_inner(&messages[0].content),
            "hello after stale UDS"
        );
        assert!(messages[0].msg_id.is_some());
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
        let _w = registry
            .spawn_worker("e".into(), "worker".into(), "t".into())
            .await
            .unwrap();
        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "worker",
            "message": { "type": "shutdown_response", "request_id": "r1", "approve": true }
        });
        let err = tool.call(input, fresh_ctx(), fresh_tx()).await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => {
                assert_eq!(m, "shutdown_response must be sent to \"team-lead\"")
            }
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

        assert!(
            seam.killed.load(Ordering::SeqCst),
            "approved shutdown must signal kill"
        );
        assert_eq!(
            seam.killed_task.lock().unwrap().as_deref(),
            Some("task-worker")
        );
    }

    /// Serializes the env-mutating departure test below.
    static DEPART_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The full oracle departure sequence on shutdown approval (Urv
    /// @261175890 / print.ts @262044791): member removed from the team file
    /// (jqt), the teammate's tasks unassigned to ownerless-pending (RSr), and
    /// the lead's mailbox receives the `{type:"teammate_terminated", message}`
    /// frame (Qyt @248040794) carrying RSr's byte-exact notification.
    #[tokio::test]
    async fn approved_shutdown_removes_member_unassigns_tasks_and_notifies_lead() {
        let _lock = DEPART_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let prev_config = std::env::var_os(branding::CONFIG_DIR_ENV);
        let prev_list = std::env::var_os("LINGXI_TASK_LIST_ID");
        std::env::set_var(branding::CONFIG_DIR_ENV, tmp.path());
        std::env::remove_var("LINGXI_TASK_LIST_ID");
        struct EnvRestore(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
        impl Drop for EnvRestore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
                    None => std::env::remove_var(branding::CONFIG_DIR_ENV),
                }
                match &self.1 {
                    Some(v) => std::env::set_var("LINGXI_TASK_LIST_ID", v),
                    None => std::env::remove_var("LINGXI_TASK_LIST_ID"),
                }
            }
        }
        let _restore = EnvRestore(prev_config, prev_list);

        let team_name = "depart-team";
        let coordinator = AgentId::new();
        let registry = Arc::new(TeamRegistry::new(coordinator));
        registry.set_team_name(Some(team_name.to_string())).await;
        let lead_mailbox = observable_mailbox(&registry, coordinator).await;
        let worker = registry
            .spawn_worker("e".into(), "nova".into(), "task-nova".into())
            .await
            .unwrap();

        // Team file with the lead + the departing member.
        let home = crate::team_file::lingxi_home().unwrap();
        crate::team_file::write_team_file(
            &home,
            team_name,
            &crate::team_file::TeamFile {
                name: team_name.into(),
                description: None,
                created_at: 0,
                lead_agent_id: format!("team-lead@{team_name}"),
                lead_session_id: None,
                members: vec![
                    crate::team_file::TeamMember {
                        agent_id: format!("team-lead@{team_name}"),
                        name: "team-lead".into(),
                        agent_type: None,
                        model: None,
                        joined_at: 0,
                        tmux_pane_id: String::new(),
                        cwd: String::new(),
                        subscriptions: vec![],
                    },
                    crate::team_file::TeamMember {
                        agent_id: worker.to_string(),
                        name: "nova".into(),
                        agent_type: None,
                        model: None,
                        joined_at: 0,
                        tmux_pane_id: String::new(),
                        cwd: String::new(),
                        subscriptions: vec![],
                    },
                ],
            },
        )
        .unwrap();

        // The departing teammate owns one open task on the shared list.
        let store = task_store::TodoStore::for_list(team_name);
        let mut owned = task_store::TodoTask::new(
            "Fix parser".into(),
            "d".into(),
            None,
            serde_json::Map::new(),
        );
        owned.status = engine::TodoState::InProgress;
        owned.owner = Some("nova".into());
        let tid = store.create(owned).await.unwrap();

        let tool = SendMessageTool::new(registry);
        let input = json!({
            "to": "team-lead",
            "message": { "type": "shutdown_response", "request_id": "r9", "approve": true }
        });
        tool.call(input, ctx_as(worker), fresh_tx()).await.unwrap();

        // jqt: the member is gone from config.json; the lead remains.
        let file = crate::team_file::read_team_file(&home, team_name).unwrap();
        assert_eq!(file.members.len(), 1);
        assert_eq!(file.members[0].name, "team-lead");

        // RSr: the task is ownerless-pending again.
        let t = store.get(&tid).await.unwrap();
        assert_eq!(t.owner, None);
        assert_eq!(t.status, engine::TodoState::Pending);

        // Qyt frame in the lead's inbox, after the shutdown_approved frame.
        let frames = lead_mailbox.drain();
        let terminated = frames
            .iter()
            .find_map(|m| {
                let v: serde_json::Value = serde_json::from_str(&m.content).ok()?;
                (v["type"] == "teammate_terminated").then_some(v)
            })
            .expect("lead received the teammate_terminated frame");
        assert_eq!(
            terminated["message"],
            format!(
                "nova has shut down. 1 task(s) were unassigned: #{tid} \"Fix parser\". Use TaskList to check availability and TaskUpdate with owner to reassign them to idle teammates."
            )
        );
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
            ToolError::InvalidInput(m) => {
                assert!(m.contains("there is only one team per session"), "got {m}")
            }
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
