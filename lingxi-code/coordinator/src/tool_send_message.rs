//! `SendMessageTool` — the coordinator-only `SendMessage` tool.
//!
//! 1:1 Rust port of the lingxi/claude-code `SendMessageTool`
//! (`src/tools/SendMessageTool/SendMessageTool.ts`, name constant
//! `SEND_MESSAGE_TOOL_NAME = "SendMessage"`). On the coordinator side the
//! recipient is a worker's [`AgentId`], so this tool resolves the `to`
//! agent id and routes a [`TeammateMessage`] to that worker's mailbox via
//! [`TeamRegistry::mailbox_router`] — the in-process-subagent routing branch
//! the coordinator uses to continue a worker by its agent id
//! (see `coordinatorMode.ts`: *"Continue an existing worker (send a
//! follow-up to its `to` agent ID)"*).
//!
//! Telemetry is intentionally omitted: coordinator-side tools hold an
//! `Arc<TeamRegistry>` directly rather than a `BuiltinToolContext`, so there
//! is no `bus` to emit on (per the implementation brief, telemetry is
//! optional for these tools).

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

use crate::mailbox::{MailboxError, MessageSender, TeammateMessage};
use crate::team_registry::TeamRegistry;

/// Canonical tool name — must match `SEND_MESSAGE_TOOL_NAME` in
/// `src/tools/SendMessageTool/constants.ts` exactly.
pub const SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";

/// Short human-readable description — matches `DESCRIPTION` in
/// `src/tools/SendMessageTool/prompt.ts`.
const DESCRIPTION: &str = "Send a message to another agent";

/// `maxResultSizeChars` in the TS tool is `100_000`.
const MAX_RESULT_SIZE_CHARS: usize = 100_000;

/// Lazily-built input schema cache.
///
/// `std::sync::OnceLock` is used instead of `once_cell::sync::Lazy` so the
/// file stays self-contained: the coordinator crate does not directly depend
/// on `once_cell`.
static INPUT_SCHEMA: OnceLock<Value> = OnceLock::new();

/// Build the JSON Schema for `SendMessage` inputs.
///
/// Mirrors the TS `z.object({ to, summary, message })` 1:1: `to` and
/// `message` are required, `summary` is optional, and `message` is a
/// `oneOf` plain-string / `StructuredMessage` discriminated union.
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
/// [`AgentId`] and route through the registry's `mailbox_router`.
pub struct SendMessageTool {
    team: Arc<TeamRegistry>,
}

impl SendMessageTool {
    /// Construct a new tool wired to the shared team registry.
    #[must_use]
    pub fn new(team: Arc<TeamRegistry>) -> Self {
        Self { team }
    }

