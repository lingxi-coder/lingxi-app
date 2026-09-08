//! Three-way mode dispatch added in M6-01. Routes argv to one of:
//! - `Mode::Print(prompt)`: existing `run::run_oneshot` (v0.6.0, unchanged)
//! - `Mode::Tui`: the ratatui TUI via `run_ratatui` (default)
//! - `Mode::StdioRepl`: existing `repl::run_repl` (v0.6.0, kept as `--no-tui`
//!   and non-TTY fallback)
//!
//! TTY detection uses [`std::io::IsTerminal`] (stable since Rust 1.70). If
//! stdin OR stdout isn't a terminal, we fall back to `StdioRepl` (CI, pipes,
//! redirected I/O). The `--no-tui` flag forces `StdioRepl` regardless of
//! TTY state.

use crate::argv::Argv;
use std::io::IsTerminal;

/// The resolved execution mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// One-shot print mode: prompt is set, exit after first `end_turn`.
    /// Carries the prompt string so the caller doesn't re-clone argv.
    Print(String),
    /// Fullscreen iocraft TUI (default when no prompt + TTY + no `--no-tui`).
    Tui,
    /// Line-based stdio REPL from v0.6.0 (`--no-tui` or non-TTY).
    StdioRepl,
}

/// Pick a mode from argv + the current process's TTY state.
///
/// Decision tree:
/// 1. If `argv.prompt` has a non-empty value → `Print(prompt)`.
/// 2. Else if `argv.no_tui` is set OR stdin/stdout isn't a TTY → `StdioRepl`.
/// 3. Else → `Tui`.
#[must_use]
pub fn decide_mode(argv: &Argv) -> Mode {
    decide_mode_with(argv, is_full_tty())
}

/// Test seam: `is_tty` argument decouples the decision from the real
/// process TTY state.
#[must_use]
pub fn decide_mode_with(argv: &Argv, is_tty: bool) -> Mode {
    if let Some(p) = argv.prompt.as_deref() {
        if !p.trim().is_empty() {
            return Mode::Print(p.to_string());
        }
    }
    if argv.no_tui || !is_tty {
        return Mode::StdioRepl;
    }
    Mode::Tui
}

/// True iff BOTH stdin and stdout are terminals. (M7-12) Exposed
/// `pub(crate)` so `run::run_resume` can make the same TTY decision the mode
/// dispatcher uses — one source of truth for "are we interactive".
pub(crate) fn is_full_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use platform_api::OrchestratorHandle;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct TuiMsgQueueInput {
    queue: Arc<msgqueue::MessageQueueManager>,
}

#[async_trait::async_trait]
impl orchestrator::prompt::mid_turn_input::MidTurnInputSource for TuiMsgQueueInput {
    async fn take_mid_turn_input(&self) -> Option<String> {
        self.queue.take_mid_turn_prompt().await
    }
}

fn tui_prompt_command(text: String) -> msgqueue::QueuedCommand {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    msgqueue::QueuedCommand {
        uuid: format!(
            "tui-prompt-{}",
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        content: msgqueue::QueuedCommandContent::UserInput { text },
        priority: msgqueue::QueuePriority::Next,
        queued_at: std::time::SystemTime::now(),
        source: msgqueue::QueueSource::PromptInput,
        agent_id: None,
        skip_slash_commands: false,
        is_meta: false,
    }
}

async fn drain_teammate_prompts(
    queue: &msgqueue::MessageQueueManager,
    orchestrator: &dyn OrchestratorHandle,
    turn_tx: &tokio::sync::mpsc::UnboundedSender<tui::TurnEvent>,
) {
    while let Some(command) = queue.dequeue_main_thread().await {
        let Some(text) = command.text() else {
            continue;
        };
        let cancel = CancellationToken::new();
        let _ = turn_tx.send(tui::TurnEvent::TurnStartedWithCancel(cancel.clone()));
        queue.register_active_turn(cancel.clone()).await;
        if let Err(error) = orchestrator
            .run_turn_streaming_with_cancel(text, cancel)
            .await
        {
            let _ = turn_tx.send(tui::TurnEvent::TextDelta(error.to_string()));
            let _ = turn_tx.send(tui::TurnEvent::TurnEnded(
                platform_api::TurnOutcome::EndTurn,
            ));
        }
        queue.clear_active_turn().await;
    }
}

fn trim_decimal(mut rendered: String) -> String {
    while rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    rendered
}

fn format_token_count(value: Option<u64>) -> Option<String> {
    let value = value?;
    if value >= 1_000_000 {
        Some(format!(
            "{}M",
            trim_decimal(format!("{:.2}", value as f64 / 1_000_000.0))
        ))
    } else if value >= 1_000 {
        Some(format!(
            "{}K",
            trim_decimal(format!("{:.0}", value as f64 / 1_000.0))
        ))
    } else {
        Some(value.to_string())
    }
}

fn format_price(value: Option<f64>) -> Option<String> {
    value.map(|price| {
        let rendered = if price >= 1.0 {
            format!("{price:.2}")
        } else {
            format!("{price:.3}")
        };
        format!("${}", trim_decimal(rendered))
    })
}

fn reasoning_summary(spec: &platform_api::ReasoningControlSpec) -> Option<String> {
    let labels = spec
        .available
        .iter()
        .filter_map(|selection| match selection {
            platform_api::ReasoningSelection::Automatic => Some("auto".to_string()),
            platform_api::ReasoningSelection::Disabled => Some("off".to_string()),
            platform_api::ReasoningSelection::Enabled => Some("on".to_string()),
            platform_api::ReasoningSelection::Level { id } => Some(id.clone()),
            platform_api::ReasoningSelection::TokenBudget { tokens } => Some(format!("{tokens}t")),
        })
        .collect::<Vec<_>>();
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn pricing_summary(pricing: Option<&platform_api::ModelPricing>) -> String {
    let Some(pricing) = pricing else {
        return "价格 未提供".to_string();
    };
    match pricing.billing_mode {
        platform_api::ModelBillingMode::Subscription => "价格 套餐/订阅内".to_string(),
        platform_api::ModelBillingMode::Free => "价格 免费".to_string(),
        platform_api::ModelBillingMode::Unknown => "价格 未提供".to_string(),
        platform_api::ModelBillingMode::PerToken => match (
            format_price(pricing.input_per_million),
            format_price(pricing.output_per_million),
        ) {
            (Some(input), Some(output)) => format!("价格 {input}/{output}"),
            _ => "价格 未提供".to_string(),
        },
    }
}

fn build_model_details(m: &platform_api::ModelListing) -> Vec<String> {
    let mut summary_bits = Vec::new();
    let mut details = Vec::new();

    let mut capabilities = Vec::new();
    if m.capabilities.tools {
        capabilities.push("tools");
    }
    if m.capabilities.vision {
        capabilities.push("image");
    }
    if m.capabilities.documents {
        capabilities.push("pdf");
    }
    if m.capabilities.structured_output {
        capabilities.push("json");
    }
    if !capabilities.is_empty() {
        summary_bits.push(capabilities.join("/"));
        details.push(format!("能力: {}", capabilities.join(", ")));
    }

    let mut limits = Vec::new();
    if let Some(context) = format_token_count(m.metadata.context_window_tokens) {
        limits.push(format!("ctx {context}"));
    }
    if let Some(max_input) = format_token_count(m.metadata.max_input_tokens) {
        limits.push(format!("in {max_input}"));
    }
    if let Some(max_output) = format_token_count(m.metadata.max_output_tokens) {
        limits.push(format!("out {max_output}"));
    }
    if !limits.is_empty() {
        let rendered = limits.join(" · ");
        summary_bits.push(rendered.clone());
        details.push(format!("Limits: {rendered}"));
    }

    let pricing = m.metadata.pricing.as_ref();
    summary_bits.push(pricing_summary(pricing));
    match pricing {
        Some(pricing) => {
            let mut price_bits = Vec::new();
            if let Some(input) = format_price(pricing.input_per_million) {
                price_bits.push(format!("输入 {input}"));
            }
            if let Some(output) = format_price(pricing.output_per_million) {
                price_bits.push(format!("输出 {output}"));
            }
            if let Some(cache_read) = format_price(pricing.cache_read_per_million) {
                price_bits.push(format!("cache读 {cache_read}"));
            }
            if let Some(cache_write) = format_price(pricing.cache_write_per_million) {
                price_bits.push(format!("cache写 {cache_write}"));
            }
            if let Some(reasoning) = format_price(pricing.reasoning_per_million) {
                price_bits.push(format!("thinking {reasoning}"));
            }
            if !price_bits.is_empty() {
                details.push(format!("价格: {} / 1M tok", price_bits.join(" · ")));
            }
            for tier in &pricing.tiers {
                let threshold = format_token_count(Some(tier.context_threshold_tokens))
                    .unwrap_or_else(|| tier.context_threshold_tokens.to_string());
                details.push(format!(
                    "长上下文层级: >{threshold} 输入 {} 输出 {}",
                    format_price(tier.input_per_million).unwrap_or_else(|| "未提供".to_string()),
                    format_price(tier.output_per_million).unwrap_or_else(|| "未提供".to_string())
                ));
            }
        }
        None => details.push("价格: 未提供".to_string()),
    }

    if let Some(reasoning) = reasoning_summary(&m.reasoning) {
        details.push(format!("Reasoning: {reasoning}"));
    }
    if let Some(status) = m.metadata.status.as_deref() {
        details.push(format!("状态: {status}"));
    }
    if let Some(family) = m.metadata.family.as_deref() {
        details.push(format!("家族: {family}"));
    }
    if let Some(cutoff) = m.metadata.knowledge_cutoff.as_deref() {
        details.push(format!("知识截止: {cutoff}"));
    }
    if !m.metadata.input_modalities.is_empty() || !m.metadata.output_modalities.is_empty() {
        details.push(format!(
            "模态: in [{}] → out [{}]",
            m.metadata.input_modalities.join(", "),
            m.metadata.output_modalities.join(", ")
        ));
    }
    details.push(format!("模型 ID: {}", m.request_model));
    if let Some(description) = m.description.as_deref() {
        details.push(description.to_string());
    }

    let mut out = Vec::new();
    if !summary_bits.is_empty() {
        out.push(summary_bits.join(" · "));
    }
    out.extend(details);
    out
}

/// Execute the chosen mode. Returns the process exit code.
///
/// `Mode::Print` uses the caller-provided runtime. Interactive modes own their
/// runtime construction so callers can route to them without pre-building and
/// discarding a generic runtime.
pub async fn dispatch(
    mode: Mode,
    argv: &Argv,
    runtime: &Runtime,
    sink: Arc<dyn OutputSink>,
) -> i32 {
    match mode {
        Mode::Print(_) => {
            // run_oneshot reads the prompt directly from argv.prompt;
            // the captured-prompt copy in Mode::Print(prompt) exists for
            // test introspection only.
            crate::run::run_oneshot(argv, runtime, sink.as_ref()).await
        }
        Mode::StdioRepl => {
            let _ = runtime;
            let _ = sink;
            run_stdio_repl(argv).await
        }
        Mode::Tui => run_tui(argv).await,
    }
}

pub(crate) async fn dispatch_interactive(mode: Mode, argv: &Argv) -> i32 {
    match mode {
        Mode::StdioRepl => run_stdio_repl(argv).await,
        Mode::Tui => run_tui(argv).await,
        Mode::Print(_) => {
            eprintln!("lingxi-cli: internal error: print mode requires a runtime");
            exit_codes::RUNTIME_ERROR
        }
    }
}

async fn run_stdio_repl(argv: &Argv) -> i32 {
    crate::startup_trace::mark("stdio_repl_start");
    crate::repl::run_repl(argv).await
}

async fn run_tui(argv: &Argv) -> i32 {
    crate::startup_trace::mark("tui_start");
    if !startup_preflight(argv).await {
        return exit_codes::RUNTIME_ERROR;
    }
    let tui_build = match crate::init::build_runtime_for_tui(argv).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lingxi-cli: tui init failed: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };
    // (M7 cc2.1.198) Register this interactive session in the cross-process
    // live-session registry (`~/.lingxi/sessions/<pid>.json`) so
    // `lingxi-cli agents --json` and the agents view can list it — the binary
    // registers every process the same way (observed `sessions/<pid>.json`
    // shape). Best-effort: a write failure never blocks the session; the record
    // is unlinked when the TUI returns.
    //
    // (M8 cc2.1.198) Live status refreshes: the registration is shared (`Arc`)
    // with the ratatui mount, whose channel forwarders rewrite
    // `status`/`updatedAt`/`statusUpdatedAt` on idle ↔ busy ↔ waiting transitions
    // (binary `mvn`, fed by the REPL status effect @222989611).
    // Capture the freshly-launched session id up front: it seeds the switch
    // loop's failed-switch fallback (finding #2) so a `/resume` to an unloadable
    // target re-mounts THIS session instead of exiting.
    let initial_session_id = tui_build.runtime.orchestrator.current_session_id().await;
    let session_registration = {
        let derived = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let user_name = argv
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let display = user_name.or(derived.as_deref());
        let home = crate::run::lingxi_home_dir();
        let reg = Arc::new(crate::agents_registry::SessionRegistration::register(
            &home,
            Some(initial_session_id.to_string()).as_deref(),
            display,
        ));
        let dir = platform_api::live_sessions::LiveSessionDir::at_live(
            crate::agents_registry::sessions_dir(&home),
        );
        let claim = platform_api::live_sessions::install_process(
            dir,
            &initial_session_id.to_string(),
            user_name,
        );
        if let Some(claim) = claim {
            let source = if claim.notice.is_some() {
                "collision"
            } else {
                "user"
            };
            reg.set_name(&claim.name, source);
        } else if let Some(derived) = display {
            platform_api::live_sessions::set_process_name(derived);
        }
        ensure_live_messaging(&initial_session_id.to_string(), user_name, Some(&reg));
        reg
    };
    // FRESH launch: ratatui (`tui-rata`) is the only TUI backend. It drives the
    // live orchestrator directly via `tui_build`'s bridge channel; no replayed
    // scrollback (empty seed).
    let outcome = run_ratatui(tui_build, Some(session_registration.clone()), Vec::new()).await;
    // Inbox + live record stay up across in-process `/resume` remounts (same
    // pid). Unlink only after the switch loop exits so other sessions can
    // still find this process by name.
    let code = crate::run::drive_tui_switch_loop(
        argv,
        outcome,
        Some(initial_session_id.as_uuid()),
        Some(session_registration.clone()),
    )
    .await;
    session_registration.deregister();
    code
}

/// Process-wide `--messaging-socket-path <path>` override (cc 2.1.238
/// @307414302 — "Cross-session messaging server path: a Unix domain socket on
/// Mac/Linux, a \\.\pipe\ name on Windows (defaults to an auto-generated
/// path)"). Set once from argv in `run_cli`, before any inbox bind.
static MESSAGING_SOCKET_OVERRIDE: std::sync::OnceLock<std::path::PathBuf> =
    std::sync::OnceLock::new();

/// Record the `--messaging-socket-path` value. Idempotent: the first call wins,
/// matching the single top-level option the oracle carries.
pub(crate) fn set_messaging_socket_override(path: &str) {
    let _ = MESSAGING_SOCKET_OVERRIDE.set(std::path::PathBuf::from(path));
}

/// The `--messaging-socket-path` override, if one was given.
#[must_use]
pub(crate) fn messaging_socket_override() -> Option<std::path::PathBuf> {
    MESSAGING_SOCKET_OVERRIDE.get().cloned()
}

/// Bind the process UDS inbox and advertise it on the live session record.
pub(crate) fn ensure_live_messaging(
    session_id: &str,
    user_name: Option<&str>,
    registration: Option<&Arc<crate::agents_registry::SessionRegistration>>,
) {
    let home = crate::run::lingxi_home_dir();
    let dir = platform_api::live_sessions::LiveSessionDir::at_live(
        crate::agents_registry::sessions_dir(&home),
    );
    if platform_api::live_sessions::process_dir().is_none() {
        let claim =
            platform_api::live_sessions::install_process(dir.clone(), session_id, user_name);
        if let Some(claim) = claim {
            if let Some(reg) = registration {
                reg.set_name(
                    &claim.name,
                    if claim.notice.is_some() {
                        "collision"
                    } else {
                        "user"
                    },
                );
            }
        }
    } else {
        platform_api::live_sessions::set_process_session_id(session_id);
        if let Some(name) = user_name.filter(|s| !s.is_empty()) {
            platform_api::live_sessions::set_process_name(name);
        }
    }
    let mut derived_name: Option<String> = None;
    if platform_api::live_sessions::process_name()
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        if let Some(derived) = user_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            })
        {
            platform_api::live_sessions::set_process_name(derived.clone());
            derived_name = Some(derived);
        }
    }
    let path = match platform_api::uds_inbox::process_socket_path() {
        Some(existing) => existing,
        None => {
            // (CLI-12, cc2.1.238) `--messaging-socket-path <path>` overrides the
            // auto-generated `mum()` path; absent, the oracle's own default
            // applies (`platform_api::uds_inbox::default_socket_path`).
            let sock = messaging_socket_override().unwrap_or_else(|| {
                platform_api::uds_inbox::default_socket_path(std::process::id())
            });
            match platform_api::uds_inbox::start_process_inbox(sock) {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!(%error, "cross-session inbox unavailable");
                    eprintln!("lingxi-cli: cross-session inbox unavailable: {error}");
                    return;
                }
            }
        }
    };
    let name_source = derived_name.as_deref().map(|_| "derived");
    if let Some(reg) = registration {
        reg.set_messaging_socket(&path);
        reg.set_session_id(session_id);
        if let Some(name) = derived_name.as_deref().filter(|s| !s.trim().is_empty()) {
            reg.set_name(name, "derived");
        }
        if let Some(class) = platform_api::live_sessions::process_permission_class() {
            reg.set_permission_class(&class);
        }
    }
    let _ = dir.upsert_identity(
        std::process::id(),
        session_id,
        platform_api::live_sessions::process_name().as_deref(),
        name_source,
        Some(&path),
        platform_api::live_sessions::process_permission_class().as_deref(),
    );
}

/// (iocraft → ratatui migration) Launch the `tui-rata` interactive chat wired
/// to the live orchestrator: the bridge receiver streams `TurnEvent`s into the
/// app, and each submit emits `TurnStarted` + spawns a streaming turn on the
/// same channel. The blocking ratatui loop runs on a `spawn_blocking` thread;
/// turns are spawned back onto the async runtime via the captured handle.
///
/// (M8 cc2.1.198) `registration` — when `Some`, the bridge + permission
/// channels are interposed with status forwarders that rewrite this session's
/// `sessions/<pid>.json` record on state changes (binary `mvn` + the REPL
/// status effect @222989611): `TurnStarted` → `busy`, `TurnEnded` → `idle`,
/// a pending permission exchange → `waiting` with `waitingFor: "permission
/// prompt"` (the binary's dialog-open reason), resolved exchange → `busy`.
/// How [`run_ratatui`] exited, so the mount caller
/// ([`crate::run::drive_tui_switch_loop`]) can either finish (returning the
/// process exit code) or follow an in-session `/resume` switch by re-mounting
/// the chosen session in-process (writer retargeted) — never an in-place
/// `resume_session` swap, which would fork the conversation across files.
/// The in-session live session state carried through a re-mount so the rebuilt
/// runtime keeps the user's mid-session toggles instead of reverting to the
/// boot/config defaults. `None` (the whole `Option`) on a cold `--resume` — the
/// runtime opens on config/CLI defaults, matching claude-code.
///
/// PARITY NOTE: claude-code's `/rewind`/`/resume` are in-place React `setState`
/// transitions — the process/runtime is never rebuilt, so `mainLoopModel`,
/// fast-mode and plan-mode (all separate state) naturally persist, and
/// claude-code has NO per-session persistence for any of them. Our synchronous
/// ratatui loop must tear down + rebuild (the JSONL-writer-retarget seam), so we
/// carry this state IN MEMORY to reproduce the same observable behavior WITHOUT
/// writing config keys the upstream never had.
pub(crate) struct RemountState {
    /// Active `(request_model, provider_profile)` — `None` resolves the model
    /// by-id (no profile). Applied via `switch_model`.
    pub model: Option<(String, Option<String>)>,
    /// `/fast` toggle. Applied via `set_fast_mode`.
    pub fast_mode: bool,
    /// `/plan` mode. Applied via `set_plan_mode`.
    pub plan_mode: bool,
    /// The live permission mode as a wire string (Shift+Tab indicator +
    /// enforcing gate), or `None` when no enforcing gate is wired. Carried so
    /// the re-mount restores the user's mid-session mode instead of resetting to
    /// the CLI/config default — applied to BOTH the rebuilt gate (enforcement)
    /// and the indicator seed on re-mount. See [`PermissionGate::permission_mode`].
    pub permission_mode: Option<String>,
    /// One-shot system notice appended to the re-mounted transcript (the
    /// `/branch` success confirmation — claude-code renders it in the NEW
    /// branch). `None` for /resume switches and rewinds.
    pub notice: Option<String>,
}

pub(crate) enum RunOutcome {
    /// The TUI exited normally; carry the process exit code.
    Exit(i32),
    /// The `/resume` picker resolved to this session uuid; the caller re-mounts
    /// it in-process via [`crate::run::load_resume_session`] +
    /// [`crate::run::mount_resumed_tui`], carrying the outgoing session's active
    /// [`RemountModel`].
    SwitchTo {
        target: uuid::Uuid,
        state: Option<RemountState>,
    },
    /// The agents view selected a session. The switch loop resolves the live
    /// owner, attaches or queues an exact resume when appropriate, and only
    /// converts this to `SwitchTo` after proving the transcript unowned.
    OpenAgentSession {
        target: tui::bottom_pane::view::AgentSessionTarget,
        state: Option<RemountState>,
    },
    /// `/branch`: fork the session driving the loop into a new session and
    /// switch into it. [`crate::run::drive_tui_switch_loop`] creates the branch
    /// transcript from the CURRENT session, then re-mounts the new id via
    /// [`crate::run::mount_resumed_tui`]. `title` is the optional `/branch
    /// [name]` argument (`None` ⇒ derive the branch name from the first prompt).
    BranchFrom {
        title: Option<String>,
        state: Option<RemountState>,
    },
    /// `/rewind`: restore working tree and/or conversation to `message`, then
    /// re-mount via [`crate::run::mount_resumed_tui`], carrying the model.
    RewindTo {
        message: uuid::Uuid,
        scope: tui::bottom_pane::view::RewindScope,
        state: Option<RemountState>,
    },
}

pub(crate) async fn run_ratatui(
    tui_build: crate::init::TuiBuild,
    registration: Option<Arc<crate::agents_registry::SessionRegistration>>,
    resumed_messages: Vec<tui::RenderedMessage>,
) -> RunOutcome {
    run_ratatui_with_initial_prompt(tui_build, registration, resumed_messages, None).await
}

/// Mount the normal interactive TUI and submit one prompt after the terminal
/// and widget are live. Background PTY children use this rather than placing a
/// prompt on their argv, because argv prompt routing intentionally selects
/// print mode before the TUI can mount.
pub(crate) async fn run_ratatui_with_initial_prompt(
    tui_build: crate::init::TuiBuild,
    registration: Option<Arc<crate::agents_registry::SessionRegistration>>,
    resumed_messages: Vec<tui::RenderedMessage>,
    initial_prompt: Option<String>,
) -> RunOutcome {
    run_ratatui_with_initial_state(
        tui_build,
        registration,
        resumed_messages,
        initial_prompt,
        None,
    )
    .await
}

