//! Subagent state-machine loop.
//!
//! [`run_subagent`] is the future that
//! [`crate::pool::StateMachinePool::allocate`] hands to the runtime.
//!
//! Two modes coexist, selected on [`SubagentContext::api_client`]:
//!
//! * **Real multi-turn loop** (`api_client = Some`): an imperative loop that
//!   mirrors the orchestrator's `execute_one_turn` — call the model, append
//!   the assistant turn, dispatch any `tool_use` blocks through the inherited
//!   [`traits::ToolInvoker`], feed the results back as a user message, and
//!   repeat until the model stops (`end_turn` / no tool use) or `max_turns`
//!   is hit. A `UserExit` / `UserInterrupt` arriving on `event_rx` aborts the
//!   loop and surfaces [`SubagentEvent::Killed`]. When
//!   [`crate::context::SubagentContext::persistent`] is set, the loop does not
//!   return on a terminal stop: it parks awaiting the next inbound
//!   [`engine::Event::UserMessage`], appends it to history, and runs the next
//!   turn-set — modelling a long-lived, message-driven teammate.
//! * **Legacy stub** (`api_client = None`): the M1.11 reducer-driven stub that
//!   completes after the first inbound event. Retained for back-compat with
//!   callers that haven't wired an API client yet.

use crate::context::SubagentContext;
use protocol::AgentId;
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
        /// Wire usage from the FINAL model response (the spawner translates this
        /// into `traits::SubagentUsage` + the result-level token total). The
        /// legacy stub path has no real round-trips and emits `Usage::default()`.
        usage: llm_client::Usage,
        /// Number of tool-use blocks executed across the run (claude
        /// `totalToolUseCount`). `0` on the stub path.
        total_tool_use_count: u64,
        /// Wall-clock duration of the run in milliseconds (claude
        /// `totalDurationMs`). `0` on the stub path.
        total_duration_ms: u64,
        /// Number of assistant messages produced across the run (claude
        /// `agentMessages.length`, fed into `tengu_agent_tool_completed`'s
        /// `assistant_message_count`). `0` on the stub path.
        assistant_message_count: u64,
        /// The FINAL assistant turn's provider request id (claude
        /// `lastAssistantMessage.requestId`) — used to gate
        /// `tengu_cache_eviction_hint`. `None` on the stub path.
        last_request_id: Option<String>,
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

