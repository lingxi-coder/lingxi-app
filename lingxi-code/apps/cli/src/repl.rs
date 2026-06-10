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
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{stderr, stdin, BufReader};
use tokio_util::sync::CancellationToken;
use traits::{OrchestratorHandle, OutputStream};

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
    let runtime = match crate::init::build_runtime(argv, adapter, permission_mode).await {
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
    let mut reader = BufReader::new(stdin());
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
            &mut reader,
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
    use super::session_end_reason;

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
}