/// Mount a TUI while restoring a durable foreground→background boundary.
pub(crate) async fn run_ratatui_with_initial_state(
    tui_build: crate::init::TuiBuild,
    registration: Option<Arc<crate::agents_registry::SessionRegistration>>,
    resumed_messages: Vec<tui::RenderedMessage>,
    initial_prompt: Option<String>,
    handoff: Option<platform_api::BackgroundingSnapshot>,
) -> RunOutcome {
    let prompt_queue = Arc::new(msgqueue::MessageQueueManager::new());
    let teammate_turn_gate = Arc::new(tokio::sync::Mutex::new(()));
    let leader_mailbox = tui_build
        .runtime
        .coordinator
        .mailbox_router
        .get(&tui_build.runtime.coordinator.coordinator_id)
        .await;
    tui_build
        .runtime
        .orchestrator
        .set_mid_turn_input(Arc::new(TuiMsgQueueInput {
            queue: prompt_queue.clone(),
        }));
    let queue_cancel_reason = orchestrator::prompt::mid_turn_input::CancelReasonFlag::new();
    tui_build
        .runtime
        .orchestrator
        .set_cancel_reason(queue_cancel_reason.clone());
    {
        let reason = queue_cancel_reason.clone();
        prompt_queue
            .set_now_abort_hook(Arc::new(move || {
                reason.set(orchestrator::prompt::mid_turn_input::CancelReason::QueueNowCommand);
            }))
            .await;
    }
    let concrete_orchestrator = tui_build.runtime.orchestrator.clone();
    let orchestrator: Arc<dyn OrchestratorHandle> = concrete_orchestrator.clone();
    // Boot permission mode + bypass-cycle availability for the indicator (Copy,
    // captured before `tui_build` is partly consumed below).
    let initial_permission_mode = tui_build.initial_permission_mode;
    let bypass_available = tui_build.bypass_available;
    platform_api::live_sessions::set_process_permission_mode(
        initial_permission_mode.wire_str(),
        bypass_available,
    );
    let current_session_id = orchestrator.current_session_id().await.to_string();
    ensure_live_messaging(&current_session_id, None, registration.as_ref());
    let (bridge_rx, permission_rx, ask_user_question_rx, computer_access_rx) = match &registration {
        Some(reg) => (
            spawn_status_bridge_forwarder(tui_build.bridge_rx, reg.clone()),
            spawn_status_permission_forwarder(tui_build.permission_rx, reg.clone()),
            spawn_status_ask_user_question_forwarder(tui_build.ask_user_question_rx, reg.clone()),
            spawn_status_computer_access_forwarder(tui_build.computer_access_rx, reg.clone()),
        ),
        None => (
            tui_build.bridge_rx,
            tui_build.permission_rx,
            tui_build.ask_user_question_rx,
            tui_build.computer_access_rx,
        ),
    };
    let workflow_events = tui_build.workflow_events;
    let turn_tx = tui_build.turn_tx;
    // (companyAnnouncements) The merged array, moved out before `tui_build` is
    // consumed further below; selected + rendered into the startup banner.
    let company_announcements = tui_build.company_announcements;
    let emoji_completion_enabled = tui_build.emoji_completion_enabled;
    let startup_view_mode = tui_build.startup_view_mode.clone();
    // (/permissions) The gate's live allow-rule bucket + the settings-file
    // roots the interactive editor writes to (same roots the AllowAlways
    // persist uses). Grabbed before `tui_build` is consumed further below.
    let permission_paths = tui_build.permission_paths.clone();
    let session_allow_rules = tui_build.session_allow_rules.clone();
    let agents_snapshot_provider = {
        let home = crate::run::lingxi_home_dir();
        let self_pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
        let self_session_id = current_session_id.clone();
        Arc::new(move || {
            crate::background_dispatch::ensure_daemon_for_control(&home);
            crate::commands::agents::tui_agents_snapshot(
                &home,
                self_pid,
                Some(self_session_id.as_str()),
            )
        })
    };
    // (P1-08 runtime `/add-dir`) the live session-cwd cell + MCP registry the
    // `/add-dir` effect widens; cloned before `tui_build.runtime` is consumed.
    let permission_session_cwd = tui_build.runtime.session_cwd.clone();
    let permission_mcp_registry = tui_build.runtime.mcp_registry.clone();
    // Native scrollback must resolve relative attachment paths against the
    // LIVE session cwd: `/cd` swaps this shared cell after the TUI mounts.
    // Keep the TUI independent of the concrete cell type by passing a small
    // read-only callback into its blocking render loop.
    let hyperlink_cwd_provider = {
        let session_cwd = permission_session_cwd.clone();
        std::sync::Arc::new(move || session_cwd.cwd())
            as std::sync::Arc<dyn Fn() -> std::path::PathBuf + Send + Sync>
    };
    // The ENFORCING permission gate, for Shift+Tab live permission-mode cycling.
    let set_mode_gate = tui_build.runtime.enforcing_permission_gate.clone();
    // A second clone kept in THIS scope (the `on_set_permission_mode` closure
    // moves `set_mode_gate`): read on exit to snapshot the live permission mode
    // into `RemountState`, so an in-process `/resume`/`/branch`/`/rewind`
    // restores it instead of resetting to the CLI/config default.
    let carried_mode_gate = tui_build.runtime.enforcing_permission_gate.clone();
    // Cloned BEFORE `on_submit` (below) moves `turn_tx` into its closure.
    let web_turn_tx = turn_tx.clone();
    let teammate_turn_tx = turn_tx.clone();
    let set_mode_turn_tx = turn_tx.clone();
    let connect_turn_tx = turn_tx.clone();
    let permission_turn_tx = turn_tx.clone();
    let bash_turn_tx = turn_tx.clone();
    let compact_turn_tx = turn_tx.clone();
    let rename_turn_tx = turn_tx.clone();
    let fast_turn_tx = turn_tx.clone();
    let plan_turn_tx = turn_tx.clone();
    // (/reload-skills) The SAME shared `Arc<RwLock<CommandRegistry>>` the
    // dispatcher mutates, handed to the `ChatWidget` so `/reload-skills`
    // reloads the live registry. Cloned before `tui_build` is consumed.
    let command_registry = tui_build.runtime.dispatcher.registry();
    // (B4 Task 5 parity) Thread the composition root's shared subscription
    // slot so the widget's rate-limit composer reads the live snapshot at
    // compose time — same wiring as the iocraft `with_subscription` path.
    let subscription = tui_build.runtime.subscription.clone();
    // (/web async effects) Shared HTTP transport + credential store for the
    // `/web` test-search + secret-save effects, wired below.
    let web_key_store = tui_build.runtime.provider_key_store.clone();
    let web_http = tui_build.runtime.http.clone();
    // (/connect picker) Real per-provider login-method + availability maps,
    // cloned before `tui_build` is consumed further below — same convention
    // as `subscription`/`web_key_store` above.
    let connect_auth_methods = tui_build.runtime.provider_auth_methods.clone();
    let connect_availability = tui_build.runtime.provider_availability.clone();
    // (/connect async effects) Shared credential store + OAuth/Copilot
    // drivers for the real store-key / device-flow / browser sign-in
    // effects, wired below — same convention as the `/web` cluster above.
    // `web_key_store` is cloned again here (cheap `Arc` clone) rather than
    // reused so each closure owns an independent handle.
    let connect_key_store = tui_build.runtime.provider_key_store.clone();
    let connect_oauth = tui_build.runtime.oauth_connect_driver.clone();
    let connect_copilot = tui_build.runtime.connect_copilot.clone();
    // (`!` bash mode) Sandboxed bash runner for `!`-prefixed commands, cloned
    // before `tui_build` is consumed — same convention as the clusters above.
    let bash_runner = tui_build.runtime.bash_runner.clone();
    // (#3 shell-expansion) The shared prompt shell-expansion provider, cloned
    // before `tui_build` is consumed — threaded into the `ChatWidget` so a typed
    // `/commit` expands its embedded `!`git …`` bodies before submit.
    let shell_expansion = tui_build.runtime.shell_expansion.clone();
    let session =
        build_session_info(orchestrator.as_ref(), tui_build.runtime.model_provenance).await;
    // (cc 2.1.218) Persistent prompt history: the GLOBAL
    // `~/.lingxi/history.jsonl` store (rows carry a `project` field), keyed to
    // this session id for the recall ordering + dedupe key. `None` under the
    // `CLAUDE_CODE_SKIP_PROMPT_HISTORY` escape hatch, keeping recall
    // session-local like the pre-store TUI.
    let prompt_history = if session::prompt_history::PromptHistoryStore::disabled_by_env() {
        None
    } else {
        let history_session_id = orchestrator.current_session_id().await.to_string();
        let history_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Some(std::sync::Arc::new(
            session::prompt_history::PromptHistoryStore::new(
                &crate::run::lingxi_home_dir(),
                &history_cwd,
                Some(history_session_id),
            ),
        ))
    };
    let handle = tokio::runtime::Handle::current();
    let switch_orch = orchestrator.clone();
    // Cloned here (before the turn closure takes ownership) for the
    // `AddDirectory` arm's `DirectoryAdded` hook fire.
    let permission_orch = concrete_orchestrator.clone();
    let switch_handle = handle.clone();
    let web_handle = handle.clone();
    let connect_handle = handle.clone();
    let permission_handle = handle.clone();
    let bash_handle = handle.clone();
    let summary_orch = orchestrator.clone();
    // (/compact) A handle + orchestrator clone for the off-loop `force_compact`
    // effect closure, and a clone threaded into the `ChatWidget` to drive the
    // read/inject OrchestratorHandle-backed commands. Both taken BEFORE
    // `on_submit` moves `orchestrator`/`handle` into its closure.
    let compact_orch = orchestrator.clone();
    let compact_handle = handle.clone();
    let set_mode_handle = handle.clone();
    let set_mode_reg = registration.clone();
    let rename_orch = orchestrator.clone();
    let rename_reg = registration.clone();
    let rename_handle = handle.clone();
    let fast_orch = orchestrator.clone();
    let fast_handle = handle.clone();
    let plan_orch = orchestrator.clone();
    let plan_handle = handle.clone();
    // (registry slash dispatch) A fully-wired SHARED clone of the runtime's
    // dispatcher — it carries the SAME `Arc<RwLock<CommandRegistry>>` plus the
    // expansion hooks + shell-expansion provider — so a `/loop`/user-command/
    // skill typed in the TUI expands identically to the `-p` one-shot path.
    // Plus an orchestrator/handle/tx triplet for the off-loop dispatch closure.
    // All taken BEFORE `on_submit` moves `orchestrator`/`handle` into its closure.
    let dispatch_dispatcher = std::sync::Arc::new(tui_build.runtime.dispatcher.clone_shared());
    let dispatch_orch = orchestrator.clone();
    let dispatch_handle = handle.clone();
    let dispatch_turn_tx = turn_tx.clone();
    let rewake_orch = orchestrator.clone();
    let rewake_handle = handle.clone();
    let rewake_turn_tx = turn_tx.clone();
    // (/sandbox) The shared toggle cell threaded into the widget + clones for
    // the off-loop settings-persistence effect (mirrors the sibling triplets).
    let sandbox_toggle = tui_build.runtime.sandbox_toggle.clone();
    // (/sandbox description fidelity) Register the static config flags the dynamic
    // `/sandbox` popup description renders (claude-code auto-allow / fallback).
    tui::command::register_sandbox_desc_flags(tui::command::SandboxDescFlags {
        auto_allow: tui_build.runtime.sandbox_desc_auto_allow,
        fallback_allowed: tui_build.runtime.sandbox_desc_fallback,
        deps_ok: tui_build.runtime.sandbox_desc_deps_ok,
        ..Default::default()
    });
    // (/tasks) The live background-task registry (already `TaskRegistryHandle`),
    // cloned as a trait object for the widget's snapshot read; plus a handle +
    // tx clone for the off-loop stop effect (mirrors the sandbox triplet).
    let task_registry_handle: std::sync::Arc<dyn platform_api::task_registry::TaskRegistryHandle> =
        tui_build.runtime.task_registry.clone();
    if let Some(workflow_events) = workflow_events {
        spawn_workflow_event_forwarder(
            workflow_events,
            task_registry_handle.clone(),
            turn_tx.clone(),
        );
    }
    let task_handle = handle.clone();
    let task_turn_tx = turn_tx.clone();
    let agent_status_registry = task_registry_handle.clone();
    let agent_status_turn_tx = turn_tx.clone();
    let sandbox_paths = permission_paths.clone();
    let sandbox_handle = handle.clone();
    let sandbox_turn_tx = turn_tx.clone();
    let widget_orch = orchestrator.clone();
    // (/web async effects) Preload the shared `/web` config snapshot from the
    // real on-disk settings + credential-store presence, mirroring the
    // deleted iocraft `AppState::web_config_snapshot` startup seed. Shared
    // (`Arc<Mutex<_>>`) between `ChatWidget::cmd_web` (sync read to open the
    // picker) and `run_web_action` (async write-back after a save/test).
    let web_snapshot = {
        let cfg = tui::web::persist::web_settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| tool_web::web_search_config::WebSearchConfig::from_settings_json(&v))
            .unwrap_or_default();
        let tavily = web_key_store
            .get_provider_key("web:tavily")
            .await
            .ok()
            .flatten()
            .is_some();
        let brave = web_key_store
            .get_provider_key("web:brave")
            .await
            .ok()
            .flatten()
            .is_some();
        std::sync::Arc::new(std::sync::Mutex::new(tui::web::picker::WebConfigSnapshot {
            active: cfg.provider,
            tavily_key: tavily,
            brave_key: brave,
            searxng_url: cfg.searxng_url,
            last_test: None,
        }))
    };
    // (/permissions) Preload the rule snapshot from the user/project/local
    // settings files into a shared slot, mirroring the `/web` snapshot above.
    // `ChatWidget::cmd_permissions` reads a clone to seed the editor; the async
    // `on_permission_action` effect re-reads disk into this slot after each
    // edit so the NEXT `/permissions` open is current. Kept DISTINCT from the
    // startup `--resume` path (this is a settings read, not a session scan).
    let permission_snapshot = std::sync::Arc::new(std::sync::Mutex::new(
        tui::bottom_pane::permissions_editor_view::PermissionsSnapshot::load(&permission_paths),
    ));
    // (/plugin) Preload the installed-plugin list + enabled state for the
    // interactive manager; the `on_plugin_action` effect refreshes this slot
    // after each toggle. Scope roots: `home = ~/.lingxi` (user settings),
    // `cwd` = process dir (project/local settings), `plugins_dir = home/plugins`.
    let plugin_home = crate::run::lingxi_home_dir();
    let plugin_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let plugin_dir = plugin_home.join("plugins");
    let plugin_snapshot = std::sync::Arc::new(std::sync::Mutex::new(
        load_plugins_snapshot(&plugin_dir, &plugin_home, &plugin_cwd).await,
    ));
    let plugin_snapshot_cb = plugin_snapshot.clone();
    let plugin_effect_home = plugin_home.clone();
    let plugin_effect_cwd = plugin_cwd.clone();
    let plugin_effect_dir = plugin_dir.clone();
    let plugin_handle = handle.clone();
    let plugin_turn_tx = turn_tx.clone();
    let on_plugin_action = move |action: tui::bottom_pane::PluginAction| {
        let slot = plugin_snapshot_cb.clone();
        let home = plugin_effect_home.clone();
        let cwd = plugin_effect_cwd.clone();
        let dir = plugin_effect_dir.clone();
        let tx = plugin_turn_tx.clone();
        plugin_handle.spawn(async move {
            run_plugin_action(action, &dir, &home, &cwd, &slot, tx).await;
        });
    };
    // (`/reload-plugins`) The retained plugin subsystem — `None` when plugins are
    // disabled for the session. The effect re-reads the on-disk enabled set and
    // applies pending enable/disable changes to the LIVE registries off-loop,
    // reporting the component tallies via `TurnEvent::SystemNotice` — same
    // off-loop shape as `on_plugin_action`, but a READ+RECONCILE against the
    // engine's live `PluginManager` rather than a settings-file write.
    let reload_plugin_runtime = tui_build.runtime.plugin_runtime.clone();
    let reload_command_registry = command_registry.clone();
    let reload_handle = handle.clone();
    let reload_turn_tx = turn_tx.clone();
    let on_reload_plugins = move || {
        let rt = reload_plugin_runtime.clone();
        let registry = reload_command_registry.clone();
        let tx = reload_turn_tx.clone();
        reload_handle.spawn(async move {
            run_reload_plugins(rt, registry, tx).await;
        });
    };
    // (/resume) Preload the recent-session rows for the interactive picker.
    // This is an ASYNC disk scan (the M5-08 loader), so it MUST run here — the
    // blocking ratatui loop can't `.await`. The rows go slightly stale as new
    // sessions are written, exactly like claude-code's snapshot. Kept DISTINCT
    // from the startup `--resume` picker path (`run_resume_iocraft`): this seeds
    // the IN-SESSION `/resume` command, which switches the live session by an
    // in-process re-mount rather than a fresh launch.
    let resume_rows = crate::run::load_resume_picker_rows().await;
    let current_model = session
        .models
        .iter()
        .find(|m| m.is_current)
        .map_or_else(|| "(default)".to_string(), |m| m.display.clone());
    // A fresh launch opens on the welcome banner; a `--resume` mount opens on
    // the replayed prior conversation instead (matching the iocraft resume UX,
    // which shows the history with no fresh welcome). Both paths render through
    // the SAME ratatui backend so the composer/footer chrome is identical.
    let fresh_launch = resumed_messages.is_empty();
    let mut initial = if fresh_launch {
        vec![tui::RenderedMessage::SystemText {
            body: format!(
                "✻ Welcome to LingXi Code ({})\n  /help for commands · Esc interrupts a running turn · Esc (idle) or Ctrl-C twice to quit\n  cwd: {}\n  model: {}",
                session.doctor.cli_version, session.doctor.cwd, current_model
            ),
            timestamp: 0,
            is_error: false,
        }]
    } else {
        resumed_messages
    };
    // (companyAnnouncements) CC's `LVs`/`oip()` startup notice: select one of
    // the configured announcements (memoized on the process `Wxo` cache) and
    // render it as a dim startup-banner block. Just below the welcome header on
    // a fresh launch; top-of-feed (before the replayed history) on a resume.
    if let Some(msg) = build_company_announcement_message(company_announcements.as_deref()) {
        if fresh_launch {
            initial.push(msg);
        } else {
            initial.insert(0, msg);
        }
    }
    // Resume parity: if the cost tracker was seeded from a restored session
    // (`mount_resumed_tui`), surface its accumulated cost in the footer on the
    // very first frame — before any new turn — by emitting an initial
    // `CostUpdated`. A fresh session's tracker is zero, so this is a no-op
    // there. `turn_tx` and the widget's `bridge_rx` share one channel
    // (init.rs), so the event reaches the widget as soon as it starts draining.
    let initial_cost = orchestrator.snapshot_cost().await.total_usd;
    if initial_cost > 0.0 {
        let _ = turn_tx.send(tui::TurnEvent::CostUpdated(format!("${initial_cost:.4}")));
    }
    let submit_queue = prompt_queue.clone();
    let submit_turn_gate = teammate_turn_gate.clone();
    let submit_cancel_reason = queue_cancel_reason.clone();
    let on_submit =
        move |prompt: String, images: Vec<std::path::PathBuf>, cancel: CancellationToken| {
            let _ = turn_tx.send(tui::TurnEvent::TurnStarted);
            let orch = orchestrator.clone();
            let tx = turn_tx.clone();
            let queue = submit_queue.clone();
            let turn_gate = submit_turn_gate.clone();
            let cancel_reason = submit_cancel_reason.clone();
            handle.spawn(async move {
                let _turn_guard = turn_gate.lock().await;
                cancel_reason.reset();
                queue.register_active_turn(cancel.clone()).await;
                // Image-aware entry: with no images this is byte-identical to
                // `run_turn_streaming_with_cancel`; with pasted/attached images
                // they become `ContentBlock::Image` on the user message.
                if let Err(e) = orch
                    .run_turn_streaming_with_images(&prompt, &images, cancel)
                    .await
                {
                    // A HARD terminal error (rate limit, auth, model-unavailable, …)
                    // propagates as `Err` WITHOUT being surfaced as an assistant
                    // message or an `emit_end_turn` — unlike a graceful `model_error`,
                    // which the orchestrator renders + ends itself. So the retry loop
                    // could exhaust (e.g. "Retrying… attempt 10/10") and then hang the
                    // spinner forever with no error shown. Surface it as the turn's
                    // reply and end the turn so the spinner + any "Retrying…" status
                    // clear.
                    let _ = tx.send(tui::TurnEvent::TextDelta(format!("{e}")));
                    let _ = tx.send(tui::TurnEvent::TurnEnded(
                        platform_api::TurnOutcome::EndTurn,
                    ));
                }
                queue.clear_active_turn().await;
                drain_teammate_prompts(&queue, orch.as_ref(), &tx).await;
            });
        };
    let queued_prompt_queue = prompt_queue.clone();
    let queued_prompt_handle = tokio::runtime::Handle::current();
    let queued_prompt_tx = web_turn_tx.clone();
    let on_queue_prompt = move |prompt: String, images: Vec<std::path::PathBuf>| {
        let queue = queued_prompt_queue.clone();
        let tx = queued_prompt_tx.clone();
        queued_prompt_handle.spawn(async move {
            queue.enqueue(tui_prompt_command(prompt)).await;
            if !images.is_empty() {
                let _ = tx.send(tui::TurnEvent::SystemNotice {
                    body: "Queued text; pending image attachments are not supported yet."
                        .to_string(),
                    is_error: true,
                });
            }
        });
    };
    let on_switch_model = move |model: String, profile: Option<String>| {
        let orch = switch_orch.clone();
        switch_handle.spawn(async move {
            // Persist the pick only when the live switch succeeded (best-effort
            // writes; a failure never breaks the switch): `settings.model`
            // (qualified) so the choice survives a restart — the boot path
            // reads it back via `load_settings_model` — plus a `recentModels`
            // entry, the boot connected-provider fallback's first-preference
            // pass. This recording was lost in the iocraft-TUI deletion; the
            // ratatui picker previously only mutated in-memory session state.
            if orch
                .switch_model_with_source(&model, profile.as_deref(), "picker")
                .await
                .is_ok()
            {
                match profile.as_deref() {
                    Some(p) => {
                        tui_core::recent_models::record_default_model(&format!("{p}/{model}"));
                        tui_core::recent_models::record_recent_model(p, &model);
                    }
                    None => tui_core::recent_models::record_default_model(&model),
                }
            }
        });
    };
    // (/web async effects) The picker/config views return `WebAction`s
    // synchronously from the blocking ratatui loop; the actual persistence
    // (credential-store write, settings-file merge) and the test search are
    // async, so each action is spawned back onto the captured runtime handle
    // — same shape as `on_submit`/`on_switch_model` above. Results land in the
    // transcript via `TurnEvent::SystemNotice` on the shared `turn_tx`.
    let web_snapshot_cb = web_snapshot.clone();
    let on_web_action = move |action: tui::bottom_pane::WebAction| {
        let key_store = web_key_store.clone();
        let http = web_http.clone();
        let tx = web_turn_tx.clone();
        let snapshot = web_snapshot_cb.clone();
        web_handle.spawn(async move {
            run_web_action(action, key_store, http, tx, snapshot).await;
        });
    };
    // (/connect async effects) Mirrors `on_web_action` above: the picker/
    // method/key views return a `ConnectAction` synchronously from the
    // blocking ratatui loop; the actual persistence (credential-store
    // write) and the OAuth/Copilot device-flow are async, so the action is
    // spawned back onto the captured runtime handle. Results land in the
    // transcript via `TurnEvent::SystemNotice` on the shared `turn_tx`.
    let on_connect_action = move |action: tui::bottom_pane::ConnectAction| {
        let key_store = connect_key_store.clone();
        let oauth = connect_oauth.clone();
        let copilot = connect_copilot.clone();
        let tx = connect_turn_tx.clone();
        connect_handle.spawn(async move {
            run_connect_action(action, key_store, oauth, copilot, tx).await;
        });
    };
    // (/permissions async effect) The editor returns a `PermissionAction`
    // synchronously from the blocking ratatui loop; the settings-file write is
    // async, so it is spawned onto the captured handle — same shape as
    // `on_web_action` above. For an ADDED allow rule the effect also pushes
    // into the gate's live `session_allow_rules` so it takes effect THIS
    // session (deny/ask are effective-next-load). The result lands in the
    // transcript via `TurnEvent::SystemNotice`, and the shared snapshot slot is
    // refreshed so the next `/permissions` open reflects the edit.
    let permission_snapshot_cb = permission_snapshot.clone();
    let on_permission_action = move |action: tui::bottom_pane::PermissionAction| {
        let paths = permission_paths.clone();
        let allow_rules = session_allow_rules.clone();
        let tx = permission_turn_tx.clone();
        let snapshot = permission_snapshot_cb.clone();
        let session_cwd = permission_session_cwd.clone();
        let mcp_registry = permission_mcp_registry.clone();
        let orch = permission_orch.clone();
        permission_handle.spawn(async move {
            run_permission_action(
                action,
                paths,
                allow_rules,
                tx,
                snapshot,
                session_cwd,
                mcp_registry,
                orch,
            )
            .await;
        });
    };
    // (`!` bash mode) `!command` is submitted synchronously from the blocking
    // ratatui loop; the sandboxed run is async, so it is spawned onto the
    // captured handle and its captured stdout/stderr fold back into the
    // transcript via `TurnEvent::BashOutput` on the shared `turn_tx` — no LLM
    // turn, 1:1 with claude-code bash mode.
    let on_bash = move |command: String| {
        let runner = bash_runner.clone();
        let tx = bash_turn_tx.clone();
        bash_handle.spawn(async move {
            let out = runner.run(&command).await;
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::BashOutput {
                stdout: out.stdout,
                stderr: out.stderr,
            });
        });
    };
    // (/compact async effect) `/compact` returns `ChatOutcome::Compact` from the
    // blocking ratatui loop; the actual `force_compact()` is a real,
    // potentially multi-second LLM summarization round-trip whose reqwest/
    // websocket sockets are registered on THIS runtime. It is spawned onto the
    // captured handle — never `block_on`'d on the render thread — so it runs on
    // the correct reactor and never freezes input. `ChatWidget` prevents a live
    // turn or second submission from overlapping this whole-history swap. The
    // summary (or failure) lands in the transcript via `TurnEvent::SystemNotice`
    // on the shared `turn_tx`, mirroring `command_core::compact`'s display text.
    let on_compact = move |args: String, cancel: CancellationToken| {
        let orch = compact_orch.clone();
        let tx = compact_turn_tx.clone();
        compact_handle.spawn(async move {
            let custom = (!args.trim().is_empty()).then_some(args.as_str());
            let (body, is_error) = match orch
                .force_compact_with_instructions_and_cancel(custom, cancel)
                .await
            {
                Ok(_summary) => ("Compacted (ctrl+o to see full summary)".to_string(), false),
                // The per-class CC 2.1.217 failure display (hook reason
                // verbatim, exact cancel sentinel → `Compaction canceled.`,
                // `Error during compaction: …` passthrough) — shared with the
                // headless `/compact` handler.
                Err(e) => {
                    let raw = e.to_string();
                    (
                        command_core::compact::compact_failure_display(&raw),
                        command_core::compact::compact_failure_is_error(&raw),
                    )
                }
            };
            // Clear the spinner/bar first, then land the terminal line.
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::CompactEnded);
            let _ =
                tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
        });
    };
    // `/rename [name]`: a bare invocation first runs the history-inert,
    // tool-denied `rename_generate_name` side query; either path then persists
    // the custom title off the render thread.
    let on_rename = move |requested_name: String| {
        let orch = rename_orch.clone();
        let tx = rename_turn_tx.clone();
        let rename_reg = rename_reg.clone();
        rename_handle.spawn(async move {
            let name = if requested_name.trim().is_empty() {
                match orch.generate_session_name().await {
                    Ok(Some(name)) => name,
                    Ok(None) => {
                        let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                            body: "Could not generate a name: no conversation context yet. Usage: /rename <name>".to_string(),
                            is_error: false,
                        });
                        return;
                    }
                    Err(_) => {
                        let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                            body: "Error renaming session".to_string(),
                            is_error: true,
                        });
                        return;
                    }
                }
            } else {
                requested_name.trim().to_string()
            };
            let (body, is_error) = match orch.rename_session(name.clone()).await {
                Ok(()) => {
                    let advertised = if let Some(dir) = platform_api::live_sessions::process_dir() {
                        let sid = platform_api::live_sessions::process_session_id()
                            .unwrap_or_default();
                        match dir.claim_unique_name(&name, &sid, std::process::id()) {
                            Ok(claim) => {
                                platform_api::live_sessions::set_process_name(claim.name.clone());
                                if let Some(reg) = rename_reg.as_ref() {
                                    reg.set_name(
                                        &claim.name,
                                        if claim.notice.is_some() {
                                            "collision"
                                        } else {
                                            "user"
                                        },
                                    );
                                }
                                if let Some(notice) = claim.notice {
                                    let _ = tx.send(
                                        tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                                            body: notice,
                                            is_error: false,
                                        },
                                    );
                                    claim.name
                                } else {
                                    claim.name
                                }
                            }
                            Err(_) => name.clone(),
                        }
                    } else {
                        name.clone()
                    };
                    (format!("Session renamed to: {advertised}"), false)
                }
                Err(_e) => ("Error renaming session".to_string(), true),
            };
            let _ =
                tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
        });
    };
    // (/fast async effect) `/fast [on|off]` returns `ChatOutcome::FastMode` from
    // the blocking ratatui loop; the flag flip (and, for a bare `/fast`, the
    // read of the current value) runs off the render thread on the captured
    // handle (doctrine: side-effects go off-loop). The applied state is reported
    // via `TurnEvent::SystemNotice`. `None` = toggle; `Some(target)` = set.
    let on_fast_mode = move |target: Option<bool>| {
        let orch = fast_orch.clone();
        let tx = fast_turn_tx.clone();
        fast_handle.spawn(async move {
            let desired = match target {
                Some(v) => v,
                None => !orch.fast_mode().await,
            };
            let (body, is_error) = match orch.set_fast_mode(desired).await {
                Ok(()) => (
                    if desired {
                        "\u{26a1} Fast mode ON".to_string()
                    } else {
                        "Fast mode OFF".to_string()
                    },
                    false,
                ),
                Err(_e) => ("Error toggling fast mode".to_string(), true),
            };
            let _ =
                tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
        });
    };
    // `/plan`: enter plan mode, then reuse the orchestrator's real plan-file
    // path for view/open instead of maintaining a second TUI-only store.
    let on_plan_mode = move |args: String| {
        let orch = plan_orch.clone();
        let tx = plan_turn_tx.clone();
        plan_handle.spawn(async move {
            let already_in_plan_mode = orch.plan_mode().await;
            if !already_in_plan_mode {
                if orch.set_plan_mode(true).await.is_err() {
                    let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                        body: "Error entering plan mode".to_string(),
                        is_error: true,
                    });
                    return;
                }
                let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                    body: "Enabled plan mode".to_string(),
                    is_error: false,
                });
                return;
            }

            let action = args.split_whitespace().next().unwrap_or("");
            let plan = match orch.current_plan().await {
                Ok(plan) => plan,
                Err(_) => {
                    let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                        body: "Could not read the current plan".to_string(),
                        is_error: true,
                    });
                    return;
                }
            };
            let (body, is_error) = match plan {
                None => (
                    "Already in plan mode. No plan written yet.".to_string(),
                    false,
                ),
                Some(plan) if action == "open" => match orch.open_plan_editor().await {
                    Ok(outcome) if outcome.exit_code == 0 => (
                        format!("Opened plan in editor: {}", plan.path.display()),
                        false,
                    ),
                    Ok(_) | Err(_) => ("Could not open plan in editor".to_string(), true),
                },
                Some(_) if action == "share" => (
                    "Publishing plans is not available in this session.".to_string(),
                    false,
                ),
                Some(plan) => (render_plan_snapshot(&plan), false),
            };
            let _ =
                tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
        });
    };
    // (Shift+Tab) Push the cycled permission mode to the live ENFORCING gate
    // off the render thread, so enforcement follows the bottom-of-composer
    // indicator (the pane already updated its display). On failure — e.g. the
    // bypass killswitch — report via `TurnEvent::SystemNotice`.
    let on_set_permission_mode = move |mode: String| {
        let Some(gate) = set_mode_gate.clone() else {
            return;
        };
        let tx = set_mode_turn_tx.clone();
        let bypass_available = bypass_available;
        let set_mode_reg = set_mode_reg.clone();
        set_mode_handle.spawn(async move {
            if let Err(e) = gate.set_permission_mode(&mode).await {
                let _ = tx.send(tui::TurnEvent::SystemNotice {
                    body: format!("Could not change permission mode: {e}"),
                    is_error: true,
                });
            } else {
                platform_api::live_sessions::set_process_permission_mode(&mode, bypass_available);
                if let Some(reg) = set_mode_reg.as_ref() {
                    reg.set_permission_class(platform_api::live_sessions::permission_class_for(
                        &mode,
                        bypass_available,
                    ));
                }
            }
        });
    };
    // (/sandbox) The live toggle already flipped in the widget (a lock-free
    // AtomicBool store); this closure only PERSISTS the choice / appends an
    // exclude to the settings files off the render thread, reporting via
    // `TurnEvent::SystemNotice` — same off-loop shape as `on_permission_action`.
    let on_sandbox_action = move |action: tui::chat_widget::SandboxAction| {
        let paths = sandbox_paths.clone();
        let tx = sandbox_turn_tx.clone();
        sandbox_handle.spawn(async move {
            run_sandbox_action(action, paths, tx).await;
        });
    };
    // (/tasks) The picker returns a `TaskAction` synchronously from the blocking
    // ratatui loop; the kill (abort the background task + mark it `killed` in
    // the registry) is async, so it is spawned onto the captured handle — same
    // off-loop shape as `on_permission_action`. The result lands in the
    // transcript via `TurnEvent::SystemNotice`.
    let task_registry_effect = task_registry_handle.clone();
    let on_task_action = move |action: tui::bottom_pane::TaskAction| {
        let registry = task_registry_effect.clone();
        let tx = task_turn_tx.clone();
        task_handle.spawn(async move {
            let tui::bottom_pane::TaskAction::Kill { task_id } = action;
            // The `/tasks` picker is a USER gesture, so the killed agent's
            // notification reads "was stopped by user" (claude `killedBy` default).
            let (body, is_error) = match registry.kill_with_reason(&task_id, "user").await {
                Ok(_) => (format!("Stopped task {task_id}"), false),
                Err(e) => (format!("Could not stop task {task_id}: {e}"), true),
            };
            let _ =
                tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
        });
    };
    // (registry slash dispatch) `/loop`, user commands, skills, and plugin/
    // bundled commands are NOT in the TUI's static BUILTIN table. The widget
    // echoes the invocation and hands the raw input back as
    // `ChatOutcome::DispatchSlash`; here we run it through the live shared
    // dispatcher OFF the render thread — mirroring the `-p` path's
    // `run_slash_command`. A `type:"prompt"` command's expanded prompt runs as a
    // turn; a local command's output / the unknown-command literal surfaces via
    // `TurnEvent::SystemNotice`.
    let on_dispatch_slash = move |input: String, token: CancellationToken| {
        let dispatcher = dispatch_dispatcher.clone();
        let orch = dispatch_orch.clone();
        let tx = dispatch_turn_tx.clone();
        dispatch_handle.spawn(async move {
            use platform_api::{SlashCommandDispatcher, SlashDispatchResult};
            match dispatcher.dispatch(&input).await {
                SlashDispatchResult::RunAsTurn { prompt } => {
                    // The widget already set this token as `current_turn`; pass it
                    // through so Ctrl-C cancels the dispatched turn. On success the
                    // orchestrator emits `TurnEnded`; on a hard error we do.
                    let _ = tx.send(tui::TurnEvent::TurnStarted);
                    if let Err(e) = orch
                        .run_turn_streaming_with_images(&prompt, &[], token)
                        .await
                    {
                        let _ = tx.send(tui::TurnEvent::TextDelta(format!("{e}")));
                        let _ = tx.send(tui::TurnEvent::TurnEnded(
                            platform_api::TurnOutcome::EndTurn,
                        ));
                    }
                }
                // A local / unknown result runs no turn: surface the text and
                // emit `TurnEnded` so the widget clears the `current_turn` it set
                // when it echoed the invocation.
                SlashDispatchResult::Handled { display }
                | SlashDispatchResult::Unknown { display, .. } => {
                    let _ = tx.send(tui::TurnEvent::SystemNotice {
                        body: display,
                        is_error: false,
                    });
                    let _ = tx.send(tui::TurnEvent::TurnEnded(
                        platform_api::TurnOutcome::EndTurn,
                    ));
                }
                SlashDispatchResult::NotASlashCommand => {
                    let _ = tx.send(tui::TurnEvent::TurnEnded(
                        platform_api::TurnOutcome::EndTurn,
                    ));
                }
            }
        });
    };
    let on_rewake_peer = move || {
        let orch = rewake_orch.clone();
        let tx = rewake_turn_tx.clone();
        rewake_handle.spawn(async move {
            if let Err(error) = orch.run_async_hook_rewake().await {
                let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                    body: format!("Failed to deliver held peer message: {error}"),
                    is_error: true,
                });
            }
        });
    };
    // (statusline) Shared slot for the custom `statusLine` command, built from
    // the User+Local setting, plus the debounced single-flight pump (the
    // claude-code `StatusLine.tsx` execute-on-change analog: 300ms tick, run
    // only when the widget re-armed `dirty` on a turn boundary, set-only-on-
    // change). The slot is shared with the render thread via `run_app`.
    let resolved_status_lines = read_status_line_configs(
        tui_build.flag_settings.as_ref(),
        tui_build.status_line_source_scope,
    )
    .await;
    let subagent_status_line = resolved_status_lines.subagent;
    let status_line = tui::status_line::new_slot(resolved_status_lines.main);
    // Seed the boot-known 2.1.206 payload base fields (`Rf()`): session_id +
    // transcript_path (`<lingxi_home>/projects/<sanitize(cwd)>/<uuid>.jsonl`).
    {
        let sid = tui_build.runtime.orchestrator.current_session_id().await;
        let uuid = sid.as_uuid().to_string();
        let home = memory::session_memory::config_home_dir();
        let cwd = std::env::current_dir().unwrap_or_default();
        let transcript = orchestrator::transcript_paths::main_transcript_path(
            &home,
            &cwd.to_string_lossy(),
            &uuid,
        );
        if let Ok(mut s) = status_line.lock() {
            s.data.session_id = uuid;
            s.data.transcript_path = transcript.display().to_string();
        }
    }
    let pump_slot = status_line.clone();
    let status_pump = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut warned_failure = false;
        loop {
            interval.tick().await;
            // Build the (command, stdin-json) payload under the lock, then DROP
            // it before the off-thread command run. Skip unless re-armed.
            let payload = {
                let Ok(mut s) = pump_slot.lock() else {
                    continue;
                };
                let periodic_due = s
                    .config
                    .as_ref()
                    .and_then(|config| config.refresh_interval)
                    .is_some_and(|refresh| {
                        s.last_run_at
                            .is_none_or(|last_run| last_run.elapsed() >= refresh)
                    });
                if !s.dirty && !periodic_due {
                    continue;
                }
                s.dirty = false;
                s.last_run_at = Some(std::time::Instant::now());
                tui::status_line::build_payload(&s)
            };
            let Some((command, stdin_json)) = payload else {
                continue;
            };
            let out = tokio::task::spawn_blocking(move || {
                tui_core::status_line_command::run_status_line_command(
                    &command,
                    &stdin_json,
                    tui_core::status_line_command::STATUS_LINE_TIMEOUT,
                )
            })
            .await
            .unwrap_or_default();
            // A failed command must remove stale custom output so the built-in
            // footer remains authoritative. Keep diagnostics one-shot because
            // refreshInterval can otherwise emit the same warning forever.
            if let Ok(mut s) = pump_slot.lock() {
                if let Some(text) = out {
                    if s.text.as_deref() != Some(text.as_str()) {
                        s.text = Some(text);
                    }
                } else {
                    s.text = None;
                    if !warned_failure {
                        warned_failure = true;
                        tracing::warn!("statusLine failed; using the built-in status UI");
                    }
                }
            }
        }
    });
    // Live Agent/Task status below the composer. The task registry is the
    // authoritative lifecycle source for detached agents; send full snapshots
    // only on change so the blocking render loop does no async polling itself.
    let agent_status_line_slot = status_line.clone();
    let agent_status_pump = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = Vec::new();
        let mut custom_by_id = std::collections::HashMap::<String, String>::new();
        let mut custom_default: Option<String> = None;
        let mut custom_active = false;
        let mut last_custom_run: Option<std::time::Instant> = None;
        let mut last_custom_task_ids = Vec::<String>::new();
        let mut warned_custom_failure = false;
        loop {
            interval.tick().await;
            let Ok(records) = agent_status_registry
                .list(platform_api::task_registry::TaskListFilter::default())
                .await
            else {
                continue;
            };
            let mut records = records
                .into_iter()
                .filter(|record| {
                    is_agent_task_type(&record.task_type)
                        && matches!(record.status.as_str(), "pending" | "running")
                })
                .collect::<Vec<_>>();
            records.sort_by(|left, right| left.task_id.cmp(&right.task_id));
            let task_ids = records
                .iter()
                .map(|record| record.task_id.clone())
                .collect::<Vec<_>>();
            if task_ids.is_empty() {
                custom_by_id.clear();
                custom_default = None;
                custom_active = false;
                last_custom_task_ids.clear();
            }

            if let Some(config) = subagent_status_line
                .as_ref()
                .filter(|config| config.should_run(true))
            {
                let refresh_due = !records.is_empty()
                    && (task_ids != last_custom_task_ids
                        || last_custom_run
                            .is_none_or(|last_run| last_run.elapsed() >= config.refresh_interval));
                if refresh_due {
                    last_custom_run = Some(std::time::Instant::now());
                    last_custom_task_ids.clone_from(&task_ids);
                    let columns = crossterm::terminal::size()
                        .map(|(columns, _)| columns.max(1))
                        .unwrap_or(80);
                    let (base, model, effort, context_window_size, cwd) = {
                        let Ok(shared) = agent_status_line_slot.lock() else {
                            continue;
                        };
                        (
                            tui::status_line::build_input(&shared),
                            shared.data.model_id.clone(),
                            shared.data.effort_level.clone(),
                            shared.data.context_window_tokens,
                            shared.data.cwd.display().to_string(),
                        )
                    };
                    let tasks = records
                        .iter()
                        .map(|record| {
                            let name = record.agent_type.clone().unwrap_or_else(|| {
                                if record.task_type == "in_process_teammate" {
                                    "teammate".to_string()
                                } else {
                                    "Agent".to_string()
                                }
                            });
                            tui_core::status_line_command::SubagentStatusLineTask {
                                id: record.task_id.clone(),
                                name,
                                task_type: record.task_type.clone(),
                                status: record.status.clone(),
                                description: record.description.clone(),
                                label: record.description.clone(),
                                start_time: record.started_at_ms.unwrap_or(0),
                                model: model.clone(),
                                effort: effort.clone(),
                                context_window_size,
                                token_count: 0,
                                token_samples: Vec::new(),
                                cwd: cwd.clone(),
                            }
                        })
                        .collect::<Vec<_>>();
                    let input = tui_core::status_line_command::build_subagent_status_line_input(
                        &base, columns, &tasks,
                    )
                    .to_string();
                    let command = config.command.clone();
                    let output = tokio::task::spawn_blocking(move || {
                        tui_core::status_line_command::run_subagent_status_line_command(
                            &command,
                            &input,
                            columns,
                            tui_core::status_line_command::STATUS_LINE_TIMEOUT,
                        )
                    })
                    .await
                    .ok()
                    .flatten();
                    match output.filter(|rows| {
                        tui_core::status_line_command::validate_subagent_status_line_output(
                            rows, &task_ids,
                        )
                    }) {
                        Some(rows) => {
                            custom_by_id.clear();
                            custom_default = None;
                            for row in rows {
                                if let Some(id) = row.id.filter(|id| !id.is_empty()) {
                                    custom_by_id.insert(id, row.content);
                                } else {
                                    custom_default = Some(row.content);
                                }
                            }
                            custom_active = true;
                        }
                        None => {
                            custom_active = false;
                            if !warned_custom_failure {
                                warned_custom_failure = true;
                                tracing::warn!(
                                    "subagentStatusLine failed; using the built-in agent status"
                                );
                            }
                        }
                    }
                }
            }

            let agents = records
                .into_iter()
                .map(|record| tui_core::orchestrator_bridge::RunningAgentStatus {
                    awaiting_plan_approval: record.awaiting_plan_approval,
                    id: record.task_id.clone(),
                    task_type: record.task_type.clone(),
                    agent_type: record.agent_type.unwrap_or_else(|| {
                        if record.task_type == "in_process_teammate" {
                            "teammate".to_string()
                        } else {
                            "Agent".to_string()
                        }
                    }),
                    description: record.description,
                    status: record.status,
                    custom_content: custom_active.then(|| {
                        custom_by_id
                            .get(&record.task_id)
                            .cloned()
                            .or_else(|| custom_default.clone())
                            .unwrap_or_default()
                    }),
                })
                .collect::<Vec<_>>();
            if agents == previous {
                continue;
            }
            previous.clone_from(&agents);
            if agent_status_turn_tx
                .send(tui_core::orchestrator_bridge::TurnEvent::AgentStatusSnapshot { agents })
                .is_err()
            {
                break;
            }
        }
    });
    let teammate_prompt_pumps = leader_mailbox.map(|inbox| {
        let incoming_queue = prompt_queue.clone();
        let incoming = tokio::spawn(async move {
            loop {
                let Some(message) = inbox.wait_for_message(std::time::Duration::from_millis(500)).await else { continue; };
                incoming_queue.enqueue(msgqueue::QueuedCommand {
                    uuid: message.message_id,
                    content: msgqueue::QueuedCommandContent::UserInput { text: tasks::handlers::in_process_teammate::teammate_message_envelope_with_summary(&message.from_name,&message.content,message.summary.as_deref()) },
                    priority: msgqueue::QueuePriority::Next,
                    queued_at: message.timestamp,
                    source: msgqueue::QueueSource::AgentSendMessage,
                    agent_id: None,
                    skip_slash_commands: true,
                    is_meta: true,
                }).await;
            }
        });
        let queue = prompt_queue.clone();
        let orch = concrete_orchestrator.clone();
        let drain = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Ok(_guard) = teammate_turn_gate.try_lock() {
                    drain_teammate_prompts(&queue, orch.as_ref(), &teammate_turn_tx).await;
                }
            }
        });
        (incoming, drain)
    });
    let run_result = tokio::task::spawn_blocking(move || {
        tui::app::run_app(
            initial,
            initial_prompt,
            handoff,
            session,
            bridge_rx,
            permission_rx,
            ask_user_question_rx,
            computer_access_rx,
            Some(subscription),
            Some(status_line),
            Some(web_snapshot),
            Some(permission_snapshot),
            Some(plugin_snapshot),
            resume_rows,
            connect_auth_methods,
            connect_availability,
            Some(shell_expansion),
            Some(widget_orch),
            Some(hyperlink_cwd_provider),
            Some(sandbox_toggle),
            Some(command_registry),
            Some(task_registry_handle),
            prompt_history,
            initial_permission_mode,
            bypass_available,
            emoji_completion_enabled,
            startup_view_mode,
            Some(agents_snapshot_provider),
            on_submit,
            on_queue_prompt,
            on_switch_model,
            on_web_action,
            on_connect_action,
            on_permission_action,
            on_plugin_action,
            on_reload_plugins,
            on_bash,
            on_compact,
            on_rename,
            on_fast_mode,
            on_plan_mode,
            on_set_permission_mode,
            on_sandbox_action,
            on_task_action,
            on_dispatch_slash,
            on_rewake_peer,
        )
    })
    .await;
    if let Some((incoming, drain)) = teammate_prompt_pumps {
        incoming.abort();
        drain.abort();
    }
    status_pump.abort();
    agent_status_pump.abort();
    // (/stop, and clean shutdown) The render loop has torn down; close the
    // responses websocket + set `should_exit` on the LIVE handle HERE — on the
    // OUTER runtime where those sockets are registered — rather than on the
    // render thread's throwaway `block_on` runtime (a cross-reactor hazard). A
    // `/stop` reached this path by returning `ChatOutcome::Quit`; this is the
    // deferred half of its handler's `request_exit()`. Idempotent and harmless
    // on a normal `/exit`/Ctrl-C quit. The pure `session.lock()` reads below
    // (cost/session id) are unaffected by the closed websocket.
    summary_orch.request_exit().await;
    // Resume parity (claude-code `saveCurrentSessionCosts`, fired on process
    // exit): persist this session's accumulated cost to the project config,
    // keyed by (project, session id), so a later `--resume` of THIS session
    // restores it into the footer. Best-effort — a config-write failure must
    // never change the exit code. Runs for every exit arm (clean quit or TUI
    // failure), matching the reference's unconditional exit hook.
    {
        let total_usd = summary_orch.snapshot_cost().await.total_usd;
        let session_uuid = summary_orch.current_session_id().await.as_uuid();
        if let Some(cfg_path) = migrations::global_config::global_config_path() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            crate::session_cost::save_session_cost(
                &cfg_path,
                &cwd,
                &session_uuid.to_string(),
                total_usd,
            );
        }
    }
    // (/rewind, /resume, /branch re-mount) Capture the in-session live state
    // (model + fast-mode + plan-mode) so the re-mount can carry it into the
    // rebuilt runtime — see [`RemountState`] for why this is in-memory (not a
    // config key). Only consumed by the re-mount arms below; the plain-quit arm
    // drops it.
    let carried_state = {
        let status = summary_orch.get_status_snapshot().await;
        Some(RemountState {
            model: Some((status.model, status.model_profile)),
            fast_mode: summary_orch.fast_mode().await,
            plan_mode: summary_orch.plan_mode().await,
            permission_mode: carried_mode_gate.as_ref().and_then(|g| g.permission_mode()),
            notice: None,
        })
    };
    match run_result {
        Ok(Ok(tui::app::AppExit::Quit)) => {
            // Print the BARE uuid (not the `sess:`-prefixed SessionId Display):
            // it matches the on-disk `<uuid>.jsonl` and what `--resume` resolves
            // to (claude-code uses bare uuids for session ids end-to-end).
            let session_uuid = summary_orch.current_session_id().await.as_uuid();
            println!("\nSession {session_uuid} saved. Resume with: lingxi --resume {session_uuid}");
            RunOutcome::Exit(exit_codes::SUCCESS)
        }
        // (/resume) The picker resolved a session uuid: hand it up so the mount
        // caller re-mounts that session in-process. The outgoing session's cost
        // was already persisted above (that block runs for EVERY exit arm), and
        // `summary_orch.request_exit()` above closed the outgoing responses
        // websocket while `RataApp::run` cancelled the outgoing in-flight turn
        // before unwinding — so the outgoing session is fully wound down. SUPPRESS
        // the "Session saved" tail here: the process continues into the re-mount,
        // so that stdout line would otherwise scroll into the next session.
        Ok(Ok(tui::app::AppExit::SwitchSession(uuid))) => RunOutcome::SwitchTo {
            target: uuid,
            state: carried_state,
        },
        Ok(Ok(tui::app::AppExit::OpenAgentSession(target))) => RunOutcome::OpenAgentSession {
            target,
            state: carried_state,
        },
        Ok(Ok(tui::app::AppExit::BranchSession { title })) => RunOutcome::BranchFrom {
            title,
            state: carried_state,
        },
        Ok(Ok(tui::app::AppExit::Rewind { message, scope })) => RunOutcome::RewindTo {
            message,
            scope,
            state: carried_state,
        },
        Ok(Err(e)) => {
            eprintln!("lingxi-cli: tui-rata session failed: {e}");
            RunOutcome::Exit(exit_codes::RUNTIME_ERROR)
        }
        Err(e) => {
            eprintln!("lingxi-cli: tui-rata task join failed: {e}");
            RunOutcome::Exit(exit_codes::RUNTIME_ERROR)
        }
    }
}