/// Milliseconds elapsed since `start`, saturated into a `u64` (claude
/// `totalDurationMs`).
fn elapsed_ms(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Subagent state-machine loop.
///
/// When [`SubagentContext::api_client`] is `Some`, drives the real
/// multi-turn agentic loop (see [`run_subagent_loop`]). Otherwise falls back
/// to the legacy reducer-driven stub (see [`run_subagent_stub`]). Both emit
/// [`SubagentEvent`]s on `out_tx`.
pub async fn run_subagent(
    ctx: SubagentContext,
    event_rx: mpsc::Receiver<engine::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    // G4 (frontmatter hooks): register the agent definition's frontmatter hooks
    // scoped to this child `agent_id` BEFORE the run and clear them AFTER —
    // claude `registerFrontmatterHooks(…, isAgent=true)` (runAgent.ts:557-575)
    // then `clearSessionHooks(agentId)` in the `runAgent` finally. `isAgent=true`
    // retargets each `Stop` subscription to `SubagentStop` (a subagent's loop end
    // fires `SubagentStop`). Wrapped here at the dispatcher so the clear runs
    // regardless of how the body returns (the loop has many early returns), and
    // so an un-wired `hook_executor` (tests / minimal builds) is a strict no-op.
    let frontmatter_cleanup = match &ctx.hook_executor {
        Some(he) if !ctx.agent_definition.frontmatter_hooks.is_empty() => {
            he.register_agent_hooks(
                ctx.agent_id,
                &ctx.agent_definition.frontmatter_hooks,
                true,
            )
            .await;
            Some((he.clone(), ctx.agent_id))
        }
        _ => None,
    };

    // #9 (dead SubagentStop): when this agent registered frontmatter hooks, the
    // `Stop`→`SubagentStop` retargeted ones (registerFrontmatterHooks isAgent=true)
    // must actually FIRE at the loop's natural end — claude fires the subagent's
    // stop hooks INSIDE the child (`stopHooks.ts` via `query.ts`, keyed on
    // `toolUseContext.agentId`). The LingXi orchestrator-side `SubagentStop`
    // chokepoint runs AFTER this dispatcher returns (i.e. after `clear_agent_hooks`
    // below), so those retargeted frontmatter hooks would be dead code without an
    // in-child fire. We fire SubagentStop here, AGENT-SCOPED to this child's own
    // bucket (see `execute_agent_scoped`), so session / plugin `SubagentStop`
    // hooks are NOT double-fired — the chokepoint already covers those.
    //
    // To carry a faithful run status we proxy `out_tx`: forward every
    // `SubagentEvent` to the real channel while remembering the terminal one, so
    // the status maps to claude's outcome (`completed` / `failed` — `Killed` does
    // not fire SubagentStop, matching claude where an aborted child's loop does
    // not reach `stopHooks`). When there are NO frontmatter hooks (the common
    // case) we skip the proxy entirely and pass `out_tx` straight through, so the
    // hot path is byte-identical to legacy.
    let agent_scoped_stop = frontmatter_cleanup.as_ref().map(|(he, agent_id)| {
        // FIX 2: the agent-scoped SubagentStop carries `agent_transcript_path`
        // (claude-code `getAgentTranscriptPath(subagentId)`, coreSchemas.ts:556 /
        // utils/hooks.ts:3676). TS builds it as
        // `…/subagents[/subdir]/agent-${agentId}.jsonl`; LingXi mirrors the
        // `agent-<id>.jsonl` leaf under this child's `transcript_subdir`. NOTE:
        // the production spawn path currently seeds `transcript_subdir` to a
        // `/tmp` placeholder (handle.rs / pool.rs), so the path is shape-faithful
        // but not yet the real session-scoped location — same cosmetic caveat as
        // `hook_session_id`. Filling it is strictly better than the prior empty
        // value, which serialized as a bare default.
        let agent_transcript_path = ctx
            .transcript_subdir
            .join(format!("agent-{agent_id}.jsonl"));
        (
            he.clone(),
            *agent_id,
            ctx.agent_definition.agent_type.clone(),
            ctx.hook_session_id,
            ctx.hook_cwd.clone(),
            agent_transcript_path,
        )
    });

    let terminal_status = if agent_scoped_stop.is_some() {
        // Proxy: forward events, capture the terminal disposition.
        let (proxy_tx, mut proxy_rx) = mpsc::channel::<SubagentEvent>(16);
        let forwarder = {
            let real = out_tx.clone();
            tokio::spawn(async move {
                let mut status: Option<&'static str> = None;
                while let Some(ev) = proxy_rx.recv().await {
                    status = match &ev {
                        SubagentEvent::Completed { .. } => Some("completed"),
                        SubagentEvent::Failed { .. } => Some("failed"),
                        // Killed does not fire SubagentStop (claude: an aborted
                        // child throws before reaching its stop hooks).
                        SubagentEvent::Killed { .. } => None,
                        // Non-terminal: keep whatever terminal we last saw.
                        _ => status,
                    };
                    // Best-effort forward; a closed receiver drops the rest.
                    if real.send(ev).await.is_err() {
                        break;
                    }
                }
                status
            })
        };
        // Run the body against the proxy, then drop our proxy sender so the
        // forwarder's `recv()` loop ends and we can read the captured status.
        if ctx.api_client.is_some() {
            run_subagent_loop(ctx, event_rx, proxy_tx).await;
        } else {
            run_subagent_stub(ctx, event_rx, proxy_tx).await;
        }
        forwarder.await.unwrap_or(None)
    } else {
        // No frontmatter hooks: straight passthrough, no proxy overhead.
        if ctx.api_client.is_some() {
            run_subagent_loop(ctx, event_rx, out_tx).await;
        } else {
            run_subagent_stub(ctx, event_rx, out_tx).await;
        }
        None
    };

    // Fire the agent-scoped SubagentStop BEFORE clearing the frontmatter hooks
    // (otherwise the retargeted Stop→SubagentStop hooks are already gone). Only
    // when the child reached a terminal that fires SubagentStop in claude
    // (`completed` / `failed`).
    if let (Some((he, agent_id, agent_type, session_id, cwd, agent_transcript_path)), Some(status)) =
        (agent_scoped_stop, terminal_status)
    {
        let stop_ctx = hooks::registry::HookContext {
            session_id,
            agent_id: Some(agent_id),
            cwd,
            agent_type: Some(agent_type),
            // FIX 2: SubagentStop carries the agent's own transcript path
            // (claude-code `agent_transcript_path`). See the tuple build above.
            agent_transcript_path: Some(agent_transcript_path),
            ..Default::default()
        };
        he.execute_agent_scoped(
            hooks::events::HookEvent::SubagentStop {
                agent_id,
                status: status.to_string(),
            },
            stop_ctx,
            agent_id,
        )
        .await;
    }

    if let Some((he, agent_id)) = frontmatter_cleanup {
        he.clear_agent_hooks(agent_id).await;
    }
}

/// Resolve the wire model string from the agent definition.
///
/// This forwards the definition's model string verbatim. For the production
/// spawn path the model is ALREADY resolved to a concrete wire id at spawn time
/// by [`crate::model_resolution::resolve_agent_model`] (`Inherit` → parent /
/// main-loop model, bare family alias → concrete `claude-*` id), so the
/// definition here carries an `Explicit(...)` id and `resolve_model` simply
/// forwards it. Only when the spawner has no `default_model` wired (legacy /
/// tests) does this forward a raw `"inherit"` / bare alias — which then resolves
/// solely via any configured `routing.aliases`.
fn resolve_model(ctx: &SubagentContext) -> String {
    match &ctx.agent_definition.model {
        crate::definition::AgentModel::Inherit => "inherit".to_string(),
        crate::definition::AgentModel::Alias(n) | crate::definition::AgentModel::Explicit(n) => {
            n.clone()
        }
    }
}

/// Extract the text blocks the final agent response surfaces, with claude's
/// backward-scan fallback.
///
/// Port of claude-code `finalizeAgentTool` (agentToolUtils.ts:304-317): take the
/// text blocks from the LAST assistant message; if it carried none (the loop
/// exited mid-turn on a pure `tool_use` turn), fall back to the most recent
/// assistant message in `history` that DOES have text blocks. Returns the raw
/// text strings (one per surviving text block) in source order — the caller maps
/// them into claude's `content: [{type:'text', text}]` array.
fn final_text_blocks(
    history: &[protocol::ConversationMessage],
    final_assistant_blocks: &[protocol::ContentBlock],
) -> Vec<String> {
    let texts_of = |blocks: &[protocol::ContentBlock]| -> Vec<String> {
        blocks
            .iter()
            .filter_map(|b| match b {
                protocol::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
    };
    // 1. Text from the final assistant message.
    let primary = texts_of(final_assistant_blocks);
    if !primary.is_empty() {
        return primary;
    }
    // 2. Backward scan: most recent assistant message WITH text.
    for msg in history.iter().rev() {
        if let protocol::ConversationMessage::Assistant { content, .. } = msg {
            let t = texts_of(content);
            if !t.is_empty() {
                return t;
            }
        }
    }
    Vec::new()
}

/// Build the terminal `Completed.result` JSON for a clean stop.
///
/// Carries claude's `content` array (`[{type:'text', text}]`, agentToolUtils.ts
/// `finalizeAgentTool` return) computed via the backward-scan
/// [`final_text_blocks`], plus the legacy `text`/`stop_reason` keys existing
/// consumers (and the runner's own tests) read. `AgentTool` reads `content` to
/// build claude's structured result + model-facing trailer; the joined `text`
/// stays for back-compat (`tasks::handlers::local_agent` / `dream`).
fn build_completed_result(
    history: &[protocol::ConversationMessage],
    final_assistant_blocks: &[protocol::ContentBlock],
    stop_reason: Option<&str>,
) -> serde_json::Value {
    let blocks = final_text_blocks(history, final_assistant_blocks);
    let content: Vec<serde_json::Value> = blocks
        .iter()
        .map(|t| serde_json::json!({ "type": "text", "text": t }))
        .collect();
    serde_json::json!({
        "content": content,
        "text": blocks.join("\n"),
        "stop_reason": stop_reason,
    })
}

/// Byte-locked `formatSkillLoadingMetadata(skillName)` port
/// (claude `processSlashCommand.tsx:786`): the leading text block of a preloaded
/// skill's meta user message. claude ignores the `progressMessage` arg
/// (`_progressMessage` is unused), so this renders only the (resolved) name:
/// `<command-message>{name}</command-message>\n<command-name>{name}</command-name>\n<skill-format>true</skill-format>`.
fn format_skill_loading_metadata(skill_name: &str) -> String {
    format!(
        "<command-message>{skill_name}</command-message>\n\
<command-name>{skill_name}</command-name>\n\
<skill-format>true</skill-format>"
    )
}

/// Build the G4 (SubagentStart additionalContext) + G5 (skills) messages claude
/// `runAgent` prepends to a child's INITIAL messages before the query loop, in
/// claude's order: additionalContext (runAgent.ts:530-555) → skills
/// (runAgent.ts:577-646). (Frontmatter-hook registration — runAgent.ts:557-575,
/// ordered between them — is handled at the [`run_subagent`] dispatcher so the
/// clear is guaranteed; its registration is side-effecting, not message-producing,
/// so its position relative to these two message-producing steps is unobservable
/// in the child history.)
///
/// Returns the extra [`ConversationMessage`]s to append after the prompt seed.
/// A `None` `hook_executor` / `skill_loader` (tests / minimal builds) makes the
/// respective step a strict no-op, so the child history stays byte-identical to
/// legacy.
async fn build_preload_messages(ctx: &SubagentContext) -> Vec<protocol::ConversationMessage> {
    use protocol::{ContentBlock, ConversationMessage, MessageId};

    let agent_type = ctx.agent_definition.agent_type.clone();
    let mut out: Vec<ConversationMessage> = Vec::new();

    // --- G4: SubagentStart hooks → additionalContext injection --------------
    // claude fires `executeSubagentStartHooks(agentId, agentType, signal)`,
    // collects every hook's `additionalContexts` into ONE `string[]`, and pushes
    // a SINGLE `hook_additional_context` user message into `initialMessages`
    // (runAgent.ts:530-555). That attachment renders (messages.ts:4117-4128 via
    // `wrapInSystemReminder`) as ONE `<system-reminder>` message:
    //   `<system-reminder>\nSubagentStart hook additional context: ` +
    //   contexts.join("\n") + `\n</system-reminder>`
    // An empty collection produces NO message (messages.ts:4118 early return).
    // We match those bytes exactly: one message, the `SubagentStart hook
    // additional context: ` prefix, the `\n`-join of all contexts.
    if let Some(hooks) = &ctx.hook_executor {
        let hook_ctx = hooks::registry::HookContext {
            session_id: ctx.hook_session_id,
            agent_id: Some(ctx.agent_id),
            cwd: ctx.hook_cwd.clone(),
            agent_type: Some(agent_type.clone()),
            ..Default::default()
        };
        let agg = hooks
            .execute(
                hooks::events::HookEvent::SubagentStart {
                    agent_id: ctx.agent_id,
                    agent_type: agent_type.clone(),
                    parent_agent_id: ctx.parent_agent_id,
                },
                hook_ctx,
            )
            .await;
        if !agg.additional_contexts.is_empty() {
            let joined = agg.additional_contexts.join("\n");
            out.push(ConversationMessage::user(
                MessageId::new(),
                format!(
                    "<system-reminder>\nSubagentStart hook additional context: {joined}\n</system-reminder>"
                ),
            ));
        }
    }

    // --- G5: skills preload -------------------------------------------------
    // claude resolves+loads each frontmatter skill and pushes a `isMeta` user
    // message whose first block is `formatSkillLoadingMetadata(skillName,
    // skill.progressMessage)` followed by the loaded content blocks
    // (runAgent.ts:577-646). A missing / non-prompt skill logs claude's exact
    // warn and is skipped.
    if let Some(loader) = &ctx.skill_loader {
        for skill_name in &ctx.agent_definition.skills {
            match loader.resolve_and_load(skill_name, &agent_type).await {
                None => {
                    // claude runAgent.ts:600 — exact warn string.
                    tracing::warn!(
                        "[Agent: {agent_type}] Warning: Skill '{skill_name}' specified in frontmatter was not found"
                    );
                }
                Some(load) => {
                    tracing::debug!("[Agent: {agent_type}] Preloaded skill '{skill_name}'");
                    // Leading metadata text block + the loaded content blocks
                    // (claude `createUserMessage({ content: [metadata, ...content],
                    // isMeta: true })`). LingXi's `ConversationMessage::User` has
                    // no `isMeta` flag (the meta-ness is a UI concern claude uses
                    // for rendering "Skill(name)"); the model-facing bytes — the
                    // metadata block then the skill content — are what matter for
                    // parity, and those are preserved here.
                    let mut blocks: Vec<ContentBlock> =
                        Vec::with_capacity(1 + load.content.len());
                    blocks.push(ContentBlock::Text {
                        text: format_skill_loading_metadata(&load.display_name),
                    });
                    blocks.extend(load.content);
                    out.push(ConversationMessage::User {
                        id: MessageId::new(),
                        content: blocks,
                        is_meta: false,
                    });
                }
            }
        }
    }

    out
}

/// Translate llm-client content blocks into protocol content blocks.
///
/// Mirrors the orchestrator's `translate_response_blocks`: `Text` /
/// `ToolCall` / `Reasoning` map through; server-side and other variants
/// are dropped.
fn translate_response_blocks(content: &[llm_client::ContentBlock]) -> Vec<protocol::ContentBlock> {
    content
        .iter()
        .filter_map(|b| match b {
            llm_client::ContentBlock::Text { text, .. } => {
                Some(protocol::ContentBlock::Text { text: text.clone() })
            }
            llm_client::ContentBlock::ToolCall { id, name, input } => {
                Some(protocol::ContentBlock::ToolUse {
                    // The provider-issued id IS the canonical ToolUseId (byte
                    // parity with claude-code); the provider_id sidecar stays None.
                    id: protocol::ToolUseId::from(id.clone()),
                    name: name.clone(),
                    input: input.clone(),
                    provider_id: None,
                })
            }
            llm_client::ContentBlock::Reasoning { text, signature } => {
                Some(protocol::ContentBlock::Thinking {
                    thinking: text.clone(),
                    signature: signature.clone(),
                })
            }
            // Low-frequency server-side blocks: PRESERVED verbatim for resume/replay
            // byte parity (matches orchestrator::turn_loop::translate_response_blocks).
            llm_client::ContentBlock::RedactedThinking { data } => {
                Some(protocol::ContentBlock::RedactedThinking { data: data.clone() })
            }
            llm_client::ContentBlock::ServerToolUse { id, name, input } => {
                Some(protocol::ContentBlock::ServerToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                })
            }
            llm_client::ContentBlock::ConnectorText {
                connector_text,
                signature,
            } => Some(protocol::ContentBlock::ConnectorText {
                connector_text: connector_text.clone(),
                signature: signature.clone(),
            }),
            llm_client::ContentBlock::AdvisorToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(protocol::ContentBlock::AdvisorToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            }),
            // Input-only / non-output variants remain dropped on the response path.
            llm_client::ContentBlock::Image { .. }
            | llm_client::ContentBlock::ImageUrl { .. }
            | llm_client::ContentBlock::Document { .. }
            | llm_client::ContentBlock::ToolResult { .. }
            // cache_edits is a request-only directive — never in a response.
            | llm_client::ContentBlock::CacheEdits { .. } => None,
        })
        .collect()
}

/// Emit `msg` as a [`SubagentEvent::Message`] on `out_tx`.
async fn emit_message(
    out_tx: &mpsc::Sender<SubagentEvent>,
    agent_id: AgentId,
    msg: &protocol::ConversationMessage,
) {
    let _ = out_tx
        .send(SubagentEvent::Message {
            agent_id,
            message: serde_json::to_value(msg).unwrap_or(serde_json::Value::Null),
        })
        .await;
}

/// Real multi-turn agentic loop.
///
/// Imperative — mirrors `orchestrator::turn_loop::execute_one_turn`: call the
/// model, append the assistant turn, dispatch `tool_use` blocks through the
/// inherited [`traits::ToolInvoker`], feed results back as a user message,
/// and repeat. Each model round-trip goes over the streaming seam
/// ([`crate::api::SubagentApiClient::messages_create_stream`] drained through
/// `crate::accumulator::accumulate_stream`) and races a `UserExit` /
/// `UserInterrupt` on `event_rx` via [`tokio::select!`]; a termination event
/// aborts the loop and surfaces [`SubagentEvent::Killed`].
#[allow(
    clippy::too_many_lines,
    reason = "imperative multi-turn agentic loop — splitting the turn body hurts readability"
)]
async fn run_subagent_loop(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<engine::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    use protocol::{ContentBlock, ConversationMessage, MessageId};

    let agent_id = ctx.agent_id;
    let api_client = ctx
        .api_client
        .clone()
        .expect("run_subagent_loop requires an api_client");
    // Inherited budget enforcer (cloned Option<Arc> — cheap refcount bump).
    // `Some` consults the parent's cumulative cost once per turn; `None`
    // disables enforcement (legacy/test contexts).
    let budget = ctx.budget.clone();
    let model = resolve_model(&ctx);
    let system: Option<String> = ctx.rendered_system_prompt.as_ref().map(std::string::ToString::to_string);
    // Wire tool definitions advertised to the model on every round-trip (empty
    // when the spawner wired none). Cloned per round-trip below.
    let tool_schemas = ctx.tool_schemas.clone();
    // Per-agent tool allow-list enforced at dispatch (see below). Empty = no
    // restriction (the resolver has not filtered, e.g. `AgentToolPolicy::All`).
    // This is the dispatch-time guard the advertised set relies on: the
    // inherited `RegistryToolInvoker` itself does NOT check policy.
    let allowed_tools = ctx.allowed_tools.clone();

    // Seed history: fork-context prefix (if any) followed by the prompt.
    let mut history: Vec<ConversationMessage> = Vec::new();
    if let Some(fork) = &ctx.fork_context_messages {
        history.extend(fork.iter().cloned());
    }
    history.extend(ctx.prompt_messages.iter().cloned());

    // G4 + G5 (claude runAgent.ts:530-646): SubagentStart-hook additionalContext
    // injection then frontmatter skills preload, appended to the child's INITIAL
    // messages before the first turn. Strict no-op (no extra messages) when
    // neither `hook_executor` nor `skill_loader` is wired, so legacy / test
    // builds keep a byte-identical history. (Frontmatter-hook registration is
    // done at the `run_subagent` dispatcher for guaranteed cleanup.)
    history.extend(build_preload_messages(&ctx).await);

    let max_turns = ctx.agent_definition.max_turns;

    // Once the cancellation channel closes, no UserExit / UserInterrupt can
    // ever arrive, so we stop racing it and await the API future directly
    // (racing a perpetually-ready `recv() -> None` arm would busy-loop).
    let mut event_channel_open = true;

    // Result-level rollups carried onto the terminal `Completed` event so the
    // spawner can populate claude's `totalDurationMs` / `totalToolUseCount`
    // without re-deriving them. `run_start` spans the whole run (every turn-set
    // in persistent mode); `total_tool_use_count` accumulates `tool_uses.len()`
    // across turns. `last_usage` keeps the FINAL response usage (claude
    // `getTokenCountFromUsage` reads the LAST assistant usage, not a sum), so it
    // is overwritten — never accumulated — each turn.
    let run_start = std::time::Instant::now();
    let mut total_tool_use_count: u64 = 0;
    let mut last_usage = llm_client::Usage::default();
    // claude `agentMessages.length` — assistant turns produced across the run
    // (one per round-trip) — and the FINAL turn's provider request id (claude
    // `lastAssistantMessage.requestId`), both surfaced on the terminal
    // `Completed` event so the spawner can emit `tengu_agent_tool_completed` /
    // `tengu_cache_eviction_hint`.
    let mut assistant_message_count: u64 = 0;
    let mut last_request_id: Option<String> = None;

    // Outer loop: one iteration per turn-set. In non-persistent mode the
    // turn-set runs exactly once (we `return` after it). In persistent mode the
    // runner parks at the bottom awaiting the next inbound `UserMessage` and
    // loops back here to run the next turn-set, retaining `history` across
    // turn-sets (matching the TS teammate's accumulated transcript). `max_turns`
    // is per-turn-set: the `_turn` counter re-zeroes each outer iteration, so
    // every injected message gets a fresh budget.
    loop {
    // Set to `true` when the inner turn loop hits a clean terminal stop (it has
    // already emitted its `Completed`). Stays `false` if the loop instead falls
    // through by exhausting `max_turns`, which needs the max-turns `Completed`.
    let mut terminated_cleanly = false;
    for _turn in 0..max_turns {
        // Per-turn budget gate. This is the achievable analog of
        // `QueryEngine.ts`'s `error_max_budget_usd` loop-terminator, built on
        // the same frozen seam `AgentTool`'s pre-spawn gate uses
        // (tools/agent/src/agent.rs:321): charge 0 to ask "is cumulative cost
        // already over the configured limit?" without pricing tokens (the
        // agent crate can't depend on lingxi-cost; the enforcer tracks cost
        // globally, exactly like TS reading `getTotalCost()`).
        //
        // It intentionally diverges from QueryEngine in two ways, both forced
        // by the frozen `BudgetEnforcerHandle` surface: (1) PLACEMENT — checked
        // at the TOP of the turn (stop before spending) rather than TS's
        // post-message check, so an already-over budget makes zero round-trips;
        // (2) STRING — the denial reports the *current* cost (`format_budget_denied`
        // byte-for-byte), not TS's "Reached maximum budget ($limit)", because the
        // handle exposes the cumulative total but never the configured limit.
        if let Some(b) = &budget {
            if let Err(traits::budget::BudgetError::Exceeded { current_nano_usd }) =
                b.check_and_charge(0).await
            {
                // Stop with a budget-exhausted terminal carrying the M3-05
                // byte-locked denial string (matches `format_budget_denied`:
                // nano_usd / 1e9, `{:.2}`). Reproduced inline because the agent
                // crate cannot depend on lingxi-tools / lingxi-cost.
                #[allow(clippy::cast_precision_loss)]
                let dollars = current_nano_usd as f64 / 1_000_000_000.0;
                let _ = out_tx
                    .send(SubagentEvent::Failed {
                        agent_id,
                        error: format!("Budget exceeded (${dollars:.2}); stopped."),
                    })
                    .await;
                return;
            }
            // `Ok` and `BudgetError::Internal` fall through to the round-trip:
            // TS has no analog branch that errors the loop on an internal
            // budget condition, so an internal failure is non-fatal here.
        }

        // Race the model round-trip against a user-termination event. A
        // UserExit / UserInterrupt on event_rx aborts the loop -> Killed.
        // Any other inbound event is ignored (the loop is self-driving) and
        // we re-issue the round-trip on the next iteration.
        //
        // The round-trip goes over the STREAMING seam: open the SSE stream and
        // drain it through `accumulate_stream` into the same `MessageResponse`
        // the non-streaming path produced (the default `messages_create_stream`
        // wraps `messages_create` losslessly, so a non-streaming client behaves
        // identically). Dropping this future on the termination arm cancels the
        // in-flight stream, exactly as dropping a non-streaming call would.
        let response = loop {
            let api_call = async {
                let stream = api_client
                    .messages_create_stream(
                        &model,
                        system.as_deref(),
                        history.clone(),
                        tool_schemas.clone(),
                    )
                    .await?;
                crate::accumulator::accumulate_stream(stream).await
            };
            if !event_channel_open {
                break api_call.await;
            }
            tokio::select! {
                biased;
                ev = event_rx.recv() => {
                    match ev {
                        Some(engine::Event::UserExit | engine::Event::UserInterrupt) => {
                            let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
                            return;
                        }
                        // Non-termination event: drop the in-flight API future
                        // and retry the round-trip on the next iteration.
                        Some(_) => continue,
                        // Channel closed: stop racing it from now on.
                        None => {
                            event_channel_open = false;
                            continue;
                        }
                    }
                }
                resp = api_call => break resp,
            }
        };

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                let _ = out_tx
                    .send(SubagentEvent::Failed {
                        agent_id,
                        error: format!("subagent api error: {e}"),
                    })
                    .await;
                return;
            }
        };

        // Keep the FINAL response usage for the terminal `Completed` rollup
        // (claude `getTokenCountFromUsage` reads the LAST assistant usage — so
        // overwrite, never accumulate, to stay byte-faithful).
        last_usage = response.usage.clone();
        // Track the assistant-message count (claude `agentMessages.length`) and
        // the FINAL turn's provider request id (claude
        // `lastAssistantMessage.requestId`). One assistant turn per round-trip;
        // `response.id` is the provider response id (the `requestId` analog).
        assistant_message_count = assistant_message_count.saturating_add(1);
        last_request_id = if response.id.is_empty() {
            None
        } else {
            Some(response.id.clone())
        };

        // Build the assistant turn and append to history.
        let assistant_blocks = translate_response_blocks(&response.content);
        let stop_reason = response.stop_reason.clone();
        let assistant_msg = ConversationMessage::Assistant {
            id: MessageId::new(),
            content: assistant_blocks.clone(),
            stop_reason: stop_reason.clone(),
        };
        history.push(assistant_msg.clone());
        emit_message(&out_tx, agent_id, &assistant_msg).await;

        // Extract tool_use blocks.
        let tool_uses: Vec<(protocol::ToolUseId, String, serde_json::Value, Option<String>)> =
            assistant_blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input,
                        provider_id,
                    } => Some((id.clone(), name.clone(), input.clone(), provider_id.clone())),
                    _ => None,
                })
                .collect();

        // Accumulate the run-wide tool-use count (claude `totalToolUseCount`).
        total_tool_use_count =
            total_tool_use_count.saturating_add(tool_uses.len() as u64);

        // Dispatch any tool_use blocks FIRST, then decide loop disposition by
        // stop_reason — mirroring the orchestrator references. `execute_one_turn`
        // (turn_loop.rs:111) always dispatches tool_uses when present regardless
        // of stop_reason, and the streaming path (conversation.rs:815-839) only
        // continues on `Some("tool_use")` with non-empty tool_uses, terminating
        // on end_turn / None / any other reason. Deciding terminality *before*
        // dispatch (the prior `tool_uses.is_empty() || end_turn` test) silently
        // dropped tool calls on an `end_turn`+tool_use response (MAJOR #1) and
        // looped to max_turns on a truncated/refused turn that still carried
        // tool_uses (MAJOR #2).
        if !tool_uses.is_empty() {
            // Dispatch each tool_use through the inherited invoker.
            let Some(invoker) = &ctx.tool_invoker else {
                let _ = out_tx
                    .send(SubagentEvent::Failed {
                        agent_id,
                        error: "subagent requested a tool but no tool_invoker was inherited"
                            .to_string(),
                    })
                    .await;
                return;
            };

            let mut tool_results: Vec<ContentBlock> = Vec::with_capacity(tool_uses.len());
            for (tool_use_id, name, input, provider_id) in &tool_uses {
                // Allow-list guard: when `allowed_tools` is non-empty, a model
                // request for a tool outside it is refused WITHOUT dispatching
                // (the inherited `RegistryToolInvoker` would otherwise run any
                // registered tool by name). Surfaced as an `is_error` ToolResult
                // so the model sees the refusal and can recover, mirroring how a
                // tool error is fed back. Empty `allowed_tools` skips the guard.
                if !allowed_tools.is_empty() && !allowed_tools.iter().any(|t| t == name) {
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: format!(
                            "tool {name:?} is not in this agent's allowed tools"
                        ),
                        is_error: true,
                        provider_tool_use_id: provider_id.clone(),
                    });
                    continue;
                }
                let inv_ctx = traits::tool_invoker::SubagentInvocationContext {
                    parent_agent_id: ctx.parent_agent_id,
                    // Swarm identity (claude-code `getAgentName()` /
                    // `getTeammateContext()?.teamName`): a teammate's dispatched
                    // tools see the teammate's DISPLAY name + team name so the
                    // swarm-only `TaskUpdate` side-effects key on them. `None`
                    // for one-shot subagents / the main thread.
                    agent_name: ctx.agent_name.clone(),
                    team_name: ctx.team_name.clone(),
                };
                match invoker.invoke(name, input.clone(), inv_ctx).await {
                    Ok(value) => {
                        let content = match &value {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content,
                            is_error: false,
                            provider_tool_use_id: provider_id.clone(),
                        });
                    }
                    Err(e) => {
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            // Match the main turn-loop convention
                            // (`turn_loop.rs` `"Error: {bare}"`): the bare
                            // model-facing message, NOT the `Display` form which
                            // would leak the LingXi-internal `ToolInvoker: …`
                            // prefix into the child's tool_result wire bytes.
                            content: format!("Error: {}", e.model_facing_message()),
                            is_error: true,
                            provider_tool_use_id: provider_id.clone(),
                        });
                    }
                }
            }

            let tool_results_msg = ConversationMessage::User {
                id: MessageId::new(),
                content: tool_results,
                is_meta: false,
            };
            history.push(tool_results_msg.clone());
            emit_message(&out_tx, agent_id, &tool_results_msg).await;
        }

        // Loop disposition. Continue ONLY when the model asked to use tools and
        // actually emitted some; every other case is terminal — including
        // `end_turn`, a stream with no stop_reason (`None`), and any other
        // reason (max_tokens / stop_sequence / pause_turn / refusal), even when
        // the truncated turn carried tool_uses we just dispatched.
        let should_continue =
            stop_reason.as_deref() == Some("tool_use") && !tool_uses.is_empty();
        if !should_continue {
            // claude `finalizeAgentTool`: the result's `content` is the LAST
            // assistant message's text blocks, with a backward-scan fallback to
            // the most recent assistant message that has text when the final turn
            // was tool-only (agentToolUtils.ts:304-317). `history` already holds
            // the current assistant turn (pushed above) + every prior turn.
            let result = build_completed_result(&history, &assistant_blocks, stop_reason.as_deref());
            let _ = out_tx
                .send(SubagentEvent::Completed {
                    agent_id,
                    result,
                    usage: last_usage.clone(),
                    total_tool_use_count,
                    total_duration_ms: elapsed_ms(run_start),
                    assistant_message_count,
                    last_request_id: last_request_id.clone(),
                })
                .await;
            // Terminal stop for this turn-set: leave the inner turn loop and
            // let the persist decision below choose between returning
            // (non-persistent) and parking for the next message (persistent).
            terminated_cleanly = true;
            break;
        }
        // Otherwise loop to the next turn.
    }

    if !terminated_cleanly {
        // The inner loop fell through: `max_turns` exhausted without a terminal
        // stop. claude-code surfaces this as a completion carrying a max-turns
        // reason rather than a hard failure, so the parent can still consume
        // whatever work was produced.
        let _ = out_tx
            .send(SubagentEvent::Completed {
                agent_id,
                result: serde_json::json!({
                    "reason": "max_turns_exhausted",
                    "max_turns": max_turns,
                }),
                usage: last_usage.clone(),
                total_tool_use_count,
                total_duration_ms: elapsed_ms(run_start),
                assistant_message_count,
                last_request_id: last_request_id.clone(),
            })
            .await;
    }

    // ----- Persist decision ------------------------------------------------
    // Non-persistent (batch-8) behavior: end after one turn-set. This preserves
    // today's exact semantics — every existing call site sets `persistent`
    // false, so they `return` here as before.
    if !ctx.persistent {
        return;
    }

    // Persistent teammate: park awaiting the next inbound `UserMessage`. If the
    // event channel has already closed, no message can ever arrive again, so we
    // terminate gracefully.
    if !event_channel_open {
        return;
    }
    loop {
        match event_rx.recv().await {
            Some(engine::Event::UserMessage { content, .. }) => {
                // Append the injected message to history (minting our own
                // MessageId, consistent with the assistant-id minting above —
                // the event's message_id / request_id are the host's bookkeeping)
                // and resume the inner turn loop with a fresh `max_turns` budget.
                history.push(ConversationMessage::user(MessageId::new(), content));
                break;
            }
            Some(engine::Event::UserExit | engine::Event::UserInterrupt) => {
                let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
                return;
            }
            // Ignore any other event while idle and keep parking.
            Some(_) => {}
            // Channel closed -> graceful terminate.
            None => return,
        }
    }
    }
}

