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
use crate::control_plane::StdioControlPlane;
use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use crate::stream_json::{build_init_params, permission_mode_str, StreamJsonStream};
use crate::stream_json_input::{
    content_to_prompt, control_frame_request_id, control_request_subtype, emit_replay_ack,
    spawn_stdin_router, ControlPlaneWriter, StdinChannels,
};
use command_api::format_description_with_source;
use permission;
use serde_json::{json, Value};
use session::jsonl::loader::{
    list_recent_sessions, load_session, select_session_interactive, LoaderError, SessionMetadata,
};
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::sync::Arc;
use traits::{
    FileSystem, McpStatus, OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult,
};

/// Install the print/SDK-mode process-tree cleanup (parity 2.1.212 — "Fixed
/// SIGTERM during Bash tool orphaning process trees in print/SDK mode").
///
/// Enables the posix runner to spawn foreground Bash children in their own
/// process group (`setsid`) and track them, then spawns a task that — on
/// `SIGTERM`/`SIGHUP`/`SIGINT` (claude-code's signal-exit `[SIGHUP, SIGINT,
/// SIGTERM]`) — `killpg`s every live Bash subtree before the process exits. Tokio
/// `kill_on_drop` only fires on a graceful future drop, so an abrupt signal to
/// `-p`/SDK mode would otherwise orphan the subtree. Installed once per process
/// (idempotent); a no-op in interactive TUI/REPL mode, which never calls it.
fn install_print_mode_process_cleanup() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    platform_posix::process::enable_print_mode_child_cleanup();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut hup), Ok(mut intr)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        let signum = tokio::select! {
            _ = term.recv() => nix::libc::SIGTERM,
            _ = hup.recv() => nix::libc::SIGHUP,
            _ = intr.recv() => nix::libc::SIGINT,
        };
        // Tree-kill every still-running foreground Bash child, then exit with the
        // conventional 128+signal status so the subtree never outlives us.
        platform_posix::process::kill_all_active_children();
        std::process::exit(128 + signum);
    });
}

