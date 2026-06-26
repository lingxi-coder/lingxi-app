//! REPL entry point — wraps `repl_loop::step` in a real `tokio::io::stdin`
//! / `tokio::io::stderr` driver and emits lifecycle telemetry.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 5.

use crate::argv::Argv;
use crate::exit_codes;
use crate::output::{JsonSink, OutputSink, PlainSink};
use crate::output_adapter::SinkAdapter;
use crate::repl_loop::{step, StepOutcome};
use crate::sigint::SigintSource;
use futures::future::BoxFuture;
use orchestrator::{OrchestratorError, TurnOutcome};
use protocol::SessionId;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{stderr, stdin, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Stdin};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use traits::{OrchestratorHandle, OutputStream};

/// Pure decision: should the REPL surface an interactive permission prompt?
///
/// `true` only when stdin is a TTY AND we are not in `--print` mode. The REPL
/// is never `print` (that path is `Mode::Print`), but the rule is kept explicit
/// + testable so the gate is never injected for a piped/CI session.
fn should_prompt_interactively(is_tty: bool, print: bool) -> bool {
    is_tty && !print
}

/// Outcome of the startup trust gate.
///
/// `Proceed` ⇒ build the runtime + enter the loop (today's behavior).
/// `Decline` ⇒ exit BEFORE building the runtime, with claude-code's
/// "No, exit" exit code (`exit_codes::RUNTIME_ERROR` = 1, see below).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustOutcome {
    Proceed,
    Decline,
}

/// Byte-locked startup trust-dialog prompt for the stdio gate path.
///
/// Transcribes claude-code's `TrustDialog` (`components/TrustDialog/
/// TrustDialog.tsx:206-263`) literal copy faithfully to a flat stderr
/// `[y/N]` prompt. The Ink `PermissionDialog`/`Box`/`Select`/`Link` chrome
/// cannot render through the gate's single-line `read_line` path, so the
/// surrounding layout (newlines, the inlined Security-guide URL since stdio
/// has no clickable `Link`, the `[y/N]` answer line modeled on
/// `format_prompt_tool_use`) is the faithful flat adaptation — but every
/// SENTENCE / LABEL / TITLE string is byte-locked to the TS:
///   - title         `TrustDialog.tsx:257` `title="Accessing workspace:"`
///   - cwd (bold)    `:207` `<Text bold>{getFsImplementation().cwd()}</Text>`
///   - safety check  `:208` "Quick safety check: …review what's in this folder first."
///   - capabilities  `:209` "Claude Code'll be able to read, edit, and execute files here."
///   - security link `:220` "Security guide" → https://code.claude.com/docs/en/security
///   - options       `:228,231` "Yes, I trust this folder" / "No, exit"
fn format_trust_prompt(cwd: &Path) -> String {
    format!(
        "Accessing workspace:\n\n{cwd}\n\nQuick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.\nLingXi'll be able to read, edit, and execute files here.\n\nSecurity guide: https://code.claude.com/docs/en/security\n\n  Yes, I trust this folder\n  No, exit\n[y/N] ",
        cwd = cwd.display()
    )
}

