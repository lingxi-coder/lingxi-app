//! Subagent state-machine loop.
//!
//! [`run_subagent`] is the future that
//! [`crate::pool::StateMachinePool::allocate`] hands to the runtime. M1.11
//! ships a stub completion after the first inbound event; the full agentic
//! loop in Plan 09+ uses `§22 SessionStorage` and `§23 FileStateCache`.

use crate::context::SubagentContext;
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Events emitted by [`run_subagent`] back to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentEvent {
    /// Periodic progress beacon while the agent is still running.
    Progress {
        /// Agent emitting the progress event.
        agent_id: AgentId,
        /// Number of tool calls processed so far.
        tool_use_count: u32,
        /// Cumulative token count consumed so far.
        token_count: u64,
    },
    /// Agent finished normally with `result`.
    Completed {
        /// Agent that completed.
        agent_id: AgentId,
        /// Final result payload (free-form JSON).
        result: serde_json::Value,
    },
    /// Agent terminated due to an error.
    Failed {
        /// Agent that failed.
        agent_id: AgentId,
        /// Human-readable error message.
        error: String,
    },
    /// Agent was cancelled by the host.
    Killed {
        /// Agent that was killed.
        agent_id: AgentId,
    },
    /// A raw message produced by the agent (assistant or tool result).
    Message {
        /// Agent that produced the message.
        agent_id: AgentId,
        /// Free-form message payload (full schema lands in Plan 09+).
        message: serde_json::Value,
    },
}

