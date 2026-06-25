//! Library surface of the `lingxi-cli` crate.
//!
//! Integration tests import the public API instead of shelling out to the
//! binary so they don't need the workspace target dir hot.
//!
//! # One-shot mode
//!
//! ```text
//! $ lingxi-cli "fix the bug in foo.rs"
//! $ lingxi-cli -p "list the files in this repo"
//! $ lingxi-cli --no-stream --json "what's the weather?"
//! ```
//!
//! # Resume mode
//!
//! ```text
//! $ lingxi-cli --resume <uuid>   # load that session
//! $ lingxi-cli --resume          # TTY: iocraft Resume screen (M7-12)
//! $ lingxi-cli --resume --no-tui # stdio picker over 5 most-recent (M5-08)
//! ```
//!
//! # REPL mode (M5-13)
//!
//! Invoking `lingxi-cli` without a positional prompt drops into a
//! line-based REPL:
//!
//! ```text
//! $ lingxi-cli
//! > /version
//! lingxi-cli 0.5.0 (abc1234)
//! > hello, claude
//! Hi! How can I help?
//! > /exit
//! Exiting.
//! ```
//!
//! - **Prompt**: `"> "` printed to stderr (so stdout stays parseable in
//!   `--json` mode).
//! - **EOF (Ctrl+D)**: persists the session and exits 0.
//! - **First Ctrl+C during a turn**: cancels the turn, returns to prompt.
//! - **First Ctrl+C at idle prompt**: arms a flag; second Ctrl+C within
//!   2 seconds exits with code 130.
//! - **`/exit`**: flips the orchestrator's `should_exit` flag; REPL
//!   detects it after the dispatcher returns and exits 0.
//!
//! # Exit codes
//!
//! See [`exit_codes`].
//!
//! # Plan reference
//!
//! `docs/superpowers/plans/2026-05-25-m5-12-cli-binary.md` (one-shot),
//! `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` (REPL).

#![forbid(unsafe_code)]

pub mod argv;
mod bypass_env;
pub mod commands;
pub mod control_plane;
pub mod cwd;
pub mod exit_codes;
pub mod idle_notify;
pub mod init;
pub mod logging;
pub mod mode;
pub mod output;
pub mod output_adapter;
pub mod repl;
pub mod repl_loop;
pub mod run;
pub mod sigint;
pub mod stream_json;
pub mod stream_json_input;
pub mod structured_output;

use crate::argv::Argv;
use clap::error::ErrorKind;
use std::ffi::OsString;
use std::sync::Arc;