/// Parse a single line of trust-dialog input: accept iff `y`/`yes`
/// (case-insensitive, trimmed). Everything else — `n`/`no`, empty (just
/// Enter), garbage — declines. This mirrors the permission gate's
/// trim+lowercase classification (`prompting_gate.rs:120-130`), but the
/// default here is DENY (the `[y/N]` capital N + claude-code's "No, exit"
/// being the safe default selection): an empty line / EOF declines.
fn parse_trust_input(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// REPL startup trust GATE (design doc §2; parity
/// `components/TrustDialog/TrustDialog.tsx` + `screens/REPL.tsx` startup).
///
/// claude-code shows the trust dialog BEFORE any tool/hook/plugin can run; a
/// running session is therefore always trusted (decline exits). This is the
/// stdio-REPL analogue: prompt over the SHARED stdin reader + stderr, BEFORE
/// the runtime is built and the loop entered.
///
/// Gating (matches the dialog's skip/exit logic):
/// - Non-interactive (non-TTY / `--print`) ⇒ `Proceed`, NO prompt, NO byte
///   consumed — byte-identical to today (the dialog is interactive-only;
///   `checkHasTrustDialogAccepted` short-circuits headless via the same
///   `should_prompt_interactively` seam used for the permission gate).
/// - No resolvable global config path OR already-accepted
///   (`check_has_trust_dialog_accepted`, parent-walk) ⇒ `Proceed`, NO prompt
///   (`TrustDialog.tsx:199-202`: `if (hasTrustDialogAccepted) { onDone() }`).
/// - Otherwise prompt; `y`/`yes` ⇒ `mark_trust_dialog_accepted` (best-effort,
///   the "Yes, I trust this folder" branch `TrustDialog.tsx:177` →
///   `saveCurrentProjectConfig({ hasTrustDialogAccepted: true })`) then
///   `Proceed`; `n`/`no`/empty/EOF ⇒ `Decline` (the "No, exit" branch
///   `TrustDialog.tsx:158-160` → `gracefulShutdownSync(1)`).
///
/// The read locks the SAME `Arc<Mutex<BufReader<Stdin>>>` the loop later uses,
/// sequentially BEFORE the loop, so there is zero stdin contention.
async fn trust_gate(
    reader: &Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
    stderr_sink: &Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
    is_tty: bool,
    print: bool,
    config_path: Option<&Path>,
    cwd: &Path,
) -> TrustOutcome {
    // Non-interactive ⇒ proceed exactly as today, no prompt, no byte consumed.
    if !should_prompt_interactively(is_tty, print) {
        return TrustOutcome::Proceed;
    }
    // No config path to persist/consult, or already trusted (parent-walk) ⇒
    // proceed without prompting (no byte consumed).
    let Some(cfg_path) = config_path else {
        return TrustOutcome::Proceed;
    };
    if migrations::global_config::check_has_trust_dialog_accepted(cfg_path, cwd) {
        return TrustOutcome::Proceed;
    }

    // Render the byte-locked dialog to stderr.
    {
        let prompt = format_trust_prompt(cwd);
        let mut err = stderr_sink.lock().await;
        if err.write_all(prompt.as_bytes()).await.is_err() || err.flush().await.is_err() {
            // Cannot render the dialog ⇒ fail-safe to DECLINE (never silently
            // trust an unprompted directory).
            return TrustOutcome::Decline;
        }
    }

    // Read ONE line off the shared reader.
    let mut line = String::new();
    let n = {
        let mut guard = reader.lock().await;
        match guard.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => 0,
        }
    };
    if n == 0 {
        // EOF / read error ⇒ decline (claude-code "No, exit" default).
        return TrustOutcome::Decline;
    }
    if parse_trust_input(&line) {
        // "Yes, I trust this folder" → record acceptance via the shared accept
        // branch (`TrustDialog.tsx:162,174-177`): SESSION-ONLY in-memory when
        // `cwd == $HOME`, else persisted to disk best-effort.
        migrations::global_config::record_trust_accept(cfg_path, cwd);
        TrustOutcome::Proceed
    } else {
        TrustOutcome::Decline
    }
}

/// Map a REPL `ended_via` discriminator to the claude-code `SessionEnd`
/// `reason` (`ExitReason`) string fired at teardown.
///
/// Byte-faithful to claude-code: `/exit`, `/quit`, Ctrl+C (double-press), and
/// Ctrl+D all funnel through the unified `handleExit` →
/// `exit.tsx`/`ExitFlow.tsx` → `gracefulShutdown(0, 'prompt_input_exit')`
/// (`commands/exit/exit.tsx:30`, `components/ExitFlow.tsx:26` — the exit.tsx
/// comment is explicit: "Covers /exit, /quit, ctrl+c, ctrl+d"). So every clean
/// user-initiated REPL exit emits `ExitReason = "prompt_input_exit"`. The
/// `"other"` `ExitReason` (`gracefulShutdown`'s default, `coreTypes.ts:55`
/// `EXIT_REASONS`) is reserved for error / signal / non-user-initiated
/// shutdowns (sandbox failure, SSH drop, unhandled rejection), none of which
/// correspond to a clean `StepOutcome` exit path — so all three of our
/// `ended_via` values map to `prompt_input_exit`.
fn session_end_reason(ended_via: &str) -> &'static str {
    match ended_via {
        // Ctrl+D (EOF), `/exit`/`/quit`, and double-Ctrl+C all route through
        // claude-code's `handleExit` → `gracefulShutdown(0, "prompt_input_exit")`.
        "eof" | "exit_command" | "double_sigint" => "prompt_input_exit",
        // Defensive default mirrors `gracefulShutdown`'s `ExitReason = "other"`
        // fallback for any non-user-initiated teardown.
        _ => "other",
    }
}

