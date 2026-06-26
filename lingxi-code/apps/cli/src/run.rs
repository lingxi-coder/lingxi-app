//! One-shot conversation: feed the prompt, run the orchestrator, print
//! results, exit. Also hosts the resume entrypoint.
//!
//! (M7-12) `--resume` is a three-way split routed by the pure [`resume_route`]:
//!   - `--resume <uuid>`            → [`run_resume_by_id`] (load by id).
//!   - `--resume` (no id) + TTY     → [`run_resume_iocraft`] (the iocraft
//!     Resume screen over the M5-08 loader).
//!   - `--resume` (no id) + non-TTY → [`run_resume_stdio_picker`] (the
//!     unchanged M5-08 `select_session_interactive` stdio fallback).

use crate::argv::Argv;
use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use crate::stream_json::{build_init_params, permission_mode_str, StreamJsonStream};
use crate::control_plane::StdioControlPlane;
use crate::stream_json_input::{
    content_to_prompt, control_frame_request_id, control_request_subtype, emit_replay_ack,
    spawn_stdin_router, ControlPlaneWriter, StdinChannels,
};
use command_api::format_description_with_source;
use permission;
use serde_json::json;
use session::jsonl::loader::{
    list_recent_sessions, load_session, select_session_interactive, LoaderError, SessionMetadata,
};
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::sync::Arc;
use traits::{FileSystem, McpStatus, OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};

/// Drive a one-shot conversation: either a `/slash-command` or a normal
/// prompt that runs through the orchestrator turn loop.
pub async fn run_oneshot(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let prompt = argv.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        // Byte-parity with claude-code print.ts: the empty-input error in print
        // mode is this exact line, then exit 1 (ARGV_ERROR == 1 post-flip).
        eprintln!(
            "Error: Input must be provided either through stdin or as a prompt argument when using --print"
        );
        return exit_codes::ARGV_ERROR;
    }

    // Slash branch — bypasses the API entirely.
    if prompt.starts_with('/') {
        return run_slash_command(&prompt, runtime, sink).await;
    }

    // Structured-output branch (`--json-schema`): the model is forced through the
    // `StructuredOutput` tool (forced tool_choice wired in `engine_desktop::build`);
    // we validate its captured result against the schema and retry. Only active
    // when `build()` surfaced a capture slot (i.e. `--json-schema` + `--print`).
    if let Some(slot) = runtime.structured_output_slot.clone() {
        if let Some(schema) = argv
            .json_schema
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            return run_structured_output(runtime, &prompt, &slot, &schema, sink).await;
        }
    }

    // Non-slash branch — drive the orchestrator turn loop. Without a real
    // ANTHROPIC_API_KEY this returns 401; we surface the error verbatim.
    sink.turn_start().await;
    match runtime.orchestrator.run_turn(&prompt).await {
        Ok(_outcome) => exit_codes::SUCCESS,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Drive a one-shot `--output-format stream-json` conversation.
///
/// Emits `system/init` → `system/status` → [streaming frames via the trait
/// impls on `stream`] → exit.  The `stream` is already installed as the
/// orchestrator's output sink (set up in `lib.rs`).
pub async fn run_stream_json_print(
    argv: &Argv,
    runtime: &Runtime,
    stream: Arc<StreamJsonStream>,
    permission_mode: permission::PermissionMode,
) -> i32 {
    let prompt = argv.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        // Byte-parity with claude-code print.ts (see run_oneshot).
        eprintln!(
            "Error: Input must be provided either through stdin or as a prompt argument when using --print"
        );
        return exit_codes::ARGV_ERROR;
    }

    // Slash commands bypass the API; stream-json doesn't apply.
    if prompt.starts_with('/') {
        eprintln!("lingxi-cli: slash commands not supported in stream-json mode");
        return exit_codes::ARGV_ERROR;
    }

    // Collect the real session_id and model from the orchestrator after build.
    let (session_id_str, model_str) = {
        let session_handle = runtime.orchestrator.session();
        let session = session_handle.lock().await;
        (session.session_id.to_string(), session.model.clone())
    };

    // Collect tool names (Agent → Task SDK rename handled inside build_init_params).
    let tool_names = runtime.orchestrator.tool_names();

    // ── P2b: real init-frame population ─────────────────────────────────────

    // MCP servers: name + status string from the live orchestrator registry.
    let mcp_servers: Vec<(String, String)> = runtime
        .orchestrator
        .list_mcp_servers()
        .await
        .into_iter()
        .map(|s| {
            let status_str = match s.status {
                McpStatus::Connected => "connected".to_string(),
                McpStatus::Disconnected => "disconnected".to_string(),
                McpStatus::Error(_) => "error".to_string(),
            };
            (s.name, status_str)
        })
        .collect();

    // Slash commands + skills: read from the shared command registry.
    // slash_commands = all registered commands (sorted by name).
    // skills = commands loaded_from=="skills" (the model-invocable subset
    //          contributed by plugin skill files).
    let (slash_commands, skills) = {
        let reg = runtime.dispatcher.registry();
        let reg_guard = reg.read().await;
        let mut all_cmds: Vec<String> = reg_guard
            .list_all()
            .into_iter()
            .map(|c| c.name.clone())
            .collect();
        all_cmds.sort();
        let mut skill_names: Vec<String> = reg_guard
            .list_all()
            .into_iter()
            .filter(|c| c.loaded_from.as_deref() == Some("skills"))
            .map(|c| c.name.clone())
            .collect();
        // sort for determinism.
        skill_names.sort();
        drop(reg_guard);
        (all_cmds, skill_names)
    };

    // Agents: names from the agent catalog via the orchestrator handle.
    let agents: Vec<String> = runtime
        .orchestrator
        .list_agents()
        .await
        .into_iter()
        .map(|a| a.name)
        .collect();
    // list_agents already sorts; no re-sort needed.

    // Plugins: no clean surface from the CLI Runtime — the PluginManager is
    // local to engine-desktop and not re-exported. Stays [] with this note.
    // The plugin name/path/source would need engine_desktop::DesktopRuntime
    // to expose a `loaded_plugins()` accessor (follow-up).
    let plugins: Vec<(String, String, String)> = vec![];

    // Build the init parameters now that the runtime is available.
    let init_params = build_init_params(
        &session_id_str,
        tool_names,
        mcp_servers,
        &model_str,
        permission_mode_str(permission_mode),
        slash_commands,
        agents,
        skills,
        plugins,
        "default", // output_style
        None,   // memory_auto_path
        "off",  // fast_mode_state
    );

    // Thread the real params + session_id into the stream.
    stream.set_init_params(init_params).await;

    // Phase 0 (0a): start the single-writer stdout drain task before any frames
    // are emitted. All subsequent emit_* calls push onto the mpsc channel;
    // the drain task is the sole stdout writer.
    stream.ensure_drain_started().await;

    // ① system/init frame
    stream.emit_init().await;

    // ② system/status frame (status: "requesting")
    stream.emit_status().await;

    // ③ Run the turn — streaming callbacks (emit_text / emit_tool_call /
    //    emit_message_start / emit_message_boundary) fire on the stream.
    let turn_result = runtime.orchestrator.run_turn(&prompt).await;

    // ④ Emit the result frame.
    let cost = runtime.orchestrator.snapshot_cost().await;
    let result_text = stream.get_last_result_text().await;
    let model = {
        let session_handle = runtime.orchestrator.session();
        let session = session_handle.lock().await;
        session.model.clone()
    };

    // Betas: LingXi doesn't yet track active betas in SessionState, so we
    // infer from the model string itself: if the model carries "[1m]" the
    // 1M context beta is effectively active and the modelUsage key should
    // reflect the real contextWindow = 1_000_000. The `context_window_for_model`
    // helper already handles the "[1m]" substring check — no beta list needed.
    let betas: Vec<String> = vec![];

    if turn_result.is_err() {
        let err_msg = turn_result.unwrap_err().to_string();
        stream
            .emit_result_error(
                "error_during_execution",
                vec![err_msg],
                &cost,
                &model,
                "off",
                &betas,
            )
            .await;
        exit_codes::RUNTIME_ERROR
    } else {
        stream
            .emit_result_success(&result_text, "end_turn", &cost, &model, "off", &betas)
            .await;
        exit_codes::SUCCESS
    }
}