/// Render the same plan file used by the plan-mode reminder. The defensive
/// character cap prevents an unexpectedly large or replaced file from flooding
/// the TUI event channel while preserving valid UTF-8 boundaries.
fn render_plan_snapshot(plan: &platform_api::PlanSnapshot) -> String {
    const MAX_PLAN_CHARS: usize = 1_000_000;
    let content: String = plan.content.chars().take(MAX_PLAN_CHARS).collect();
    format!("Current Plan\n{}\n\n{content}", plan.path.display())
}

fn is_agent_task_type(task_type: &str) -> bool {
    matches!(
        task_type,
        "local_agent" | "remote_agent" | "in_process_teammate"
    )
}

/// (companyAnnouncements) Build the startup-banner announcement block from the
/// merged `settings.companyAnnouncements`, selecting one (memoized on the
/// process-global `Wxo` cache) and pairing it with the optional
/// `Message from <org>:` prefix — CC's `LVs`. `numStartups` and the org name
/// come from the global config ([`read_startup_config`]). Returns a dim
/// `SystemText` block, or `None` when there is nothing to show (`oip()` false).
///
/// DIVERGENCE (visual-only): CC renders the announcement body in normal color
/// with only the `Message from …:` prefix dim; lingxi's startup banner is
/// uniformly dim (`system_text_lines` → `theme.dim`), so the whole block is
/// rendered dim as a single `SystemText` — the parity-critical selection logic
/// and the prefix text are faithful.
fn build_company_announcement_message(
    announcements: Option<&[String]>,
) -> Option<tui::RenderedMessage> {
    let (num_startups, organization_name) = read_startup_config();
    let sa = lingxi_core::settings::company_announcements::startup_announcement(
        announcements,
        num_startups,
        organization_name.as_deref(),
    )?;
    Some(announcement_message(sa))
}

