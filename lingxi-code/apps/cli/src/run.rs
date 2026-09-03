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
use permission;
use platform_api::{
    FileSystem, McpStatus, OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult,
};
use serde_json::{json, Value};
use session::jsonl::loader::{
    list_recent_sessions, select_session_interactive, LoaderError, SessionMetadata,
};
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::sync::Arc;

fn stream_json_error_subtype(err: &orchestrator::OrchestratorError) -> &'static str {
    match err {
        orchestrator::OrchestratorError::MaxTurnsReached { .. } => "error_max_turns",
        orchestrator::OrchestratorError::MaxBudgetReached { .. } => "error_max_budget_usd",
        _ => "error_during_execution",
    }
}

/// Claude Code 2.1.217 print-loop budget cleanup (`Wam` + `rcr`). After every
/// main turn, compare the cumulative cost directly with `--max-budget-usd` and
/// stop every running background local agent/workflow once the ceiling is
/// reached. This deliberately does not depend on the main turn returning
/// `error_max_budget_usd`: a natural `end_turn` can itself push the cumulative
/// cost over the ceiling.
async fn stop_background_agents_at_budget(
    max_budget_usd: Option<f64>,
    orchestrator: &dyn OrchestratorHandle,
    task_registry: &dyn platform_api::task_registry::TaskRegistryHandle,
) -> usize {
    let Some(max_budget_usd) = max_budget_usd else {
        return 0;
    };
    let cost = orchestrator.snapshot_cost().await;
    if !budget_reached(max_budget_usd, cost.total_nano_usd) {
        return 0;
    }
    let notice = budget_halt_notice(cost.total_usd, max_budget_usd);
    let announce = || eprintln!("{notice}");
    let Ok(stopped) = task_registry
        .stop_background_agents_for_budget(&announce)
        .await
    else {
        return 0;
    };
    stopped
}

fn budget_reached(max_budget_usd: f64, total_nano_usd: u64) -> bool {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let budget_nano_usd = (max_budget_usd.max(0.0) * 1_000_000_000.0) as u64;
    total_nano_usd >= budget_nano_usd
}

fn budget_halt_notice(total_usd: f64, max_budget_usd: f64) -> String {
    let total_usd = js_to_fixed_2(total_usd);
    format!("Budget limit reached (${total_usd} of ${max_budget_usd}); stopping background agents.")
}

async fn emit_prompt_suggestion_if_enabled(
    argv: &Argv,
    runtime: &Runtime,
    stream: &Arc<StreamJsonStream>,
) {
    if argv.prompt_suggestions_enabled() != Some(true) {
        return;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut handle = tokio::spawn(generate_and_emit_prompt_suggestion(
        runtime.orchestrator.clone(),
        stream.clone(),
        cancel.clone(),
    ));
    if tokio::time::timeout(std::time::Duration::from_secs(30), &mut handle)
        .await
        .is_err()
    {
        cancel.cancel();
        handle.abort();
    }
}

async fn generate_and_emit_prompt_suggestion(
    orchestrator: Arc<orchestrator::ConversationOrchestrator>,
    stream: Arc<StreamJsonStream>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let Ok(suggestion) = orchestrator
        .generate_prompt_suggestion_query(cancel.clone())
        .await
    else {
        return;
    };
    if cancel.is_cancelled() {
        return;
    }
    let Some(suggestion) = suggestion else {
        return;
    };
    stream.emit_prompt_suggestion(&suggestion).await;
}

fn spawn_prompt_suggestion_if_enabled(
    argv: &Argv,
    runtime: &Runtime,
    stream: &Arc<StreamJsonStream>,
) -> Option<(
    tokio_util::sync::CancellationToken,
    tokio::task::JoinHandle<()>,
)> {
    if argv.prompt_suggestions_enabled() != Some(true) {
        return None;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let handle = tokio::spawn(generate_and_emit_prompt_suggestion(
        runtime.orchestrator.clone(),
        stream.clone(),
        cancel.clone(),
    ));
    Some((cancel, handle))
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
        return run_slash_command_with_budget(&prompt, runtime, argv.max_budget_usd, sink).await;
    }

    // Structured-output branch (`--json-schema`): `engine_desktop::build` wires
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
            return run_structured_output(
                runtime,
                &prompt,
                &slot,
                &schema,
                argv.max_budget_usd,
                sink,
            )
            .await;
        }
    }

    // Non-slash branch — drive the orchestrator turn loop. Without a real
    // ANTHROPIC_API_KEY this returns 401; we surface the error verbatim.
    sink.turn_start().await;
    let turn_result = runtime.orchestrator.run_turn(&prompt).await;
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

const FILE_SUGGESTION_LIMIT: usize = 15;
const FILE_SUGGESTION_MAX_INDEXED_PATHS: usize = 20_000;
const FILE_SUGGESTION_INDEX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Per-process file-index cache used by the stream-json `file_suggestions`
/// control request. Claude Code starts the tracked-file refresh in the
/// background and searches whatever portion of the index is already ready, so
/// the first non-trivial query may legitimately return an empty list.
#[derive(Clone, Default)]
struct StreamFileSuggestionIndex {
    paths: Arc<tokio::sync::RwLock<Vec<String>>>,
    root: Arc<tokio::sync::RwLock<Option<PathBuf>>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
    refresh_started: Arc<std::sync::atomic::AtomicBool>,
}

impl StreamFileSuggestionIndex {
    async fn suggestions(&self, cwd: &std::path::Path, query: &str) -> Vec<String> {
        self.prepare_root(cwd).await;
        if matches!(query, "" | "." | "./") {
            self.start_refresh(cwd.to_path_buf());
            return list_cwd_suggestions(cwd).await;
        }

        self.start_refresh(cwd.to_path_buf());
        let expanded_query = expand_home_query(query);
        if std::path::Path::new(&expanded_query).is_absolute() {
            return absolute_file_suggestions(query, &expanded_query).await;
        }
        fuzzy_file_suggestions(
            &self.paths.read().await,
            &expanded_query,
            FILE_SUGGESTION_LIMIT,
        )
    }

    async fn prepare_root(&self, cwd: &std::path::Path) {
        use std::sync::atomic::Ordering;

        let mut root = self.root.write().await;
        if root.as_deref() == Some(cwd) {
            return;
        }
        *root = Some(cwd.to_path_buf());
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.refresh_started.store(false, Ordering::Release);
        self.paths.write().await.clear();
    }

    fn start_refresh(&self, cwd: PathBuf) {
        use std::sync::atomic::Ordering;

        if self
            .refresh_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let paths = Arc::clone(&self.paths);
        let generation = Arc::clone(&self.generation);
        let refresh_generation = generation.load(Ordering::Acquire);
        tokio::spawn(async move {
            let indexed = build_file_suggestion_index(&cwd).await;
            if generation.load(Ordering::Acquire) == refresh_generation {
                *paths.write().await = indexed;
            }
        });
    }
}

async fn list_cwd_suggestions(cwd: &std::path::Path) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(cwd).await else {
        return Vec::new();
    };
    let mut suggestions = Vec::new();
    while suggestions.len() < FILE_SUGGESTION_LIMIT {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            name.push(std::path::MAIN_SEPARATOR);
        }
        suggestions.push(name);
    }
    suggestions
}

fn expand_home_query(query: &str) -> String {
    let Some(rest) = query.strip_prefix('~') else {
        return query.to_string();
    };
    let Some(home) = dirs::home_dir() else {
        return query.to_string();
    };
    format!("{}{}", home.to_string_lossy(), rest)
}

async fn absolute_file_suggestions(original_query: &str, expanded_query: &str) -> Vec<String> {
    let path = std::path::Path::new(expanded_query);
    let has_trailing_separator = expanded_query
        .chars()
        .last()
        .is_some_and(|character| matches!(character, '/' | '\\'));
    let (directory, needle) = if has_trailing_separator {
        (path, "")
    } else {
        (
            path.parent().unwrap_or_else(|| std::path::Path::new(".")),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        )
    };

    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut kinds = std::collections::HashMap::new();
    while kinds.len() < FILE_SUGGESTION_MAX_INDEXED_PATHS {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        let is_dir = entry.file_type().await.is_ok_and(|kind| kind.is_dir());
        kinds.insert(name, is_dir);
    }

    let mut names: Vec<String> = kinds.keys().cloned().collect();
    names.sort();
    let selected = if needle.is_empty() {
        names.into_iter().take(FILE_SUGGESTION_LIMIT).collect()
    } else {
        fuzzy_file_suggestions(&names, needle, FILE_SUGGESTION_LIMIT)
    };
    let home = dirs::home_dir();
    selected
        .into_iter()
        .map(|name| {
            let full = directory.join(&name);
            let mut rendered = render_absolute_suggestion(original_query, &full, home.as_deref());
            if kinds.get(&name).copied().unwrap_or(false) {
                rendered.push(std::path::MAIN_SEPARATOR);
            }
            rendered
        })
        .collect()
}

fn render_absolute_suggestion(
    original_query: &str,
    full: &std::path::Path,
    home: Option<&std::path::Path>,
) -> String {
    if original_query.starts_with('~') {
        if let Some(relative) = home.and_then(|home| full.strip_prefix(home).ok()) {
            let mut rendered = String::from("~");
            if !relative.as_os_str().is_empty() {
                rendered.push(std::path::MAIN_SEPARATOR);
                rendered.push_str(&relative.to_string_lossy());
            }
            return rendered;
        }
    }
    full.to_string_lossy().into_owned()
}

async fn build_file_suggestion_index(cwd: &std::path::Path) -> Vec<String> {
    let mut git = tokio::process::Command::new("git");
    git.args([
        "-c",
        "core.quotepath=false",
        "ls-files",
        "--recurse-submodules",
        "-z",
    ])
    .current_dir(cwd);
    let mut files = match collect_command_items(
        git,
        FILE_SUGGESTION_MAX_INDEXED_PATHS,
        FILE_SUGGESTION_INDEX_TIMEOUT,
    )
    .await
    {
        Some(files) => files,
        None => {
            let mut rg = tokio::process::Command::new("rg");
            rg.args([
                "--files", "--null", "--follow", "--hidden", "--glob", "!.git/", "--glob",
                "!.svn/", "--glob", "!.hg/", "--glob", "!.jj/",
            ])
            .current_dir(cwd);
            collect_command_items(
                rg,
                FILE_SUGGESTION_MAX_INDEXED_PATHS,
                FILE_SUGGESTION_INDEX_TIMEOUT,
            )
            .await
            .unwrap_or_default()
        }
    };

    let mut directories = std::collections::BTreeSet::new();
    for file in &files {
        if directories.len() >= FILE_SUGGESTION_MAX_INDEXED_PATHS {
            break;
        }
        let mut parent = std::path::Path::new(file).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() || path == std::path::Path::new(".") {
                break;
            }
            directories.insert(format!(
                "{}{}",
                path.to_string_lossy(),
                std::path::MAIN_SEPARATOR
            ));
            parent = path.parent();
        }
    }
    let mut indexed: Vec<String> = directories
        .into_iter()
        .take(FILE_SUGGESTION_MAX_INDEXED_PATHS)
        .collect();
    let remaining = FILE_SUGGESTION_MAX_INDEXED_PATHS.saturating_sub(indexed.len());
    indexed.extend(files.drain(..files.len().min(remaining)));
    indexed
}

async fn collect_command_items(
    mut command: tokio::process::Command,
    limit: usize,
    timeout: std::time::Duration,
) -> Option<Vec<String>> {
    use tokio::io::AsyncBufReadExt as _;

    if limit == 0 {
        return Some(Vec::new());
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut items = Vec::new();
    let read = tokio::time::timeout(timeout, async {
        loop {
            let mut bytes = Vec::new();
            let read = reader.read_until(0, &mut bytes).await?;
            if read == 0 {
                return Ok::<bool, std::io::Error>(false);
            }
            if bytes.last() == Some(&0) {
                bytes.pop();
            }
            if bytes.is_empty() {
                continue;
            }
            items.push(String::from_utf8_lossy(&bytes).into_owned());
            if items.len() >= limit {
                return Ok(true);
            }
        }
    })
    .await;

    match read {
        Ok(Ok(hit_limit)) => {
            if hit_limit {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Some(items)
            } else {
                child
                    .wait()
                    .await
                    .ok()
                    .filter(std::process::ExitStatus::success)
                    .map(|_| items)
            }
        }
        Ok(Err(_)) | Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            None
        }
    }
}

fn fuzzy_file_suggestions(paths: &[String], query: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    if query.is_empty() {
        let mut top_level = std::collections::BTreeSet::new();
        for path in paths {
            let component = path
                .split(['/', '\\'])
                .next()
                .filter(|part| !part.is_empty());
            if let Some(component) = component {
                top_level.insert(component.to_string());
            }
        }
        return top_level.into_iter().take(limit).collect();
    }

    let case_sensitive = query != query.to_lowercase();
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let needle_chars: Vec<char> = needle.chars().take(64).collect();
    if needle_chars.is_empty() {
        return Vec::new();
    }

    let mut ranked = Vec::new();
    for path in paths {
        let candidate = if case_sensitive {
            path.clone()
        } else {
            path.to_lowercase()
        };
        let chars: Vec<char> = candidate.chars().collect();
        let mut positions = Vec::with_capacity(needle_chars.len());
        let mut cursor = 0;
        let mut matched = true;
        for needle in &needle_chars {
            let Some(relative) = chars[cursor..].iter().position(|ch| ch == needle) else {
                matched = false;
                break;
            };
            let position = cursor + relative;
            positions.push(position);
            cursor = position + 1;
        }
        if !matched {
            continue;
        }

        let mut adjacency = 0_i64;
        let mut gap_cost = 0_i64;
        for pair in positions.windows(2) {
            let gap = pair[1].saturating_sub(pair[0] + 1);
            if gap == 0 {
                adjacency += 4;
            } else {
                gap_cost += 3 + gap as i64;
            }
        }
        let mut score = needle_chars.len() as i64 * 16 + adjacency - gap_cost;
        for (index, position) in positions.iter().enumerate() {
            if *position == 0 {
                if index == 0 {
                    score += 8;
                }
                continue;
            }
            let previous = chars[*position - 1];
            if matches!(previous, '/' | '\\' | '-' | '_' | '.' | ' ') {
                score += 8;
            } else if previous.is_ascii_lowercase() && chars[*position].is_ascii_uppercase() {
                score += 6;
            }
        }
        score += 32_i64.saturating_sub((chars.len() / 4) as i64);
        ranked.push((score, path));
    }
    ranked.sort_by(|(left_score, left_path), (right_score, right_path)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_path.cmp(right_path))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, path)| path.clone())
        .collect()
}