/// Drive a one-shot conversation: either a `/slash-command` or a normal
/// prompt that runs through the orchestrator turn loop.
pub async fn run_oneshot(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    install_print_mode_process_cleanup();
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
    install_print_mode_process_cleanup();
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
        None,      // memory_auto_path
        "off",     // fast_mode_state
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
        stream.flush().await;
        exit_codes::RUNTIME_ERROR
    } else {
        stream
            .emit_result_success(&result_text, "end_turn", &cost, &model, "off", &betas)
            .await;
        stream.flush().await;
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
/// Resolution of a `set_model` control request's `model` field against the
/// session default, byte-faithful to claude-code 2.1.208's engine handler:
/// `if(fr!=null&&typeof fr!=="string"){…reject…} let or=model??"default",
/// Jr=or.trim().toLowerCase()==="default",vr=Jr?SE():or`.
enum SetModelTarget {
    /// Apply this model: the raw requested string, or the session default when
    /// the request was absent / explicit `null` / case-insensitive `"default"`.
    Apply(String),
    /// The `model` field was present but neither a string nor null — reject.
    Reject,
}

fn resolve_set_model_target(field: Option<&Value>, default_model: &str) -> SetModelTarget {
    // CC: `fr != null` — in JS `!= null` covers both `null` and `undefined`, so
    // an explicit JSON `null` is treated as absent (→ default), not a type
    // error. `model ?? "default"` collapses absent/null to `"default"`.
    let requested = match field {
        Some(Value::String(m)) => m.as_str(),
        None | Some(Value::Null) => "default",
        Some(_) => return SetModelTarget::Reject,
    };
    // CC: `or.trim().toLowerCase() === "default"` — trimmed, case-insensitive.
    if requested.trim().to_lowercase() == "default" {
        SetModelTarget::Apply(default_model.to_string())
    } else {
        // CC: `vr = Jr ? SE() : or` — the RAW requested string (untrimmed).
        SetModelTarget::Apply(requested.to_string())
    }
}

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
            let payload = initialize_response_payload(
                init_commands,
                init_agents,
                init_models,
                init_account,
                std::process::id(),
            );
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
            let default_model = orchestrator.default_model();
            let target = match resolve_set_model_target(field("model"), default_model.as_str()) {
                SetModelTarget::Apply(t) => t,
                SetModelTarget::Reject => {
                    // CC 2.1.208: `set_model: model must be a string`.
                    writer.reply_error(request_id, "set_model: model must be a string");
                    return;
                }
            };
            match orchestrator.switch_model(&target, None).await {
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
        "mcp_authenticate" | "mcp_reconnect" => {
            // ORACLE (2.1.201 `-p` handler): both branches first resolve the
            // MCP server config by `serverName`; when no server matches, they
            // reply `error: "Server not found: {serverName}"` (verified live —
            // `mcp_authenticate`/`mcp_reconnect` for an unknown server both
            // return that exact string). A fresh `-p` session has no MCP
            // servers, so this is the dominant observable path.
            //
            // DEFERRED (found-server path): the live handler then starts an
            // OAuth flow (mcp_authenticate → `{authUrl, requiresUserAction,…}`)
            // or tears down + reconnects the transport (mcp_reconnect → bare
            // success ack). The port's stream-json server has no live OAuth /
            // reconnect seam wired here, so a matched server is acked
            // best-effort: `mcp_reconnect` → bare success (mirrors the binary's
            // `Ur(_t)`), `mcp_authenticate` → success `{}`. Full flows tracked
            // as a follow-up.
            let server_name = field("serverName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let known: Vec<String> = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .map(|s| s.name)
                .collect();
            if !known.iter().any(|n| n == &server_name) {
                writer.reply_error(request_id, &format!("Server not found: {server_name}"));
            } else if subtype == "mcp_reconnect" {
                writer.reply_success(request_id, None);
            } else {
                writer.reply_success(request_id, Some(json!({})));
            }
        }
        // The orchestrator-free arms (set_max_thinking_tokens, get_binary_version,
        // rename_session, message_rated, seed_read_state, file_suggestions,
        // mcp_oauth_callback_url), the CLI-originated guard subtypes (no-reply),
        // and the byte-exact `Unsupported control request subtype` fallthrough
        // are pure — classified by `pure_control_response` so the wire shapes
        // are unit-testable without a live orchestrator.
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
        // ORACLE (2.1.201 `-p` handler): `file_suggestions` resolves against the
        // session's FileIndex and replies `{suggestions:[...]}`. Verified live
        // that a fresh `-p` session with no built file index returns
        // `{suggestions:[]}` for arbitrary queries (including a query that
        // name-matches a real cwd file). The port has no FileIndex seam wired
        // into the stream-json server, so it faithfully returns the empty-index
        // result. DEFERRED: index-backed suggestions once a FileIndex seam is
        // exposed.
        "file_suggestions" => PureControlReply::Success(Some(json!({ "suggestions": [] }))),
        // ORACLE (2.1.201 `-p` handler): `mcp_oauth_callback_url` looks up the
        // in-flight OAuth flow for `serverName`; with no active flow it replies
        // `error: "No active OAuth flow for server: {serverName}"` (verified
        // live). The port keeps no active-flow registry in the stream-json
        // server, so this is always the faithful reply.
        "mcp_oauth_callback_url" => {
            let server_name = field("serverName").and_then(|v| v.as_str()).unwrap_or("");
            PureControlReply::Error(format!("No active OAuth flow for server: {server_name}"))
        }
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

/// Build the `initialize` control_response `response` payload.
///
/// ORACLE (2.1.201, verified live via
/// `{"subtype":"initialize"} | claude -p --input-format stream-json \
///   --output-format stream-json --verbose`): the `-p` handler replies with
/// `{commands, agents, output_style, available_output_styles, models, account,
/// pid}` where `output_style` is `"default"` and `available_output_styles` is
/// the 4-item list `["default","Proactive","Explanatory","Learning"]`.
/// (NOTE: the separate REPL-bridge handler defaults these to `"normal"` /
/// `["normal"]`, but that bridge is NOT the `-p --input-format stream-json`
/// role this dispatcher models — the observable `-p` truth is the 4-item list.)
fn initialize_response_payload(
    commands: &[serde_json::Value],
    agents: &[serde_json::Value],
    models: &[serde_json::Value],
    account: &serde_json::Value,
    pid: u32,
) -> serde_json::Value {
    json!({
        "commands": commands,
        "agents": agents,
        "output_style": "default",
        "available_output_styles": ["default", "Proactive", "Explanatory", "Learning"],
        "models": models,
        "account": account,
        "pid": pid,
    })
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
            reason: "Tool permission request failed: malformed orphaned control_response"
                .to_string(),
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
    install_print_mode_process_cleanup();
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
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
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
                    obj["supportedEffortLevels"] = serde_json::Value::Array(
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
        stream.flush().await;
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
        stream.flush().await;
        exit_codes::RUNTIME_ERROR
    } else {
        stream
            .emit_result_success(&result_text, "end_turn", &cost, &model, "off", &betas)
            .await;
        stream.flush().await;
        exit_codes::SUCCESS
    }
}

/// Map a model's `request_model` string to its capability flags.
///
/// Returns `(supportsEffort, supportedEffortLevels, supportsAdaptiveThinking,
///           supportsFastMode, supportsAutoMode)`.
///
/// Refreshed to the 2.1.198 registry truth. The binary's initialize models
/// builder (print.ts @223434963) computes per-row: effort = `iw` (registry
/// "effort" capability), levels = `UR = [low,medium,high,xhigh,max]` filtered
/// by `BIe` ("max_effort") and `Zne` ("xhigh_effort", which additionally
/// EXCLUDES opus-4-6/sonnet-4-6 by name), adaptive = `Vit`
/// ("adaptive_thinking"), fast = `_h` (registry "fast_mode" or the
/// opus-4-7/opus-4-8 pair, gated on firstParty via `lc()`), auto = `mTe`
/// (true on firstParty for every non-legacy model). Capability sets come from
/// the baked-in catalog (binary blob @207769000..207775500). Legacy Claude
/// ids (claude-3-*, opus-4-0/4-1/4-5, sonnet-4-0/4-5, haiku-4-5) are the
/// shared exclusion list in all four predicates → all-false. Unknown /
/// non-Anthropic models keep all-false / empty defaults (multi-provider
/// divergence: the binary's `RN(Fh(e))` non-1P fallback has no lingxi seam).
fn model_capabilities(request_model: &str) -> (bool, Vec<&'static str>, bool, bool, bool) {
    /// `UR` — the full effort ladder (binary: `UR=["low","medium","high",
    /// "xhigh","max"]`).
    const LEVELS_WITH_XHIGH: &[&str] = &["low", "medium", "high", "xhigh", "max"];
    /// `UR` minus `xhigh` (`Zne` excludes opus-4-6 / sonnet-4-6 by name).
    const LEVELS_NO_XHIGH: &[&str] = &["low", "medium", "high", "max"];

    let rm = request_model.to_lowercase();

    // The "default" pseudo-model: the binary computes capabilities on the
    // RESOLVED model (`r = R_()` for the Default row); lingxi's default
    // resolves to claude-sonnet-5 (2.1.197/198, M1).
    if rm == "default" {
        return model_capabilities("claude-sonnet-5");
    }

    // opus-4-7 / opus-4-8: full ladder incl. xhigh, adaptive thinking, and
    // the ONLY two fast-mode models (`_h`: registry "fast_mode" / name pair).
    if rm.contains("opus-4-7") || rm.contains("opus-4-8") {
        return (true, LEVELS_WITH_XHIGH.to_vec(), true, true, true);
    }
    // sonnet-5 / fable-5 / mythos-5: full ladder + adaptive + auto, NO fast
    // mode (their registry entries carry no "fast_mode"). NB the substring
    // hazard is safe: "claude-sonnet-4-5" does NOT contain "sonnet-5".
    if rm.contains("sonnet-5") || rm.contains("fable-5") || rm.contains("mythos-5") {
        return (true, LEVELS_WITH_XHIGH.to_vec(), true, false, true);
    }
    // sonnet-4-6 / opus-4-6: effort WITHOUT xhigh (`Zne` name-excludes them;
    // registry has "max_effort" but no "xhigh_effort"), adaptive, auto.
    if rm.contains("sonnet-4-6") || rm.contains("opus-4-6") {
        return (true, LEVELS_NO_XHIGH.to_vec(), true, false, true);
    }
    // Legacy Claude exclusions + unknown / non-Anthropic ids: all-false.
    (false, vec![], false, false, false)
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
    let max_retries = resolve_max_retries(
        std::env::var("MAX_STRUCTURED_OUTPUT_RETRIES")
            .ok()
            .as_deref(),
    );
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

/// (M4 cc2.1.198) `--from-pr [value]` — resume a session linked to a PR.
///
/// The binary routes this through the SAME interactive resume picker as a
/// bare `--resume`, passing `filterByPr: rt` (main action: `if(a.fromPr){
/// if(a.fromPr===!0)rt=!0;else if(typeof a.fromPr==="string")rt=a.fromPr}` →
/// picker props `{…, initialSearchQuery: gc, forkSession: a.forkSession,
/// filterByPr: rt}`). There is NO by-id fast path: even `--from-pr 123` opens
/// the picker filtered to `prNumber === 123`. So the route here is the picker
/// split only (TTY → iocraft screen, `--no-tui`/non-TTY → stdio picker), with
/// [`filter_rows_by_pr`] applied to the loaded rows by both pickers.
pub async fn run_from_pr(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    if argv.no_tui || !crate::mode::is_full_tty() {
        run_resume_stdio_picker(argv, sink).await
    } else {
        run_resume_iocraft(argv, sink).await
    }
}

/// (M4 cc2.1.198) Port of the picker's `filterByPr` row filter (`ne = k.filter
/// ((be)=>!be.isSidechain)` then `if(f===!0)…prNumber!==void 0; else if(typeof
/// f==="number")…prNumber===f; else if(typeof f==="string"){let be=wqc(f);
/// if(be!==null)…prNumber===be}`):
///
/// * `None` (no `--from-pr`) → rows unchanged;
/// * bare flag (`""`) → only PR-linked sessions;
/// * a value parsing to a PR number ([`parse_pr_value`]) → `prNumber === n`;
/// * an unparseable value → NO narrowing (the binary applies no filter).
///
/// RESIDUAL: lingxi's [`SessionMetadata`] does not carry `prNumber` yet (the
/// session JSONL `prNumbers/prUrls/prRepositories` envelope is deferred —
/// `session/src/jsonl/reader.rs` "pr-link"), so every row counts as "no
/// linked PR" and a PR filter yields the empty picker ("No conversations
/// found to resume."). When the pr-link metadata lands, this helper is the
/// single place to consult it.
fn filter_rows_by_pr(rows: Vec<SessionMetadata>, from_pr: Option<&str>) -> Vec<SessionMetadata> {
    let Some(raw) = from_pr else { return rows };
    if raw.is_empty() {
        // Bare `--from-pr`: keep only sessions with a linked PR — none yet.
        return Vec::new();
    }
    match parse_pr_value(raw) {
        // `prNumber === n` — no row carries a prNumber yet.
        Some(_n) => Vec::new(),
        // Unparseable value: the binary applies NO narrowing.
        None => rows,
    }
}

/// (M4 cc2.1.198) Port of `wqc` (2.1.198): `parseInt(e,10)` when `> 0`, else
/// the PR-URL match `/(?:https?:\/\/)?[^/\s]+\/[^\s]+?\/(?:pull|
/// pull-requests|-\/merge_requests)\/(\d+)/` (GitHub / Bitbucket / GitLab
/// forms), else `None`.
#[must_use]
fn parse_pr_value(raw: &str) -> Option<u64> {
    // JS `parseInt(e, 10)`: skip leading whitespace, optional sign, leading
    // digits; trailing garbage ignored ("123abc" → 123). Must be > 0.
    let t = raw.trim_start();
    let (neg, digits_part) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits: String = digits_part
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if !digits.is_empty() && !neg {
        if let Ok(n) = digits.parse::<u64>() {
            if n > 0 {
                return Some(n);
            }
        }
    }
    // URL forms. The JS regex needs a host token + at least one path segment
    // before the marker (`[^/\s]+\/[^\s]+?\/`), then the marker + digits.
    for marker in ["/pull/", "/pull-requests/", "/-/merge_requests/"] {
        if let Some(idx) = raw.find(marker) {
            let prefix = &raw[..idx];
            let prefix = prefix
                .strip_prefix("https://")
                .or_else(|| prefix.strip_prefix("http://"))
                .unwrap_or(prefix);
            if prefix.contains('/') && !prefix.contains(char::is_whitespace) {
                let digits: String = raw[idx + marker.len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if !digits.is_empty() {
                    if let Ok(n) = digits.parse::<u64>() {
                        return Some(n);
                    }
                }
            }
        }
    }
    None
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
            sink.error("runtime", "No conversation found to continue")
                .await;
            return exit_codes::RUNTIME_ERROR;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let Some(first) = rows.first() else {
        sink.error("runtime", "No conversation found to continue")
            .await;
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
        // Drive the mount, following any in-session `/resume` switch by
        // re-mounting the chosen session in-process (writer retargeted) until
        // the user quits — never an in-place `resume_session` swap. Cold
        // `--resume` carries no in-session model (`None`) — it opens on the
        // config/CLI model, matching claude-code's `--resume`.
        let first = mount_resumed_tui(argv, session_id, messages, None).await;
        return drive_tui_switch_loop(argv, first, Some(session_id)).await;
    }

    sink.text(&format!("Resumed session {session_id}\n")).await;
    eprintln!("lingxi-cli: resumed; stdio REPL not yet wired (M5-13)");
    exit_codes::NOT_IMPLEMENTED
}

/// (M5-13) Mount the live TUI for a resumed `--resume <uuid>` session, seeded
/// with the prior conversation.
///
/// Reuses the FRESH TUI mount end-to-end ([`crate::init::build_runtime_for_tui`]
/// → [`crate::mode::run_ratatui`]), adding exactly the two resume seeds the W38
/// seam + the engine resume path expose:
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
async fn mount_resumed_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    carried_state: Option<crate::mode::RemountState>,
) -> crate::mode::RunOutcome {
    // Build with the RESUMED session id as the JSONL writer's file name, so new
    // turns append to `<session_id>.jsonl` (the loaded file) instead of forking a
    // fresh-uuid file — the fix for resume splitting a conversation across files.
    let mut tui_build = match crate::init::build_runtime_for_tui_inner(argv, Some(session_id)).await
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lingxi-cli: tui init failed: {e}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    };
    // ENGINE seed: replay the transcript into the orchestrator's session so a
    // live turn continues the prior conversation.
    seed_orchestrator_session(&tui_build.runtime.orchestrator, session_id, &messages).await;
    // COST seed (resume parity, claude-code `restoreCostStateForSession`): the
    // freshly-built cost tracker starts at zero, so without this the footer
    // would show `$0.0000` after resume until the first new turn. Restore the
    // prior accumulated cost from the project config IF it was saved for THIS
    // session id (the `run_ratatui` exit path writes it via `saveCurrentSessionCosts`).
    if let Some(cfg_path) = migrations::global_config::global_config_path() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if let Some(usd) =
            crate::session_cost::restore_session_cost_usd(&cfg_path, &cwd, &session_id.to_string())
        {
            tui_build
                .runtime
                .orchestrator
                .restore_session_cost(crate::session_cost::usd_to_nano(usd))
                .await;
        }
    }
    // LIVE-STATE carry (parity with claude-code's in-place `/rewind`, a React
    // setState that never rebuilds and so keeps model + fast-mode + plan-mode):
    // apply the state the OUTGOING runtime had, carried IN MEMORY through the
    // `RunOutcome`, so the rebuilt runtime keeps the user's mid-session toggles
    // rather than the boot/config defaults. `None` on a cold `--resume`. All are
    // pure session-state writes, run BEFORE `run_ratatui` — model drives the
    // `session.models` the freshly-read status snapshot builds (display follows);
    // fast/plan are read live by the turn loop, no cached display to seed.
    let mut boot_notice = None;
    if let Some(state) = carried_state {
        let orch = &tui_build.runtime.orchestrator;
        if let Some((model, profile)) = state.model {
            let _ = orch.switch_model(&model, profile.as_deref()).await;
        }
        let _ = orch.set_fast_mode(state.fast_mode).await;
        let _ = orch.set_plan_mode(state.plan_mode).await;
        // Restore the live permission mode the user was in (Shift+Tab): apply it
        // to BOTH the freshly-built enforcing gate (so tool checks follow it) and
        // the indicator seed (`initial_permission_mode` drives the bottom-of-
        // composer badge). Without this the re-mount reset the mode to the
        // CLI/config default even though the process never restarted.
        if let Some(wire) = state.permission_mode {
            if let Some(gate) = tui_build.runtime.enforcing_permission_gate.as_ref() {
                let _ = gate.set_permission_mode(&wire).await;
            }
            tui_build.initial_permission_mode = permission::permission_mode_from_cli_string(&wire);
        }
        boot_notice = state.notice;
    }
    // RENDER seed: map the raw JSONL into TUI scrollback rows (W38 seam), then
    // launch the ratatui backend with that replayed scrollback. Resume has no
    // SessionRegistration (fresh launches register; resume does not), so no
    // status forwarder is threaded. A carried one-shot notice (the `/branch`
    // success confirmation) renders as the newest system cell.
    let mut resumed_messages = tui::replay::rebuild_from_jsonl(&messages);
    if let Some(body) = boot_notice {
        resumed_messages.push(tui_core::message::RenderedMessage::SystemText {
            body,
            timestamp: 0,
            is_error: false,
        });
    }
    crate::mode::run_ratatui(tui_build, None, resumed_messages).await
}

/// Drive a mounted TUI, following any in-session `/resume` switch by re-mounting
/// the chosen session in-process until the user quits.
///
/// This is the seam that makes `/resume` a REAL mid-conversation switch WITHOUT
/// an in-place `resume_session` swap (which does not retarget the JSONL writer,
/// so it would fork the conversation across files). Each switch:
///   1. tears the current runtime down through the normal `run_ratatui` exit —
///      the outgoing session's cost is persisted and its in-flight turn +
///      responses websocket are cancelled there (`mode::run_ratatui`), BEFORE
///      this loop sees the [`crate::mode::RunOutcome::SwitchTo`]; then
///   2. rebuilds a fresh runtime pinned to the target session file via the SAME
///      proven startup resume seam ([`mount_resumed_tui`] →
///      `build_runtime_for_tui_inner(argv, Some(id))`), replaying its scrollback.
///
/// A load failure for the switch TARGET must NOT kill the process: the session
/// that was just driving the loop is a known-good, resumable file (it was live a
/// moment ago), so a failed `/resume` re-mounts THAT session instead of exiting
/// (finding #2). Only if even the fallback re-mount fails — a genuinely
/// unrecoverable state — does the loop surface the error and exit.
///
/// `initial_session_id` is the id of the session that produced `first` (both
/// mount sites know it); it seeds the fallback so even a FIRST failed switch has
/// a session to return to.
pub(crate) async fn drive_tui_switch_loop(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
) -> i32 {
    let mut outcome = first;
    // The session currently driving the loop — the fallback for a failed switch.
    let mut current = initial_session_id;
    loop {
        match outcome {
            crate::mode::RunOutcome::Exit(code) => return code,
            crate::mode::RunOutcome::SwitchTo { target, state } => {
                match load_resume_session(target).await {
                    Ok(messages) => {
                        current = Some(target);
                        outcome = mount_resumed_tui(argv, target, messages, state).await;
                    }
                    Err(e) => {
                        // The outgoing runtime is already unwound, so we cannot just
                        // continue it — but we CAN re-mount the session it was, which
                        // reloads cleanly. Never exit on a single failed switch when a
                        // working session was in progress.
                        eprintln!("lingxi-cli: couldn't resume {target}: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                eprintln!("lingxi-cli: staying in current session {fallback}");
                                match load_resume_session(fallback).await {
                                    Ok(messages) => {
                                        outcome =
                                            mount_resumed_tui(argv, fallback, messages, state)
                                                .await;
                                    }
                                    Err(e2) => {
                                        // Double failure: even the known-good session
                                        // won't reload. Genuinely unrecoverable.
                                        eprintln!(
                                            "lingxi-cli: failed to re-mount current session \
                                         {fallback}: {e2}"
                                        );
                                        return exit_codes::RUNTIME_ERROR;
                                    }
                                }
                            }
                            SwitchRecovery::Exit(code) => return code,
                        }
                    }
                }
            }
            crate::mode::RunOutcome::BranchFrom { title, state } => {
                let Some(source) = current else {
                    eprintln!("lingxi-cli: cannot branch — no active session");
                    return exit_codes::RUNTIME_ERROR;
                };
                let lingxi_home = lingxi_home_dir();
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let cwd_str = cwd.to_string_lossy().into_owned();
                let fs: Arc<dyn FileSystem> =
                    Arc::new(platform_posix::PosixFileSystem::new(cwd.clone()));
                let mut state = state;
                let mount_target = match session::create_branch(
                    &lingxi_home,
                    &cwd_str,
                    source,
                    title.as_deref(),
                    fs,
                )
                .await
                {
                    Ok(result) => {
                        // claude-code 2.1.205 success confirmation, rendered in
                        // the NEW branch's transcript: `Branched
                        // conversation (Branch N). You are now in the new
                        // branch (session <new>). Use /resume <src> to return
                        // to the original, or run `lingxi-cli -r <src>` in a
                        // new terminal.` The saved title already embeds the
                        // " (Branch[ N])" marker — reuse it.
                        let marker = result
                            .title
                            .rfind(" (Branch")
                            .map(|i| result.title[i..].to_string())
                            .unwrap_or_default();
                        let notice = format!(
                            "Branched conversation{marker}. You are now in the new branch \
                             (session {new}). Use /resume {src} to return to the original, \
                             or run `lingxi-cli -r {src}` in a new terminal.",
                            new = result.new_session_id,
                            src = result.source_session_id,
                        );
                        if let Some(s) = state.as_mut() {
                            s.notice = Some(notice);
                        }
                        result.new_session_id
                    }
                    Err(session::BranchError::NoConversation) => {
                        // claude-code's in-transcript empty-state line (was a
                        // stderr-only eprintln): re-mount the source session
                        // carrying the notice.
                        if let Some(s) = state.as_mut() {
                            s.notice = Some("No conversation to branch".to_string());
                        }
                        source
                    }
                    Err(e) => {
                        // Branch creation failed; never drop the user —
                        // re-mount the still-good source session.
                        eprintln!("lingxi-cli: failed to branch conversation: {e}");
                        source
                    }
                };
                match load_resume_session(mount_target).await {
                    Ok(messages) => {
                        current = Some(mount_target);
                        outcome = mount_resumed_tui(argv, mount_target, messages, state).await;
                    }
                    Err(e) => {
                        // The branch (or fallback) target won't load. Fall back to
                        // the known-good source, mirroring the SwitchTo recovery.
                        eprintln!("lingxi-cli: couldn't open {mount_target}: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                match load_resume_session(fallback).await {
                                    Ok(messages) => {
                                        outcome =
                                            mount_resumed_tui(argv, fallback, messages, state)
                                                .await;
                                    }
                                    Err(e2) => {
                                        eprintln!(
                                            "lingxi-cli: failed to re-mount current session \
                                             {fallback}: {e2}"
                                        );
                                        return exit_codes::RUNTIME_ERROR;
                                    }
                                }
                            }
                            SwitchRecovery::Exit(code) => return code,
                        }
                    }
                }
            }
            crate::mode::RunOutcome::RewindTo {
                message,
                scope,
                state,
            } => {
                use tui::bottom_pane::view::RewindScope;
                let Some(source) = current else {
                    eprintln!("lingxi-cli: cannot rewind — no active session");
                    return exit_codes::RUNTIME_ERROR;
                };
                let lingxi_home = lingxi_home_dir();
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let cwd_str = cwd.to_string_lossy().into_owned();
                // 1. Code restore (unless conversation-only): rebuild the
                //    file-history index from the persisted transcript + rewind
                //    the working tree to the checkpoint.
                if scope != RewindScope::ConversationOnly {
                    match session::file_history::rewind_from_disk(
                        &lingxi_home,
                        &cwd_str,
                        source,
                        message,
                    )
                    .await
                    {
                        Ok(files) => eprintln!("lingxi-cli: rewound {} file(s)", files.len()),
                        Err(e) => eprintln!("lingxi-cli: file rewind failed: {e}"),
                    }
                }
                // 2. Conversation truncation (unless code-only): truncate the
                //    live transcript in place up to `message`, re-mount same id.
                let mount_target = if scope != RewindScope::CodeOnly {
                    match session::rewind_conversation(&lingxi_home, &cwd_str, source, message)
                        .await
                    {
                        Ok(()) => source,
                        Err(e) => {
                            eprintln!("lingxi-cli: conversation rewind failed: {e}");
                            source
                        }
                    }
                } else {
                    source
                };
                match load_resume_session(mount_target).await {
                    Ok(messages) => {
                        current = Some(mount_target);
                        outcome = mount_resumed_tui(argv, mount_target, messages, state).await;
                    }
                    // A conversation-scope rewind can legitimately truncate the
                    // transcript to EMPTY (rewinding to before the FIRST turn).
                    // `load_resume_session` reports that as `EmptyDirectory`
                    // ("No conversations found to resume"), but the session is
                    // still valid — re-mount it with an empty history so the user
                    // lands on a fresh composer for the SAME session id instead of
                    // being dropped to the shell. Code-only rewinds don't touch
                    // the transcript, so an empty load there IS a real error and
                    // falls through to recovery below.
                    Err(LoaderError::EmptyDirectory) if scope != RewindScope::CodeOnly => {
                        eprintln!("lingxi-cli: rewound to the start — empty conversation");
                        current = Some(mount_target);
                        outcome = mount_resumed_tui(argv, mount_target, Vec::new(), state).await;
                    }
                    Err(e) => {
                        eprintln!("lingxi-cli: couldn't open {mount_target} after rewind: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                match load_resume_session(fallback).await {
                                    Ok(messages) => {
                                        outcome =
                                            mount_resumed_tui(argv, fallback, messages, state)
                                                .await;
                                    }
                                    Err(e2) => {
                                        eprintln!(
                                            "lingxi-cli: failed to re-mount current session \
                                             {fallback}: {e2}"
                                        );
                                        return exit_codes::RUNTIME_ERROR;
                                    }
                                }
                            }
                            SwitchRecovery::Exit(code) => return code,
                        }
                    }
                }
            }
        }
    }
}

