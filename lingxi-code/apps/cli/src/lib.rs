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

    logging::init(parsed.debug);
    tracing::debug!(?parsed, "argv parsed");

    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    // Pick the sink first so we can install it on the orchestrator at
    // construction time. For `--json` the session id used in `turn_start`
    // is minted afresh (synchronous mint via `SessionId::new`); for
    // plain mode the id is unused.
    let sink: Arc<dyn output::OutputSink> = if parsed.json {
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
    let runtime = match init::build_runtime(&parsed, adapter).await {
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
    // DEFERRED: the sibling permission-mode notice (TS
    // `permissionModeNotification`, `main.tsx:2882-2887`). In TS it is set ONLY
    // when `--dangerously-skip-permissions` requested bypassPermissions mode AND
    // that mode was disabled by org policy / settings (one of two exact strings,
    // `permissionSetup.ts:783-789`). The Rust CLI exposes no
    // `--permission-mode` / `--dangerously-skip-permissions` flag (`argv.rs`) and
    // hardwires `BuiltinToolContext.permission_mode = PermissionMode::Default`
    // (`engine-desktop:1395`); the only mode-resolution path
    // (`engine-desktop:1066`) is gated behind opt-in `LINGXI_ENFORCE_PERMISSIONS`
    // and never produces the bypass-disabled notification. So this notice is
    // unreachable without first porting `initialPermissionModeFromCLI` (a new CLI
    // flag + the bypass-disable settings check threaded to startup) — out of
    // scope here, deferred with that precise missing piece.
    if let Some(notice) = startup_deprecation_notice(&parsed) {
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
/// The lookup itself is `providers::deprecation::model_deprecation_warning`
/// (re-exported through `engine_desktop` so the CLI needs no direct `providers`
/// dependency); it returns `Some(warning)` only for a deprecated model under the
/// active provider and `None` otherwise — so a current default model yields
/// `None` and the caller prints nothing (byte-identical startup).
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
            prompt: None,
            print: false,
            resume: None,
            model: model.map(String::from),
            fallback_model: None,
            cwd: None,
            no_stream: false,
            json: false,
            debug: false,
            no_tui: false,
            continue_session: false,
            fork_session: false,
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
    /// different date per provider; see `providers::deprecation`).
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
