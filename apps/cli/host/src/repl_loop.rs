//! Testable REPL loop body.  Decoupled from real stdin/stderr so unit tests
//! can drive it with `tokio::io::duplex` pipes.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 4.

use crate::idle_notify::IdleNotifier;
use crate::sigint::SigintSource;
use futures::future::BoxFuture;
use harness_runtime::headless::output::OutputSink;
use lingxi_core::host::{OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
use orchestrator::{OrchestratorError, TurnOutcome};
use std::io::IsTerminal;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
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
    ExitCommand {
        /// The raw input, when `/exit` / `/quit` short-circuited BEFORE
        /// dispatch so the host could be offered stay/stop/handoff while the
        /// one-way exit flag was still unset. On that path the handler never
        /// ran and its locked literal was never rendered, so the caller owes
        /// the output once the exit is actually confirmed.
        ///
        /// `None` when an already-dispatched command printed its own output
        /// and merely left `should_exit` set — `/stop` does exactly that, and
        /// must not be re-announced as `/exit`.
        undispatched: Option<String>,
    },
}

/// Host adapter for completion delivery while the prompt is idle.
#[async_trait::async_trait]
pub trait TaskNotificationWake: Send + Sync {
    /// Cancel pending session wakeups when the user interrupts an idle prompt.
    async fn user_interrupt(&self) {}
    /// Only called after a real stdin line selects `/tasks message`.
    async fn send_human_task_message(&self, _task_id: &str, _message: &str) -> Result<(), String> {
        Err("task messaging is unavailable".into())
    }
    async fn wait(&self);
    async fn run(&self, cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError>;
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
/// preserves partially read input across wakeups, and a failing hook never
/// affects input.
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
    step_with_notifications(
        stdin,
        stderr,
        dispatcher,
        handle,
        sink,
        sigint,
        idle,
        None,
        run_turn_fn,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn step_with_notifications<W>(
    stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
    stderr: &mut W,
    dispatcher: &dyn SlashCommandDispatcher,
    handle: Arc<dyn OrchestratorHandle>,
    sink: Arc<dyn OutputSink>,
    sigint: &SigintSource,
    idle: Option<&dyn IdleNotifier>,
    notifications: Option<&dyn TaskNotificationWake>,
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

    // read_until is cancellation-safe: partial bytes stay in `line_bytes`
    // when a completion wakes the host. Release the shared reader before the
    // machine turn so permission tools can acquire the same stdin mutex.
    let mut line_bytes = Vec::new();
    let mut idle_timer = idle
        .and_then(IdleNotifier::arm_timer)
        .unwrap_or_else(|| Box::pin(std::future::pending::<()>()));
    enum InputEvent {
        Read(std::io::Result<usize>),
        Signal,
        Idle,
        Completion,
    }
    loop {
        let event = {
            let mut reader = stdin.lock().await;
            tokio::select! {
                result = reader.read_until(b'\n', &mut line_bytes) => InputEvent::Read(result),
                () = sigint.wait() => InputEvent::Signal,
                () = &mut idle_timer => InputEvent::Idle,
                () = async {
                    match notifications {
                        Some(wake) => wake.wait().await,
                        None => std::future::pending::<()>().await,
                    }
                } => InputEvent::Completion,
            }
        };
        match event {
            InputEvent::Read(Ok(0)) => {
                let _ = tokio::io::stdout().write_all(b"\n").await;
                return StepOutcome::Eof;
            }
            InputEvent::Read(Ok(_)) => break,
            InputEvent::Read(Err(_)) => return StepOutcome::Continue,
            InputEvent::Signal => {
                if let Some(wake) = notifications {
                    wake.user_interrupt().await;
                }
                if sigint.take_idle_armed() {
                    return StepOutcome::DoubleSigintExit;
                }
                sigint.arm_idle();
                let _ = stderr.write_all(b"\n").await;
                let armed = sigint.idle_armed.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    armed.store(false, std::sync::atomic::Ordering::SeqCst);
                });
                return StepOutcome::Continue;
            }
            InputEvent::Idle => {
                if let Some(notifier) = idle {
                    notifier.fire().await;
                }
                idle_timer = Box::pin(std::future::pending::<()>());
            }
            InputEvent::Completion => {
                let cancel = CancellationToken::new();
                let signal = sigint.arm_for_turn(cancel.clone());
                sink.turn_start().await;
                if let Some(wake) = notifications {
                    if let Err(error) = wake.run(cancel).await {
                        sink.error("runtime", &error.to_string()).await;
                    }
                }
                signal.abort();
            }
        }
    }
    let Ok(line) = String::from_utf8(line_bytes) else {
        return StepOutcome::Continue;
    };

    // Strip trailing newline(s).
    let input = line.trim_end_matches('\n').trim_end_matches('\r');
    if input.is_empty() {
        return StepOutcome::Continue;
    }

    // A real line arrived — clear any pending idle-armed state.
    sigint.disarm_idle();

    let (command, args) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
    if command == "/tasks" {
        if let (Some(parsed), Some(host)) = (
            lingxi_core::host::human_task_message::parse(args),
            notifications,
        ) {
            let result = match parsed {
                Ok((task_id, message)) => host
                    .send_human_task_message(task_id, message)
                    .await
                    .map(|()| format!("Message accepted for task {task_id}")),
                Err(error) => Err(error.into()),
            };
            match result {
                Ok(display) => sink.command_output(input, &display).await,
                Err(error) => sink.error("task_message", &error).await,
            }
            return StepOutcome::Continue;
        }
    }

    // Let the host choose stay/stop/handoff before the one-way exit flag is set.
    if notifications.is_some() && matches!(input.trim(), "/exit" | "/quit") {
        return StepOutcome::ExitCommand {
            undispatched: Some(input.trim().to_string()),
        };
    }

    // 3. Dispatch: slash command vs plain text.
    if input.starts_with('/') {
        let columns = if std::io::stdout().is_terminal() {
            crossterm::terminal::size()
                .map(|(columns, _)| columns.max(1))
                .unwrap_or(80)
        } else {
            80
        };
        let context = command_api::ModCommandRunContext {
            origin: serde_json::json!({"kind":"composer"}),
            is_fullscreen: false,
            columns,
        };
        let res = command_api::with_mod_command_context(context, dispatcher.dispatch(input)).await;
        match res {
            SlashDispatchResult::Handled { display }
            | SlashDispatchResult::Unknown { display, .. } => {
                sink.command_output(input, &display).await;
            }
            // A prompt-expanding command (`/loop`, Markdown/Plugin) runs AS a
            // turn: feed the expanded prompt to the model instead of printing it
            // (claude-code `type: "prompt"`). This makes a typed `/loop 5m /foo`
            // actually schedule the cron + execute now.
            SlashDispatchResult::RunAsTurn { prompt } => {
                let token = CancellationToken::new();
                let _sigint_guard = sigint.arm_for_turn(token.clone());
                sink.turn_start().await;
                let outcome = run_turn_fn(prompt, token).await;
                _sigint_guard.abort();
                match outcome {
                    Ok(TurnOutcome::EndTurn) => {}
                    Ok(TurnOutcome::MaxTurns) => {
                        sink.text("[turn ended: reached MAX_TURNS_PER_CONVERSATION]\n")
                            .await;
                    }
                    Ok(TurnOutcome::Cancelled) => {
                        let _ = stderr
                            .write_all(
                                b"^C (turn cancelled; press Ctrl+C again or Ctrl+D to exit)\n",
                            )
                            .await;
                    }
                    Err(e) => {
                        sink.error("runtime", &e.to_string()).await;
                    }
                }
            }
            SlashDispatchResult::NotASlashCommand => {
                // Dispatcher contract: won't happen for "/" prefixed input.
            }
        }
        if handle.current_should_exit().await {
            return StepOutcome::ExitCommand { undispatched: None };
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
    _sigint_guard.abort();
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
        assert_ne!(
            StepOutcome::DoubleSigintExit,
            StepOutcome::ExitCommand { undispatched: None }
        );
        // The two exit shapes are NOT interchangeable: one still owes the
        // locked "Exiting." literal and the other has already printed its own
        // output. An `==` here would let a `/stop` be re-announced as `/exit`.
        assert_ne!(
            StepOutcome::ExitCommand { undispatched: None },
            StepOutcome::ExitCommand {
                undispatched: Some("/exit".to_string())
            }
        );
    }

    /// Verify all four variants are reachable and `Clone` works.
    #[test]
    fn step_outcome_clone() {
        let variants = [
            StepOutcome::Continue,
            StepOutcome::Eof,
            StepOutcome::DoubleSigintExit,
            StepOutcome::ExitCommand { undispatched: None },
            StepOutcome::ExitCommand {
                undispatched: Some("/quit".to_string()),
            },
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
    use futures::future::BoxFuture;
    use harness_runtime::headless::output::PlainSink;
    use lingxi_core::host::{SlashCommandDispatcher, SlashDispatchResult};
    use orchestrator::test_support::MockOrchestratorHandle;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{duplex, AsyncBufRead, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;

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
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
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
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
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
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
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

    /// Dispatcher that returns a fixed `SlashDispatchResult` for any slash input
    /// — lets a test drive the RunAsTurn / Handled arms deterministically.
    struct FixedDispatcher(SlashDispatchResult);
    #[async_trait::async_trait]
    impl SlashCommandDispatcher for FixedDispatcher {
        async fn dispatch(&self, _raw: &str) -> SlashDispatchResult {
            self.0.clone()
        }
    }

    /// Run `step` once with the given dispatcher + a `run_turn_fn` that records
    /// every prompt it is invoked with. Returns `(outcome, recorded_prompts)`.
    async fn run_step_slash(
        dispatcher: &dyn SlashCommandDispatcher,
        line: &str,
    ) -> (StepOutcome, Arc<Mutex<Vec<String>>>) {
        let (mut writer, client) = duplex(64);
        let stdin = shared_stdin(client);
        let mut stderr = Vec::new();
        let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let sigint = SigintSource::spawn();

        let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
        let prompts_for_turn = prompts.clone();
        let run_turn_fn = move |prompt: String, _token: CancellationToken| {
            let p = prompts_for_turn.clone();
            Box::pin(async move {
                p.lock().await.push(prompt);
                Ok(TurnOutcome::EndTurn)
            }) as BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
        };

        let line_owned = format!("{line}\n");
        let writer_task = tokio::spawn(async move {
            let _ = writer.write_all(line_owned.as_bytes()).await;
            let _ = writer.flush().await;
            writer
        });

        let outcome = step(
            stdin,
            &mut stderr,
            dispatcher,
            handle,
            sink,
            &sigint,
            None,
            run_turn_fn,
        )
        .await;
        let _ = writer_task.await;
        (outcome, prompts)
    }

    /// A prompt-expanding command (`/loop`, Markdown/Plugin) → the expanded
    /// prompt is fed to `run_turn` verbatim, not printed. PARITY: claude-code
    /// `getPromptForCommand` result becomes the user turn.
    #[tokio::test]
    async fn run_as_turn_feeds_expanded_prompt_to_run_turn() {
        let dispatcher = FixedDispatcher(SlashDispatchResult::RunAsTurn {
            prompt: "EXPANDED /loop body".to_string(),
        });
        let (outcome, prompts) = run_step_slash(&dispatcher, "/loop 5m /foo").await;
        assert_eq!(outcome, StepOutcome::Continue);
        let recorded = prompts.lock().await.clone();
        assert_eq!(
            recorded,
            vec!["EXPANDED /loop body".to_string()],
            "RunAsTurn must invoke run_turn_fn with the expanded prompt exactly once"
        );
    }

    /// A display-only builtin (`/help`, `/model`) stays display-only — it must
    /// NOT run a turn.
    #[tokio::test]
    async fn handled_builtin_stays_display_only() {
        let dispatcher = FixedDispatcher(SlashDispatchResult::Handled {
            display: "Available commands: ...".to_string(),
        });
        let (outcome, prompts) = run_step_slash(&dispatcher, "/help").await;
        assert_eq!(outcome, StepOutcome::Continue);
        assert!(
            prompts.lock().await.is_empty(),
            "a Handled (display-only) command must NOT invoke run_turn_fn"
        );
    }
    struct TestTaskWake {
        ready: tokio::sync::Notify,
        ran: tokio::sync::Notify,
        stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl TaskNotificationWake for TestTaskWake {
        async fn wait(&self) {
            self.ready.notified().await;
        }
        async fn run(&self, _cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError> {
            assert!(
                self.stdin.try_lock().is_ok(),
                "machine tools must be able to read permission input"
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.ran.notify_one();
            Ok(TurnOutcome::EndTurn)
        }
    }

    #[tokio::test]
    async fn task_completion_wakes_stdio_without_losing_partial_human_input() {
        let (mut writer, reader) = duplex(64);
        let stdin = shared_stdin(reader);
        let wake = Arc::new(TestTaskWake {
            ready: tokio::sync::Notify::new(),
            ran: tokio::sync::Notify::new(),
            stdin: stdin.clone(),
            calls: AtomicUsize::new(0),
        });
        let producer = wake.clone();
        let writing = tokio::spawn(async move {
            writer.write_all(b"hel").await.unwrap();
            // Let read_until consume the partial line before delivering completion.
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            producer.ready.notify_one();
            producer.ran.notified().await;
            writer.write_all(b"lo\n").await.unwrap();
            writer
        });
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let observed = prompts.clone();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                stdin,
                &mut Vec::new(),
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                Arc::new(PlainSink::new(
                    crate::headless_host::process_stdout(),
                    crate::headless_host::process_stderr(),
                )),
                &SigintSource::spawn(),
                None,
                Some(wake.as_ref()),
                move |prompt, _| {
                    let prompts = observed.clone();
                    Box::pin(async move {
                        prompts.lock().await.push(prompt);
                        Ok(TurnOutcome::EndTurn)
                    })
                },
            ),
        )
        .await
        .expect("idle task wake must not wait for a human newline");
        assert_eq!(result, StepOutcome::Continue);
        assert_eq!(*prompts.lock().await, vec!["hello"]);
        assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
        let _ = writing.await.unwrap();
    }
    struct HumanTaskHost(std::sync::Mutex<Vec<(String, String)>>);
    #[async_trait::async_trait]
    impl TaskNotificationWake for HumanTaskHost {
        async fn wait(&self) {
            std::future::pending::<()>().await;
        }
        async fn run(&self, _: CancellationToken) -> Result<TurnOutcome, OrchestratorError> {
            unreachable!()
        }
        async fn send_human_task_message(
            &self,
            task_id: &str,
            message: &str,
        ) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((task_id.into(), message.into()));
            Ok(())
        }
    }
    #[tokio::test]
    async fn typed_tasks_message_uses_human_host_and_never_model_dispatch() {
        let (mut writer, reader) = duplex(128);
        writer
            .write_all(b"/tasks message a123  continue  \n")
            .await
            .unwrap();
        let host = HumanTaskHost(Default::default());
        let result = step_with_notifications(
            shared_stdin(reader),
            &mut Vec::new(),
            &NoopDispatcher,
            Arc::new(MockOrchestratorHandle::new()),
            Arc::new(PlainSink::new(
                crate::headless_host::process_stdout(),
                crate::headless_host::process_stderr(),
            )),
            &SigintSource::spawn(),
            None,
            Some(&host),
            |_, _| panic!("human task message must not become a main model prompt"),
        )
        .await;
        assert_eq!(result, StepOutcome::Continue);
        assert_eq!(
            *host.0.lock().unwrap(),
            vec![("a123".into(), " continue  ".into())]
        );
    }
}