/// Legacy reducer-driven stub.
///
/// Drives [`engine::reduce`] over `event_rx` and emits [`SubagentEvent`]s on
/// `out_tx`. M1.11 stubs completion after the first event so the pool can be
/// wired end-to-end before the real agentic loop arrives. Selected when
/// [`SubagentContext::api_client`] is `None`.
async fn run_subagent_stub(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<engine::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    use engine::{reduce, ConversationState, SessionState};
    use protocol::SessionId;

    let agent_id = ctx.agent_id;

    // Seed initial state. The runner's local SessionState is transient —
    // the orchestrator (M5-02) owns durable session persistence. We use
    // SessionId::nil() and an inherited model string projected from the
    // agent definition; both are placeholders the reducer accepts.
    let model = match &ctx.agent_definition.model {
        crate::definition::AgentModel::Inherit => "inherit".to_string(),
        crate::definition::AgentModel::Alias(n) | crate::definition::AgentModel::Explicit(n) => {
            n.clone()
        }
    };
    let mut state = ConversationState::Idle {
        session: SessionState::empty(SessionId::nil(), model),
    };
    // Track whether at least one Message has been emitted — informs the
    // EOF branch's choice between Completed(graceful) and Failed.
    let mut produced_useful_work = false;

    while let Some(event) = event_rx.recv().await {
        // Fast path: explicit user-termination events bypass reason-string
        // inspection and surface as Killed directly. The reducer's reason
        // strings are an implementation detail; the input event itself is
        // authoritative for the Killed signal. UserInterrupt in particular
        // does NOT reach Terminated via the M1 reducer (catch-all), so
        // without this fast path it would never produce Killed.
        if matches!(
            &event,
            engine::Event::UserExit | engine::Event::UserInterrupt
        ) {
            // Drive the reducer anyway for state consistency, but ignore
            // the resulting reason.
            let (new_state, _effects) = reduce(state, event);
            state = new_state;
            let _ = out_tx.send(SubagentEvent::Killed { agent_id }).await;
            // Suppress the unused-assignment lint by referencing `state`.
            let _ = &state;
            return;
        }

        // Capture whether this event represents a stream-end completion
        // BEFORE the reducer consumes it — we need to peek at the
        // final_message for the Message emit.
        let api_end_msg = match &event {
            engine::Event::ApiStreamEnd { final_message, .. } => Some(final_message.clone()),
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
                            // Stub path makes no real round-trips: no usage / no
                            // tool-use count / no measured duration.
                            usage: llm_client::Usage::default(),
                            total_tool_use_count: 0,
                            total_duration_ms: 0,
                            assistant_message_count: 0,
                            last_request_id: None,
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
                // Stub path makes no real round-trips.
                usage: llm_client::Usage::default(),
                total_tool_use_count: 0,
                total_duration_ms: 0,
                assistant_message_count: 0,
                last_request_id: None,
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
    use async_trait::async_trait;
    use engine::token::Usage;
    use protocol::{ContentBlock, ConversationMessage, MessageId, RequestId, ToolUseId};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    // ---- Scripted loop-mode fixtures -------------------------------------

    /// `SubagentApiClient` that hands back a pre-scripted queue of responses,
    /// one per `messages_create` call. Counts calls so tests can assert the
    /// number of model round-trips (`max_turns` bound, multi-turn loop).
    struct MockSubagentApiClient {
        responses: Mutex<VecDeque<Result<llm_client::LlmResponse, llm_client::LlmError>>>,
        calls: AtomicUsize,
    }

    impl MockSubagentApiClient {
        fn new(
            responses: Vec<Result<llm_client::LlmResponse, llm_client::LlmError>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: AtomicUsize::new(0),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl crate::api::SubagentApiClient for MockSubagentApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _system: Option<&str>,
            _messages: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses.lock().unwrap().pop_front().unwrap_or_else(|| {
                // Out of scripted responses: a non-terminal, no-tool turn keeps
                // the loop honest (it terminates on empty tool_uses).
                Ok(text_response("(exhausted)", Some("end_turn")))
            })
        }
    }

    /// `SubagentApiClient` that OVERRIDES the streaming seam with scripted
    /// `LlmEvent` sequences (one `Vec` per turn) and makes the non-streaming
    /// `messages_create` unreachable — proving the runner drives the loop
    /// through `messages_create_stream` + `accumulate_stream`, not the
    /// non-streaming fallback.
    struct StreamingMockApiClient {
        turns: Mutex<VecDeque<Vec<llm_client::LlmEvent>>>,
        calls: AtomicUsize,
        /// Tools seen on the most recent `messages_create_stream` call — lets a
        /// test prove `ctx.tool_schemas` threads through the seam.
        last_tools: Mutex<Vec<serde_json::Value>>,
    }

    impl StreamingMockApiClient {
        fn new(turns: Vec<Vec<llm_client::LlmEvent>>) -> Arc<Self> {
            Arc::new(Self {
                turns: Mutex::new(turns.into_iter().collect()),
                calls: AtomicUsize::new(0),
                last_tools: Mutex::new(Vec::new()),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn last_tools(&self) -> Vec<serde_json::Value> {
            self.last_tools.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl crate::api::SubagentApiClient for StreamingMockApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _system: Option<&str>,
            _messages: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
            unreachable!("streaming mock must be driven through messages_create_stream")
        }

        async fn messages_create_stream(
            &self,
            _model: &str,
            _system: Option<&str>,
            _messages: Vec<ConversationMessage>,
            tools: Vec<serde_json::Value>,
        ) -> Result<
            futures::stream::BoxStream<
                'static,
                Result<llm_client::LlmEvent, llm_client::LlmError>,
            >,
            llm_client::LlmError,
        > {
            use futures::StreamExt;
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_tools.lock().unwrap() = tools;
            let events = self.turns.lock().unwrap().pop_front().unwrap_or_default();
            Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
        }
    }

    /// Build the `message_start` envelope shared by the streamed-turn builders.
    fn ev_message_start() -> llm_client::LlmEvent {
        llm_client::LlmEvent::MessageStart {
            response: Box::new(llm_client::LlmResponse {
                id: "mock".into(),
                model: "mock".into(),
                content: vec![],
                stop_reason: None,
                usage: llm_client::Usage::default(),
                cost: None,
                provider_metadata: serde_json::Value::Null,
            }),
        }
    }

    /// One streamed turn carrying a single text block + `stop` reason.
    fn streamed_text_turn(text: &str, stop: &str) -> Vec<llm_client::LlmEvent> {
        use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDeltaPayload};
        vec![
            ev_message_start(),
            LlmEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta { text: text.into() },
            },
            LlmEvent::ContentBlockStop { index: 0 },
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some(stop.into()),
                },
                usage: None,
            },
            LlmEvent::MessageStop,
        ]
    }

    /// One streamed turn carrying a single `tool_call` block + `stop` reason.
    fn streamed_tool_use_turn(name: &str, stop: &str) -> Vec<llm_client::LlmEvent> {
        use llm_client::{ContentBlock, ContentDelta, LlmEvent, MessageDeltaPayload};
        vec![
            ev_message_start(),
            LlmEvent::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::ToolCall {
                    id: ToolUseId::new().to_string(),
                    name: name.into(),
                    input: serde_json::Value::Null,
                },
            },
            LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{}".into(),
                },
            },
            LlmEvent::ContentBlockStop { index: 0 },
            LlmEvent::MessageDelta {
                delta: MessageDeltaPayload {
                    stop_reason: Some(stop.into()),
                },
                usage: None,
            },
            LlmEvent::MessageStop,
        ]
    }

    /// `ToolInvoker` that counts invocations and returns a canned value.
    struct CountingInvoker {
        calls: AtomicUsize,
    }
    impl CountingInvoker {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl traits::ToolInvoker for CountingInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: traits::tool_invoker::SubagentInvocationContext,
        ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("tool-output"))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// `BudgetEnforcerHandle` that reports the budget already exhausted when
    /// `exceeded` is set. `check_and_charge` returns
    /// `Err(BudgetError::Exceeded { current_nano_usd: 1_500_000_000 })`
    /// (i.e. $1.50) when exhausted, else `Ok`. Mirrors the real enforcer's
    /// charge-0 consult used by the per-turn budget gate.
    struct MockBudget {
        exceeded: bool,
    }
    #[async_trait]
    impl traits::budget::BudgetEnforcerHandle for MockBudget {
        async fn check_and_charge(&self, _: u64) -> Result<(), traits::budget::BudgetError> {
            if self.exceeded {
                Err(traits::budget::BudgetError::Exceeded {
                    current_nano_usd: 1_500_000_000,
                })
            } else {
                Ok(())
            }
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            1_500_000_000
        }
    }

    /// Build an `LlmResponse` carrying a single text block.
    fn text_response(text: &str, stop_reason: Option<&str>) -> llm_client::LlmResponse {
        llm_client::LlmResponse {
            id: "mock".into(),
            model: "mock".into(),
            content: vec![llm_client::ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
            stop_reason: stop_reason.map(str::to_string),
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    /// Build an `LlmResponse` carrying one `tool_call` block (+ the given `stop_reason`).
    fn tool_use_response(name: &str, stop_reason: Option<&str>) -> llm_client::LlmResponse {
        llm_client::LlmResponse {
            id: "mock".into(),
            model: "mock".into(),
            content: vec![llm_client::ContentBlock::ToolCall {
                id: ToolUseId::new().to_string(),
                name: name.into(),
                input: serde_json::json!({}),
            }],
            stop_reason: stop_reason.map(str::to_string),
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    /// Build an `LlmResponse` carrying a text block AND a `tool_call` block
    /// (+ the given `stop_reason`) — for the G2 backward-scan test (a turn that
    /// surfaces text then a later turn that is tool-only).
    fn text_and_tool_response(
        text: &str,
        name: &str,
        stop_reason: Option<&str>,
    ) -> llm_client::LlmResponse {
        llm_client::LlmResponse {
            id: "mock".into(),
            model: "mock".into(),
            content: vec![
                llm_client::ContentBlock::Text {
                    text: text.into(),
                    cache_control: None,
                },
                llm_client::ContentBlock::ToolCall {
                    id: ToolUseId::new().to_string(),
                    name: name.into(),
                    input: serde_json::json!({}),
                },
            ],
            stop_reason: stop_reason.map(str::to_string),
            usage: llm_client::Usage::default(),
            cost: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    /// Build an `LlmResponse` carrying one `tool_call` block AND a non-default
    /// `usage` (for the G1 usage-threading test).
    fn tool_use_response_with_usage(
        name: &str,
        stop_reason: Option<&str>,
        usage: llm_client::Usage,
    ) -> llm_client::LlmResponse {
        llm_client::LlmResponse {
            usage,
            ..tool_use_response(name, stop_reason)
        }
    }

    /// `fresh_subagent_ctx` plus a scripted `api_client` (and optional invoker),
    /// raising `max_turns` so multi-turn loops are reachable.
    fn loop_ctx(
        api_client: Arc<dyn crate::api::SubagentApiClient>,
        tool_invoker: Option<Arc<dyn traits::ToolInvoker>>,
        max_turns: u32,
    ) -> SubagentContext {
        let mut ctx = fresh_subagent_ctx();
        ctx.agent_definition.max_turns = max_turns;
        ctx.api_client = Some(api_client);
        ctx.tool_invoker = tool_invoker;
        ctx
    }

    /// Build a `SubagentContext` with the minimum fields the runner reads.
    fn fresh_subagent_ctx() -> SubagentContext {
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_name: None,
            team_name: None,
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
                disallowed_tools: vec![],
                skills: vec![],
                required_mcp_servers: vec![],
                background: false,
                isolation: None,
                memory: None,
                effort: None,
                initial_prompt: None,
                color: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
            persistent: false,
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
            api_client: None,
            tool_invoker: None,
            tool_schemas: vec![],
            budget: None,
            hook_executor: None,
            skill_loader: None,
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
        }
    }

    /// Drain the `SubagentEvent` receiver into a `Vec`.
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

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
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
            .send(engine::Event::UserMessage {
                message_id: msg_id,
                request_id: req,
                content: "hi".into(),
            })
            .await
            .unwrap();
        event_tx
            .send(engine::Event::ApiStreamStart { request_id: req })
            .await
            .unwrap();
        event_tx
            .send(engine::Event::ApiStreamEnd {
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

    #[tokio::test]
    async fn run_subagent_emits_killed_on_user_exit_terminal() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        // Drive directly to terminal via UserExit. The fast-path in the
        // runner short-circuits to Killed on this input event.
        event_tx.send(engine::Event::UserExit).await.unwrap();
        drop(event_tx);

        handle.await.unwrap();
        let evs = drain(out_rx).await;

        let killed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Killed { .. }))
            .count();
        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();

        // Exactly one Killed, no Completed (Killed is the terminal here).
        assert_eq!(
            killed_count, 1,
            "exactly one Killed expected on UserExit; got events: {evs:?}"
        );
        assert_eq!(
            completed_count, 0,
            "no Completed expected when terminal is UserExit; got events: {evs:?}"
        );

        // Killed carries the agent id.
        let killed_aid = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Killed { agent_id } => Some(*agent_id),
                _ => None,
            })
            .unwrap();
        assert_eq!(killed_aid, agent_id);
    }

    #[tokio::test]
    async fn run_subagent_emits_killed_on_user_interrupt_terminal() {
        // UserInterrupt does NOT terminate via the M1 reducer (catch-all),
        // so without the fast-path this would emit Failed (channel close
        // before terminal). The fast-path makes it produce Killed instead.
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        event_tx.send(engine::Event::UserInterrupt).await.unwrap();
        drop(event_tx);

        handle.await.unwrap();
        let evs = drain(out_rx).await;

        assert!(
            evs.iter()
                .any(|e| matches!(e, SubagentEvent::Killed { agent_id: aid } if *aid == agent_id)),
            "expected exactly one Killed on UserInterrupt; got events: {evs:?}"
        );
        assert!(
            !evs.iter()
                .any(|e| matches!(e, SubagentEvent::Failed { .. })),
            "no Failed expected on UserInterrupt; got events: {evs:?}"
        );
    }

    #[tokio::test]
    async fn run_subagent_emits_failed_on_eof_before_any_work() {
        let ctx = fresh_subagent_ctx();
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(8);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        // Drop the sender immediately — runner sees event_rx close on the
        // very first recv, with no prior Message or Terminated emission.
        drop(event_tx);

        handle.await.unwrap();
        let evs = drain(out_rx).await;

        let failed = evs.iter().find_map(|e| match e {
            SubagentEvent::Failed {
                agent_id: aid,
                error,
            } => Some((*aid, error.clone())),
            _ => None,
        });
        assert!(
            failed.is_some(),
            "exactly one Failed expected on premature EOF; got events: {evs:?}"
        );
        let (failed_aid, failed_err) = failed.unwrap();
        assert_eq!(failed_aid, agent_id);
        assert_eq!(
            failed_err, "run_subagent: event channel closed without terminal state",
            "byte-locked error message"
        );

        let completed_count = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();
        assert_eq!(
            completed_count, 0,
            "no Completed expected; got events: {evs:?}"
        );
    }

    // ---- Loop-mode tests (api_client = Some) -----------------------------

    /// Pull the single `Completed.result` payload (panics if none / many).
    fn one_completed(evs: &[SubagentEvent]) -> serde_json::Value {
        let mut found = evs.iter().filter_map(|e| match e {
            SubagentEvent::Completed { result, .. } => Some(result.clone()),
            _ => None,
        });
        let r = found.next().expect("exactly one Completed");
        assert!(found.next().is_none(), "more than one Completed: {evs:?}");
        r
    }

    #[tokio::test]
    async fn loop_single_end_turn_completes_with_aggregated_text() {
        // One turn: end_turn, no tools. Asserts the terminal-text path and the
        // `{text, stop_reason}` result shape, and that the model was called once.
        let api = MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
        let ctx = loop_ctx(api.clone(), None, 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 1, "exactly one model round-trip");
        let result = one_completed(&evs);
        assert_eq!(result["text"], "final answer");
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_completed_result_carries_claude_content_array() {
        // #3: the terminal result carries claude's `content` array of text
        // blocks (one per text block), not only the joined `text` string.
        let api =
            MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
        let ctx = loop_ctx(api.clone(), None, 4);
        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;
        let result = one_completed(&evs);
        assert_eq!(
            result["content"],
            serde_json::json!([{ "type": "text", "text": "final answer" }]),
            "result carries claude content[] array"
        );
        assert_eq!(result["text"], "final answer", "legacy `text` still present");
    }

    #[tokio::test]
    async fn loop_g2_backward_scan_recovers_text_from_earlier_turn() {
        // G2 (agentToolUtils.ts:304-317): when the FINAL assistant turn is
        // tool-only (no text), the result content falls back to the most recent
        // assistant message that HAS text. Turn 1: text "partial" + a tool_use
        // (continues). Turn 2: tool-only with end_turn (terminates, no text in
        // the final block) → content must be "partial" from turn 1.
        let api = MockSubagentApiClient::new(vec![
            Ok(text_and_tool_response("partial", "Read", Some("tool_use"))),
            Ok(tool_use_response("Read", Some("end_turn"))),
        ]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;
        let result = one_completed(&evs);
        // The final turn was tool-only; the backward scan recovered turn 1's text.
        assert_eq!(
            result["content"],
            serde_json::json!([{ "type": "text", "text": "partial" }]),
            "backward scan recovers the most recent assistant text; got {result:?}"
        );
        assert_eq!(result["text"], "partial");
    }

    #[tokio::test]
    async fn loop_g1_completed_carries_final_turn_usage_and_tool_count() {
        // G1: the terminal Completed event carries the FINAL turn's usage (claude
        // reads only the last message usage, not a cross-turn sum) plus the
        // run-wide tool-use count. Turn 1: tool_use with usage A (dispatched).
        // Turn 2: end_turn text with usage B → carried usage == B; tool_uses == 1.
        let usage_a = llm_client::Usage {
            billable_tokens: llm_client::TokenUsage {
                input: 1000,
                output: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let usage_b = llm_client::Usage {
            billable_tokens: llm_client::TokenUsage {
                input: 10,
                output: 5,
                cache_write: 3,
                cache_read: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let api = MockSubagentApiClient::new(vec![
            Ok(tool_use_response_with_usage("Read", Some("tool_use"), usage_a)),
            Ok(llm_client::LlmResponse {
                usage: usage_b.clone(),
                ..text_response("done", Some("end_turn"))
            }),
        ]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;
        let (usage, tool_count) = evs
            .iter()
            .find_map(|e| match e {
                SubagentEvent::Completed {
                    usage,
                    total_tool_use_count,
                    ..
                } => Some((usage.clone(), *total_tool_use_count)),
                _ => None,
            })
            .expect("one Completed");
        // The carried usage is the FINAL turn's (B), NOT a sum with A.
        assert_eq!(usage.billable_tokens.input, 10, "final-turn input, not summed");
        assert_eq!(usage.billable_tokens.output, 5);
        assert_eq!(usage.billable_tokens.cache_write, 3);
        assert_eq!(usage.billable_tokens.cache_read, 2);
        // One tool_use across the run (turn 1).
        assert_eq!(tool_count, 1, "run-wide tool-use count");
    }

    #[tokio::test]
    async fn loop_consumes_streaming_seam_end_to_end() {
        // Proves the runner drives the loop through `messages_create_stream`:
        // the mock's non-streaming `messages_create` is `unreachable!`. Turn 1
        // streams a `tool_use` (dispatched via the invoker); turn 2 streams the
        // final text. Asserts two streamed round-trips, one tool dispatch, and
        // that the accumulated turn carries the streamed text + stop_reason.
        let api = StreamingMockApiClient::new(vec![
            streamed_tool_use_turn("Read", "tool_use"),
            streamed_text_turn("streamed answer", "end_turn"),
        ]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 2, "two streamed round-trips");
        assert_eq!(invoker.call_count(), 1, "streamed tool_use dispatched once");
        let result = one_completed(&evs);
        assert_eq!(result["text"], "streamed answer");
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_advertises_context_tool_schemas_to_the_seam() {
        // `ctx.tool_schemas` must reach `messages_create_stream`'s `tools` arg
        // on every round-trip (this is what lets the model emit `tool_use`).
        let api = StreamingMockApiClient::new(vec![streamed_text_turn("done", "end_turn")]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        let schemas = vec![serde_json::json!({
            "name": "Read",
            "description": "Reads a file.",
            "input_schema": {"type": "object"}
        })];
        ctx.tool_schemas = schemas.clone();

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        assert_eq!(
            api.last_tools(),
            schemas,
            "ctx.tool_schemas must be forwarded verbatim to the streaming seam"
        );
    }

    #[tokio::test]
    async fn loop_streaming_protocol_error_surfaces_failed() {
        // A streamed turn that ends without `message_stop` accumulates to
        // `LlmError::StreamInterrupted`, which the loop surfaces as Failed
        // (same path as a non-streaming api error).
        let truncated = vec![
            ev_message_start(),
            llm_client::LlmEvent::ContentBlockStart {
                index: 0,
                content_block: llm_client::ContentBlock::Text {
                    text: String::new(),
                    cache_control: None,
                },
            },
            // no content_block_stop, no message_stop
        ];
        let api = StreamingMockApiClient::new(vec![truncated]);
        let ctx = loop_ctx(api.clone(), None, 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        let failed = evs.iter().find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        });
        let err = failed.expect("Failed on truncated stream");
        assert!(err.starts_with("subagent api error:"), "got: {err}");
    }

    #[tokio::test]
    async fn loop_refuses_tool_outside_allowed_list_without_dispatching() {
        // allowed_tools = ["Bash"]; the model asks for "Read" → refused WITHOUT
        // dispatch (invoker never called), surfaced as an is_error ToolResult,
        // and the loop continues to a clean end_turn.
        let api = StreamingMockApiClient::new(vec![
            streamed_tool_use_turn("Read", "tool_use"),
            streamed_text_turn("done", "end_turn"),
        ]);
        let invoker = CountingInvoker::new();
        let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
        ctx.allowed_tools = vec!["Bash".to_string()];

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(
            invoker.call_count(),
            0,
            "a tool outside allowed_tools must NOT be dispatched"
        );
        // The refusal must be a structured is_error ToolResult carrying the
        // reason (so the model sees it like any tool error and can recover) —
        // deserialize the emitted Message rather than substring-matching.
        let refused = evs.iter().any(|e| {
            let SubagentEvent::Message { message, .. } = e else {
                return false;
            };
            let Ok(ConversationMessage::User { content, .. }) =
                serde_json::from_value::<ConversationMessage>(message.clone())
            else {
                return false;
            };
            content.iter().any(|b| {
                matches!(b, ContentBlock::ToolResult { is_error: true, content, .. }
                    if content.contains("not in this agent's allowed tools"))
            })
        });
        assert!(
            refused,
            "refusal must be an is_error ToolResult carrying the reason; got {evs:?}"
        );
        let result = one_completed(&evs);
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_allows_tool_in_allowed_list() {
        // allowed_tools = ["Read"]; "Read" is dispatched normally.
        let api = StreamingMockApiClient::new(vec![
            streamed_tool_use_turn("Read", "tool_use"),
            streamed_text_turn("done", "end_turn"),
        ]);
        let invoker = CountingInvoker::new();
        let mut ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);
        ctx.allowed_tools = vec!["Read".to_string()];

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        assert_eq!(
            invoker.call_count(),
            1,
            "a tool inside allowed_tools is dispatched"
        );
    }

    #[tokio::test]
    async fn loop_budget_exhausted_stops_before_any_round_trip() {
        // Test A: the inherited budget is already over the limit. The per-turn
        // gate fires BEFORE the first model round-trip, so the loop emits a
        // single budget-exhausted Failed and makes ZERO model calls. The error
        // is the M3-05 byte-locked denial string formatted from the enforcer's
        // current_nano_usd (1.5e9 -> "$1.50").
        let api = MockSubagentApiClient::new(vec![Ok(text_response("unused", Some("end_turn")))]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        ctx.budget = Some(Arc::new(MockBudget { exceeded: true }));

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(
            api.call_count(),
            0,
            "the budget gate precedes messages_create — no round-trip"
        );
        let failed = evs.iter().find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        });
        assert_eq!(
            failed.as_deref(),
            Some("Budget exceeded ($1.50); stopped."),
            "byte-locked M3-05 denial string; got events: {evs:?}"
        );
        assert!(
            !evs.iter().any(|e| matches!(e, SubagentEvent::Completed { .. })),
            "no Completed when stopped on budget; got events: {evs:?}"
        );
    }

    #[tokio::test]
    async fn loop_budget_ok_does_not_interfere_with_completion() {
        // Test B (non-interference): a within-limit budget lets the existing
        // single-end_turn path complete with aggregated text and the api is
        // called exactly once — the gate is transparent when `Ok`.
        let api = MockSubagentApiClient::new(vec![Ok(text_response("final answer", Some("end_turn")))]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        ctx.budget = Some(Arc::new(MockBudget { exceeded: false }));

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 1, "exactly one model round-trip");
        let result = one_completed(&evs);
        assert_eq!(result["text"], "final answer");
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_tool_use_then_end_turn_invokes_tool_and_runs_two_turns() {
        // Core happy path: turn 1 emits a tool_use (stop_reason tool_use) ->
        // tool is invoked -> results fed back -> turn 2 ends. Asserts 2 model
        // calls, exactly one tool invocation, and final aggregated text.
        let api = MockSubagentApiClient::new(vec![
            Ok(tool_use_response("Read", Some("tool_use"))),
            Ok(text_response("done", Some("end_turn"))),
        ]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 2, "two model round-trips");
        assert_eq!(invoker.call_count(), 1, "tool invoked once (1:1 with tool_use)");
        let result = one_completed(&evs);
        assert_eq!(result["text"], "done");
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_end_turn_with_tool_use_still_dispatches_then_completes() {
        // MAJOR #1 regression: an `end_turn` response that ALSO carries a
        // tool_use must NOT silently drop the tool. The reference dispatches
        // tools whenever present, then terminates on end_turn. Assert the tool
        // was invoked AND the run completed in a single turn (no continuation).
        let api =
            MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("end_turn")))]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 1, "end_turn terminates after one round-trip");
        assert_eq!(
            invoker.call_count(),
            1,
            "tool_use on an end_turn response is still dispatched"
        );
        let result = one_completed(&evs);
        assert_eq!(result["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn loop_truncated_tool_use_terminates_instead_of_looping() {
        // MAJOR #2 regression: a non-`tool_use` reason (e.g. max_tokens) that
        // also carried a tool_use must dispatch the tool then TERMINATE — not
        // continue looping until max_turns. With max_turns=4 the loop would
        // make 4 calls if it (incorrectly) continued; the fix caps it at 1.
        let api =
            MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("max_tokens")))]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(
            api.call_count(),
            1,
            "max_tokens terminates after one round-trip (no loop-to-max_turns)"
        );
        assert_eq!(invoker.call_count(), 1, "the truncated turn's tool is still dispatched");
        let result = one_completed(&evs);
        assert_eq!(result["stop_reason"], "max_tokens");
    }

    #[tokio::test]
    async fn loop_api_error_surfaces_failed() {
        let api = MockSubagentApiClient::new(vec![Err(llm_client::LlmError::InvalidRequest {
            message: "boom".into(),
        })]);
        let ctx = loop_ctx(api.clone(), None, 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        let failed = evs.iter().find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        });
        let err = failed.expect("Failed on api error");
        assert!(err.starts_with("subagent api error:"), "got: {err}");
    }

    #[tokio::test]
    async fn loop_tool_use_without_invoker_fails() {
        // A tool_use with tool_invoker = None surfaces Failed.
        let api =
            MockSubagentApiClient::new(vec![Ok(tool_use_response("Read", Some("tool_use")))]);
        let ctx = loop_ctx(api.clone(), None, 4);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        let failed = evs.iter().find_map(|e| match e {
            SubagentEvent::Failed { error, .. } => Some(error.clone()),
            _ => None,
        });
        assert_eq!(
            failed.as_deref(),
            Some("subagent requested a tool but no tool_invoker was inherited")
        );
    }

    #[tokio::test]
    async fn loop_exhausts_max_turns_when_never_terminal() {
        // Every turn emits a tool_use with stop_reason tool_use, so the loop
        // continues. With max_turns=3 it makes exactly 3 model calls then
        // surfaces Completed{reason: "max_turns_exhausted"}.
        let api = MockSubagentApiClient::new(vec![
            Ok(tool_use_response("Read", Some("tool_use"))),
            Ok(tool_use_response("Read", Some("tool_use"))),
            Ok(tool_use_response("Read", Some("tool_use"))),
            // a 4th would only be reached on an off-by-one bug:
            Ok(text_response("should-not-reach", Some("end_turn"))),
        ]);
        let invoker = CountingInvoker::new();
        let ctx = loop_ctx(api.clone(), Some(invoker.clone()), 3);

        let (_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 3, "exactly max_turns model round-trips");
        assert_eq!(invoker.call_count(), 3, "one tool dispatch per turn");
        let result = one_completed(&evs);
        assert_eq!(result["reason"], "max_turns_exhausted");
        assert_eq!(result["max_turns"], 3);
    }

    #[tokio::test]
    async fn loop_user_interrupt_mid_flight_surfaces_killed() {
        // A UserInterrupt delivered while the loop is racing the API future
        // aborts to Killed. We pre-load the event so the biased select! takes
        // the termination arm on the first poll.
        let api = MockSubagentApiClient::new(vec![Ok(text_response("unused", Some("end_turn")))]);
        let ctx = loop_ctx(api.clone(), None, 4);

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        event_tx.send(engine::Event::UserInterrupt).await.unwrap();

        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert!(
            evs.iter().any(|e| matches!(e, SubagentEvent::Killed { .. })),
            "expected Killed on UserInterrupt; got: {evs:?}"
        );
        assert!(
            !evs.iter().any(|e| matches!(e, SubagentEvent::Completed { .. })),
            "no Completed when killed mid-flight; got: {evs:?}"
        );
    }

    // ---- Persist-mode tests (ctx.persistent = true) ----------------------

    #[tokio::test]
    async fn persist_mode_processes_second_message_after_idling() {
        // Turn-set 1: a single end_turn turn completes, then the runner parks
        // (it does NOT return because persistent = true). We then inject a
        // second UserMessage which un-idles it and drives turn-set 2; finally
        // we close the channel to terminate gracefully. Asserts: exactly two
        // model round-trips and two Completed events (one per turn-set).
        let api = MockSubagentApiClient::new(vec![
            Ok(text_response("answer one", Some("end_turn"))),
            Ok(text_response("answer two", Some("end_turn"))),
        ]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        ctx.persistent = true;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        // Wait for turn-set 1 to complete (the runner has now idled), then
        // inject the second message that drives turn-set 2.
        let mut out_rx = out_rx;
        let first_completed = loop {
            let ev = out_rx.recv().await.expect("turn-set 1 should complete");
            if matches!(ev, SubagentEvent::Completed { .. }) {
                break ev;
            }
        };
        let SubagentEvent::Completed { result, .. } = &first_completed else {
            unreachable!()
        };
        assert_eq!(result["text"], "answer one", "turn-set 1 result");

        event_tx
            .send(engine::Event::UserMessage {
                message_id: MessageId::new(),
                request_id: RequestId::new(),
                content: "second question".into(),
            })
            .await
            .unwrap();

        // Wait for turn-set 2 to complete.
        let second_completed = loop {
            let ev = out_rx.recv().await.expect("turn-set 2 should complete");
            if matches!(ev, SubagentEvent::Completed { .. }) {
                break ev;
            }
        };
        let SubagentEvent::Completed { result, .. } = &second_completed else {
            unreachable!()
        };
        assert_eq!(result["text"], "answer two", "turn-set 2 result");

        // Close the channel: the parked runner terminates gracefully.
        drop(event_tx);
        handle.await.unwrap();

        assert_eq!(
            api.call_count(),
            2,
            "exactly two model round-trips (one per turn-set)"
        );
    }

    #[tokio::test]
    async fn persist_mode_terminates_on_channel_close_after_turn_set() {
        // With persistent = true, closing the event channel after the first
        // turn-set completes makes the parked runner return gracefully (no
        // further events, no Failed).
        let api = MockSubagentApiClient::new(vec![Ok(text_response("done", Some("end_turn")))]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        ctx.persistent = true;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

        // Drop the sender immediately: the runner runs turn-set 1, parks, sees
        // the channel already closed, and returns.
        drop(event_tx);

        run_subagent(ctx, event_rx, out_tx).await;
        let evs = drain(out_rx).await;

        assert_eq!(api.call_count(), 1, "one turn-set ran before EOF");
        let completed = evs
            .iter()
            .filter(|e| matches!(e, SubagentEvent::Completed { .. }))
            .count();
        assert_eq!(completed, 1, "one Completed; got: {evs:?}");
        assert!(
            !evs.iter().any(|e| matches!(e, SubagentEvent::Failed { .. })),
            "no Failed on graceful EOF; got: {evs:?}"
        );
    }

    #[tokio::test]
    async fn persist_mode_user_exit_while_idle_surfaces_killed() {
        // While parked between turn-sets, a UserExit terminates the teammate
        // with Killed (cooperative shutdown).
        let api = MockSubagentApiClient::new(vec![Ok(text_response("done", Some("end_turn")))]);
        let mut ctx = loop_ctx(api.clone(), None, 4);
        ctx.persistent = true;
        let agent_id = ctx.agent_id;

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);

        let handle = tokio::spawn(run_subagent(ctx, event_rx, out_tx));

        let mut out_rx = out_rx;
        // Wait for turn-set 1 to complete (runner now idle).
        loop {
            let ev = out_rx.recv().await.expect("turn-set 1 completes");
            if matches!(ev, SubagentEvent::Completed { .. }) {
                break;
            }
        }
        // Deliver UserExit to the idle runner.
        event_tx.send(engine::Event::UserExit).await.unwrap();
        handle.await.unwrap();

        let evs = drain(out_rx).await;
        assert!(
            evs.iter()
                .any(|e| matches!(e, SubagentEvent::Killed { agent_id: aid } if *aid == agent_id)),
            "UserExit while idle yields Killed; got: {evs:?}"
        );
    }

    // ── G4 (SubagentStart additionalContext) + G5 (skills preload) ──────────

    /// `SubagentApiClient` that captures the `messages` of its FIRST round-trip
    /// so a test can assert what the runner seeded as the child's initial
    /// history (the preload messages are sent to the model, not emitted as
    /// events). Replies with a single end_turn text turn.
    struct CapturingApiClient {
        first_messages: Mutex<Option<Vec<ConversationMessage>>>,
    }
    impl CapturingApiClient {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                first_messages: Mutex::new(None),
            })
        }
        fn captured(&self) -> Vec<ConversationMessage> {
            self.first_messages.lock().unwrap().clone().unwrap_or_default()
        }
    }
    #[async_trait]
    impl crate::api::SubagentApiClient for CapturingApiClient {
        async fn messages_create(
            &self,
            _model: &str,
            _system: Option<&str>,
            messages: Vec<ConversationMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<llm_client::LlmResponse, llm_client::LlmError> {
            let mut slot = self.first_messages.lock().unwrap();
            if slot.is_none() {
                *slot = Some(messages);
            }
            Ok(text_response("done", Some("end_turn")))
        }
    }

    /// A `SubagentStart` builtin hook handler that returns one `additionalContext`
    /// string so the runner injects it into the child's initial history (G4).
    /// `handler_id` lets a test register more than one handler (distinct ids).
    struct AdditionalContextStartHook {
        handler_id: String,
        context: String,
    }
    #[async_trait]
    impl hooks::executor::BuiltinHookHandler for AdditionalContextStartHook {
        fn id(&self) -> &str {
            &self.handler_id
        }
        async fn handle(
            &self,
            _event: &hooks::events::HookEvent,
            _ctx: &hooks::registry::HookContext,
        ) -> hooks::response::HookResult {
            hooks::response::HookResult {
                outcome: hooks::response::HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: Some(hooks::response::HookResponse {
                    additional_context: Some(self.context.clone()),
                    ..Default::default()
                }),
            }
        }
    }

    /// Build an `Arc<HookExecutorImpl>` with ONE registered SubagentStart hook
    /// that returns `context` as additionalContext.
    async fn exec_with_start_context(context: &str) -> Arc<hooks::HookExecutorImpl> {
        use hooks::definition::{HookDefinition, HookExecutor, HookSource};
        use hooks::events::HookEventType;
        let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
        registry.write().await.register(HookDefinition {
            id: protocol::HookId::new(),
            name: "additional-context-start".into(),
            events: vec![HookEventType::SubagentStart],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: "additional-context-start".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        });
        let mut exec = hooks::HookExecutorImpl::new(
            registry,
            Arc::new(test_harness::mocks::MockHttpTransport::new()),
            Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
        );
        exec.register_builtin(Arc::new(AdditionalContextStartHook {
            handler_id: "additional-context-start".into(),
            context: context.to_string(),
        }));
        Arc::new(exec)
    }

    /// Build an `Arc<HookExecutorImpl>` with TWO registered SubagentStart hooks,
    /// each returning its own additionalContext — to prove the runner JOINS them
    /// into a single `<system-reminder>` message (claude byte-parity).
    async fn exec_with_two_start_contexts(c0: &str, c1: &str) -> Arc<hooks::HookExecutorImpl> {
        use hooks::definition::{HookDefinition, HookExecutor, HookSource};
        use hooks::events::HookEventType;
        let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
        for (i, handler_id) in ["start-ctx-0", "start-ctx-1"].iter().enumerate() {
            registry.write().await.register(HookDefinition {
                id: protocol::HookId::new(),
                name: (*handler_id).into(),
                events: vec![HookEventType::SubagentStart],
                if_condition: None,
                executor: HookExecutor::Builtin {
                    handler_id: (*handler_id).into(),
                },
                source: HookSource::User,
                blocking: true,
                timeout: None,
                // Distinct DESCENDING priorities pin the firing order so the
                // join is deterministic (c0 then c1). `match_event` sorts
                // priority-descending, so index 0 (priority 0) fires before
                // index 1 (priority -1).
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                priority: -(i as i32),
                once: false,
                status_message: None,
            });
        }
        let mut exec = hooks::HookExecutorImpl::new(
            registry,
            Arc::new(test_harness::mocks::MockHttpTransport::new()),
            Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
        );
        exec.register_builtin(Arc::new(AdditionalContextStartHook {
            handler_id: "start-ctx-0".into(),
            context: c0.to_string(),
        }));
        exec.register_builtin(Arc::new(AdditionalContextStartHook {
            handler_id: "start-ctx-1".into(),
            context: c1.to_string(),
        }));
        Arc::new(exec)
    }

    /// A builtin hook handler that records every `SubagentStop` it sees (the
    /// `status` carried on the event) — used to prove a frontmatter
    /// `Stop`→`SubagentStop` hook actually fires inside the child runner (#9).
    struct RecordingStopHook {
        seen: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl hooks::executor::BuiltinHookHandler for RecordingStopHook {
        fn id(&self) -> &str {
            "record-subagent-stop-in-runner"
        }
        async fn handle(
            &self,
            event: &hooks::events::HookEvent,
            _ctx: &hooks::registry::HookContext,
        ) -> hooks::response::HookResult {
            if let hooks::events::HookEvent::SubagentStop { status, .. } = event {
                self.seen.lock().unwrap().push(status.clone());
            }
            hooks::response::HookResult {
                outcome: hooks::response::HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: None,
            }
        }
    }

    /// Build an executor wired with the [`RecordingStopHook`] builtin and NO
    /// source/plugin hooks — so any SubagentStop the recorder sees must have come
    /// from the agent-scoped frontmatter fire (the runner path), not a chokepoint.
    fn exec_recording_stop(seen: Arc<Mutex<Vec<String>>>) -> Arc<hooks::HookExecutorImpl> {
        let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
        let mut exec = hooks::HookExecutorImpl::new(
            registry,
            Arc::new(test_harness::mocks::MockHttpTransport::new()),
            Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
        );
        exec.register_builtin(Arc::new(RecordingStopHook { seen }));
        Arc::new(exec)
    }

    /// A frontmatter `Stop` hook (Builtin executor) the runner retargets to
    /// `SubagentStop` (registerFrontmatterHooks isAgent=true).
    fn frontmatter_stop_hook(handler_id: &str) -> hooks::definition::HookDefinition {
        use hooks::definition::{HookExecutor, HookSource};
        use hooks::events::HookEventType;
        hooks::definition::HookDefinition {
            id: protocol::HookId::new(),
            name: handler_id.into(),
            events: vec![HookEventType::Stop],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: handler_id.into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[tokio::test]
    async fn frontmatter_stop_hook_fires_as_subagent_stop_in_runner() {
        // #9: a frontmatter `Stop` hook is retargeted to `SubagentStop`
        // (isAgent=true) and MUST fire at the child loop's clean end — BEFORE
        // `clear_agent_hooks` removes it. A clean (end_turn) run yields one
        // `SubagentStop` with status "completed".
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        ctx.agent_definition.agent_type = "stop-agent".into();
        ctx.agent_definition.frontmatter_hooks =
            vec![frontmatter_stop_hook("record-subagent-stop-in-runner")];
        ctx.hook_executor = Some(exec_recording_stop(seen.clone()));

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let recorded = seen.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["completed".to_string()],
            "frontmatter Stop→SubagentStop must fire exactly once (status completed): {recorded:?}"
        );
    }

    #[tokio::test]
    async fn no_frontmatter_hooks_means_no_runner_subagent_stop() {
        // With NO frontmatter hooks the runner takes the passthrough path and
        // fires NO agent-scoped SubagentStop (the orchestrator chokepoint owns
        // session/plugin SubagentStop). The recorder sees nothing.
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        // No frontmatter_hooks (default empty). Executor still wired.
        ctx.hook_executor = Some(exec_recording_stop(seen.clone()));

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        assert!(
            seen.lock().unwrap().is_empty(),
            "no frontmatter hooks ⇒ no agent-scoped SubagentStop fired"
        );
    }

    /// Counts every SubagentStart and SubagentStop event the runner fires, so a
    /// test can assert each canonical lifecycle hook fires EXACTLY once through
    /// the REAL runner (R7 — no double-fire).
    struct StartStopCounter {
        starts: Arc<Mutex<u32>>,
        stops: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl hooks::executor::BuiltinHookHandler for StartStopCounter {
        fn id(&self) -> &str {
            "r7-start-stop-counter"
        }
        async fn handle(
            &self,
            event: &hooks::events::HookEvent,
            _ctx: &hooks::registry::HookContext,
        ) -> hooks::response::HookResult {
            match event {
                hooks::events::HookEvent::SubagentStart { .. } => {
                    *self.starts.lock().unwrap() += 1;
                }
                hooks::events::HookEvent::SubagentStop { status, .. } => {
                    self.stops.lock().unwrap().push(status.clone());
                }
                _ => {}
            }
            hooks::response::HookResult {
                outcome: hooks::response::HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                response: None,
            }
        }
    }

    #[tokio::test]
    async fn runner_fires_subagent_start_and_frontmatter_stop_exactly_once_each() {
        // R7 integration: a REAL runner run with BOTH a SubagentStart hook AND a
        // frontmatter `Stop`→`SubagentStop` hook (isAgent=true) must fire
        // SubagentStart EXACTLY once (the canonical, additionalContext-collecting
        // fire) and the frontmatter SubagentStop EXACTLY once (agent-scoped,
        // BEFORE clear_agent_hooks). This is the assertion the orchestrator-side
        // FakeAgentTool fixtures (no runner) cannot make.
        use hooks::definition::{HookDefinition, HookExecutor, HookSource};
        use hooks::events::HookEventType;

        let starts = Arc::new(Mutex::new(0u32));
        let stops = Arc::new(Mutex::new(Vec::<String>::new()));

        // One executor with: a SESSION-level SubagentStart hook (fires in G4) and
        // the frontmatter Stop hook is supplied via `frontmatter_hooks` below
        // (the runner registers + retargets it to SubagentStop, isAgent=true).
        let registry = Arc::new(tokio::sync::RwLock::new(hooks::HookRegistry::new()));
        registry.write().await.register(HookDefinition {
            id: protocol::HookId::new(),
            name: "r7-start".into(),
            events: vec![HookEventType::SubagentStart],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: "r7-start-stop-counter".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        });
        let mut exec = hooks::HookExecutorImpl::new(
            registry,
            Arc::new(test_harness::mocks::MockHttpTransport::new()),
            Arc::new(test_harness::mocks::MockRuntimeSpawner::default()),
        );
        exec.register_builtin(Arc::new(StartStopCounter {
            starts: starts.clone(),
            stops: stops.clone(),
        }));
        let exec = Arc::new(exec);

        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        ctx.agent_definition.agent_type = "r7-agent".into();
        // The frontmatter Stop hook points at the SAME counter handler; the
        // runner retargets Stop→SubagentStop (isAgent=true) and fires it
        // agent-scoped at the loop's clean end.
        ctx.agent_definition.frontmatter_hooks = vec![frontmatter_stop_hook("r7-start-stop-counter")];
        ctx.hook_executor = Some(exec);

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        assert_eq!(
            *starts.lock().unwrap(),
            1,
            "SubagentStart must fire EXACTLY once through the real runner (no double-fire)"
        );
        assert_eq!(
            stops.lock().unwrap().clone(),
            vec!["completed".to_string()],
            "frontmatter Stop→SubagentStop must fire EXACTLY once (status completed) in the runner"
        );
    }

    /// Mock [`SkillLoader`] that resolves a fixed name to canned content, else None.
    struct MockSkillLoader {
        known: String,
        content_text: String,
    }
    #[async_trait]
    impl traits::skill_loader::SkillLoader for MockSkillLoader {
        async fn resolve_and_load(
            &self,
            skill_name: &str,
            _agent_type: &str,
        ) -> Option<traits::skill_loader::SkillLoad> {
            if skill_name == self.known {
                Some(traits::skill_loader::SkillLoad {
                    display_name: skill_name.to_string(),
                    progress_message: None,
                    content: vec![ContentBlock::Text {
                        text: self.content_text.clone(),
                    }],
                })
            } else {
                None
            }
        }
    }

    fn user_text(msg: &ConversationMessage) -> Option<String> {
        if let ConversationMessage::User { content, .. } = msg {
            let t: Vec<String> = content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            Some(t.join("\n"))
        } else {
            None
        }
    }

    #[tokio::test]
    async fn subagent_start_additional_context_injected_as_system_reminder() {
        // G4: a SubagentStart hook's additionalContext lands as a
        // `<system-reminder>` user message in the child's initial history,
        // AFTER the prompt seed and BEFORE turn 1.
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
        ctx.hook_executor = Some(exec_with_start_context("extra from hook").await);

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let msgs = api.captured();
        // The prompt is first, the injected system-reminder follows it.
        let texts: Vec<String> = msgs.iter().filter_map(user_text).collect();
        assert!(
            texts.iter().any(|t| t == "do it"),
            "prompt seed present: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t
                == "<system-reminder>\nSubagentStart hook additional context: extra from hook\n</system-reminder>"),
            "additionalContext injected as the claude-byte <system-reminder> message: {texts:?}"
        );
    }

    #[tokio::test]
    async fn subagent_start_multiple_contexts_join_into_one_reminder() {
        // G4 byte-parity (runAgent.ts:530-555 + messages.ts:4117-4128): when
        // multiple SubagentStart hooks each return additionalContext, claude
        // collects them into ONE `string[]` and emits a SINGLE
        // `hook_additional_context` attachment whose body is
        // `SubagentStart hook additional context: ` + contexts.join("\n").
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
        ctx.hook_executor = Some(exec_with_two_start_contexts("alpha", "beta").await);

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
        // Exactly ONE system-reminder message (not one per context).
        let reminders: Vec<&String> = texts
            .iter()
            .filter(|t| t.starts_with("<system-reminder>\nSubagentStart hook additional context: "))
            .collect();
        assert_eq!(
            reminders.len(),
            1,
            "exactly one joined SubagentStart reminder: {texts:?}"
        );
        assert_eq!(
            reminders[0],
            "<system-reminder>\nSubagentStart hook additional context: alpha\nbeta\n</system-reminder>",
            "contexts joined with \\n in a single reminder"
        );
    }

    #[tokio::test]
    async fn no_hook_executor_means_no_preload_injection() {
        // G4: with hook_executor=None the child history carries ONLY the prompt
        // seed — byte-identical to legacy (no SubagentStart fire).
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "do it".into())];
        // hook_executor + skill_loader both unset (default).

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let msgs = api.captured();
        assert_eq!(msgs.len(), 1, "only the prompt seed; got: {msgs:?}");
        assert_eq!(user_text(&msgs[0]).as_deref(), Some("do it"));
    }

    #[tokio::test]
    async fn resolved_skill_prepends_metadata_then_content() {
        // G5: a resolved skill is injected as a user message whose first block is
        // the byte-locked loading metadata, followed by the loaded content.
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        ctx.agent_definition.agent_type = "my-agent".into();
        ctx.agent_definition.skills = vec!["my-skill".into()];
        ctx.skill_loader = Some(Arc::new(MockSkillLoader {
            known: "my-skill".into(),
            content_text: "SKILL BODY".into(),
        }));

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let msgs = api.captured();
        // The skill meta message carries the <skill-format> marker block first,
        // then the loaded content.
        let skill_text = msgs
            .iter()
            .filter_map(user_text)
            .find(|t| t.contains("<skill-format>true</skill-format>"))
            .expect("skill meta message present");
        assert_eq!(
            skill_text,
            "<command-message>my-skill</command-message>\n\
<command-name>my-skill</command-name>\n\
<skill-format>true</skill-format>\nSKILL BODY",
            "metadata block then content"
        );
    }

    #[tokio::test]
    async fn missing_skill_is_skipped_no_message() {
        // G5: an unresolved skill injects NOTHING (claude logs the warn + skips).
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        ctx.agent_definition.agent_type = "my-agent".into();
        ctx.agent_definition.skills = vec!["nope".into()];
        ctx.skill_loader = Some(Arc::new(MockSkillLoader {
            known: "my-skill".into(),
            content_text: "SKILL BODY".into(),
        }));

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let msgs = api.captured();
        assert_eq!(msgs.len(), 1, "only the prompt seed (missing skill skipped): {msgs:?}");
    }

    #[tokio::test]
    async fn preload_order_additional_context_then_skills() {
        // Ordering parity (runAgent.ts 530→577): additionalContext message(s)
        // come BEFORE the skills message(s) in the seeded history.
        let api = CapturingApiClient::new();
        let mut ctx = loop_ctx(api.clone(), None, 2);
        ctx.prompt_messages = vec![ConversationMessage::user(MessageId::new(), "go".into())];
        ctx.agent_definition.agent_type = "my-agent".into();
        ctx.agent_definition.skills = vec!["my-skill".into()];
        ctx.hook_executor = Some(exec_with_start_context("ctx0").await);
        ctx.skill_loader = Some(Arc::new(MockSkillLoader {
            known: "my-skill".into(),
            content_text: "SKILL BODY".into(),
        }));

        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(8);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(16);
        drop(event_tx);
        run_subagent(ctx, event_rx, out_tx).await;
        let _ = drain(out_rx).await;

        let texts: Vec<String> = api.captured().iter().filter_map(user_text).collect();
        let ac_idx = texts
            .iter()
            .position(|t| t.contains("ctx0"))
            .expect("additionalContext present");
        let skill_idx = texts
            .iter()
            .position(|t| t.contains("<skill-format>"))
            .expect("skill present");
        assert!(
            ac_idx < skill_idx,
            "additionalContext must precede skills: {texts:?}"
        );
    }
}