/// Dispatch a single `control_request` frame to the appropriate handler.
///
/// This is a synchronous function called from inside the async ctrl-dispatcher
/// task. All initialization data that requires `.await` must be pre-collected
/// before the task is spawned and passed in as owned values.
///
/// Phase 1 implements: `initialize` (full payload) and `interrupt` (cancel signal).
/// All other subtypes return the byte-exact fallthrough error.
#[allow(clippy::too_many_arguments)]
async fn dispatch_control_request(
    subtype: &str,
    request_id: &str,
    frame: &serde_json::Value,
    writer: &ControlPlaneWriter,
    cancel_tx: &tokio::sync::watch::Sender<bool>,
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    task_registry: &Arc<tasks::registry::TaskRegistry>,
    end_notify: &Arc<tokio::sync::Notify>,
    init_commands: &[serde_json::Value],
    init_agents: &[serde_json::Value],
    init_models: &[serde_json::Value],
    init_account: &serde_json::Value,
) {
    // Request body fields live at `frame.request.<field>` (already key-normalized).
    let field = |k: &str| frame.get("request").and_then(|r| r.get(k));

    match subtype {
        "initialize" => {
            // SDKControlInitializeResponse keys ONLY (commands, agents,
            // output_style, available_output_styles, models, account, pid). The
            // binary does NOT emit a `feedback_survey_config` here — that key was
            // fabricated and is dropped (it is not in the schema).
            let payload = json!({
                "commands": init_commands,
                "agents": init_agents,
                "output_style": "default",
                "available_output_styles": ["default", "Proactive", "Explanatory", "Learning"],
                "models": init_models,
                "account": init_account,
                "pid": std::process::id(),
            });
            writer.reply_success(request_id, Some(payload));
        }
        "interrupt" => {
            // §2.2 #1: cancel the per-turn token, then ack.
            let _ = cancel_tx.send(true);
            writer.reply_success(request_id, None);
        }
        "set_model" => {
            // §2.2 #5: `"default"` (or an absent model) resolves to the session
            // default model and APPLIES it — so a client can revert a prior
            // `set_model` override (claude-code re-resolves via
            // getDefaultMainLoopModel() and calls setMainLoopModelOverride).
            let requested = field("model").and_then(|v| v.as_str()).unwrap_or("default");
            let default_model = orchestrator.default_model();
            let target = if requested == "default" {
                default_model.as_str()
            } else {
                requested
            };
            match orchestrator.switch_model(target, None).await {
                Ok(()) => writer.reply_success(request_id, None),
                Err(e) => writer.reply_error(request_id, &e.to_string()),
            }
        }
        "mcp_status" => {
            // §2.2 #7: `{mcpServers: [...]}`.
            let servers: Vec<serde_json::Value> = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .map(|s| {
                    let status = match s.status {
                        McpStatus::Connected => "connected",
                        McpStatus::Disconnected => "disconnected",
                        McpStatus::Error(_) => "error",
                    };
                    json!({"name": s.name, "status": status})
                })
                .collect();
            writer.reply_success(request_id, Some(json!({"mcpServers": servers})));
        }
        "get_context_usage" => {
            // §2.2 #9: token-budget breakdown (shape inferred — not byte-dumped).
            let (used, total) = orchestrator.context_window_usage().await;
            writer.reply_success(
                request_id,
                Some(json!({
                    "usedTokens": used,
                    "maxTokens": total
                })),
            );
        }
        "get_session_cost" => {
            // §2.2 #10: `{text}` (format inferred — not byte-dumped).
            let cost = orchestrator.snapshot_cost().await;
            writer.reply_success(
                request_id,
                Some(json!({"text": format!("Total cost: ${:.4}", cost.total_usd)})),
            );
        }
        "get_usage" => {
            // §2.2 #11: usage snapshot (shape inferred — not byte-dumped).
            let cost = orchestrator.snapshot_cost().await;
            writer.reply_success(
                request_id,
                Some(json!({
                    "input_tokens": cost.input_tokens,
                    "output_tokens": cost.output_tokens,
                    "cache_read_tokens": cost.cache_read_tokens,
                    "cache_creation_tokens": cost.cache_creation_tokens,
                    "total_tokens": cost.total_tokens
                })),
            );
        }
        "stop_task" => {
            // §2.2 #38: best-effort kill; not_found/not_running ⇒ success `{}`.
            if let Some(task_id) = field("task_id").and_then(|v| v.as_str()) {
                let _ = task_registry.kill(task_id).await;
            }
            writer.reply_success(request_id, Some(json!({})));
        }
        "set_permission_mode" => {
            // §2.2 #4: the net-new runtime mode-mutation surface. The gate
            // parses + validates the wire mode and applies it live; success
            // echoes `{mode}`, an invalid/disallowed mode returns an error frame.
            let mode = field("mode").and_then(|v| v.as_str()).unwrap_or("default");
            match orchestrator.set_permission_mode(mode).await {
                Ok(()) => writer.reply_success(request_id, Some(json!({"mode": mode}))),
                Err(e) => writer.reply_error(request_id, &e),
            }
        }
        "end_session" => {
            // §2.2 #2: abort the in-flight turn, ack, then break the loop.
            let _ = cancel_tx.send(true);
            writer.reply_success(request_id, None);
            end_notify.notify_one();
        }
        // The orchestrator-free arms (set_max_thinking_tokens, get_binary_version,
        // rename_session, message_rated, seed_read_state), the CLI-originated
        // guard subtypes (no-reply), and the byte-exact `Unsupported control
        // request subtype` fallthrough are pure — classified by
        // `pure_control_response` so the wire shapes are unit-testable without a
        // live orchestrator.
        other => match pure_control_response(other, frame) {
            PureControlReply::Success(payload) => writer.reply_success(request_id, payload),
            PureControlReply::Error(msg) => writer.reply_error(request_id, &msg),
            // CLI-originated subtype seen inbound — no control_response (see below).
            PureControlReply::Ignore => {}
        },
    }
}

/// Reply for a pure (orchestrator-free) control arm.
#[derive(Debug, PartialEq)]
enum PureControlReply {
    /// `control_response` success; `None` ⇒ inner `response` key omitted.
    Success(Option<serde_json::Value>),
    /// `control_response` error with this message.
    Error(String),
    /// No `control_response` at all — a CLI-originated subtype seen inbound that
    /// the binary handles as a top-of-chain guard, never in the server switch.
    Ignore,
}

/// Classify the control arms that need no async orchestrator/registry access,
/// including the byte-exact `Unsupported control request subtype` fallthrough.
///
/// Several arms are accept-and-ack approximations per spec §2.2 `[T (partial)]`:
/// `set_max_thinking_tokens` and `seed_read_state` lack a storage seam (acked,
/// not persisted); `rename_session` validates non-empty but defers persistence.
fn pure_control_response(subtype: &str, frame: &serde_json::Value) -> PureControlReply {
    let field = |k: &str| frame.get("request").and_then(|r| r.get(k));
    match subtype {
        // §2.2 #6: accepted + acked; value not persisted (no session field yet).
        "set_max_thinking_tokens" => PureControlReply::Success(None),
        // §2.2 #8: `{version, buildTime}`.
        "get_binary_version" => PureControlReply::Success(Some(json!({
            "version": traits::CLAUDE_CODE_VERSION,
            "buildTime": ""
        }))),
        // §2.2 #41: trim + validate; error on empty; persistence deferred.
        "rename_session" => {
            let title = field("title").and_then(|v| v.as_str()).unwrap_or("");
            if title.trim().is_empty() {
                PureControlReply::Error("title must be non-empty".to_string())
            } else {
                PureControlReply::Success(None)
            }
        }
        // §2.2 #45: telemetry-only; ack with `{}`.
        "message_rated" => PureControlReply::Success(Some(json!({}))),
        // §2.2 #21: seed read-state cache; errors swallowed ⇒ empty ack (no seam).
        "seed_read_state" => PureControlReply::Success(None),
        // CLI-ORIGINATED subtypes: `can_use_tool` / `request_user_dialog` /
        // `elicitation` are CLIENT→SERVER frames the CLI itself SENDS (their
        // `control_response` is handled by the resolver task). The binary checks
        // them as top-of-chain GUARDS routed to the StructuredIO pending-request
        // path — they NEVER enter this server switch nor reach the Unsupported
        // fallthrough. A well-behaved host never sends them inbound as a
        // control_request, so we emit NO control_response rather than erroring.
        "can_use_tool" | "request_user_dialog" | "elicitation" => PureControlReply::Ignore,
        // The binary fallthrough for every unhandled / deep [D] subtype.
        _ => PureControlReply::Error(format!("Unsupported control request subtype: {subtype}")),
    }
}