/// REPL entry point.  Builds the runtime, runs the prompt loop, emits
/// `tengu_repl_session_started` / `tengu_repl_session_ended` telemetry.
pub async fn run_repl(argv: &Argv) -> i32 {
    // Build the output sink first — it is constructor-injected into the
    // orchestrator via `build_runtime`.
    let sink: Arc<dyn OutputSink> = if argv.json {
        Arc::new(JsonSink::new(SessionId::new()))
    } else {
        Arc::new(PlainSink::new())
    };
    let adapter: Arc<dyn OutputStream> = Arc::new(SinkAdapter::new(sink.clone()));

    // (Task 8) The stdio-REPL path mints its own runtime here, so it re-derives
    // the CLI-resolved session permission mode via the shared resolver
    // (`initialPermissionModeFromCLI`) and threads it into the orchestrator —
    // instead of the previously-hardwired `Default`. The bypass-safety guard /
    // notice already ran once in `run_cli` before dispatch (a refusal exits
    // before this fn is reached), so we take only the mode here.
    let (permission_mode, _notice) = crate::resolve_permission_mode(argv);

    // ONE shared `BufReader<Stdin>` over fd 0, used by BOTH the REPL prompt loop
    // (`step`) and — on the interactive TTY path — the injected
    // `InteractivePromptingGate`. A single buffer means type-ahead bytes are
    // never stranded behind a second `BufReader`, and only one
    // `tokio::io::stdin()` exists (no double blocking-reader-thread race).
    let stdin_reader: Arc<Mutex<BufReader<Stdin>>> = Arc::new(Mutex::new(BufReader::new(stdin())));

    // (trust dialog) Startup trust GATE — parity `screens/REPL.tsx` +
    // `components/TrustDialog/TrustDialog.tsx`: before ANY tool/hook/plugin can
    // run (i.e. before the runtime is built and the loop entered), show the
    // one-time trust dialog for an un-trusted directory. Decline ⇒ exit with
    // claude-code's "No, exit" code (`gracefulShutdownSync(1)`,
    // `TrustDialog.tsx:158-160`) WITHOUT building the runtime. Accept persists
    // `hasTrustDialogAccepted` and proceeds (so the runtime is built — and any
    // tool/hook/plugin reached — only after this gate clears). Non-TTY /
    // `--print` / already-accepted ⇒ no
    // prompt, byte-identical to today. The read shares the SAME `stdin_reader`
    // the loop uses below — sequential, before the loop, so no contention.
    let trust_stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>> = Arc::new(Mutex::new(stderr()));
    let trust_reader: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>> = stdin_reader.clone();
    let trust_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let trust_cfg = migrations::global_config::global_config_path();
    if trust_gate(
        &trust_reader,
        &trust_stderr,
        std::io::stdin().is_terminal(),
        argv.print,
        trust_cfg.as_deref(),
        &trust_cwd,
    )
    .await
        == TrustOutcome::Decline
    {
        // "No, exit" → `gracefulShutdownSync(1)` (`TrustDialog.tsx:159`,
        // `gracefulShutdown.ts:347` sets `process.exitCode = 1`). Exit BEFORE
        // building the runtime.
        return exit_codes::RUNTIME_ERROR;
    }

    // Interactive (TTY, non-print) REPL injects the y/n permission gate sharing
    // the stdin reader; the piped/CI path stays byte-identical (no gate).
    let runtime = if should_prompt_interactively(std::io::stdin().is_terminal(), argv.print) {
        // Resolve config ONCE so the injected gate and the engine build from the
        // SAME cfg (mirrors `build_runtime_for_tui`).
        let mut cfg = crate::init::resolve_desktop_config(argv, permission_mode);
        let shared: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>> = stdin_reader.clone();
        let gate = permission::InteractivePromptingGate::new(
            shared,
            Arc::new(Mutex::new(stderr())),
        );
        cfg.injected_permission_gate =
            Some(Arc::new(gate) as Arc<dyn permission::gate::PermissionGate>);
        crate::init::build_runtime_from_config(cfg, adapter).await
    } else {
        crate::init::build_runtime(argv, adapter, permission_mode).await
    };
    let runtime = match runtime {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lingxi-cli: {e}");
            return exit_codes::RUNTIME_ERROR;
        }
    };

    // Retrieve the session id for telemetry via the OrchestratorHandle trait.
    let handle: Arc<dyn OrchestratorHandle> = runtime.orchestrator.clone();
    let session_id = handle.current_session_id().await;
    let started = Instant::now();
    let mut turn_count: u32 = 0;

    tracing::info!(
        event = telemetry::tengu::orchestrator::REPL_SESSION_STARTED,
        session_id = %session_id,
        started_at = %chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );

    let sigint = SigintSource::spawn();
    let mut err_writer = stderr();

    let orch = runtime.orchestrator.clone();

    // hooks (runtime lifecycle): idle-prompt `Notification` wiring (parity
    // `screens/REPL.tsx:3930-3940`). Resolve the cheap registration gate ONCE
    // at startup — when no `Notification` hook subscribes, the notifier's timer
    // is gated off (`arm_timer() == None`) so the repl input loop never arms a
    // useless timer (mirrors the `ConfigChange` watcher gate in engine-desktop).
    // The concrete `Arc<ConversationOrchestrator>` is required for
    // `fire_notification` (the `OrchestratorHandle` trait does not expose it),
    // and it is in scope here exactly like the `SessionEnd` fire below.
    let notif_armed = orch.has_notification_hook().await;
    let idle_notifier = crate::idle_notify::OrchestratorIdleNotifier::new(orch.clone(), notif_armed);

    let ended_via;
    let exit_code;
    loop {
        let orch_for_turn = orch.clone();
        let run_turn_fn =
            move |prompt: String,
                  token: CancellationToken|
                  -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>> {
                let o = orch_for_turn.clone();
                Box::pin(async move { o.run_turn_with_cancel(&prompt, token).await })
            };

        let outcome = step(
            stdin_reader.clone() as Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
            &mut err_writer,
            &runtime.dispatcher,
            handle.clone(),
            sink.clone(),
            &sigint,
            Some(&idle_notifier),
            run_turn_fn,
        )
        .await;

        turn_count += 1;
        match outcome {
            StepOutcome::Continue => continue,
            StepOutcome::Eof => {
                ended_via = "eof";
                exit_code = exit_codes::SUCCESS;
                break;
            }
            StepOutcome::DoubleSigintExit => {
                ended_via = "double_sigint";
                exit_code = exit_codes::SIGINT;
                break;
            }
            StepOutcome::ExitCommand => {
                ended_via = "exit_command";
                exit_code = exit_codes::SUCCESS;
                break;
            }
        }
    }

    // hooks (session lifecycle): fire `SessionEnd` at the CLI session-end seam,
    // mirroring how `engine_desktop::build` fires `fire_session_start("startup")`
    // at boot. `orch` is the CONCRETE `Arc<ConversationOrchestrator>` (the
    // `OrchestratorHandle` trait does NOT expose `fire_session_end`), so we fire
    // here where the concrete type is still in scope, AFTER the repl loop breaks.
    // Best-effort, like `fire_session_start`: a failing SessionEnd hook never
    // breaks shutdown (the helper discards each hook aggregate). The `reason` is
    // the byte-faithful claude-code `ExitReason` for this exit path.
    orch.fire_session_end(session_end_reason(ended_via)).await;

    tracing::info!(
        event = telemetry::tengu::orchestrator::REPL_SESSION_ENDED,
        session_id = %session_id,
        duration_secs = started.elapsed().as_secs(),
        turn_count = turn_count,
        ended_via = ended_via,
    );

    exit_code
}

