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
    content_to_prompt, control_frame_request_id, control_request_subtype, emit_raw_frame_queued,
    emit_replay_ack_queued, spawn_stdin_router, ControlPlaneWriter, StdinChannels,
    StdinControlFrame, StreamInput,
};
use command_api::format_description_with_source;
use control::{
    dispatch_control_request, env_api_provider_is_first_party, flag_settings_fast_mode_opt_in,
    model_capabilities, recover_orphaned_permission, resolve_fast_mode_disabled_reason,
    resolve_fast_mode_state, session_model_is_first_party,
};
#[cfg(test)]
use fusion::await_local_fusion_result_bounded;
use fusion::{
    await_local_fusion_result, fusion_result_exit_code, fusion_spawn_failure_exit_code,
    local_fusion_task_id_to_await,
};
#[cfg(not(unix))]
use lifecycle::print_shutdown_signal;
use lifecycle::{
    finish_print_branch, run_print_owned, run_print_owned_with_cleanup,
    stop_background_agents_at_budget, stop_print_tasks, wind_down_print_tasks, PrintAuxTaskGroup,
};
use permission;
use platform_api::{McpStatus, OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
pub(crate) use resume::{
    daemon_runtime_dir, drive_background_tui_switch_loop, drive_tui_switch_loop,
    inherited_resume_effort, lingxi_home_dir, load_resume_picker_rows,
    mount_background_resumed_tui, run_rewind_files,
};
pub use resume::{resume_route, run_continue, run_from_pr, run_resume, ResumeRoute};
use serde_json::{json, Value};
use std::sync::Arc;
use suggestions::{
    emit_prompt_suggestion_if_enabled, spawn_prompt_suggestion_if_enabled,
    StreamFileSuggestionIndex,
};
mod control;
mod fusion;
mod lifecycle;
mod resume;
mod suggestions;

fn stream_json_error_subtype(err: &orchestrator::OrchestratorError) -> &'static str {
    match err {
        orchestrator::OrchestratorError::MaxTurnsReached { .. } => "error_max_turns",
        orchestrator::OrchestratorError::MaxBudgetReached { .. } => "error_max_budget_usd",
        _ => "error_during_execution",
    }
}

/// Render a non-negative finite `f64` like JavaScript `Number#toFixed(2)`.
/// Rust's precision formatter uses ties-to-even (`1.125 -> 1.12`), while the
/// ECMAScript rule chooses the larger decimal integer on an exact tie
/// (`1.125 -> 1.13`). Work from the exact IEEE-754 rational so nearby values
/// such as `2.675` still produce JavaScript's `2.67`.
fn js_to_fixed_2(value: f64) -> String {
    debug_assert!(value.is_finite() && value >= 0.0);

    let bits = value.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (significand, exponent) = if raw_exponent == 0 {
        (u128::from(fraction), -1022 - 52)
    } else {
        (
            u128::from(fraction | (1_u64 << 52)),
            raw_exponent - 1023 - 52,
        )
    };
    let scaled = significand * 100;
    let cents = if exponent >= 0 {
        scaled << exponent.unsigned_abs()
    } else {
        let shift = exponent.unsigned_abs();
        if shift >= 128 {
            0
        } else {
            let whole = scaled >> shift;
            let remainder = scaled & ((1_u128 << shift) - 1);
            let halfway = 1_u128 << (shift - 1);
            whole + u128::from(remainder >= halfway)
        }
    };

    format!("{}.{:02}", cents / 100, cents % 100)
}

/// Drive a one-shot conversation: either a `/slash-command` or a normal
/// prompt that runs through the orchestrator turn loop.
pub async fn run_oneshot(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    run_print_owned(runtime, run_oneshot_inner(argv, runtime, sink)).await
}