/// Top-level entrypoint. Returns the process exit code.
pub async fn run_cli(args: Vec<OsString>) -> i32 {
    let parsed = match Argv::from_iter(args) {
        Ok(a) => a,
        Err(e) => {
            // clap prints its own help/usage; we just return the locked
            // code. Help/version are not errors.
            e.print().ok();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit_codes::SUCCESS,
                _ => exit_codes::ARGV_ERROR,
            };
        }
    };

    // `--debug` is now `Option<String>` (optional category filter); collapse to
    // on/off for logging init. `--mcp-debug` (deprecated alias) and `--debug-file`
    // also imply debug mode.
    logging::init(parsed.debug_enabled());
    tracing::debug!(?parsed, "argv parsed");

    // Top-level subcommand dispatch (mcp/auth/plugin/project/setup-token/agents/
    // install/update/doctor/auto-mode/ultrareview). When clap matched a leading
    // command token, run that family and exit — this is what stops a bare `mcp`/
    // `auth` token from being swallowed as a billable chat prompt.
    if let Some(command) = parsed.command.clone() {
        return command.run().await;
    }

    // `--session-id <uuid>` validation (claude-code main.tsx:1276-1300), byte-exact
    // messages + exit 1. Runs for every session mode (print/TUI/REPL) before any
    // runtime is built; the validated id then threads into
    // `DesktopConfig.session_id_override` via `resolve_desktop_config`.
    if let Some(sid) = parsed.session_id.as_deref() {
        // (a) cross-flag rule: pairing --session-id with --continue/--resume
        //     requires --fork-session (else the resumed session's own id wins).
        if (parsed.continue_session || parsed.resume.is_some()) && !parsed.fork_session {
            eprintln!(
                "Error: --session-id can only be used with --continue or --resume if --fork-session is also specified."
            );
            return exit_codes::ARGV_ERROR;
        }
        // (b) UUID validation (a bare UUID; `parse_prefixed` also tolerates the
        //     `sess:`-prefixed display form).
        if protocol::SessionId::parse_prefixed(sid).is_none() {
            eprintln!("Error: Invalid session ID. Must be a valid UUID.");
            return exit_codes::ARGV_ERROR;
        }
        // (c) DEFERRED vs claude: the "Session ID <id> is already in use" check
        //     (sessionIdExists) needs a cross-project session-store lookup; not
        //     yet wired. A collision currently reuses the transcript path rather
        //     than erroring.
    }

    // `--setting-sources <user,project,local>` token validation (claude-code
    // main.tsx): each comma-split token must be one of user/project/local
    // (CASE-SENSITIVE, raw token echoed) else hard-error with the byte-exact
    // message + exit 1. An empty string is VALID (treated as no sources).
    if let Some(sources) = parsed.setting_sources.as_deref() {
        for tok in sources.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !matches!(tok, "user" | "project" | "local") {
                eprintln!(
                    "Error processing --setting-sources: Invalid setting source: {tok}. Valid options are: user, project, local"
                );
                return exit_codes::ARGV_ERROR;
            }
        }
    }

    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // (Item B) Resolve the session permission mode from CLI flags + settings,
    // run the bypass safety guards, and capture the startup notice. This must
    // happen AFTER `cwd::apply_cwd` (so the project `.claude/settings.json` is
    // read from the effective project dir) and BEFORE `build_runtime` — a
    // refused bypass exits before the runtime is constructed, and the resolved
    // mode threads into `DesktopConfig.permission_mode`.
    //
    // (Task 8) The resolution itself is now the shared `resolve_permission_mode`
    // helper so the interactive TUI/REPL paths resolve the SAME mode without
    // re-implementing it. The bypass-safety GUARD stays HERE in `run_cli`: it
    // runs exactly once, before mode dispatch, for ALL modes — a refusal exits 1
    // before any runtime is built, so the interactive paths never re-run it.
    let (permission_mode, permission_notice) = resolve_permission_mode(&parsed);
    // Guards run when bypass is requested OR resolved (setup.ts:396).
    if permission_mode == permission::PermissionMode::BypassPermissions
        || parsed.dangerously_skip_permissions
    {
        if let Err(msg) = permission::enforce_bypass_safety(&bypass_env::RealBypassEnv::new()).await
        {
            eprintln!("{msg}");
            return exit_codes::RUNTIME_ERROR; // TS process.exit(1)
        }
    }

    // P3 cross-flag validation for --input-format=stream-json and
    // --replay-user-messages (§4.1 SPEC-inferred.md, exact error strings).
    if let Err(msg) = parsed.validate_stream_json_input_args() {
        eprintln!("Error: {msg}");
        return exit_codes::ARGV_ERROR;
    }

    // `--include-partial-messages` requires BOTH --print and
    // --output-format=stream-json (claude-code main.tsx:1848-1852). Byte-exact
    // error + exit 1 (ARGV_ERROR). Without this gate lingxi silently accepted
    // the misuse and ran a (billable) turn.
    if parsed.include_partial_messages && !(parsed.print && parsed.is_stream_json()) {
        eprintln!("Error: --include-partial-messages requires --print and --output-format=stream-json.");
        return exit_codes::ARGV_ERROR;
    }

    // stream-json: `--output-format stream-json --verbose` (print-only, no
    // SinkAdapter/OutputSink layer — the StreamJsonStream IS the OutputStream).
    //
    // Gate: when `--print`/`-p` is combined with `stream-json` the caller MUST
    // also pass `--verbose` (mirrors claude-code's argv validation:
    // `main.tsx printMode && !verbose && outputFormat=="stream-json"` →
    // "When using --print, --output-format=stream-json requires --verbose").
    if parsed.is_stream_json() {
        if parsed.print && !parsed.verbose {
            eprintln!("Error: When using --print, --output-format=stream-json requires --verbose");
            return exit_codes::ARGV_ERROR;
        }
        let stream = Arc::new(stream_json::StreamJsonStream::new_placeholder());
        // P4: wire --include-partial-messages and --include-hook-events flags
        // before build_runtime so the stream is fully configured before any
        // hook or SSE events flow through it.
        stream.set_flags(parsed.include_partial_messages, parsed.include_hook_events);
        let adapter: Arc<dyn traits::OutputStream> = stream.clone();

        // P5 Phase 2: for the bidirectional `--input-format stream-json` path,
        // build the shared control plane BEFORE `build_runtime` (its outbound
        // handle comes from the stream, available now) and inject the
        // `can_use_tool` permission decider as the inner transport. The
        // `PolicyPermissionGate` (enforcement default on) wraps it as the OUTER
        // local pre-check, so only an unresolved `Ask` round-trips over stdio.
        // The output-only print path keeps the headless deny-on-ask default
        // (no stdin reader to answer a control_response).
        let control_plane = if parsed.is_stream_json_input() {
            Some(control_plane::StdioControlPlane::new(stream.outbound_tx()))
        } else {
            None
        };

        let runtime = if let Some(plane) = &control_plane {
            let mut cfg = init::resolve_desktop_config(&parsed, permission_mode);
            // §2b: persist an ALLOW response's `updatedPermissions` rule updates to
            // the SAME settings tree the engine resolved (claude-code
            // `persistPermissionUpdates`). Paths come from the same `cfg` so a
            // host-allowed rule lands where the next session loads it.
            let gate = Arc::new(
                control_plane::StdioControlPermissionGate::new(plane.clone()).with_persist(
                    permission::PermissionPaths {
                        claude_home: cfg.claude_home.clone(),
                        cwd: cfg.cwd.clone(),
                    },
                ),
            );
            cfg.injected_permission_gate =
                Some(gate as Arc<dyn permission::gate::PermissionGate>);
            match init::build_runtime_from_config(cfg, adapter).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        } else {
            match init::build_runtime(&parsed, adapter, permission_mode).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("lingxi-cli: {e}");
                    return exit_codes::RUNTIME_ERROR;
                }
            }
        };
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        // P3: when --input-format=stream-json is also set, use the multi-turn
        // stdin loop instead of the single-prompt one-shot path.
        if let Some(plane) = control_plane {
            return run::run_stream_json_input_loop(
                &parsed,
                &runtime,
                stream,
                permission_mode,
                plane,
            )
            .await;
        }
        return run::run_stream_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    // `--output-format json` / `--json` in PRINT mode: emit only the final
    // result JSON line (suppress all streaming frames). Only intercept when
    // a non-slash prompt is present or `--print` is active (no slash prompt)
    // — slash commands keep the old JSON event format, interactive/REPL mode
    // with `--json` falls through to the normal dispatch path (repl.rs handles it).
    let is_non_slash_print = parsed.prompt.as_deref().map_or(false, |p| !p.trim_start().starts_with('/'));
    if parsed.is_json_output() && (is_non_slash_print || parsed.print) {
        let stream = Arc::new(stream_json::StreamJsonStream::new_json_mode_placeholder());
        let adapter: Arc<dyn traits::OutputStream> = stream.clone();
        let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lingxi-cli: {e}");
                return exit_codes::RUNTIME_ERROR;
            }
        };
        if let Some(notice) = startup_deprecation_notice(&parsed) {
            eprintln!("{notice}");
        }
        if let Some(notice) = &permission_notice {
            eprintln!("{notice}");
        }
        return run::run_json_print(&parsed, &runtime, stream, permission_mode).await;
    }

    // Pick the sink first so we can install it on the orchestrator at
    // construction time. `--json` with a slash command uses `JsonSink` so the
    // old `{"event":"command_output",...}` format is preserved. All other
    // modes use `PlainSink` (the `--json` + normal-prompt path already returned
    // above via `run_json_print`).
    let sink: Arc<dyn output::OutputSink> = if parsed.is_json_output() {
        Arc::new(output::JsonSink::new(protocol::SessionId::new()))
    } else {
        Arc::new(output::PlainSink::new())
    };
    let adapter: Arc<dyn traits::OutputStream> =
        Arc::new(output_adapter::SinkAdapter::new(sink.clone()));

    // For `Mode::Print` we still need the runtime; for `Mode::StdioRepl`
    // and `Mode::Tui` we also build it once so `mode::dispatch` can pass
    // the orchestrator's session id into the TUI. Building the runtime
    // is cheap (no API calls until `run_turn`).
    let runtime = match init::build_runtime(&parsed, adapter, permission_mode).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lingxi-cli: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // (W40-follow-up) One-time startup notices, the bounded stand-in for
    // claude-code's startup notification queue (`main.tsx:2872-2896`). TS pushes
    // up to two HIGH-priority notices onto a UI queue rendered above the REPL:
    // the model-deprecation warning and the permission-mode notice. The Rust CLI
    // has no UI notification queue, so — per the brief — we emit the bounded
    // subset as a plain startup line instead of building that abstraction.
    //
    // Surfaced here: the DEPRECATION warning. `startup_deprecation_notice`
    // resolves the same initial model `resolve_desktop_config` threads into the
    // engine (`--model`, else the desktop default — the analog of TS
    // `resolvedInitialModel = parseUserSpecifiedModel(initialMainLoopModel ??
    // getDefaultMainLoopModel())`) and returns `Some` only when that model is
    // deprecated for the active provider. It goes to STDERR so `--json` stdout
    // stays parseable (the same channel discipline the REPL's `"> "` prompt
    // uses). SAFETY: with any current (Claude 4-generation) default model the
    // lookup is `None`, so this prints nothing and startup is byte-identical.
    //
    // The sibling permission-mode notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`) is now wired below: `initialPermissionModeFromCLI`
    // (resolved pre-`build_runtime` above) sets `permission_notice` when the
    // bypass killswitch suppressed a requested bypass.
    if let Some(notice) = startup_deprecation_notice(&parsed) {
        eprintln!("{notice}");
    }

    // (Item B) Permission-mode startup notice (TS `permissionModeNotification`,
    // `main.tsx:2882-2887`): set only when the bypass killswitch suppressed a
    // requested bypass (`initialPermissionModeFromCLI`). Emitted on the same
    // bounded stderr channel as the deprecation/migration notices (no UI
    // notification-queue substrate); `None` in the common case, so startup is
    // byte-identical when no bypass was disabled.
    if let Some(notice) = &permission_notice {
        eprintln!("{notice}");
    }

    // Config migrations (`main.tsx runMigrations`, CURRENT_MIGRATION_VERSION
    // = 11) — the same pre-REPL point as the deprecation notice above, common
    // to Print/Tui/StdioRepl. At `migrationVersion == 11` this is a read-only
    // no-op (version guard). A NEWER real claude-code may have moved the file
    // past 11; the TS `!==` guard then re-runs the set (all 9 migrations
    // no-op on an already-migrated config) and writes 11 back — the same
    // bounded version ping-pong two coexisting real claude-code versions
    // produce. `bus: None`: no pre-boot telemetry bus substrate exists (same
    // as the deprecation notice); the 9 event names are registered for when
    // one does. Tier is structurally None (no keychain subscriptionType) —
    // the subscriber-gated migrations take their faithful fail-closed
    // branches; the CLI deliberately does not read the keychain pre-boot
    // (avoids a second keychain prompt).
    //
    // `project_dir`: `std::env::current_dir()` is read AFTER `cwd::apply_cwd`
    // above, so it reflects the effective `--cwd` project directory — the CLI
    // keeps no pre-chdir "original cwd"; the post-chdir dir is the project
    // dir the migrations should target (matches TS, where migrations run
    // against the resolved working directory).
    if let (Some(global_config_path), Some(claude_home)) = (
        migrations::global_config::global_config_path(),
        migrations::global_config::claude_config_home(),
    ) {
        let project_dir =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let env = migrations::MigrationEnv {
            global_config_path,
            claude_config_home: claude_home,
            project_dir,
            ctx: migrations::MigrationContext::from_env(),
            bus: None,
        };
        migrations::run_migrations(&env).await;
        // Async fire-and-forget (TS `.catch(() => {})`): retried next startup.
        tokio::spawn(async move {
            migrations::migrate_changelog_from_config(&env).await;
        });
    }

    // --resume routes through run::run_resume, which itself splits (M7-12):
    //   <uuid>            → load by id
    //   (none) + TTY      → iocraft Resume screen
    //   (none) + --no-tui → M5-08 stdio picker (unchanged fallback)
    if parsed.resume.is_some() {
        return run::run_resume(&parsed, &runtime, sink.as_ref()).await;
    }

    let chosen = mode::decide_mode(&parsed);
    mode::dispatch(chosen, &parsed, &runtime, sink).await
}