/// Map the raw inner `control_response.response` permission payload onto a
/// [`permission::gate::PermissionOutcome`] for orphaned-tool recovery.
///
/// Mirrors the allow/deny shape of `StdioControlPermissionGate::map_payload`
/// but is deliberately LENIENT where the live gate is strict: an `allow`
/// WITHOUT `updatedInput` is honoured (falling back to the original tool input)
/// rather than rejected — matching claude-code's `handleOrphanedPermission`,
/// which logs a warning and uses the original input when `updatedInput` is
/// `undefined` (queryHelpers.ts:262-272), instead of `map_payload`'s strict
/// §3.3 "missing updatedInput" deny used for live responses.
fn orphan_decision_from_payload(
    payload: &serde_json::Value,
) -> permission::gate::PermissionOutcome {
    use permission::gate::PermissionOutcome;
    match payload.get("behavior").and_then(serde_json::Value::as_str) {
        Some("allow") => {
            // Carry `updatedInput` only when it is a non-empty object (claude-code
            // applies it "when it has keys"); otherwise fall back to the original.
            let updated_input = match payload.get("updatedInput") {
                Some(serde_json::Value::Object(m)) if !m.is_empty() => {
                    Some(serde_json::Value::Object(m.clone()))
                }
                _ => None,
            };
            PermissionOutcome::Allow {
                updated_input,
                permission_updates: vec![],
            }
        }
        Some("deny") => PermissionOutcome::Deny {
            reason: payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Tool permission denied")
                .to_string(),
        },
        // Any non-allow/deny behaviour is a schema-invalid result; deny safely
        // rather than execute on a malformed recovered decision.
        _ => PermissionOutcome::Deny {
            reason: "Tool permission request failed: malformed orphaned control_response".to_string(),
        },
    }
}

/// Re-run a single ORPHANED tool: dequeued between turns, this looks the
/// unresolved `tool_use` up in the (resumed) session history and executes it
/// with the recovered permission decision. 1:1 with claude-code's
/// `handleOrphanedPermission` (queryHelpers.ts:224-343). Deduped per-toolUseID
/// via `handled_orphans` (twin of `handledOrphanedToolUseIds`, print.ts:2766):
/// a given id recovers once, but DISTINCT orphans each recover. An id is marked
/// handled ONLY on a real recovery (`Ok(true)`, which also covers the
/// unknown-tool case where the gate is consumed but nothing runs), so a
/// not-found orphan (`Ok(false)`) leaves a later same-id delivery able to
/// recover — matching claude-code, which adds to the Set only when
/// `findUnresolvedToolUse` succeeds.
async fn recover_orphaned_permission(
    runtime: &Runtime,
    cmd: msgqueue::QueuedCommand,
    handled_orphans: &mut std::collections::HashSet<protocol::ToolUseId>,
) {
    let msgqueue::QueuedCommandContent::OrphanedPermission {
        tool_use_id,
        permission_decision_json,
        ..
    } = cmd.content
    else {
        return;
    };
    if handled_orphans.contains(&tool_use_id) {
        tracing::debug!(
            "ignoring duplicate orphaned permission for toolUseID={} (already handled)",
            tool_use_id.as_str()
        );
        return;
    }
    let decision = orphan_decision_from_payload(&permission_decision_json);
    match runtime
        .orchestrator
        .run_orphaned_permission(&tool_use_id, decision)
        .await
    {
        Ok(true) => {
            handled_orphans.insert(tool_use_id.clone());
            tracing::info!(
                "recovered orphaned permission for toolUseID={}",
                tool_use_id.as_str()
            );
        }
        Ok(false) => {
            tracing::debug!(
                "orphaned permission toolUseID={} had no unresolved tool_use; skipped",
                tool_use_id.as_str()
            );
        }
        Err(e) => {
            tracing::warn!(
                "orphaned permission recovery failed for toolUseID={}: {e}",
                tool_use_id.as_str()
            );
        }
    }
}