async fn run_oneshot_inner(argv: &Argv, runtime: &Runtime, sink: &dyn OutputSink) -> i32 {
    let prompt = argv.prompt.clone().unwrap_or_default();
    if prompt.trim().is_empty() {
        // Byte-parity with claude-code print.ts: the empty-input error in print
        // mode is this exact line, then exit 1 (ARGV_ERROR == 1 post-flip).
        eprintln!(
            "Error: Input must be provided either through stdin or as a prompt argument when using --print"
        );
        return exit_codes::ARGV_ERROR;
    }

    // This producer is the actual print-mode user prompt, before generic
    // slash dispatch (which is also used by non-human internal callers).
    let (command, args) = prompt
        .split_once(char::is_whitespace)
        .unwrap_or((&prompt, ""));
    if command == "/tasks" {
        if let Some(parsed) = platform_api::human_task_message::parse(args) {
            let result = match parsed {
                Ok((task_id, message)) if runtime.orchestrator.workspace_trusted().await => runtime
                    .task_registry
                    .send_human_task_message(task_id, message)
                    .await
                    .map(|()| format!("Message accepted for task {task_id}"))
                    .map_err(|error| error.to_string()),
                Ok(_) => Err("Trust this workspace before messaging a task".into()),
                Err(error) => Err(error.into()),
            };
            let code = match result {
                Ok(display) => {
                    sink.command_output(&prompt, &display).await;
                    exit_codes::SUCCESS
                }
                Err(error) => {
                    sink.error("task_message", &error).await;
                    exit_codes::RUNTIME_ERROR
                }
            };
            return finish_print_branch(runtime, argv.max_budget_usd, sink, code).await;
        }
    }
    // Slash branch — bypasses the API entirely.
    if prompt.starts_with('/') {
        let code = run_slash_command_with_budget(&prompt, runtime, argv.max_budget_usd, sink).await;
        return finish_print_branch(runtime, argv.max_budget_usd, sink, code).await;
    }

    // Structured-output branch (`--json-schema`): `harness_runtime::desktop::build` wires
    // the StructuredOutput tool and requests forced tool choice. Providers that
    // reject that request still receive an explicit prompt requirement here. We
    // validate the captured result against the schema and retry. Only active when
    // `build()` surfaced a capture slot (`--json-schema` + `--print`).
    if let Some(slot) = runtime.structured_output_slot.clone() {
        if let Some(schema) = argv
            .json_schema
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            let code =
                run_structured_output(runtime, &prompt, &slot, &schema, argv.max_budget_usd, sink)
                    .await;
            return finish_print_branch(runtime, argv.max_budget_usd, sink, code).await;
        }
    }

    // Non-slash branch — drive the orchestrator turn loop. Without a real
    // ANTHROPIC_API_KEY this returns 401; we surface the error verbatim.
    sink.turn_start().await;
    let turn_result = runtime.orchestrator.run_turn(&prompt).await;
    let turn_result = match turn_result {
        Ok(outcome) => wind_down_print_tasks(
            runtime,
            argv.max_budget_usd,
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .map(|()| outcome),
        Err(error) => {
            stop_print_tasks(runtime).await;
            Err(error)
        }
    };
    stop_background_agents_at_budget(
        argv.max_budget_usd,
        runtime.orchestrator.as_ref(),
        runtime.task_registry.as_ref(),
    )
    .await;
    match turn_result {
        Ok(_outcome) => exit_codes::SUCCESS,
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Recognize only the built-in /compact command, preserving its focus text.
fn compact_command_instructions(prompt: &str) -> Option<&str> {
    let tail = prompt.trim().strip_prefix("/compact")?;
    (tail.is_empty() || tail.starts_with(char::is_whitespace)).then(|| tail.trim())
}

async fn run_stream_compact_command(
    argv: &Argv,
    runtime: &Runtime,
    stream: &StreamJsonStream,
    instructions: &str,
    command_uuid: Option<&str>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let started = std::time::Instant::now();
    let uuid = command_uuid.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_string);
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let result = runtime
        .orchestrator
        .force_compact_with_instructions_and_cancel(
            (!instructions.is_empty()).then_some(instructions),
            cancel,
        )
        .await;
    // A successful boundary emits the init frame immediately before itself.
    // A failed attempt has no boundary but still completes a local SDK command.
    if result.is_err() {
        stream.emit_init().await;
    }
    let failure = result.err().map(|error| {
        let message = error.to_string();
        let mut display = command_api::builtins::compact::compact_failure_display(&message);
        if display == "No messages to compact" {
            display.insert_str(0, "Error: ");
        }
        (
            display,
            command_api::builtins::compact::compact_failure_is_error(&message),
        )
    });
    stream
        .emit_compact_command_output(
            instructions,
            &uuid,
            &timestamp,
            failure
                .as_ref()
                .map(|(display, is_error)| (display.as_str(), *is_error)),
            argv.verbose,
            argv.replay_user_messages,
        )
        .await;
    let cost = runtime.orchestrator.snapshot_cost().await;
    stream
        .emit_compact_command_result(
            &cost,
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            failure.as_ref().map(|(display, _)| display.as_str()),
        )
        .await;
}

/// Drive a one-shot stream-json conversation or local /compact command.
/// The stream is already installed as the orchestrator's output sink.
pub async fn run_stream_json_print(
    argv: &Argv,
    runtime: &Runtime,
    stream: Arc<StreamJsonStream>,
    permission_mode: permission::PermissionMode,
) -> i32 {
    run_print_owned(
        runtime,
        run_stream_json_print_inner(argv, runtime, stream, permission_mode),
    )
    .await
}

async fn run_stream_json_print_inner(
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

    // /compact is a local SDK command with compaction status and replay frames.
    // Other local-command transports retain their existing dispatch policy.
    if prompt.starts_with('/') && compact_command_instructions(&prompt).is_none() {
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
    // local to harness-runtime::desktop and not re-exported. Stays [] with this note.
    // The plugin name/path/source would need harness_runtime::desktop::DesktopRuntime
    // to expose a `loaded_plugins()` accessor (follow-up).
    let plugins: Vec<(String, String, String)> = vec![];

    // 2.1.219 `JW()`: why fast mode is unavailable here. The `-p` surface is
    // the Agent SDK, so without the `--settings` `fastMode:true` opt-in this
    // resolves `sdk_opt_in_required` (live 2.1.220 init/result capture).
    let sdk_fast_mode_opt_in = flag_settings_fast_mode_opt_in(argv.settings.as_deref());
    let fast_mode_disabled_reason = {
        let listings = runtime.orchestrator.list_model_listings().await;
        resolve_fast_mode_disabled_reason(
            session_model_is_first_party(env_api_provider_is_first_party(), &listings, &model_str),
            sdk_fast_mode_opt_in,
        )
    };
    // `cK(mt,ce.fastMode)` — the state rides the SAME inputs as the reason, so
    // an opted-in first-party fast-mode model reports `on` instead of the
    // self-contradicting `off`-with-no-reason pair.
    let fast_mode_state =
        resolve_fast_mode_state(&model_str, fast_mode_disabled_reason, sdk_fast_mode_opt_in);

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
        fast_mode_state,
        fast_mode_disabled_reason,
    );

    // Thread the real params + session_id into the stream.
    stream.set_init_params(init_params).await;

    // Phase 0 (0a): start the single-writer stdout drain task before any frames
    // are emitted. All subsequent emit_* calls push onto the mpsc channel;
    // the drain task is the sole stdout writer.
    stream.ensure_drain_started().await;

    if let Some(instructions) = compact_command_instructions(&prompt) {
        run_stream_compact_command(
            argv,
            runtime,
            &stream,
            instructions,
            None,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        stream.flush().await;
        return exit_codes::SUCCESS;
    }

    // ① system/init frame
    stream.emit_init().await;

    // ② system/status frame (status: "requesting")
    stream.emit_status().await;

    // ③ Run the turn — streaming callbacks (emit_text / emit_tool_call /
    //    emit_message_start / emit_message_boundary) fire on the stream.
    let turn_result = runtime.orchestrator.run_turn(&prompt).await;
    let turn_result = match turn_result {
        Ok(outcome) => wind_down_print_tasks(
            runtime,
            argv.max_budget_usd,
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .map(|()| outcome),
        Err(error) => {
            stop_print_tasks(runtime).await;
            Err(error)
        }
    };

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
    let betas = argv.betas.clone().unwrap_or_default();

    stop_background_agents_at_budget(
        argv.max_budget_usd,
        runtime.orchestrator.as_ref(),
        runtime.task_registry.as_ref(),
    )
    .await;
    if let Err(err) = turn_result {
        let err_msg = err.to_string();
        let subtype = stream_json_error_subtype(&err);
        stream
            .emit_result_error(
                subtype,
                vec![err_msg],
                &cost,
                &model,
                fast_mode_state,
                fast_mode_disabled_reason,
                &betas,
            )
            .await;
        stream.flush().await;
        exit_codes::RUNTIME_ERROR
    } else {
        stream
            .emit_result_success(
                &result_text,
                "end_turn",
                &cost,
                &model,
                fast_mode_state,
                fast_mode_disabled_reason,
                &betas,
            )
            .await;
        // The SDK contract queues the result first, then starts the
        // history-inert suggestion side query. Informational suggestion frames
        // may therefore follow the turn-complete result.
        emit_prompt_suggestion_if_enabled(argv, runtime, &stream).await;
        stream.flush().await;
        exit_codes::SUCCESS
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
    let auxiliary_tasks = Arc::new(PrintAuxTaskGroup::default());
    run_print_owned_with_cleanup(
        runtime,
        run_stream_json_input_loop_inner(
            argv,
            runtime,
            stream,
            permission_mode,
            control_plane,
            auxiliary_tasks.clone(),
        ),
        auxiliary_tasks.abort_and_join(),
    )
    .await
}

async fn run_stream_json_input_loop_inner(
    argv: &Argv,
    runtime: &Runtime,
    stream: Arc<StreamJsonStream>,
    permission_mode: permission::PermissionMode,
    control_plane: Arc<StdioControlPlane>,
    auxiliary_tasks: Arc<PrintAuxTaskGroup>,
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

    // 2.1.219 `JW()`: fast-mode unavailability reason for this SDK surface —
    // threaded into system/init, the `initialize` control_response, and every
    // result frame (all live-verified 2.1.220 emission sites).
    let sdk_fast_mode_opt_in = flag_settings_fast_mode_opt_in(argv.settings.as_deref());
    let fast_mode_disabled_reason = {
        let listings = runtime.orchestrator.list_model_listings().await;
        resolve_fast_mode_disabled_reason(
            session_model_is_first_party(env_api_provider_is_first_party(), &listings, &model_str),
            sdk_fast_mode_opt_in,
        )
    };
    // `cK(mt,ce.fastMode)` — same inputs as the reason (see
    // `resolve_fast_mode_state`), so the two never contradict each other.
    let fast_mode_state =
        resolve_fast_mode_state(&model_str, fast_mode_disabled_reason, sdk_fast_mode_opt_in);

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
        fast_mode_state,
        fast_mode_disabled_reason,
    );

    stream.set_init_params(init_params).await;

    // Phase 0 (0a): start the single-writer stdout drain task before any frames
    // are emitted. All subsequent emit_* calls push onto the mpsc channel;
    // the drain task is the sole stdout writer.
    stream.ensure_drain_started().await;

    // Defer the initial conversation frames until the first model turn.
    // A leading /compact has its own status → init lifecycle and must not
    // acquire an unrelated "requesting" status before it starts.
    let mut emitted_initial_conversation_frames = false;

    // Phase 0 (0b): spawn the streaming stdin router. Frames arrive AS THEY
    // ARE SENT (not buffered to EOF), routed by type onto three channels:
    // - input_rx    → ordered user/history/bash frames consumed below.
    // - control_req_rx → control_request frames: dispatched to control-plane and cancel.
    // - control_resp_rx → control_response frames: resolved by `resolver_task`.
    //
    // The reader runs in a spawn_blocking thread so stdin I/O doesn't block
    // the async runtime. When stdin closes or a fatal error occurs all senders
    // drop, signalling EOF to all receivers.
    // Interrupt-receipt substrate (2.1.220 `interrupt_receipt_v1` /
    // `interrupt_cancel_queued_v1` / `msg_lifecycle_v1`): a shadow registry of
    // uuid-stamped queued user messages, shared by the stdin router (`queued`
    // lifecycle), the turn loop (`started` / terminal lifecycles + cancel
    // skip), and the control dispatcher (interrupt receipts).
    let queue_lifecycle = Arc::new(crate::queued_commands::QueueLifecycle::new(
        stream.outbound_tx(),
        session_id_str.clone(),
    ));

    let StdinChannels {
        mut input_rx,
        mut control_req_rx,
        mut control_resp_rx,
    } = spawn_stdin_router(
        argv.replay_user_messages,
        session_id_str.clone(),
        stream.outbound_tx(),
        queue_lifecycle.clone(),
    );

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
    auxiliary_tasks.push(tokio::spawn(async move {
        while let Some(frame) = control_resp_rx.recv().await {
            resolver_plane.resolve_response(&frame).await;
        }
        // Stdin closed (EOF): reject any in-flight pending control_request so a
        // gate awaiting a `can_use_tool` response does not hang forever
        // (claude-code StructuredIO rejects all pendingRequests at input close).
        resolver_plane
            .fail_all_pending("Tool permission stream closed before response received")
            .await;
    }));

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
    // The cancel_tx is shared with the ctrl-dispatcher task; each turn
    // subscribes so it can abort an in-flight turn on `interrupt`.
    // `_cancel_anchor_rx` is never read — it exists because
    // `watch::Sender::send` is a no-op at zero receivers, and BETWEEN turns
    // (no subscriber) an `interrupt` would otherwise be silently dropped.
    let (cancel_tx, _cancel_anchor_rx) = tokio::sync::watch::channel(false);
    let cancel_tx_clone = cancel_tx.clone();

    // ③ Drain control-request/control-cancel and control-response channels
    //    concurrently with the turn loop.
    //
    // The dispatcher handles both server-initiated control requests and
    // inbound `control_cancel_request` frames for outbound permission
    // round-trips.
    let outbound_tx = stream.outbound_tx();
    let ctrl_plane = ControlPlaneWriter::new(outbound_tx.clone());
    let ctrl_orch = runtime.orchestrator.clone();
    let ctrl_tasks = runtime.task_registry.clone();
    // `set_cwd` moves the live session; it needs the cwd cell to swap and the
    // control plane to know whether a turn is in flight.
    let ctrl_session_cwd = runtime.session_cwd.clone();
    let ctrl_plane_busy = control_plane.clone();
    // `end_session` signals the turn loop to drain + exit (the loop selects on it).
    let end_notify = Arc::new(tokio::sync::Notify::new());
    let end_notify_ctrl = end_notify.clone();
    let resolver_plane_for_cancel = control_plane.clone();
    let ctrl_lifecycle = queue_lifecycle.clone();
    let ctrl_file_suggestions = StreamFileSuggestionIndex::default();
    auxiliary_tasks.push(tokio::spawn(async move {
        // §2.2: a second `initialize` is an error, not a re-handshake — the
        // binary's handleInitializeRequest replies {subtype:'error', error:
        // 'Already initialized'} when the `initialized` flag is already set.
        let mut initialized = false;
        while let Some(frame) = control_req_rx.recv().await {
            match frame {
                StdinControlFrame::Cancel(request_id) => {
                    resolver_plane_for_cancel.cancel_request(&request_id).await;
                    continue;
                }
                StdinControlFrame::Request(frame) => {
                    let request_id = control_frame_request_id(&frame).to_string();
                    let subtype = control_request_subtype(&frame).to_string();
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
                        &ctrl_lifecycle,
                        &ctrl_orch,
                        &ctrl_tasks,
                        &ctrl_session_cwd,
                        &ctrl_plane_busy,
                        &end_notify_ctrl,
                        &init_commands,
                        &init_agents,
                        &init_models,
                        &init_account,
                        fast_mode_state,
                        fast_mode_disabled_reason,
                        &ctrl_file_suggestions,
                    )
                    .await;
                }
            }
        }
    }));

    // ④ Consume user turns sequentially through the orchestrator.
    let betas = argv.betas.clone().unwrap_or_default();
    let mut last_turn_err: Option<orchestrator::OrchestratorError> = None;
    let mut had_any_turn = false;
    let mut prompt_suggestion_task: Option<(
        tokio_util::sync::CancellationToken,
        tokio::task::JoinHandle<()>,
    )> = None;
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

    let mut explicit_end_session = false;
    loop {
        let turn = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if runtime.task_registry.has_pending_task_notifications_for(None).await {
                    let cancel = tokio_util::sync::CancellationToken::new();
                    control_plane.set_active_turn(cancel.clone()).await;
                    let result = runtime.orchestrator.run_task_notification_rewake(runtime.task_registry.as_ref(), cancel).await;
                    control_plane.clear_active_turn().await;
                    if let Err(error) = result { last_turn_err = Some(error); break; }
                    let cost = runtime.orchestrator.snapshot_cost().await;
                    let result_text = stream.get_last_result_text().await;
                    let model = runtime.orchestrator.session().lock().await.model.clone();
                    stream.emit_result_success(&result_text, "end_turn", &cost, &model, fast_mode_state, fast_mode_disabled_reason, &betas).await;
                }
                continue;
            },
            // `end_session` (§2.2 #2): the host asked us to drain + exit.
            _ = end_notify.notified() => { explicit_end_session = true; break; },
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
            recv = input_rx.recv() => match recv {
                Some(StreamInput::User(t)) => t,
                Some(StreamInput::History(history)) => {
                    runtime
                        .orchestrator
                        .append_external_history_message(history.message)
                        .await;
                    if argv.replay_user_messages {
                        if let Some(frame) = history.replay_frame {
                            emit_raw_frame_queued(&outbound_tx, &frame);
                        }
                    }
                    continue;
                }
                Some(StreamInput::Bash(command)) => {
                    let input = format!("<bash-input>{}</bash-input>", command.command);
                    let output = runtime.bash_runner.run(&command.command).await;
                    // Oracle frame:
                    // `<bash-stdout>..</bash-stdout><bash-stderr>..</bash-stderr>
                    //  <bash-exit-code>N</bash-exit-code>`
                    let result = format!(
                        "<bash-stdout>{}</bash-stdout><bash-stderr>{}</bash-stderr><bash-exit-code>{}</bash-exit-code>",
                        output.stdout, output.stderr, output.exit_code
                    );
                    for content in [input, result] {
                        runtime
                            .orchestrator
                            .append_external_history_message(protocol::ConversationMessage::user(
                                protocol::MessageId::new(),
                                content.clone(),
                            ))
                            .await;
                        emit_replay_ack_queued(
                            &outbound_tx,
                            &uuid::Uuid::new_v4().to_string(),
                            &Value::String(content),
                            None,
                            &session_id_str,
                        );
                    }
                    continue;
                }
                None => break, // stdin closed or fatal error — exit the loop.
            },
        };
        // interrupt_cancel_queued_v1: a uuid cancelled while queue-resident
        // must not run — its terminal `cancelled` lifecycle already went out
        // with the interrupt receipt. `on_dequeued` also retires the uuid from
        // the `still_queued` shadow registry.
        if let Some(uuid) = turn.uuid.as_deref() {
            if !queue_lifecycle.queued.on_dequeued(uuid) {
                continue;
            }
        }
        let prompt = content_to_prompt(&turn.content);
        let external_message_id = turn
            .uuid
            .as_deref()
            .and_then(protocol::MessageId::parse_prefixed);

        if let Some(uuid) = turn.uuid.as_deref() {
            if runtime
                .orchestrator
                .session_contains_message_uuid(uuid)
                .await
            {
                if argv.replay_user_messages {
                    emit_replay_ack_queued(
                        &outbound_tx,
                        uuid,
                        &turn.content,
                        None,
                        &session_id_str,
                    );
                }
                emit_dedup_skip_terminal(&queue_lifecycle, uuid);
                continue;
            }
        }

        // UUID replays are discarded by Claude Code before they reach the
        // query loop. Only a genuinely new query aborts the prior suggestion
        // and makes this a non-empty session.
        had_any_turn = true;
        if let Some((cancel, _)) = prompt_suggestion_task.take() {
            cancel.cancel();
        }

        if !emitted_initial_conversation_frames && compact_command_instructions(&prompt).is_none() {
            stream.emit_init().await;
            stream.emit_status().await;
            emitted_initial_conversation_frames = true;
        }

        // Under --replay-user-messages, re-emit the inbound user frame as
        // isReplay:true (the initial-prompt ack for each new turn). Echo the
        // ORIGINAL uuid + content so the host can correlate the ack.
        if argv.replay_user_messages && compact_command_instructions(&prompt).is_none() {
            let ack_uuid = turn
                .uuid
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            // Route through the single-writer drain queue (`outbound_tx`, bound
            // above): a direct stdout write here would let the ack overtake data
            // frames still queued in the outbound channel mid-turn.
            emit_replay_ack_queued(
                &outbound_tx,
                &ack_uuid,
                &turn.content,
                None,
                &session_id_str,
            );
        }

        // msg_lifecycle_v1: the dequeued command's turn is now dispatching.
        if let Some(uuid) = turn.uuid.as_deref() {
            queue_lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_STARTED);
        }

        // Phase 1: use cancel-aware turn entry point so `interrupt` can abort
        // the in-flight SSE stream. A watcher task bridges the watch channel
        // to the CancellationToken that `run_turn_streaming_with_cancel` consumes.
        //
        // `subscribe()` — NOT `cancel_rx.clone()`: a watch Receiver clone
        // inherits the version its source last SAW, and `cancel_rx` is never
        // awaited, so from turn 2 on (after the end-of-turn `send(false)`
        // below) a clone would satisfy `changed()` immediately with `false`,
        // the bridge would exit, and no later `interrupt` could reach the
        // token. Loop rather than await a single change so a `false` reset
        // racing the turn start does not retire the bridge either.
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let mut cancel_rx2 = cancel_tx.subscribe();
        let cancel_bridge = tokio::spawn(async move {
            loop {
                if *cancel_rx2.borrow_and_update() {
                    cancel_clone.cancel();
                    return;
                }
                if cancel_rx2.changed().await.is_err() {
                    return;
                }
            }
        });

        // P5 Phase 2: register this turn's token so a `can_use_tool`
        // `deny+interrupt` response (§3.4) can abort the whole turn.
        control_plane.set_active_turn(cancel.clone()).await;

        if let Some(instructions) = compact_command_instructions(&prompt) {
            run_stream_compact_command(
                argv,
                runtime,
                &stream,
                instructions,
                turn.uuid.as_deref(),
                cancel.clone(),
            )
            .await;
            control_plane.clear_active_turn().await;
            cancel_bridge.abort();
            if let Some(uuid) = turn.uuid.as_deref() {
                queue_lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_COMPLETED);
            }
            let _ = cancel_tx.send(false);
            last_turn_err = None;
            continue;
        }

        // Probe handle: after the turn, `is_cancelled()` distinguishes an
        // interrupt-aborted turn from a completed one (binary `mCo(reason)`).
        let cancel_probe = cancel.clone();
        let turn_result = runtime
            .orchestrator
            .run_turn_streaming_with_cancel_image_sources_and_message_id(
                &prompt,
                Vec::new(),
                cancel,
                external_message_id,
            )
            .await;
        // The control plane's token is also its busy flag. Release it only
        // after the orchestrator future has naturally completed so Block tools
        // remain protected, but always release it before accepting between-turn
        // control operations such as set_cwd.
        control_plane.clear_active_turn().await;
        // The bridge outlives the turn otherwise (it parks on `changed()`),
        // and the next turn subscribes its own.
        cancel_bridge.abort();
        emit_turn_terminal_lifecycle(
            &queue_lifecycle,
            turn.uuid.as_deref(),
            &turn_result,
            cancel_probe.is_cancelled(),
        );
        stop_background_agents_at_budget(
            argv.max_budget_usd,
            runtime.orchestrator.as_ref(),
            runtime.task_registry.as_ref(),
        )
        .await;
        match turn_result {
            Ok(_) => {
                let cost = runtime.orchestrator.snapshot_cost().await;
                let result_text = stream.get_last_result_text().await;
                let model = {
                    let session_handle = runtime.orchestrator.session();
                    let session = session_handle.lock().await;
                    session.model.clone()
                };
                stream
                    .emit_result_success(
                        &result_text,
                        "end_turn",
                        &cost,
                        &model,
                        fast_mode_state,
                        fast_mode_disabled_reason,
                        &betas,
                    )
                    .await;
                // The SDK starts generation after queuing every successful
                // result. A later user turn aborts this task when dequeued;
                // starting even when input is already queued preserves that
                // timing and its suppression telemetry.
                prompt_suggestion_task = spawn_prompt_suggestion_if_enabled(argv, runtime, &stream);
                // Reset the cancel signal for the next turn.
                let _ = cancel_tx.send(false);
                last_turn_err = None;
            }
            Err(e) => {
                // Reset cancel state regardless.
                let _ = cancel_tx.send(false);
                last_turn_err = Some(e);
                break;
            }
        }
    }

    if explicit_end_session || last_turn_err.is_some() {
        stop_print_tasks(runtime).await;
    } else {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let winding_down = wind_down_print_tasks(
            runtime,
            argv.max_budget_usd,
            shutdown.clone(),
            Some(&control_plane),
        );
        tokio::pin!(winding_down);
        let result = tokio::select! {
            result = &mut winding_down => result,
            _ = end_notify.notified() => {
                shutdown.cancel();
                control_plane.cancel_active_turn().await;
                // Keep polling so Block tools unwind under their normal policy.
                winding_down.await
            }
        };
        if let Err(error) = result {
            last_turn_err = Some(error);
        }
    }

    // Stream teardown (binary `Hkm`): every uuid still queue-resident gets a
    // terminal `discarded` lifecycle — covers both stdin EOF and `end_session`.
    for uuid in queue_lifecycle.queued.drain_for_discard() {
        queue_lifecycle.emit(&uuid, crate::queued_commands::LIFECYCLE_DISCARDED);
    }

    // Wait for the control dispatcher + response resolver to finish (they exit
    // when their channels close, which happens when the stdin reader task
    // finishes or drops the senders).
    auxiliary_tasks.join().await;

    if let Some((cancel, mut handle)) = prompt_suggestion_task.take() {
        if tokio::time::timeout(std::time::Duration::from_secs(30), &mut handle)
            .await
            .is_err()
        {
            cancel.cancel();
            handle.abort();
        }
    }

    if !had_any_turn {
        // No user turns received — emit an empty-result envelope.
        let cost = runtime.orchestrator.snapshot_cost().await;
        stream
            .emit_result_success(
                "",
                "end_turn",
                &cost,
                &model_str,
                fast_mode_state,
                fast_mode_disabled_reason,
                &betas,
            )
            .await;
        stream.flush().await;
        return exit_codes::SUCCESS;
    }

    // ⑤ Emit the result frame.
    let cost = runtime.orchestrator.snapshot_cost().await;
    let model = {
        let session_handle = runtime.orchestrator.session();
        let session = session_handle.lock().await;
        session.model.clone()
    };

    if let Some(err) = last_turn_err {
        let err_msg = err.to_string();
        let subtype = stream_json_error_subtype(&err);
        stream
            .emit_result_error(
                subtype,
                vec![err_msg],
                &cost,
                &model,
                fast_mode_state,
                fast_mode_disabled_reason,
                &betas,
            )
            .await;
        stream.flush().await;
        exit_codes::RUNTIME_ERROR
    } else {
        // Successful multi-turn results are emitted at each turn boundary,
        // before that turn's optional prompt suggestion. Do not duplicate the
        // last result when stdin closes.
        stream.flush().await;
        exit_codes::SUCCESS
    }
}