/// Subagent state-machine loop.
///
/// Drives [`lingxi_core::reduce`] over `event_rx` and emits
/// [`SubagentEvent`]s on `out_tx`. M1.11 stubs completion after the first
/// event so the pool can be wired end-to-end before the full agentic loop
/// arrives in Plan 09+.
pub async fn run_subagent(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<lingxi_core::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    use lingxi_core::{reduce, ConversationState, SessionState};
    use lingxi_protocol::SessionId;

    let agent_id = ctx.agent_id;

    // Seed initial state. The runner's local SessionState is transient —
    // the orchestrator (M5-02) owns durable session persistence. We use
    // SessionId::nil() and an inherited model string projected from the
    // agent definition; both are placeholders the reducer accepts.
    let model = match &ctx.agent_definition.model {
        crate::definition::AgentModel::Inherit => "inherit".to_string(),
        crate::definition::AgentModel::Alias(n) => n.clone(),
        crate::definition::AgentModel::Explicit(n) => n.clone(),
    };
    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::nil(), model),
    };
    // Track whether at least one Message has been emitted — informs the
    // EOF branch's choice between Completed(graceful) and Failed.
    let mut produced_useful_work = false;

    while let Some(event) = event_rx.recv().await {
        // Capture whether this event represents a stream-end completion
        // BEFORE the reducer consumes it — we need to peek at the
        // final_message for the Message emit.
        let api_end_msg = match &event {
            lingxi_core::Event::ApiStreamEnd { final_message, .. } => Some(final_message.clone()),
            _ => None,
        };

        let (new_state, _effects) = reduce(state, event);
        state = new_state;

        if let Some(msg) = api_end_msg {
            produced_useful_work = true;
            let _ = out_tx
                .send(SubagentEvent::Message {
                    agent_id,
                    message: serde_json::to_value(&msg).unwrap_or(serde_json::Value::Null),
                })
                .await;
        }

        if state.is_terminal() {
            // Inspect the terminal reason; the M1 reducer puts it on
            // `Terminated { reason, .. }`. Killed prefixes per Task 1 step 3.
            if let ConversationState::Terminated { reason, .. } = &state {
                if reason.starts_with("user_exit")
                    || reason.starts_with("user_interrupt")
                    || reason.starts_with("killed")
                {
                    let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
                } else {
                    let _ = out_tx
                        .send(SubagentEvent::Completed {
                            agent_id,
                            result: serde_json::json!({ "reason": reason }),
                        })
                        .await;
                }
            }
            return;
        }
    }

    // event_rx closed before reaching Terminated. If we already emitted a
    // Message (the Task 2 happy path), it's a graceful end — emit Completed
    // with a synthetic reason. Otherwise (no useful work done), Task 4 will
    // refine this to Failed.
    if produced_useful_work {
        let _ = out_tx
            .send(SubagentEvent::Completed {
                agent_id,
                result: serde_json::json!({ "reason": "eof_graceful" }),
            })
            .await;
    } else {
        let _ = out_tx
            .send(SubagentEvent::Failed {
                agent_id,
                error: "run_subagent: event channel closed without terminal state".into(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
    };
    use crate::display::{AgentColor, AgentDisplay};
    use lingxi_core::token::Usage;
    use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId, RequestId};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// Build a `SubagentContext` with the minimum fields the runner reads.
    fn fresh_subagent_ctx() -> SubagentContext {
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_definition: AgentDefinition {
                agent_type: "test".into(),
                when_to_use: String::new(),
                tools: AgentToolPolicy::All {
                    use_exact_tools: true,
                },
                max_turns: 1,
                model: AgentModel::Inherit,
                permission_mode: AgentPermissionMode::Bubble,
                source: AgentSource::BuiltIn,
                base_dir: "/tmp".into(),
                system_prompt: None,
                mcp_servers: vec![],
                frontmatter_hooks: vec![],
                icon: None,
                allowed_tools: vec![],
                worktree_requirement: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            can_show_permission_prompts: true,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt: Some(Arc::from("")),
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon: None,
            },
        }
    }

    /// Drain the SubagentEvent receiver into a Vec.
    async fn drain(mut rx: mpsc::Receiver<SubagentEvent>) -> Vec<SubagentEvent> {
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    /// Build a minimal assistant message with a single text block.
    fn assistant_text(text: &str) -> ConversationMessage {
        ConversationMessage::Assistant {
            id: MessageId::new(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some("end_turn".into()),
        }
    }

    #[tokio::test]
    async fn run_subagent_emits_message_on_api_stream_end_then_completed() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        // Drive the runner from a request-response cycle the M1 reducer
        // accepts: UserMessage -> ApiStreamStart -> ApiStreamEnd -> EOF.
        // The runner is expected to surface the assistant Message on
        // ApiStreamEnd and finally Completed (Killed only on user_exit-prefixed
        // terminal — see Task 1 step 3 notes).
        let req = RequestId::new();
        let msg_id = MessageId::new();
        let final_msg = assistant_text("hello world");

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        event_tx
            .send(lingxi_core::Event::UserMessage {
                message_id: msg_id,
                request_id: req,
                content: "hi".into(),
            })
            .await
            .unwrap();
        event_tx
            .send(lingxi_core::Event::ApiStreamStart { request_id: req })
            .await
            .unwrap();
        event_tx
            .send(lingxi_core::Event::ApiStreamEnd {
                request_id: req,
                final_message: final_msg.clone(),
                usage: Usage::default(),
            })
            .await
            .unwrap();
        // Close the event channel — the runner should NOT treat clean close
        // as a failure when it has already produced a Message; instead it
        // emits a Completed terminal (graceful end on EOF).
        drop(event_tx);

        handle.await.unwrap();
        let evs = drain(out_rx).await;

        // At least one Message and exactly one terminal Completed.
        let message_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Message { .. }))
            .count();
        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();
        let failed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Failed { .. }))
            .count();

        assert_eq!(
            message_count, 1,
            "exactly one Message emitted on ApiStreamEnd; got events: {evs:?}"
        );
        assert_eq!(
            completed_count, 1,
            "exactly one Completed terminal; got events: {evs:?}"
        );
        assert_eq!(
            failed_count, 0,
            "no Failed events expected; got events: {evs:?}"
        );

        // The Message payload must be JSON-equivalent to the serialized final_message.
        let msg_payload = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Message {
                    agent_id: aid,
                    message,
                } => Some((*aid, message.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            msg_payload.0, agent_id,
            "Message agent_id matches ctx.agent_id"
        );
        let expected = serde_json::to_value(&final_msg).unwrap();
        assert_eq!(
            msg_payload.1, expected,
            "Message payload byte-equals serialized final_message"
        );
    }
}