/// Drive a multi-turn `--input-format stream-json` conversation (P3).
///
/// Reads user turns from stdin (one JSON line per turn), deduplicates by uuid,
/// and feeds each turn sequentially through `run_turn`. Emits `system/init` +
/// `system/status` before the first turn and a `result` frame after the last.
///
/// Under `--replay-user-messages`, duplicate-uuid acks (`isReplay:true`) are
/// emitted when a dup is detected.
///
/// This function is called from `run_cli` when BOTH `--output-format stream-json`
/// AND `--input-format stream-json` are set. The stream is already installed as
/// the orchestrator's `OutputStream`.
pub async fn run_stream_json_input_loop(
    argv: &Argv,
    runtime: &Runtime,
    stream: Arc<StreamJsonStream>,
    permission_mode: permission::PermissionMode,
    control_plane: Arc<StdioControlPlane>,
) -> i32 {
    // Collect the real session_id and model from the orchestrator after build.
    let (session_id_str, model_str) = {
        let session_handle = runtime.orchestrator.session();
        let session = session_handle.lock().await;
        (session.session_id.to_string(), session.model.clone())
    };

    // Collect tool names, MCP servers, slash commands, agents, etc. — same as
    // run_stream_json_print's init-frame population.
    let tool_names = runtime.orchestrator.tool_names();

    let mcp_servers: Vec<(String, String)> = runtime
        .orchestrator
        .list_mcp_servers()
        .await
        .into_iter()
        .map(|s| {
            let status_str = match s.status {
                McpStatus::Connected => "connected".to_string(),
                McpStatus::Disconnected => "disconnected".to_string(),
                McpStatus::Error(_) => "error".to_string(),
            };
            (s.name, status_str)
        })
        .collect();

    let (slash_commands, skills) = {
        let reg = runtime.dispatcher.registry();
        let reg_guard = reg.read().await;
        let mut all_cmds: Vec<String> =
            reg_guard.list_all().into_iter().map(|c| c.name.clone()).collect();
        all_cmds.sort();
        let mut skill_names: Vec<String> = reg_guard
            .list_all()
            .into_iter()
            .filter(|c| c.loaded_from.as_deref() == Some("skills"))
            .map(|c| c.name.clone())
            .collect();
        skill_names.sort();
        drop(reg_guard);
        (all_cmds, skill_names)
    };

    let agents: Vec<String> = runtime
        .orchestrator
        .list_agents()
        .await
        .into_iter()
        .map(|a| a.name)
        .collect();

    let plugins: Vec<(String, String, String)> = vec![];

    let init_params = build_init_params(
        &session_id_str,
        tool_names,
        mcp_servers,
        &model_str,
        permission_mode_str(permission_mode),
        slash_commands,
        agents,
        skills,
        plugins,
        "default",
        None,
        "off",
    );

    stream.set_init_params(init_params).await;

    // Phase 0 (0a): start the single-writer stdout drain task before any frames
    // are emitted. All subsequent emit_* calls push onto the mpsc channel;
    // the drain task is the sole stdout writer.
    stream.ensure_drain_started().await;

    // ① system/init frame (emitted once before any turns).
    stream.emit_init().await;

    // ② system/status frame (the init "requesting" handshake).
    stream.emit_status().await;

    // Phase 0 (0b): spawn the streaming stdin router. Frames arrive AS THEY
    // ARE SENT (not buffered to EOF), routed by type onto three channels:
    // - turn_rx     → user turns consumed sequentially below.
    // - control_req_rx → control_request frames (Phase 0 stub: reply Unsupported).
    // - control_resp_rx → control_response frames (Phase 0 stub: ignored).
    //
    // The reader runs in a spawn_blocking thread so stdin I/O doesn't block
    // the async runtime. When stdin closes or a fatal error occurs all senders
    // drop, signalling EOF to all receivers.
    let StdinChannels {
        mut turn_rx,
        mut control_req_rx,
        mut control_resp_rx,
    } = spawn_stdin_router(argv.replay_user_messages, session_id_str.clone());

    // ORPHANED PERMISSION recovery channel. A late `control_response` whose
    // `can_use_tool` request was lost (process restart with `--resume`, or a
    // duplicate/late delivery) cannot be matched to a pending request; the
    // control-plane forwards it here as an `OrphanedPermission` command, and the
    // turn loop's `select!` re-runs the tool BETWEEN turns
    // (`run_orphaned_permission`). 1:1 with claude-code's
    // `setUnexpectedResponseCallback` → `enqueue({mode:'orphaned-permission'})`
    // → `handleOrphanedPermission` (print.ts:2767/5291, queryHelpers.ts:224).
    let (orphan_tx, mut orphan_rx) =
        tokio::sync::mpsc::unbounded_channel::<msgqueue::QueuedCommand>();
    control_plane.set_orphan_sender(orphan_tx).await;

    // P5 Phase 2: a dedicated resolver task drains `control_response` frames and
    // resolves the matching pending `send_request` future (the `can_use_tool`
    // round-trip). It runs concurrently with the turn loop so a host's
    // permission answer can arrive mid-turn while the gate awaits.
    let resolver_plane = control_plane.clone();
    let resolver_task = tokio::spawn(async move {
        while let Some(frame) = control_resp_rx.recv().await {
            resolver_plane.resolve_response(&frame).await;
        }
        // Stdin closed (EOF): reject any in-flight pending control_request so a
        // gate awaiting a `can_use_tool` response does not hang forever
        // (claude-code StructuredIO rejects all pendingRequests at input close).
        resolver_plane
            .fail_all_pending("Tool permission stream closed before response received")
            .await;
    });

    // ③ Phase 1: pre-collect initialization data for the `initialize` handler.
    // These require async access to runtime — must be collected here before the
    // move into the spawned ctrl-dispatcher task.

    // Commands for the initialize response: user-invocable commands with
    // name + source-annotated description + argument hint.
    let init_commands: Vec<serde_json::Value> = {
        let reg = runtime.dispatcher.registry();
        let reg_guard = reg.read().await;
        let mut cmds: Vec<serde_json::Value> = reg_guard
            .list_all()
            .into_iter()
            .filter(|c| c.user_invocable != Some(false))
            .map(|c| {
                json!({
                    "name": c.name,
                    "description": format_description_with_source(c),
                    "argumentHint": c.argument_hint.as_deref().unwrap_or("")
                })
            })
            .collect();
        // Sort deterministically by name.
        cmds.sort_by(|a, b| {
            a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
        });
        cmds
    };

    // Agents for the initialize response.
    let init_agents: Vec<serde_json::Value> = runtime
        .orchestrator
        .list_agents()
        .await
        .into_iter()
        .map(|a| json!({"name": a.name, "description": a.description}))
        .collect();

    // Models for the initialize response. Use the live model listings from the
    // orchestrator; map known request_model strings to capability flags.
    let init_models: Vec<serde_json::Value> = {
        let listings = runtime.orchestrator.list_model_listings().await;
        listings
            .into_iter()
            .map(|m| {
                // Capability mapping for known Anthropic models.
                let (
                    supports_effort,
                    supported_effort_levels,
                    supports_adaptive_thinking,
                    supports_fast_mode,
                    supports_auto_mode,
                ) = model_capabilities(&m.request_model);
                let mut obj = json!({
                    "value": m.request_model,
                    "displayName": m.display_model,
                    "description": m.provider_label,
                    "supportsEffort": supports_effort,
                    "supportsAdaptiveThinking": supports_adaptive_thinking,
                    "supportsFastMode": supports_fast_mode,
                    "supportsAutoMode": supports_auto_mode,
                });
                if !supported_effort_levels.is_empty() {
                    obj["supportedEffortLevels"] =
                        serde_json::Value::Array(
                            supported_effort_levels
                                .into_iter()
                                .map(|s| serde_json::Value::String(s.to_string()))
                                .collect(),
                        );
                }
                obj
            })
            .collect()
    };

    // Account: emit what we can; full auth integration is deferred (Phase 3+).
    let init_account = json!({
        "email": "",
        "organization": "",
        "subscriptionType": "Claude Max",
        "apiProvider": "firstParty"
    });

    // ③ Phase 1: cancel watch channel for interrupt support.
    // The cancel_tx is shared with the ctrl-dispatcher task; the turn loop
    // listens to cancel_rx so it can abort an in-flight turn on `interrupt`.
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    let cancel_tx_clone = cancel_tx.clone();

    // ③ Drain control_request and control_response channels concurrently with
    //    the turn loop.
    //
    // Phase 1: `initialize`/`interrupt` + byte-exact fallthrough.
    // Phase 3: the tractable inbound arms (set_model/get_*/mcp_status/
    // get_binary_version/rename_session/message_rated/stop_task/end_session/…),
    // which need async orchestrator/registry access — so the dispatcher is async
    // and owns clones of the handle + task registry.
    let outbound_tx = stream.outbound_tx();
    let ctrl_plane = ControlPlaneWriter::new(outbound_tx.clone());
    let ctrl_orch = runtime.orchestrator.clone();
    let ctrl_tasks = runtime.task_registry.clone();
    // `end_session` signals the turn loop to drain + exit (the loop selects on it).
    let end_notify = Arc::new(tokio::sync::Notify::new());
    let end_notify_ctrl = end_notify.clone();
    let ctrl_req_task = tokio::spawn(async move {
        // §2.2: a second `initialize` is an error, not a re-handshake — the
        // binary's handleInitializeRequest replies {subtype:'error', error:
        // 'Already initialized'} when the `initialized` flag is already set.
        let mut initialized = false;
        while let Some(frame) = control_req_rx.recv().await {
            let subtype = control_request_subtype(&frame).to_string();
            let request_id = control_frame_request_id(&frame).to_string();
            if subtype == "initialize" {
                if initialized {
                    ctrl_plane.reply_error(&request_id, "Already initialized");
                    continue;
                }
                initialized = true;
            }
            dispatch_control_request(
                &subtype,
                &request_id,
                &frame,
                &ctrl_plane,
                &cancel_tx_clone,
                &ctrl_orch,
                &ctrl_tasks,
                &end_notify_ctrl,
                &init_commands,
                &init_agents,
                &init_models,
                &init_account,
            )
            .await;
        }
    });

    // ④ Consume user turns sequentially through the orchestrator.
    let betas: Vec<String> = vec![];
    let mut last_turn_err: Option<String> = None;
    let mut had_any_turn = false;
    // Per-toolUseID orphaned-permission dedup (twin of claude-code's
    // `handledOrphanedToolUseIds` Set, print.ts:2766/5272/5287): each DISTINCT
    // unresolved tool_use recovers once; a same-id re-delivery is skipped. NOT a
    // session-wide single-shot — a `--resume` that lost several `can_use_tool`
    // requests recovers each of them, matching claude-code (whose
    // `hasHandledOrphanedPermission` boolean is a per-command QueryEngine field,
    // not a cross-command cap).
    let mut handled_orphans: std::collections::HashSet<protocol::ToolUseId> =
        std::collections::HashSet::new();
    // Disable the orphan `select!` branch once its channel closes (all senders
    // dropped) so a perpetually-ready `recv() → None` can't busy-spin the loop.
    let mut orphan_closed = false;

    loop {
        let turn = tokio::select! {
            // `end_session` (§2.2 #2): the host asked us to drain + exit.
            _ = end_notify.notified() => break,
            // ORPHANED PERMISSION recovery, drained BETWEEN turns (the `select!`
            // is not polled while `run_turn_streaming_with_cancel` runs, so an
            // orphan that arrives mid-turn is buffered and recovered after — never
            // concurrently with a turn, since both mutate `session.history`).
            recv = orphan_rx.recv(), if !orphan_closed => match recv {
                Some(cmd) => {
                    recover_orphaned_permission(runtime, cmd, &mut handled_orphans).await;
                    continue;
                }
                None => {
                    orphan_closed = true;
                    continue;
                }
            },
            recv = turn_rx.recv() => match recv {
                Some(t) => t,
                None => break, // stdin closed or fatal error — exit the loop.
            },
        };
        had_any_turn = true;
        let prompt = content_to_prompt(&turn.content);

        // Under --replay-user-messages, re-emit the inbound user frame as
        // isReplay:true (the initial-prompt ack for each new turn). Echo the
        // ORIGINAL uuid + content so the host can correlate the ack.
        if argv.replay_user_messages {
            let ack_uuid = turn
                .uuid
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            emit_replay_ack(&ack_uuid, &turn.content, None, &session_id_str);
        }

        // Phase 1: use cancel-aware turn entry point so `interrupt` can abort
        // the in-flight SSE stream. A watcher task bridges the watch channel
        // to the CancellationToken that `run_turn_streaming_with_cancel` consumes.
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let mut cancel_rx2 = cancel_rx.clone();
        tokio::spawn(async move {
            if cancel_rx2.changed().await.is_ok() && *cancel_rx2.borrow() {
                cancel_clone.cancel();
            }
        });

        // P5 Phase 2: register this turn's token so a `can_use_tool`
        // `deny+interrupt` response (§3.4) can abort the whole turn.
        control_plane.set_active_turn(cancel.clone()).await;

        match runtime
            .orchestrator
            .run_turn_streaming_with_cancel(&prompt, cancel)
            .await
        {
            Ok(_) => {
                // Reset the cancel signal for the next turn.
                let _ = cancel_tx.send(false);
                last_turn_err = None;
            }
            Err(e) => {
                // Reset cancel state regardless.
                let _ = cancel_tx.send(false);
                last_turn_err = Some(e.to_string());
                break;
            }
        }
    }

    // Wait for the control dispatcher + response resolver to finish (they exit
    // when their channels close, which happens when the stdin reader task
    // finishes or drops the senders).
    let _ = ctrl_req_task.await;
    let _ = resolver_task.await;

    if !had_any_turn {
        // No user turns received — emit an empty-result envelope.
        let cost = runtime.orchestrator.snapshot_cost().await;
        stream
            .emit_result_success("", "end_turn", &cost, &model_str, "off", &betas)
            .await;
        return exit_codes::SUCCESS;
    }

    // ⑤ Emit the result frame.
    let cost = runtime.orchestrator.snapshot_cost().await;
    let result_text = stream.get_last_result_text().await;
    let model = {
        let session_handle = runtime.orchestrator.session();
        let session = session_handle.lock().await;
        session.model.clone()
    };

    if let Some(err_msg) = last_turn_err {
        stream
            .emit_result_error(
                "error_during_execution",
                vec![err_msg],
                &cost,
                &model,
                "off",
                &betas,
            )
            .await;
        exit_codes::RUNTIME_ERROR
    } else {
        stream
            .emit_result_success(&result_text, "end_turn", &cost, &model, "off", &betas)
            .await;
        exit_codes::SUCCESS
    }
}

