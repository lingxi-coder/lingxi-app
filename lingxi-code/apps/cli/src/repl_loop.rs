//! Testable REPL loop body.  Decoupled from real stdin/stderr so unit tests
//! can drive it with `tokio::io::duplex` pipes.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 4.

use crate::idle_notify::IdleNotifier;
use crate::output::OutputSink;
use crate::sigint::SigintSource;
use futures::future::BoxFuture;
use orchestrator::{OrchestratorError, TurnOutcome};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use traits::{OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};

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
///
/// `idle` is the additive idle-prompt seam (parity `screens/REPL.tsx:3930-3940`):
/// when `Some`, the prompt's input-read is RACED against an idle timer for the
/// current post-turn idle period; if the timer elapses BEFORE input arrives the
/// `Notification {idle_prompt}` hook is fired ONCE and the read keeps going.
/// `None` (or a notifier that gates its timer off) preserves the exact prior
/// input behaviour. The timer is purely additive — it never drives the loop,
/// never drops the in-flight read (`read_line` is NOT cancel-safe; see the
/// input `select!`), and a failing fire can never affect input.
// `step` is the REPL iteration seam with all its dependencies injected for
// testability (stdin/stderr, dispatcher, handle, sink, sigint, idle notifier,
// run-turn). The idle notifier is the 8th injected arg; the list is cohesive
// (one iteration's collaborators), so the count is intentional.
#[allow(clippy::too_many_arguments)]
pub async fn step<W>(
    stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
    stderr: &mut W,
    dispatcher: &dyn SlashCommandDispatcher,
    handle: Arc<dyn OrchestratorHandle>,
    sink: Arc<dyn OutputSink>,
    sigint: &SigintSource,
    idle: Option<&dyn IdleNotifier>,
    run_turn_fn: impl Fn(
        String,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> StepOutcome
where
    W: tokio::io::AsyncWrite + Unpin,
{
    // 1. Print the "> " prompt to stderr (L1 + L2 byte-lock from T0).
    let _ = stderr.write_all(b"> ").await;
    let _ = stderr.flush().await;

    // 2. Read a line, racing against SIGINT and (additively) the idle timer.
    //
    // SHARED STDIN: `stdin` is the SINGLE `BufReader<Stdin>` shared with the
    // injected `InteractivePromptingGate`. We lock it ONLY for the duration of
    // the prompt read (between turns) and DROP the guard before invoking
    // `run_turn_fn`, so the gate can lock the SAME reader to prompt `y/n`
    // during the turn. The two phases are strictly sequential within one loop
    // iteration, so the `tokio::sync::Mutex` is never contended and never
    // deadlocks.
    //
    // CANCEL-SAFETY: `AsyncBufReadExt::read_line` is NOT cancel-safe — dropping
    // its future mid-read can lose buffered bytes. We therefore pin it ONCE and
    // re-await the SAME future across `select!` iterations: only the SIGINT arm
    // and a completed read leave this loop (the SIGINT drop is the pre-existing,
    // accepted behaviour). The idle arm fires the notification ONCE and then
    // loops back to re-await the in-flight read, so the timer NEVER drops it.
    // The idle timer is armed fresh for THIS post-turn idle period (parity:
    // `clearTimeout` + re-`setTimeout` per render, `screens/REPL.tsx:3920-3941`)
    // and `idle_fired` makes it at-most-once per idle period.
    let mut line = String::new();

    // Acquire the shared-stdin lock for the prompt read ONLY. The guard is
    // dropped at the end of this block (`read_scope`) — BEFORE `run_turn_fn` —
    // so the gate can lock the SAME reader during the turn.
    let read_scope: Option<StepOutcome> = {
        let mut guard = stdin.lock().await;

        let read_fut = guard.read_line(&mut line);
        tokio::pin!(read_fut);

        // Arm the idle timer only when a notifier is present AND not gated off
        // (`arm_timer() == None` ⇒ no `Notification` hook registered). A
        // `pending()` future is selected over only when no real timer exists, so
        // the no-hook path is byte-identical to the prior two-arm `select!`.
        let idle_timer = idle.and_then(IdleNotifier::arm_timer);
        let mut idle_timer = match idle_timer {
            Some(fut) => fut,
            None => Box::pin(std::future::pending::<()>()),
        };
        let mut idle_fired = false;

        loop {
            tokio::select! {
                result = &mut read_fut => {
                    match result {
                        Ok(0) => {
                            // EOF (Ctrl+D) — L3 byte-lock: print "\n" to stdout.
                            let _ = tokio::io::stdout().write_all(b"\n").await;
                            break Some(StepOutcome::Eof);
                        }
                        Ok(_bytes) => break None,
                        Err(_) => {
                            // IO error — try again on next iteration.
                            break Some(StepOutcome::Continue);
                        }
                    }
                }
                () = sigint.wait() => {
                    // SIGINT at the idle prompt.
                    if sigint.take_idle_armed() {
                        // Second Ctrl+C within the 2-second window → exit 130.
                        break Some(StepOutcome::DoubleSigintExit);
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
                    break Some(StepOutcome::Continue);
                }
                () = &mut idle_timer, if !idle_fired => {
                    // Idle threshold elapsed before input arrived. Fire the
                    // `Notification {idle_prompt}` hook ONCE, best-effort — it can
                    // never affect the input read, which is still in flight — then
                    // loop back to keep awaiting the SAME pinned `read_line`.
                    idle_fired = true;
                    if let Some(notifier) = idle {
                        notifier.fire().await;
                    }
                    // Re-arm with a never-resolving timer so the disabled idle arm
                    // is cheap on subsequent loop turns (at-most-once per period).
                    idle_timer = Box::pin(std::future::pending::<()>());
                }
            }
        }
        // `guard` (and the pinned `read_fut` borrowing it) drop HERE — the
        // shared reader is unlocked before any turn runs.
    };

    // A terminal read outcome (Eof / DoubleSigintExit / first-idle-Ctrl+C /
    // IO error) returns now; otherwise `line` holds the prompt input.
    if let Some(outcome) = read_scope {
        return outcome;
    }

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

    // ---- idle-prompt Notification race tests (parity REPL.tsx:3930-3940) ----
    //
    // These drive the REAL `step` input `select!` loop over `tokio::io::duplex`
    // pipes, with a DETERMINISTIC injected `IdleNotifier`: the timer is either
    // an already-resolved future (fires immediately) or a never-resolving one
    // (never fires) — NO real wall-clock sleeps. Ordering is made deterministic
    // by withholding stdin until after the timer has had its chance to win.

    use crate::idle_notify::IdleNotifier;
    use crate::output::PlainSink;
    use futures::future::BoxFuture;
    use orchestrator::test_support::MockOrchestratorHandle;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{duplex, AsyncBufRead, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;
    use traits::{SlashCommandDispatcher, SlashDispatchResult};

    /// Wrap a duplex client end in the shared `Arc<Mutex<BufReader<_>>>` the
    /// refactored `step` expects (upcast to the `dyn AsyncBufRead` trait object).
    fn shared_stdin<R>(client: R) -> Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
    {
        Arc::new(Mutex::new(BufReader::new(client)))
    }

    /// Dispatcher stub — `step` never calls it for plain (non-slash) input, and
    /// these tests feed only plain input, so this is unreachable in practice.
    struct NoopDispatcher;
    #[async_trait::async_trait]
    impl SlashCommandDispatcher for NoopDispatcher {
        async fn dispatch(&self, _raw: &str) -> SlashDispatchResult {
            SlashDispatchResult::NotASlashCommand
        }
    }

    /// Deterministic notifier: `arm_timer` yields a controllable future and
    /// `fire` bumps a shared counter so the test asserts at-most-once.
    struct TestNotifier {
        /// `true` ⇒ `arm_timer` returns an immediately-ready future (fires);
        /// `false` ⇒ a never-resolving future (does not fire).
        fire_immediately: bool,
        /// `false` ⇒ `arm_timer` returns `None` (gated off / no hook).
        armed: bool,
        fires: Arc<AtomicUsize>,
    }
    impl IdleNotifier for TestNotifier {
        fn arm_timer(&self) -> Option<BoxFuture<'static, ()>> {
            if !self.armed {
                return None;
            }
            if self.fire_immediately {
                Some(Box::pin(std::future::ready(())))
            } else {
                Some(Box::pin(std::future::pending::<()>()))
            }
        }
        fn fire(&self) -> BoxFuture<'_, ()> {
            self.fires.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::ready(()))
        }
    }

    /// `run_turn_fn` that records nothing and returns `EndTurn` immediately.
    fn end_turn_fn(
    ) -> impl Fn(String, CancellationToken) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
    {
        |_prompt, _token| Box::pin(async { Ok(TurnOutcome::EndTurn) })
    }

    /// Run `step` over a duplex stdin pipe whose line is written only AFTER the
    /// idle timer has had its first poll, so an immediate timer wins
    /// deterministically. Returns the `StepOutcome`.
    async fn run_step_with(notifier: &TestNotifier, line: &str) -> StepOutcome {
        // 64-byte duplex: server end = our writer, client end = step's stdin.
        let (mut writer, client) = duplex(64);
        let stdin = shared_stdin(client);
        let mut sink_buf = Vec::new();
        // stderr sink for the "> " prompt — a Vec is fine (no real terminal).
        let stderr = &mut sink_buf;

        let dispatcher = NoopDispatcher;
        let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new());
        let sigint = SigintSource::spawn();

        let line_owned = format!("{line}\n");
        // Writer task: yield a few times so `step`'s first `select!` poll sees an
        // empty pipe (read pending) and the immediate idle timer wins; THEN send
        // the line so the read completes and `step` returns.
        let writer_task = tokio::spawn(async move {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let _ = writer.write_all(line_owned.as_bytes()).await;
            let _ = writer.flush().await;
            // Keep the write half open until step has consumed the line.
            writer
        });

        let outcome = step(
            stdin,
            stderr,
            &dispatcher,
            handle,
            sink,
            &sigint,
            Some(notifier),
            end_turn_fn(),
        )
        .await;

        let _ = writer_task.await;
        outcome
    }

    /// (a) Idle past threshold fires the Notification exactly once, then the
    /// input still drives the loop to a normal `Continue`.
    #[tokio::test]
    async fn idle_timer_elapsed_fires_notification_once_then_input_drives_loop() {
        let fires = Arc::new(AtomicUsize::new(0));
        let notifier = TestNotifier {
            fire_immediately: true,
            armed: true,
            fires: fires.clone(),
        };
        let outcome = run_step_with(&notifier, "hello").await;
        assert_eq!(
            outcome,
            StepOutcome::Continue,
            "input must still drive the loop to Continue after an idle fire"
        );
        assert_eq!(
            fires.load(Ordering::SeqCst),
            1,
            "idle past threshold must fire the Notification exactly ONCE"
        );
    }

    /// (b) Input arriving before the threshold does NOT fire (never-resolving
    /// timer stands in for "threshold not yet reached").
    #[tokio::test]
    async fn input_before_threshold_does_not_fire() {
        let fires = Arc::new(AtomicUsize::new(0));
        let notifier = TestNotifier {
            fire_immediately: false, // timer never resolves
            armed: true,
            fires: fires.clone(),
        };
        let outcome = run_step_with(&notifier, "hello").await;
        assert_eq!(outcome, StepOutcome::Continue);
        assert_eq!(
            fires.load(Ordering::SeqCst),
            0,
            "input before the idle threshold must NOT fire the Notification"
        );
    }

    /// (d) No notifier wired (or gated off) is a strict no-op AND preserves the
    /// exact prior input behaviour (Continue), proving the timer is additive.
    #[tokio::test]
    async fn no_notifier_is_a_noop() {
        // `None` notifier — the pre-existing two-arm behaviour.
        let (mut writer, client) = duplex(64);
        let stdin = shared_stdin(client);
        let mut stderr = Vec::new();
        let dispatcher = NoopDispatcher;
        let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new());
        let sigint = SigintSource::spawn();
        let writer_task = tokio::spawn(async move {
            let _ = writer.write_all(b"hello\n").await;
            let _ = writer.flush().await;
            writer
        });
        let outcome = step(
            stdin,
            &mut stderr,
            &dispatcher,
            handle,
            sink,
            &sigint,
            None, // no idle notifier
            end_turn_fn(),
        )
        .await;
        let _ = writer_task.await;
        assert_eq!(outcome, StepOutcome::Continue);
    }

    /// (d') A notifier that gates its timer off (`arm_timer() == None`, i.e. no
    /// Notification hook registered) never fires, even though a notifier is
    /// wired.
    #[tokio::test]
    async fn gated_off_notifier_never_fires() {
        let fires = Arc::new(AtomicUsize::new(0));
        let notifier = TestNotifier {
            fire_immediately: true, // would fire, but armed=false gates the timer off
            armed: false,
            fires: fires.clone(),
        };
        let outcome = run_step_with(&notifier, "hello").await;
        assert_eq!(outcome, StepOutcome::Continue);
        assert_eq!(
            fires.load(Ordering::SeqCst),
            0,
            "a gated-off notifier (no Notification hook) must never fire"
        );
    }

    /// LOCK-RELEASE: `step` must DROP the shared-stdin guard BEFORE invoking
    /// `run_turn_fn`. A fake `run_turn_fn` locks the SAME shared reader and
    /// reads a pre-queued `"y\n"` (simulating the gate prompting during the
    /// turn). If `step` still held the guard, this would deadlock; success
    /// proves the guard was released and the byte is available to the gate.
    #[tokio::test]
    async fn step_releases_stdin_lock_before_running_turn() {
        use tokio::io::AsyncBufReadExt;

        // Queue BOTH the prompt line `"go\n"` (consumed by step's read) and the
        // gate's `"y\n"` answer (consumed by run_turn_fn) up front.
        let (mut writer, client) = duplex(64);
        writer.write_all(b"go\ny\n").await.unwrap();
        let shared = shared_stdin(client);

        let mut stderr = Vec::new();
        let dispatcher = NoopDispatcher;
        let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new());
        let sigint = SigintSource::spawn();

        // `run_turn_fn` re-locks the SAME shared reader and reads the queued
        // `"y\n"` — exactly what the injected gate does during a turn.
        let answered = Arc::new(AtomicUsize::new(0));
        let stdin_for_turn = shared.clone();
        let answered_for_turn = answered.clone();
        let run_turn_fn = move |_prompt: String, _token: CancellationToken| {
            let r = stdin_for_turn.clone();
            let a = answered_for_turn.clone();
            Box::pin(async move {
                let mut buf = String::new();
                let mut guard = r.lock().await;
                let n = guard.read_line(&mut buf).await.unwrap();
                assert_eq!(n, "y\n".len());
                assert_eq!(buf, "y\n");
                a.fetch_add(1, Ordering::SeqCst);
                Ok(TurnOutcome::EndTurn)
            }) as BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
        };

        let outcome = step(
            shared,
            &mut stderr,
            &dispatcher,
            handle,
            sink,
            &sigint,
            None,
            run_turn_fn,
        )
        .await;

        let _ = writer;
        assert_eq!(outcome, StepOutcome::Continue);
        assert_eq!(
            answered.load(Ordering::SeqCst),
            1,
            "run_turn_fn must have locked the shared reader and read `y\\n` — \
             proving step dropped the guard before the turn"
        );
    }
}