async fn dispatch_control_request(
    subtype: &str,
    request_id: &str,
    frame: &serde_json::Value,
    writer: &ControlPlaneWriter,
    cancel_tx: &tokio::sync::watch::Sender<bool>,
    lifecycle: &crate::queued_commands::QueueLifecycle,
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    task_registry: &Arc<tasks::registry::TaskRegistry>,
    session_cwd: &Arc<tool_api::SessionCwd>,
    control_plane: &Arc<crate::control_plane::StdioControlPlane>,
    end_notify: &Arc<tokio::sync::Notify>,
    init_commands: &[serde_json::Value],
    init_agents: &[serde_json::Value],
    init_models: &[serde_json::Value],
    init_account: &serde_json::Value,
    init_fast_mode_state: &'static str,
    init_fast_mode_disabled_reason: Option<&'static str>,
    file_suggestions: &StreamFileSuggestionIndex,
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
                init_fast_mode_state,
                init_fast_mode_disabled_reason,
            );
            writer.reply_success(request_id, Some(payload));
        }
        "interrupt" => {
            // §2.2 #1: cancel the per-turn token, then send the interrupt
            // RECEIPT (2.1.220 `interrupt_receipt_v1`, advertised on
            // system/init). Live-captured contract:
            //   plain            → `{"still_queued":[uuids…]}` — queued async
            //                      user messages SURVIVE the interrupt;
            //   cancel_queued:true → sweep them too (2.1.219
            //                      `interrupt_cancel_queued_v1`): a terminal
            //                      `command_lifecycle`/`cancelled` frame per
            //                      uuid, then `{"still_queued":[],
            //                      "cancelled":[uuids…]}`. Idempotent — a
            //                      repeat interrupt lists nothing twice.
            // Cancel the token owned by the turn that is live NOW. Do not leave
            // a sticky watch value behind when the CLI is between turns: that
            // would cancel the next queued user message instead of the turn the
            // host intended to interrupt.
            if control_plane.cancel_active_turn().await {
                let _ = cancel_tx.send(true);
            } else {
                let _ = cancel_tx.send(false);
            }
            if field("cancel_queued").and_then(Value::as_bool) == Some(true) {
                let cancelled = lifecycle.queued.cancel_all_queued();
                for uuid in &cancelled {
                    lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_CANCELLED);
                }
                writer.reply_success(
                    request_id,
                    Some(json!({"still_queued": [], "cancelled": cancelled})),
                );
            } else {
                writer.reply_success(
                    request_id,
                    Some(json!({"still_queued": lifecycle.queued.still_queued()})),
                );
            }
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
            match orchestrator
                .switch_model_with_source(&target, None, "sdk")
                .await
            {
                Ok(()) => writer.reply_success(request_id, None),
                Err(e) => writer.reply_error(request_id, &e.to_string()),
            }
        }
        "set_max_thinking_tokens" => {
            let max_tokens = match field("max_thinking_tokens") {
                Some(Value::Null) => None,
                Some(Value::Number(value)) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
                None | Some(_) => None,
            };
            let max_tokens_valid =
                matches!(field("max_thinking_tokens"), Some(Value::Null)) || max_tokens.is_some();
            let display_valid = match field("thinking_display") {
                None | Some(Value::Null) => true,
                Some(Value::String(value)) => value == "summarized" || value == "omitted",
                Some(_) => false,
            };
            if !max_tokens_valid || !display_valid {
                writer.reply_error(
                    request_id,
                    "set_max_thinking_tokens: max_thinking_tokens must be an integer or null and thinking_display must be \"summarized\", \"omitted\", or null",
                );
                return;
            }
            let thinking = match max_tokens {
                Some(0) => llm_client::model::thinking::ThinkingConfig::Disabled,
                Some(budget_tokens) => {
                    llm_client::model::thinking::ThinkingConfig::Enabled { budget_tokens }
                }
                None => llm_client::model::thinking::ThinkingConfig::Adaptive,
            };
            orchestrator.set_thinking_config(thinking);
            orchestrator.set_thinking_display(field("thinking_display").and_then(Value::as_str));
            writer.reply_success(request_id, None);
        }
        "rename_session" => {
            let title = field("title").and_then(Value::as_str).unwrap_or("");
            if title.trim().is_empty() {
                writer.reply_error(request_id, "title must be non-empty");
                return;
            }
            match orchestrator.rename_session(title.to_string()).await {
                Ok(()) => writer.reply_success(request_id, None),
                Err(err) => writer.reply_error(request_id, &format!("rename_session: {err}")),
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
                // The control-channel `stopTask` — claude-code's `source:"user"`
                // caller, which inherits `killedBy = "user"`.
                let _ = task_registry.kill_with_reason(task_id, "user").await;
            }
            writer.reply_success(request_id, Some(json!({})));
        }
        "register_repo_root" => {
            let request_value = frame.get("request").cloned().unwrap_or_else(|| json!({}));
            match serde_json::from_value::<platform_api::RegisterRepoRootRequest>(request_value) {
                Ok(request) if !request.path.trim().is_empty() => {
                    match orchestrator.register_repo_root(request).await {
                        Ok(outcome) => match serde_json::to_value(outcome) {
                            Ok(value) => writer.reply_success(request_id, Some(value)),
                            Err(error) => writer.reply_error(
                                request_id,
                                &format!("register_repo_root: failed to encode response: {error}"),
                            ),
                        },
                        Err(error) => {
                            writer.reply_error(request_id, &format!("register_repo_root: {error}"));
                        }
                    }
                }
                Ok(_) => {
                    writer.reply_error(request_id, "register_repo_root: path must not be empty")
                }
                Err(error) => writer.reply_error(
                    request_id,
                    &format!("register_repo_root: invalid request: {error}"),
                ),
            }
        }
        "set_cwd" => {
            // Move the live session to another directory. This crosses the
            // TRUST boundary — the target's files become readable and writable
            // under the session's rules — so an untrusted directory is
            // confirmed by the client before the move, via the
            // `needs_trust` → `trust_accepted` + `trusted_directory` echo
            // handshake. The decision (and every byte-exact rejection) lives in
            // `permission::set_cwd`; this arm only gathers the facts and
            // performs the move.
            let request = permission::set_cwd::SetCwdRequest {
                path: field("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                trust_accepted: field("trust_accepted").and_then(serde_json::Value::as_bool),
                trusted_directory: field("trusted_directory")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            let trimmed = request.path.trim().to_string();
            let raw = std::path::PathBuf::from(&trimmed);
            let target = if raw.is_absolute() {
                raw
            } else {
                session_cwd.cwd().join(raw)
            };
            // Canonicalise when we can; a path that cannot be canonicalised is
            // reported at the path the user typed, not at a half-resolved one.
            let display = std::fs::canonicalize(&target).unwrap_or(target.clone());
            let display_str = display.to_string_lossy().into_owned();
            let resolved = if !display.exists() {
                permission::set_cwd::ResolvedPath::NotFound(display_str.clone())
            } else if display.is_dir() {
                permission::set_cwd::ResolvedPath::Directory(display_str.clone())
            } else {
                permission::set_cwd::ResolvedPath::NotADirectory(display_str.clone())
            };
            let ctx = permission::set_cwd::SetCwdContext {
                resolved,
                current_cwd: session_cwd.cwd().to_string_lossy().into_owned(),
                // `Cd(…)` rules have no port representation yet, so no rule can
                // block. Left explicit rather than implied: when Cd rules land,
                // this is the one line that has to change.
                blocking_cd_rule: None,
                // No config path (no resolvable home) ⇒ nothing can be
                // recorded as trusted, so the handshake runs — fail closed.
                trusted: migrations::global_config::global_config_path().is_some_and(|p| {
                    migrations::global_config::check_has_trust_dialog_accepted(&p, &display)
                }),
                project_root: permission::set_cwd::project_root_of(&display)
                    .map(|p| p.to_string_lossy().into_owned()),
                // The control channel is served off the turn loop, so a
                // concurrently running turn is exactly what this guards.
                busy: control_plane.is_busy().await,
            };
            match permission::set_cwd::decide_set_cwd(&request, &ctx) {
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::Invalid(message),
                ) => writer.reply_error(request_id, &message),
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::Rejected { reason, message },
                ) => writer.reply_success(
                    request_id,
                    Some(json!({
                        "status": "rejected",
                        "reason": reason.as_str(),
                        "message": message,
                    })),
                ),
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::NeedsTrust {
                        directory,
                        trust_root,
                    },
                ) => {
                    let mut payload = serde_json::Map::new();
                    payload.insert("status".into(), json!("needs_trust"));
                    payload.insert("directory".into(), json!(directory));
                    // Omitted, not null, when there is nothing useful to offer.
                    if let Some(root) = trust_root {
                        payload.insert("trust_root".into(), json!(root));
                    }
                    writer.reply_success(request_id, Some(Value::Object(payload)));
                }
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::AlreadyThere { cwd },
                ) => writer.reply_success(
                    request_id,
                    Some(json!({
                        "status": "ok",
                        "cwd": cwd,
                        "changed": false,
                        "transcript_relocated": true,
                    })),
                ),
                permission::set_cwd::SetCwdDecision::Proceed {
                    directory,
                    mark_trusted,
                } => {
                    // Record the trust BEFORE the move, so a crash in between
                    // leaves a trusted directory the user did approve rather
                    // than a session sitting in one it never confirmed.
                    if mark_trusted {
                        if let Some(cfg) = migrations::global_config::global_config_path() {
                            migrations::global_config::record_trust_accept(&cfg, &display);
                        }
                    }
                    let dir = std::path::PathBuf::from(&directory);
                    let previous = session_cwd.snapshot();
                    // The new cwd becomes the SOLE trusted directory, matching
                    // what `EnterWorktree` and the worktree restore already do.
                    // Any `--add-dir` extras are dropped rather than carried
                    // across: narrowing a trust boundary on a move is the safe
                    // direction, and inheriting the old session's extras into a
                    // directory the user has just been asked to trust would
                    // grant more than the prompt described.
                    let trusted = vec![dir.clone()];
                    session_cwd.swap(dir.clone(), trusted);
                    let transcript_path = match orchestrator.retarget_transcript_for_cwd(&dir).await
                    {
                        Ok(path) => path,
                        Err(error) => {
                            // Do not acknowledge a cwd move if its transcript could
                            // not be rehomed. Restore both cwd and trusted roots so
                            // a subsequent request cannot run against a different
                            // directory while still reading the old session file.
                            session_cwd.swap(previous.0, previous.1);
                            tracing::warn!(%error, "failed to retarget transcript after set_cwd; rolled back cwd");
                            writer.reply_error(
                                request_id,
                                &format!("Could not change directory: {error}"),
                            );
                            return;
                        }
                    };
                    if let Some(path) = transcript_path {
                        if let Err(error) =
                            crate::background_launch::refresh_current_background_launch_identity(
                                &dir, &path,
                            )
                        {
                            let body = crate::mode::rollback_cd_after_launch_identity_failure(
                                orchestrator,
                                session_cwd,
                                previous,
                                &error.to_string(),
                            )
                            .await;
                            tracing::warn!(
                                %error,
                                "rejected set_cwd because background launch identity is stale"
                            );
                            writer.reply_error(request_id, &body);
                            return;
                        }
                    }
                    writer.reply_success(
                        request_id,
                        Some(json!({
                            "status": "ok",
                            "cwd": directory,
                            "changed": true,
                            "transcript_relocated": true,
                        })),
                    );
                }
            }
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
        "set_mcp_permission_mode_override" => {
            let server_name = field("serverName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mode = match field("mode") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s == "default" || s == "auto" => Some(s.as_str()),
                Some(Value::String(s))
                    if matches!(
                        s.as_str(),
                        "acceptEdits"
                            | "auto"
                            | "bypassPermissions"
                            | "default"
                            | "dontAsk"
                            | "plan"
                    ) =>
                {
                    writer.reply_error(
                        request_id,
                        &format!(
                            "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected '{s}'"
                        ),
                    );
                    return;
                }
                _ => {
                    writer.reply_error(
                        request_id,
                        "Cannot set permission mode: must be one of acceptEdits, auto, bypassPermissions, default, dontAsk, plan",
                    );
                    return;
                }
            };
            let known = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .any(|s| s.name == server_name);
            if !known {
                if let Err(e) = orchestrator
                    .set_mcp_permission_mode_override(&server_name, mode)
                    .await
                {
                    writer.reply_error(request_id, &e);
                    return;
                }
                let warning = match mode {
                    Some(_) => format!(
                        "MCP server '{server_name}' is not yet known; override stored but will not apply until a server with that exact name connects."
                    ),
                    None => format!(
                        "MCP server '{server_name}' is not known; no override was present to clear."
                    ),
                };
                writer.reply_success(request_id, Some(json!({"warning": warning})));
                return;
            }

            match orchestrator
                .set_mcp_permission_mode_override(&server_name, mode)
                .await
            {
                Ok(()) => writer.reply_success(request_id, None),
                Err(e) => writer.reply_error(request_id, &e),
            }
        }
        "end_session" => {
            // §2.2 #2: abort the in-flight turn, ack, then break the loop.
            let _ = cancel_tx.send(true);
            writer.reply_success(request_id, None);
            end_notify.notify_one();
        }
        "file_suggestions" => {
            let query = field("query").and_then(Value::as_str).unwrap_or("");
            let suggestions = file_suggestions
                .suggestions(&session_cwd.cwd(), query)
                .await
                .into_iter()
                .map(|path| json!({"path": path}))
                .collect::<Vec<_>>();
            writer.reply_success(request_id, Some(json!({"suggestions": suggestions})));
        }
        "seed_read_state" => {
            if let (Some(path), Some(mtime)) = (
                field("path").and_then(Value::as_str),
                field("mtime").and_then(Value::as_f64),
            ) {
                let _ = orchestrator.seed_read_state_from_host(path, mtime).await;
            }
            writer.reply_success(request_id, None);
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
        // The orchestrator-free arms (get_binary_version, message_rated,
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
fn pure_control_response(subtype: &str, frame: &serde_json::Value) -> PureControlReply {
    let field = |k: &str| frame.get("request").and_then(|r| r.get(k));
    match subtype {
        // §2.2 #8: `{version, buildTime}`.
        "get_binary_version" => PureControlReply::Success(Some(json!({
            "version": platform_api::CLAUDE_CODE_VERSION,
            "buildTime": ""
        }))),
        // §2.2 #45: telemetry-only; ack with `{}`.
        "message_rated" => PureControlReply::Success(Some(json!({}))),
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
/// 2.1.220 (live re-capture) extends the tail with the remote-control gate
/// booleans and `fast_mode_state` / `fast_mode_disabled_reason`.
/// (NOTE: the separate REPL-bridge handler defaults these to `"normal"` /
/// `["normal"]`, but that bridge is NOT the `-p --input-format stream-json`
/// role this dispatcher models — the observable `-p` truth is the 4-item list.)
fn initialize_response_payload(
    commands: &[serde_json::Value],
    agents: &[serde_json::Value],
    models: &[serde_json::Value],
    account: &serde_json::Value,
    pid: u32,
    fast_mode_state: &str,
    fast_mode_disabled_reason: Option<&str>,
) -> serde_json::Value {
    // 2.1.220 live capture appends five keys after `pid`:
    // `remote_control_auto_enable`, `remote_control_auto_on_by_default`,
    // `ide_rc_auto_enable_gate` (all `false` in a clean sandbox — LingXi has
    // no remote-control feature, an accepted divergence, so `false` is always
    // truthful), then `fast_mode_state` + optional `fast_mode_disabled_reason`.
    let mut payload = json!({
        "commands": commands,
        "agents": agents,
        "output_style": "default",
        "available_output_styles": ["default", "Proactive", "Explanatory", "Learning"],
        "models": models,
        "account": account,
        "pid": pid,
        "remote_control_auto_enable": false,
        "remote_control_auto_on_by_default": false,
        "ide_rc_auto_enable_gate": false,
        "fast_mode_state": fast_mode_state,
    });
    if let Some(reason) = fast_mode_disabled_reason {
        payload["fast_mode_disabled_reason"] = json!(reason);
    }
    payload
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
            let permission_updates = payload
                .get("updatedPermissions")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let decision_classification = payload
                .get("decisionClassification")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| match value {
                    "user_temporary" => Some(
                        platform_api::permission_gate::ToolDecisionClassification::UserTemporary,
                    ),
                    "user_permanent" => Some(
                        platform_api::permission_gate::ToolDecisionClassification::UserPermanent,
                    ),
                    "user_reject" => {
                        Some(platform_api::permission_gate::ToolDecisionClassification::UserReject)
                    }
                    _ => None,
                });
            PermissionOutcome::Allow {
                updated_input,
                permission_updates,
                decision_classification,
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

    // ① system/init frame (emitted once before any turns).
    stream.emit_init().await;

    // ② system/status frame (the init "requesting" handshake).
    stream.emit_status().await;

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
    let ctrl_req_task = tokio::spawn(async move {
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
    });

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

        // Under --replay-user-messages, re-emit the inbound user frame as
        // isReplay:true (the initial-prompt ack for each new turn). Echo the
        // ORIGINAL uuid + content so the host can correlate the ack.
        if argv.replay_user_messages {
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

    // Stream teardown (binary `Hkm`): every uuid still queue-resident gets a
    // terminal `discarded` lifecycle — covers both stdin EOF and `end_session`.
    for uuid in queue_lifecycle.queued.drain_for_discard() {
        queue_lifecycle.emit(&uuid, crate::queued_commands::LIFECYCLE_DISCARDED);
    }

    // Wait for the control dispatcher + response resolver to finish (they exit
    // when their channels close, which happens when the stdin reader task
    // finishes or drops the senders).
    let _ = ctrl_req_task.await;
    let _ = resolver_task.await;

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

/// Port of the 2.1.219 fast-mode reason resolver `JW()` (binary @227890982),
/// narrowed to the inputs reachable on the print/stream-json surface:
///
/// ```js
/// function JW(e){
///   if(!El())return xn()!=="firstParty"?"not_first_party":"disabled_by_env";
///   if(Ke("tengu_penguins_off",null)!==null)return"unknown";
///   if(!Hl(jkt())){…}                                    // model_not_allowed
///   let t=Hr("flagSettings")?.fastMode===!0;
///   if(_n()&&LVt()&&!t)return"sdk_opt_in_required";
///   if(mB.status==="pending"&&…)return"pending";
///   if(mB.status==="disabled"&&…)return mB.reason;       // free|preference|…
///   return null}
/// ```
///
/// * `El()` = firstParty provider && `!CLAUDE_CODE_DISABLE_FAST_MODE` (raw JS
///   truthiness — any non-empty value disables).
/// * `tengu_penguins_off` is a dynamic-config STRING read (`Ke(key,null)`);
///   with no fetcher wired the shipped binary resolves `null` there too, so
///   the port's flag-absent default falls through identically.
/// * `Hl` (org allowed-models policy) has no port surface — managed
///   `allowedModels` is unported, so `model_not_allowed` is unreachable.
/// * `_n()&&LVt()` — the SDK/non-interactive entrypoint check — is
///   constitutively TRUE here: this resolver only runs on the `-p`
///   stream-json/json paths, which ARE the Agent-SDK surface.
/// * The availability prober (`mB`) is unported; its `pending` and
///   `free|preference|extra_usage_disabled|network_error|unknown` arms are
///   unreachable, matching the fall-through `null` of an active status.
fn resolve_fast_mode_disabled_reason(
    first_party: bool,
    sdk_fast_mode_opt_in: bool,
) -> Option<&'static str> {
    if !first_party {
        return Some("not_first_party");
    }
    if std::env::var("CLAUDE_CODE_DISABLE_FAST_MODE").is_ok_and(|v| !v.is_empty()) {
        return Some("disabled_by_env");
    }
    if !sdk_fast_mode_opt_in {
        return Some("sdk_opt_in_required");
    }
    None
}

/// `xn()==="firstParty"` — the ENV-derived API provider (binary @227682549):
///
/// ```js
/// function xn(){if(C_())return"gateway";
///   return Z.CLAUDE_CODE_USE_BEDROCK?"bedrock":Z.CLAUDE_CODE_USE_FOUNDRY?"foundry":
///     Z.CLAUDE_CODE_USE_ANTHROPIC_AWS?"anthropicAws":
///     Z.CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD?"anthropicGoogleCloud":
///     Z.CLAUDE_CODE_USE_MANTLE?"mantle":Z.CLAUDE_CODE_USE_VERTEX?"vertex":"firstParty"}
/// ```
///
/// The model id plays NO part: a Claude model under `CLAUDE_CODE_USE_VERTEX=1`
/// is `vertex`, hence `not_first_party` (live-captured on 2.1.220). `C_()` is
/// the gateway-auth cell, which has no port surface. Value test is the shared
/// `isEnvTruthy` allowlist, as everywhere else the port reads these vars.
const MANAGED_CLOUD_PROVIDER_ENV: [&str; 6] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_VERTEX",
];

/// `xn()==="firstParty"` — see [`MANAGED_CLOUD_PROVIDER_ENV`].
fn env_api_provider_is_first_party() -> bool {
    !MANAGED_CLOUD_PROVIDER_ENV
        .iter()
        .any(|key| platform_api::env::is_env_truthy(std::env::var(key).ok().as_deref()))
}

/// `xn()==="firstParty"` for this session: the env-derived provider
/// (`env_first_party`, from [`env_api_provider_is_first_party`]) AND — LingXi
/// multi-provider divergence, which the oracle has no analogue for — a model
/// actually served by the Anthropic profile. Either half falsy maps to
/// `not_first_party`.
fn session_model_is_first_party(
    env_first_party: bool,
    listings: &[platform_api::orchestrator::ModelListing],
    model: &str,
) -> bool {
    if !env_first_party {
        return false;
    }
    if let Some(listing) = listings.iter().find(|l| l.request_model == model) {
        return listing.provider_id == "anthropic";
    }
    // Not in the live catalog (offline/test builds): bare `claude-*` ids and
    // the `default` pseudo-model route to the first-party Anthropic profile.
    model == "default" || model.to_lowercase().starts_with("claude-")
}

/// Port of `cK(model, fastModeOptIn)` (binary @227895153) — the
/// `fast_mode_state` carried by `system/init`, the `initialize`
/// control_response and every `result` frame:
///
/// ```js
/// function cK(e,t){let r=El()&&QN()&&!!t&&fE(e);
///   if(r&&z0e())return"cooldown";if(r)return"on";return"off"}
/// ```
///
/// * `QN()` is `El()&&fde(undefined)===null` i.e. `El()&&JW()===null`, so
///   `El()&&QN()` collapses to "the disabled reason resolved to null" — the
///   value this function is handed.
/// * `fE(model)` (@227892311) is the canonical registry's `fast_mode`
///   capability. The UI state, initialize response, and request path all
///   consume the same table.
/// * `z0e()` (`"cooldown"`) rides the unported availability prober `mB` — the
///   same dead arm as `JW`'s `pending` / `disabled` branches.
fn resolve_fast_mode_state(
    model: &str,
    fast_mode_disabled_reason: Option<&str>,
    sdk_fast_mode_opt_in: bool,
) -> &'static str {
    let model_supports_fast_mode = platform_api::model_capabilities::has_capability(
        model,
        platform_api::model_capabilities::ModelCapability::FastMode,
    );
    if fast_mode_disabled_reason.is_none() && sdk_fast_mode_opt_in && model_supports_fast_mode {
        "on"
    } else {
        "off"
    }
}

/// `Hr("flagSettings")?.fastMode===!0` — the Agent-SDK fast-mode opt-in
/// carried by `--settings` (inline JSON or a settings-file path). Strictly
/// boolean `true`, like the oracle's `===!0`.
fn flag_settings_fast_mode_opt_in(settings: Option<&str>) -> bool {
    let Some(raw) = settings else { return false };
    let trimmed = raw.trim();
    let text = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        match std::fs::read_to_string(trimmed) {
            Ok(t) => t,
            Err(_) => return false,
        }
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("fastMode").and_then(serde_json::Value::as_bool))
        == Some(true)
}

/// Map a model's `request_model` string to its capability flags.
///
/// Returns `(supportsEffort, supportedEffortLevels, supportsAdaptiveThinking,
///           supportsFastMode, supportsAutoMode)`.
///
/// Refreshed to the 2.1.220 registry truth. The binary's initialize models
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
    // The "default" pseudo-model: the binary computes capabilities on the
    // RESOLVED model (`r = R_()` for the Default row); lingxi's default
    // resolves to claude-sonnet-5 (2.1.197/198, M1).
    if request_model.eq_ignore_ascii_case("default") {
        return model_capabilities("claude-sonnet-5");
    }
    let capabilities =
        platform_api::model_capabilities::initialization_capabilities_for(request_model);
    (
        capabilities.supports_effort,
        capabilities.supported_effort_levels.to_vec(),
        capabilities.supports_adaptive_thinking,
        capabilities.supports_fast_mode,
        capabilities.supports_auto_mode,
    )
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
            exit_codes::SUCCESS
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

/// (CLI-16, cc 2.1.238) `--rewind-files <user-message-id>` — "Restore files to
/// state at the specified user message and exit (requires --resume)"
/// (oracle @307409495; hidden, present in 2.1.220 too). The port had the whole
/// machinery — [`session::file_history::rewind_from_disk`] rebuilds a
/// `FileHistory` from the persisted transcript and restores the tracked
/// backups — but no argv seam ever reached it.
///
/// Oracle order (@307222470, inside `runHeadless` after the transcript loads):
///
/// ```js
/// if(c.rewindFiles){
///   let ze=L.find((Ze)=>Ze.uuid===c.rewindFiles);
///   if(!ze||ze.type!=="user"){…`Error: --rewind-files requires a user message UUID, but ${c.rewindFiles} is not a user message in this session`; exit(1)}
///   let Te=await ZRy(c.rewindFiles,r(),!1);
///   if(!Te.canRewind){…`Error: ${Te.error||"Unexpected error"}`; exit(1)}
///   …
///   bl(`Files rewound to state at message ${c.rewindFiles}\n`), exit(0)}
/// ```
///
/// The two argv gates that precede it (`requires --resume`, `cannot be used
/// with a prompt`) live in [`Argv::validate_truncating_resume_args`] so they
/// fire in the oracle's position, before any session load.
///
/// NOT ported: the `skippedLinks` warning line, which counts symlinked tracked
/// paths the oracle's checkpoint layer refuses — the port's `FileHistory` has
/// no such skip list, so emitting the sentence would report a filter that never
/// ran.
pub(crate) async fn run_rewind_files(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    let Some(target) = argv.rewind_files.as_deref() else {
        return exit_codes::SUCCESS;
    };
    // `--rewind-files` is `requires --resume`-gated, so the session is whatever
    // `--resume` names. A bare `--resume` (picker) has no id to rewind against.
    let raw = argv.resume.as_deref().unwrap_or("").trim();
    let session_id = match resolve_session_id(raw) {
        Ok(id) => id,
        Err(error) => {
            sink.error("runtime", &error.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    let messages = match load_resume_session(session_id).await {
        Ok(m) => m,
        Err(error) => {
            sink.error("runtime", &error.to_string()).await;
            return exit_codes::RUNTIME_ERROR;
        }
    };
    // `L.find(e=>e.uuid===c.rewindFiles)` then `!ze || ze.type!=="user"` — one
    // message, byte-exact, for both the missing and the wrong-kind case.
    let names_user_entry = messages
        .iter()
        .any(|m| m.uuid == target && m.message_type == "user");
    if !names_user_entry {
        eprintln!(
            "Error: --rewind-files requires a user message UUID, but {target} is not a user message in this session"
        );
        return exit_codes::ARGV_ERROR;
    }
    let message_id = match uuid::Uuid::parse_str(target) {
        Ok(id) => id,
        Err(_) => {
            eprintln!(
                "Error: --rewind-files requires a user message UUID, but {target} is not a user message in this session"
            );
            return exit_codes::ARGV_ERROR;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match session::file_history::rewind_from_disk(
        &lingxi_home_dir(),
        &cwd.to_string_lossy(),
        session_id,
        message_id,
    )
    .await
    {
        Ok(_) => {
            // `bl(...)` is the stdout writer; the oracle's literal already ends
            // in `\n`, which `println!` supplies.
            println!("Files rewound to state at message {target}");
            exit_codes::SUCCESS
        }
        // `if(!Te.canRewind){process.stderr.write(`Error: ${Te.error||"Unexpected error"}`)}`
        Err(error) => {
            let detail = if error.is_empty() {
                "Unexpected error".to_string()
            } else {
                error
            };
            eprintln!("Error: {detail}");
            exit_codes::RUNTIME_ERROR
        }
    }
}

/// Resolve the saved effort for resume paths that know their target before the
/// runtime is constructed (`--resume <uuid>` and `--continue`). An explicit
/// `--effort` always wins. Picker paths resolve after selection in
/// [`mount_resumed_tui`].
pub(crate) async fn inherited_resume_effort(argv: &Argv) -> Option<String> {
    if argv.effort.is_some() {
        return None;
    }
    let session_id = if argv.continue_session {
        load_resume_rows().await.ok()?.first()?.uuid
    } else {
        let raw = argv.resume.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        uuid::Uuid::parse_str(raw).ok()?
    };
    let messages = load_resume_session(session_id).await.ok()?;
    orchestrator::runtime_metadata_from_messages(&messages).effort
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
fn filter_rows_by_pr(rows: Vec<SessionMetadata>, from_pr: Option<&str>) -> Vec<SessionMetadata> {
    let Some(raw) = from_pr else { return rows };
    if raw.is_empty() {
        return rows
            .into_iter()
            .filter(|row| row.pr_number.is_some())
            .collect();
    }
    match parse_pr_value(raw) {
        Some(number) => rows
            .into_iter()
            .filter(|row| row.pr_number == Some(number))
            .collect(),
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
    session::jsonl::parse_pr_number(raw)
}

/// `--resume <uuid>` — the concrete-id path.
///
/// Parses the arg as a UUID, then (SESSION.4) verifies the session actually
/// exists on disk via the cross-worktree session loader BEFORE reporting success: a valid-but-
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
    // `t.resume.trim()` — the oracle trims before deciding id-vs-title.
    let arg = argv.resume.as_deref().unwrap_or("").trim();
    let session_id = match resolve_session_id(arg) {
        Ok(id) => id,
        // Not a UUID: fall back to a session-TITLE lookup before failing.
        Err(uuid_error) => match resolve_resume_title(arg).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                // Empty arg keeps the original uuid-parse error — there is no
                // title to look up and the oracle's title copy would misdescribe
                // it.
                sink.error("runtime", &uuid_error.to_string()).await;
                return exit_codes::RUNTIME_ERROR;
            }
            Err(message) => {
                sink.error("runtime", &message).await;
                return exit_codes::RUNTIME_ERROR;
            }
        },
    };
    resume_resolved_session(argv, runtime, sink, session_id).await
}

/// Resolve `--resume <title>` to a session id — the `!s && i` arm of the
/// oracle's print entrypoint (2.1.220 @246508120):
///
/// ```js
/// let u = await OEe(i, {exact:!0});
/// if (u.length === 1) { let d = zS(u[0]); if (d) s = Cbi(d) }
/// else if (u.length > 1) { …"matches N sessions. Pass one of these session IDs to disambiguate:" }
/// ```
///
/// EXACT matching — `--resume` passes `{exact:!0}`, unlike the `/resume`
/// argument-completer which substring-matches with a limit of 10. Searching a
/// substring here would resume an arbitrary session on a partial title.
///
/// Returns `Ok(None)` for an empty argument (nothing to search), `Ok(Some(id))`
/// on a unique hit, and `Err(message)` for a no-match, an ambiguous match, or a
/// real catalog I/O failure. A genuinely empty catalog still follows the
/// no-match copy; unreadable/corrupt storage must not masquerade as one.
async fn resolve_resume_title(arg: &str) -> Result<Option<uuid::Uuid>, String> {
    if arg.is_empty() {
        return Ok(None);
    }
    // The oracle loads EVERY log for the project before filtering; the picker's
    // 5-row cap is a display limit and must not silently bound the search.
    let rows = match load_resume_rows_all().await {
        Ok(rows) => rows,
        Err(LoaderError::EmptyDirectory) => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    resolve_resume_title_from(rows, arg)
}

/// The pure resolution half of [`resolve_resume_title`], split out so the
/// one-match / no-match / ambiguous arms are unit-testable without touching the
/// environment or the process cwd — the same split [`load_resume_rows_from`]
/// exists for.
fn resolve_resume_title_from(
    rows: Vec<SessionMetadata>,
    arg: &str,
) -> Result<Option<uuid::Uuid>, String> {
    if arg.is_empty() {
        return Ok(None);
    }
    let matches = session::jsonl::search_sessions_by_custom_title(rows, arg, true, None);
    match matches.len() {
        0 => Err(resume_title_not_found(arg)),
        1 => Ok(Some(matches[0].uuid)),
        _ => Err(resume_title_ambiguous(arg, &matches)),
    }
}

/// `--resume <title>` matched nothing. Oracle (@246508120):
///
/// ```js
/// let u = "Error: --resume requires a valid session ID or session title when used with
///          --print. Usage: claude -p --resume <session-id|title>";
/// if (i) u += `. Provided value "${i}" is not a UUID and does not match any session title.`;
/// ```
///
/// The base sentence carries no trailing period — the appended clause supplies
/// it. Only the binary name is adapted (`lingxi`, matching the "Resume with:"
/// hint); the product this port ships is not `claude`.
fn resume_title_not_found(arg: &str) -> String {
    format!(
        "Error: --resume requires a valid session ID or session title when used with --print. \
         Usage: lingxi -p --resume <session-id|title>. Provided value \"{arg}\" is not a UUID \
         and does not match any session title."
    )
}

/// `--resume <title>` matched more than one session. Oracle (@246508120):
///
/// ```js
/// let d = u.map((p) => `  ${zS(p) ?? "(unknown)"}  (modified ${p.modified.toISOString()})`)
///          .join(`\n`);
/// `Error: --resume "${i}" matches ${u.length} sessions. Pass one of these session IDs to disambiguate:\n${d}`
/// ```
///
/// TWO spaces lead each row and TWO separate id from `(modified …)`. Rows arrive
/// newest-first from [`session::jsonl::search_sessions_by_custom_title`].
fn resume_title_ambiguous(arg: &str, matches: &[SessionMetadata]) -> String {
    let listed = matches
        .iter()
        .map(|row| {
            let millis = row
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            format!(
                "  {}  (modified {})",
                row.uuid,
                session::jsonl::format_iso_millis(millis)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Error: --resume \"{arg}\" matches {} sessions. Pass one of these session IDs to \
         disambiguate:\n{listed}",
        matches.len()
    )
}

/// Every resumable row for the cwd's project, unbounded — the search corpus for
/// [`resolve_resume_title`] and the backing catalog for the paged picker.
async fn load_resume_rows_all() -> Result<Vec<SessionMetadata>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd.clone()));
    list_recent_sessions(&lingxi_home, &cwd_str, usize::MAX, fs).await
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

    // (CLI-13, cc 2.1.238) The truncating resume. The oracle runs this block
    // IMMEDIATELY after the transcript load and before anything consumes
    // `u.messages` (@307370121), so it sits here — ahead of the prompt/TUI
    // split below, which is where the history is first seeded.
    //
    // Both flags are print-mode-only ("Ignored outside print mode" in their own
    // help text), and `validate_truncating_resume_args` has already enforced
    // `--resume-session-at requires --resume` and `--resume-drops-turn requires
    // --resume-session-at`.
    let messages = if argv.print {
        match crate::resume_truncation::apply_truncating_resume(
            messages,
            argv.resume_session_at.as_deref(),
            argv.resume_drops_turn.as_deref(),
        ) {
            Ok(m) => m,
            Err(message) => {
                sink.error("runtime", &message).await;
                return exit_codes::RUNTIME_ERROR;
            }
        }
    } else {
        messages
    };

    // A follow-up prompt keeps the one-shot path (matches the fresh
    // `Mode::Print` arm): print the resume line then run the turn. The prompt
    // continues the *resumed* conversation only when the orchestrator carries
    // the replayed history — but the supplied `runtime` is the standard
    // sink-adapter build, so seed its session here too before running.
    let (resolved_permission_mode, _notice) = crate::resolve_permission_mode(argv);
    let explicit_permission_mode = resume_has_explicit_permission_mode(argv);
    if let Some(p) = &argv.prompt {
        if !p.trim().is_empty() {
            if let Err(error) = seed_orchestrator_session(
                &runtime.orchestrator,
                session_id,
                &messages,
                resolved_permission_mode,
                explicit_permission_mode,
            )
            .await
            {
                sink.error("runtime", &format!("resume permission mode: {error}"))
                    .await;
                return exit_codes::RUNTIME_ERROR;
            }
            let entries = match load_resume_entries(session_id).await {
                Ok(entries) => entries,
                Err(error) => {
                    sink.error("runtime", &format!("resume deferred tools: {error}"))
                        .await;
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // Prompt snapshots are generic attachments and may not be part of
            // the resumable user/assistant chain used by the seed helper.
            runtime
                .orchestrator
                .restore_resume_runtime_metadata(&entries)
                .await;
            if let Err(error) = orchestrator::replay_deferred_tools_after_resume(
                &runtime.orchestrator,
                orchestrator::deferred_tool_replays_from_messages(&entries),
            )
            .await
            {
                sink.error("runtime", &format!("resume deferred tools: {error}"))
                    .await;
                return exit_codes::RUNTIME_ERROR;
            }
            sink.text(&format!("Resumed session {session_id}\n")).await;
            return run_oneshot(argv, runtime, sink).await;
        }
    }

    // No prompt: mirror the fresh interactive dispatch. Under a full TTY (and no
    // `--no-tui`) run the same trust + dangerous-bypass acknowledgement gates
    // as a fresh TUI before mounting the resumed conversation.
    if crate::mode::is_full_tty() && !argv.no_tui {
        if !crate::mode::startup_preflight(argv).await {
            return exit_codes::RUNTIME_ERROR;
        }
        // Drive the mount, following any in-session `/resume` switch by
        // re-mounting the chosen session in-process (writer retargeted) until
        // the user quits — never an in-place `resume_session` swap. Cold
        // `--resume` carries no in-session model (`None`) — it opens on the
        // config/CLI model, matching claude-code's `--resume`.
        let first = mount_resumed_tui(argv, session_id, messages, None).await;
        return drive_tui_switch_loop(argv, first, Some(session_id), None).await;
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
async fn mount_resumed_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    carried_state: Option<crate::mode::RemountState>,
) -> crate::mode::RunOutcome {
    mount_resumed_tui_inner(argv, session_id, messages, carried_state, None, None, None).await
}

/// Mount a resumed conversation inside a background worker's real PTY.
/// Reuses the standard resume construction and replay path while threading the
/// background registration and its one-shot initial prompt into the normal TUI
/// mount.
pub(crate) async fn mount_background_resumed_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    registration: std::sync::Arc<crate::agents_registry::SessionRegistration>,
    initial_prompt: Option<String>,
    handoff: Option<platform_api::BackgroundingSnapshot>,
) -> crate::mode::RunOutcome {
    mount_resumed_tui_inner(
        argv,
        session_id,
        messages,
        None,
        Some(registration),
        initial_prompt,
        handoff,
    )
    .await
}

async fn mount_resumed_tui_inner(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    carried_state: Option<crate::mode::RemountState>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
    initial_prompt: Option<String>,
    handoff: Option<platform_api::BackgroundingSnapshot>,
) -> crate::mode::RunOutcome {
    // A cold resume inherits the last persisted assistant effort unless the
    // caller explicitly supplied a new `--effort`. Resolve this before build:
    // both the provider adapter and the orchestrator config are immutable once
    // the runtime starts, so seeding it afterwards would update transcript
    // display only while requests silently fell back to the default effort.
    let mut resumed_argv = argv.clone();
    if resumed_argv.effort.is_none() {
        resumed_argv.effort = orchestrator::runtime_metadata_from_messages(&messages).effort;
    }
    let (resolved_permission_mode, _notice) = crate::resolve_permission_mode(&resumed_argv);
    let explicit_permission_mode = resume_has_explicit_permission_mode(argv);
    // Build with the RESUMED session id as the JSONL writer's file name, so new
    // turns append to `<session_id>.jsonl` (the loaded file) instead of forking a
    // fresh-uuid file — the fix for resume splitting a conversation across files.
    let parent_session_id = parent_session_id_from_messages(&messages);
    let mut tui_build = match crate::init::build_runtime_for_tui_inner_with_parent(
        &resumed_argv,
        Some(session_id),
        parent_session_id,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lingxi-cli: tui init failed: {e}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    };
    // ENGINE seed: replay the transcript into the orchestrator's session so a
    // live turn continues the prior conversation.
    let effective_permission_mode = match seed_orchestrator_session(
        &tui_build.runtime.orchestrator,
        session_id,
        &messages,
        resolved_permission_mode,
        explicit_permission_mode,
    )
    .await
    {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("lingxi-cli: resume permission mode failed: {error}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    };
    tui_build.initial_permission_mode = effective_permission_mode;
    let entries = match load_resume_entries(session_id).await {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("lingxi-cli: resume deferred tools failed: {error}");
            return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
        }
    };
    // Restore from the full routed entry set so an off-chain prompt_snapshot
    // attachment survives the CLI's chain-only history seed.
    tui_build
        .runtime
        .orchestrator
        .restore_resume_runtime_metadata(&entries)
        .await;
    if let Err(error) = orchestrator::replay_deferred_tools_after_resume(
        &tui_build.runtime.orchestrator,
        orchestrator::deferred_tool_replays_from_messages(&entries),
    )
    .await
    {
        eprintln!("lingxi-cli: resume deferred tools failed: {error}");
        return crate::mode::RunOutcome::Exit(exit_codes::RUNTIME_ERROR);
    }
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
            let _ = orch
                .switch_model_with_source(&model, profile.as_deref(), "resume")
                .await;
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
    // launch the ratatui backend with that replayed scrollback. Cold `--resume`
    // has no SessionRegistration (fresh launches register). In-process `/resume`
    // remounts pass the live registration through so status + permissionClass
    // stay on `sessions/<pid>.json`. A carried one-shot notice (the `/branch`
    // success confirmation) renders as the newest system cell.
    let mut resumed_messages = tui::replay::rebuild_from_jsonl(&messages);
    if let Some(body) = boot_notice {
        resumed_messages.push(tui_core::message::RenderedMessage::SystemText {
            body,
            timestamp: 0,
            is_error: false,
        });
    }
    crate::mode::run_ratatui_with_initial_state(
        tui_build,
        registration,
        resumed_messages,
        initial_prompt,
        handoff,
    )
    .await
}

/// Recover the source session stamped by `session::branch::create_branch`.
/// Legacy and ordinary transcripts have no marker and intentionally return
/// `None`.
fn parent_session_id_from_messages(messages: &[JsonlMessage]) -> Option<String> {
    messages.iter().find_map(|entry| {
        entry
            .extra
            .get("forkedFrom")?
            .get("sessionId")?
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    })
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
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
) -> i32 {
    drive_tui_switch_loop_inner(argv, first, initial_session_id, registration).await
}

/// Background variant that preserves the worker registration across every
/// `/resume`, `/branch`, and `/rewind` remount.
pub(crate) async fn drive_background_tui_switch_loop(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
    registration: std::sync::Arc<crate::agents_registry::SessionRegistration>,
) -> i32 {
    drive_tui_switch_loop_inner(argv, first, initial_session_id, Some(registration)).await
}

async fn remount_tui(
    argv: &Argv,
    session_id: uuid::Uuid,
    messages: Vec<JsonlMessage>,
    state: Option<crate::mode::RemountState>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
) -> crate::mode::RunOutcome {
    mount_resumed_tui_inner(argv, session_id, messages, state, registration, None, None).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentOpenPlan {
    StayOnCurrent,
    RemountTarget,
    QueueBackgroundResume {
        short: String,
        session_id: Option<String>,
    },
}

fn plan_agent_open(
    target: &tui::bottom_pane::view::AgentSessionTarget,
    disposition: &crate::commands::attach::AttachDisposition,
) -> AgentOpenPlan {
    use crate::commands::attach::AttachDisposition;
    match disposition {
        AttachDisposition::Attached | AttachDisposition::LiveEndpointUnavailable { .. } => {
            AgentOpenPlan::StayOnCurrent
        }
        AttachDisposition::NotFound if target.live => AgentOpenPlan::StayOnCurrent,
        AttachDisposition::NotFound => AgentOpenPlan::RemountTarget,
        AttachDisposition::NotRunning { short, session_id } if target.background => {
            AgentOpenPlan::QueueBackgroundResume {
                short: short.clone(),
                session_id: session_id.clone(),
            }
        }
        AttachDisposition::NotRunning { .. } if target.live => AgentOpenPlan::StayOnCurrent,
        AttachDisposition::NotRunning { .. } => AgentOpenPlan::RemountTarget,
    }
}

fn set_agent_open_notice(
    state: &mut Option<crate::mode::RemountState>,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Some(state) = state.as_mut() {
        state.notice = Some(message.clone());
    }
    eprintln!("lingxi-cli: {message}");
}

async fn drive_tui_switch_loop_inner(
    argv: &Argv,
    first: crate::mode::RunOutcome,
    initial_session_id: Option<uuid::Uuid>,
    registration: Option<std::sync::Arc<crate::agents_registry::SessionRegistration>>,
) -> i32 {
    struct InboxShutdown;
    impl Drop for InboxShutdown {
        fn drop(&mut self) {
            platform_api::uds_inbox::stop_process_inbox();
        }
    }
    let _inbox = InboxShutdown;
    let mut outcome = first;
    // The session currently driving the loop — the fallback for a failed switch.
    let mut current = initial_session_id;
    loop {
        match outcome {
            crate::mode::RunOutcome::Exit(code) => return code,
            crate::mode::RunOutcome::OpenAgentSession { target, mut state } => {
                let home = lingxi_home_dir();
                let selector = target.session_id.to_string();
                let disposition = match crate::commands::attach::attach_target(&home, &selector) {
                    Ok(disposition) => disposition,
                    Err(error) => {
                        set_agent_open_notice(
                            &mut state,
                            format!("couldn't attach to agent {selector}: {error}"),
                        );
                        let Some(current) = current else {
                            return exit_codes::RUNTIME_ERROR;
                        };
                        outcome = crate::mode::RunOutcome::SwitchTo {
                            target: current,
                            state,
                        };
                        continue;
                    }
                };
                let plan = plan_agent_open(&target, &disposition);
                let mount_target = match plan {
                    AgentOpenPlan::RemountTarget => target.session_id,
                    AgentOpenPlan::StayOnCurrent => {
                        if !matches!(
                            disposition,
                            crate::commands::attach::AttachDisposition::Attached
                        ) {
                            set_agent_open_notice(
                                &mut state,
                                format!(
                                    "agent {selector} is still owned by another process; its transcript was not reopened"
                                ),
                            );
                        }
                        let Some(current) = current else {
                            return exit_codes::RUNTIME_ERROR;
                        };
                        current
                    }
                    AgentOpenPlan::QueueBackgroundResume { short, session_id } => {
                        let resolved_session = session_id.as_deref().unwrap_or(&selector);
                        match crate::commands::respawn::queue_resume_for_short_if_safe(
                            &home,
                            &short,
                            resolved_session,
                        ) {
                            Ok(true) => {
                                crate::background_dispatch::ensure_daemon_for_control(&home);
                                match crate::commands::agents::wait_for_auto_resumed_attach(
                                    &home,
                                    resolved_session,
                                ) {
                                    Ok(crate::commands::agents::OpenSessionDisposition::Attached) => {}
                                    Ok(
                                        crate::commands::agents::OpenSessionDisposition::LiveEndpointUnavailable {
                                            ..
                                        }
                                        | crate::commands::agents::OpenSessionDisposition::NotRunning {
                                            ..
                                        },
                                    ) => set_agent_open_notice(
                                        &mut state,
                                        format!(
                                            "agent {selector} is restarting in the background; try opening it again in a moment"
                                        ),
                                    ),
                                    Ok(crate::commands::agents::OpenSessionDisposition::ForegroundResume) => {
                                        set_agent_open_notice(
                                            &mut state,
                                            format!(
                                                "agent {selector} changed while its restart was being queued"
                                            ),
                                        );
                                    }
                                    Err(error) => set_agent_open_notice(
                                        &mut state,
                                        format!("couldn't attach to agent {selector}: {error}"),
                                    ),
                                }
                                let Some(current) = current else {
                                    return exit_codes::RUNTIME_ERROR;
                                };
                                current
                            }
                            // `false` can mean either a terminal row OR that a
                            // concurrent daemon/delete/restart won the exact
                            // state CAS. Only the former proves the transcript
                            // may be mounted as a foreground writer.
                            Ok(false) => {
                                let safely_terminal = crate::agents_registry::read_job(
                                    &home, &short,
                                )
                                .is_some_and(|job| {
                                    crate::agents_registry::job_is_terminal(&job)
                                        && job.phase.as_deref()
                                            != Some(crate::commands::respawn::PHASE_DELETING)
                                        && job.worker_pid.is_none()
                                        && job.worker_proc_start.is_none()
                                });
                                if safely_terminal {
                                    target.session_id
                                } else {
                                    set_agent_open_notice(
                                        &mut state,
                                        format!(
                                            "agent {selector} changed while its restart was being queued"
                                        ),
                                    );
                                    let Some(current) = current else {
                                        return exit_codes::RUNTIME_ERROR;
                                    };
                                    current
                                }
                            }
                            Err(error) => {
                                set_agent_open_notice(
                                    &mut state,
                                    format!("couldn't restart agent {selector}: {error}"),
                                );
                                let Some(current) = current else {
                                    return exit_codes::RUNTIME_ERROR;
                                };
                                current
                            }
                        }
                    }
                };
                outcome = crate::mode::RunOutcome::SwitchTo {
                    target: mount_target,
                    state,
                };
            }
            crate::mode::RunOutcome::SwitchTo { target, state } => {
                match load_resume_session(target).await {
                    Ok(messages) => {
                        current = Some(target);
                        outcome =
                            remount_tui(argv, target, messages, state, registration.clone()).await;
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
                                        outcome = remount_tui(
                                            argv,
                                            fallback,
                                            messages,
                                            state,
                                            registration.clone(),
                                        )
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
                        outcome =
                            remount_tui(argv, mount_target, messages, state, registration.clone())
                                .await;
                    }
                    Err(e) => {
                        // The branch (or fallback) target won't load. Fall back to
                        // the known-good source, mirroring the SwitchTo recovery.
                        eprintln!("lingxi-cli: couldn't open {mount_target}: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                match load_resume_session(fallback).await {
                                    Ok(messages) => {
                                        outcome = remount_tui(
                                            argv,
                                            fallback,
                                            messages,
                                            state,
                                            registration.clone(),
                                        )
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
                        outcome =
                            remount_tui(argv, mount_target, messages, state, registration.clone())
                                .await;
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
                        outcome = remount_tui(
                            argv,
                            mount_target,
                            Vec::new(),
                            state,
                            registration.clone(),
                        )
                        .await;
                    }
                    Err(e) => {
                        eprintln!("lingxi-cli: couldn't open {mount_target} after rewind: {e}");
                        match recover_from_failed_switch(current) {
                            SwitchRecovery::Remount(fallback) => {
                                match load_resume_session(fallback).await {
                                    Ok(messages) => {
                                        outcome = remount_tui(
                                            argv,
                                            fallback,
                                            messages,
                                            state,
                                            registration.clone(),
                                        )
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

/// Seed an already-built orchestrator's in-memory [`lingxi_core::SessionState`] from
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
pub(crate) async fn seed_orchestrator_session(
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    session_id: uuid::Uuid,
    messages: &[JsonlMessage],
    resolved_permission_mode: permission::PermissionMode,
    explicit_cli_permission_mode: bool,
) -> Result<permission::PermissionMode, String> {
    let replayed = orchestrator::state_from_messages(session_id, messages);
    let effective_permission_mode = effective_resume_permission_mode(
        resolved_permission_mode,
        explicit_cli_permission_mode,
        replayed.plan_mode,
    );
    orchestrator
        .set_permission_mode(effective_permission_mode.wire_str())
        .await?;
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
    session.transcript_only_messages = replayed.transcript_only_messages;
    session.compact_summary_messages = replayed.compact_summary_messages;
    session.active_goal = replayed.active_goal;
    // Restore the saved model (recovered from the last assistant line by
    // `state_from_messages`) so a resumed session continues on — and shows — its
    // saved model, not the launch default. `state_from_messages` yields
    // `DEFAULT_MODEL` when the transcript has no assistant lines, which is the
    // correct fallback.
    session.model = replayed.model;
    // Newer transcripts persist the provider profile beside each real
    // assistant response. Legacy transcripts reconstruct `None`, preserving
    // the safe global-by-model-id fallback instead of keeping the launch
    // default provider's stale routing hint.
    session.model_profile = replayed.model_profile;
    session.plan_mode = effective_permission_mode == permission::PermissionMode::Plan;
    if session.plan_mode {
        session.plan_reminder_shown = false;
    }
    drop(session);
    orchestrator
        .sync_active_goal_stop_hook_for_current_state()
        .await;
    orchestrator.restore_resume_runtime_metadata(messages).await;
    Ok(effective_permission_mode)
}

fn resume_has_explicit_permission_mode(argv: &Argv) -> bool {
    argv.permission_mode.is_some() || argv.dangerously_skip_permissions
}

fn effective_resume_permission_mode(
    resolved_permission_mode: permission::PermissionMode,
    explicit_cli_permission_mode: bool,
    replayed_plan_mode: bool,
) -> permission::PermissionMode {
    if explicit_cli_permission_mode {
        resolved_permission_mode
    } else if replayed_plan_mode {
        permission::PermissionMode::Plan
    } else {
        resolved_permission_mode
    }
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
/// the same M5-08 loader rows. A selected UUID is mounted through the standard
/// resumed-TUI path; cancelling exits without constructing a runtime.
async fn run_resume_iocraft(argv: &Argv, sink: &dyn OutputSink) -> i32 {
    if !crate::mode::startup_preflight(argv).await {
        return exit_codes::RUNTIME_ERROR;
    }
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
            let messages = match load_resume_session(uuid).await {
                Ok(messages) => messages,
                Err(error) => {
                    sink.error("runtime", &error.to_string()).await;
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            let first = mount_resumed_tui(argv, uuid, messages, None).await;
            drive_tui_switch_loop(argv, first, Some(uuid), None).await
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
/// asks the M5-08 loader for the complete sorted catalog. The picker itself
/// exposes this in 50-row pages (`tui::resume::RESUME_PAGE_SIZE`), matching
/// Claude Code 2.1.246's `allStatLogs` / `nextIndex` behavior.
async fn load_resume_rows_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<Vec<SessionMetadata>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd.to_path_buf()));
    list_recent_sessions(lingxi_home, &cwd_str, usize::MAX, fs).await
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

async fn load_resume_entries(session_id: uuid::Uuid) -> Result<Vec<JsonlMessage>, LoaderError> {
    let lingxi_home = lingxi_home_dir();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd));
    session::jsonl::load_session_entries_across_worktrees(&lingxi_home, &cwd_str, session_id, fs)
        .await
}

/// Production disk→`Vec<JsonlMessage>` load with the inputs passed in (no env /
/// process-cwd reads) so it is directly testable. Builds the same disk-backed
/// [`platform_posix::PosixFileSystem`] the row loader uses and asks the
/// worktree-aware session loader for the session. This is deliberately the
/// same sibling-worktree scope as [`list_recent_sessions`], so a row surfaced
/// by either resume picker or title search is always loadable.
pub(crate) async fn load_resume_session_from(
    lingxi_home: &std::path::Path,
    cwd: &std::path::Path,
    session_id: uuid::Uuid,
) -> Result<Vec<JsonlMessage>, LoaderError> {
    let cwd_str = cwd.to_string_lossy().into_owned();
    let fs: Arc<dyn FileSystem> = Arc::new(platform_posix::PosixFileSystem::new(cwd.to_path_buf()));
    session::jsonl::load_session_across_worktrees(lingxi_home, &cwd_str, session_id, fs).await
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

    #[test]
    fn agents_open_plan_never_remounts_a_known_or_reported_live_writer() {
        use crate::commands::attach::AttachDisposition;

        let session_id = Uuid::new_v4();
        let live_background = tui::bottom_pane::view::AgentSessionTarget {
            session_id,
            background: true,
            live: true,
        };
        assert_eq!(
            plan_agent_open(&live_background, &AttachDisposition::NotFound),
            AgentOpenPlan::StayOnCurrent
        );
        assert_eq!(
            plan_agent_open(
                &live_background,
                &AttachDisposition::LiveEndpointUnavailable {
                    short: "abcd1234".into(),
                    session_id: Some(session_id.to_string()),
                },
            ),
            AgentOpenPlan::StayOnCurrent
        );

        let stopped_background = tui::bottom_pane::view::AgentSessionTarget {
            live: false,
            ..live_background.clone()
        };
        assert_eq!(
            plan_agent_open(
                &stopped_background,
                &AttachDisposition::NotRunning {
                    short: "abcd1234".into(),
                    session_id: Some(session_id.to_string()),
                },
            ),
            AgentOpenPlan::QueueBackgroundResume {
                short: "abcd1234".into(),
                session_id: Some(session_id.to_string()),
            }
        );

        let stopped_interactive = tui::bottom_pane::view::AgentSessionTarget {
            background: false,
            live: false,
            ..live_background
        };
        assert_eq!(
            plan_agent_open(&stopped_interactive, &AttachDisposition::NotFound),
            AgentOpenPlan::RemountTarget
        );
    }

    #[test]
    fn stream_json_terminal_limits_keep_their_specific_error_subtypes() {
        assert_eq!(
            stream_json_error_subtype(&orchestrator::OrchestratorError::MaxTurnsReached {
                max_turns: 3,
            }),
            "error_max_turns"
        );
        assert_eq!(
            stream_json_error_subtype(&orchestrator::OrchestratorError::MaxBudgetReached {
                budget_nano_usd: 1_500_000_000,
            }),
            "error_max_budget_usd"
        );
        assert_eq!(
            stream_json_error_subtype(&orchestrator::OrchestratorError::Internal("boom".into())),
            "error_during_execution"
        );
    }

    #[test]
    fn structured_output_retries_the_one_turn_cap_when_no_result_was_captured() {
        assert!(structured_output_turn_error_is_retryable(
            &orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 1 }
        ));
        assert!(!structured_output_turn_error_is_retryable(
            &orchestrator::OrchestratorError::Internal("boom".into())
        ));
        assert!(!structured_output_turn_error_is_retryable(
            &orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 3 }
        ));
    }

    #[test]
    fn budget_halt_notice_matches_claude_bytes() {
        assert_eq!(
            budget_halt_notice(1.75, 5.0),
            "Budget limit reached ($1.75 of $5); stopping background agents."
        );
        assert_eq!(
            budget_halt_notice(1.505, 1.5),
            "Budget limit reached ($1.50 of $1.5); stopping background agents."
        );
        assert_eq!(
            budget_halt_notice(1.125, 2.0),
            "Budget limit reached ($1.13 of $2); stopping background agents."
        );
        assert_eq!(
            budget_halt_notice(2.675, 3.0),
            "Budget limit reached ($2.67 of $3); stopping background agents."
        );
    }

    #[test]
    fn budget_reached_matches_claude_print_loop_boundary() {
        assert!(!budget_reached(1.5, 1_499_999_999));
        assert!(budget_reached(1.5, 1_500_000_000));
        assert!(budget_reached(1.5, 1_500_000_001));
    }

    #[test]
    fn orphaned_allow_retains_updated_permissions() {
        let update = json!({
            "type": "setMode",
            "mode": "acceptEdits",
            "destination": "session"
        });
        let outcome = orphan_decision_from_payload(&json!({
            "behavior": "allow",
            "updatedPermissions": [update.clone()]
        }));
        assert_eq!(
            outcome,
            permission::gate::PermissionOutcome::Allow {
                updated_input: None,
                permission_updates: vec![update],
                decision_classification: None,
            }
        );
    }

    /// Write one valid `<uuid>.jsonl` session file (a single first-user message
    /// in the M5-07/M5-08 on-disk format) into `project_dir`, stamp both its
    /// transcript timestamp and mtime from `mtime`, and return the uuid.
    /// `prompt` becomes the row's extracted title.
    fn write_session(project_dir: &std::path::Path, prompt: &str, mtime: SystemTime) -> Uuid {
        let uuid = Uuid::new_v4();
        let path = project_dir.join(format!("{uuid}.jsonl"));
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": chrono::DateTime::<chrono::Utc>::from(mtime)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
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
        for s in [
            "default",
            "DEFAULT",
            "Default",
            "  default  ",
            "\tdefault\n",
        ] {
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

        // Three sessions with staggered transcript timestamps.
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
        // Touch every file to the same later mtime. Claude sorts by
        // min(lastMessageAtMs, file mtime), so the transcript timestamps still
        // determine the order and a forward touch cannot reshuffle the picker.
        let touched = filetime::FileTime::from_system_time(base + Duration::from_secs(60));
        for id in [_oldest, _middle, newest] {
            filetime::set_file_mtime(project_dir.join(format!("{id}.jsonl")), touched).unwrap();
        }

        let rows = load_resume_rows_from(&lingxi_home, &cwd)
            .await
            .expect("loader should produce rows");

        assert_eq!(rows.len(), 3, "all three sessions surface as rows");
        // Newest-first by the clamped transcript activity timestamp.
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
    async fn load_resume_rows_from_keeps_sessions_beyond_the_first_page() {
        let temp = tempfile::TempDir::new().unwrap();
        let lingxi_home = temp.path().join("home");
        let cwd = std::path::PathBuf::from("/tmp/many-sessions");
        let cwd_str = cwd.to_string_lossy().into_owned();
        let project_dir = make_project_dir(&lingxi_home, &cwd_str);
        let base = SystemTime::now();

        for i in 0..55 {
            write_session(
                &project_dir,
                &format!("session {i}"),
                base + Duration::from_secs(i),
            );
        }

        let rows = load_resume_rows_from(&lingxi_home, &cwd)
            .await
            .expect("loader should retain the complete picker catalog");
        assert_eq!(rows.len(), 55);
        assert_eq!(rows[0].title, "session 54");
        assert_eq!(rows[54].title, "session 0");
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
    async fn seed_orchestrator_session_restores_saved_model_and_profile() {
        // Regression (reported): a resumed cross-provider session failed its
        // first LIVE turn with "model unavailable". The engine seeds
        // `model_profile` to the default provider at startup; restoring the
        // saved model but leaving that profile scopes routing to the wrong
        // provider. Seeding must restore the model AND its persisted profile.
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
            "modelProfile": "deepseek",
            "message": {"content": "answered on deepseek", "model": "deepseek-v4-pro"},
        }))
        .expect("valid JsonlMessage");
        let messages = vec![jsonl_line("user", &serde_json::json!("hi")), assistant];
        seed_orchestrator_session(
            &build.runtime.orchestrator,
            Uuid::new_v4(),
            &messages,
            permission::PermissionMode::Default,
            false,
        )
        .await
        .expect("resume seed");

        let handle = build.runtime.orchestrator.session();
        let s = handle.lock().await;
        assert_eq!(
            s.model, "deepseek-v4-pro",
            "resume restores the saved cross-provider model"
        );
        assert_eq!(
            s.model_profile.as_deref(),
            Some("deepseek"),
            "the persisted provider profile replaces the stale startup profile"
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
        seed_orchestrator_session(
            &build.runtime.orchestrator,
            resumed_id,
            &messages,
            permission::PermissionMode::Default,
            false,
        )
        .await
        .expect("resume seed");

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
    async fn seed_resume_restores_open_plan_without_an_explicit_mode() {
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let sid = Uuid::new_v4();
        let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
            "type": "user",
            "uuid": Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": sid.to_string(),
            "timestamp": "2026-08-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.12.0",
            "permissionMode": "plan",
            "message": {"role": "user", "content": "continue the plan"}
        }))
        .expect("valid plan line");

        let effective = seed_orchestrator_session(
            &build.runtime.orchestrator,
            sid,
            std::slice::from_ref(&plan_line),
            permission::PermissionMode::Default,
            false,
        )
        .await
        .expect("resume seed");
        assert_eq!(effective, permission::PermissionMode::Plan);
        assert!(build.runtime.orchestrator.plan_mode().await);
        assert_eq!(
            build.runtime.orchestrator.permission_mode().as_deref(),
            Some("plan"),
            "resume must push the enforcing gate into plan mode too"
        );
    }

    #[tokio::test]
    async fn seed_resume_preserves_an_explicit_plan_mode() {
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let sid = Uuid::new_v4();
        let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
            "type": "user",
            "uuid": Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": sid.to_string(),
            "timestamp": "2026-08-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.12.0",
            "permissionMode": "plan",
            "message": {"role": "user", "content": "continue the plan"}
        }))
        .expect("valid plan line");

        let effective = seed_orchestrator_session(
            &build.runtime.orchestrator,
            sid,
            std::slice::from_ref(&plan_line),
            permission::PermissionMode::Plan,
            true,
        )
        .await
        .expect("resume seed");
        assert_eq!(effective, permission::PermissionMode::Plan);
        assert!(build.runtime.orchestrator.plan_mode().await);
        assert_eq!(
            build.runtime.orchestrator.permission_mode().as_deref(),
            Some("plan"),
            "an explicit --permission-mode plan must keep the session and gate in plan mode"
        );
    }

    #[tokio::test]
    async fn seed_resume_suppresses_transcript_plan_when_cli_mode_is_explicitly_non_plan() {
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let sid = Uuid::new_v4();
        let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
            "type": "user",
            "uuid": Uuid::new_v4().to_string(),
            "parentUuid": null,
            "sessionId": sid.to_string(),
            "timestamp": "2026-08-25T12:00:00.000Z",
            "cwd": "/tmp/workproj",
            "version": "0.12.0",
            "permissionMode": "plan",
            "message": {"role": "user", "content": "continue the plan"}
        }))
        .expect("valid plan line");

        let effective = seed_orchestrator_session(
            &build.runtime.orchestrator,
            sid,
            std::slice::from_ref(&plan_line),
            permission::PermissionMode::Default,
            true,
        )
        .await
        .expect("resume seed");
        assert_eq!(effective, permission::PermissionMode::Default);
        assert!(
            !build.runtime.orchestrator.plan_mode().await,
            "an explicit invocation mode must suppress transcript plan restoration"
        );
        assert_eq!(
            build.runtime.orchestrator.permission_mode().as_deref(),
            Some("default"),
            "the enforcing gate must stay aligned with the explicit non-plan mode"
        );
    }

    #[test]
    fn dangerously_skip_permissions_counts_as_an_explicit_resume_override() {
        let mut argv = tui_argv();
        argv.dangerously_skip_permissions = true;
        assert!(
            resume_has_explicit_permission_mode(&argv),
            "--dangerously-skip-permissions must suppress transcript plan restoration just like an explicit --permission-mode"
        );
        assert_eq!(
            effective_resume_permission_mode(
                permission::PermissionMode::BypassPermissions,
                resume_has_explicit_permission_mode(&argv),
                true,
            ),
            permission::PermissionMode::BypassPermissions
        );
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
    fn file_suggestions_use_subsequence_ranking_and_limit() {
        let paths = vec![
            "src/main.rs".to_string(),
            "src/manager.rs".to_string(),
            "tests/main_test.rs".to_string(),
            "README.md".to_string(),
        ];
        let suggestions = fuzzy_file_suggestions(&paths, "smr", 2);
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0], "src/main.rs");
        assert!(suggestions.iter().all(|path| path.contains(".rs")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_item_collection_stops_at_the_index_bound() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "printf 'a\\0b\\0c\\0'"]);
        assert_eq!(
            collect_command_items(command, 2, std::time::Duration::from_secs(1)).await,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_item_collection_times_out_and_reaps_the_child() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "sleep 1; printf 'late\\0'"]);
        assert_eq!(
            collect_command_items(command, 2, std::time::Duration::from_millis(20)).await,
            None
        );
    }

    #[tokio::test]
    async fn absolute_file_suggestions_use_the_filesystem_coordinate_space() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("alpha.txt"), "").expect("write alpha");
        std::fs::write(dir.path().join("beta.txt"), "").expect("write beta");
        std::fs::create_dir(dir.path().join("archive")).expect("create archive");

        let query = dir.path().join("al");
        let suggestions =
            absolute_file_suggestions(&query.to_string_lossy(), &query.to_string_lossy()).await;
        assert_eq!(
            suggestions,
            vec![dir.path().join("alpha.txt").to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn home_relative_suggestions_preserve_the_tilde_prefix() {
        let home = std::path::Path::new("/home/tester");
        assert_eq!(
            render_absolute_suggestion(
                "~/Doc",
                &home.join("Documents").join("notes.md"),
                Some(home)
            ),
            format!(
                "~{}Documents{}notes.md",
                std::path::MAIN_SEPARATOR,
                std::path::MAIN_SEPARATOR
            )
        );
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
        // 2.1.220 re-capture appends the remote-control gates + fast-mode tail
        // (covered exhaustively by `initialize_payload_tail_matches_2_1_220`).
        let payload = initialize_response_payload(&[], &[], &[], &json!({}), 4242, "off", None);
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
                "remote_control_auto_enable",
                "remote_control_auto_on_by_default",
                "ide_rc_auto_enable_gate",
                "fast_mode_state",
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
        assert_eq!(payload["version"], platform_api::CLAUDE_CODE_VERSION);
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

        // opus-4-7 / opus-4-8 / opus-5: full ladder + adaptive + FAST + auto.
        for m in [
            "claude-opus-4-7",
            "claude-opus-4-8-20260115",
            "claude-opus-5",
            "us.anthropic.claude-opus-5-v1:0",
        ] {
            assert_eq!(
                model_capabilities(m),
                (true, all.clone(), true, true, true),
                "{m}"
            );
        }
        // sonnet-5 / fable-5: full ladder + adaptive + auto, no fast.
        for m in ["claude-sonnet-5", "claude-fable-5-1"] {
            assert_eq!(
                model_capabilities(m),
                (true, all.clone(), true, false, true),
                "{m}"
            );
        }
        // Mythos 5 is present in the 2.1.220 table with no registry
        // capabilities; auto mode is derived separately for modern Claude.
        assert_eq!(
            model_capabilities("claude-mythos-5-1"),
            (false, vec![], false, false, true)
        );
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
    fn pure_message_rated_acks_empty_object() {
        let frame = req("message_rated", json!({"sentiment": "up"}));
        assert_eq!(
            pure_control_response("message_rated", &frame),
            PureControlReply::Success(Some(json!({})))
        );
    }

    fn outbound_line(msg: crate::stream_json::OutboundMsg) -> String {
        match msg {
            crate::stream_json::OutboundMsg::Line(line) => line,
            crate::stream_json::OutboundMsg::StreamEvent(line) => line,
            crate::stream_json::OutboundMsg::Heartbeats(_) => {
                panic!("unexpected heartbeat message")
            }
            crate::stream_json::OutboundMsg::Flush(_) => panic!("unexpected flush message"),
        }
    }

    async fn dispatch_and_capture(
        orch: &Arc<orchestrator::ConversationOrchestrator>,
        task_registry: &Arc<tasks::registry::TaskRegistry>,
        frame: serde_json::Value,
    ) -> serde_json::Value {
        dispatch_and_capture_in(orch, task_registry, frame, &std::env::temp_dir()).await
    }

    /// Serializes the `set_cwd` tests, which redirect `LINGXI_CONFIG_DIR` — a
    /// PROCESS-GLOBAL mutation. Without the lock two of them race and one reads
    /// the other's config home; without the redirect at all they write trust
    /// entries into the developer's REAL `~/.lingxi.json` (which the first
    /// version of these tests did).
    static SET_CWD_CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Redirect the global config home at a tempdir for the duration, so a
    /// `set_cwd` that records trust cannot touch the user's real config.
    struct IsolatedConfigHome {
        _dir: tempfile::TempDir,
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
    }
    impl IsolatedConfigHome {
        fn new() -> Self {
            let guard = SET_CWD_CONFIG_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = tempfile::tempdir().unwrap();
            let previous = std::env::var_os(branding::CONFIG_DIR_ENV);
            std::env::set_var(branding::CONFIG_DIR_ENV, dir.path());
            Self {
                _dir: dir,
                _guard: guard,
                previous,
            }
        }
    }
    impl Drop for IsolatedConfigHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
                None => std::env::remove_var(branding::CONFIG_DIR_ENV),
            }
        }
    }

    /// `dispatch_and_capture` with an explicit session cwd, for `set_cwd`.
    async fn dispatch_and_capture_in(
        orch: &Arc<orchestrator::ConversationOrchestrator>,
        task_registry: &Arc<tasks::registry::TaskRegistry>,
        frame: serde_json::Value,
        cwd: &std::path::Path,
    ) -> serde_json::Value {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out_tx = std::sync::Arc::new(tx);
        let writer = ControlPlaneWriter::new(out_tx.clone());
        let lifecycle =
            crate::queued_commands::QueueLifecycle::new(out_tx, "sess-test".to_string());
        let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
        let end_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let session_cwd = std::sync::Arc::new(tool_api::SessionCwd::new(
            cwd.to_path_buf(),
            vec![cwd.to_path_buf()],
        ));
        let plane = std::sync::Arc::new(crate::control_plane::StdioControlPlane::new(
            std::sync::Arc::new(tokio::sync::mpsc::unbounded_channel().0),
        ));
        dispatch_control_request(
            frame["request"]["subtype"].as_str().unwrap_or(""),
            frame["request_id"].as_str().unwrap_or("r1"),
            &frame,
            &writer,
            &cancel_tx,
            &lifecycle,
            orch,
            task_registry,
            &session_cwd,
            &plane,
            &end_notify,
            &[],
            &[],
            &[],
            &json!({}),
            "off",
            None,
            &StreamFileSuggestionIndex::default(),
        )
        .await;
        serde_json::from_str::<serde_json::Value>(&outbound_line(rx.recv().await.expect("reply")))
            .expect("valid control_response json")
    }

    #[tokio::test]
    async fn seed_read_state_stays_out_of_model_context_and_rejects_stale_host_snapshot() {
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let file = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(file.path(), "host snapshot").expect("write fixture");
        let mtime_ms = std::fs::metadata(file.path())
            .expect("metadata")
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("post epoch")
            .as_millis() as u64;

        let reply = dispatch_and_capture(
            &build.runtime.orchestrator,
            &build.runtime.task_registry,
            req(
                "seed_read_state",
                json!({
                    "path": file.path().to_string_lossy(),
                    "mtime": mtime_ms + 1,
                }),
            ),
        )
        .await;
        assert_eq!(reply["response"]["subtype"], "success");
        assert!(reply["response"].get("response").is_none());
        assert!(
            !build
                .runtime
                .orchestrator
                .files_in_context()
                .await
                .contains(&file.path().to_path_buf()),
            "accepted host snapshots are cache-only and must not enter model context"
        );

        let stale = tempfile::NamedTempFile::new().expect("stale temp file");
        std::fs::write(stale.path(), "newer content").expect("write stale fixture");
        assert!(
            !build
                .runtime
                .orchestrator
                .seed_read_state_from_host(&stale.path().to_string_lossy(), 0.0)
                .await,
            "an on-disk file newer than the host mtime must not be seeded"
        );
    }

    #[tokio::test]
    async fn dot_file_suggestions_list_cwd_entries_with_directory_suffix() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "").expect("write file");
        std::fs::create_dir(dir.path().join("src")).expect("create directory");
        let suggestions = StreamFileSuggestionIndex::default()
            .suggestions(dir.path(), "./")
            .await;
        assert!(suggestions.contains(&"Cargo.toml".to_string()));
        assert!(suggestions.contains(&format!("src{}", std::path::MAIN_SEPARATOR)));
    }

    #[tokio::test]
    async fn file_suggestions_control_response_matches_shape() {
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "").expect("write file");
        std::fs::create_dir(dir.path().join("src")).expect("create directory");

        let reply = dispatch_and_capture_in(
            &build.runtime.orchestrator,
            &build.runtime.task_registry,
            req("file_suggestions", json!({ "query": "./" })),
            dir.path(),
        )
        .await;

        assert_eq!(reply["type"], "control_response");
        assert_eq!(reply["response"]["subtype"], "success");
        assert_eq!(reply["response"]["request_id"], "r1");
        let suggestions = reply["response"]["response"]["suggestions"]
            .as_array()
            .expect("suggestions array");
        assert!(suggestions
            .iter()
            .any(|entry| entry == &json!({"path": "Cargo.toml"})));
        assert!(suggestions.iter().any(|entry| {
            entry == &json!({"path": format!("src{}", std::path::MAIN_SEPARATOR)})
        }));
    }

    #[tokio::test]
    async fn file_suggestion_cache_is_invalidated_when_session_cwd_changes() {
        use std::sync::atomic::Ordering;

        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");
        let index = StreamFileSuggestionIndex::default();

        index.prepare_root(first.path()).await;
        index.paths.write().await.push("first-only.rs".to_string());
        index.refresh_started.store(true, Ordering::Release);

        index.prepare_root(first.path()).await;
        assert_eq!(&*index.paths.read().await, &["first-only.rs".to_string()]);
        assert!(index.refresh_started.load(Ordering::Acquire));

        index.prepare_root(second.path()).await;
        assert!(index.paths.read().await.is_empty());
        assert!(!index.refresh_started.load(Ordering::Acquire));
    }

    /// 2.1.220 interrupt receipt contract (live-captured against the binary
    /// with a seeded queue): a plain interrupt lists queue-resident uuids
    /// under `still_queued`; `cancel_queued:true` sweeps them — one terminal
    /// `command_lifecycle`/`cancelled` frame per uuid BEFORE the receipt,
    /// then `{"still_queued":[],"cancelled":[…]}`; a repeat interrupt is
    /// idempotent (nothing re-listed, nothing re-cancelled).
    #[tokio::test]
    async fn interrupt_receipt_contract_matches_2_1_220() {
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let out_tx = std::sync::Arc::new(tx);
        let writer = ControlPlaneWriter::new(out_tx.clone());
        let lifecycle = crate::queued_commands::QueueLifecycle::new(out_tx, "sess-int".to_string());
        // Seed: u1 dequeued for the in-flight turn; u2/u3 queue-resident.
        lifecycle.queued.on_queued("u1");
        lifecycle.queued.on_queued("u2");
        lifecycle.queued.on_queued("u3");
        assert!(lifecycle.queued.on_dequeued("u1"));

        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let end_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let session_cwd = std::sync::Arc::new(tool_api::SessionCwd::new(
            std::env::temp_dir(),
            vec![std::env::temp_dir()],
        ));
        let plane = std::sync::Arc::new(crate::control_plane::StdioControlPlane::new(
            std::sync::Arc::new(tokio::sync::mpsc::unbounded_channel().0),
        ));
        // `u1` represents a genuinely in-flight turn, so register the same
        // owner token the production turn loop installs before dispatching an
        // interrupt. An idle control plane deliberately does not emit a sticky
        // watch cancellation, because that would poison the next queued turn.
        let active_cancel = tokio_util::sync::CancellationToken::new();
        plane.set_active_turn(active_cancel.clone()).await;

        // ① Plain interrupt: survivors listed, queue untouched.
        dispatch_control_request(
            "interrupt",
            "i1",
            &req("interrupt", json!({})),
            &writer,
            &cancel_tx,
            &lifecycle,
            orch,
            tasks,
            &session_cwd,
            &plane,
            &end_notify,
            &[],
            &[],
            &[],
            &json!({}),
            "off",
            None,
            &StreamFileSuggestionIndex::default(),
        )
        .await;
        assert!(*cancel_rx.borrow(), "interrupt must fire the cancel signal");
        assert!(
            active_cancel.is_cancelled(),
            "interrupt cancels the active owner"
        );
        let receipt: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt"))).unwrap();
        assert_eq!(receipt["response"]["subtype"], "success");
        assert_eq!(receipt["response"]["request_id"], "i1");
        assert_eq!(
            receipt["response"]["response"],
            json!({"still_queued": ["u2", "u3"]}),
            "plain interrupt: survivors under still_queued, no cancelled key"
        );

        // ② cancel_queued:true — terminal lifecycles precede the receipt.
        dispatch_control_request(
            "interrupt",
            "i2",
            &req("interrupt", json!({"cancel_queued": true})),
            &writer,
            &cancel_tx,
            &lifecycle,
            orch,
            tasks,
            &session_cwd,
            &plane,
            &end_notify,
            &[],
            &[],
            &[],
            &json!({}),
            "off",
            None,
            &StreamFileSuggestionIndex::default(),
        )
        .await;
        for expected in ["u2", "u3"] {
            let life: serde_json::Value =
                serde_json::from_str(&outbound_line(rx.recv().await.expect("lifecycle"))).unwrap();
            assert_eq!(life["type"], "command_lifecycle");
            assert_eq!(life["command_uuid"], expected);
            assert_eq!(life["state"], "cancelled");
            assert_eq!(life["session_id"], "sess-int");
        }
        let receipt2: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt2"))).unwrap();
        assert_eq!(
            receipt2["response"]["response"],
            json!({"still_queued": [], "cancelled": ["u2", "u3"]}),
        );

        // ③ Repeat interrupt: idempotent — empty receipt, no extra lifecycles.
        dispatch_control_request(
            "interrupt",
            "i3",
            &req("interrupt", json!({"cancel_queued": true})),
            &writer,
            &cancel_tx,
            &lifecycle,
            orch,
            tasks,
            &session_cwd,
            &plane,
            &end_notify,
            &[],
            &[],
            &[],
            &json!({}),
            "off",
            None,
            &StreamFileSuggestionIndex::default(),
        )
        .await;
        let receipt3: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt3"))).unwrap();
        assert_eq!(
            receipt3["response"]["response"],
            json!({"still_queued": [], "cancelled": []}),
        );

        // ④ The swept uuids must not run when the turn loop dequeues them.
        assert!(!lifecycle.queued.on_dequeued("u2"));
        assert!(!lifecycle.queued.on_dequeued("u3"));
    }

    /// 2.1.220 initialize payload tail (live-captured): the remote-control
    /// gate booleans then `fast_mode_state` + optional
    /// `fast_mode_disabled_reason` follow `pid`; the reason key is omitted
    /// when no reason resolved.
    #[test]
    fn initialize_payload_tail_matches_2_1_220() {
        let p = initialize_response_payload(
            &[],
            &[],
            &[],
            &json!({}),
            42,
            "off",
            Some("sdk_opt_in_required"),
        );
        let keys: Vec<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
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
                "remote_control_auto_enable",
                "remote_control_auto_on_by_default",
                "ide_rc_auto_enable_gate",
                "fast_mode_state",
                "fast_mode_disabled_reason",
            ],
        );
        assert_eq!(p["fast_mode_state"], "off");
        assert_eq!(p["fast_mode_disabled_reason"], "sdk_opt_in_required");

        let bare = initialize_response_payload(&[], &[], &[], &json!({}), 42, "off", None);
        assert!(
            !bare
                .as_object()
                .unwrap()
                .contains_key("fast_mode_disabled_reason"),
            "no reason ⇒ key omitted"
        );
    }

    /// `JW()` resolution order: `not_first_party` (from `!El()`'s ternary)
    /// outranks everything; the env kill-switch outranks the SDK opt-in gate;
    /// a first-party opted-in session resolves no reason. Env mutation is
    /// serialized because it is process-global.
    #[test]
    fn fast_mode_reason_resolver_matches_jw_order() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var("CLAUDE_CODE_DISABLE_FAST_MODE");
        assert_eq!(
            resolve_fast_mode_disabled_reason(false, true),
            Some("not_first_party")
        );
        assert_eq!(
            resolve_fast_mode_disabled_reason(true, false),
            Some("sdk_opt_in_required")
        );
        assert_eq!(resolve_fast_mode_disabled_reason(true, true), None);

        std::env::set_var("CLAUDE_CODE_DISABLE_FAST_MODE", "1");
        assert_eq!(
            resolve_fast_mode_disabled_reason(true, true),
            Some("disabled_by_env")
        );
        assert_eq!(
            resolve_fast_mode_disabled_reason(false, true),
            Some("not_first_party"),
            "non-first-party wins the !El() ternary even with the env set"
        );
        std::env::remove_var("CLAUDE_CODE_DISABLE_FAST_MODE");
    }

    /// `--settings` fastMode opt-in: strict boolean `true` (oracle `===!0`),
    /// accepted as inline JSON or a settings-file path.
    #[test]
    fn flag_settings_fast_mode_opt_in_parses_inline_and_file() {
        assert!(flag_settings_fast_mode_opt_in(Some(
            r#"{"fastMode": true}"#
        )));
        assert!(!flag_settings_fast_mode_opt_in(Some(
            r#"{"fastMode": "true"}"#
        )));
        assert!(!flag_settings_fast_mode_opt_in(Some(r#"{}"#)));
        assert!(!flag_settings_fast_mode_opt_in(None));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, r#"{"fastMode": true}"#).unwrap();
        assert!(flag_settings_fast_mode_opt_in(file.to_str()));
        assert!(!flag_settings_fast_mode_opt_in(Some(
            "/nonexistent/lingxi-settings.json"
        )));
    }

    /// The turn loop's terminal `command_lifecycle` state, driven through the
    /// same helper the loop calls (`Njo(In,Nn)` @239414857 behind the
    /// stream-json call site @240906553). Before this, EVERY finished turn —
    /// including one that died on a hard failure — reported `completed`.
    #[test]
    fn turn_terminal_lifecycle_matches_njo_call_site() {
        use crate::queued_commands::QueueLifecycle;
        use orchestrator::{OrchestratorError, TurnOutcome};

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let lifecycle = QueueLifecycle::new(std::sync::Arc::new(tx), "sess-term".to_string());
        let next =
            |rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::stream_json::OutboundMsg>| {
                serde_json::from_str::<serde_json::Value>(&outbound_line(
                    rx.try_recv().expect("lifecycle frame"),
                ))
                .expect("valid command_lifecycle json")
            };

        // Clean turn → `completed` (reason "completed", not aborted).
        emit_turn_terminal_lifecycle(&lifecycle, Some("u-ok"), &Ok(TurnOutcome::EndTurn), false);
        let f = next(&mut rx);
        assert_eq!(f["type"], "command_lifecycle");
        assert_eq!(f["command_uuid"], "u-ok");
        assert_eq!(f["state"], "completed");

        // `max_turns` is one of `Bxs`'s `return!1` arms — still `completed`.
        emit_turn_terminal_lifecycle(&lifecycle, Some("u-max"), &Ok(TurnOutcome::MaxTurns), false);
        assert_eq!(next(&mut rx)["state"], "completed");

        // Interrupted turn: `aborted_streaming` (Wpt) AND the abort flag.
        emit_turn_terminal_lifecycle(&lifecycle, Some("u-int"), &Ok(TurnOutcome::Cancelled), true);
        assert_eq!(next(&mut rx)["state"], "cancelled");

        // Abort flag alone (`Njo`'s `t||…`) forces `cancelled` even on a turn
        // that otherwise ended naturally.
        emit_turn_terminal_lifecycle(&lifecycle, Some("u-ab"), &Ok(TurnOutcome::EndTurn), true);
        assert_eq!(next(&mut rx)["state"], "cancelled");

        // Hard failure — the `rn!==null?"cancelled"` arm. STREAM-1: this
        // reported `completed` while the same run's result frame said
        // `is_error:true`.
        for err in [
            OrchestratorError::MaxBudgetReached {
                budget_nano_usd: 100,
            },
            OrchestratorError::Internal("boom".to_string()),
        ] {
            emit_turn_terminal_lifecycle(&lifecycle, Some("u-err"), &Err(err), false);
            let f = next(&mut rx);
            assert_eq!(f["command_uuid"], "u-err");
            assert_eq!(f["state"], "cancelled");
        }

        // An unstamped frame is not lifecycle-tracked — no frame at all.
        emit_turn_terminal_lifecycle(&lifecycle, None, &Ok(TurnOutcome::EndTurn), false);
        assert!(rx.try_recv().is_err(), "no uuid ⇒ no lifecycle frame");
    }

    /// Every lifecycle-tracked uuid reaches EXACTLY ONE terminal. The resume
    /// dedup path retires the uuid from the shadow registry (`on_dequeued`)
    /// before skipping the turn, so teardown's `discarded` sweep can no longer
    /// cover it — the skip must emit its own terminal (binary @246492139).
    #[test]
    fn resume_dedup_skip_emits_its_own_terminal() {
        use crate::queued_commands::QueueLifecycle;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let lifecycle = QueueLifecycle::new(std::sync::Arc::new(tx), "sess-dedup".to_string());

        // Router: uuid enters the queue.
        lifecycle.command_queued("u-dup");
        let queued: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.try_recv().expect("queued"))).unwrap();
        assert_eq!(queued["state"], "queued");

        // Turn loop: dequeue (not cancel-pending), then the dedup skip.
        assert!(lifecycle.queued.on_dequeued("u-dup"));
        emit_dedup_skip_terminal(&lifecycle, "u-dup");
        let terminal: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.try_recv().expect("terminal"))).unwrap();
        assert_eq!(terminal["command_uuid"], "u-dup");
        assert_eq!(terminal["state"], "completed");

        // Teardown cannot make up for a missing terminal: the uuid is gone.
        // A uuid that never reached the turn loop IS still reachable, and gets
        // `discarded` (binary `Hkm`) — the contrast that makes the skip's own
        // terminal load-bearing.
        lifecycle.command_queued("u-resident");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&outbound_line(
                rx.try_recv().expect("queued")
            ))
            .unwrap()["state"],
            "queued"
        );
        let survivors = lifecycle.queued.drain_for_discard();
        assert_eq!(
            survivors,
            vec!["u-resident"],
            "a dequeued uuid is unreachable from the teardown discard sweep"
        );
        for uuid in &survivors {
            lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_DISCARDED);
        }
        let discarded: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.try_recv().expect("discarded"))).unwrap();
        assert_eq!(discarded["command_uuid"], "u-resident");
        assert_eq!(discarded["state"], "discarded");
        assert!(rx.try_recv().is_err(), "exactly one terminal per command");
    }

    /// `cK(mt,ce.fastMode)` (@227895153): the state rides the SAME inputs as
    /// `JW()`, so the `-p` surface never emits `off` with no reason.
    #[test]
    fn fast_mode_state_tracks_the_disabled_reason() {
        // Opted in, no reason, fast-mode-capable model → `on`.
        assert_eq!(resolve_fast_mode_state("claude-opus-5", None, true), "on");
        assert_eq!(
            resolve_fast_mode_state("claude-opus-5[1m]", None, true),
            "on"
        );
        assert_eq!(resolve_fast_mode_state("claude-opus-4-7", None, true), "on");
        assert_eq!(resolve_fast_mode_state("claude-opus-4-8", None, true), "on");
        // `fE(model)` is part of the conjunction: a model with no fast-mode
        // capability stays `off` even fully opted in.
        assert_eq!(
            resolve_fast_mode_state("claude-sonnet-5", None, true),
            "off"
        );
        // Any reason ⇒ `El()&&QN()` is false ⇒ `off`.
        assert_eq!(
            resolve_fast_mode_state("claude-opus-5", Some("not_first_party"), true),
            "off"
        );
        // No opt-in ⇒ `!!t` false (and `JW` would report sdk_opt_in_required).
        assert_eq!(
            resolve_fast_mode_state("claude-opus-5", Some("sdk_opt_in_required"), false),
            "off"
        );
    }

    /// `xn()` (@227682549) is purely env-derived — a first-party Claude model
    /// under any managed-cloud env var is `not_first_party`. The env read is
    /// NOT exercised here: `CLAUDE_CODE_USE_*` is process-global and ~840
    /// sibling tests resolve models off it, so the verdict is a parameter and
    /// the var LIST is asserted against the binary instead.
    #[test]
    fn managed_cloud_env_forces_not_first_party() {
        assert_eq!(
            MANAGED_CLOUD_PROVIDER_ENV,
            [
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_FOUNDRY",
                "CLAUDE_CODE_USE_ANTHROPIC_AWS",
                "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
                "CLAUDE_CODE_USE_MANTLE",
                "CLAUDE_CODE_USE_VERTEX",
            ],
            "xn()'s provider chain, in binary order"
        );

        let listings = vec![platform_api::orchestrator::ModelListing {
            display_model: "Opus".to_string(),
            request_model: "claude-opus-4-8".to_string(),
            provider_id: "anthropic".to_string(),
            provider_label: "Anthropic".to_string(),
            description: None,
            metadata: Default::default(),
            capabilities: Default::default(),
            reasoning: Default::default(),
            supports_reasoning: true,
        }];
        // A first-party Claude model on a managed-cloud provider: the catalog
        // says `anthropic`, `xn()` says otherwise, and `xn()` wins.
        assert!(!session_model_is_first_party(
            false,
            &listings,
            "claude-opus-4-8"
        ));
        assert_eq!(
            resolve_fast_mode_disabled_reason(
                session_model_is_first_party(false, &listings, "claude-opus-4-8"),
                true,
            ),
            Some("not_first_party"),
        );
        assert!(session_model_is_first_party(
            true,
            &listings,
            "claude-opus-4-8"
        ));
    }

    /// First-party detection prefers the live catalog row's provider; unknown
    /// ids fall back to the `claude-*` / `default` family rule.
    #[test]
    fn session_model_first_party_uses_catalog_provider() {
        let listings = vec![
            platform_api::orchestrator::ModelListing {
                display_model: "Opus".to_string(),
                request_model: "claude-opus-4-8".to_string(),
                provider_id: "anthropic".to_string(),
                provider_label: "Anthropic".to_string(),
                description: None,
                metadata: Default::default(),
                capabilities: Default::default(),
                reasoning: Default::default(),
                supports_reasoning: true,
            },
            platform_api::orchestrator::ModelListing {
                display_model: "GPT-4o".to_string(),
                request_model: "gpt-4o".to_string(),
                provider_id: "openai".to_string(),
                provider_label: "OpenAI".to_string(),
                description: None,
                metadata: Default::default(),
                capabilities: Default::default(),
                reasoning: Default::default(),
                supports_reasoning: false,
            },
        ];
        assert!(session_model_is_first_party(
            true,
            &listings,
            "claude-opus-4-8"
        ));
        assert!(!session_model_is_first_party(true, &listings, "gpt-4o"));
        // Fallback family rule when the model is not in the catalog.
        assert!(session_model_is_first_party(
            true,
            &listings,
            "claude-opus-5[1m]"
        ));
        assert!(session_model_is_first_party(true, &listings, "default"));
        assert!(!session_model_is_first_party(true, &listings, "grok-3"));
    }

    #[tokio::test]
    async fn dispatch_live_thinking_and_rename_controls_validate_and_ack() {
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;

        let bad = dispatch_and_capture(
            orch,
            tasks,
            req(
                "set_max_thinking_tokens",
                json!({"max_thinking_tokens": "lots", "thinking_display": "raw"}),
            ),
        )
        .await;
        assert_eq!(bad["response"]["subtype"], "error");
        assert!(bad["response"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("max_thinking_tokens must be an integer or null"));

        let thinking = dispatch_and_capture(
            orch,
            tasks,
            req(
                "set_max_thinking_tokens",
                json!({"max_thinking_tokens": 2048, "thinking_display": "summarized"}),
            ),
        )
        .await;
        assert_eq!(thinking["response"]["subtype"], "success");

        let empty =
            dispatch_and_capture(orch, tasks, req("rename_session", json!({"title": "   "}))).await;
        assert_eq!(empty["response"]["subtype"], "error");

        let renamed = dispatch_and_capture(
            orch,
            tasks,
            req("rename_session", json!({"title": "Control Rename"})),
        )
        .await;
        assert_eq!(renamed["response"]["subtype"], "success");
    }

    #[tokio::test]
    async fn dispatch_unknown_mcp_permission_override_matches_claude_warning() {
        const UNKNOWN_SERVER: &str = "__lingxi_unknown_test_server_4ad4__";
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let frame = req(
            "set_mcp_permission_mode_override",
            json!({ "serverName": UNKNOWN_SERVER, "mode": "default" }),
        );
        let resp = dispatch_and_capture(
            &build.runtime.orchestrator,
            &build.runtime.task_registry,
            frame,
        )
        .await;
        assert_eq!(
            resp,
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "r1",
                    "response": {
                        "warning": "MCP server '__lingxi_unknown_test_server_4ad4__' is not yet known; override stored but will not apply until a server with that exact name connects."
                    }
                }
            })
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_mcp_permission_override_clear_matches_claude_warning() {
        const UNKNOWN_SERVER: &str = "__lingxi_unknown_test_server_4ad4__";
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let frame = req(
            "set_mcp_permission_mode_override",
            json!({ "serverName": UNKNOWN_SERVER, "mode": serde_json::Value::Null }),
        );
        let resp = dispatch_and_capture(
            &build.runtime.orchestrator,
            &build.runtime.task_registry,
            frame,
        )
        .await;
        assert_eq!(
            resp,
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": "r1",
                    "response": {
                        "warning": "MCP server '__lingxi_unknown_test_server_4ad4__' is not known; no override was present to clear."
                    }
                }
            })
        );
    }

    #[tokio::test]
    async fn dispatch_mcp_permission_override_rejects_non_tightening_known_modes() {
        let argv = tui_argv();
        let build = crate::init::build_runtime_for_tui(&argv)
            .await
            .expect("build_runtime_for_tui");
        let frame = req(
            "set_mcp_permission_mode_override",
            json!({ "serverName": "context7", "mode": "bypassPermissions" }),
        );
        let resp = dispatch_and_capture(
            &build.runtime.orchestrator,
            &build.runtime.task_registry,
            frame,
        )
        .await;
        assert_eq!(
            resp,
            json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": "r1",
                    "error": "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected 'bypassPermissions'"
                }
            })
        );
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

    /// `filterByPr` port over REAL loader rows, including a persisted `pr-link`.
    #[tokio::test]
    async fn filter_rows_by_pr_semantics() {
        let temp = tempfile::TempDir::new().unwrap();
        let lingxi_home = temp.path().join("home");
        let cwd_str = "/tmp/workproj".to_string();
        let project_dir = make_project_dir(&lingxi_home, &cwd_str);
        let linked_id = write_session(&project_dir, "some prompt", SystemTime::now());
        let linked_path = project_dir.join(format!("{linked_id}.jsonl"));
        let pr_link = serde_json::json!({
            "type": "pr-link",
            "sessionId": linked_id.to_string(),
            "prNumber": 123,
            "prUrl": "https://github.com/foo/bar/pull/123",
            "prRepository": "foo/bar"
        });
        use std::io::Write as _;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(linked_path)
                .unwrap(),
            "{}",
            serde_json::to_string(&pr_link).unwrap()
        )
        .unwrap();
        let rows = load_resume_rows_from(&lingxi_home, std::path::Path::new(&cwd_str))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);

        // No --from-pr → unchanged.
        assert_eq!(filter_rows_by_pr(rows.clone(), None).len(), 1);
        // Bare --from-pr → PR-linked only.
        assert_eq!(filter_rows_by_pr(rows.clone(), Some("")).len(), 1);
        // Parseable PR number/URL → prNumber === n.
        assert_eq!(filter_rows_by_pr(rows.clone(), Some("123")).len(), 1);
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

    /// Byte-exact against the oracle's no-match copy (2.1.220 @246508120). The
    /// base sentence ends WITHOUT a period; the appended clause supplies it.
    #[test]
    fn resume_title_not_found_copy_is_byte_exact() {
        assert_eq!(
            resume_title_not_found("my session"),
            "Error: --resume requires a valid session ID or session title when used with \
             --print. Usage: lingxi -p --resume <session-id|title>. Provided value \"my session\" \
             is not a UUID and does not match any session title."
        );
    }

    /// Byte-exact against the oracle's multi-match copy. Two spaces lead each
    /// row; two more separate the id from `(modified …)`; rows are newline
    /// joined under the header.
    #[test]
    fn resume_title_ambiguous_copy_is_byte_exact() {
        let row = |id: u128, secs: u64| SessionMetadata {
            uuid: uuid::Uuid::from_u128(id),
            title: "shared".to_string(),
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            created: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            message_count: 1,
            mode: session::jsonl::SessionMode::Code,
            path: PathBuf::from("x.jsonl"),
            pr_number: None,
            custom_or_ai_title: Some("shared".to_string()),
            resume_model: None,
            resume_model_profile: None,
        };
        let matches = vec![row(2, 1_700_000_001), row(1, 1_700_000_000)];
        assert_eq!(
            resume_title_ambiguous("shared", &matches),
            "Error: --resume \"shared\" matches 2 sessions. Pass one of these session IDs to \
             disambiguate:\n  \
             00000000-0000-0000-0000-000000000002  (modified 2023-11-14T22:13:21.000Z)\n  \
             00000000-0000-0000-0000-000000000001  (modified 2023-11-14T22:13:20.000Z)"
        );
    }

    /// An empty `--resume` has no title to look up, so the uuid-parse error
    /// stands rather than the title copy misdescribing it.
    #[tokio::test]
    async fn resume_title_lookup_declines_an_empty_argument() {
        assert_eq!(resolve_resume_title("").await, Ok(None));
    }

    fn titled_row(id: u128, secs: u64, searchable: &str) -> SessionMetadata {
        SessionMetadata {
            uuid: uuid::Uuid::from_u128(id),
            title: searchable.to_string(),
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            created: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            message_count: 1,
            mode: session::jsonl::SessionMode::Code,
            path: PathBuf::from(format!("{id}.jsonl")),
            pr_number: None,
            custom_or_ai_title: Some(searchable.to_string()),
            resume_model: None,
            resume_model_profile: None,
        }
    }

    /// The success path: exactly one title match resolves to that session's id.
    #[test]
    fn resume_title_resolves_a_unique_match_to_its_session_id() {
        let rows = vec![
            titled_row(1, 100, "ship the parser"),
            titled_row(2, 200, "unrelated"),
        ];
        assert_eq!(
            resolve_resume_title_from(rows, "ship the parser"),
            Ok(Some(uuid::Uuid::from_u128(1)))
        );
    }

    /// `--resume` searches EXACTLY, so a partial title must not silently resume
    /// the session it happens to be a prefix of.
    #[test]
    fn resume_title_refuses_a_partial_match() {
        let rows = vec![titled_row(1, 100, "ship the parser")];
        assert!(
            resolve_resume_title_from(rows, "ship")
                .expect_err("partial title must not resolve")
                .contains("does not match any session title"),
            "a substring must fall through to the no-match copy"
        );
    }

    /// Two sessions sharing a title cannot be disambiguated by the port, so the
    /// user is handed the ids rather than an arbitrary pick.
    #[test]
    fn resume_title_reports_every_candidate_when_ambiguous() {
        let rows = vec![titled_row(1, 100, "dup"), titled_row(2, 200, "dup")];
        let error = resolve_resume_title_from(rows, "dup").expect_err("ambiguous must error");
        assert!(error.contains("matches 2 sessions"), "{error}");
        assert!(
            error.contains("00000000-0000-0000-0000-000000000001")
                && error.contains("00000000-0000-0000-0000-000000000002"),
            "both candidate ids must be listed: {error}"
        );
    }

    #[test]
    fn resolve_session_id_rejects_garbage() {
        assert!(matches!(
            resolve_session_id("not-a-uuid"),
            Err(LoaderError::SessionNotFound { .. })
        ));
    }

    /// `set_cwd` moves the live session, and an already-TRUSTED target does it
    /// without a handshake. The response carries the new cwd and `changed`.
    #[tokio::test]
    async fn set_cwd_moves_the_session_to_a_trusted_directory() {
        let _config = IsolatedConfigHome::new();
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("target");
        std::fs::create_dir_all(&target).unwrap();

        // Trusting is confirmed via the echo handshake, so drive the full
        // two-step exchange rather than pre-seeding global trust state.
        let first = dispatch_and_capture_in(
            orch,
            tasks,
            req("set_cwd", json!({ "path": target.to_string_lossy() })),
            home.path(),
        )
        .await;
        assert_eq!(first["response"]["subtype"], "success");
        let body = &first["response"]["response"];
        assert_eq!(
            body["status"], "needs_trust",
            "an untrusted target asks first"
        );
        let shown = body["directory"].as_str().expect("directory").to_string();

        let second = dispatch_and_capture_in(
            orch,
            tasks,
            req(
                "set_cwd",
                json!({
                    "path": target.to_string_lossy(),
                    "trust_accepted": true,
                    "trusted_directory": shown,
                }),
            ),
            home.path(),
        )
        .await;
        let body = &second["response"]["response"];
        assert_eq!(body["status"], "ok");
        assert_eq!(body["changed"], true);
        assert_eq!(body["cwd"], shown);
    }

    /// A confirmation that echoes a DIFFERENT directory re-prompts instead of
    /// moving — the echo pins the approval to the path the user was shown.
    #[tokio::test]
    async fn set_cwd_re_prompts_when_the_trust_echo_does_not_match() {
        let _config = IsolatedConfigHome::new();
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("target");
        std::fs::create_dir_all(&target).unwrap();

        let resp = dispatch_and_capture_in(
            orch,
            tasks,
            req(
                "set_cwd",
                json!({
                    "path": target.to_string_lossy(),
                    "trust_accepted": true,
                    "trusted_directory": "/somewhere/else",
                }),
            ),
            home.path(),
        )
        .await;
        assert_eq!(resp["response"]["response"]["status"], "needs_trust");
    }

    /// The rejection shapes reach the wire with the binary's `reason` strings,
    /// and a malformed request is an ERROR frame rather than a `status` body.
    #[tokio::test]
    async fn set_cwd_rejection_shapes_reach_the_wire() {
        let _config = IsolatedConfigHome::new();
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;
        let home = tempfile::tempdir().unwrap();

        let blank = dispatch_and_capture_in(
            orch,
            tasks,
            req("set_cwd", json!({ "path": "  " })),
            home.path(),
        )
        .await;
        assert_eq!(blank["response"]["subtype"], "error");
        assert!(blank["response"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("path must be a non-empty string"));

        let missing = dispatch_and_capture_in(
            orch,
            tasks,
            req(
                "set_cwd",
                json!({ "path": home.path().join("nope").to_string_lossy() }),
            ),
            home.path(),
        )
        .await;
        assert_eq!(missing["response"]["response"]["reason"], "not_found");

        let file = home.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let not_dir = dispatch_and_capture_in(
            orch,
            tasks,
            req("set_cwd", json!({ "path": file.to_string_lossy() })),
            home.path(),
        )
        .await;
        assert_eq!(not_dir["response"]["response"]["reason"], "not_a_directory");
    }

    /// Re-entering the CURRENT directory is a no-op `ok`, never a prompt.
    #[tokio::test]
    async fn set_cwd_to_the_current_directory_reports_unchanged() {
        let _config = IsolatedConfigHome::new();
        let build = crate::init::build_runtime_for_tui(&tui_argv())
            .await
            .expect("build_runtime_for_tui");
        let orch = &build.runtime.orchestrator;
        let tasks = &build.runtime.task_registry;
        let home = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(home.path()).unwrap();

        let resp = dispatch_and_capture_in(
            orch,
            tasks,
            req("set_cwd", json!({ "path": canonical.to_string_lossy() })),
            &canonical,
        )
        .await;
        let body = &resp["response"]["response"];
        assert_eq!(body["status"], "ok");
        assert_eq!(body["changed"], false);
    }
}