/// Map a model's `request_model` string to its capability flags.
///
/// Returns `(supportsEffort, supportedEffortLevels, supportsAdaptiveThinking,
///           supportsFastMode, supportsAutoMode)`.
///
/// Known Anthropic models are hard-coded based on the golden capture
/// (GROUND-TRUTH-init.md). Unknown models get all-false / empty defaults.
fn model_capabilities(
    request_model: &str,
) -> (bool, Vec<&'static str>, bool, bool, bool) {
    let rm = request_model.to_lowercase();
    if rm.contains("opus") {
        // claude-opus-4 / opus[1m]: supportsEffort + adaptiveThinking
        (true, vec!["low", "medium", "high"], true, false, false)
    } else if rm.contains("sonnet") {
        // claude-sonnet-4: supportsEffort + fastMode + autoMode
        (true, vec!["low", "medium", "high"], false, true, true)
    } else if rm.contains("haiku") {
        // claude-haiku-3-5: no special capabilities in the golden capture
        (false, vec![], false, false, false)
    } else if rm == "default" {
        // The "default" pseudo-model routes to the system default.
        (false, vec![], false, false, false)
    } else {
        (false, vec![], false, false, false)
    }
}

/// Drive a one-shot `--output-format json` / `--json` conversation.
///
/// Same as `run_stream_json_print` but with `suppress_frames=true` baked into
/// the stream: only the final `result` JSON line is emitted.
pub async fn run_json_print(
    argv: &Argv,
    runtime: &Runtime,
    stream: Arc<StreamJsonStream>,
    permission_mode: permission::PermissionMode,
) -> i32 {
    // The stream already has suppress_frames=true; we delegate to the shared
    // implementation which respects that flag.
    run_stream_json_print(argv, runtime, stream, permission_mode).await
}

/// `--json-schema` structured-output loop: run the turn (the model is forced to
/// call `StructuredOutput`), then validate the captured arguments against
/// `schema` and retry up to `MAX_STRUCTURED_OUTPUT_RETRIES` — 1:1 with
/// claude-code. Emits the validated JSON to stdout on success; on exhausted
/// retries surfaces `error_max_structured_output_retries`.
async fn run_structured_output(
    runtime: &Runtime,
    prompt: &str,
    slot: &orchestrator::structured_output::StructuredOutputSlot,
    schema: &serde_json::Value,
    sink: &dyn OutputSink,
) -> i32 {
    use crate::structured_output::{
        resolve_max_retries, structured_output_decision, StructuredDecision,
    };
    let max_retries =
        resolve_max_retries(std::env::var("MAX_STRUCTURED_OUTPUT_RETRIES").ok().as_deref());
    let mut turn_prompt = prompt.to_string();
    for _ in 0..max_retries {
        // Clear the slot before each attempt (no await while the lock is held).
        if let Ok(mut s) = slot.lock() {
            *s = None;
        }
        sink.turn_start().await;
        let turn_result = runtime.orchestrator.run_turn(&turn_prompt).await;
        let captured = slot.lock().ok().and_then(|mut s| s.take());
        // The forced StructuredOutput call trips the 1-turn cap AFTER capturing the
        // result, so a turn error WITH a captured value is success, not failure —
        // only surface the error when nothing was captured.
        if captured.is_none() {
            if let Err(e) = turn_result {
                sink.error("runtime", &e.to_string()).await;
                return exit_codes::RUNTIME_ERROR;
            }
        }
        match structured_output_decision(captured, schema) {
            StructuredDecision::Emit(value) => {
                let json =
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
                println!("{json}");
                return exit_codes::SUCCESS;
            }
            StructuredDecision::Retry(corrective) => turn_prompt = corrective,
        }
    }
    sink.error(
        "structured_output",
        &format!("Failed to provide valid structured output after {max_retries} attempts"),
    )
    .await;
    exit_codes::RUNTIME_ERROR
}