    /// Parse the `to` field into a recipient [`AgentId`].
    ///
    /// On the coordinator side `to` is the worker's agent id (a UUID), as
    /// produced by `TeamRegistry::spawn_worker` / surfaced in the
    /// `<task-notification>`'s `<task-id>`. We accept both the bare UUID and
    /// the `AgentId` display form (`"agent:<uuid>"`).
    ///
    /// # Errors
    /// Returns [`ToolError::InvalidInput`] when `to` is empty, contains `@`
    /// (one team per session — bare name / `*` only in the TS contract), or
    /// is not a parseable agent id.
    fn parse_recipient(to: &str) -> Result<AgentId, ToolError> {
        let trimmed = to.trim();
        if trimmed.is_empty() {
            return Err(ToolError::InvalidInput(
                "SendMessage: 'to' must not be empty".into(),
            ));
        }
        if trimmed.contains('@') {
            return Err(ToolError::InvalidInput(
                "SendMessage: 'to' must be a bare agent id — there is only one team per session"
                    .into(),
            ));
        }
        // Accept the `agent:<uuid>` display form as well as a raw UUID.
        let candidate = trimmed.strip_prefix("agent:").unwrap_or(trimmed);
        let uuid = Uuid::parse_str(candidate).map_err(|_| {
            ToolError::InvalidInput(format!(
                "SendMessage: 'to' is not a valid agent id: {trimmed}"
            ))
        })?;
        Ok(AgentId::from_uuid(uuid))
    }
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
        // scheme, which does not exist on the coordinator side (recipient is
        // always a local worker agent id). All coordinator sends are allowed.
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
            "Continue an existing worker by sending a follow-up to its agent id.\n\n",
            "```json\n",
            "{\"to\": \"agent-a1b\", \"summary\": \"fix npe\", \"message\": \"Fix the null pointer...\"}\n",
            "```\n\n",
            "Your plain text output is NOT visible to other agents — to communicate, ",
            "you MUST call this tool. Refer to the worker by its agent id (the ",
            "`<task-id>` from its `<task-notification>`)."
        )
        .into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // --- Parse `to` (defensively, not relying on schema validation) ----
        let to = input
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("SendMessage: missing 'to'".into()))?;
        let to_agent_id = Self::parse_recipient(to)?;

        // --- Parse `message`: plain string or structured object -------------
        let message = input
            .get("message")
            .ok_or_else(|| ToolError::InvalidInput("SendMessage: missing 'message'".into()))?;

        let content = match message {
            Value::String(s) => {
                // TS: a string message requires a non-empty `summary`.
                let summary = input.get("summary").and_then(Value::as_str);
                if summary.is_none_or(|s| s.trim().is_empty()) {
                    return Err(ToolError::InvalidInput(
                        "summary is required when message is a string".into(),
                    ));
                }
                s.clone()
            }
            // Structured messages: serialize the object verbatim as the
            // mailbox payload (mirrors the TS `jsonStringify(structured)`
            // path written to the recipient mailbox).
            Value::Object(_) => serde_json::to_string(message).map_err(|e| {
                ToolError::Internal(format!("SendMessage: failed to serialize message: {e}"))
            })?,
            _ => {
                return Err(ToolError::InvalidInput(
                    "SendMessage: 'message' must be a string or a structured object".into(),
                ));
            }
        };

        // --- Determine the sender ------------------------------------------
        // If the calling agent id is known, attribute the message to that
        // teammate; otherwise it is from the coordinator itself.
        let from = ctx
            .agent_id
            .map_or(MessageSender::Coordinator, MessageSender::Teammate);

        let msg = TeammateMessage {
            from,
            content,
            message_id: tool_api::util::ids::ulid_or_uuid(),
            timestamp: SystemTime::now(),
        };

        // --- Route to the recipient worker's mailbox ------------------------
        self.team
            .mailbox_router
            .route(&to_agent_id, msg)
            .await
            .map_err(|e| match e {
                // Unknown recipient → cleaner "invalid input" surface.
                MailboxError::NotFound(id) => ToolError::InvalidInput(format!(
                    "SendMessage: no such worker: {}",
                    id.as_uuid()
                )),
                other => ToolError::Internal(format!("SendMessage: {other}")),
            })?;

        Ok(ToolCallResult {
            data: json!({
                "success": true,
                "message": format!("Message sent to {}'s inbox", to_agent_id.as_uuid()),
            }),
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
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
            content_replacement_state: None,
            session: None,
            subagent_registry: None,
        }
    }

    fn make_registry() -> Arc<TeamRegistry> {
        Arc::new(TeamRegistry::new(AgentId::new()))
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

        // Grab the recipient mailbox so we can drain it after the send.
        // (Re-register a known mailbox so `drain()` is observable.)
        let mailbox = Arc::new(crate::mailbox::TeammateMailbox::new(agent_id));
        registry
            .mailbox_router
            .register(agent_id, mailbox.clone())
            .await;

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
    async fn unknown_recipient_is_invalid_input() {
        let registry = make_registry();
        let tool = SendMessageTool::new(registry);
        // A well-formed but unregistered agent id.
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
    async fn malformed_recipient_is_invalid_input() {
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
        assert!(matches!(err, ToolError::InvalidInput(_)), "got {err:?}");
    }
}