/// The oracle terminal reason (`In` at the stream-json call site @240906553)
/// for a port turn outcome, so the `command_lifecycle` terminal can run
/// through the real `Njo`/`mCo` split instead of a bare cancel probe.
/// `Cancelled` is the interrupt arm `Wpt` reports as `aborted_streaming`;
/// `MaxTurns` and `EndTurn` are two of `Bxs`'s `completed` arms.
fn turn_outcome_terminal_reason(outcome: &orchestrator::TurnOutcome) -> &'static str {
    match outcome {
        orchestrator::TurnOutcome::Cancelled => "aborted_streaming",
        orchestrator::TurnOutcome::MaxTurns => "max_turns",
        orchestrator::TurnOutcome::EndTurn => "completed",
    }
}

/// msg_lifecycle_v1 terminal for a command the resume dedup skipped — the
/// binary's stdin dedup arm @246492139, right after the replay ack:
///
/// ```js
/// if(bi&&!Ma&&!Br) e.onCommandLifecycle?.(dt.uuid,"completed"),Qwt.delete(dt.uuid)
/// ```
///
/// `bi` is `mUo` (the message is already in the session file). Without this the
/// uuid would get `queued` and nothing else: `on_dequeued` already retired it
/// from the shadow registry, so teardown's `discarded` sweep can no longer see
/// it and a host awaiting the terminal hangs. The `!Br` guard (`hUo` = turn
/// UNANSWERED ⇒ re-execute) has no port surface — this branch skips the turn
/// unconditionally, so the command is terminal either way.
fn emit_dedup_skip_terminal(lifecycle: &crate::queued_commands::QueueLifecycle, uuid: &str) {
    lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_COMPLETED);
}