/// Dispatch a `/command [args]` line through the registry.
pub async fn run_slash_command(input: &str, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match runtime.dispatcher.dispatch(input).await {
        SlashDispatchResult::Handled { display } => {
            sink.command_output("", &display).await;
            exit_codes::SUCCESS
        }
        // A prompt-expanding command (`/loop`, Markdown/Plugin): run the expanded
        // prompt AS a turn through the orchestrator (claude-code `type: "prompt"`)
        // instead of just printing it, so a `/loop` invocation actually schedules
        // + executes.
        SlashDispatchResult::RunAsTurn { prompt } => {
            sink.turn_start().await;
            match runtime.orchestrator.run_turn(&prompt).await {
                Ok(_outcome) => exit_codes::SUCCESS,
                Err(e) => {
                    sink.error("runtime", &e.to_string()).await;
                    exit_codes::RUNTIME_ERROR
                }
            }
        }
        SlashDispatchResult::Unknown { name: _, display } => {
            sink.command_output("", &display).await;
            exit_codes::RUNTIME_ERROR
        }
        SlashDispatchResult::NotASlashCommand => {
            // Defensive: only reached when caller violated the slash-prefix
            // contract.
            sink.error("runtime", "not a slash command (internal error)")
                .await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Where a `--resume` invocation should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRoute {
    /// `--resume <uuid>` — load that concrete session.
    LoadById,
    /// `--resume` (no id), TTY, no `--no-tui` → iocraft Resume screen (M7-12).
    IocraftScreen,
    /// `--resume` (no id), `--no-tui` or non-TTY → M5-08 stdio picker.
    StdioPicker,
}

/// Decide how to handle a `--resume` invocation. Pure (TTY passed in).
#[must_use]
pub fn resume_route(argv: &Argv, is_tty: bool) -> ResumeRoute {
    let arg = argv.resume.as_deref().unwrap_or("");
    if !arg.is_empty() {
        return ResumeRoute::LoadById;
    }
    if argv.no_tui || !is_tty {
        ResumeRoute::StdioPicker
    } else {
        ResumeRoute::IocraftScreen
    }
}

/// Resume an existing session.
///
/// (M7-12) Splits three ways on [`resume_route`]: a concrete id loads by id,
/// an empty arg under a full TTY opens the iocraft Resume screen, and an empty
/// arg under `--no-tui` / a non-TTY falls back to the unchanged M5-08 stdio
/// picker.
pub async fn run_resume(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    match resume_route(argv, crate::mode::is_full_tty()) {
        ResumeRoute::LoadById => run_resume_by_id(argv, runtime, sink).await,
        ResumeRoute::IocraftScreen => run_resume_iocraft(argv, sink).await,
        ResumeRoute::StdioPicker => run_resume_stdio_picker(argv, sink).await,
    }
}

/// `--resume <uuid>` — the concrete-id path.
///
/// Parses the arg as a UUID, then (SESSION.4) verifies the session actually
/// exists on disk via [`load_session`] BEFORE reporting success: a valid-but-
/// unknown id errors with the TS "No conversation found with session ID: {id}"
/// line and a non-zero exit instead of a false "Resumed session {id}".
///
/// Once confirmed present the dispatch mirrors the FRESH launch's
/// [`crate::mode::decide_mode`]:
///   - a non-empty prompt → run the follow-up turn one-shot (`run_oneshot`);
///   - else under a full TTY (no `--no-tui`) → mount the live TUI with the
///     prior conversation replayed (M5-13 — [`mount_resumed_tui`]);
///   - else (`--no-tui` / non-TTY, no prompt) → keep the stdio fallback:
///     surface "Resumed session {id}" + the not-yet-wired stdio REPL notice.
async fn run_resume_by_id(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let arg = argv.resume.as_deref().unwrap_or("");
    let session_id = match resolve_session_id(arg) {
        Ok(id) => id,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    resume_resolved_session(argv, runtime, sink, session_id).await
}

/// `-c/--continue` — resume the MOST-RECENT conversation in the current cwd's
/// project dir (claude-code `main.tsx`: `options.continue` →
/// `loadConversationForResume(undefined)` → newest log). When the project has no
/// resumable conversation, error with the byte-exact `No conversation found to
/// continue` and exit non-zero (TS `exitWithError`). Once a session is picked the
/// dispatch is identical to `--resume <uuid>` (prompt one-shot / TUI / stdio).
pub async fn run_continue(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    // Newest-first rows over the cwd's project dir (same loader the picker uses);
    // `EmptyDirectory` (or an empty list) ⇒ nothing to continue.
    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => {
            sink.error("runtime", "No conversation found to continue").await;
            return exit_codes::RUNTIME_ERROR;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let Some(first) = rows.first() else {
        sink.error("runtime", "No conversation found to continue").await;
        return exit_codes::RUNTIME_ERROR;
    };
    resume_resolved_session(argv, runtime, sink, first.uuid).await
}

/// Shared post-resolution resume dispatch for both `--resume <uuid>` and
/// `--continue`: confirm the session exists on disk, then mirror the fresh launch
/// (prompt one-shot → TUI mount → stdio fallback).
async fn resume_resolved_session(
    argv: &Argv,
    runtime: &Runtime,
    sink: &dyn OutputSink,
    session_id: uuid::Uuid,
) -> i32 {
    // SESSION.4 parity: a resume for a session that does NOT exist on disk must
    // NOT report success. TS (claude-code/src/main.tsx:3675-3681) calls
    // `loadConversationForResume(sessionId)` and, when it yields nothing, exits
    // via `exitWithError(root, "No conversation found with session ID:
    // {sessionId}")` (exit code 1). We mirror that by loading the session up
    // front and only proceeding once it is confirmed to exist and parse.
    let loaded = load_resume_session(session_id).await;
    if let Some((message, code)) = resume_by_id_error(session_id, loaded.as_ref()) {
        sink.error("runtime", &message).await;
        return code;
    }
    // Session exists and parsed — these are the raw transcript lines that seed
    // both the orchestrator's `SessionState.history` (engine side) and the TUI
    // scrollback (render side).
    let messages = loaded.unwrap_or_default();

    // A follow-up prompt keeps the one-shot path (matches the fresh
    // `Mode::Print` arm): print the resume line then run the turn. The prompt
    // continues the *resumed* conversation only when the orchestrator carries
    // the replayed history — but the supplied `runtime` is the standard
    // sink-adapter build, so seed its session here too before running.
    if let Some(p) = &argv.prompt {
        if !p.trim().is_empty() {
            seed_orchestrator_session(&runtime.orchestrator, session_id, &messages).await;
            sink.text(&format!("Resumed session {session_id}\n")).await;
            return run_oneshot(argv, runtime, sink).await;
        }
    }

    // No prompt: mirror the fresh interactive dispatch. Under a full TTY (and no
    // `--no-tui`) mount the live TUI with the prior conversation replayed (the
    // M5-13 milestone); otherwise fall back to the stdio notice.
    if crate::mode::is_full_tty() && !argv.no_tui {
        return mount_resumed_tui(argv, session_id, messages).await;
    }

    sink.text(&format!("Resumed session {session_id}\n")).await;
    eprintln!("lingxi-cli: resumed; stdio REPL not yet wired (M5-13)");
    exit_codes::NOT_IMPLEMENTED
}

/// (M5-13) Mount the live TUI for a resumed `--resume <uuid>` session, seeded
/// with the prior conversation.
///
/// Reuses the FRESH TUI mount end-to-end ([`crate::init::build_runtime_for_tui`]
/// → [`crate::mode::build_tui_runtime`] → [`crate::mode::mount_tui_runtime`]),
/// adding exactly the two resume seeds the W38 seam + the engine resume path
/// expose:
///   1. ENGINE side — overwrite the freshly-built orchestrator's in-memory
///      `SessionState` (`history` + `session_id`) with the replayed transcript
///      via [`seed_orchestrator_session`], so a follow-up turn continues the
///      prior conversation rather than starting empty.
///   2. RENDER side — seed the TUI scrollback via
///      `tui::replay::rebuild_from_jsonl(&messages)`, so the existing history is
///      painted on the very first frame (the claude-code REPL `initialMessages`
///      analog).
///
/// A FRESH launch never reaches here; the fresh `Mode::Tui` arm calls
/// `build_tui_runtime` with an empty replay vec, so this change leaves the fresh
/// path byte-identical.
///
/// PARITY-GAP (documented follow-up): this resume-into-TUI path does NOT mount
/// the `BypassPermissionsModeDialog` that the fresh `Mode::Tui` arm shows
/// (`mode.rs`). TS `showSetupScreens` runs the acknowledgement dialog on every
/// interactive startup, resume included. NOT a security hole — the root/sandbox
/// bypass guard already ran once in `run_cli` before this dispatch, so no
/// un-acknowledged session reaches tool execution un-guarded; only the one-time
/// acknowledgement UX is skipped when a first-time bypass user resumes straight
/// into the TUI. `build_runtime_for_tui` still threads the resolved permission
/// mode, so the mode itself is correct here.
async fn mount_resumed_tui(argv: &Argv, session_id: uuid::Uuid, messages: Vec<JsonlMessage>) -> i32 {
    let tui_build = match crate::init::build_runtime_for_tui(argv).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lingxi-cli: tui init failed: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    // ENGINE seed: replay the transcript into the orchestrator's session so a
    // live turn continues the prior conversation.
    seed_orchestrator_session(&tui_build.runtime.orchestrator, session_id, &messages).await;
    // RENDER seed: map the raw JSONL into TUI scrollback rows (W38 seam).
    let resumed_messages = tui::replay::rebuild_from_jsonl(&messages);
    let tui_runtime = crate::mode::build_tui_runtime(tui_build, argv, resumed_messages).await;
    crate::mode::mount_tui_runtime(tui_runtime).await
}

/// Seed an already-built orchestrator's in-memory [`engine::SessionState`] from
/// a resumed transcript.
///
/// The fresh-mount path builds the orchestrator via `engine_desktop::build`,
/// which hands back an `Arc<ConversationOrchestrator>` with a fresh, empty
/// session — it has no resume parameter. Rather than introduce a second,
/// divergent resumed-orchestrator construction path, we rebuild the
/// `SessionState` from the transcript lines ALREADY in hand via the
/// orchestrator's own public replay mapping
/// ([`orchestrator::state_from_messages`], the same per-line conversion its
/// `with_resume` constructor uses — no redundant disk re-read, no TOCTOU window),
/// then overwrite the live session through its public `session()` accessor (an
/// `Arc<Mutex<SessionState>>`). We copy `history` + `session_id` so the resumed
/// id is reported and a follow-up turn appends onto the prior history.
async fn seed_orchestrator_session(
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    session_id: uuid::Uuid,
    messages: &[JsonlMessage],
) {
    let replayed = orchestrator::state_from_messages(session_id, messages);
    // `session()` returns an owned `Arc<Mutex<SessionState>>`; bind it so the
    // lock guard does not borrow a temporary that is freed at end-of-statement.
    let session_handle = orchestrator.session();
    let mut session = session_handle.lock().await;
    session.session_id = replayed.session_id;
    session.history = replayed.history;
}

/// `--resume` (no id) under `--no-tui` / non-TTY — the UNCHANGED M5-08 stdio
/// picker (`select_session_interactive`) over the 5 most-recent sessions in
/// the current cwd's project dir. The regression-free fallback.
///
/// On `Ok(Some(uuid))` surface "Resumed session {uuid}"; on `Ok(None)`
/// (cancel / EOF) print "Cancelled."; on `Err(EmptyDirectory)` print the
/// M5-08 "No conversations found to resume." All return [`exit_codes::SUCCESS`]
/// except a hard I/O failure (`RUNTIME_ERROR`).
async fn run_resume_stdio_picker(_argv: &Argv, sink: &dyn OutputSink) -> i32 {
    use tokio::io::BufReader;

    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => {
            sink.text("No conversations found to resume.\n").await;
            return exit_codes::SUCCESS;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    match select_session_interactive(&rows, &mut stdin, &mut stdout).await {
        Ok(Some(uuid)) => {
            sink.text(&format!("Resumed session {uuid}\n")).await;
            exit_codes::SUCCESS
        }
        Ok(None) => {
            sink.text("Cancelled.\n").await;
            exit_codes::SUCCESS
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// `--resume` (no id) under a full TTY — open the iocraft Resume screen over
/// the same M5-08 loader rows. After the TUI returns, read the chosen UUID:
/// `Some(uuid)` → "Resumed session {uuid}"; `None` → "Cancelled."
async fn run_resume_iocraft(_argv: &Argv, sink: &dyn OutputSink) -> i32 {
    let rows = match load_resume_rows().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => {
            // Render the empty-state screen so the user still sees the locked
            // "No conversations found to resume." line, then cancels out.
            Vec::new()
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };

    match tui::session::run_resume_picker(rows).await {
        Ok(Some(uuid)) => {
            sink.text(&format!("Resumed session {uuid}\n")).await;
            exit_codes::SUCCESS
        }
        Ok(None) => {
            sink.text("Cancelled.\n").await;
            exit_codes::SUCCESS
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Load the recent-session rows for the current cwd via the M5-08 loader.
/// Shared by the stdio + iocraft branches (DRY). Resolves `claude_home`
/// (`$LINGXI_CONFIG_DIR` → `~/.claude`), the cwd, and a disk-backed
/// [`PosixFileSystem`] — the same loader inputs M5-08 expects, then delegates
/// to the pure [`load_resume_rows_from`].
async fn load_resume_rows() -> Result<Vec<SessionMetadata>, LoaderError> {
    let claude_home = claude_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_rows_from(&claude_home, &cwd).await
}

/// Production disk→[`SessionMetadata`] path with the inputs passed in (no env /
/// process-cwd reads), so it is directly testable. Builds the same disk-backed
/// [`platform_posix::PosixFileSystem`] the live branches use and
/// asks the M5-08 loader for up to 5 most-recent rows.
async fn load_resume_rows_from(
    claude_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(
        cwd.to_path_buf(),
    ));
    list_recent_sessions(claude_home, &cwd_str, 5, fs).await
}

/// Load a concrete session by UUID for the `--resume <uuid>` path, using the
/// live `claude_home` (`$LINGXI_CONFIG_DIR` → `~/.claude`) + process cwd. Thin
/// env-reading wrapper over [`load_resume_session_from`] (mirrors the
/// `load_resume_rows` / `load_resume_rows_from` split so the disk logic stays
/// testable with no env / process-cwd reads).
async fn load_resume_session(session_id: uuid::Uuid) -> Result<Vec<JsonlMessage>, LoaderError> {
    let claude_home = claude_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_session_from(&claude_home, &cwd, session_id).await
}

/// Production disk→`Vec<JsonlMessage>` load with the inputs passed in (no env /
/// process-cwd reads) so it is directly testable. Builds the same disk-backed
/// [`platform_posix::PosixFileSystem`] the row loader uses and asks the
/// M5-07/M5-08 [`load_session`] loader for the session, which returns
/// [`LoaderError::SessionNotFound`] when no `<uuid>.jsonl` exists under the
/// cwd's project dir.
async fn load_resume_session_from(
    claude_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(
        cwd.to_path_buf(),
    ));
    load_session(claude_home, &cwd_str, session_id, fs).await
}

/// Claude config home dir. `$LINGXI_CONFIG_DIR` when set wins (claude-code `tr()`
/// `??`: an empty value is honored verbatim → cwd-relative), else `~/.claude`.
/// Shared across the CLI's settings/MCP/desktop-config resolution (`lib`, `mode`,
/// `init`) so every user-tier path honors `$LINGXI_CONFIG_DIR`.
pub(crate) fn claude_home_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var(branding::CONFIG_DIR_ENV) {
        return PathBuf::from(explicit);
    }
    dirs::home_dir().map_or_else(
        || PathBuf::from(branding::DOT_DIR),
        |h| h.join(branding::DOT_DIR),
    )
}

/// Resolve the `--resume <ID>` argument into a concrete UUID.
fn resolve_session_id(arg: &str) -> Result<uuid::Uuid, LoaderError> {
    uuid::Uuid::parse_str(arg).map_err(|_| LoaderError::SessionNotFound {
        arg: arg.to_string(),
    })
}

/// Map a `--resume <uuid>` load outcome to the user-facing error to emit, if
/// any. `None` means the session loaded — keep the "Resumed session {id}"
/// success path. On [`LoaderError::SessionNotFound`] this returns the
/// TS-faithful "No conversation found with session ID: {id}" line
/// (claude-code/src/main.tsx:3681); any other loader failure maps to TS's
/// catch-arm "Failed to resume session {id}" (main.tsx:3704). Both carry
/// [`exit_codes::RUNTIME_ERROR`] (TS `exitWithError` → exit 1). Pure so the
/// SESSION.4 existence check is unit-testable without a `Runtime` / sink.
fn resume_by_id_error(
    session_id: uuid::Uuid,
    loaded: Result<&Vec<JsonlMessage>, &LoaderError>,
) -> Option<(String, i32)> {
    match loaded {
        Ok(_) => None,
        Err(LoaderError::SessionNotFound { .. }) => Some((
            format!("No conversation found with session ID: {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
        Err(_) => Some((
            format!("Failed to resume session {session_id}"),
            exit_codes::RUNTIME_ERROR,
        )),
    }
}

#[cfg(test)]
mod tests {
    //! Loader-fixture coverage for the `--resume` disk→[`SessionMetadata`]→
    //! row production path (`load_resume_rows_from`). Drives the *real* CLI
    //! wiring — `platform_posix::PosixFileSystem` + the M5-08
    //! `list_recent_sessions` — over a `tempfile` fixture, with no env or
    //! process-cwd reads so the test stays deterministic and parallel-safe.

    use super::*;
    use session::jsonl::project_dir_name;
    use std::time::{Duration, SystemTime};
    use uuid::Uuid;

    /// Write one valid `<uuid>.jsonl` session file (a single first-user message
    /// in the M5-07/M5-08 on-disk format) into `project_dir`, stamp its mtime,
    /// and return the uuid. `prompt` becomes the row's extracted title.
    fn write_session(project_dir: &std::path::Path, prompt: &str, mtime: SystemTime) -> Uuid {
        let uuid = Uuid::new_v4();
        let path = project_dir.join(format!("{uuid}.jsonl"));
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.8.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": prompt},
        });
        let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
        std::fs::write(&path, bytes).unwrap();
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
        uuid
    }

    /// `<claude_home>/projects/<sanitize(cwd)>/` — the dir the loader scans.
    fn make_project_dir(claude_home: &std::path::Path, cwd: &str) -> std::path::PathBuf {
        let dir = claude_home.join("projects").join(project_dir_name(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn load_resume_rows_from_returns_sorted_rows_with_titles_and_counts() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        // `cwd` is only used as the project-dir key; it need not exist on disk.
        let cwd = std::path::PathBuf::from("/tmp/workproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&claude_home, &cwd_str);

        // Three sessions with staggered mtimes; "newest" has the latest mtime.
        let base = SystemTime::now();
        let _oldest = write_session(&project_dir, "oldest prompt", base);
        let _middle = write_session(
            &project_dir,
            "middle prompt",
            base + Duration::from_secs(10),
        );
        let newest = write_session(
            &project_dir,
            "newest prompt",
            base + Duration::from_secs(20),
        );

        let rows = load_resume_rows_from(&claude_home, &cwd)
            .await
            .expect("loader should produce rows");

        assert_eq!(rows.len(), 3, "all three sessions surface as rows");
        // Newest-first (mtime desc).
        assert_eq!(rows[0].uuid, newest);
        assert_eq!(rows[0].title, "newest prompt");
        assert_eq!(rows[1].title, "middle prompt");
        assert_eq!(rows[2].title, "oldest prompt");
        for w in rows.windows(2) {
            assert!(w[0].modified >= w[1].modified, "rows sorted newest-first");
        }
        // Each fixture file has exactly one JSONL line.
        for row in &rows {
            assert_eq!(row.message_count, 1, "one message per fixture session");
        }
    }

    #[tokio::test]
    async fn load_resume_rows_from_empty_project_dir_is_empty_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/emptyproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write no `.jsonl` files into it.
        make_project_dir(&claude_home, &cwd_str);

        match load_resume_rows_from(&claude_home, &cwd).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_resume_rows_from_missing_project_dir_is_empty_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        // No projects dir at all — the loader treats NotFound as empty-state.
        let cwd = std::path::PathBuf::from("/tmp/neverproj");

        match load_resume_rows_from(&claude_home, &cwd).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    // ── SESSION.4: `--resume <uuid>` existence check ────────────────────────

    #[tokio::test]
    async fn resume_by_id_nonexistent_uuid_errors_with_ts_message_and_nonzero_exit() {
        // Regression: a valid-but-unknown session id used to print a false
        // "Resumed session {id}" success. It must now error with the
        // TS-faithful line (main.tsx:3681) and a non-zero exit instead.
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write NO session file for this id.
        make_project_dir(&claude_home, &cwd_str);
        let missing = Uuid::new_v4();

        let loaded = load_resume_session_from(&claude_home, &cwd, missing).await;
        assert!(
            matches!(loaded, Err(LoaderError::SessionNotFound { .. })),
            "a missing <uuid>.jsonl must surface SessionNotFound, got {loaded:?}"
        );

        let (message, code) =
            resume_by_id_error(missing, loaded.as_ref()).expect("missing session must error");
        assert_eq!(
            message,
            format!("No conversation found with session ID: {missing}"),
            "exact TS string (claude-code/src/main.tsx:3681)"
        );
        assert_eq!(code, exit_codes::RUNTIME_ERROR);
        assert_ne!(
            code,
            exit_codes::SUCCESS,
            "a non-existent id must NOT report a zero (success) exit"
        );
    }

    #[tokio::test]
    async fn resume_by_id_existing_uuid_loads_and_does_not_error() {
        // Happy path: when the <uuid>.jsonl exists the load succeeds and
        // `resume_by_id_error` returns None, so the "Resumed session {id}"
        // success line is reached.
        let temp = tempfile::TempDir::new().unwrap();
        let claude_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&claude_home, &cwd_str);
        let id = write_session(&project_dir, "hello", SystemTime::now());

        let loaded = load_resume_session_from(&claude_home, &cwd, id).await;
        let messages = loaded.as_ref().expect("existing session must load");
        assert_eq!(messages.len(), 1, "the single fixture line is parsed");
        assert!(
            resume_by_id_error(id, loaded.as_ref()).is_none(),
            "an existing session must NOT produce an error"
        );
    }

    #[test]
    fn resume_by_id_error_maps_other_failures_to_failed_to_resume() {
        // A non-SessionNotFound loader failure mirrors TS's catch arm
        // ("Failed to resume session {id}", main.tsx:3704) with a non-zero exit.
        let id = Uuid::new_v4();
        let err = LoaderError::InvalidSelection;
        let (message, code) =
            resume_by_id_error(id, Err(&err)).expect("a loader failure must error");
        assert_eq!(message, format!("Failed to resume session {id}"));
        assert_eq!(code, exit_codes::RUNTIME_ERROR);
    }

    // ── M5-13: `--resume <uuid>` → live TUI mount wiring ────────────────────
    //
    // The full PTY mount (`run_tui_session`) can't run headless, so these tests
    // assert the WIRING the resume mount builds: (a) the engine-side seed places
    // the replayed history into the orchestrator's live session, and (b) the
    // render-side `build_tui_runtime` carries the replayed scrollback + a live
    // orchestrator/bridge — and a FRESH build carries neither.

    /// A test `Argv` with a fresh (TUI-style, no prompt) shape.
    fn tui_argv() -> Argv {
        Argv::default()
    }

    /// A raw `JsonlMessage` (wire-shape line) the loader hands the resume path.
    fn jsonl_line(message_type: &str, content: &serde_json::Value) -> JsonlMessage {
        serde_json::from_value(serde_json::json!({
            "type": message_type,
            "uuid": Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": Uuid::new_v4().to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.8.0",
            "message": {"content": content},
        }))
        .expect("valid JsonlMessage")
    }

    #[tokio::test]
    async fn seed_orchestrator_session_replays_history_and_id() {
        // Build a real orchestrator (fresh, empty session) via the same TUI
        // builder the mount uses, then seed it from a two-line transcript and
        // assert the live session now carries the replayed history + the
        // resumed session id (engine-side resume seed).
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        // Fresh session starts empty.
        let resumed_id = Uuid::new_v4();
        {
            let handle = build.runtime.orchestrator.session();
            let s = handle.lock().await;
            assert!(s.history.is_empty(), "fresh session starts empty");
            assert_ne!(
                s.session_id,
                protocol::SessionId::from_uuid(resumed_id),
                "fresh id differs from the resumed id we will seed"
            );
        }

        let messages = vec![
            jsonl_line("user", &serde_json::json!("hello from the past")),
            jsonl_line("assistant", &serde_json::json!("hi, welcome back")),
        ];
        seed_orchestrator_session(&build.runtime.orchestrator, resumed_id, &messages).await;

        let handle = build.runtime.orchestrator.session();
        let s = handle.lock().await;
        assert_eq!(
            s.session_id,
            protocol::SessionId::from_uuid(resumed_id),
            "seed overrides the session id with the resumed id"
        );
        assert_eq!(s.history.len(), 2, "both transcript lines replayed");
        match &s.history[0] {
            protocol::ConversationMessage::User { content, .. } => {
                assert!(matches!(
                    content.first(),
                    Some(protocol::ContentBlock::Text { text }) if text == "hello from the past"
                ));
            }
            other => panic!("expected first history entry User, got {other:?}"),
        }
        assert!(matches!(
            &s.history[1],
            protocol::ConversationMessage::Assistant { .. }
        ));
    }

    #[tokio::test]
    async fn resumed_tui_runtime_carries_replay_and_live_orchestrator() {
        // The render-side seam: `build_tui_runtime` with replayed scrollback
        // produces a `tui::session::Runtime` whose `resumed_messages` match
        // `rebuild_from_jsonl(transcript)` and which carries a live orchestrator
        // + bridge (not NOT_IMPLEMENTED).
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");

        let messages = vec![
            jsonl_line("user", &serde_json::json!("resume me")),
            jsonl_line("assistant", &serde_json::json!("resumed")),
        ];
        let expected = tui::replay::rebuild_from_jsonl(&messages);
        assert_eq!(expected.len(), 2, "two rows rebuilt from the transcript");

        let tui_runtime = crate::mode::build_tui_runtime(build, &argv, expected.clone()).await;

        // Replayed scrollback is carried verbatim into the TUI runtime.
        assert_eq!(
            tui_runtime.resumed_messages.len(),
            expected.len(),
            "resumed_messages match rebuild_from_jsonl output"
        );
        assert!(matches!(
            &tui_runtime.resumed_messages[0],
            tui::state::RenderedMessage::UserText { body, .. } if body == "resume me"
        ));
        // A live orchestrator + bridge are wired (the mount is real, not stubbed).
        assert!(
            tui_runtime.orchestrator.is_some(),
            "resumed runtime carries a live orchestrator handle"
        );
        assert!(
            tui_runtime.bridge.is_some(),
            "resumed runtime carries a live streaming bridge"
        );
        assert!(
            tui_runtime.turn_tx.is_some(),
            "resumed runtime carries the turn-spawn sender"
        );
    }

    #[tokio::test]
    async fn fresh_tui_runtime_carries_no_replay() {
        // SAFETY: a FRESH launch passes an empty replay vec, so the resulting
        // runtime's `resumed_messages` is empty — byte-identical to the
        // pre-M5-13 fresh mount (no scrollback seed).
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let tui_runtime = crate::mode::build_tui_runtime(build, &argv, Vec::new()).await;
        assert!(
            tui_runtime.resumed_messages.is_empty(),
            "a fresh mount seeds no replayed scrollback"
        );
        assert!(tui_runtime.orchestrator.is_some());
        assert!(tui_runtime.bridge.is_some());
    }

    // ── P5 Phase 3: pure control-arm classification ──────────────────────────

    fn req(subtype: &str, body: serde_json::Value) -> serde_json::Value {
        let mut request = body;
        request["subtype"] = json!(subtype);
        json!({"type": "control_request", "request_id": "r1", "request": request})
    }

    #[test]
    fn pure_unknown_subtype_falls_through_byte_exact() {
        let frame = req("totally_made_up", json!({}));
        assert_eq!(
            pure_control_response("totally_made_up", &frame),
            PureControlReply::Error(
                "Unsupported control request subtype: totally_made_up".to_string()
            )
        );
    }

    #[test]
    fn pure_cli_originated_subtypes_are_ignored_not_unsupported() {
        // #5: an inbound control_request for a CLI-originated subtype is a guard
        // case — no control_response, NOT an Unsupported error.
        for st in ["can_use_tool", "request_user_dialog", "elicitation"] {
            let frame = req(st, json!({}));
            assert_eq!(
                pure_control_response(st, &frame),
                PureControlReply::Ignore,
                "{st} must be ignored (top-of-chain guard), not Unsupported"
            );
        }
    }

    #[test]
    fn pure_get_binary_version_shape() {
        let frame = req("get_binary_version", json!({}));
        let PureControlReply::Success(Some(payload)) =
            pure_control_response("get_binary_version", &frame)
        else {
            panic!("expected success payload");
        };
        assert_eq!(payload["version"], traits::CLAUDE_CODE_VERSION);
        assert!(payload.get("buildTime").is_some());
    }

    #[test]
    fn pure_rename_session_empty_title_errors() {
        let frame = req("rename_session", json!({"title": "   "}));
        assert_eq!(
            pure_control_response("rename_session", &frame),
            PureControlReply::Error("title must be non-empty".to_string())
        );
    }

    #[test]
    fn pure_rename_session_valid_title_acks_empty() {
        let frame = req("rename_session", json!({"title": "My Session"}));
        assert_eq!(
            pure_control_response("rename_session", &frame),
            PureControlReply::Success(None)
        );
    }

    #[test]
    fn pure_message_rated_acks_empty_object() {
        let frame = req("message_rated", json!({"sentiment": "up"}));
        assert_eq!(
            pure_control_response("message_rated", &frame),
            PureControlReply::Success(Some(json!({})))
        );
    }

    #[test]
    fn pure_set_max_thinking_and_seed_read_state_ack_no_payload() {
        for st in ["set_max_thinking_tokens", "seed_read_state"] {
            let frame = req(st, json!({}));
            assert_eq!(
                pure_control_response(st, &frame),
                PureControlReply::Success(None),
                "{st} should ack with no payload"
            );
        }
    }
}