#[cfg(test)]
mod tests {
    use super::{
        format_trust_prompt, parse_trust_input, session_end_reason, should_prompt_interactively,
        trust_gate, TrustOutcome,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tokio::io::{
        duplex, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader,
    };
    use tokio::sync::Mutex;

    /// Interactive prompting is enabled ONLY for a TTY that is not in `--print`
    /// mode. A non-TTY (piped/CI) or a print session never injects the gate.
    #[test]
    fn should_prompt_interactively_truth_table() {
        assert!(
            should_prompt_interactively(true, false),
            "TTY + not print → prompt interactively"
        );
        assert!(
            !should_prompt_interactively(false, false),
            "non-TTY → never prompt (piped/CI auto-allow)"
        );
        assert!(
            !should_prompt_interactively(true, true),
            "print mode → never prompt"
        );
    }

    /// Every clean REPL `StepOutcome` exit path (`eof` = Ctrl+D,
    /// `exit_command` = `/exit`/`/quit`, `double_sigint` = double-Ctrl+C) maps
    /// to the claude-code `ExitReason` `"prompt_input_exit"`, because all three
    /// funnel through claude-code's unified `handleExit` →
    /// `gracefulShutdown(0, "prompt_input_exit")` (`commands/exit/exit.tsx:30`,
    /// `components/ExitFlow.tsx:26`).
    #[test]
    fn clean_exit_paths_map_to_prompt_input_exit() {
        assert_eq!(session_end_reason("eof"), "prompt_input_exit");
        assert_eq!(session_end_reason("exit_command"), "prompt_input_exit");
        assert_eq!(session_end_reason("double_sigint"), "prompt_input_exit");
    }

    /// Any unrecognised discriminator falls back to `gracefulShutdown`'s default
    /// `ExitReason = "other"` (`utils/gracefulShutdown.ts:393`).
    #[test]
    fn unknown_reason_falls_back_to_other() {
        assert_eq!(session_end_reason("something_else"), "other");
        assert_eq!(session_end_reason(""), "other");
    }

    // ---- startup trust GATE (design doc §2) ----

    /// A scripted shared `BufReader` over a `duplex` end, upcast to the same
    /// `Arc<Mutex<dyn AsyncBufRead>>` trait object the production loop hands the
    /// gate. Returns the reader and a sink draining the gate's stderr writes.
    fn scripted_reader(bytes: &[u8]) -> Arc<Mutex<dyn AsyncBufRead + Send + Unpin>> {
        let (mut writer, client) = duplex(1024);
        let bytes = bytes.to_vec();
        tokio::spawn(async move {
            let _ = writer.write_all(&bytes).await;
            // drop(writer) on task end closes the pipe ⇒ EOF after the bytes.
        });
        Arc::new(Mutex::new(BufReader::new(client)))
    }

    fn null_stderr() -> Arc<Mutex<dyn AsyncWrite + Send + Unpin>> {
        let (out_end, _drain) = duplex(4096);
        // Keep the drain alive for the test's lifetime by leaking it — the
        // gate only ever writes a few hundred bytes which fit the 4096 buffer.
        std::mem::forget(_drain);
        Arc::new(Mutex::new(out_end))
    }

    /// A temp `~/.lingxi.json` config path + an un-trusted cwd, isolated from
    /// the real home via a unique tempdir (no `env_lock` needed: we pass the
    /// path explicitly to the gate, never reading `HOME`).
    fn temp_cfg_and_cwd() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = dir.path().join(".lingxi.json");
        // A real, canonicalizable cwd inside the tempdir.
        let cwd = dir.path().join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");
        (dir, cfg, cwd)
    }