/// What to do after a `/resume` switch target fails to load, given the session
/// that was driving the loop. Pure so the "never exit while a session is in
/// progress" contract (finding #2) is unit-testable without mounting a TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwitchRecovery {
    /// Re-mount this known-good session instead of exiting.
    Remount(uuid::Uuid),
    /// No session to fall back to (defensive — both mount sites seed one); exit.
    Exit(i32),
}

fn recover_from_failed_switch(current: Option<uuid::Uuid>) -> SwitchRecovery {
    match current {
        Some(id) => SwitchRecovery::Remount(id),
        None => SwitchRecovery::Exit(exit_codes::RUNTIME_ERROR),
    }
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
    // Seed the parent-uuid chain off the resumed transcript's tail so the first
    // append after resume chains cleanly (the writer file is the same
    // `<session_id>.jsonl` when built via `build_runtime_for_tui_inner`).
    let last_uuid = messages
        .iter()
        .rev()
        .map(|m| m.uuid.clone())
        .find(|u| !u.is_empty());
    orchestrator.seed_last_jsonl_uuid(last_uuid).await;
    // `session()` returns an owned `Arc<Mutex<SessionState>>`; bind it so the
    // lock guard does not borrow a temporary that is freed at end-of-statement.
    let session_handle = orchestrator.session();
    let mut session = session_handle.lock().await;
    session.session_id = replayed.session_id;
    session.history = replayed.history;
    // Restore the saved model (recovered from the last assistant line by
    // `state_from_messages`) so a resumed session continues on — and shows — its
    // saved model, not the launch default. `state_from_messages` yields
    // `DEFAULT_MODEL` when the transcript has no assistant lines, which is the
    // correct fallback.
    session.model = replayed.model;
    // Clear the provider profile. The JSONL doesn't persist it, and the
    // freshly-built orchestrator seeded `model_profile` to the DEFAULT model's
    // provider (engine-desktop's startup `switch_model(default, Some(profile))`
    // → e.g. `Some("anthropic")`). Leaving it scopes routing to the WRONG
    // provider: a resumed cross-provider model like `deepseek-v4-pro` then fails
    // its FIRST LIVE TURN with `ModelUnavailable` — `registry.resolve_in` finds
    // nothing under `anthropic`. With `None`, the registry resolves the model id
    // to its provider BY ID across all providers; a switched-to id like
    // `deepseek-v4-pro` is unique, so it lands on the right provider.
    session.model_profile = None;
}

