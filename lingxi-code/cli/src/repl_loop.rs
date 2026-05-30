//! Testable REPL loop body.  Decoupled from real stdin/stderr so unit tests
//! can drive it with `tokio::io::duplex` pipes.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 4.

use crate::output::OutputSink;
use crate::sigint::SigintSource;
use futures::future::BoxFuture;
use lingxi_orchestrator::{OrchestratorError, TurnOutcome};
use lingxi_traits::{OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Outcome returned by [`step`] — tells the outer loop what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// Normal completion: loop back to the prompt.
    Continue,
    /// EOF (Ctrl+D) received: persist session and exit 0.
    Eof,
    /// Second Ctrl+C at the idle prompt within 2 seconds: exit 130.
    DoubleSigintExit,
    /// The orchestrator's `should_exit` flag was set (e.g. `/exit`): exit 0.
    ExitCommand,
}

/// Execute one REPL iteration: print prompt, read a line, dispatch.
///
/// Generic over the concrete `AsyncRead` / `AsyncWrite` types so tests can
/// inject `tokio::io::duplex` pipes in place of real stdin/stderr.
///
/// The `run_turn_fn` callback is a `BoxFuture`-returning closure so the
/// caller can borrow-capture the `Arc<ConversationOrchestrator>` without
/// tying the lifetime of the future to the closure's borrow.
pub async fn step<R, W>(
    stdin: &mut BufReader<R>,
    stderr: &mut W,
    dispatcher: &dyn SlashCommandDispatcher,
    handle: Arc<dyn OrchestratorHandle>,
    sink: Arc<dyn OutputSink>,
    sigint: &SigintSource,
    run_turn_fn: impl Fn(
        String,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> StepOutcome
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // 1. Print the "> " prompt to stderr (L1 + L2 byte-lock from T0).
    let _ = stderr.write_all(b"> ").await;
    let _ = stderr.flush().await;

    // 2. Read a line, racing against SIGINT.
    let mut line = String::new();
    let n: usize;

    tokio::select! {
        result = stdin.read_line(&mut line) => {
            match result {
                Ok(0) => {
                    // EOF (Ctrl+D) — L3 byte-lock: print "\n" to stdout.
                    let _ = tokio::io::stdout().write_all(b"\n").await;
                    return StepOutcome::Eof;
                }
                Ok(bytes) => {
                    n = bytes;
                }
                Err(_) => {
                    // IO error — try again on next iteration.
                    return StepOutcome::Continue;
                }
            }
        }
        () = sigint.wait() => {
            // SIGINT at the idle prompt.
            if sigint.take_idle_armed() {
                // Second Ctrl+C within the 2-second window → exit 130.
                return StepOutcome::DoubleSigintExit;
            }
            // First idle Ctrl+C: arm the flag, print "\n", start the
            // 2-second disarm timer.
            sigint.arm_idle();
            let _ = stderr.write_all(b"\n").await;
            let idle_armed = sigint.idle_armed.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                idle_armed.store(false, std::sync::atomic::Ordering::SeqCst);
            });
            return StepOutcome::Continue;
        }
    }

    let _ = n; // bytes read — used implicitly via `line`

    // Strip trailing newline(s).
    let input = line.trim_end_matches('\n').trim_end_matches('\r');
    if input.is_empty() {
        return StepOutcome::Continue;
    }

    // A real line arrived — clear any pending idle-armed state.
    sigint.disarm_idle();

    // 3. Dispatch: slash command vs plain text.
    if input.starts_with('/') {
        let res = dispatcher.dispatch(input).await;
        match res {
            SlashDispatchResult::Handled { display }
            | SlashDispatchResult::Unknown { display, .. } => {
                sink.command_output(input, &display).await;
            }
            SlashDispatchResult::NotASlashCommand => {
                // Dispatcher contract: won't happen for "/" prefixed input.
            }
        }
        if handle.current_should_exit().await {
            return StepOutcome::ExitCommand;
        }
        return StepOutcome::Continue;
    }

    // 4. Plain text → orchestrator turn.
    let token = CancellationToken::new();
    // Arm the SIGINT watcher; the JoinHandle drops (and aborts the task)
    // when this scope exits, ensuring the next iteration gets a fresh one.
    let _sigint_guard = sigint.arm_for_turn(token.clone());
    sink.turn_start().await;
    let outcome = run_turn_fn(input.to_string(), token).await;
    match outcome {
        Ok(TurnOutcome::EndTurn) => {}
        Ok(TurnOutcome::MaxTurns) => {
            sink.text("[turn ended: reached MAX_TURNS_PER_CONVERSATION]\n")
                .await;
        }
        Ok(TurnOutcome::Cancelled) => {
            // L5 byte-lock from T0 step 2.
            let _ = stderr
                .write_all(b"^C (turn cancelled; press Ctrl+C again or Ctrl+D to exit)\n")
                .await;
        }
        Err(e) => {
            sink.error("runtime", &e.to_string()).await;
        }
    }
    StepOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the `StepOutcome` enum derives `PartialEq` correctly.
    #[test]
    fn step_outcome_eq_works() {
        assert_eq!(StepOutcome::Continue, StepOutcome::Continue);
        assert_ne!(StepOutcome::Continue, StepOutcome::Eof);
        assert_ne!(StepOutcome::DoubleSigintExit, StepOutcome::ExitCommand);
    }

    /// Verify all four variants are reachable and `Clone` works.
    #[test]
    fn step_outcome_clone() {
        let variants = [
            StepOutcome::Continue,
            StepOutcome::Eof,
            StepOutcome::DoubleSigintExit,
            StepOutcome::ExitCommand,
        ];
        for v in &variants {
            assert_eq!(v, &v.clone());
        }
    }
}