    /// not-accepted + scripted `"y\n"` ⇒ marks accepted AND proceeds.
    #[tokio::test]
    async fn not_accepted_scripted_y_marks_and_proceeds() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        assert!(
            !migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd),
            "precondition: not yet trusted"
        );
        let reader = scripted_reader(b"y\n");
        let outcome = trust_gate(&reader, &null_stderr(), true, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Proceed);
        assert!(
            migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd),
            "y must persist hasTrustDialogAccepted"
        );
    }

    /// scripted `"n\n"` ⇒ declines, store NOT marked (caller returns
    /// RUNTIME_ERROR without building the runtime).
    #[tokio::test]
    async fn scripted_n_declines_without_mark() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        let reader = scripted_reader(b"n\n");
        let outcome = trust_gate(&reader, &null_stderr(), true, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Decline);
        assert!(
            !migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd),
            "decline must NOT persist trust"
        );
    }

    /// EOF (empty/closed reader, `read_line` returns 0) ⇒ decline, no mark.
    #[tokio::test]
    async fn eof_declines_without_mark() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        let reader = scripted_reader(b"");
        let outcome = trust_gate(&reader, &null_stderr(), true, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Decline);
        assert!(!migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd));
    }

    /// Empty line (just Enter) ⇒ decline (deny-default `[y/N]`), no mark.
    #[tokio::test]
    async fn empty_line_declines() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        let reader = scripted_reader(b"\n");
        let outcome = trust_gate(&reader, &null_stderr(), true, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Decline);
        assert!(!migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd));
    }

    /// Already-accepted ⇒ proceed with NO byte consumed (a follow-up line is
    /// still readable off the SAME shared reader — proves no prompt happened).
    #[tokio::test]
    async fn already_accepted_no_byte_consumed() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        migrations::global_config::mark_trust_dialog_accepted(&cfg, &cwd).expect("pre-mark");
        let reader = scripted_reader(b"keep\n");
        let outcome = trust_gate(&reader, &null_stderr(), true, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Proceed);
        let mut follow = String::new();
        let mut guard = reader.lock().await;
        let n = guard.read_line(&mut follow).await.unwrap();
        assert_eq!(n, "keep\n".len(), "no byte should have been consumed");
        assert_eq!(follow, "keep\n");
    }

    /// Non-TTY ⇒ proceed, no prompt, no byte consumed, store untouched.
    #[tokio::test]
    async fn non_tty_proceeds_no_prompt() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        let reader = scripted_reader(b"keep\n");
        let outcome = trust_gate(&reader, &null_stderr(), false, false, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Proceed);
        assert!(!migrations::global_config::check_has_trust_dialog_accepted(&cfg, &cwd));
        let mut follow = String::new();
        let mut guard = reader.lock().await;
        let n = guard.read_line(&mut follow).await.unwrap();
        assert_eq!(n, "keep\n".len());
        assert_eq!(follow, "keep\n");
    }

    /// `--print` (even on a TTY) ⇒ proceed, no prompt, no byte consumed.
    #[tokio::test]
    async fn print_mode_proceeds() {
        let (_dir, cfg, cwd) = temp_cfg_and_cwd();
        let reader = scripted_reader(b"keep\n");
        let outcome = trust_gate(&reader, &null_stderr(), true, true, Some(&cfg), &cwd).await;
        assert_eq!(outcome, TrustOutcome::Proceed);
        let mut follow = String::new();
        let mut guard = reader.lock().await;
        let n = guard.read_line(&mut follow).await.unwrap();
        assert_eq!(n, "keep\n".len());
    }

    /// No resolvable config path ⇒ proceed, no prompt, no byte consumed.
    #[tokio::test]
    async fn no_config_path_proceeds() {
        let reader = scripted_reader(b"keep\n");
        let outcome =
            trust_gate(&reader, &null_stderr(), true, false, None, Path::new("/tmp")).await;
        assert_eq!(outcome, TrustOutcome::Proceed);
        let mut follow = String::new();
        let mut guard = reader.lock().await;
        let n = guard.read_line(&mut follow).await.unwrap();
        assert_eq!(n, "keep\n".len());
    }

    /// The flat trust prompt is byte-locked to claude-code's `TrustDialog`
    /// copy (title / safety-check / capabilities / security-link / option
    /// labels), with the cwd interpolated.
    #[test]
    fn format_trust_prompt_byte_locked() {
        let s = format_trust_prompt(Path::new("/home/dev/project"));
        assert_eq!(
            s.as_bytes(),
            b"Accessing workspace:\n\n/home/dev/project\n\nQuick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your team). If not, take a moment to review what's in this folder first.\nLingXi'll be able to read, edit, and execute files here.\n\nSecurity guide: https://code.claude.com/docs/en/security\n\n  Yes, I trust this folder\n  No, exit\n[y/N] " as &[u8]
        );
    }

    /// y/Y/yes/YES (trimmed, case-insensitive) accept; everything else —
    /// n/no/empty/garbage — declines.
    #[test]
    fn parse_trust_input_accepts_only_yes_variants() {
        assert!(parse_trust_input("y\n"));
        assert!(parse_trust_input("Y\n"));
        assert!(parse_trust_input("yes\n"));
        assert!(parse_trust_input("YES\r\n"));
        assert!(parse_trust_input("  yes  \n"));
        assert!(!parse_trust_input("n\n"));
        assert!(!parse_trust_input("no\n"));
        assert!(!parse_trust_input("\n"));
        assert!(!parse_trust_input(""));
        assert!(!parse_trust_input("maybe\n"));
        assert!(!parse_trust_input("yy\n"));
    }
}