/// Resolve the session permission mode (and any suppression notice) from CLI
/// flags + merged `settings.json` — the shared resolver every dispatch path
/// uses so the one-shot/print, interactive TUI, and stdio-REPL paths all see
/// the SAME `initialPermissionModeFromCLI` result.
///
/// This is `read_cli_mode_settings` + `permission::initial_permission_mode_from_cli`,
/// with NO bypass-safety guard: the guard runs exactly once in [`run_cli`]
/// (before mode dispatch, for all modes), so the interactive paths that call
/// this helper to re-derive the mode must NOT re-run it. Returns
/// `(mode, notice)` where `notice` is `Some` only when the bypass killswitch
/// suppressed a requested bypass (`permissionModeNotification`).
pub(crate) fn resolve_permission_mode(
    argv: &Argv,
) -> (permission::PermissionMode, Option<String>) {
    let settings = read_cli_mode_settings(argv);
    permission::initial_permission_mode_from_cli(
        argv.permission_mode.as_deref(),
        argv.dangerously_skip_permissions,
        &settings,
    )
}

/// Build [`permission::CliModeSettings`] from the merged user+project
/// `settings.json` files (the bypass-killswitch + settings `defaultMode`
/// inputs the mode resolver reads).
///
/// Reads the user `~/.claude/settings.json` then the project
/// `<cwd>/.claude/settings.json` raw, deriving the two fields via the existing
/// `permission` helpers: `defaultMode` takes project-wins precedence (project
/// read last), and the bypass-disable killswitch is sticky (set by any tier).
/// On any load failure (missing/unreadable/malformed file) it degrades to the
/// no-op default `{ default_mode: None, bypass_disabled: false }` — a faithful
/// port of TS `getSettings_DEPRECATED() || {}`.
///
/// `parsed` is currently unused (the CLI has no settings-path override flag);
/// it is threaded for forward-compatibility with such a flag.
pub(crate) fn read_cli_mode_settings(parsed: &Argv) -> permission::CliModeSettings {
    // `--setting-sources <user,project,local>` gates which settings files this
    // permission-mode reader consults too (claude scopes ALL settings loading,
    // not just providers/routing). `None` ⟶ both layers (default).
    let (incl_user, incl_project) = init::setting_source_flags(parsed.setting_sources.as_deref());
    let project_dir =
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut default_mode = None;
    let mut bypass_disabled = false;
    let home = incl_user.then(|| crate::run::claude_home_dir().join("settings.json"));
    let proj = incl_project.then(|| project_dir.join(".claude").join("settings.json"));
    // User first, then project (ascending priority): project read last wins on
    // `defaultMode`; `bypass_disabled` is sticky across tiers.
    for path in [home, proj].into_iter().flatten() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                default_mode = Some(m);
            }
            if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                bypass_disabled = true;
            }
        }
    }
    permission::CliModeSettings {
        default_mode,
        bypass_disabled,
    }
}