/// `--resume` (no id) under `--no-tui` / non-TTY — the UNCHANGED M5-08 stdio
/// picker (`select_session_interactive`) over the 5 most-recent sessions in
/// the current cwd's project dir. The regression-free fallback.
///
/// On `Ok(Some(uuid))` surface "Resumed session {uuid}"; on `Ok(None)`
/// (cancel / EOF) print "Cancelled."; on `Err(EmptyDirectory)` print the
/// M5-08 "No conversations found to resume." All return [`exit_codes::SUCCESS`]
/// except a hard I/O failure (`RUNTIME_ERROR`).
async fn run_resume_stdio_picker(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    use tokio::io::BufReader;

    let rows = match load_resume_rows().await {
        // (M4 cc2.1.198) `--from-pr` narrows the picker rows (`filterByPr`);
        // a no-flag `--resume` passes through unchanged.
        Ok(rows) => filter_rows_by_pr(rows, argv.from_pr.as_deref()),
        Err(LoaderError::EmptyDirectory) => {
            sink.text("No conversations found to resume.\n").await;
            return exit_codes::SUCCESS;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    if rows.is_empty() {
        // A PR filter over rows with no PR metadata yields the same locked
        // empty-state line as an empty project dir.
        sink.text("No conversations found to resume.\n").await;
        return exit_codes::SUCCESS;
    }

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

/// `--resume` (no id) under a full TTY — open the ratatui Resume picker over
/// the same M5-08 loader rows. After the picker returns, read the chosen UUID:
/// `Some(uuid)` → "Resumed session {uuid}"; `None` → "Cancelled."
async fn run_resume_iocraft(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    let rows = match load_resume_rows().await {
        // (M4 cc2.1.198) `--from-pr` narrows the picker rows (`filterByPr`);
        // a no-flag `--resume` passes through unchanged. An emptied list
        // renders the same empty-state Resume screen as an empty project dir.
        Ok(rows) => filter_rows_by_pr(rows, argv.from_pr.as_deref()),
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

    // Map the loader metadata into the picker's lean rows (the picker crate does
    // not depend on the `session` loader).
    let picker_rows = map_resume_rows(&rows);

    // Blocking terminal IO → off the async runtime, like the chat `run_app`.
    let picked =
        tokio::task::spawn_blocking(move || tui::resume::run_resume_picker(picker_rows)).await;

    match picked {
        Ok(Ok(Some(uuid))) => {
            sink.text(&format!("Resumed session {uuid}\n")).await;
            exit_codes::SUCCESS
        }
        Ok(Ok(None)) => {
            sink.text("Cancelled.\n").await;
            exit_codes::SUCCESS
        }
        Ok(Err(e)) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Load the recent-session rows for the current cwd via the M5-08 loader.
/// Shared by the stdio + iocraft branches (DRY). Resolves `lingxi_home`
/// (`$LINGXI_CONFIG_DIR` → `~/.claude`), the cwd, and a disk-backed
/// [`PosixFileSystem`] — the same loader inputs M5-08 expects, then delegates
/// to the pure [`load_resume_rows_from`].
async fn load_resume_rows() -> Result<Vec<SessionMetadata>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_rows_from(&lingxi_home, &cwd).await
}

/// Production disk→[`SessionMetadata`] path with the inputs passed in (no env /
/// process-cwd reads), so it is directly testable. Builds the same disk-backed
/// [`platform_posix::PosixFileSystem`] the live branches use and
/// asks the M5-08 loader for up to 5 most-recent rows.
async fn load_resume_rows_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd.to_path_buf()));
    list_recent_sessions(lingxi_home, &cwd_str, 5, fs).await
}

/// Map M5-08 loader metadata into the picker's lean [`tui::resume::ResumeRow`]s
/// (newest-first). The dim metadata line is built with the picker's
/// `relative_time_ago` so it stays byte-identical to the alt-screen picker:
/// `<relative time ago> · <N> messages`. Shared by the startup `--resume` picker
/// ([`run_resume_iocraft`]) and the in-session `/resume` preload
/// ([`load_resume_picker_rows`]).
fn map_resume_rows(rows: &[SessionMetadata]) -> Vec<tui::resume::ResumeRow> {
    let now = std::time::SystemTime::now();
    rows.iter()
        .map(|m| {
            let msgs = if m.message_count == 1 {
                "1 message".to_string()
            } else {
                format!("{} messages", m.message_count)
            };
            tui::resume::ResumeRow {
                uuid: m.uuid,
                title: m.title.clone(),
                metadata_label: format!(
                    "{} \u{00b7} {}",
                    tui::resume::relative_time_ago(m.modified, now),
                    msgs
                ),
            }
        })
        .collect()
}

/// (in-session `/resume`) Preload the recent-session rows for the interactive
/// `/resume` bottom-pane picker, mapped from the M5-08 loader. This is the ASYNC
/// disk scan the blocking ratatui loop cannot run itself, so it is called ONCE
/// before the loop starts (`mode::run_ratatui`) into a shared slot the widget
/// reads. Kept DISTINCT from the startup `--resume` picker path
/// ([`run_resume_iocraft`]): this seeds the LIVE `/resume` command, which
/// switches the session by an in-process re-mount rather than a fresh launch.
/// Returns an empty list on any load error (the picker then shows its
/// empty-state screen), so a scan failure never blocks the mount.
pub(crate) async fn load_resume_picker_rows() -> Vec<tui::resume::ResumeRow> {
    match load_resume_rows().await {
        Ok(rows) => map_resume_rows(&rows),
        Err(_) => Vec::new(),
    }
}

/// Load a concrete session by UUID for the `--resume <uuid>` path, using the
/// live `lingxi_home` (`$LINGXI_CONFIG_DIR` → `~/.claude`) + process cwd. Thin
/// env-reading wrapper over [`load_resume_session_from`] (mirrors the
/// `load_resume_rows` / `load_resume_rows_from` split so the disk logic stays
/// testable with no env / process-cwd reads).
async fn load_resume_session(session_id: uuid::Uuid) -> Result<Vec<JsonlMessage>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    load_resume_session_from(&lingxi_home, &cwd, session_id).await
}

/// Production disk→`Vec<JsonlMessage>` load with the inputs passed in (no env /
/// process-cwd reads) so it is directly testable. Builds the same disk-backed
/// [`platform_posix::PosixFileSystem`] the row loader uses and asks the
/// M5-07/M5-08 [`load_session`] loader for the session, which returns
/// [`LoaderError::SessionNotFound`] when no `<uuid>.jsonl` exists under the
/// cwd's project dir.
async fn load_resume_session_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd.to_path_buf()));
    load_session(lingxi_home, &cwd_str, session_id, fs).await
}