/// Format a selected [`lingxi_core::settings::company_announcements::StartupAnnouncement`]
/// into the dim startup-banner `SystemText` block: the `Message from <org>:`
/// prefix line (when present) directly above the announcement body — CC's `LVs`
/// column `[em_, PVs]`.
fn announcement_message(
    sa: lingxi_core::settings::company_announcements::StartupAnnouncement,
) -> tui::RenderedMessage {
    let body = match sa.org_prefix {
        Some(prefix) => format!("{prefix}\n{}", sa.body),
        None => sa.body,
    };
    tui::RenderedMessage::SystemText {
        body,
        timestamp: 0,
        is_error: false,
    }
}

/// Read `(numStartups, oauthAccount.organizationName)` from the global config
/// (`~/.lingxi.json`) — CC `St().numStartups` + `Nc()?.organizationName`. A
/// missing HOME, missing file, or broken JSON degrades to `(0, None)` (the
/// announcement still shows via the random branch; no org prefix).
fn read_startup_config() -> (u64, Option<String>) {
    let Some(path) = migrations::global_config::global_config_path() else {
        return (0, None);
    };
    let Ok(map) = migrations::global_config::read_map(&path) else {
        return (0, None);
    };
    read_startup_config_from(&map)
}

/// Extract `(numStartups, oauthAccount.organizationName)` from an already-read
/// global-config map (the testable core of [`read_startup_config`]).
fn read_startup_config_from(
    map: &serde_json::Map<String, serde_json::Value>,
) -> (u64, Option<String>) {
    let num_startups = map
        .get("numStartups")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let organization_name = map
        .get("oauthAccount")
        .and_then(serde_json::Value::as_object)
        .and_then(|o| o.get("organizationName"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    (num_startups, organization_name)
}

/// (`/plugin`) Merged `enabledPlugins` allowlist across the writable settings
/// scopes (user `~/.lingxi/settings.json` < project `.lingxi/settings.json` <
/// local `.lingxi/settings.local.json`). A small local reader —
/// `plugin_settings::read_enabled` is private. A missing / broken file is
/// skipped (never overwritten; this is a read).
fn read_merged_enabled_plugins(
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> std::collections::BTreeMap<String, bool> {
    use serde_json::Value;
    let mut merged = std::collections::BTreeMap::new();
    let files = [
        home.join("settings.json"),
        cwd.join(branding::DOT_DIR).join("settings.json"),
        cwd.join(branding::DOT_DIR).join("settings.local.json"),
    ];
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(Value::Object(ep)) = obj.get("enabledPlugins") {
            for (k, v) in ep {
                if let Some(b) = v.as_bool() {
                    merged.insert(k.clone(), b);
                }
            }
        }
    }
    merged
}

/// (`/plugin`) Build the manager snapshot: every installed plugin
/// (`plugin::discover_recorded_plugins`) joined with its merged enabled state.
/// A plugin is "enabled" if its bare name or any `name@marketplace` key is set
/// true in the merged allowlist.
async fn load_plugins_snapshot(
    plugins_dir: &std::path::Path,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> tui::bottom_pane::plugins_view::PluginsSnapshot {
    use tui::bottom_pane::plugins_view::{PluginRow, PluginsSnapshot};
    let enabled_map = read_merged_enabled_plugins(home, cwd);
    let discovered = plugin::discover_recorded_plugins(plugins_dir).await;
    let plugins = discovered
        .into_iter()
        .map(|(_, m, _)| {
            let name = m.name.clone();
            let enabled = enabled_map
                .iter()
                .any(|(k, v)| *v && (k == &name || k.starts_with(&format!("{name}@"))));
            PluginRow {
                id: name.clone(),
                name,
                version: m.version,
                description: m.description,
                enabled,
            }
        })
        .collect();
    PluginsSnapshot { plugins }
}

/// (`/plugin`) Run one toggle to completion off the render loop: flip the
/// on-disk `enabledPlugins` allowlist via the CLI `plugin_settings` seam, then
/// refresh the shared snapshot so the next open reflects it. Result via
/// `TurnEvent::SystemNotice`.
async fn run_plugin_action(
    action: tui::bottom_pane::PluginAction,
    plugins_dir: &std::path::Path,
    home: &std::path::Path,
    cwd: &std::path::Path,
    slot: &std::sync::Arc<std::sync::Mutex<tui::bottom_pane::plugins_view::PluginsSnapshot>>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use tui::bottom_pane::PluginAction;
    let result = match &action {
        PluginAction::Enable { id } => {
            crate::commands::plugin_settings::run_enable(id, None, home, cwd)
        }
        PluginAction::Disable { id } => {
            crate::commands::plugin_settings::run_disable(Some(id), None, false, home, cwd)
        }
    };
    let (body, is_error) = match result {
        Ok(msg) => (msg, false),
        Err(e) => (e, true),
    };
    // Refresh the snapshot so the manager's next open reflects the toggle.
    let fresh = load_plugins_snapshot(plugins_dir, home, cwd).await;
    if let Ok(mut guard) = slot.lock() {
        *guard = fresh;
    }
    let _ = turn_tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice { body, is_error });
}

/// `N noun` / `N nouns` — naive `+s` plural, matching claude-code's
/// `plural()` for these ASCII nouns.
fn reload_plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The `/reload-plugins` result line's body (mirrors claude-code's
/// `refreshActivePlugins` formatter). §22: the error tally routes readers to
/// `/plugin` (`N error(s) during load. Run /plugin for details.`) — a prior
/// port revision misrouted this to `/doctor`, which has no plugin-load detail
/// to show.
fn reload_summary_body(c: &engine_desktop::PluginRefreshCounts) -> String {
    // claude-code labels plugin COMMANDS "skills" in this line
    // (`n(command_count, 'skill')`); `agent_count`/hooks/MCP/LSP mirror the
    // same result struct.
    let parts = [
        reload_plural(c.enabled, "plugin"),
        reload_plural(c.commands, "skill"),
        reload_plural(c.agents, "agent"),
        reload_plural(c.hooks, "hook"),
        reload_plural(c.mcp, "plugin MCP server"),
        reload_plural(c.lsp, "plugin LSP server"),
    ];
    let mut body = format!("Reloaded: {}", parts.join(" \u{00b7} "));
    if c.errors > 0 {
        body.push_str(&format!(
            "\n{} during load. Run /plugin for details.",
            reload_plural(c.errors, "error")
        ));
    }
    body
}

/// (`/reload-plugins`) Apply pending plugin enable/disable changes to the LIVE
/// session: [`engine_desktop::PluginRuntime::refresh`] re-reads the on-disk
/// enabled set and reconciles it into the engine's retained registries
/// (commands/hooks/agents/MCP/LSP swap in place), then this reports the component
/// tallies via `TurnEvent::SystemNotice`. Mirrors claude-code's
/// `refreshActivePlugins` result line (`commands/reload-plugins/reload-plugins.ts`
/// `Reloaded: N plugins · N skills · …`). A `None` runtime means plugins are
/// disabled for the session (safe mode / `--bare`).
async fn run_reload_plugins(
    plugin_runtime: Option<std::sync::Arc<engine_desktop::PluginRuntime>>,
    command_registry: std::sync::Arc<tokio::sync::RwLock<command_api::CommandRegistry>>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use tui_core::orchestrator_bridge::TurnEvent;

    let Some(rt) = plugin_runtime else {
        let _ = turn_tx.send(TurnEvent::SystemNotice {
            body: "Plugins are disabled for this session.".to_string(),
            is_error: false,
        });
        return;
    };

    let c = rt.refresh().await;
    let registry_rows = {
        use command_api::SlashCommandKind;
        let with_slash = |name: &str| {
            if name.starts_with('/') {
                name.to_string()
            } else {
                format!("/{name}")
            }
        };
        command_registry
            .read()
            .await
            .list_all()
            .into_iter()
            .filter(|cmd| {
                matches!(
                    cmd.kind,
                    SlashCommandKind::Markdown { .. }
                        | SlashCommandKind::Plugin { .. }
                        | SlashCommandKind::Bundled { .. }
                        | SlashCommandKind::Mcp { .. }
                )
            })
            .filter(|cmd| cmd.user_invocable != Some(false))
            .map(|cmd| tui_core::orchestrator_bridge::CommandCatalogEntry {
                name: with_slash(&cmd.name),
                description: cmd.description.clone(),
                menu_description: cmd.menu_description.clone(),
                aliases: cmd.aliases.iter().map(|alias| with_slash(alias)).collect(),
            })
            .collect()
    };
    let _ = turn_tx.send(TurnEvent::CommandCatalogRefreshed {
        commands: registry_rows,
    });
    let body = reload_summary_body(&c);
    let _ = turn_tx.send(TurnEvent::SystemNotice {
        body,
        is_error: c.errors > 0,
    });
}

/// Run one `/sandbox` [`tui::chat_widget::SandboxAction`] to completion off the
/// render loop: persist the toggled `sandbox.enabled` into the USER settings
/// file, or append an `exclude` pattern to `sandbox.excludedCommands` in the
/// LOCAL settings file (claude-code `addToExcludedCommands`). The live session's
/// bash sandboxing was already flipped by the widget via the shared toggle cell;
/// this write makes the choice stick. A JSON-broken settings file is left
/// untouched (the read errors out) rather than clobbered. Result reported via
/// `TurnEvent::SystemNotice`.
async fn run_sandbox_action(
    action: tui::chat_widget::SandboxAction,
    paths: permission::PermissionPaths,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use permission::PermissionUpdateDestination;
    use serde_json::{json, Map, Value};
    use tui::chat_widget::SandboxAction;
    use tui_core::orchestrator_bridge::TurnEvent;

    // Read → mutate the `sandbox` object → write, preserving sibling keys. A
    // missing file starts empty; a JSON-broken file surfaces an error and is
    // NOT overwritten.
    fn merge<F: FnOnce(&mut Map<String, Value>)>(
        path: &std::path::Path,
        mutate: F,
    ) -> std::io::Result<()> {
        let mut root: Map<String, Value> = match std::fs::read_to_string(path) {
            Ok(c) if c.trim().is_empty() => Map::new(),
            Ok(c) => serde_json::from_str(&c)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(e),
        };
        let sandbox = root
            .entry("sandbox")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(obj) = sandbox.as_object_mut() {
            mutate(obj);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let serialized = serde_json::to_string_pretty(&root).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(path, serialized + "\n")
    }

    let (body, is_error) = match action {
        SandboxAction::SetEnabled(enabled) => {
            let Some(path) = paths.destination_path(PermissionUpdateDestination::UserSettings)
            else {
                return;
            };
            match merge(&path, |o| {
                o.insert("enabled".to_string(), json!(enabled));
            }) {
                Ok(()) => (
                    format!(
                        "Sandbox mode {}.",
                        if enabled { "enabled" } else { "disabled" }
                    ),
                    false,
                ),
                Err(e) => (format!("Failed to save sandbox setting: {e}"), true),
            }
        }
        SandboxAction::Exclude(pattern) => {
            let Some(path) = paths.destination_path(PermissionUpdateDestination::LocalSettings)
            else {
                return;
            };
            let clean = pattern.clone();
            match merge(&path, |o| {
                let list = o
                    .entry("excludedCommands")
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Some(arr) = list.as_array_mut() {
                    if !arr.iter().any(|v| v.as_str() == Some(clean.as_str())) {
                        arr.push(json!(clean));
                    }
                }
            }) {
                Ok(()) => {
                    // claude-code 2.1.205 echoes the settings file's
                    // cwd-relative path (fallback literal when unresolvable).
                    let rel = std::env::current_dir()
                        .ok()
                        .and_then(|cwd| {
                            path.strip_prefix(&cwd)
                                .ok()
                                .map(std::path::Path::to_path_buf)
                        })
                        .map_or_else(
                            || ".lingxi/settings.local.json".to_string(),
                            |p| p.display().to_string(),
                        );
                    (
                        format!("Added \"{pattern}\" to excluded commands in {rel}"),
                        false,
                    )
                }
                Err(e) => (format!("Failed to update excluded commands: {e}"), true),
            }
        }
    };
    let _ = turn_tx.send(TurnEvent::SystemNotice { body, is_error });
}

/// Roll a `/cd` transcript move back when the durable background launch
/// identity could not be refreshed. `launch.json` is authoritative: callers
/// invoke this only after its read/write path returned an error, before
/// acknowledging the directory change. If the inverse move also fails, keep
/// the live cwd paired with the transcript that did move and terminal-fail the
/// background job so a later respawn cannot consume the stale launch record.
pub(crate) async fn rollback_cd_after_launch_identity_failure(
    orch: &std::sync::Arc<orchestrator::ConversationOrchestrator>,
    session_cwd: &std::sync::Arc<tool_api::SessionCwd>,
    previous: (std::path::PathBuf, Vec<std::path::PathBuf>),
    refresh_error: &str,
) -> String {
    let previous_cwd = previous.0.clone();
    match orch.retarget_transcript_for_cwd(&previous_cwd).await {
        Ok(_) => {
            session_cwd.swap(previous.0, previous.1);
            format!("Could not change directory: {refresh_error}")
        }
        Err(rollback_error) => {
            fail_current_background_job_after_cd_failure(&format!(
                "background launch identity refresh failed and transcript rollback failed: {refresh_error}; rollback: {rollback_error}"
            ));
            tracing::error!(
                %refresh_error,
                %rollback_error,
                "failed to roll back /cd after background launch identity refresh failure; job disabled"
            );
            format!(
                "Could not change directory: {refresh_error}; background session disabled because transcript rollback failed: {rollback_error}"
            )
        }
    }
}

fn fail_current_background_job_after_cd_failure(detail: &str) {
    let Ok(job_dir_raw) = std::env::var("LINGXI_JOB_DIR") else {
        return;
    };
    let job_dir = std::path::PathBuf::from(job_dir_raw);
    let Some(short) = job_dir.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    let Some(config_home) = job_dir.parent().and_then(std::path::Path::parent) else {
        return;
    };
    if let Err(error) = crate::agents_registry::update_job_state_with_detail(
        config_home,
        short,
        "failed",
        None,
        detail,
    ) {
        tracing::warn!(%error, "failed to disable background job after /cd rollback failure");
    }
}

/// Run one `/permissions` [`tui::bottom_pane::PermissionAction`] to
/// completion: merge the added/removed rule into its destination settings file
/// via [`permission::persist_permission_update`] /
/// [`permission::remove_permission_update`], and (for an ADDED allow rule) push
/// it into the gate's live `session_allow_rules` so it takes effect this
/// session. The snapshot slot is re-read from disk afterward so the next
/// `/permissions` open reflects the edit, and the outcome is reported to the
/// transcript via `TurnEvent::SystemNotice` — mirroring [`run_web_action`]'s
/// off-loop shape.
async fn run_permission_action(
    action: tui::bottom_pane::PermissionAction,
    paths: permission::PermissionPaths,
    session_allow_rules: std::sync::Arc<tokio::sync::Mutex<Vec<permission::PermissionRule>>>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
    snapshot: std::sync::Arc<
        std::sync::Mutex<tui::bottom_pane::permissions_editor_view::PermissionsSnapshot>,
    >,
    // (P1-08 runtime `/add-dir` live effect) the SAME session cwd cell the file
    // tools gate on + the live MCP registry — used only by the `AddDirectory`
    // arm to apply the add to the running session (file access + roots/list).
    session_cwd: std::sync::Arc<tool_api::SessionCwd>,
    mcp_registry: std::sync::Arc<mcp::McpRegistry>,
    // Used by the `AddDirectory` arm to fire the `DirectoryAdded` hook
    // (2.1.219) after a directory is actually added, and by `/cd` to persist
    // the transcript relocation against the concrete session writer.
    orch: std::sync::Arc<orchestrator::ConversationOrchestrator>,
) {
    use permission::{
        persist_permission_update, remove_permission_update, PermissionBehavior, PermissionRule,
        PermissionRuleSource, PermissionRuleValue, PermissionUpdate, PermissionUpdateDestination,
    };
    use tui::bottom_pane::permissions_editor_view::PermissionsSnapshot;
    use tui::bottom_pane::PermissionAction;
    use tui_core::orchestrator_bridge::TurnEvent;

    let behavior_word = |b: PermissionBehavior| match b {
        PermissionBehavior::Allow => "allow",
        PermissionBehavior::Deny => "deny",
        PermissionBehavior::Ask => "ask",
    };
    let dest_word = |d: PermissionUpdateDestination| match d {
        PermissionUpdateDestination::UserSettings => "user settings",
        PermissionUpdateDestination::ProjectSettings => "project settings",
        PermissionUpdateDestination::LocalSettings => "local settings",
        PermissionUpdateDestination::Session => "session",
        PermissionUpdateDestination::CliArg => "cli",
    };
    // The source an added/removed rule is tagged with, for its destination file
    // (persist keys off `behavior` + `destination`, not `source`; this is for
    // the live `session_allow_rules` citation on an allow add).
    let dest_source = |d: PermissionUpdateDestination| match d {
        PermissionUpdateDestination::UserSettings => PermissionRuleSource::UserSettings,
        PermissionUpdateDestination::ProjectSettings => PermissionRuleSource::ProjectSettings,
        PermissionUpdateDestination::LocalSettings => PermissionRuleSource::LocalSettings,
        PermissionUpdateDestination::Session => PermissionRuleSource::Session,
        PermissionUpdateDestination::CliArg => PermissionRuleSource::CliArg,
    };
    let refresh = |paths: &permission::PermissionPaths| {
        let fresh = PermissionsSnapshot::load(paths);
        if let Ok(mut slot) = snapshot.lock() {
            *slot = fresh;
        }
    };

    match action {
        PermissionAction::Add {
            rule,
            behavior,
            dest,
        } => {
            let update = PermissionUpdate {
                rule: PermissionRule {
                    value: PermissionRuleValue::from_rule_string(&rule),
                    behavior,
                    source: dest_source(dest),
                },
                destination: dest,
            };
            match persist_permission_update(&update, &paths).await {
                Ok(written) => {
                    // Live this-session effect for allow rules: mirror the
                    // AllowAlways dialog's `session_allow_rules` push so the
                    // rule short-circuits the gate immediately (deny/ask have
                    // no live bucket → effective-next-load).
                    if behavior == PermissionBehavior::Allow {
                        session_allow_rules.lock().await.push(update.rule.clone());
                    }
                    refresh(&paths);
                    let body = if written {
                        format!(
                            "✓ Added {rule} to {} rules ({}).",
                            behavior_word(behavior),
                            dest_word(dest)
                        )
                    } else {
                        format!(
                            "{rule} is already in {} rules ({}).",
                            behavior_word(behavior),
                            dest_word(dest)
                        )
                    };
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body,
                        is_error: false,
                    });
                }
                Err(e) => {
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!("✗ Failed to add permission rule: {e}"),
                        is_error: true,
                    });
                }
            }
        }
        PermissionAction::Remove {
            rule,
            behavior,
            dest,
        } => {
            let update = PermissionUpdate {
                rule: PermissionRule {
                    value: PermissionRuleValue::from_rule_string(&rule),
                    behavior,
                    source: dest_source(dest),
                },
                destination: dest,
            };
            match remove_permission_update(&update, &paths).await {
                Ok(removed) => {
                    // Revoke the live this-session grant too: the Add branch
                    // pushed allow rules into `session_allow_rules` (the gate's
                    // step-1 short-circuit), so removing from disk alone would
                    // leave an added-then-removed rule auto-approving for the
                    // rest of the session. Drop every session allow rule whose
                    // VALUE matches (independent of source, so a grant made via
                    // the AllowAlways dialog is revoked too).
                    if behavior == PermissionBehavior::Allow {
                        let target = update.rule.value.clone();
                        session_allow_rules
                            .lock()
                            .await
                            .retain(|r| r.value != target);
                    }
                    refresh(&paths);
                    let body = if removed {
                        format!(
                            "✓ Removed {rule} from {} rules ({}).",
                            behavior_word(behavior),
                            dest_word(dest)
                        )
                    } else {
                        format!(
                            "{rule} was not found in {} rules ({}).",
                            behavior_word(behavior),
                            dest_word(dest)
                        )
                    };
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body,
                        is_error: false,
                    });
                }
                Err(e) => {
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!("✗ Failed to remove permission rule: {e}"),
                        is_error: true,
                    });
                }
            }
        }
        // `/add-dir <path>`: add a working directory to the destination
        // settings file's `permissions.additionalDirectories` via the shared
        // `permission::persist_workspace_directory` seam (idempotent). Reuses
        // this off-loop effect channel rather than adding a new callback. The
        // shared `PermissionsSnapshot` tracks only allow/ask/deny rules (not
        // directories), so no `refresh(&paths)` is needed here.
        PermissionAction::AddDirectory { path, dest } => {
            match permission::persist_workspace_directory(&path, true, dest, &paths).await {
                Ok(written) => {
                    // LIVE session effect (parity 2.1.207 P1-08): whether or not
                    // the settings WRITE was new, apply the add to the RUNNING
                    // session so file tools and MCP roots honor it without a
                    // reboot. `path` is already absolute + normalized
                    // (`add_dir::resolve_and_validate`), matching
                    // `expand_trusted_dir` on an absolute path. `add_trusted_dir`
                    // is the authoritative jzn-style change-compare: on a REAL
                    // change (not already trusted) we also push the dir into the
                    // live MCP roots source and fan out
                    // `notifications/roots/list_changed` to every connected
                    // server; an already-trusted dir is a no-op with NO
                    // notification. This mirrors claude-code's live
                    // `toolPermissionContext.additionalWorkingDirectories`
                    // update → `notifyMcpRootsListChanged`.
                    if session_cwd.add_trusted_dir(std::path::PathBuf::from(&path)) {
                        mcp_registry.add_root(std::path::PathBuf::from(&path));
                        mcp_registry.notify_roots_list_changed_all().await;
                        // `DirectoryAdded` (2.1.219) fires ONLY on a real
                        // change, alongside the roots notification — an
                        // already-trusted directory is a no-op for both. The
                        // source doubles as the hook matcher query.
                        orch.fire_directory_added(&path, "slash_command").await;
                    }
                    // claude-code 2.1.205 success/already echoes, byte-exact:
                    // `Added ${path} as a working directory and saved to local
                    // settings` (persisted) / `… for this session` (session
                    // scope) / `${path} is already added as a working
                    // directory.`
                    let body = if written {
                        match dest {
                            PermissionUpdateDestination::Session => {
                                format!("Added {path} as a working directory for this session")
                            }
                            PermissionUpdateDestination::LocalSettings => format!(
                                "Added {path} as a working directory and saved to local settings"
                            ),
                            other => format!(
                                "Added {path} as a working directory and saved to {}",
                                dest_word(other)
                            ),
                        }
                    } else {
                        format!("{path} is already added as a working directory.")
                    };
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body,
                        is_error: false,
                    });
                }
                Err(e) => {
                    // claude-code's save-failure variant (the session add is
                    // what failed here, but the message shape is preserved).
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!(
                            "Added {path} as a working directory. Failed to save to local settings: {e}"
                        ),
                        is_error: true,
                    });
                }
            }
        }
        // `/cd <path>`: move the session's working directory (parity 2.1.207
        // `local-jsx` `name:"cd"`). Reuses this off-loop effect channel (like
        // `AddDirectory`) rather than a dedicated app callback. The confirm
        // already happened in the TUI (`cd_confirm_view`); `path` is absolute +
        // validated (`add_dir::resolve_and_validate`). Move the SAME shared
        // `SessionCwd` cell — so every FS/Bash tool + the memory hierarchy
        // observe the new cwd, and the swap's registered on-swap callback clears
        // the cwd-keyed conditional-rules cache (the `<env>`/gitStatus sections
        // recompute each turn: this IS the "loads project configuration from
        // that location" reload) — then emit `tengu_cd_command` and print the
        // byte-exact result message. `change_cwd` PRESERVES the additional
        // working directories (settings dirs + runtime `/add-dir` grants),
        // publishing `[target, ...additional]`: claude-code's `/cd` (`sMs` →
        // `process.chdir; kN(Ct())`) only moves the cwd and never touches
        // `additionalWorkingDirectories`, whose union with the cwd is the
        // file-tool allow-set (`EJ = new Set([cwd, ...additional])`). A plain
        // `swap(target, vec![target])` would instead drop every `/add-dir` grant.
        PermissionAction::ChangeDirectory { path } => {
            let target = std::path::PathBuf::from(&path);
            let previous = session_cwd.snapshot();
            session_cwd.change_cwd(target.clone());
            let transcript_path = match orch.retarget_transcript_for_cwd(&target).await {
                Ok(path) => path,
                Err(error) => {
                    // A cwd move without its transcript is not a successful
                    // session move: resume/indexing from the target directory
                    // could select a partial file and hide the pre-move history.
                    // Restore the cwd + trusted roots atomically before reporting
                    // the rejection. `SessionCwd::swap` is infallible, so this
                    // rollback cannot leave the file tools in a half-moved state.
                    session_cwd.swap(previous.0, previous.1);
                    tracing::warn!(%error, "failed to retarget transcript after /cd; rolled back cwd");
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!("Could not change directory: {error}"),
                        is_error: true,
                    });
                    return;
                }
            };
            if let Some(path) = transcript_path {
                if let Err(error) =
                    crate::background_launch::refresh_current_background_launch_identity(
                        &target, &path,
                    )
                {
                    let body = rollback_cd_after_launch_identity_failure(
                        &orch,
                        &session_cwd,
                        previous,
                        &error.to_string(),
                    )
                    .await;
                    tracing::warn!(
                        %error,
                        "rejected /cd because background launch identity is stale"
                    );
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body,
                        is_error: true,
                    });
                    return;
                }
            }
            command_core::cd::emit_command();
            let _ = turn_tx.send(TurnEvent::SystemNotice {
                body: command_core::cd::result_message(&path),
                is_error: false,
            });
        }
    }
}

