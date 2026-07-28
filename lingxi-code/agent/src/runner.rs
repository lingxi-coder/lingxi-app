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

fn completed_result_text(result: &serde_json::Value) -> Option<String> {
    if let Some(text) = result.get("text").and_then(serde_json::Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    let content = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(serde_json::Value::as_str))
                        .flatten()
                        .map(str::to_string)
                })
                .collect::<Vec<String>>()
        })?;
    if content.is_empty() {
        None
    } else {
        Some(content.join("\n"))
    }
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
    //
    // (cc 2.1.218 `mvo`) ORIGIN TRUST gate: registering these hooks installs
    // COMMANDS, so a definition whose folder has never been trusted must not get
    // them — the `--add-dir <untrusted-repo>` case, where the repo ships
    // `<dot>/agents/*.md` with a `hooks:` block. 2.1.217 registered
    // unconditionally; 2.1.218 skips + logs + counts instead.
    let frontmatter_cleanup = match &ctx.hook_executor {
        Some(he) if !ctx.agent_definition.frontmatter_hooks.is_empty() => {
            let cwd = ctx
                .cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if crate::hooks_trust::agent_hooks_origin_trusted(&ctx.agent_definition, &cwd) {
                he.register_agent_hooks(
                    ctx.agent_id,
                    &ctx.agent_definition.frontmatter_hooks,
                    true,
                )
                .await;
                Some((he.clone(), ctx.agent_id))
            } else {
                crate::hooks_trust::report_untrusted_hooks(
                    &ctx.agent_definition,
                    &cwd,
                    crate::hooks_trust::HooksTrustSurface::Subagent,
                    false,
                );
                None
            }
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
        // `agent-<id>.jsonl` leaf under this child's `transcript_subdir`.
        // FIX C: the production spawn path (handle.rs) now seeds `transcript_subdir`
        // to the REAL session-scoped dir
        // `<lingxi_home>/projects/<sanitize(cwd)>/<session>/subagents` (threaded
        // from the composition root via `with_hook_context`), so this is the true
        // `getAgentTranscriptPath` location — not the former `/tmp` placeholder.
        // Tests / minimal builds that wire no subagents dir keep the `/tmp` default.
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
                let mut terminal: Option<(&'static str, Option<String>)> = None;
                while let Some(ev) = proxy_rx.recv().await {
                    terminal = match &ev {
                        SubagentEvent::Completed { result, .. } => {
                            Some(("completed", completed_result_text(result)))
                        }
                        SubagentEvent::Failed { .. } => Some(("failed", None)),
                        // Killed does not fire SubagentStop (claude: an aborted
                        // child throws before reaching its stop hooks).
                        SubagentEvent::Killed { .. } => None,
                        // Non-terminal: keep whatever terminal we last saw.
                        _ => terminal,
                    };
                    // Best-effort forward; a closed receiver drops the rest.
                    if real.send(ev).await.is_err() {
                        break;
                    }
                }
                terminal
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
    if let (
        Some((he, agent_id, agent_type, session_id, cwd, agent_transcript_path)),
        Some((status, last_assistant_message)),
    ) = (agent_scoped_stop, terminal_status)
    {
        let stop_ctx = hooks::registry::HookContext {
            session_id,
            agent_id: Some(agent_id),
            cwd,
            agent_type: Some(agent_type.clone()),
            last_assistant_message,
            // FIX 2: SubagentStop carries the agent's own transcript path
            // (claude-code `agent_transcript_path`). See the tuple build above.
            agent_transcript_path: Some(agent_transcript_path),
            ..Default::default()
        };
        he.execute_agent_scoped(
            hooks::events::HookEvent::SubagentStop {
                agent_id,
                status: status.to_string(),
                // claude keys SubagentStop matchers on the subagent's type.
                agent_type,
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
/// Validate a captured `StructuredOutput` input against the workflow
/// `agent({schema})` JSON Schema (claude-code's Ajv `validateSchema`/`compile`
/// inside the StructuredOutput tool `call`). Returns a concise leaf-error string
/// on mismatch, `Ok(())` on a valid input.
///
/// BEHAVIORAL parity only: PASS/FAIL matches claude-code (full JSON-Schema
/// validation), but the error-detail bytes differ from Ajv's
/// `${instancePath}: ${message}` form (boon's native messages are unportable —
/// the same intentional divergence as `orchestrator::schema_validation`). The
/// byte-exact part is the `Output does not match required schema: ` WRAPPER the
/// caller prepends. A schema that fails to COMPILE is treated as PASS (a LingXi
/// schema bug must not block the model), matching the binary's stance.
fn validate_structured_output(
    schema_json: Option<&str>,
    input: &serde_json::Value,
) -> Result<(), String> {
    let Some(schema_str) = schema_json else {
        return Ok(());
    };
    let schema: serde_json::Value = match serde_json::from_str(schema_str) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    const URL: &str = "mem://structured-output-schema";
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    if compiler.add_resource(URL, schema).is_err() {
        return Ok(());
    }
    let sch = match compiler.compile(URL, &mut schemas) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    match schemas.validate(input, sch) {
        Ok(()) => Ok(()),
        Err(err) => {
            let mut out = Vec::new();
            flatten_schema_error(&err, &mut out);
            Err(out.join(", "))
        }
    }
}

/// Flatten a boon validation error into concise `at '<loc>': <kind>` leaves
/// (mirrors `orchestrator::schema_validation::flatten`).
fn flatten_schema_error(err: &boon::ValidationError, out: &mut Vec<String>) {
    if err.causes.is_empty() {
        let loc = err.instance_location.to_string();
        let loc = if loc.is_empty() {
            "(root)".to_string()
        } else {
            loc
        };
        out.push(format!("at '{loc}': {}", err.kind));
    } else {
        for cause in &err.causes {
            flatten_schema_error(cause, out);
        }
    }
}

/// The workflow `agent({schema})` StructuredOutput retry cap — claude-code
/// `Fe.MAX_STRUCTURED_OUTPUT_RETRIES ?? OBp`, where `OBp = 5`. The env override
/// matches the binary's `parseInt(process.env.MAX_STRUCTURED_OUTPUT_RETRIES||"5")`.
fn structured_output_retry_cap() -> u32 {
    std::env::var("MAX_STRUCTURED_OUTPUT_RETRIES")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(5)
}

pub(crate) fn resolve_model(ctx: &SubagentContext) -> String {
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

/// CC 2.1.207 subagent api-error classification (`CTy` / `zho`
/// `AgentApiErrorTerminationError`). Maps an [`llm_client::LlmError`] surfaced by
/// a mid-stream round-trip to `(errorKind, api_error_text)` when it is an API
/// TERMINATION whose kind is in `CTy = {rate_limit, overloaded, server_error}` —
/// the only kinds CC recovers as `api_error_partial` (every other kind rethrows
/// → `Failed`). The `api_error_text` is the model-visible `API Error: …` string
/// the query-loop finalize embeds into `zho.message`
/// (`yield tu({content:…,error:"server_error"})`); the finalize always tags the
/// synthesized message `server_error`, so the connection-close / stall variants
/// still qualify. The exact request-level rate-limit copy is NOT reproduced
/// here (it lives in the orchestrator's `errors.ts` port, which the agent crate
/// cannot depend on); a mid-stream 429 is surfaced with the server-error text,
/// which is what the finalize path yields.
fn classify_api_termination(e: &llm_client::LlmError) -> Option<(&'static str, &'static str)> {
    use llm_client::LlmError;
    match e {
        LlmError::Overloaded { .. } => Some((
            "overloaded",
            "API Error: Server error mid-response. The response above may be incomplete.",
        )),
        LlmError::RateLimited { .. } => Some((
            "rate_limit",
            "API Error: Server error mid-response. The response above may be incomplete.",
        )),
        LlmError::ProviderInternal => Some((
            "server_error",
            "API Error: Server error mid-response. The response above may be incomplete.",
        )),
        LlmError::Transport { .. } => Some((
            "server_error",
            "API Error: Connection closed mid-response. The response above may be incomplete.",
        )),
        LlmError::StreamInterrupted { .. } => Some((
            "server_error",
            "API Error: Response stalled mid-stream. The response above may be incomplete.",
        )),
        // Auth / permission / invalid-request / quota / context / TLS / cost /
        // unsupported-capability / model-unavailable are terminal — CC rethrows
        // (errorKind not in CTy), so they surface as `Failed`.
        _ => None,
    }
}

/// Build the CC 2.1.207 subagent `api_error_partial` result: the normal
/// completed result (final text blocks via the backward scan) with `cutoff_note`
/// prepended as the FIRST text block — claude's sync-agent recovery
/// (`On.content=[{type:"text",text:Dn},...On.content]`, status `"completed"`).
fn build_recovered_result(
    history: &[protocol::ConversationMessage],
    final_assistant_blocks: &[protocol::ContentBlock],
    cutoff_note: &str,
) -> serde_json::Value {
    let mut blocks = final_text_blocks(history, final_assistant_blocks);
    blocks.insert(0, cutoff_note.to_string());
    let content: Vec<serde_json::Value> = blocks
        .iter()
        .map(|t| serde_json::json!({ "type": "text", "text": t }))
        .collect();
    serde_json::json!({
        "content": content,
        "text": blocks.join("\n"),
        "stop_reason": serde_json::Value::Null,
    })
}

/// Assemble the CC 2.1.207 `cutoffNote` (`wTy`): the
/// `AgentApiErrorTerminationError` message (`Agent terminated early due to an
/// API error: {api_error_text}`) followed by the byte-locked incomplete-output
/// notice, joined by a blank line (two newlines) — the binary builds
/// `cutoffNote:` + "${e.message}\n\n" + "Everything below…".
fn build_cutoff_note(api_error_text: &str) -> String {
    format!(
        "Agent terminated early due to an API error: {api_error_text}\n\n\
Everything below is PARTIAL output recovered from the agent before it was cut off. The agent did NOT finish its task \u{2014} treat these results as incomplete."
    )
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
                    let mut blocks: Vec<ContentBlock> = Vec::with_capacity(1 + load.content.len());
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
                // (cc 2.1.218 `jYd`) Same literal-`\uXXXX` repair the orchestrator
                // applies — a subagent's tool inputs must be normalized too.
                let (input, _stats) =
                    llm_client::unicode_repair::repair_tool_input(name, input);
                Some(protocol::ContentBlock::ToolUse {
                    // The provider-issued id IS the canonical ToolUseId (byte
                    // parity with claude-code); the provider_id sidecar stays None.
                    id: protocol::ToolUseId::from(id.clone()),
                    name: name.clone(),
                    input,
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

/// Returns the companion note suffix (`yyo` in the binary, `nke` set) appended
/// to the allow-list-refusal error when a subagent tries to call a tool from
/// the "external companion" set that has been stripped from its pool.
///
/// Binary anchor: `function yyo(e,t,n,r)` at 203453056 in v2.1.186.
/// Trigger branch: `if (n && o && nke.has(o.name)) return …` where `n` = inside
/// a subagent, `o` = the resolved tool, `nke = HDd("external")`.
///
/// `HDd("external")` at 198067186:
/// ```text
/// new Set([lW, iO, qz, Qp, brt, tke, ...(e!=="ant"?[SI]:[]), Mh])
/// ```
/// Resolved (confirmed from binary):
/// - `lW`  = `"TaskOutput"`
/// - `iO`  = `"ExitPlanMode"`
/// - `qz`  = `"EnterPlanMode"`
/// - `Qp`  = `"AskUserQuestion"`
/// - `brt` = `"ConnectGitHub"`
/// - `tke` = `"WaitForMcpServers"`
/// - `SI`  = `"Workflow"` (non-ant only; `USER_TYPE !== "ant"`)
/// - `Mh`  = `"ScheduleWakeup"`
///
/// All subagent runners are inside a subagent by definition (`n` = true).
/// `is_ant` gates `Workflow` exactly like `HDd`'s ant-gate.
///
/// Returns `Some(note_suffix)` when the tool is in the `nke` set, `None`
/// otherwise. The note starts with `. ` to append to an in-progress sentence.
fn companion_note_for_disallowed_tool(tool_name: &str, is_ant: bool) -> Option<String> {
    // The static nke set elements always present for both ant and non-ant:
    const NKE_BASE: &[&str] = &[
        "TaskOutput",
        "ExitPlanMode",
        "EnterPlanMode",
        "AskUserQuestion",
        "ConnectGitHub",
        "WaitForMcpServers",
        "ScheduleWakeup",
    ];
    // "Workflow" is added for non-ant (HDd: `...(e!=="ant"?[SI]:[])`).
    let in_nke = NKE_BASE.contains(&tool_name) || (!is_ant && tool_name == "Workflow");
    if in_nke {
        // Binary §7 verbatim (leading `. ` — appended to an in-progress sentence):
        // `. ${toolName} is not available inside subagents. Complete the task with
        //  the tools provided and return findings to the orchestrator.`
        Some(format!(
            ". {tool_name} is not available inside subagents. Complete the task with the tools provided and return findings to the orchestrator."
        ))
    } else {
        None
    }
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
    let system: Option<String> = ctx
        .rendered_system_prompt
        .as_ref()
        .map(std::string::ToString::to_string);
    // Wire tool definitions advertised to the model on every round-trip (empty
    // when the spawner wired none). Cloned per round-trip below.
    //
    // Structured output (claude-code workflow `agent({schema})`): when a schema
    // was requested, inject a synthetic `StructuredOutput` tool whose
    // `input_schema` IS the schema and force the model to call it (`tool_choice`
    // via `messages_create_stream_forced`); its tool input is captured below as
    // the run's result.
    let mut tool_schemas = ctx.tool_schemas.clone();
    let force_structured_tool: Option<&'static str> = if let Some(schema_str) = &ctx.schema {
        let input_schema: serde_json::Value = serde_json::from_str(schema_str)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
        tool_schemas.push(serde_json::json!({
            "name": "StructuredOutput",
            "description":
                "Return the final result as a single structured object matching the required schema.",
            "input_schema": input_schema,
        }));
        Some("StructuredOutput")
    } else {
        None
    };
    // Captured when the model calls the synthetic `StructuredOutput` tool with an
    // input that VALIDATES against the schema — that input becomes the run's
    // result, and the loop terminates.
    let mut structured_result: Option<serde_json::Value> = None;
    // claude-code `agent({schema})` run-scoped counters: `kn` (failed
    // StructuredOutput validations) and `ft` (in-conversation nudges injected
    // when the model ends a turn without calling StructuredOutput). `Yr` is the
    // retry cap (`MAX_STRUCTURED_OUTPUT_RETRIES ?? 5`).
    let mut structured_failed_count: u32 = 0;
    let mut structured_nudge_count: u32 = 0;
    let structured_retry_cap = structured_output_retry_cap();
    // Per-agent tool allow-list enforced at dispatch (see below). Empty = no
    // restriction (the resolver has not filtered, e.g. `AgentToolPolicy::All`).
    // This is the dispatch-time guard the advertised set relies on: the
    // inherited `RegistryToolInvoker` itself does NOT check policy.
    let allowed_tools = ctx.allowed_tools.clone();

    // Seed history. A RESTORED agent is seeded from its persisted transcript
    // and nothing else: fork context, prompt and preload are all already inside
    // that history (they were persisted on the original run), so re-adding them
    // would duplicate context the agent has seen and re-fire `SubagentStart`
    // for a run that began in another process.
    let mut history: Vec<ConversationMessage> = Vec::new();
    if let Some(resumed) = &ctx.resumed_history {
        history.extend(resumed.iter().cloned());
    } else {
        // Fork-context prefix (if any) followed by the prompt.
        if let Some(fork) = &ctx.fork_context_messages {
            history.extend(fork.iter().cloned());
        }
        history.extend(ctx.prompt_messages.iter().cloned());

        // G4 + G5 (claude runAgent.ts:530-646): SubagentStart-hook
        // additionalContext injection then frontmatter skills preload, appended
        // to the child's INITIAL messages before the first turn. Strict no-op
        // (no extra messages) when neither `hook_executor` nor `skill_loader` is
        // wired, so legacy / test builds keep a byte-identical history.
        // (Frontmatter-hook registration is done at the `run_subagent`
        // dispatcher for guaranteed cleanup.)
        history.extend(build_preload_messages(&ctx).await);
    }

    // Per-agent transcript. Appended by WATERMARK — everything in
    // `history[written..]` is flushed at each turn-set boundary — rather than
    // at each `history.push` site. There are five of those and a future sixth
    // would silently skip persistence; a watermark cannot miss one.
    //
    // The flush points are the turn-set boundaries because those are exactly
    // the RESUME boundaries: a persistent agent parks between turn-sets, so an
    // on-disk transcript that is complete at every park is complete at every
    // point anything could resume from.
    let transcript = ctx.transcript_fs.clone().map(|fs| {
        crate::transcript::AgentTranscriptWriter::new(
            ctx.transcript_subdir
                .join(format!("agent-{agent_id}.jsonl")),
            agent_id,
            fs,
        )
    });
    // A RESTORED run starts with its transcript already on disk, so its
    // watermark starts past the recovered messages — otherwise the first flush
    // would append the whole conversation a second time. A fresh run starts at
    // 0: its seeded prompt and preload are new and must be persisted.
    let mut transcript_written: usize = if ctx.resumed_history.is_some() {
        history.len()
    } else {
        0
    };

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
            // Placement remains at the TOP of the turn (stop before spending)
            // rather than TS's post-message check, so an already-over-budget
            // child makes zero additional round-trips. The denial string uses
            // the same current/maximum bytes as Claude Code 2.1.217's background
            // task budget halt when the configured ceiling is available.
            if let Some(b) = &budget {
                if let Err(traits::budget::BudgetError::Exceeded { current_nano_usd }) =
                    b.check_and_charge(0).await
                {
                    // Stop with the 2.1.217 background-agent budget string.
                    #[allow(clippy::cast_precision_loss)]
                    let dollars = current_nano_usd as f64 / 1_000_000_000.0;
                    let error = b.max_session_nano_usd().map_or_else(
                        || format!("Budget exceeded (${dollars:.2}); stopped."),
                        |limit_nano_usd| {
                            let whole = limit_nano_usd / 1_000_000_000;
                            let fractional = limit_nano_usd % 1_000_000_000;
                            let maximum = if fractional == 0 {
                                whole.to_string()
                            } else {
                                let fraction = format!("{fractional:09}");
                                format!("{whole}.{}", fraction.trim_end_matches('0'))
                            };
                            format!(
                                "Budget limit reached (${dollars:.2} of ${maximum}); stopping background agents."
                            )
                        },
                    );
                    let _ = out_tx.send(SubagentEvent::Failed { agent_id, error }).await;
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
            // Per-request thinking-effort (claude-code `me.effort`): the subagent's
            // resolved effort (its definition's, possibly overridden by a workflow
            // `agent({effort})` opt at spawn) → `output_config.effort`.
            let effort_wire = ctx
                .agent_definition
                .effort
                .as_ref()
                .map(crate::definition::AgentEffort::to_wire);
            // (M9 cc2.1.198 wake-on-message) Captured by the wake arm in the
            // select below and appended to `history` HERE, before the next
            // `api_call` is built, because the in-flight future immutably
            // borrows `history` inside the select.
            let mut wake_message: Option<String> = None;
            let response = loop {
                // (M9) A wake message injected below rides into the next
                // round-trip as a user turn (mirrors the persist-park path,
                // which appends without emitting a Message event).
                if let Some(content) = wake_message.take() {
                    history.push(ConversationMessage::user(MessageId::new(), content));
                }
                let api_call = async {
                    // Provider routing (dual-LLM dual-PROVIDER): thread the
                    // per-spawn `model_profile` as the api client's `profile` so the
                    // round-trip targets the candidate's resolved provider. `None`
                    // ⇒ default/unscoped resolution (legacy). The `_in` variants
                    // default to the profile-less methods, so a client that only
                    // implements the legacy seam is unaffected.
                    //
                    // The error carries the partial content blocks completed
                    // before the failure so the arm below can SALVAGE them (CC
                    // 2.1.207 `api_error_partial`). A connect-phase error yields no
                    // partial (empty vec); a mid-stream error yields whatever
                    // blocks were finalized.
                    let profile = ctx.model_profile.as_deref();
                    let stream = if let Some(forced) = force_structured_tool {
                        api_client
                            .messages_create_stream_forced_in(
                                &model,
                                profile,
                                system.as_deref(),
                                history.clone(),
                                tool_schemas.clone(),
                                Some(forced),
                                effort_wire.clone(),
                            )
                            .await
                            .map_err(|e| (Vec::new(), e))?
                    } else {
                        api_client
                            .messages_create_stream_in(
                                &model,
                                profile,
                                system.as_deref(),
                                history.clone(),
                                tool_schemas.clone(),
                                effort_wire.clone(),
                            )
                            .await
                            .map_err(|e| (Vec::new(), e))?
                    };
                    crate::accumulator::accumulate_stream_salvaging(stream).await
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
                            // (M9 cc2.1.198 wake-on-message) messaging a stuck
                            // persistent teammate wakes it: drop the in-flight
                            // future, append the message to history (above), and
                            // re-issue the round-trip NOW — previously this fell
                            // into the catch-all below and silently DISCARDED the
                            // text. Persistent (teammate) runners only; one-shot
                            // subagents keep the legacy drop-and-retry semantics.
                            Some(engine::Event::UserMessage { content, .. })
                                if ctx.persistent =>
                            {
                                wake_message = Some(content);
                                continue;
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
                Err((partial_blocks, e)) => {
                    // CC 2.1.207 subagent `api_error_partial` recovery
                    // (`Wyd`/`wTy`): when the round-trip is cut off by an API
                    // TERMINATION whose kind is in `CTy`
                    // ({rate_limit,overloaded,server_error}) AND the agent has
                    // already produced content (a prior completed turn OR blocks
                    // salvaged from THIS turn before the cutoff), return the
                    // partial work as a `completed` result with the incomplete-
                    // output `cutoffNote` prepended — instead of failing the
                    // whole tool call and discarding everything the child did.
                    // Every other kind (auth/invalid/quota/…) or an empty
                    // transcript rethrows as `Failed`, exactly as CC does.
                    let salvaged = translate_response_blocks(&partial_blocks);
                    match classify_api_termination(&e) {
                        Some((_error_kind, api_error_text))
                            if !final_text_blocks(&history, &salvaged).is_empty() =>
                        {
                            let cutoff_note = build_cutoff_note(api_error_text);
                            let result = build_recovered_result(&history, &salvaged, &cutoff_note);
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
                            return;
                        }
                        _ => {
                            let _ = out_tx
                                .send(SubagentEvent::Failed {
                                    agent_id,
                                    error: format!("subagent api error: {e}"),
                                })
                                .await;
                            return;
                        }
                    }
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
            let tool_uses: Vec<(
                protocol::ToolUseId,
                String,
                serde_json::Value,
                Option<String>,
            )> = assistant_blocks
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
            total_tool_use_count = total_tool_use_count.saturating_add(tool_uses.len() as u64);

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
                    // Structured output: the synthetic `StructuredOutput` tool is not
                    // dispatched — its input IS the run's result, but ONLY when it
                    // VALIDATES against the schema (claude-code Ajv validation inside
                    // the tool `call`). A valid input is captured + a benign result fed
                    // back (the loop terminates below); an INVALID input feeds back an
                    // `is_error` ToolResult (`Output does not match required schema: …`)
                    // and increments the failed-validation count `kn` — the model sees
                    // the error and retries on the next turn (the `tool_use` stop keeps
                    // the loop going), up to the retry cap checked after dispatch.
                    if force_structured_tool == Some(name.as_str()) {
                        match validate_structured_output(ctx.schema.as_deref(), input) {
                            Ok(()) => {
                                structured_result = Some(input.clone());
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    // claude's StructuredOutput tool returns
                                    // `data: "Structured output provided successfully"`
                                    // as the tool-result content (binary 2.1.195
                                    // strings :311431 / :438281).
                                    content: "Structured output provided successfully".to_string(),
                                    is_error: false,
                                    provider_tool_use_id: provider_id.clone(),
                                    content_blocks: None,
                                });
                            }
                            Err(detail) => {
                                structured_failed_count = structured_failed_count.saturating_add(1);
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: format!(
                                        "Output does not match required schema: {detail}"
                                    ),
                                    is_error: true,
                                    provider_tool_use_id: provider_id.clone(),
                                    content_blocks: None,
                                });
                            }
                        }
                        continue;
                    }
                    // Allow-list guard: when `allowed_tools` is non-empty, a model
                    // request for a tool outside it is refused WITHOUT dispatching
                    // (the inherited `RegistryToolInvoker` would otherwise run any
                    // registered tool by name). Surfaced as an `is_error` ToolResult
                    // so the model sees the refusal and can recover, mirroring how a
                    // tool error is fed back. Empty `allowed_tools` skips the guard.
                    if !allowed_tools.is_empty() && !allowed_tools.iter().any(|t| t == name) {
                        // yyo companion note (binary v2.1.186 §7): when the blocked tool
                        // is in the `nke` external companion set, append the byte-exact
                        // guidance suffix so the model knows the tool is a subagent
                        // boundary, not a typo. Gate on USER_TYPE like HDd.
                        let is_ant = std::env::var("USER_TYPE").is_ok_and(|v| v == "ant");
                        let note =
                            companion_note_for_disallowed_tool(name, is_ant).unwrap_or_default();
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: format!(
                                "tool {name:?} is not in this agent's allowed tools{note}"
                            ),
                            is_error: true,
                            provider_tool_use_id: provider_id.clone(),
                            content_blocks: None,
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
                        // R1: an async (backgrounded) subagent runs its tools with
                        // is_non_interactive_session=true (claude-code runAgent.ts:668-672).
                        is_async: ctx.is_async,
                        // Whether this worker may surface a permission prompt to the
                        // user — drives the worker attribution on the prompt dialog
                        // (claude-code's worker permission badge).
                        can_show_permission_prompts: ctx.can_show_permission_prompts,
                        // Per-agent cwd (worktree isolation / explicit cwd) → the
                        // dispatched tools' working directory.
                        cwd: ctx.cwd.clone(),
                        // The REAL `tool_use` block id of THIS dispatching call
                        // (claude-code `createCanUseTool(toolUseID)`): threaded into
                        // the gate's `PermissionCheckContext` so a subagent's stdio
                        // `can_use_tool` prompt carries the byte-faithful id instead
                        // of a freshly minted one — matching the main loop's path.
                        tool_use_id: Some(tool_use_id.as_str().to_string()),
                        // This subagent's own recursion depth (claude `agentContext.depth`)
                        // → mapped into the dispatched tool's `ToolUseContext.depth`, so a
                        // nested `Agent` call computes the grandchild's depth (`depth+1`)
                        // and the resolver applies the configured spawn-depth cap.
                        depth: ctx.depth,
                        observer: ctx
                            .observer
                            .as_ref()
                            .filter(|observer| {
                                observer.observe_subagents
                                    && ctx.depth < crate::observer::DEFAULT_OBSERVER_FANOUT_DEPTH
                            })
                            .cloned(),
                        // This subagent's OWN resolved main-loop model — so a NESTED
                        // `Agent` tool call resolves its child's model against THIS
                        // subagent's model (claude-code `runAgent.ts:678` seeds each
                        // child's `mainLoopModel: resolvedAgentModel`), read by the
                        // recursive AgentTool via `ToolUseContext.options.main_loop_model`
                        // (claude `AgentTool.tsx:418`). The RegistryToolInvoker maps this
                        // into that field; the definition's model is already the concrete
                        // Explicit id resolved at spawn time.
                        parent_model: Some(resolve_model(&ctx)),
                        // This subagent's EFFECTIVE permission mode (claude-code
                        // 2.1.207 Agent `mode` → the child's
                        // `toolPermissionContext.mode`, `wKe`/`ve`): threaded into
                        // the dispatch gate's `PermissionCheckContext` so the child's
                        // tool calls authorize under it (a `mode:"plan"` child gates
                        // mutations while reads stay frictionless). `None` = inherit
                        // the gate's live/boot mode.
                        mode_override: ctx.permission_mode_override.clone(),
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
                                content_blocks: None,
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
                                content_blocks: None,
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

            // claude `agent({schema})`: `kn>0 && kn>=Yr && rn===undefined` → throw the
            // retry-cap-exceeded error (surfaced here as a terminal `Failed`). The
            // model's StructuredOutput validations have exhausted the cap (`Yr`,
            // `MAX_STRUCTURED_OUTPUT_RETRIES ?? 5`) with no valid output captured.
            if force_structured_tool.is_some()
                && structured_result.is_none()
                && structured_failed_count > 0
                && structured_failed_count >= structured_retry_cap
            {
                let calls = if structured_failed_count == 1 {
                    "call"
                } else {
                    "calls"
                };
                let _ = out_tx
                .send(SubagentEvent::Failed {
                    agent_id,
                    error: format!(
                        "agent({{schema}}): StructuredOutput retry cap ({structured_retry_cap}) exceeded \u{2014} {structured_failed_count} failed {calls} with no valid output"
                    ),
                })
                .await;
                return;
            }

            // Loop disposition. Continue ONLY when the model asked to use tools and
            // actually emitted some; every other case is terminal — including
            // `end_turn`, a stream with no stop_reason (`None`), and any other
            // reason (max_tokens / stop_sequence / pause_turn / refusal), even when
            // the truncated turn carried tool_uses we just dispatched.
            // A captured structured output terminates the run (it IS the result),
            // even though the forced tool call carries a `tool_use` stop reason.
            let should_continue = stop_reason.as_deref() == Some("tool_use")
                && !tool_uses.is_empty()
                && structured_result.is_none();
            if !should_continue {
                // claude `agent({schema})` SubagentStop nudge: when the model ends a
                // turn without a captured (valid) StructuredOutput, inject an
                // in-conversation nudge and run another turn — up to 2 nudges (`ft`).
                // After the 2nd, give up with the byte-exact "completed without
                // calling" error. (The validation-RETRY case — StructuredOutput called
                // but its input failed — does NOT reach here: that turn's `tool_use`
                // stop keeps `should_continue` true, so the model retries until the
                // retry cap above fires.)
                if force_structured_tool.is_some() && structured_result.is_none() {
                    if structured_nudge_count < 2 {
                        structured_nudge_count = structured_nudge_count.saturating_add(1);
                        let nudge = ConversationMessage::user(
                        MessageId::new(),
                        "You did not call StructuredOutput. You MUST call StructuredOutput to return your answer \u{2014} the tool input IS your answer. Call it now.".to_string(),
                    );
                        history.push(nudge.clone());
                        emit_message(&out_tx, agent_id, &nudge).await;
                        // Re-run the turn loop with the nudge appended (still bounded
                        // by `max_turns`).
                        continue;
                    }
                    let _ = out_tx
                    .send(SubagentEvent::Failed {
                        agent_id,
                        // Byte-locked to claude 2.1.195 (binary strings :331985 /
                // :514141, the workflow `agent({schema})` runtime): the give-up
                // wording is SINGULAR "(after in-conversation nudge)" with no
                // count. (claude's exact in-conversation nudge body and its
                // nudge count are not discoverable static strings in the binary,
                // so the port's nudge text + 2× retry are left as-is.)
                error: "agent({schema}): subagent completed without calling StructuredOutput (after in-conversation nudge)".to_string(),
                    })
                    .await;
                    return;
                }
                // claude `finalizeAgentTool`: the result's `content` is the LAST
                // assistant message's text blocks, with a backward-scan fallback to
                // the most recent assistant message that has text when the final turn
                // was tool-only (agentToolUtils.ts:304-317). `history` already holds
                // the current assistant turn (pushed above) + every prior turn.
                // A `schema` run returns the captured StructuredOutput tool input.
                let result = match structured_result.take() {
                    Some(structured) => structured,
                    None => {
                        build_completed_result(&history, &assistant_blocks, stop_reason.as_deref())
                    }
                };
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

        // Flush the turn-set's messages to the per-agent transcript. Runs for
        // BOTH dispositions below — a one-shot subagent's transcript is just as
        // much a record as a persistent one's, and the `SubagentStop` hook
        // reports its path either way. Best-effort: a transcript write failure
        // must never mask the agent's result.
        if let Some(writer) = &transcript {
            for message in &history[transcript_written..] {
                if writer.record(message).await.is_err() {
                    break;
                }
                transcript_written += 1;
            }
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
#[path = "runner_test.rs"]
mod runner_test;