/// Resolve the initial main-loop model the engine will use, then return the
/// model-deprecation startup notice for it (or `None` when current).
///
/// The model resolution is byte-identical to the one
/// `init::resolve_desktop_config` threads into `DesktopConfig.default_model`:
/// the `--model` override if present, else the desktop default
/// (`engine_desktop::DesktopConfig::default().default_model`). This mirrors
/// claude-code's `resolvedInitialModel = parseUserSpecifiedModel(
/// initialMainLoopModel ?? getDefaultMainLoopModel())` (`main.tsx:2116`), the
/// same value fed to `getModelDeprecationWarning` at `main.tsx:2873`.
///
/// The lookup itself is `engine_desktop::model_deprecation_warning`
/// (moved from the deleted `providers` crate in Plan 3b); it returns
/// `Some(warning)` only for a deprecated model under the active provider and
/// `None` otherwise — so a current default model yields `None` and the caller
/// prints nothing (byte-identical startup).
fn startup_deprecation_notice(argv: &Argv) -> Option<String> {
    let resolved_model = argv
        .model
        .clone()
        .unwrap_or_else(|| engine_desktop::DesktopConfig::default().default_model);
    engine_desktop::model_deprecation_warning(Some(&resolved_model))
}

#[cfg(test)]
mod startup_notice_tests {
    use super::*;