/// Run one `/web` [`tui::bottom_pane::WebAction`] to completion: persist a
/// saved secret/settings change (credential store + `~/.lingxi/settings.json`
/// merge) or run the real test search, then report the outcome to the
/// transcript via `TurnEvent::SystemNotice`. Ported faithfully from the
/// deleted iocraft `tui` crate's `pump_save_web_secret` / `pump_save_web_settings`
/// / `pump_test_web_search` (git ref `f4ddad16f`, `tui/src/root.rs`), adapted
/// from the old `Arc<Mutex<AppState>>` pump shape to the new effect-closure
/// shape: `snapshot` is the composition root's shared `/web` config snapshot
/// (read by the sync `ChatWidget::cmd_web` to seed the picker), updated here
/// after a successful save so the NEXT `/web` open reflects it.
async fn run_web_action(
    action: tui::bottom_pane::WebAction,
    key_store: Arc<secret::CredentialManager>,
    http: Arc<dyn platform_api::HttpTransport>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
    snapshot: std::sync::Arc<std::sync::Mutex<tui::web::picker::WebConfigSnapshot>>,
) {
    use tool_web::web_search_config::{WebSearchConfig, WebSearchProvider};
    use tui::bottom_pane::WebAction;
    use tui_core::orchestrator_bridge::TurnEvent;

    let label = tui::web::picker::provider_label;

    match action {
        WebAction::SaveSecret { provider, secret } => {
            let Some(id) = tui::web::persist::web_credential_id(provider) else {
                return;
            };
            match key_store.set_provider_key(id, &secret).await {
                Ok(()) => {
                    let searxng_url = {
                        let mut s = snapshot.lock().unwrap();
                        match provider {
                            WebSearchProvider::Tavily => s.tavily_key = true,
                            WebSearchProvider::Brave => s.brave_key = true,
                            _ => {}
                        }
                        s.active = provider;
                        s.searxng_url.clone()
                    };
                    let cfg = WebSearchConfig {
                        provider,
                        searxng_url,
                    };
                    if let Some(p) = tui::web::persist::web_settings_path() {
                        let _ = tui::web::persist::save_web_settings_to(&p, &cfg);
                    }
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!("✓ Saved {} API key.", label(provider)),
                        is_error: false,
                    });
                }
                Err(e) => {
                    let _ = turn_tx.send(TurnEvent::SystemNotice {
                        body: format!("✗ Failed to save key: {e}"),
                        is_error: true,
                    });
                }
            }
        }
        WebAction::SaveSettings {
            provider,
            searxng_url,
        } => {
            let cfg = WebSearchConfig {
                provider,
                searxng_url: searxng_url.clone(),
            };
            let ok = tui::web::persist::web_settings_path()
                .map(|p| tui::web::persist::save_web_settings_to(&p, &cfg).is_ok())
                .unwrap_or(false);
            if ok {
                let mut s = snapshot.lock().unwrap();
                s.active = provider;
                s.searxng_url = searxng_url;
                drop(s);
                let _ = turn_tx.send(TurnEvent::SystemNotice {
                    body: format!("✓ Web search set to {}.", label(provider)),
                    is_error: false,
                });
            } else {
                let _ = turn_tx.send(TurnEvent::SystemNotice {
                    body: "✗ Failed to save web settings.".to_string(),
                    is_error: true,
                });
            }
        }
        WebAction::TestSearch {
            provider,
            typed_key,
        } => {
            use tool_web::web_search_client::{
                resolve_client_search_provider_with_credentials, run_client_web_search,
                EnvSearchConfig, ResolvedWebCredentials,
            };

            let searxng_url = snapshot.lock().unwrap().searxng_url.clone();
            let cfg = WebSearchConfig {
                provider,
                searxng_url,
            };
            let mut creds = ResolvedWebCredentials::empty();
            match (provider, typed_key) {
                (WebSearchProvider::Tavily, Some(k)) => creds.tavily_key = Some(k),
                (WebSearchProvider::Brave, Some(k)) => creds.brave_key = Some(k),
                _ => {
                    if let Ok(Some(secret)) = key_store.get_provider_key("web:tavily").await {
                        creds.tavily_key = Some(secret.expose_secret().clone());
                    }
                    if let Ok(Some(secret)) = key_store.get_provider_key("web:brave").await {
                        creds.brave_key = Some(secret.expose_secret().clone());
                    }
                }
            }
            let env = EnvSearchConfig::from_env();
            let result = match resolve_client_search_provider_with_credentials(&cfg, &creds, &env) {
                Ok(resolved) => {
                    run_client_web_search(&http, &resolved, "current weather Beijing", &[], &[], 3)
                        .await
                        .map(|hits| (resolved, hits))
                }
                Err(e) => Err(e),
            };
            // Build the BARE message (no ✓/✗ mark) — matches the iocraft
            // oracle's `WebTestSummary.message`. The mark is added only when
            // formatting the notice below (and by the picker's
            // `web_provider_detail_lines` "Last test: {mark} {msg}" line), so
            // storing it bare avoids a doubled mark.
            let (bare, is_error) = match result {
                Ok((resolved, hits)) => {
                    let count = hits.len();
                    // Guard an empty top-hit title so no dangling " · " renders
                    // (oracle parity — DuckDuckGo/SearXNG hits can be titleless).
                    let top = hits.first().map_or(String::new(), |h| {
                        if h.title.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", h.title)
                        }
                    });
                    (format!("{}: {count} results{top}", resolved.label()), false)
                }
                Err(e) => (format!("{} test failed: {e}", label(provider)), true),
            };
            let mark = if is_error { "✗" } else { "✓" };
            {
                let mut s = snapshot.lock().unwrap();
                s.last_test = Some(tui::web::picker::WebTestSummary {
                    provider,
                    ok: !is_error,
                    message: bare.clone(),
                });
            }
            let _ = turn_tx.send(TurnEvent::SystemNotice {
                body: format!("{mark} {bare}"),
                is_error,
            });
        }
    }
}

/// Finish the TUI's raw API-key write after the engine has decided whether
/// the already-built provider route can use it. Keeping this branch separate
/// makes the fail-closed event contract testable without fabricating a
/// process-wide desktop catalog.
fn finish_stored_key_connect(
    provider_id: &str,
    credential_id: &str,
    routable: bool,
    turn_tx: &tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use tui_core::orchestrator_bridge::TurnEvent;

    if routable {
        let _ = turn_tx.send(TurnEvent::ProviderConnected {
            provider_id: provider_id.to_string(),
        });
        let _ = turn_tx.send(TurnEvent::SystemNotice {
            body: format!(
                "✓ Saved {} API key.",
                tui::connect::picker::provider_label(provider_id)
            ),
            is_error: false,
        });
        engine_desktop::spawn_fusion_catalog_refresh();
    } else {
        let _ = turn_tx.send(TurnEvent::SystemNotice {
            body: engine_desktop::fusion_credential_restart_required_message(credential_id),
            is_error: true,
        });
    }
}

/// Run one `/connect` [`tui::bottom_pane::ConnectAction`] to completion:
/// persist an API key, drive the GitHub Copilot device-flow, or drive an
/// OAuth browser sign-in — then report the outcome to the transcript via
/// `TurnEvent::SystemNotice`. Mirrors [`run_web_action`]'s shape: the
/// picker/method/key views return the action synchronously from the
/// blocking ratatui loop; this async tail does the real persistence/network
/// work off the render thread.
pub(crate) async fn run_connect_action(
    action: tui::bottom_pane::ConnectAction,
    key_store: Arc<secret::CredentialManager>,
    oauth: Arc<dyn command_core::OAuthConnectDriver>,
    copilot: Arc<dyn command_core::CopilotConnectDriver>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use tui::bottom_pane::ConnectAction;
    use tui_core::orchestrator_bridge::TurnEvent;

    let label = tui::connect::picker::provider_label;
    let notice = |body: String, is_error: bool| {
        let _ = turn_tx.send(TurnEvent::SystemNotice { body, is_error });
    };
    // On a successful connect, flip the widget's LIVE availability map so the
    // /model picker (gated by it) surfaces this provider's models — and the
    // /connect picker badges it ✓ — mid-session, no restart. Keyed by the same
    // profile_name the launch availability map uses.
    let connected = |provider_id: String| {
        let _ = turn_tx.send(TurnEvent::ProviderConnected { provider_id });
    };

    match action {
        ConnectAction::StoreKey { provider_id, key } => {
            match key_store.set_provider_key(&provider_id, &key).await {
                Ok(()) => {
                    // The TUI key view writes through the raw credential
                    // manager, so it bypasses command-layer wrappers. Publish
                    // the just-written route through the cheap engine seam
                    // before exposing the provider to the UI. The engine
                    // owns the detached full re-probe and generation ordering.
                    let credential_id = if provider_id == "anthropic" {
                        "anthropic-api-key"
                    } else {
                        provider_id.as_str()
                    };
                    let routable =
                        engine_desktop::publish_fusion_catalog_credential(credential_id).await;
                    // A false result means the credential was persisted, but
                    // this process was assembled with an incompatible fixed
                    // auth route. The helper deliberately omits both the
                    // readiness event and the otherwise-useful detached scan.
                    finish_stored_key_connect(&provider_id, credential_id, routable, &turn_tx);
                }
                Err(e) => notice(format!("✗ Failed to store key: {e}"), true),
            }
        }
        ConnectAction::Copilot { provider_id } => {
            notice("Connecting to GitHub Copilot…".to_string(), false);
            match copilot.begin(None).await {
                Ok(step) => {
                    // Best-effort: copy the user code to the clipboard so the
                    // user can paste it straight into the browser tab.
                    tui::copy::copy_to_clipboard_native(&step.user_code);
                    notice(
                        format!(
                            "Enter code {} at {} — waiting for authorization…",
                            step.user_code, step.verification_uri
                        ),
                        false,
                    );
                    match copilot.poll_to_completion(&step).await {
                        Ok(()) => {
                            notice("✓ Connected to GitHub Copilot.".to_string(), false);
                            connected(provider_id);
                        }
                        Err(e) => notice(format!("✗ Copilot authorization failed: {e}"), true),
                    }
                }
                Err(e) => notice(format!("✗ Copilot device flow failed: {e}"), true),
            }
        }
        ConnectAction::OAuth { provider_id } => {
            notice(
                format!(
                    "Opening your browser to sign in to {}…",
                    label(&provider_id)
                ),
                false,
            );
            match oauth.login(&provider_id).await {
                Ok(msg) => {
                    let body = if msg.trim().is_empty() {
                        format!("✓ Signed in to {}.", label(&provider_id))
                    } else {
                        msg
                    };
                    connected(provider_id.clone());
                    // The production OAuth driver publishes the canonical
                    // route and starts the one detached full refresh before
                    // returning. Calling the fanout here as well would race
                    // and duplicate that refresh, so the CLI only publishes
                    // the UI event after login succeeds.
                    notice(body, false);
                }
                // (H-BIN-09) A managed `forceLoginOrgUUID` org pin rejected the
                // sign-in: surface the admin message VERBATIM. No "network error"
                // prefix and no API-key fallback hint — a non-OAuth credential
                // cannot satisfy the org pin either, and the login already rolled
                // back its persisted token.
                Err(command_core::ConnectError::LoginPolicyDenied(message)) => {
                    notice(message, true);
                }
                Err(e) => notice(
                    format!("✗ Sign-in failed: {e}. Try connecting with an API key instead."),
                    true,
                ),
            }
        }
    }
}

/// Merge the desktop engine's push-only workflow feed into the same render
/// channel as turn output. The registry is consulted only when a new task/run
/// first appears, so this is event-triggered seeding rather than periodic
/// polling.
fn forward_desktop_workflow_event(
    turn_tx: &tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
    event: engine_desktop::DesktopWorkflowEvent,
    status_run_id: Option<String>,
) {
    use tui_core::multiagent::{MultiAgentEvent, WorkflowProgressEvent};
    use tui_core::orchestrator_bridge::TurnEvent;

    match event {
        engine_desktop::DesktopWorkflowEvent::Progress {
            task_id,
            run_id,
            progress,
        } => {
            let _ = turn_tx.send(TurnEvent::MultiAgent(MultiAgentEvent::WorkflowProgress(
                WorkflowProgressEvent {
                    task_id,
                    run_id,
                    kind: progress.kind,
                    index: progress.index,
                    title: progress.title,
                    label: progress.label,
                    phase_index: progress.phase_index.map(|value| value as usize),
                    phase_title: progress.phase_title,
                    state: progress.state,
                    queued_at_ms: progress.queued_at_ms,
                    started_at_ms: progress.started_at_ms,
                    tokens: progress.tokens,
                    tool_calls: progress.tool_calls,
                },
            )));
        }
        engine_desktop::DesktopWorkflowEvent::Status { task_id, status } => {
            let _ = turn_tx.send(TurnEvent::MultiAgent(
                MultiAgentEvent::WorkflowStatusChanged {
                    task_id,
                    run_id: status_run_id,
                    status: match status {
                        tasks::TaskStatus::Pending => "pending",
                        tasks::TaskStatus::Running => "running",
                        tasks::TaskStatus::Paused => "paused",
                        tasks::TaskStatus::Completed => "completed",
                        tasks::TaskStatus::Failed => "failed",
                        tasks::TaskStatus::Killed => "killed",
                    }
                    .to_owned(),
                    ended_at_ms: None,
                },
            ));
        }
    }
}

