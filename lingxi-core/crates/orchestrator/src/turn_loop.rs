//! Inner turn-by-turn loop helpers. Private to `ConversationOrchestrator`.

use crate::conversation::ConversationOrchestrator;
use crate::error::OrchestratorError;
use crate::test_support::PermissionDecision;
use lingxi_api_client::types::ContentBlockApi;
use lingxi_core::SessionState;
use lingxi_protocol::{ContentBlock, ConversationMessage, MessageId, ToolUseId};
use lingxi_tools::context::{ToolUseContext, ToolUseOptions};
use lingxi_traits::CostSnapshot;

/// What one turn step decided.
pub(crate) enum TurnStepOutcome {
    /// Continue the loop (e.g. model returned `tool_use`).
    Continue,
    /// Loop should terminate — model returned `end_turn`.
    Ended {
        final_message_id: MessageId,
        stop_reason: String,
    },
}

/// Execute one `messages_create_non_stream` round-trip + tool dispatches.
///
/// `system` is the assembled system prompt for the conversation (built
/// once by `ConversationOrchestrator::try_run_turn`). It is passed
/// through to every API round-trip in the conversation, NOT re-built
/// per turn-step — the prompt is stable across the conversation lifetime
/// (see M5-03 plan "Out of scope" note: SSE M5-04 will not re-assemble
/// per turn-step either).
pub(crate) async fn execute_one_turn(
    orch: &ConversationOrchestrator,
    system: Option<&str>,
) -> Result<TurnStepOutcome, OrchestratorError> {
    // Snapshot the current session history for the API call.
    let (history_snapshot, model) = {
        let s = orch.session.lock().await;
        (s.history.clone(), s.model.clone())
    };

    // 1. Call the API.
    let response = orch
        .api
        .messages_create(&model, system, history_snapshot)
        .await?;

    // 2. Translate `MessageResponse.content` -> `ContentBlock` history entry.
    let assistant_blocks = translate_response_blocks(&response.content);

    // 3. Append the assistant message to the session. We need the
    //    `final_message_id` to return to the caller.
    let assistant_id = MessageId::new();
    {
        let mut s = orch.session.lock().await;
        s.history.push(ConversationMessage::Assistant {
            id: assistant_id,
            content: assistant_blocks.clone(),
            stop_reason: response.stop_reason.clone(),
        });
    }

    // 4. Emit each Text block to the output stream (whole-body in M5-02;
    //    M5-04 will switch to per-delta).
    for blk in &assistant_blocks {
        if let ContentBlock::Text { text } = blk {
            orch.output.emit_text(text).await;
        }
    }

    // 5. If there are tool_use blocks, dispatch them and feed results back.
    let tool_uses: Vec<(ToolUseId, String, serde_json::Value)> = assistant_blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((*id, name.clone(), input.clone())),
            _ => None,
        })
        .collect();

    if !tool_uses.is_empty() {
        let tool_results = dispatch_tool_uses(orch, &tool_uses).await?;
        // Append a fresh user message carrying the tool results.
        let user_id = MessageId::new();
        {
            let mut s = orch.session.lock().await;
            s.history.push(ConversationMessage::User {
                id: user_id,
                content: tool_results,
            });
        }
    }

    // 6. Decide loop disposition.
    match response.stop_reason.as_deref() {
        Some("end_turn") => Ok(TurnStepOutcome::Ended {
            final_message_id: assistant_id,
            stop_reason: "end_turn".to_string(),
        }),
        _ => Ok(TurnStepOutcome::Continue),
    }
}

/// Translate api-client content blocks into protocol content blocks.
/// Server-side variants (`ServerToolUse`, `ConnectorText`, `AdvisorToolResult`)
/// are dropped in M5-02. M5-04 may revisit Thinking.
fn translate_response_blocks(content: &[ContentBlockApi]) -> Vec<ContentBlock> {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlockApi::Text { text } => Some(ContentBlock::Text { text: text.clone() }),
            ContentBlockApi::ToolUse { id, name, input } => Some(ContentBlock::ToolUse {
                id: *id,
                name: name.clone(),
                input: input.clone(),
            }),
            ContentBlockApi::Thinking {
                thinking,
                signature,
            } => Some(ContentBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            }),
            // Server-side variants are skipped in M5-02; M5-04 may revisit.
            ContentBlockApi::ServerToolUse { .. }
            | ContentBlockApi::ConnectorText { .. }
            | ContentBlockApi::AdvisorToolResult { .. } => None,
        })
        .collect()
}