/// msg_lifecycle_v1 terminal for a finished turn's own uuid — the stream-json
/// call site @240906553:
///
/// ```js
/// if(gt.uuid!==void 0)_r=Fe(gt.uuid, rn!==null?"cancelled":Njo(In,Nn))
/// ```
///
/// `rn` is a THROWN turn error, `In` the turn's terminal reason, `Nn` the
/// abort-signal state. A turn that threw is `cancelled` outright; otherwise
/// the reason runs through `Njo`/`mCo`. `Err(_)` is the thrown arm here —
/// `MaxTurnsReached` never reaches it (conversation.rs folds it into
/// `Ok(TurnOutcome::MaxTurns)`), which is why `max_turns` keeps `completed`.
fn emit_turn_terminal_lifecycle(
    lifecycle: &crate::queued_commands::QueueLifecycle,
    uuid: Option<&str>,
    turn_result: &Result<orchestrator::TurnOutcome, orchestrator::OrchestratorError>,
    aborted: bool,
) {
    let Some(uuid) = uuid else { return };
    let state = match turn_result {
        Err(_) => crate::queued_commands::LIFECYCLE_CANCELLED,
        Ok(outcome) => crate::queued_commands::terminal_lifecycle_state(
            Some(turn_outcome_terminal_reason(outcome)),
            aborted,
        ),
    };
    lifecycle.emit(uuid, state);
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
/// call `StructuredOutput` when the provider supports forced tool choice, and
/// is explicitly prompted to call it otherwise. Validate the captured arguments
/// against `schema` and retry up to `MAX_STRUCTURED_OUTPUT_RETRIES`. Emits the
/// validated JSON to stdout on success; on exhausted retries surfaces
/// `error_max_structured_output_retries`.
fn structured_output_turn_error_is_retryable(err: &orchestrator::OrchestratorError) -> bool {
    matches!(
        err,
        orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 1 }
    )
}