fn spawn_workflow_event_forwarder(
    mut src: tokio::sync::mpsc::UnboundedReceiver<engine_desktop::DesktopWorkflowEvent>,
    registry: Arc<dyn platform_api::task_registry::TaskRegistryHandle>,
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use std::collections::HashMap;
    use tui_core::multiagent::{workflow_row_from_record, MultiAgentEvent};
    use tui_core::orchestrator_bridge::TurnEvent;

    tokio::spawn(async move {
        let mut known_runs: HashMap<String, String> = HashMap::new();
        let mut pending_events: HashMap<String, Vec<engine_desktop::DesktopWorkflowEvent>> =
            HashMap::new();
        while let Some(event) = src.recv().await {
            match event {
                engine_desktop::DesktopWorkflowEvent::Progress {
                    task_id,
                    run_id,
                    progress,
                } => {
                    let current = engine_desktop::DesktopWorkflowEvent::Progress {
                        task_id: task_id.clone(),
                        run_id: run_id.clone(),
                        progress,
                    };
                    if known_runs.get(&task_id) != Some(&run_id) {
                        let mut seeded = false;
                        if let Ok(rows) = registry.list_workflows().await {
                            if let Some(row) = rows.into_iter().find(|row| {
                                row.task_id == task_id
                                    && row.run_id.as_deref() == Some(run_id.as_str())
                            }) {
                                known_runs.insert(task_id.clone(), run_id.clone());
                                let _ = turn_tx.send(TurnEvent::MultiAgent(
                                    MultiAgentEvent::WorkflowUpsert(workflow_row_from_record(row)),
                                ));
                                seeded = true;
                            }
                        }
                        if !seeded {
                            pending_events.entry(task_id).or_default().push(current);
                            continue;
                        }
                        if let Some(events) = pending_events.remove(&task_id) {
                            for event in events {
                                forward_desktop_workflow_event(
                                    &turn_tx,
                                    event,
                                    known_runs.get(&task_id).cloned(),
                                );
                            }
                        }
                    }
                    forward_desktop_workflow_event(&turn_tx, current, None);
                }
                engine_desktop::DesktopWorkflowEvent::Status { task_id, status } => {
                    if known_runs.contains_key(&task_id) {
                        let status_run_id = known_runs.get(&task_id).cloned();
                        forward_desktop_workflow_event(
                            &turn_tx,
                            engine_desktop::DesktopWorkflowEvent::Status { task_id, status },
                            status_run_id,
                        );
                        continue;
                    }
                    let current = engine_desktop::DesktopWorkflowEvent::Status {
                        task_id: task_id.clone(),
                        status,
                    };
                    let mut seeded = false;
                    if let Ok(rows) = registry.list_workflows().await {
                        if let Some(row) = rows.into_iter().find(|row| row.task_id == task_id) {
                            known_runs
                                .insert(task_id.clone(), row.run_id.clone().unwrap_or_default());
                            let _ = turn_tx.send(TurnEvent::MultiAgent(
                                MultiAgentEvent::WorkflowUpsert(workflow_row_from_record(row)),
                            ));
                            seeded = true;
                        }
                    }
                    if !seeded {
                        pending_events.entry(task_id).or_default().push(current);
                        continue;
                    }
                    if let Some(events) = pending_events.remove(&task_id) {
                        for event in events {
                            forward_desktop_workflow_event(
                                &turn_tx,
                                event,
                                known_runs.get(&task_id).cloned(),
                            );
                        }
                    }
                    forward_desktop_workflow_event(
                        &turn_tx,
                        current,
                        known_runs.get(&task_id).cloned(),
                    );
                }
            }
        }
    });
}

/// (M8 cc2.1.198) Interpose the bridge channel: pass every [`tui::events::
/// orchestrator_bridge::TurnEvent`] through unchanged while mirroring the
/// turn lifecycle into the live-session registry record — `TurnStarted` →
/// `busy`, `TurnEnded` → `idle` (binary status `Za`/`Cu`: busy while
/// loading, idle at rest). Mid-turn stream events (text/tool deltas) carry
/// no status change; the waiting → busy edge is owned by the permission
/// forwarder (which observes the dialog's actual resolution). Detached task;
/// ends when the source channel closes.
fn spawn_status_bridge_forwarder(
    mut src: tokio::sync::mpsc::UnboundedReceiver<tui_core::orchestrator_bridge::TurnEvent>,
    reg: Arc<crate::agents_registry::SessionRegistration>,
) -> tokio::sync::mpsc::UnboundedReceiver<tui_core::orchestrator_bridge::TurnEvent> {
    use tui_core::orchestrator_bridge::TurnEvent;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(ev) = src.recv().await {
            match &ev {
                TurnEvent::TurnStarted | TurnEvent::TurnStartedWithCancel(_) => {
                    reg.update_status("busy", None)
                }
                TurnEvent::TurnEnded(_) => reg.update_status("idle", None),
                // The bridge-level PermissionRequest variant (reserved M6-05
                // wiring) also marks waiting when it fires; the live dialog
                // path goes through the permission forwarder below.
                TurnEvent::PermissionRequest { .. } => {
                    reg.update_status("waiting", Some("permission prompt"));
                }
                _ => {}
            }
            if tx.send(ev).is_err() {
                break;
            }
        }
    });
    rx
}

/// (M8 cc2.1.198) Interpose the permission channel: each
/// [`tui_core::permission_bridge::PermissionExchange`] marks the session
/// `waiting` / `"permission prompt"` (the binary's `Pb` reason for an open
/// permission dialog @222989611) and has its one-shot responder wrapped so
/// the RESOLUTION (user answered, or dialog dropped = cancel) flips the
/// status back to `busy` — the turn is running again; `TurnEnded` later
/// settles it to `idle`. The wrapped responder forwards the response (or the
/// drop) to the original gate unchanged.
fn spawn_status_permission_forwarder(
    mut src: tokio::sync::mpsc::Receiver<tui_core::permission_bridge::PermissionExchange>,
    reg: Arc<crate::agents_registry::SessionRegistration>,
) -> tokio::sync::mpsc::Receiver<tui_core::permission_bridge::PermissionExchange> {
    // Same capacity as the gate's channel (init.rs `channel(16)`).
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Some(mut exchange) = src.recv().await {
            reg.update_status("waiting", Some("permission prompt"));
            let (wrapped_tx, wrapped_rx) = tokio::sync::oneshot::channel();
            let original_tx = std::mem::replace(&mut exchange.resp_tx, wrapped_tx);
            let reg = reg.clone();
            tokio::spawn(async move {
                let resolved = wrapped_rx.await;
                reg.update_status("busy", None);
                if let Ok(resp) = resolved {
                    let _ = original_tx.send(resp);
                }
                // Err = the TUI dropped the responder (cancel); dropping
                // `original_tx` here propagates exactly that to the gate.
            });
            if tx.send(exchange).await.is_err() {
                break;
            }
        }
    });
    rx
}

fn spawn_status_ask_user_question_forwarder(
    mut src: tokio::sync::mpsc::Receiver<
        tui_core::ask_user_question_bridge::AskUserQuestionExchange,
    >,
    reg: Arc<crate::agents_registry::SessionRegistration>,
) -> tokio::sync::mpsc::Receiver<tui_core::ask_user_question_bridge::AskUserQuestionExchange> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Some(mut exchange) = src.recv().await {
            reg.update_status("waiting", Some("question prompt"));
            let (wrapped_tx, wrapped_rx) = tokio::sync::oneshot::channel();
            let original_tx = std::mem::replace(&mut exchange.resp_tx, wrapped_tx);
            let reg = reg.clone();
            tokio::spawn(async move {
                let resolved = wrapped_rx.await;
                reg.update_status("busy", None);
                if let Ok(resp) = resolved {
                    let _ = original_tx.send(resp);
                }
            });
            if tx.send(exchange).await.is_err() {
                break;
            }
        }
    });
    rx
}

fn spawn_status_computer_access_forwarder(
    mut src: tokio::sync::mpsc::Receiver<tui_core::computer_access_bridge::ComputerAccessExchange>,
    reg: Arc<crate::agents_registry::SessionRegistration>,
) -> tokio::sync::mpsc::Receiver<tui_core::computer_access_bridge::ComputerAccessExchange> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Some(mut exchange) = src.recv().await {
            reg.update_status("waiting", Some("computer access prompt"));
            let (wrapped_tx, wrapped_rx) = tokio::sync::oneshot::channel();
            let original_tx = std::mem::replace(&mut exchange.resp_tx, wrapped_tx);
            let reg = reg.clone();
            tokio::spawn(async move {
                let resolved = wrapped_rx.await;
                reg.update_status("busy", None);
                if let Ok(resp) = resolved {
                    let _ = original_tx.send(resp);
                }
            });
            if tx.send(exchange).await.is_err() {
                break;
            }
        }
    });
    rx
}

/// Snapshot the orchestrator's MCP/hooks/agents/model listings into a
/// `tui::session::SessionInfo` for the full-page screens (`/mcp`,
/// `/hooks`, `/agents`, `/doctor`, `/model`). Awaited once before the blocking
/// TUI loop starts, mirroring the iocraft screens' capture-at-open contract.
async fn build_session_info(
    orch: &dyn OrchestratorHandle,
    default_model_provenance: platform_api::ModelProvenance,
) -> tui::session::SessionInfo {
    use tui::session::{DoctorInfo, InfoRow, ModelRow, SessionInfo};

    let servers = orch.list_mcp_servers().await;
    let mcp_connected = u32::try_from(
        servers
            .iter()
            .filter(|s| matches!(s.status, platform_api::orchestrator::McpStatus::Connected))
            .count(),
    )
    .unwrap_or(u32::MAX);
    let mcp_configured = u32::try_from(servers.len()).unwrap_or(u32::MAX);
    let mcp = servers
        .into_iter()
        .map(|s| {
            let status = match &s.status {
                platform_api::orchestrator::McpStatus::Connected => "connected".to_string(),
                platform_api::orchestrator::McpStatus::Disconnected => "disconnected".to_string(),
                platform_api::orchestrator::McpStatus::Error(e) => format!("error: {e}"),
            };
            InfoRow::new(s.name, Some(format!("{} · {status}", s.transport)))
        })
        .collect();

    let hooks = orch
        .list_hooks()
        .await
        .into_iter()
        .map(|h| {
            InfoRow::new(
                format!("{} ({})", h.name, h.event),
                Some(format!("{} · {}", h.hook_type, h.source)),
            )
        })
        .collect();

    let agents = orch
        .list_agents()
        .await
        .into_iter()
        .map(|a| {
            let detail = if a.description.is_empty() {
                a.source_group
            } else {
                a.description
            };
            InfoRow::new(a.name, (!detail.is_empty()).then_some(detail))
        })
        .collect();

    let current_snapshot = orch.get_status_snapshot().await;
    let current_model = current_snapshot.model;
    let current_profile = current_snapshot.model_profile;
    // Capture the FULL catalog (every provider's models, incl. aggregators like
    // OpenRouter). The picker no longer trims at capture time — it gates by LIVE
    // provider availability and per-provider curation at OPEN time
    // (`tui::session::connected_model_rows`), so a provider connected mid-session
    // can surface its models without a relaunch. Keeping the whole catalog here
    // is what makes that possible; a launch-time trim would permanently hide a
    // later-connected provider.
    let models = orch
        .list_model_listings()
        .await
        .into_iter()
        .map(|m| {
            let supports_multimodal = m.capabilities.vision || m.capabilities.documents;
            let details = build_model_details(&m);
            let is_current = m.request_model == current_model
                && current_profile
                    .as_deref()
                    .is_none_or(|p| p == m.provider_id);
            ModelRow {
                // Mark the current row by (model AND provider) so a wire id shared
                // across providers (e.g. `gpt-5.5` on both OpenAI and Copilot) only
                // dots the ACTUAL current provider's row. When the current profile
                // is unknown (None — e.g. resolve-by-id after a cross-provider
                // resume), fall back to matching by model id alone.
                is_current,
                // Keep `display` CLEAN — the `· 无思考` non-thinking tag is drawn at
                // picker-render time from `supports_reasoning`, NOT baked in here, so
                // the statusline / welcome identity (which read `display`) stay
                // untagged for a non-thinking current model.
                display: m.display_model,
                request_model: m.request_model,
                profile: (!m.provider_id.is_empty()).then_some(m.provider_id),
                provider_label: m.provider_label,
                provenance: if is_current {
                    default_model_provenance
                } else {
                    platform_api::ModelProvenance::ProviderCatalogTier
                },
                supports_reasoning: m.supports_reasoning,
                supports_multimodal,
                details,
            }
        })
        .collect();

    // Phase 8 (tui-rata command registry): `/skills` + `/memory` snapshots.
    // Small on-disk walks, captured once at launch like the listings above.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let skills = skills_rows(&cwd, &crate::run::lingxi_home_dir());
    // One home lookup for both the `/memory` rows and the `/status` warnings.
    let home_dir = dirs::home_dir();
    let memory = home_dir
        .as_ref()
        .map_or_else(Vec::new, |home| memory_rows(&cwd, home));
    // `/status` oversized-memory-file warnings (`htf()`, 2.1.220 @241152161).
    //
    // The oracle is `fJr(await ZH())` — it filters the ALREADY-LOADED memory
    // set, it does not re-walk the filesystem. So this reads the SAME provider
    // production loads the system-prompt block from
    // (`RealMemoryHierarchyProvider`), which is what makes the warning agree
    // with what is actually in the context: it honors the
    // `LINGXI_DISABLE_LINGXI_MDS` kill-switch (`htf`'s `if(Epe())return[]`),
    // probes the Managed tier (`LLu` admits `User|Project|Local|Managed`),
    // splices each `@import`'d file as its own entry, applies the loader's
    // 4 MiB `MEMORY_FILE_BYTE_LIMIT` skip, and yields the frontmatter- and
    // comment-STRIPPED body the oracle measures (`bn_`'s `content`, not
    // `rawContent`) in claude-code splice order.
    //
    // Two gates this still misses, both because they live in the `DesktopConfig`
    // the composition root already consumed and are not reachable from an
    // `OrchestratorHandle`: `--bare` (which sets `memory_provider: None` without
    // exporting the env kill-switch) and the `lingxiMdExcludes` setting.
    let memory_files = orchestrator::prompt::real_provider().load(&cwd).await;
    // `pJr()` (@230802563) derives the threshold from the model. Resolve the
    // picker ALIAS first (`Ei`, @227933508): `current_model` is the raw session
    // model, and `memory::memory_chars_per_token`'s contract is "pass a
    // concrete model id, not a picker alias" — a bare `opus` otherwise scores
    // as an unknown 200k/4-cpt model (threshold 40k) when it resolves to a
    // natively-1M model (threshold 150k).
    let threshold_model = agent::model_resolution::resolve_user_specified_model(&current_model);
    let active_betas = orch.active_betas().await;
    let large_memory_warnings = orchestrator::prompt::large_memory_warning_rows(
        &memory_files,
        &cwd,
        home_dir.as_deref(),
        &threshold_model,
        &active_betas,
    );

    // Managed `availableModels` allowlist for the `/model` picker filter (parity
    // 2.1.207 H-BIN-08): reads the same policy tier the boot default-model
    // constraint uses. `None` (default install) leaves the picker unfiltered.
    let (model_allowlist, model_overrides) = engine_desktop::managed_model_allowlist().await;

    SessionInfo {
        doctor: DoctorInfo::capture(mcp_configured, mcp_connected),
        mcp,
        hooks,
        agents,
        skills,
        memory,
        models,
        model_provenance: default_model_provenance,
        model_allowlist,
        model_overrides,
        large_memory_warnings,
    }
}

/// Flatten the on-disk skill sections (`skill_api::load_file_skill_sections`:
/// project `.lingxi/skills/` ancestors + user `~/.lingxi/skills/`) into
/// `/skills` rows: name + `"<section> · <description>"` detail.
fn skills_rows(cwd: &std::path::Path, lingxi_home: &std::path::Path) -> Vec<tui::session::InfoRow> {
    skill_api::load_file_skill_sections(cwd, lingxi_home)
        .into_iter()
        .flat_map(|section| {
            let source = section.title;
            section.rows.into_iter().map(move |row| {
                let detail = if row.description.trim().is_empty() {
                    source.clone()
                } else {
                    format!("{source} · {}", row.description)
                };
                tui::session::InfoRow::new(row.name, Some(detail))
            })
        })
        .collect()
}

/// The `/memory` tier rows, mirroring the iocraft `screens::memory::memory_tiers`
/// selector labels (read-only here): the always-offered Project + User tiers
/// (marked `(new)` when the file does not exist yet) plus any other LINGXI.md
/// the `memory::lingxi_md::hierarchy::walk` discovers (project parents).
fn memory_rows(cwd: &std::path::Path, os_home: &std::path::Path) -> Vec<tui::session::InfoRow> {
    use memory::lingxi_md::hierarchy::{user_config_dir, walk, FILE_NAME};
    use tui::session::InfoRow;

    let project_path = cwd.join(FILE_NAME);
    let user_path = user_config_dir(os_home).join(FILE_NAME);
    let suffix = |exists: bool| if exists { "" } else { " (new)" };
    let in_git = cwd.ancestors().any(|dir| dir.join(".git").exists());
    let mut rows = vec![
        InfoRow::new(
            "Project memory",
            Some(format!(
                "{} at ./{FILE_NAME}{}",
                if in_git { "Checked in" } else { "Saved" },
                suffix(project_path.is_file())
            )),
        ),
        InfoRow::new(
            "User memory",
            Some(format!(
                "Saved in {}{}",
                user_path.display(),
                suffix(user_path.is_file())
            )),
        ),
    ];
    for entry in walk(cwd, os_home, None).entries {
        if entry.path == project_path || entry.path == user_path {
            continue;
        }
        rows.push(InfoRow::new(
            entry.path.display().to_string(),
            Some("dynamically loaded".to_string()),
        ));
    }
    rows
}

/// Resolve `(lingxi_home, project_dir)` the settings reader/writer address.
///
/// `lingxi_home = ~/.claude` (the user settings root; `/dev/null` when no home
/// dir, matching `init::resolve_desktop_config`'s degrade); `project_dir =
/// std::env::current_dir()`, read AFTER `cwd::apply_cwd` so it reflects any
/// `--cwd`. These feed `migrations::settings_update::settings_path`.
fn settings_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    let lingxi_home = crate::run::lingxi_home_dir();
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    (lingxi_home, project_dir)
}

#[derive(Debug, Default)]
struct ResolvedStatusLineConfigs {
    main: Option<tui_core::status_line_command::StatusLineConfig>,
    subagent: Option<tui_core::status_line_command::SubagentStatusLineConfig>,
}

/// Read effective `statusLine` and `subagentStatusLine` configurations with
/// source provenance, then freeze the trust/hook-policy decision that must be
/// checked before any process creation.
fn read_status_line_configs_from(
    lingxi_home: &std::path::Path,
    project_dir: &std::path::Path,
    managed_tiers: &[String],
    workspace_trusted: bool,
    flag_settings: Option<&lingxi_core::settings::SettingsJson>,
    source_scope: (bool, bool, bool),
) -> ResolvedStatusLineConfigs {
    use migrations::settings_update::read_settings_map;
    use tui_core::status_line_command::{StatusLineExecutionPolicy, StatusLineSource};

    let mut status_line: Option<(serde_json::Value, StatusLineSource)> = None;
    let mut subagent_status_line: Option<(serde_json::Value, StatusLineSource)> = None;
    let mut disable_all_hooks = false;
    let mut managed_hooks_only = false;
    let file_layers = [
        (
            source_scope.0,
            lingxi_home.join("settings.json"),
            StatusLineSource::User,
        ),
        (
            source_scope.1,
            project_dir.join(branding::DOT_DIR).join("settings.json"),
            StatusLineSource::Project,
        ),
        (
            source_scope.2,
            project_dir
                .join(branding::DOT_DIR)
                .join("settings.local.json"),
            StatusLineSource::Local,
        ),
    ];
    for (enabled, path, source) in file_layers {
        if !enabled {
            continue;
        }
        if let Ok(map) = read_settings_map(&path) {
            if let Some(value) = map.get("statusLine") {
                status_line = Some((value.clone(), source));
            }
            if let Some(value) = map.get("subagentStatusLine") {
                subagent_status_line = Some((value.clone(), source));
            }
            if let Some(value) = map
                .get("disableAllHooks")
                .and_then(serde_json::Value::as_bool)
            {
                disable_all_hooks = value;
            }
            if let Some(value) = map
                .get("allowManagedHooksOnly")
                .and_then(serde_json::Value::as_bool)
            {
                managed_hooks_only = value;
            }
        }
    }
    if let Some(flag) = flag_settings {
        if let Some(value) = flag.status_line.as_ref() {
            status_line = Some((value.clone(), StatusLineSource::Flag));
        }
        if let Some(value) = flag.subagent_status_line.as_ref() {
            subagent_status_line = Some((value.clone(), StatusLineSource::Flag));
        }
        if let Some(value) = flag.disable_all_hooks {
            disable_all_hooks = value;
        }
        if let Some(value) = flag.allow_managed_hooks_only {
            managed_hooks_only = value;
        }
    }
    for raw in managed_tiers {
        let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw)
        else {
            continue;
        };
        if let Some(value) = map.get("statusLine") {
            status_line = Some((value.clone(), StatusLineSource::Managed));
        }
        if let Some(value) = map.get("subagentStatusLine") {
            subagent_status_line = Some((value.clone(), StatusLineSource::Managed));
        }
        if let Some(value) = map
            .get("disableAllHooks")
            .and_then(serde_json::Value::as_bool)
        {
            disable_all_hooks = value;
        }
        if let Some(value) = map
            .get("allowManagedHooksOnly")
            .and_then(serde_json::Value::as_bool)
        {
            managed_hooks_only = value;
        }
    }

    let policy = StatusLineExecutionPolicy {
        workspace_trusted,
        disable_all_hooks,
        managed_hooks_only,
    };
    let main = status_line.and_then(|(value, source)| {
        tui_core::status_line_command::StatusLineConfig::from_settings_value(&value)
            .map(|config| config.with_execution_policy(source, policy))
    });
    let subagent = subagent_status_line.and_then(|(value, source)| {
        tui_core::status_line_command::SubagentStatusLineConfig::from_settings_value(&value)
            .map(|config| config.with_execution_policy(source, policy))
    });
    ResolvedStatusLineConfigs { main, subagent }
}

/// Live wrapper over [`read_status_line_configs_from`].
fn workspace_is_trusted(
    global_config_path: Option<&std::path::Path>,
    project_dir: &std::path::Path,
) -> bool {
    global_config_path.is_some_and(|path| {
        migrations::global_config::check_has_trust_dialog_accepted(path, project_dir)
    })
}

async fn read_status_line_configs(
    flag_settings: Option<&lingxi_core::settings::SettingsJson>,
    source_scope: (bool, bool, bool),
) -> ResolvedStatusLineConfigs {
    let (lingxi_home, project_dir) = settings_dirs();
    let managed_tiers = engine_desktop::settings_watch::managed_settings_raw_tiers().await;
    let global_config_path = migrations::global_config::global_config_path();
    let workspace_trusted = workspace_is_trusted(global_config_path.as_deref(), &project_dir);
    read_status_line_configs_from(
        &lingxi_home,
        &project_dir,
        &managed_tiers,
        workspace_trusted,
        flag_settings,
        source_scope,
    )
}

/// True iff `skipDangerousModePermissionPrompt` is truthy in EITHER the user
/// (`~/.lingxi/settings.json`) OR local (`<cwd>/.lingxi/settings.local.json`)
/// settings — the `hasSkipDangerousModePermissionPrompt` user+local check
/// (claude-code `settings.ts:882-889`; the flag/policy tiers have no Rust
/// substrate). On any read failure the tier degrades to `false`.
pub(crate) fn read_skip_dangerous_prompt() -> bool {
    use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
    let (lingxi_home, project_dir) = settings_dirs();
    [SettingsSource::User, SettingsSource::Local]
        .iter()
        .any(|s| {
            let p = settings_path(*s, &lingxi_home, &project_dir);
            read_settings_map(&p)
                .ok()
                .and_then(|m| {
                    m.get("skipDangerousModePermissionPrompt")
                        .map(migrations::context::js_truthy)
                })
                .unwrap_or(false)
        })
}

