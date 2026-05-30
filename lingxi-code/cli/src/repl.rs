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

    let runtime = match crate::init::build_runtime(argv, adapter).await {
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

    tracing::info!(
        event = telemetry::tengu::orchestrator::REPL_SESSION_ENDED,
        session_id = %session_id,
        duration_secs = started.elapsed().as_secs(),
        turn_count = turn_count,
        ended_via = ended_via,
    );

    exit_code
}