async fn run_structured_output(
    runtime: &Runtime,
    prompt: &str,
    slot: &orchestrator::structured_output::StructuredOutputSlot,
    schema: &serde_json::Value,
    max_budget_usd: Option<f64>,
    sink: &dyn OutputSink,
) -> i32 {
    use crate::structured_output::{
        resolve_max_retries, structured_output_decision, structured_output_prompt,
        StructuredDecision,
    };
    let max_retries = resolve_max_retries(
        std::env::var("MAX_STRUCTURED_OUTPUT_RETRIES")
            .ok()
            .as_deref(),
    );
    let mut turn_prompt = structured_output_prompt(prompt);
    for _ in 0..max_retries {
        // Clear the slot before each attempt (no await while the lock is held).
        if let Ok(mut s) = slot.lock() {
            *s = None;
        }
        sink.turn_start().await;
        let turn_result = runtime.orchestrator.run_turn(&turn_prompt).await;
        stop_background_agents_at_budget(
            max_budget_usd,
            runtime.orchestrator.as_ref(),
            runtime.task_registry.as_ref(),
        )
        .await;
        let captured = slot.lock().ok().and_then(|mut s| s.take());
        // Any tool call trips the structured-output path's 1-turn cap. A captured
        // StructuredOutput value is success. Without one, MaxTurnsReached means a
        // provider that cannot force tool_choice selected another advertised tool;
        // feed the corrective prompt into the bounded retry loop. Other errors are
        // genuine runtime failures and remain terminal.
        if captured.is_none() {
            if let Err(e) = turn_result {
                if !structured_output_turn_error_is_retryable(&e) {
                    sink.error("runtime", &e.to_string()).await;
                    return exit_codes::RUNTIME_ERROR;
                }
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
    run_slash_command_with_budget(input, runtime, None, sink).await
}

/// Budget-aware internal form used by print mode. Keeping the public wrapper's
/// original signature avoids breaking downstream callers of the CLI library.
async fn run_slash_command_with_budget(
    input: &str,
    runtime: &Runtime,
    max_budget_usd: Option<f64>,
    sink: &dyn OutputSink,
) -> i32 {
    match runtime.dispatcher.dispatch(input).await {
        SlashDispatchResult::Handled { display } => {
            sink.command_output("", &display).await;
            // G002: `/fusion` spawns a background `local_fusion` task and
            // returns `Handled` immediately (design: no auto-turn). In print
            // mode nothing else keeps the process alive, so without this the
            // worker (and every dispatched panel) died with the process and
            // the user got only a 9-char task id for a run that may never
            // have produced output. `SlashDispatchResult` carries no
            // structured task id (§0: not this package's type to widen), so
            // recover it from the `/fusion` `Done` display's own format
            // (`fusion_command.rs`: `"{task_id}  {preset}  {scope}"`) and
            // await that one task to a terminal status before returning.
            match local_fusion_task_id_to_await(input, &display) {
                Some(task_id) => {
                    let outcome =
                        await_local_fusion_result(task_id, runtime.task_registry.as_ref(), sink)
                            .await;
                    fusion_result_exit_code(outcome.as_ref())
                }
                None => fusion_spawn_failure_exit_code(input, &display),
            }
        }
        // A prompt-expanding command (`/loop`, Markdown/Plugin): run the expanded
        // prompt AS a turn through the orchestrator (claude-code `type: "prompt"`)
        // instead of just printing it, so a `/loop` invocation actually schedules
        // + executes.
        SlashDispatchResult::RunAsTurn { prompt } => {
            sink.turn_start().await;
            let turn_result = runtime.orchestrator.run_turn(&prompt).await;
            stop_background_agents_at_budget(
                max_budget_usd,
                runtime.orchestrator.as_ref(),
                runtime.task_registry.as_ref(),
            )
            .await;
            match turn_result {
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

#[cfg(test)]
#[path = "run/tests.rs"]
mod tests;