/// Persist `skipDangerousModePermissionPrompt = true` to the USER
/// `settings.json` (`onConfirm` → save in claude-code). Best-effort: a write
/// failure warns and is otherwise ignored (TS `updateSettingsForSource` never
/// throws; the migration port follows the same warn-and-continue contract).
pub(crate) fn persist_skip_dangerous_prompt() {
    use migrations::settings_update::{settings_path, update_settings, SettingsSource};
    let (lingxi_home, project_dir) = settings_dirs();
    let path = settings_path(SettingsSource::User, &lingxi_home, &project_dir);
    if let Err(e) = update_settings(
        &path,
        vec![(
            "skipDangerousModePermissionPrompt".into(),
            Some(serde_json::json!(true)),
        )],
    ) {
        tracing::warn!(error = %e, "persist skipDangerousModePermissionPrompt failed (ignored)");
    }
}

/// Run the foreground-only trust and dangerous-bypass setup gates.
///
/// Background dispatch calls this before daemonising and records the approval
/// in its owner-only launch spec. The hidden PTY child must consume that bit
/// rather than mounting setup dialogs on an unattended pseudo-terminal.
pub(crate) async fn startup_preflight(argv: &Argv) -> bool {
    if trust_gate().await == TrustGateOutcome::Decline {
        return false;
    }
    external_includes_gate(argv).await;
    let (bypass_mode, _) = crate::resolve_permission_mode(argv);
    let is_bypass = bypass_mode == permission::PermissionMode::BypassPermissions;
    let skip_set = read_skip_dangerous_prompt();
    if !tui::startup_bypass::should_show_bypass_dialog(is_bypass, skip_set) {
        return true;
    }
    match tui::startup_bypass::mount_bypass_dialog().await {
        Ok(tui::startup_bypass::BypassDialogOutcome::Accept) => {
            persist_skip_dangerous_prompt();
            true
        }
        Ok(tui::startup_bypass::BypassDialogOutcome::Decline) => false,
        Err(e) => {
            eprintln!("lingxi-cli: bypass dialog failed: {e}");
            false
        }
    }
}

async fn external_includes_gate(argv: &Argv) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let Some(config_path) = migrations::global_config::global_config_path() else {
        return;
    };
    if !external_includes_gate_should_prompt(&cwd, &config_path, is_full_tty()) {
        return;
    }
    let (include_user, include_project) =
        crate::init::setting_source_flags(argv.setting_sources.as_deref());
    let excludes = crate::init::load_lingxi_md_excludes(include_user, include_project);
    let excluder = memory::lingxi_md::LingxiMdExcluder::new(&excludes);
    let paths = orchestrator::prompt::memory_block::pending_external_include_paths_with_excluder(
        &cwd,
        Some(&excluder),
    );
    if paths.is_empty() {
        return;
    }

    let approved =
        match tui::startup_external_includes::mount_external_includes_dialog(&paths).await {
            Ok(tui::startup_external_includes::ExternalIncludesDialogOutcome::Accept) => true,
            Ok(tui::startup_external_includes::ExternalIncludesDialogOutcome::Decline) => {
                tracing::info!(event = "tengu_claude_md_external_includes_dialog_declined");
                false
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "external LINGXI.md includes dialog failed; imports remain disabled"
                );
                return;
            }
        };
    if let Err(error) = migrations::global_config::save_lingxi_md_external_includes_decision(
        &config_path,
        &cwd,
        approved,
    ) {
        tracing::warn!(
            error = %error,
            "failed to persist external LINGXI.md includes decision; imports remain disabled"
        );
    }
}

fn external_includes_gate_should_prompt(
    cwd: &std::path::Path,
    config_path: &std::path::Path,
    is_tty: bool,
) -> bool {
    is_tty
        && !migrations::global_config::check_has_lingxi_md_external_includes_approved(
            config_path,
            cwd,
        )
        && !migrations::global_config::check_has_lingxi_md_external_includes_warning_shown(
            config_path,
            cwd,
        )
}

/// Outcome of the startup trust gate ([`trust_gate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustGateOutcome {
    /// Already trusted (incl. parent-walk), no config path, or the user
    /// accepted the dialog ⇒ continue into the session (trusted).
    Proceed,
    /// The user declined (or pressed Esc) ⇒ exit 1.
    Decline,
}

/// Whether the un-trusted cwd should mount the trust dialog, and the resolved
/// `(cwd, config_path)` to mount + persist against. `None` ⇒ proceed with NO
/// dialog (already trusted via parent-walk, OR no global config path to
/// consult/persist — degrade exactly as today's hardcoded `Trusted`).
///
/// Pure decision (no terminal I/O): factored out of [`trust_gate`] so the
/// gate's branch logic is unit-testable without a TTY (the mount itself, like
/// `run_tui_session`, can't be driven headless). Mirrors
/// `TrustDialog.tsx:199-202` — `if (hasTrustDialogAccepted) { onDone() }` skips
/// the dialog.
fn trust_gate_should_prompt(cwd: &std::path::Path, config_path: Option<&std::path::Path>) -> bool {
    match config_path {
        // No config path to consult/persist ⇒ proceed without prompting
        // (no-home degrade, same as `init::resolve_desktop_config`).
        None => false,
        // Already trusted (cwd or any ancestor) ⇒ proceed without prompting.
        Some(p) => !migrations::global_config::check_has_trust_dialog_accepted(p, cwd),
    }
}