    fn argv_with_model(model: Option<&str>) -> Argv {
        Argv {
            model: model.map(String::from),
            ..Argv::default()
        }
    }

    /// SAFETY: the resolved DEFAULT model (no `--model`) is a current Claude 4
    /// id, so the deprecation lookup returns `None` and startup prints nothing —
    /// byte-identical to before this notice landed. This is the common-case
    /// invariant the brief pins.
    #[test]
    fn default_model_yields_no_notice() {
        // Belt-and-suspenders: assert against the actual desktop default rather
        // than a hardcoded literal so a future default bump can't silently start
        // emitting a notice at every startup.
        let default_model = engine_desktop::DesktopConfig::default().default_model;
        assert!(
            engine_desktop::model_deprecation_warning(Some(&default_model)).is_none(),
            "the shipped desktop default model ({default_model}) must not be deprecated, \
             else every startup would emit a notice"
        );
        assert_eq!(startup_deprecation_notice(&argv_with_model(None)), None);
    }

    /// A `--model` override naming a CURRENT model still yields no notice.
    #[test]
    fn current_override_model_yields_no_notice() {
        assert_eq!(
            startup_deprecation_notice(&argv_with_model(Some("claude-opus-4-7"))),
            None
        );
    }

    /// A `--model` override naming a DEPRECATED model surfaces the exact
    /// TS-faithful warning line (`deprecation.ts:100` text, byte-locked
    /// including the leading `⚠ ` glyph). This is the only condition under which
    /// startup emits anything. The provider env is cleared first so the
    /// first-party retirement date is the one asserted (the table carries a
    /// different date per provider; see `engine_desktop::model_deprecation_warning`).
    #[test]
    fn deprecated_override_model_yields_first_party_notice() {
        let prior = [
            ("CLAUDE_CODE_USE_BEDROCK", std::env::var_os("CLAUDE_CODE_USE_BEDROCK")),
            ("CLAUDE_CODE_USE_VERTEX", std::env::var_os("CLAUDE_CODE_USE_VERTEX")),
            ("CLAUDE_CODE_USE_FOUNDRY", std::env::var_os("CLAUDE_CODE_USE_FOUNDRY")),
        ];
        for (k, _) in &prior {
            std::env::remove_var(k);
        }

        let notice = startup_deprecation_notice(&argv_with_model(Some("claude-3-opus-20240229")))
            .expect("a deprecated --model must produce a startup notice");
        assert_eq!(
            notice,
            "⚠ Claude 3 Opus will be retired on January 5, 2026. Consider switching to a newer model."
        );

        // Restore any provider flags this test cleared.
        for (k, v) in prior {
            if let Some(v) = v {
                std::env::set_var(k, v);
            }
        }
    }
}