/// Dispatch each `tool_use` block through hooks -> permission -> registry ->
/// hooks. Returns a list of `ContentBlock::ToolResult` blocks for the
/// next user message.
#[allow(clippy::too_many_lines)]
async fn dispatch_tool_uses(
    orch: &ConversationOrchestrator,
    tool_uses: &[(ToolUseId, String, serde_json::Value)],
) -> Result<Vec<ContentBlock>, OrchestratorError> {
    let mut results = Vec::with_capacity(tool_uses.len());
    for (tool_use_id, name, input) in tool_uses {
        orch.output.emit_tool_call(name, input).await;

        // Pre-tool hook.
        if let Err(reason) = orch.hooks.pre_tool_use(name, input).await {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: format!("Hook blocked: {reason}"),
                is_error: true,
            };
            orch.output
                .emit_tool_result(
                    name,
                    &serde_json::json!({ "error": format!("Hook blocked: {reason}") }),
                )
                .await;
            results.push(result_block);
            continue;
        }

        // Permission gate.
        match orch.perms.check(name, input).await {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                let result_block = ContentBlock::ToolResult {
                    tool_use_id: *tool_use_id,
                    content: format!("Permission denied: {reason}"),
                    is_error: true,
                };
                orch.output
                    .emit_tool_result(
                        name,
                        &serde_json::json!({ "error": format!("Permission denied: {reason}") }),
                    )
                    .await;
                results.push(result_block);
                continue;
            }
        }

        // Dispatch through ToolRegistry.
        let Some(tool_handle) = orch.tools.find_by_name(name) else {
            let result_block = ContentBlock::ToolResult {
                tool_use_id: *tool_use_id,
                content: format!("Error: tool not found: {name}"),
                is_error: true,
            };
            orch.output
                .emit_tool_result(
                    name,
                    &serde_json::json!({ "error": format!("tool not found: {name}") }),
                )
                .await;
            results.push(result_block);
            continue;
        };

        // Synthesize a minimal ToolUseContext.
        let messages = {
            let s = orch.session.lock().await;
            s.history.clone()
        };
        let ctx = ToolUseContext {
            options: ToolUseOptions {
                debug: false,
                verbose: false,
                main_loop_model: orch.config.model.clone(),
                max_budget_nano_usd: None,
                mcp_clients: Vec::new(),
                is_non_interactive_session: true,
                custom_system_prompt: orch.config.system_prompt_override.clone(),
                append_system_prompt: None,
            },
            messages,
            tool_use_id: Some(*tool_use_id),
            agent_id: None,
            content_replacement_state: None,
            session: Some(orch.session.clone()),
            subagent_registry: Some(orch.tools.clone()),
        };

        // One-shot progress channel — receiver dropped immediately.
        let (progress_tx, _progress_rx) =
            tokio::sync::mpsc::channel::<lingxi_tools::progress::ToolProgress>(8);

        let tool_outcome = tool_handle.call(input.clone(), ctx, progress_tx).await;

        let (content, is_error, emit_payload) = match tool_outcome {
            Ok(result) => {
                let text = serde_json::to_string(&result.data)
                    .unwrap_or_else(|_| "<unserializable>".into());
                (text, false, result.data)
            }
            Err(err) => {
                let text = format!("Error: {err}");
                (text, true, serde_json::json!({ "error": format!("{err}") }))
            }
        };

        orch.output.emit_tool_result(name, &emit_payload).await;

        // Post-tool hook (best-effort — failures are surfaced only via the
        // hook executor's own telemetry; the tool result is preserved).
        let _ = orch
            .hooks
            .post_tool_use(name, &emit_payload, is_error)
            .await;

        results.push(ContentBlock::ToolResult {
            tool_use_id: *tool_use_id,
            content,
            is_error,
        });
    }
    Ok(results)
}

/// Project a `SessionState` into a `CostSnapshot`. M5-02 reports zero cost;
/// M5-05/M5-11 will plug in `lingxi-cost`.
pub(crate) fn cost_snapshot_from_session(s: &SessionState) -> CostSnapshot {
    CostSnapshot {
        session_id: s.session_id,
        total_nano_usd: 0,
        total_tokens: s
            .usage
            .0
            .input_tokens
            .saturating_add(s.usage.0.output_tokens),
    }
}
