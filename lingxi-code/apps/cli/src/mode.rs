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
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use traits::OrchestratorHandle;

/// Execute the chosen mode. Returns the process exit code.
///
/// `Mode::Print` and `Mode::StdioRepl` call into the existing v0.6.0
/// code paths unchanged. `Mode::Tui` calls into the new `lingxi-tui`
/// entry point.
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
            // v0.6.0 REPL path. `repl::run_repl` rebuilds the runtime
            // internally (it owns its own sink/orchestrator construction
            // for the streaming-stdout case). Pass argv through and
            // ignore the `runtime` arg in this arm.
            let _ = runtime; // intentionally unused in this arm
            let _ = sink; // intentionally unused (repl mints its own)
            crate::repl::run_repl(argv).await
        }
        Mode::Tui => {
            // M6-03 path: rebuild the runtime with `BridgeOutputStream` as
            // the orchestrator's output, then pass the bridge_rx into
            // `run_tui_session` so streaming events route into AppState.
            //
            // The `runtime` arg here was built with the standard
            // sink-adapter output (for one-shot / NDJSON modes); we
            // discard it and construct a TUI-specific build. The original
            // sink is therefore unused in this arm.
            let _ = runtime; // intentionally unused — we mint a TUI build
            let _ = sink; // intentionally unused
            let tui_build = match crate::init::build_runtime_for_tui(argv).await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("lingxi-cli: tui init failed: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            };
            // (Task 3) Startup project-trust dialog (`TrustDialog`, shown by
            // `showSetupScreens` BEFORE the REPL/session and BEFORE any
            // tool/hook/plugin runs). TTY-only — this `Mode::Tui` arm is only
            // reached when stdin+stdout are terminals (`mode::decide_mode_with`
            // falls back to `StdioRepl` otherwise), so the raw-mode mount is
            // safe AND "non-interactive ⇒ no dialog" is satisfied for free.
            // Placed BEFORE the bypass gate: trust is the outermost "may I
            // touch this folder at all" gate. Already-accepted (incl.
            // parent-walk-trusted) ⇒ skip ⇒ byte-identical to today.
            match trust_gate().await {
                TrustGateOutcome::Proceed => {}
                TrustGateOutcome::Decline => {
                    // "No, exit" / Esc → exit 1 (`TrustDialog.tsx:158-160`
                    // `gracefulShutdownSync(1)`; matches the REPL gate).
                    return exit_codes::RUNTIME_ERROR;
                }
            }
            // (Task 8) Startup bypass-permissions confirmation dialog
            // (`BypassPermissionsModeDialog`, shown by `showSetupScreens` BEFORE
            // the REPL). TTY-only — this arm is only reached when stdin+stdout
            // are terminals (`mode::decide_mode`), so the raw-mode mount is safe
            // here. Gated on bypass-mode resolved AND no settings tier already
            // carrying `skipDangerousModePermissionPrompt`.
            let (bypass_mode, _) = crate::resolve_permission_mode(argv);
            let is_bypass = bypass_mode == permission::PermissionMode::BypassPermissions;
            let skip_set = read_skip_dangerous_prompt();
            if tui::startup_bypass::should_show_bypass_dialog(is_bypass, skip_set) {
                match tui::startup_bypass::mount_bypass_dialog().await {
                    Ok(tui::startup_bypass::BypassDialogOutcome::Accept) => {
                        // Persist so subsequent launches skip the prompt
                        // (`onConfirm` → `saveCurrentProjectConfig`). Best-effort.
                        persist_skip_dangerous_prompt();
                        // TELEMETRY (`tengu_bypass_permissions_mode_dialog_accept`,
                        // registered in Task 6): DEFERRED. There is no pre-session
                        // analytics sink wired at this seam (same situation as the
                        // startup migrations, which emit with `bus: None`). Emitting
                        // here would buffer onto a bus that is never flushed pre-REPL
                        // — an honest no-op is preferable to a call that looks wired
                        // but silently drops. The event name is registered and the
                        // emit lands once a pre-session sink exists.
                    }
                    Ok(tui::startup_bypass::BypassDialogOutcome::Decline) => {
                        // User declined (or pressed Esc): exit 1 (TS `process.exit(1)`).
                        return exit_codes::RUNTIME_ERROR;
                    }
                    Err(e) => {
                        eprintln!("lingxi-cli: bypass dialog failed: {e}");
                        return exit_codes::RUNTIME_ERROR;
                    }
                }
            }
            // (M7 cc2.1.198) Register this interactive session in the
            // cross-process live-session registry
            // (`~/.lingxi/sessions/<pid>.json`) so `lingxi-cli agents --json`
            // and the agents view can list it — the binary registers every
            // process the same way (observed `sessions/<pid>.json` shape).
            // Best-effort: a write failure never blocks the session; the
            // record is unlinked when the TUI returns.
            //
            // (M8 cc2.1.198) Live status refreshes: the registration is
            // shared (`Arc`) with the ratatui mount, whose channel forwarders
            // rewrite `status`/`updatedAt`/`statusUpdatedAt` on idle ↔ busy ↔
            // waiting transitions (binary `mvn`, fed by the REPL status
            // effect @222989611).
            // Capture the freshly-launched session id up front: it seeds the
            // switch loop's failed-switch fallback (finding #2) so a `/resume` to
            // an unloadable target re-mounts THIS session instead of exiting.
            let initial_session_id =
                tui_build.runtime.orchestrator.current_session_id().await;
            let session_registration = {
                let name = std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                Arc::new(crate::agents_registry::SessionRegistration::register(
                    &crate::run::lingxi_home_dir(),
                    Some(initial_session_id.to_string()).as_deref(),
                    name.as_deref(),
                ))
            };
            // FRESH launch: ratatui (`tui-rata`) is the only TUI backend. It
            // drives the live orchestrator directly via `tui_build`'s bridge
            // channel; no replayed scrollback (empty seed).
            let outcome =
                run_ratatui(tui_build, Some(session_registration.clone()), Vec::new()).await;
            // Unlink NOW (idempotent with Drop): the status forwarders may still
            // hold `Arc` clones inside detached tasks, and the record must not
            // outlive the interactive session.
            session_registration.deregister();
            // Follow an in-session `/resume` switch by re-mounting the chosen
            // session in-process (writer retargeted) until the user quits. A
            // switch re-mounts WITHOUT re-registering (resume never registers,
            // matching the existing `--resume` behavior).
            crate::run::drive_tui_switch_loop(argv, outcome, Some(initial_session_id.as_uuid()))
                .await
        }
    }
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
    let orchestrator: Arc<dyn OrchestratorHandle> = tui_build.runtime.orchestrator.clone();
    let (bridge_rx, permission_rx) = match &registration {
        Some(reg) => (
            spawn_status_bridge_forwarder(tui_build.bridge_rx, reg.clone()),
            spawn_status_permission_forwarder(tui_build.permission_rx, reg.clone()),
        ),
        None => (tui_build.bridge_rx, tui_build.permission_rx),
    };
    let turn_tx = tui_build.turn_tx;
    // (/permissions) The gate's live allow-rule bucket + the settings-file
    // roots the interactive editor writes to (same roots the AllowAlways
    // persist uses). Grabbed before `tui_build` is consumed further below.
    let permission_paths = tui_build.permission_paths.clone();
    let session_allow_rules = tui_build.session_allow_rules.clone();
    // Cloned BEFORE `on_submit` (below) moves `turn_tx` into its closure.
    let web_turn_tx = turn_tx.clone();
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
    let session = build_session_info(orchestrator.as_ref()).await;
    let handle = tokio::runtime::Handle::current();
    let switch_orch = orchestrator.clone();
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
    let rename_orch = orchestrator.clone();
    let rename_handle = handle.clone();
    let fast_orch = orchestrator.clone();
    let fast_handle = handle.clone();
    let plan_orch = orchestrator.clone();
    let plan_handle = handle.clone();
    // (/sandbox) The shared toggle cell threaded into the widget + clones for
    // the off-loop settings-persistence effect (mirrors the sibling triplets).
    let sandbox_toggle = tui_build.runtime.sandbox_toggle.clone();
    // (/tasks) The live background-task registry (already `TaskRegistryHandle`),
    // cloned as a trait object for the widget's snapshot read; plus a handle +
    // tx clone for the off-loop stop effect (mirrors the sandbox triplet).
    let task_registry_handle: std::sync::Arc<dyn traits::task_registry::TaskRegistryHandle> =
        tui_build.runtime.task_registry.clone();
    let task_handle = handle.clone();
    let task_turn_tx = turn_tx.clone();
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
    let reload_handle = handle.clone();
    let reload_turn_tx = turn_tx.clone();
    let on_reload_plugins = move || {
        let rt = reload_plugin_runtime.clone();
        let tx = reload_turn_tx.clone();
        reload_handle.spawn(async move {
            run_reload_plugins(rt, tx).await;
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
    let initial = if resumed_messages.is_empty() {
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
    let on_submit = move |prompt: String,
                          images: Vec<std::path::PathBuf>,
                          cancel: CancellationToken| {
        let _ = turn_tx.send(tui::TurnEvent::TurnStarted);
        let orch = orchestrator.clone();
        let tx = turn_tx.clone();
        handle.spawn(async move {
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
                let _ = tx.send(tui::TurnEvent::TurnEnded(traits::TurnOutcome::EndTurn));
            }
        });
    };
    let on_switch_model = move |model: String, profile: Option<String>| {
        let orch = switch_orch.clone();
        switch_handle.spawn(async move {
            let _ = orch.switch_model(&model, profile.as_deref()).await;
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
        permission_handle.spawn(async move {
            run_permission_action(action, paths, allow_rules, tx, snapshot).await;
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
    // the correct reactor, never freezes input, and is sequenced off the render
    // loop (avoiding the whole-history-swap race with an in-flight turn). The
    // summary (or failure) lands in the transcript via `TurnEvent::SystemNotice`
    // on the shared `turn_tx`, mirroring `command_core::compact`'s display text.
    let on_compact = move |_args: String| {
        let orch = compact_orch.clone();
        let tx = compact_turn_tx.clone();
        compact_handle.spawn(async move {
            let (body, is_error) = match orch.force_compact().await {
                Ok(_summary) => ("Compacted (ctrl+o to see full summary)".to_string(), false),
                Err(_e) => ("Error compacting conversation".to_string(), true),
            };
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                body,
                is_error,
            });
        });
    };
    // (/rename async effect) `/rename <name>` returns `ChatOutcome::RenameSession`
    // from the blocking ratatui loop; the custom-title JSONL append is a state
    // mutation, so it runs off the render thread on the captured handle (doctrine:
    // side-effects go off-loop). The confirmation (or failure) lands in the
    // transcript via `TurnEvent::SystemNotice`, mirroring claude-code's
    // `onDone("Session renamed to: ${newName}")`.
    let on_rename = move |name: String| {
        let orch = rename_orch.clone();
        let tx = rename_turn_tx.clone();
        rename_handle.spawn(async move {
            let (body, is_error) = match orch.rename_session(name.clone()).await {
                Ok(()) => (format!("Session renamed to: {name}"), false),
                Err(_e) => ("Error renaming session".to_string(), true),
            };
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                body,
                is_error,
            });
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
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                body,
                is_error,
            });
        });
    };
    // (/plan async effect) `/plan` returns `ChatOutcome::PlanMode` from the
    // blocking ratatui loop. The enter-vs-view decision (a read of the session
    // plan-mode flag) and, when entering, the flip both run off the render
    // thread on the captured handle (doctrine: session reads + mutations go
    // off-loop). LingXi has no on-disk plan store, so the "already in plan mode"
    // branch is a static message (claude-code `plan.tsx` no-plan branch); the
    // trimmed arg tail is currently advisory (no `<description>` submit / editor
    // open). The result is reported via `TurnEvent::SystemNotice`.
    let on_plan_mode = move |_args: String| {
        let orch = plan_orch.clone();
        let tx = plan_turn_tx.clone();
        plan_handle.spawn(async move {
            let (body, is_error) = if orch.plan_mode().await {
                (
                    "Already in plan mode. No plan written yet.".to_string(),
                    false,
                )
            } else {
                match orch.set_plan_mode(true).await {
                    Ok(()) => ("Enabled plan mode".to_string(), false),
                    Err(_e) => ("Error entering plan mode".to_string(), true),
                }
            };
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                body,
                is_error,
            });
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
            let (body, is_error) = match registry.kill(&task_id).await {
                Ok(_) => (format!("Stopped task {task_id}"), false),
                Err(e) => (format!("Could not stop task {task_id}: {e}"), true),
            };
            let _ = tx.send(tui_core::orchestrator_bridge::TurnEvent::SystemNotice {
                body,
                is_error,
            });
        });
    };
    // (statusline) Shared slot for the custom `statusLine` command, built from
    // the User+Local setting, plus the debounced single-flight pump (the
    // claude-code `StatusLine.tsx` execute-on-change analog: 300ms tick, run
    // only when the widget re-armed `dirty` on a turn boundary, set-only-on-
    // change). The slot is shared with the render thread via `run_app`.
    let status_line = tui::status_line::new_slot(read_status_line_config());
    let pump_slot = status_line.clone();
    let status_pump = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            // Build the (command, stdin-json) payload under the lock, then DROP
            // it before the off-thread command run. Skip unless re-armed.
            let payload = {
                let Ok(mut s) = pump_slot.lock() else {
                    continue;
                };
                if !s.dirty {
                    continue;
                }
                s.dirty = false;
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
            // `run_status_line_command` already returns formatted text — set
            // only on change (claude-code `prev.statusLineText === text`).
            if let Some(text) = out {
                if let Ok(mut s) = pump_slot.lock() {
                    if s.text.as_deref() != Some(text.as_str()) {
                        s.text = Some(text);
                    }
                }
            }
        }
    });
    let run_result = tokio::task::spawn_blocking(move || {
        tui::app::run_app(
            initial,
            session,
            bridge_rx,
            permission_rx,
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
            Some(sandbox_toggle),
            Some(command_registry),
            Some(task_registry_handle),
            on_submit,
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
            on_sandbox_action,
            on_task_action,
        )
    })
    .await;
    status_pump.abort();
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
            notice: None,
        })
    };
    match run_result {
        Ok(Ok(tui::app::AppExit::Quit)) => {
            // Print the BARE uuid (not the `sess:`-prefixed SessionId Display):
            // it matches the on-disk `<uuid>.jsonl` and what `--resume` resolves
            // to (claude-code uses bare uuids for session ids end-to-end).
            let session_uuid = summary_orch.current_session_id().await.as_uuid();
            println!(
                "\nSession {session_uuid} saved. Resume with: lingxi --resume {session_uuid}"
            );
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
    turn_tx: tokio::sync::mpsc::UnboundedSender<tui_core::orchestrator_bridge::TurnEvent>,
) {
    use tui_core::orchestrator_bridge::TurnEvent;

    /// `N noun` / `N nouns` — naive `+s` plural, matching claude-code's
    /// `plural()` for these ASCII nouns.
    fn n(count: usize, noun: &str) -> String {
        if count == 1 {
            format!("{count} {noun}")
        } else {
            format!("{count} {noun}s")
        }
    }

    let Some(rt) = plugin_runtime else {
        let _ = turn_tx.send(TurnEvent::SystemNotice {
            body: "Plugins are disabled for this session.".to_string(),
            is_error: false,
        });
        return;
    };

    let c = rt.refresh().await;
    // claude-code labels plugin COMMANDS "skills" in this line (`n(command_count,
    // 'skill')`); `agent_count`/hooks/MCP/LSP mirror the same result struct.
    let parts = [
        n(c.enabled, "plugin"),
        n(c.commands, "skill"),
        n(c.agents, "agent"),
        n(c.hooks, "hook"),
        n(c.mcp, "plugin MCP server"),
        n(c.lsp, "plugin LSP server"),
    ];
    let mut body = format!("Reloaded: {}", parts.join(" \u{00b7} "));
    if c.errors > 0 {
        body.push_str(&format!(
            "\n{} during load. Run /doctor for details.",
            n(c.errors, "error")
        ));
    }
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
                    format!("Sandbox mode {}.", if enabled { "enabled" } else { "disabled" }),
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
                Ok(()) => (
                    format!("Added \"{pattern}\" to excluded commands in local settings"),
                    false,
                ),
                Err(e) => (format!("Failed to update excluded commands: {e}"), true),
            }
        }
    };
    let _ = turn_tx.send(TurnEvent::SystemNotice { body, is_error });
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
                    // claude-code 2.1.205 success/already echoes, byte-exact:
                    // `Added ${path} as a working directory and saved to local
                    // settings` (persisted) / `… for this session` (session
                    // scope) / `${path} is already added as a working
                    // directory.`
                    let body = if written {
                        match dest {
                            PermissionUpdateDestination::Session => format!(
                                "Added {path} as a working directory for this session"
                            ),
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
                    let _ = turn_tx.send(TurnEvent::SystemNotice { body, is_error: false });
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
    http: Arc<dyn traits::HttpTransport>,
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
            let result = match resolve_client_search_provider_with_credentials(
                &cfg, &creds, &env,
            ) {
                Ok(resolved) => run_client_web_search(&http, &resolved, "current weather Beijing", &[], &[], 3)
                    .await
                    .map(|hits| (resolved, hits)),
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

/// Run one `/connect` [`tui::bottom_pane::ConnectAction`] to completion:
/// persist an API key, drive the GitHub Copilot device-flow, or drive an
/// OAuth browser sign-in — then report the outcome to the transcript via
/// `TurnEvent::SystemNotice`. Mirrors [`run_web_action`]'s shape: the
/// picker/method/key views return the action synchronously from the
/// blocking ratatui loop; this async tail does the real persistence/network
/// work off the render thread.
async fn run_connect_action(
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
                    notice(format!("✓ Saved {} API key.", label(&provider_id)), false);
                    connected(provider_id);
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
                format!("Opening your browser to sign in to {}…", label(&provider_id)),
                false,
            );
            match oauth.login(&provider_id).await {
                Ok(msg) => {
                    let body = if msg.trim().is_empty() {
                        format!("✓ Signed in to {}.", label(&provider_id))
                    } else {
                        msg
                    };
                    notice(body, false);
                    connected(provider_id);
                }
                Err(e) => notice(
                    format!("✗ Sign-in failed: {e}. Try connecting with an API key instead."),
                    true,
                ),
            }
        }
    }
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
                TurnEvent::TurnStarted => reg.update_status("busy", None),
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

/// Snapshot the orchestrator's MCP/hooks/agents/model listings into a
/// `tui::session::SessionInfo` for the full-page screens (`/mcp`,
/// `/hooks`, `/agents`, `/doctor`, `/model`). Awaited once before the blocking
/// TUI loop starts, mirroring the iocraft screens' capture-at-open contract.
async fn build_session_info(orch: &dyn OrchestratorHandle) -> tui::session::SessionInfo {
    use tui::session::{DoctorInfo, InfoRow, ModelRow, SessionInfo};

    let servers = orch.list_mcp_servers().await;
    let mcp_connected = u32::try_from(
        servers
            .iter()
            .filter(|s| matches!(s.status, traits::orchestrator::McpStatus::Connected))
            .count(),
    )
    .unwrap_or(u32::MAX);
    let mcp_configured = u32::try_from(servers.len()).unwrap_or(u32::MAX);
    let mcp = servers
        .into_iter()
        .map(|s| {
            let status = match &s.status {
                traits::orchestrator::McpStatus::Connected => "connected".to_string(),
                traits::orchestrator::McpStatus::Disconnected => "disconnected".to_string(),
                traits::orchestrator::McpStatus::Error(e) => format!("error: {e}"),
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
        .map(|m| ModelRow {
            // Mark the current row by (model AND provider) so a wire id shared
            // across providers (e.g. `gpt-5.5` on both OpenAI and Copilot) only
            // dots the ACTUAL current provider's row. When the current profile
            // is unknown (None — e.g. resolve-by-id after a cross-provider
            // resume), fall back to matching by model id alone.
            is_current: m.request_model == current_model
                && current_profile
                    .as_deref()
                    .is_none_or(|p| p == m.provider_id),
            // Keep `display` CLEAN — the `· 无思考` non-thinking tag is drawn at
            // picker-render time from `supports_reasoning`, NOT baked in here, so
            // the statusline / welcome identity (which read `display`) stay
            // untagged for a non-thinking current model.
            display: m.display_model,
            request_model: m.request_model,
            profile: (!m.provider_id.is_empty()).then_some(m.provider_id),
            provider_label: m.provider_label,
            supports_reasoning: m.supports_reasoning,
        })
        .collect();

    // Phase 8 (tui-rata command registry): `/skills` + `/memory` snapshots.
    // Small on-disk walks, captured once at launch like the listings above.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let skills = skills_rows(&cwd, &crate::run::lingxi_home_dir());
    let memory = dirs::home_dir().map_or_else(Vec::new, |home| memory_rows(&cwd, &home));

    SessionInfo {
        doctor: DoctorInfo::capture(mcp_configured, mcp_connected),
        mcp,
        hooks,
        agents,
        skills,
        memory,
        models,
    }
}

/// Flatten the on-disk skill sections (`skill_api::load_file_skill_sections`:
/// project `.lingxi/skills/` ancestors + user `~/.lingxi/skills/`) into
/// `/skills` rows: name + `"<section> · <description>"` detail.
fn skills_rows(
    cwd: &std::path::Path,
    lingxi_home: &std::path::Path,
) -> Vec<tui::session::InfoRow> {
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
fn memory_rows(
    cwd: &std::path::Path,
    os_home: &std::path::Path,
) -> Vec<tui::session::InfoRow> {
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

/// Read + merge the `statusLine` setting from the USER
/// (`~/.lingxi/settings.json`) and LOCAL (`<proj>/.lingxi/settings.local.json`)
/// tiers, Local-over-User, and parse it into a [`StatusLineConfig`]. `None`
/// when neither tier carries a `statusLine` object.
fn read_status_line_config_from(
    lingxi_home: &std::path::Path,
    project_dir: &std::path::Path,
) -> Option<tui_core::status_line_command::StatusLineConfig> {
    use migrations::settings_update::{read_settings_map, settings_path, SettingsSource};
    // Local-over-User: read User first, then let Local's `statusLine` override.
    let mut status_line: Option<serde_json::Value> = None;
    for source in [SettingsSource::User, SettingsSource::Local] {
        let p = settings_path(source, lingxi_home, project_dir);
        if let Ok(map) = read_settings_map(&p) {
            if let Some(v) = map.get("statusLine") {
                status_line = Some(v.clone());
            }
        }
    }
    status_line
        .as_ref()
        .and_then(tui_core::status_line_command::StatusLineConfig::from_settings_value)
}

/// Live wrapper over [`read_status_line_config_from`], resolving the User+Local
/// settings roots via [`settings_dirs`].
fn read_status_line_config() -> Option<tui_core::status_line_command::StatusLineConfig> {
    let (lingxi_home, project_dir) = settings_dirs();
    read_status_line_config_from(&lingxi_home, &project_dir)
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

    fn argv(prompt: Option<&str>, no_tui: bool) -> Argv {
        Argv {
            prompt: prompt.map(String::from),
            no_tui,
            ..Argv::default()
        }
    }

    #[test]
    fn prompt_routes_to_print() {
        let a = argv(Some("fix it"), false);
        assert_eq!(decide_mode_with(&a, true), Mode::Print("fix it".into()));
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
            Ok(TurnEvent::SystemNotice { is_error: false, .. })
        ));

        // Exclude appends to local settings; a repeat is idempotent.
        run_sandbox_action(SandboxAction::Exclude("npm test:*".into()), paths.clone(), tx.clone())
            .await;
        run_sandbox_action(SandboxAction::Exclude("npm test:*".into()), paths.clone(), tx.clone())
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

    // ── (A6 batch-6 Task 2) statusLine settings merge ─────────────────────

    fn write_settings(path: &std::path::Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
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
}