#[cfg(test)]
mod cli_mode_settings_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// `read_cli_mode_settings` reads process-global state (`$HOME` via
    /// `dirs::home_dir` + the process cwd), so the two tests that mutate those
    /// must not run concurrently. A local mutex serializes them (the rest of the
    /// resolver is pure and tested env-free in `permission::cli_mode`).
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn argv() -> Argv {
        Argv::default()
    }

    /// With no `~/.claude/settings.json` and no `<cwd>/.claude/settings.json`,
    /// the helper degrades to the no-op default (the faithful TS
    /// `getSettings_DEPRECATED() || {}` fallback).
    #[test]
    fn degrades_to_default_when_no_settings_files() {
        let _g = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        // Point HOME + cwd at fresh empty dirs (no `.claude/settings.json`).
        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let s = read_cli_mode_settings(&argv());
        assert!(s.default_mode.is_none());
        assert!(!s.bypass_disabled);

        // Restore process-global state.
        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    /// The project `<cwd>/.claude/settings.json` `defaultMode` is read and wins
    /// over the user tier, and `disableBypassPermissionsMode: "disable"` sets the
    /// killswitch — exercising the parse path, not just the empty degrade.
    #[test]
    fn reads_project_settings_default_mode_and_killswitch() {
        let _g = env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior_home = std::env::var_os("HOME");
        let prior_cwd = std::env::current_dir().ok();

        let home = tempfile::tempdir().expect("home tempdir");
        let proj = tempfile::tempdir().expect("proj tempdir");
        let proj_claude = proj.path().join(".claude");
        std::fs::create_dir_all(&proj_claude).expect("mkdir .claude");
        std::fs::write(
            proj_claude.join("settings.json"),
            r#"{"permissions":{"defaultMode":"acceptEdits","disableBypassPermissionsMode":"disable"}}"#,
        )
        .expect("write settings");
        std::env::set_var("HOME", home.path());
        std::env::set_current_dir(proj.path()).expect("chdir proj");

        let s = read_cli_mode_settings(&argv());
        assert_eq!(s.default_mode, Some(permission::PermissionMode::AcceptEdits));
        assert!(s.bypass_disabled);

        if let Some(cwd) = prior_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