/// Startup project-trust GATE (design doc §3; parity
/// `components/TrustDialog/TrustDialog.tsx` + `showSetupScreens`).
///
/// Resolves the cwd + the global config path, and:
/// - already-trusted (parent-walk) / no config path ⇒ [`TrustGateOutcome::Proceed`]
///   with NO dialog (byte-identical to today's hardcoded `Trusted`);
/// - otherwise mounts the TTY trust dialog; Accept ⇒ persist
///   `hasTrustDialogAccepted` (best-effort) + `Proceed`; Decline/Esc ⇒
///   [`TrustGateOutcome::Decline`] (exit 1); a mount I/O error ⇒ `Decline`
///   (fail-closed — never silently proceed on a broken prompt).
async fn trust_gate() -> TrustGateOutcome {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let config_path = migrations::global_config::global_config_path();
    if !trust_gate_should_prompt(&cwd, config_path.as_deref()) {
        return TrustGateOutcome::Proceed;
    }
    // `trust_gate_should_prompt` only returns true with `Some(config_path)`.
    let config_path = config_path.expect("prompt implies a config path");
    match tui::startup_trust::mount_trust_dialog(&cwd).await {
        Ok(tui::startup_trust::TrustDialogOutcome::Accept) => {
            // "Yes, I trust this folder" → record acceptance via the shared
            // accept branch (`TrustDialog.tsx:162,174-177`): SESSION-ONLY
            // in-memory when `cwd == $HOME`, else persisted to disk best-effort.
            migrations::global_config::record_trust_accept(&config_path, &cwd);
            TrustGateOutcome::Proceed
        }
        Ok(tui::startup_trust::TrustDialogOutcome::Decline) => TrustGateOutcome::Decline,
        Err(e) => {
            eprintln!("lingxi-cli: trust dialog failed: {e}");
            TrustGateOutcome::Decline
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FUSION_CONNECT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn argv(prompt: Option<&str>, no_tui: bool) -> Argv {
        Argv {
            prompt: prompt.map(String::from),
            no_tui,
            ..Argv::default()
        }
    }

    // --- §22: the `/reload-plugins` error-tally routing ---------------------

    /// §22: a `/reload-plugins` result carrying load errors must route the
    /// reader to `/plugin`, matching the oracle's `N error(s) during load.
    /// Run /plugin for details.` — NOT `/doctor`, which shows no plugin-load
    /// detail.
    #[test]
    fn reload_summary_routes_load_errors_to_plugin_not_doctor() {
        let counts = engine_desktop::PluginRefreshCounts {
            enabled: 2,
            commands: 3,
            agents: 1,
            hooks: 0,
            mcp: 1,
            lsp: 0,
            errors: 2,
        };
        let body = reload_summary_body(&counts);
        assert_eq!(
            body,
            "Reloaded: 2 plugins \u{b7} 3 skills \u{b7} 1 agent \u{b7} 0 hooks \u{b7} 1 plugin \
             MCP server \u{b7} 0 plugin LSP servers\n2 errors during load. Run /plugin for \
             details.",
            "got: {body}"
        );
        assert!(!body.contains("/doctor"));
    }

    #[test]
    fn reload_summary_omits_the_error_line_when_nothing_failed() {
        let counts = engine_desktop::PluginRefreshCounts {
            enabled: 1,
            ..Default::default()
        };
        let body = reload_summary_body(&counts);
        assert!(!body.contains("during load"));
        assert!(!body.contains("/plugin for details"));
    }

    #[test]
    fn prompt_routes_to_print() {
        let a = argv(Some("fix it"), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Print("fix it".into()));
    }

    #[test]
    fn cd_rollback_failure_marks_background_job_terminal() {
        use crate::agents_registry::{self, JobStateWrite};
        use std::sync::Mutex;

        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let home = tempfile::tempdir().expect("config home");
        let short = "deadbeef";
        let session_id = "11111111-2222-3333-4444-555555555555";
        let flags = Vec::new();
        agents_registry::write_job_state(
            home.path(),
            short,
            &JobStateWrite {
                state: "working",
                tempo: Some("active"),
                name: None,
                session_id: Some(session_id),
                cwd: Some("/tmp/old"),
                origin_cwd: Some("/tmp/old"),
                created_at: Some("2026-07-04T00:00:00.000Z"),
                intent: Some("resume"),
                display_intent: None,
                template: Some("bg"),
                respawn_flags: &flags,
                in_flight: None,
                backend: Some("daemon"),
                initial_prompt: None,
                detail: None,
                worker_pid: None,
                worker_proc_start: None,
                phase: Some("running"),
                worker_generation: Some("gen-1"),
                claim_token: None,
                claim_owner: None,
                claim_created_at: None,
                claim_lease_ms: None,
            },
        )
        .expect("seed background job");

        let job_dir = agents_registry::jobs_dir(home.path()).join(short);
        let prior = std::env::var_os("LINGXI_JOB_DIR");
        std::env::set_var("LINGXI_JOB_DIR", &job_dir);
        fail_current_background_job_after_cd_failure("rollback failed");
        match prior {
            Some(value) => std::env::set_var("LINGXI_JOB_DIR", value),
            None => std::env::remove_var("LINGXI_JOB_DIR"),
        }

        let job = agents_registry::read_job(home.path(), short).expect("updated job");
        assert_eq!(job.state, "failed");
        assert!(agents_registry::job_is_terminal(&job));
        assert_eq!(job.detail.as_deref(), Some("rollback failed"));
    }

    /// `Ad(e)` (2.1.220 binary offset 226626865):
    /// `let{relativePath:t}=y0h(e); if(t&&!t.startsWith(".."))return t;
    ///  let r=homedir(); if(e.startsWith(r+sep))return "~"+e.slice(r.length);
    ///  return e`.
    #[test]
    fn shorten_memory_path_prefers_cwd_relative_then_tilde_then_absolute() {
        let cwd = std::path::Path::new("/work/repo");
        let home = Some(std::path::Path::new("/home/u"));
        // Under cwd → cwd-relative, no leading "./".
        assert_eq!(
            orchestrator::prompt::shorten_memory_path(
                std::path::Path::new("/work/repo/LINGXI.md"),
                cwd,
                home
            ),
            "LINGXI.md"
        );
        assert_eq!(
            orchestrator::prompt::shorten_memory_path(
                std::path::Path::new("/work/repo/a/b/LINGXI.md"),
                cwd,
                home
            ),
            "a/b/LINGXI.md"
        );
        // Escapes cwd (relative would start with "..") but lives under $HOME →
        // "~"-abbreviated.
        assert_eq!(
            orchestrator::prompt::shorten_memory_path(
                std::path::Path::new("/home/u/.lingxi/LINGXI.md"),
                cwd,
                home
            ),
            "~/.lingxi/LINGXI.md"
        );
        // Neither → the absolute path, verbatim.
        assert_eq!(
            orchestrator::prompt::shorten_memory_path(
                std::path::Path::new("/etc/lingxi/LINGXI.md"),
                cwd,
                home
            ),
            "/etc/lingxi/LINGXI.md"
        );
        // `$HOME` itself is not `$HOME + sep`, so it does NOT abbreviate.
        assert_eq!(
            orchestrator::prompt::shorten_memory_path(std::path::Path::new("/home/u"), cwd, home),
            "/home/u"
        );
    }

    /// Build a loaded-memory-file fixture with a body of exactly `chars` UTF-16
    /// code units, as the provider would hand it to `large_memory_warning_rows`.
    fn loaded_memory_file(path: &str, chars: usize) -> orchestrator::prompt::MemoryFile {
        orchestrator::prompt::MemoryFile {
            path: std::path::PathBuf::from(path),
            body: "x".repeat(chars),
            is_local_override: false,
            tier: memory::lingxi_md::LingxiMdTier::Project,
            globs: None,
            raw_content: String::new(),
            content_differs_from_disk: false,
        }
    }

    /// `htf()` (@241152161) over `fJr(memoryFiles)` (@230809207): one row per
    /// LOADED memory file whose UTF-16 length exceeds `pJr()` (@230802563).
    ///
    /// Hermetic by construction — the fixture IS the loaded set, so the test
    /// touches neither the filesystem nor `$LINGXI_CONFIG_DIR` /
    /// `$LINGXI_MANAGED_DIR`, which a real hierarchy walk would read.
    #[test]
    fn large_memory_warning_rows_are_byte_locked_and_model_derived() {
        let cwd = std::path::Path::new("/work/repo");
        let home = Some(std::path::Path::new("/home/u"));
        // 52_310 chars — the worked example from the byte-lock test in `memory`.
        let files = vec![loaded_memory_file("/work/repo/LINGXI.md", 52_310)];

        // A 200k-context model: threshold collapses to the 40k floor → flagged.
        let rows = orchestrator::prompt::large_memory_warning_rows(
            &files,
            cwd,
            home,
            "claude-sonnet-4-5",
            &[],
        );
        assert_eq!(
            rows,
            vec!["Large LINGXI.md will impact performance (52.3k chars > 40.0k)".to_string()],
        );

        // The SAME file under a 1M-context model: `max(40000, round(1e6*0.05*3))`
        // = 150_000 > 52_310 → NOT flagged. This is the only regime in which the
        // model-derived threshold differs from the old flat 40k constant.
        assert!(orchestrator::prompt::large_memory_warning_rows(
            &files,
            cwd,
            home,
            "claude-opus-5[1m]",
            &[]
        )
        .is_empty());
        assert!(orchestrator::prompt::large_memory_warning_rows(
            &files,
            cwd,
            home,
            "claude-sonnet-4-5",
            &["context-1m-2025-08-07".to_string()],
        )
        .is_empty());

        // A file exactly AT the threshold is not flagged (`>`), one char over is.
        let at = vec![loaded_memory_file("/work/repo/LINGXI.md", 40_000)];
        assert!(orchestrator::prompt::large_memory_warning_rows(
            &at,
            cwd,
            home,
            "claude-sonnet-4-5",
            &[]
        )
        .is_empty());
        let over = vec![loaded_memory_file("/work/repo/LINGXI.md", 40_001)];
        assert_eq!(
            orchestrator::prompt::large_memory_warning_rows(
                &over,
                cwd,
                home,
                "claude-sonnet-4-5",
                &[]
            )
            .len(),
            1
        );

        // Rows follow the provider's order (claude-code splice order:
        // Managed → User → parents → cwd), one per oversized file.
        let many = vec![
            loaded_memory_file("/home/u/.lingxi/LINGXI.md", 52_310),
            loaded_memory_file("/work/repo/LINGXI.md", 52_310),
        ];
        assert_eq!(
            orchestrator::prompt::large_memory_warning_rows(
                &many,
                cwd,
                home,
                "claude-sonnet-4-5",
                &[]
            ),
            vec![
                "Large ~/.lingxi/LINGXI.md will impact performance (52.3k chars > 40.0k)"
                    .to_string(),
                "Large LINGXI.md will impact performance (52.3k chars > 40.0k)".to_string(),
            ],
        );
    }

    /// The threshold model must be the RESOLVED wire id, not the raw picker
    /// alias: `lingxi --model opus` leaves `StatusSnapshot.model` as the literal
    /// `"opus"`, which scores as an unknown 200k/4-cpt model (40k threshold)
    /// though it resolves to a natively-1M model (150k threshold).
    #[test]
    fn large_memory_warning_threshold_uses_the_resolved_model_not_the_alias() {
        let cwd = std::path::Path::new("/work/repo");
        let home = Some(std::path::Path::new("/home/u"));
        let files = vec![loaded_memory_file("/work/repo/LINGXI.md", 52_310)];

        // The bare alias would flag the file …
        assert_eq!(
            orchestrator::prompt::large_memory_warning_rows(&files, cwd, home, "opus", &[]).len(),
            1,
            "the unresolved alias must be the case that misfires"
        );
        // … while the id `Ei` resolves it to does not.
        let resolved = agent::model_resolution::resolve_user_specified_model("opus");
        assert!(
            orchestrator::prompt::large_memory_warning_rows(&files, cwd, home, &resolved, &[])
                .is_empty(),
            "resolved {resolved} should be a 1M model with a 150k threshold"
        );
    }

    /// Round-5 review finding [15]: the TUI `/connect` key seam
    /// (`ChatWidget::cmd_connect` -> `ConnectKeyView` ->
    /// `ConnectAction::StoreKey` -> here) writes through the RAW
    /// `secret::CredentialManager` the TUI runtime carries, NOT through the
    /// `FusionCatalogRefreshingCredentialWriter` wrapper that the text
    /// `/connect <provider>` command goes through. Before this fix the write
    /// therefore never reached Fusion's catalog filter: `/model` and the
    /// ordinary turn loop routed the new provider on the very next request
    /// while `/fusion` kept filtering against the BOOT availability snapshot
    /// and dropped every one of its rows for the rest of the process.
    #[tokio::test]
    async fn store_key_connect_action_makes_the_provider_visible_to_fusion() {
        use async_trait::async_trait;
        use platform_api::{
            Clock, HttpTransport, SecureStorage, SecureStorageBackend, SecureStorageError,
        };
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex as StdMutex};
        use tui::bottom_pane::ConnectAction;

        let _registry_guard = FUSION_CONNECT_TEST_LOCK.lock().await;

        #[derive(Default)]
        struct MemStorage {
            map: StdMutex<HashMap<(String, String), protocol::SecureStorageData>>,
        }
        #[async_trait]
        impl SecureStorage for MemStorage {
            async fn store(
                &self,
                service: &str,
                account: &str,
                data: protocol::SecureStorageData,
            ) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .insert((service.into(), account.into()), data);
                Ok(())
            }
            async fn retrieve(
                &self,
                service: &str,
                account: &str,
            ) -> Result<Option<protocol::SecureStorageData>, SecureStorageError> {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .get(&(service.into(), account.into()))
                    .cloned())
            }
            async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .remove(&(service.into(), account.into()));
                Ok(())
            }
            async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .keys()
                    .filter(|(s, _)| s == service)
                    .map(|(_, a)| a.clone())
                    .collect())
            }
            fn is_encrypted(&self) -> bool {
                false
            }
            fn backend(&self) -> SecureStorageBackend {
                SecureStorageBackend::PlainText
            }
        }

        struct UnusedOAuth;
        #[async_trait]
        impl command_core::OAuthConnectDriver for UnusedOAuth {
            async fn login(
                &self,
                _provider_id: &str,
            ) -> Result<String, command_core::ConnectError> {
                unreachable!("the StoreKey arm must not touch the OAuth driver")
            }
        }
        struct UnusedCopilot;
        #[async_trait]
        impl command_core::CopilotConnectDriver for UnusedCopilot {
            async fn begin(
                &self,
                _domain: Option<&str>,
            ) -> Result<command_core::CopilotConnectStep, command_core::ConnectError> {
                unreachable!("the StoreKey arm must not touch the Copilot driver")
            }
            async fn poll_to_completion(
                &self,
                _step: &command_core::CopilotConnectStep,
            ) -> Result<(), command_core::ConnectError> {
                unreachable!("the StoreKey arm must not touch the Copilot driver")
            }
        }

        let storage: Arc<dyn SecureStorage> = Arc::new(MemStorage::default());
        let clock: Arc<dyn Clock> = Arc::new(platform_posix::PosixClock::new());
        let http: Arc<dyn HttpTransport> = Arc::new(platform_posix::PosixHttp::new());
        let credentials = Arc::new(secret::CredentialManager::new(storage, clock, http));

        // The live Fusion catalog filter's shared availability map, as
        // `resolve_llm_stack` publishes it at boot: openrouter uncredentialed.
        let mut boot = std::collections::BTreeMap::new();
        boot.insert("openrouter".to_string(), false);
        let availability = Arc::new(std::sync::RwLock::new(boot));
        engine_desktop::register_fusion_catalog_refresher(
            engine_desktop::FusionCatalogRefresher::for_keychain_profiles(
                availability.clone(),
                credentials.clone(),
                &["openrouter"],
            ),
        );

        let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel();
        run_connect_action(
            ConnectAction::StoreKey {
                provider_id: "openrouter".to_string(),
                key: "sk-or-test-789".to_string(),
            },
            credentials.clone(),
            Arc::new(UnusedOAuth),
            Arc::new(UnusedCopilot),
            turn_tx,
        )
        .await;

        // Sanity: the write itself succeeded (this is the seam the TUI uses).
        let stored = credentials
            .get_provider_key("openrouter")
            .await
            .expect("read ok")
            .expect("key present");
        assert_eq!(stored.expose_secret(), "sk-or-test-789");
        let mut events = Vec::new();
        while let Ok(ev) = turn_rx.try_recv() {
            events.push(ev);
        }
        assert!(
            events.iter().any(|ev| matches!(
                ev,
                tui_core::orchestrator_bridge::TurnEvent::ProviderConnected { provider_id }
                    if provider_id == "openrouter"
            )),
            "sanity: the TUI's own availability map is already refreshed on this path, \
which is exactly why Fusion falling behind is observable to the user"
        );

        assert_eq!(
            availability.read().unwrap().get("openrouter").copied(),
            Some(true),
            "the TUI /connect key seam must make the provider visible to Fusion's catalog \
filter in THIS process — /model and the turn loop already route it"
        );
    }

    /// Round-12 review finding [5]: `connected(provider_id)` — the
    /// `TurnEvent::ProviderConnected` that flips the widget's LIVE
    /// availability map so `/model` offers the just-connected provider's
    /// models and the `/connect` picker badges it ✓ mid-session — consumes
    /// NOTHING the Fusion catalog re-probe produces, yet BOTH credential arms
    /// sequenced it AFTER `refresh_fusion_catalog_after_credential_write()`.
    /// That refresh re-runs `compute_availability_with_isolation` over every
    /// credential source and bottoms out in `has_provider_key`, i.e. the (on
    /// macOS, brokered) keychain, with no internal timeout — the very probe
    /// `resolve_llm_stack` wraps in a 5s `tokio::time::timeout` at boot
    /// because a contended broker stalls it. So the user saw
    /// "✓ Saved … API key." and then the /model picker and the /connect badge
    /// lagged for as long as the broker took to answer, once PER credential
    /// source, serially.
    ///
    /// The fix is a cheap publication plus detached reconciliation: the live
    /// provider event is announced immediately, while Fusion's catalog still
    /// receives one full broker re-probe in the background (cancelling that
    /// refresh would resurrect round-5 finding [15]).
    #[tokio::test]
    async fn connect_action_announces_the_provider_before_the_catalog_re_probe() {
        use async_trait::async_trait;
        use platform_api::{
            Clock, HttpTransport, SecureStorage, SecureStorageBackend, SecureStorageError,
        };
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex as StdMutex};
        use std::time::Duration;
        use tui::bottom_pane::ConnectAction;
        use tui_core::orchestrator_bridge::TurnEvent;

        let _registry_guard = FUSION_CONNECT_TEST_LOCK.lock().await;
        const PROVIDER: &str = "stalled-broker-provider";

        /// A `SecureStorage` that models a contended credential broker:
        /// writes go through, but every READ parks until the test releases
        /// it. `armed` flips on the first successful write so the arm's own
        /// `set_provider_key` is never the thing that stalls.
        #[derive(Default)]
        struct StallingStorage {
            map: StdMutex<HashMap<(String, String), protocol::SecureStorageData>>,
            armed: AtomicBool,
            released: AtomicBool,
            reads_while_stalled: AtomicUsize,
        }
        impl StallingStorage {
            async fn park_if_stalled(&self) {
                if !self.armed.load(Ordering::SeqCst) || self.released.load(Ordering::SeqCst) {
                    return;
                }
                self.reads_while_stalled.fetch_add(1, Ordering::SeqCst);
                // Safety valve: never park longer than the test could
                // possibly need, so a failing assertion cannot wedge the
                // process-wide refresher registry for sibling tests.
                for _ in 0..400 {
                    if self.released.load(Ordering::SeqCst) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
            fn rearm(&self) {
                self.released.store(false, Ordering::SeqCst);
                self.reads_while_stalled.store(0, Ordering::SeqCst);
            }
            fn release(&self) {
                self.released.store(true, Ordering::SeqCst);
            }
        }
        #[async_trait]
        impl SecureStorage for StallingStorage {
            async fn store(
                &self,
                service: &str,
                account: &str,
                data: protocol::SecureStorageData,
            ) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .insert((service.into(), account.into()), data);
                self.armed.store(true, Ordering::SeqCst);
                Ok(())
            }
            async fn retrieve(
                &self,
                service: &str,
                account: &str,
            ) -> Result<Option<protocol::SecureStorageData>, SecureStorageError> {
                self.park_if_stalled().await;
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .get(&(service.into(), account.into()))
                    .cloned())
            }
            async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError> {
                self.map
                    .lock()
                    .unwrap()
                    .remove(&(service.into(), account.into()));
                Ok(())
            }
            async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError> {
                self.park_if_stalled().await;
                Ok(self
                    .map
                    .lock()
                    .unwrap()
                    .keys()
                    .filter(|(s, _)| s == service)
                    .map(|(_, a)| a.clone())
                    .collect())
            }
            fn is_encrypted(&self) -> bool {
                false
            }
            fn backend(&self) -> SecureStorageBackend {
                SecureStorageBackend::PlainText
            }
        }

        struct OkOAuth;
        #[async_trait]
        impl command_core::OAuthConnectDriver for OkOAuth {
            async fn login(
                &self,
                _provider_id: &str,
            ) -> Result<String, command_core::ConnectError> {
                Ok(String::new())
            }
        }
        struct UnusedCopilot;
        #[async_trait]
        impl command_core::CopilotConnectDriver for UnusedCopilot {
            async fn begin(
                &self,
                _domain: Option<&str>,
            ) -> Result<command_core::CopilotConnectStep, command_core::ConnectError> {
                unreachable!("neither arm under test touches the Copilot driver")
            }
            async fn poll_to_completion(
                &self,
                _step: &command_core::CopilotConnectStep,
            ) -> Result<(), command_core::ConnectError> {
                unreachable!("neither arm under test touches the Copilot driver")
            }
        }

        /// First `ProviderConnected` on the channel, or `None` if two seconds
        /// pass without one.
        async fn provider_connected_within_2s(
            rx: &mut tokio::sync::mpsc::UnboundedReceiver<TurnEvent>,
        ) -> Option<String> {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    match rx.recv().await {
                        Some(TurnEvent::ProviderConnected { provider_id }) => {
                            return Some(provider_id)
                        }
                        Some(_) => continue,
                        None => return None,
                    }
                }
            })
            .await
            .ok()
            .flatten()
        }

        /// Spin (bounded) until the detached re-probe has actually reached
        /// the credential backend, so the readiness assertion above cannot
        /// pass vacuously on a refresh that never ran.
        async fn await_parked_reprobe(storage: &StallingStorage) -> usize {
            for _ in 0..200 {
                let reads = storage.reads_while_stalled.load(Ordering::SeqCst);
                if reads > 0 {
                    return reads;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            0
        }

        let storage = Arc::new(StallingStorage::default());
        let clock: Arc<dyn Clock> = Arc::new(platform_posix::PosixClock::new());
        let http: Arc<dyn HttpTransport> = Arc::new(platform_posix::PosixHttp::new());
        let credentials = Arc::new(secret::CredentialManager::new(
            storage.clone() as Arc<dyn SecureStorage>,
            clock,
            http,
        ));
        let mut boot = std::collections::BTreeMap::new();
        boot.insert(PROVIDER.to_string(), false);
        let availability = Arc::new(std::sync::RwLock::new(boot));
        engine_desktop::register_fusion_catalog_refresher(
            engine_desktop::FusionCatalogRefresher::for_keychain_profiles(
                availability.clone(),
                credentials.clone(),
                &[PROVIDER],
            ),
        );

        // --- arm 1: ConnectAction::StoreKey -------------------------------
        let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut store_key = tokio::spawn(run_connect_action(
            ConnectAction::StoreKey {
                provider_id: PROVIDER.to_string(),
                key: "sk-stalled-broker".to_string(),
            },
            credentials.clone(),
            Arc::new(OkOAuth),
            Arc::new(UnusedCopilot),
            turn_tx.clone(),
        ));
        assert_eq!(
            provider_connected_within_2s(&mut turn_rx).await.as_deref(),
            Some(PROVIDER),
            "StoreKey arm: TurnEvent::ProviderConnected must be sent while the Fusion \
catalog re-probe is still parked in the credential backend — it consumes nothing the \
re-probe produces, so gating it behind an unbounded keychain read leaves the /model \
picker and the /connect badge stale for as long as the broker stalls"
        );
        assert_eq!(
            availability.read().unwrap().get(PROVIDER).copied(),
            Some(true),
            "StoreKey arm: cheap catalog publication must make the provider available \
before ProviderConnected is announced, while the full credential re-probe is still parked"
        );
        assert!(
            await_parked_reprobe(&storage).await >= 1,
            "StoreKey arm: the re-probe must still have run (and be parked) — a green \
assertion above with zero backend reads would mean the refresh was dropped, which is \
round-5 finding [15] all over again"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), &mut store_key)
                .await
                .is_ok(),
            "StoreKey arm: the action must finish without waiting for the parked \
credential re-probe"
        );
        storage.release();
        assert_eq!(
            availability.read().unwrap().get(PROVIDER).copied(),
            Some(true),
            "StoreKey arm: once the broker answers, the re-probe still publishes the \
provider to Fusion's catalog filter"
        );

        // --- arm 2: ConnectAction::OAuth ----------------------------------
        tokio::time::sleep(Duration::from_millis(100)).await;
        storage.rearm();
        // A raw OAuth double is intentional here: production's wrapped OAuth
        // driver owns the cheap publication + detached refresh. The CLI must
        // not start a second refresh when that wrapper returns.
        let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel();
        let oauth = tokio::spawn(run_connect_action(
            ConnectAction::OAuth {
                provider_id: PROVIDER.to_string(),
            },
            credentials.clone(),
            Arc::new(OkOAuth),
            Arc::new(UnusedCopilot),
            turn_tx.clone(),
        ));
        assert_eq!(
            provider_connected_within_2s(&mut turn_rx).await.as_deref(),
            Some(PROVIDER),
            "OAuth arm: same class as StoreKey — ProviderConnected must not be gated \
behind the unbounded keychain re-probe"
        );
        oauth.await.expect("OAuth arm ran to completion");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            storage.reads_while_stalled.load(Ordering::SeqCst),
            0,
            "OAuth arm: the CLI must not duplicate the production wrapper's \
detached catalog refresh"
        );

        // --- arm 3: stored key requires a route-rebuilding restart --------
        const COLD_PROVIDER: &str = "cold-fixed-route";
        credentials
            .set_provider_key(COLD_PROVIDER, "sk-saved-needs-restart")
            .await
            .expect("credential write succeeds before route compatibility is reported");
        let (turn_tx, mut turn_rx) = tokio::sync::mpsc::unbounded_channel();
        // Route compatibility itself is an engine concern (covered by its
        // fixed-route regressions). Drive the CLI's false-result branch
        // directly: an unknown provider would be neutral, not incompatible.
        finish_stored_key_connect(COLD_PROVIDER, COLD_PROVIDER, false, &turn_tx);
        let mut restart_notice = None;
        while let Ok(event) = turn_rx.try_recv() {
            match event {
                TurnEvent::ProviderConnected { provider_id } if provider_id == COLD_PROVIDER => {
                    panic!("an unsupported hot route must not advertise ProviderConnected")
                }
                TurnEvent::SystemNotice {
                    body,
                    is_error: true,
                } => restart_notice = Some(body),
                TurnEvent::SystemNotice {
                    body,
                    is_error: false,
                } => panic!(
                    "an unsupported hot route must not report a ready/success notice: {body}"
                ),
                _ => {}
            }
        }
        let restart_notice = restart_notice.expect("saved credential reports restart required");
        assert!(restart_notice.contains("was saved"), "{restart_notice}");
        assert!(
            restart_notice.contains("Restart LingXi"),
            "{restart_notice}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            storage.reads_while_stalled.load(Ordering::SeqCst),
            0,
            "a route that cannot be adopted hot must not start a useless full re-probe"
        );
    }

    /// `/sandbox` persistence: `SetEnabled` writes `sandbox.enabled` to USER
    /// settings, `Exclude` appends to `sandbox.excludedCommands` in LOCAL
    /// settings (idempotent), and both preserve sibling keys.
    #[tokio::test]
    async fn run_sandbox_action_persists_toggle_and_excludes() {
        use permission::{PermissionPaths, PermissionUpdateDestination};
        use tui::chat_widget::SandboxAction;
        use tui_core::orchestrator_bridge::TurnEvent;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let paths = PermissionPaths {
            lingxi_home: home.clone(),
            cwd: cwd.clone(),
        };
        let user_path = paths
            .destination_path(PermissionUpdateDestination::UserSettings)
            .unwrap();
        let local_path = paths
            .destination_path(PermissionUpdateDestination::LocalSettings)
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        // SetEnabled(true) → user settings.json {"sandbox":{"enabled":true}}.
        run_sandbox_action(SandboxAction::SetEnabled(true), paths.clone(), tx.clone()).await;
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&user_path).unwrap()).unwrap();
        assert_eq!(v["sandbox"]["enabled"], serde_json::json!(true));
        assert!(matches!(
            rx.try_recv(),
            Ok(TurnEvent::SystemNotice {
                is_error: false,
                ..
            })
        ));

        // Exclude appends to local settings; a repeat is idempotent.
        run_sandbox_action(
            SandboxAction::Exclude("npm test:*".into()),
            paths.clone(),
            tx.clone(),
        )
        .await;
        run_sandbox_action(
            SandboxAction::Exclude("npm test:*".into()),
            paths.clone(),
            tx.clone(),
        )
        .await;
        let local: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&local_path).unwrap()).unwrap();
        let arr = local["sandbox"]["excludedCommands"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "idempotent: no duplicate on repeat");
        assert_eq!(arr[0], serde_json::json!("npm test:*"));

        // Toggling back to false rewrites the same user file (sibling-safe).
        run_sandbox_action(SandboxAction::SetEnabled(false), paths.clone(), tx.clone()).await;
        let v2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&user_path).unwrap()).unwrap();
        assert_eq!(v2["sandbox"]["enabled"], serde_json::json!(false));
    }

    #[test]
    fn skills_rows_flatten_sections_and_memory_rows_always_offer_both_tiers() {
        // Hermetic root: <tmp>/proj (cwd, non-git) + <tmp>/home (os home).
        let root = std::env::temp_dir().join(format!("cli-session-info-{}", std::process::id()));
        let proj = root.join("proj");
        let home = root.join("home");
        let skill_dir = proj.join(".lingxi").join("skills").join("brainstorm");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: brainstorm\ndescription: explore ideas\n---\nbody\n",
        )
        .expect("SKILL.md");
        std::fs::create_dir_all(&home).expect("home dir");
        let lingxi_home = home.join(".lingxi");

        let skills = skills_rows(&proj, &lingxi_home);
        assert_eq!(skills.len(), 1, "{skills:?}");
        assert_eq!(skills[0].title, "brainstorm");
        assert!(
            skills[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("explore ideas")),
            "{skills:?}"
        );

        let memory = memory_rows(&proj, &home);
        std::fs::remove_dir_all(&root).ok();
        // Project + User tiers are always offered, marked (new) when absent.
        assert!(memory.len() >= 2, "{memory:?}");
        assert_eq!(memory[0].title, "Project memory");
        assert!(
            memory[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("(new)")),
            "{memory:?}"
        );
        assert_eq!(memory[1].title, "User memory");
    }

    #[test]
    fn empty_prompt_does_not_route_to_print() {
        let a = argv(Some("   "), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_prompt_tty_routes_to_tui() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, true), Mode::Tui);
    }

    #[test]
    fn no_tui_flag_forces_stdio_repl() {
        let a = argv(None, true);
        assert_eq!(decide_mode_with(&a, true), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_routes_to_stdio_repl() {
        let a = argv(None, false);
        assert_eq!(decide_mode_with(&a, false), Mode::StdioRepl);
    }

    #[test]
    fn non_tty_with_prompt_still_prints() {
        // -p with piped input should still print one-shot (matches v0.6.0).
        let a = argv(Some("hi"), false);
        assert_eq!(decide_mode_with(&a, false), Mode::Print("hi".into()));
    }

    #[test]
    fn status_line_configs_share_precedence_and_pre_spawn_policy() {
        use tui_core::status_line_command::StatusLineSource;

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(branding::DOT_DIR)).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"user-main"},"subagentStatusLine":{"type":"command","command":"user-agent"}}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(branding::DOT_DIR).join("settings.local.json"),
            r#"{"subagentStatusLine":{"type":"command","command":"local-agent"}}"#,
        )
        .unwrap();

        let configs =
            read_status_line_configs_from(&home, &project, &[], true, None, (true, true, true));
        assert_eq!(configs.main.unwrap().source, StatusLineSource::User);
        let subagent = configs.subagent.unwrap();
        assert_eq!(subagent.source, StatusLineSource::Local);
        assert_eq!(subagent.command, "local-agent");
        assert!(subagent.should_run(true));

        let managed = vec![
            r#"{"allowManagedHooksOnly":true,"subagentStatusLine":{"type":"command","command":"managed-agent"}}"#.to_string(),
        ];
        let configs = read_status_line_configs_from(
            &home,
            &project,
            &managed,
            true,
            None,
            (true, true, true),
        );
        let subagent = configs.subagent.unwrap();
        assert_eq!(subagent.source, StatusLineSource::Managed);
        assert!(subagent.should_run(true));
        assert!(
            !configs.main.unwrap().should_run(true),
            "managed-hooks-only must reject a user status command before spawn"
        );

        let configs =
            read_status_line_configs_from(&home, &project, &[], false, None, (true, true, true));
        assert!(!configs.subagent.unwrap().should_run(true));

        let flag = lingxi_core::settings::SettingsJson {
            status_line: Some(serde_json::json!({
                "type":"command",
                "command":"flag-main"
            })),
            ..Default::default()
        };
        let configs = read_status_line_configs_from(
            &home,
            &project,
            &[],
            true,
            Some(&flag),
            (false, false, false),
        );
        let main = configs.main.unwrap();
        assert_eq!(main.source, StatusLineSource::Flag);
        assert_eq!(main.command, "flag-main");
        assert!(
            configs.subagent.is_none(),
            "disabled file scopes may not contribute subagentStatusLine"
        );
    }

    #[test]
    fn absent_trust_store_path_is_not_implicitly_trusted() {
        let project = tempfile::tempdir().unwrap();
        assert!(!workspace_is_trusted(None, project.path()));
    }

    #[test]
    fn external_includes_prompt_only_once_per_project_decision() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        assert!(
            external_includes_gate_should_prompt(&cwd, &config_path, true),
            "an interactive project without a recorded decision must prompt"
        );
        assert!(
            !external_includes_gate_should_prompt(&cwd, &config_path, false),
            "headless modes must never consume input for the startup dialog"
        );

        migrations::global_config::save_lingxi_md_external_includes_decision(
            &config_path,
            &cwd,
            false,
        )
        .unwrap();
        assert!(
            !external_includes_gate_should_prompt(&cwd, &config_path, true),
            "declining records warningShown and must not prompt every launch"
        );
    }

    #[test]
    fn external_includes_approval_suppresses_startup_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        migrations::global_config::save_lingxi_md_external_includes_decision(
            &config_path,
            &cwd,
            true,
        )
        .unwrap();
        assert!(
            !external_includes_gate_should_prompt(&cwd, &config_path, true),
            "an approved project must load external imports without re-prompting"
        );
    }

    #[test]
    fn plan_snapshot_renders_path_and_utf8_body() {
        let plan = platform_api::PlanSnapshot {
            path: PathBuf::from("/tmp/session-plan.md"),
            content: "步骤一\n步骤二".to_string(),
        };
        assert_eq!(
            render_plan_snapshot(&plan),
            "Current Plan\n/tmp/session-plan.md\n\n步骤一\n步骤二"
        );
    }

    // ── (Task 3) startup trust gate ───────────────────────────────────────
    //
    // The mount itself (`mount_trust_dialog`) can't be driven headless (no TTY
    // — same caveat documented on `run_tui_session` / `mount_bypass_dialog`),
    // so the gate DECISION is tested here via the store seam
    // (`trust_gate_should_prompt`, against a real `migrations::global_config`
    // temp config). The dialog's pure key→outcome map (Accept/Decline/Esc) is
    // covered by `tui::startup_trust`'s own unit tests; `apps/cli` does not
    // depend on `crossterm`, so the gate's branch logic is exercised here at
    // the predicate seam — mirroring how the bypass gate is covered (pure
    // predicate, thin I/O wrapper excluded).

    use std::path::PathBuf;

    /// An un-trusted cwd ⇒ the gate WOULD mount the dialog
    /// (`trust_gate_should_prompt == true`); persisting (the Accept path)
    /// makes a subsequent `check_` true and the gate no longer re-prompts
    /// (continue path / store marked).
    #[test]
    fn untrusted_cwd_prompts_then_accept_marks_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();

        // Un-trusted ⇒ dialog shown.
        assert!(
            trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "un-trusted cwd must mount the trust dialog"
        );

        // Accept persists ⇒ subsequent check is true (and the gate would NOT
        // re-prompt) — the "continue, store marked" path.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &cwd).unwrap();
        assert!(
            migrations::global_config::check_has_trust_dialog_accepted(&config_path, &cwd),
            "accept must persist trust"
        );
        assert!(
            !trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "after accept the gate must not re-prompt"
        );
    }

    /// A trusted cwd ⇒ NO dialog (straight to the main screen / session).
    #[test]
    fn trusted_cwd_shows_no_dialog() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        // Pre-trust the cwd.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &cwd).unwrap();
        assert!(
            !trust_gate_should_prompt(&cwd, Some(config_path.as_path())),
            "trusted cwd must NOT mount the trust dialog"
        );
    }

    /// An ancestor-trusted cwd ⇒ NO dialog (parent-walk; trusting a parent
    /// trusts children — `checkHasTrustDialogAccepted`).
    #[test]
    fn ancestor_trusted_cwd_shows_no_dialog() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join(".lingxi.json");
        let parent = tmp.path().join("workspace");
        let child = parent.join("sub").join("project");
        std::fs::create_dir_all(&child).unwrap();
        // Trust the PARENT only.
        migrations::global_config::mark_trust_dialog_accepted(&config_path, &parent).unwrap();
        assert!(
            !trust_gate_should_prompt(&child, Some(config_path.as_path())),
            "ancestor-trusted cwd must NOT mount the trust dialog (parent-walk)"
        );
    }

    /// No global config path (no-home degrade) ⇒ NO dialog, proceed as today.
    #[test]
    fn no_config_path_shows_no_dialog() {
        let cwd = PathBuf::from("/some/untracked/dir");
        assert!(
            !trust_gate_should_prompt(&cwd, None),
            "no config path must proceed without a dialog (degrade)"
        );
    }

    // ---- companyAnnouncements startup-banner wiring (CC `LVs`/`oip()`) ----

    /// A configured non-empty array with an org name renders one dim
    /// `SystemText` block: `Message from <org>:` directly above the body.
    #[test]
    fn announcement_message_joins_org_prefix_above_body() {
        use lingxi_core::settings::company_announcements::{
            startup_announcement_with, AnnouncementMemo,
        };
        let memo = AnnouncementMemo::new();
        let a = vec!["".to_string(), "heads up".to_string()];
        // num_startups == 1 → deterministic FIRST non-empty entry.
        let sa = startup_announcement_with(&memo, Some(&a), 1, Some("Acme")).unwrap();
        match announcement_message(sa) {
            tui::RenderedMessage::SystemText { body, is_error, .. } => {
                assert_eq!(body, "Message from Acme:\nheads up");
                assert!(!is_error, "startup announcement is not an error line");
            }
            other => panic!("expected SystemText, got {other:?}"),
        }
    }

    /// No org name → the body alone (no `Message from …:` prefix).
    #[test]
    fn announcement_message_body_only_without_org() {
        use lingxi_core::settings::company_announcements::{
            startup_announcement_with, AnnouncementMemo,
        };
        let memo = AnnouncementMemo::new();
        let a = vec!["solo note".to_string()];
        let sa = startup_announcement_with(&memo, Some(&a), 1, None).unwrap();
        match announcement_message(sa) {
            tui::RenderedMessage::SystemText { body, .. } => assert_eq!(body, "solo note"),
            other => panic!("expected SystemText, got {other:?}"),
        }
    }

    /// `read_startup_config_from` extracts `numStartups` +
    /// `oauthAccount.organizationName`, tolerating absent/typed-wrong keys.
    #[test]
    fn read_startup_config_extracts_num_startups_and_org() {
        let map = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            r#"{"numStartups":7,"oauthAccount":{"organizationName":"Acme","id":"x"}}"#,
        )
        .unwrap();
        assert_eq!(
            read_startup_config_from(&map),
            (7, Some("Acme".to_string()))
        );

        // Missing numStartups → 0; missing oauthAccount → no org.
        let empty = serde_json::Map::new();
        assert_eq!(read_startup_config_from(&empty), (0, None));

        // oauthAccount present but no organizationName → num only, no org.
        let no_org = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            r#"{"numStartups":3,"oauthAccount":{"id":"x"}}"#,
        )
        .unwrap();
        assert_eq!(read_startup_config_from(&no_org), (3, None));
    }

    #[test]
    fn agent_status_filter_covers_every_agent_task_type() {
        assert!(is_agent_task_type("local_agent"));
        assert!(is_agent_task_type("remote_agent"));
        assert!(is_agent_task_type("in_process_teammate"));
        assert!(!is_agent_task_type("local_bash"));
        assert!(!is_agent_task_type("local_workflow"));
    }
}