/// Claude config home dir. `$LINGXI_CONFIG_DIR` when set wins (claude-code `tr()`
/// `??`: an empty value is honored verbatim → cwd-relative), else `~/.claude`.
/// Shared across the CLI's settings/MCP/desktop-config resolution (`lib`, `mode`,
/// `init`) so every user-tier path honors `$LINGXI_CONFIG_DIR`.
pub(crate) fn lingxi_home_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var(branding::CONFIG_DIR_ENV) {
        return PathBuf::from(explicit);
    }
    dirs::home_dir().map_or_else(
        || PathBuf::from(branding::DOT_DIR),
        |h| h.join(branding::DOT_DIR),
    )
}

/// The background-agent daemon runtime dir — where `daemon.lock` + `roster.json`
/// live. Placed as direct siblings of `jobs/` and `sessions/` under the config
/// home so the `--bg` writer, the daemon supervisor, and the `agents` reader all
/// agree on one location (the only hard constraint — divergence would silently
/// split the lock/roster from the jobs the `agents` command reads). See the
/// daemon design's `parity_choices` note #1: `config_home` directly (not a
/// `daemon/` subdir) for simplicity.
pub(crate) fn daemon_runtime_dir() -> PathBuf {
    lingxi_home_dir()
}

/// Resolve the `--resume <ID>` argument into a concrete UUID.
fn resolve_session_id(arg: &str) -> Result<uuid::Uuid, LoaderError> {
    // Accept both a bare `<uuid>` and the `sess:<uuid>` display form — the
    // "Session … saved. Resume with: lingxi --resume <id>" hint prints the
    // prefixed `SessionId` Display, so a user copying it verbatim must work.
    // The on-disk JSONL is named by the bare uuid, so we normalize to that.
    let body = arg.strip_prefix("sess:").unwrap_or(arg);
    uuid::Uuid::parse_str(body).map_err(|_| LoaderError::SessionNotFound {
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

    // FIX #2: a failed `/resume` switch must NOT kill a live session. When a
    // session is in progress the loop re-mounts IT (never exits); only with no
    // known-good session at all does it exit.
    #[test]
    fn failed_switch_remounts_current_session_instead_of_exiting() {
        let current = Uuid::new_v4();
        assert_eq!(
            recover_from_failed_switch(Some(current)),
            SwitchRecovery::Remount(current),
            "a working session in progress is re-mounted, not exited"
        );
        assert_eq!(
            recover_from_failed_switch(None),
            SwitchRecovery::Exit(exit_codes::RUNTIME_ERROR),
            "only the no-session-at-all case exits"
        );
    }

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

    /// `<lingxi_home>/projects/<sanitize(cwd)>/` — the dir the loader scans.
    fn make_project_dir(lingxi_home: &std::path::Path, cwd: &str) -> std::path::PathBuf {
        let dir = lingxi_home.join("projects").join(project_dir_name(cwd));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // P2-04: `set_model` control-request resolution is byte-faithful to CC
    // 2.1.208's engine handler across null/absent/case/whitespace/type inputs.
    fn apply(field: Option<serde_json::Value>) -> Option<String> {
        match resolve_set_model_target(field.as_ref(), "session-default") {
            SetModelTarget::Apply(t) => Some(t),
            SetModelTarget::Reject => None,
        }
    }

    #[test]
    fn set_model_absent_or_null_applies_session_default() {
        // CC `model ?? "default"`: both absent and explicit JSON null collapse
        // to the session default — never a type error.
        assert_eq!(apply(None).as_deref(), Some("session-default"));
        assert_eq!(
            apply(Some(serde_json::Value::Null)).as_deref(),
            Some("session-default")
        );
    }

    #[test]
    fn set_model_default_is_case_insensitive_and_trimmed() {
        // CC `or.trim().toLowerCase() === "default"`.
        for s in ["default", "DEFAULT", "Default", "  default  ", "\tdefault\n"] {
            assert_eq!(
                apply(Some(serde_json::Value::String(s.into()))).as_deref(),
                Some("session-default"),
                "{s:?} must resolve to the session default"
            );
        }
    }

    #[test]
    fn set_model_named_model_passes_raw_string() {
        // CC `vr = Jr ? SE() : or` — the raw requested string, untrimmed.
        assert_eq!(
            apply(Some(serde_json::json!("claude-opus-4"))).as_deref(),
            Some("claude-opus-4")
        );
    }

    #[test]
    fn set_model_non_string_non_null_is_rejected() {
        // CC `if(fr!=null && typeof fr!=="string")` → reject.
        assert!(apply(Some(serde_json::json!(42))).is_none());
        assert!(apply(Some(serde_json::json!(true))).is_none());
        assert!(apply(Some(serde_json::json!({"a": 1}))).is_none());
        assert!(apply(Some(serde_json::json!(["x"]))).is_none());
    }

    #[tokio::test]
    async fn load_resume_rows_from_returns_sorted_rows_with_titles_and_counts() {
        let temp = tempfile::TempDir::new().unwrap();
        let lingxi_home = temp.path().join("home");
        // `cwd` is only used as the project-dir key; it need not exist on disk.
        let cwd = std::path::PathBuf::from("/tmp/workproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&lingxi_home, &cwd_str);

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

        let rows = load_resume_rows_from(&lingxi_home, &cwd)
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
        let lingxi_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/emptyproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write no `.jsonl` files into it.
        make_project_dir(&lingxi_home, &cwd_str);

        match load_resume_rows_from(&lingxi_home, &cwd).await {
            Err(LoaderError::EmptyDirectory) => {}
            other => panic!("expected EmptyDirectory, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_resume_rows_from_missing_project_dir_is_empty_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let lingxi_home = temp.path().join("home");
        // No projects dir at all — the loader treats NotFound as empty-state.
        let cwd = std::path::PathBuf::from("/tmp/neverproj");

        match load_resume_rows_from(&lingxi_home, &cwd).await {
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
        let lingxi_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        // Create the project dir but write NO session file for this id.
        make_project_dir(&lingxi_home, &cwd_str);
        let missing = Uuid::new_v4();

        let loaded = load_resume_session_from(&lingxi_home, &cwd, missing).await;
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
        let lingxi_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/resumeproj");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&lingxi_home, &cwd_str);
        let id = write_session(&project_dir, "hello", SystemTime::now());

        let loaded = load_resume_session_from(&lingxi_home, &cwd, id).await;
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
    async fn seed_orchestrator_session_restores_saved_model_and_clears_stale_profile() {
        // Regression (reported): a resumed cross-provider session failed its
        // first LIVE turn with "model unavailable". The engine seeds
        // `model_profile` to the default provider at startup; restoring the
        // saved model but leaving that profile scopes routing to the wrong
        // provider. Seeding must restore the model AND clear the profile so the
        // registry resolves the model id to its provider by id.
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        // Simulate the engine's startup default-profile seed.
        {
            let handle = build.runtime.orchestrator.session();
            let mut s = handle.lock().await;
            s.model_profile = Some("anthropic".to_string());
        }
        // A transcript whose last assistant line was produced on deepseek.
        let assistant: JsonlMessage = serde_json::from_value(serde_json::json!({
            "type": "assistant",
            "uuid": Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": Uuid::new_v4().to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.8.0",
            "message": {"content": "answered on deepseek", "model": "deepseek-v4-pro"},
        }))
        .expect("valid JsonlMessage");
        let messages = vec![jsonl_line("user", &serde_json::json!("hi")), assistant];
        seed_orchestrator_session(&build.runtime.orchestrator, Uuid::new_v4(), &messages).await;

        let handle = build.runtime.orchestrator.session();
        let s = handle.lock().await;
        assert_eq!(
            s.model, "deepseek-v4-pro",
            "resume restores the saved cross-provider model"
        );
        assert_eq!(
            s.model_profile, None,
            "the stale default profile is cleared so routing resolves the model id by provider"
        );
    }

    #[tokio::test]
    async fn resume_build_targets_the_resumed_session_file_not_a_fork() {
        // Regression (reported): resuming session X, asking more, then resuming X
        // again lost the later messages — because the resume build forked a
        // FRESH-uuid rollout file (session_id_override=None) while only seeding
        // the id in memory, splitting the conversation across two files sharing
        // one sessionId. The fix threads the resumed id as session_id_override,
        // which names the JSONL writer `<id>.jsonl` (append mode) — the SAME file
        // the history loads from. We assert the built orchestrator ADOPTS the
        // resumed id (the value that names the on-disk writer file), and that a
        // fresh build instead mints its own id (which would fork a new file).
        let argv = tui_argv();
        let resumed_id = Uuid::new_v4();
        let resumed = crate::init::build_runtime_for_tui_inner(&argv, Some(resumed_id))
            .await
            .expect("resume build");
        let adopted = resumed
            .runtime
            .orchestrator
            .session()
            .lock()
            .await
            .session_id
            .as_uuid();
        assert_eq!(
            adopted, resumed_id,
            "resume build must adopt the resumed id (names the <id>.jsonl writer file)"
        );
        let fresh = crate::init::build_runtime_for_tui_inner(&argv, None)
            .await
            .expect("fresh build");
        let fresh_id = fresh
            .runtime
            .orchestrator
            .session()
            .lock()
            .await
            .session_id
            .as_uuid();
        assert_ne!(
            fresh_id, resumed_id,
            "a fresh (non-resume) build mints its own id — no accidental collision"
        );
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
    fn pure_file_suggestions_returns_empty_suggestions() {
        // ORACLE 2.1.201 `-p`: unknown/no-index query → success {suggestions:[]}.
        let frame = req("file_suggestions", json!({ "query": "probe" }));
        let PureControlReply::Success(Some(payload)) =
            pure_control_response("file_suggestions", &frame)
        else {
            panic!("expected success payload");
        };
        assert_eq!(payload, json!({ "suggestions": [] }));
    }

    #[test]
    fn pure_mcp_oauth_callback_url_no_active_flow() {
        // ORACLE 2.1.201 `-p`: no in-flight OAuth flow ⇒ byte-exact error.
        let frame = req(
            "mcp_oauth_callback_url",
            json!({ "serverName": "s1", "callbackUrl": "http://x?code=1" }),
        );
        assert_eq!(
            pure_control_response("mcp_oauth_callback_url", &frame),
            PureControlReply::Error("No active OAuth flow for server: s1".to_string())
        );
    }

    #[test]
    fn initialize_payload_output_style_defaults_match_p_oracle() {
        // ORACLE 2.1.201 `-p`: output_style "default" + the 4-item list.
        // Locks the `-p` truth (NOT the REPL-bridge "normal"/["normal"]).
        let payload = initialize_response_payload(&[], &[], &[], &json!({}), 4242);
        assert_eq!(payload["output_style"], "default");
        assert_eq!(
            payload["available_output_styles"],
            json!(["default", "Proactive", "Explanatory", "Learning"])
        );
        assert_eq!(payload["pid"], 4242);
        // Exact top-level key set (no fabricated keys).
        let keys: Vec<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "commands",
                "agents",
                "output_style",
                "available_output_styles",
                "models",
                "account",
                "pid",
            ]
        );
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

    /// The initialize-response capability golden, refreshed to the 2.1.198
    /// registry (M1b). Each row mirrors the binary's per-model truth:
    /// `iw`/`UR.filter(BIe,Zne)`/`Vit`/`_h`/`mTe` over the baked-in catalog
    /// capabilities (binary blob @207769000; builder @223434963).
    #[test]
    fn model_capabilities_match_2_1_198_registry() {
        let all = vec!["low", "medium", "high", "xhigh", "max"];
        let no_xhigh = vec!["low", "medium", "high", "max"];

        // opus-4-7 / opus-4-8: full ladder + adaptive + FAST + auto (the only
        // two fast-mode models in the 2.1.198 registry).
        for m in ["claude-opus-4-7", "claude-opus-4-8-20260115"] {
            assert_eq!(
                model_capabilities(m),
                (true, all.clone(), true, true, true),
                "{m}"
            );
        }
        // sonnet-5 / fable-5 / mythos-5: full ladder + adaptive + auto, no fast.
        for m in ["claude-sonnet-5", "claude-fable-5", "claude-mythos-5"] {
            assert_eq!(
                model_capabilities(m),
                (true, all.clone(), true, false, true),
                "{m}"
            );
        }
        // sonnet-4-6 / opus-4-6: no xhigh (binary `Zne` excludes them by name).
        for m in ["claude-sonnet-4-6", "claude-opus-4-6-20260101"] {
            assert_eq!(
                model_capabilities(m),
                (true, no_xhigh.clone(), true, false, true),
                "{m}"
            );
        }
        // Legacy exclusion list shared by all four binary predicates.
        for m in [
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-20250514",
            "claude-haiku-4-5",
            "claude-opus-4-1",
            "claude-opus-4-5",
            "claude-3-5-sonnet-20241022",
        ] {
            assert_eq!(
                model_capabilities(m),
                (false, vec![], false, false, false),
                "{m}"
            );
        }
        // Unknown / non-Anthropic ids stay all-false (multi-provider divergence).
        assert_eq!(
            model_capabilities("gpt-4o"),
            (false, vec![], false, false, false)
        );
        // "default" resolves to the session default model (claude-sonnet-5).
        assert_eq!(
            model_capabilities("default"),
            model_capabilities("claude-sonnet-5")
        );
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

    // ── (M4 cc2.1.198) `--from-pr` — `wqc` + `filterByPr` ports ─────────────

    /// `wqc` port: `parseInt(e,10) > 0`, else the pull/merge-request URL
    /// capture, else `None`.
    #[test]
    fn parse_pr_value_matches_wqc() {
        // Leading integer (JS parseInt semantics: trailing garbage ignored).
        assert_eq!(parse_pr_value("123"), Some(123));
        assert_eq!(parse_pr_value(" 42 "), Some(42));
        assert_eq!(parse_pr_value("123abc"), Some(123));
        // Zero / negative are NOT `> 0`; no URL match either.
        assert_eq!(parse_pr_value("0"), None);
        assert_eq!(parse_pr_value("-3"), None);
        // GitHub / Bitbucket / GitLab URL forms (scheme optional).
        assert_eq!(
            parse_pr_value("https://github.com/foo/bar/pull/77"),
            Some(77)
        );
        assert_eq!(parse_pr_value("github.com/foo/bar/pull/77"), Some(77));
        assert_eq!(
            parse_pr_value("https://bitbucket.org/w/r/pull-requests/9"),
            Some(9)
        );
        assert_eq!(
            parse_pr_value("https://gitlab.com/g/p/-/merge_requests/5"),
            Some(5)
        );
        // The regex needs host + ≥1 path segment before the marker.
        assert_eq!(parse_pr_value("host/pull/3"), None);
        // Plain search terms don't parse.
        assert_eq!(parse_pr_value("fix the login bug"), None);
    }

    /// `filterByPr` port over REAL loader rows: bare flag / a parsed PR number
    /// filter on `prNumber`, which no lingxi row carries yet (session `pr-link`
    /// deferral) → empty; an unparseable value applies NO narrowing.
    #[tokio::test]
    async fn filter_rows_by_pr_semantics() {
        let temp = tempfile::TempDir::new().unwrap();
        let lingxi_home = temp.path().join("home");
        let cwd_str = "/tmp/workproj".to_string();
        let project_dir = make_project_dir(&lingxi_home, &cwd_str);
        write_session(&project_dir, "some prompt", SystemTime::now());
        let rows = load_resume_rows_from(&lingxi_home, std::path::Path::new(&cwd_str))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);

        // No --from-pr → unchanged.
        assert_eq!(filter_rows_by_pr(rows.clone(), None).len(), 1);
        // Bare --from-pr → PR-linked only (none yet).
        assert!(filter_rows_by_pr(rows.clone(), Some("")).is_empty());
        // Parseable PR number/URL → prNumber === n (none yet).
        assert!(filter_rows_by_pr(rows.clone(), Some("123")).is_empty());
        assert!(
            filter_rows_by_pr(rows.clone(), Some("https://github.com/foo/bar/pull/9")).is_empty()
        );
        // Unparseable value → no narrowing (binary behavior).
        assert_eq!(filter_rows_by_pr(rows, Some("login bug")).len(), 1);
    }

    #[test]
    fn resolve_session_id_accepts_both_bare_and_sess_prefixed() {
        // The "Session … saved" hint prints the `sess:`-prefixed SessionId
        // Display form; `--resume` must accept that verbatim as well as a bare
        // uuid, and resolve both to the same on-disk id.
        let uuid = "733fa772-2893-49b2-835d-d86d033daf54";
        let bare = resolve_session_id(uuid).expect("bare uuid resolves");
        let prefixed = resolve_session_id(&format!("sess:{uuid}")).expect("sess: prefix resolves");
        assert_eq!(bare, prefixed);
        assert_eq!(bare.to_string(), uuid);
    }

    #[test]
    fn resolve_session_id_rejects_garbage() {
        assert!(matches!(
            resolve_session_id("not-a-uuid"),
            Err(LoaderError::SessionNotFound { .. })
        ));
    }
}
