//! Testable REPL loop body.  Decoupled from real stdin/stderr so unit tests
//! can drive it with `tokio::io::duplex` pipes.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-13-repl-mode.md` Task 4.

use crate::idle_notify::IdleNotifier;
use crate::sigint::SigintSource;
use futures::future::BoxFuture;
use harness_runtime::desktop::{PolicyHelperRefusalProbe, PolicySemanticWorkerProbe};
use harness_runtime::headless::output::OutputSink;
use lingxi_core::host::{OrchestratorHandle, SlashCommandDispatcher, SlashDispatchResult};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use orchestrator::{OrchestratorError, TurnOutcome};
use std::io::IsTerminal;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Source-preserving runner for slash commands that expand into typed prompt rows.
pub type SkillBatchTurnFn = Arc<
    dyn Fn(
            Vec<orchestrator::QueuedPromptInput>,
            CancellationToken,
        ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
        + Send
        + Sync,
>;

/// Runs an unknown slash command as a human turn with Native's model-only
/// fallback context and transcript attachment.
pub type UnknownSlashFallbackTurnFn = Arc<
    dyn Fn(
            Utf16JsonProjection,
            Utf16JsonProjection,
            CancellationToken,
        ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
        + Send
        + Sync,
>;

/// Cancels the per-turn SIGINT watcher when its owning REPL branch exits.
/// Tokio detaches a task when its `JoinHandle` is dropped, so cleanup must be
/// explicit even on early returns.
struct TurnSigintGuard(tokio::task::JoinHandle<()>);

impl TurnSigintGuard {
    fn new(watcher: tokio::task::JoinHandle<()>) -> Self {
        Self(watcher)
    }

    fn abort(&self) {
        self.0.abort();
    }
}

impl Drop for TurnSigintGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A distinct current failure that interrupts the host input/turn loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyHelperSignalKind {
    Refusal,
    SemanticWorkerFailure,
}

/// A host policy signal. Kept as a small trait so the input/turn race has
/// deterministic unit tests without constructing a full Desktop runtime.
#[async_trait::async_trait]
pub trait PolicyHelperRefusalSignal: Send {
    fn is_refused(&self) -> bool;
    async fn wait(&mut self);

    fn failure_kind(&self) -> Option<PolicyHelperSignalKind> {
        self.is_refused().then_some(PolicyHelperSignalKind::Refusal)
    }

    /// Return the kind that actually woke the waiter so a concurrently-cleared
    /// worker status cannot be misreported as a Native refusal.
    async fn wait_kind(&mut self) -> PolicyHelperSignalKind {
        self.wait().await;
        self.failure_kind()
            .unwrap_or(PolicyHelperSignalKind::Refusal)
    }
}

#[async_trait::async_trait]
impl PolicyHelperRefusalSignal for PolicyHelperRefusalProbe {
    fn is_refused(&self) -> bool {
        self.current().is_some()
    }

    async fn wait(&mut self) {
        let _ = PolicyHelperRefusalProbe::wait(self).await;
    }
}

/// Combines the Native refusal and engine-capability status without conflating
/// the two outcomes. Refusal takes precedence if both become current together.
pub struct PolicyHelperRuntimeFailureSignal {
    refusal: PolicyHelperRefusalProbe,
    semantic_worker: PolicySemanticWorkerProbe,
}

impl PolicyHelperRuntimeFailureSignal {
    pub fn new(
        refusal: PolicyHelperRefusalProbe,
        semantic_worker: PolicySemanticWorkerProbe,
    ) -> Self {
        Self {
            refusal,
            semantic_worker,
        }
    }
}

#[async_trait::async_trait]
impl PolicyHelperRefusalSignal for PolicyHelperRuntimeFailureSignal {
    fn is_refused(&self) -> bool {
        self.refusal.current().is_some()
    }

    fn failure_kind(&self) -> Option<PolicyHelperSignalKind> {
        if self.refusal.current().is_some() {
            Some(PolicyHelperSignalKind::Refusal)
        } else if self.semantic_worker.current().is_some() {
            Some(PolicyHelperSignalKind::SemanticWorkerFailure)
        } else {
            None
        }
    }

    async fn wait(&mut self) {
        let _ = self.wait_kind().await;
    }

    async fn wait_kind(&mut self) -> PolicyHelperSignalKind {
        tokio::select! {
            biased;
            _ = self.refusal.wait() => PolicyHelperSignalKind::Refusal,
            _ = self.semantic_worker.wait() => PolicyHelperSignalKind::SemanticWorkerFailure,
        }
    }
}

enum TurnDrive {
    Completed(Result<TurnOutcome, OrchestratorError>),
    Refused,
    SemanticWorkerFailure,
}

fn signal_outcome(signal: &Option<Box<dyn PolicyHelperRefusalSignal>>) -> Option<StepOutcome> {
    signal
        .as_ref()?
        .failure_kind()
        .map(outcome_for_failure_kind)
}

fn outcome_for_failure_kind(kind: PolicyHelperSignalKind) -> StepOutcome {
    match kind {
        PolicyHelperSignalKind::Refusal => StepOutcome::PolicyHelperRefusal,
        PolicyHelperSignalKind::SemanticWorkerFailure => StepOutcome::PolicySemanticWorkerFailure,
    }
}

async fn wait_for_policy_refusal(
    signal: &mut Option<Box<dyn PolicyHelperRefusalSignal>>,
) -> PolicyHelperSignalKind {
    match signal.as_mut() {
        Some(signal) => signal.wait_kind().await,
        None => std::future::pending::<PolicyHelperSignalKind>().await,
    }
}

async fn drive_turn_or_refusal(
    signal: &mut Option<Box<dyn PolicyHelperRefusalSignal>>,
    cancel: CancellationToken,
    mut turn: BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> TurnDrive {
    if let Some(signal) = signal.as_mut() {
        tokio::select! {
            biased;
            kind = signal.wait_kind() => {
                cancel.cancel();
                // Do not drop accepted work: let the turn's existing cleanup
                // and durable append finish before the caller tears down.
                let _ = turn.await;
                match kind {
                    PolicyHelperSignalKind::SemanticWorkerFailure => TurnDrive::SemanticWorkerFailure,
                    PolicyHelperSignalKind::Refusal => TurnDrive::Refused,
                }
            }
            result = &mut turn => TurnDrive::Completed(result),
        }
    } else {
        TurnDrive::Completed(turn.await)
    }
}

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
    /// A live managed policy helper selected `onFailure=refuse`.
    PolicyHelperRefusal,
    /// Required Native policy semantics could not be checked by the pinned engine.
    PolicySemanticWorkerFailure,
}

/// Host adapter for completion delivery while the prompt is idle.
#[async_trait::async_trait]
pub trait TaskNotificationWake: Send + Sync {
    /// Cancel pending session wakeups when the user interrupts an idle prompt.
    async fn user_interrupt(&self) {}
    /// Only called after a real stdin line selects `/tasks message`.
    async fn send_human_task_message(
        &self,
        _task_id: &str,
        _message: &Utf16JsonProjection,
    ) -> Result<(), String> {
        Err("task messaging is unavailable".into())
    }
    async fn wait(&self);
    async fn run(&self, cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError>;
}

/// Build Native's request-only reminder and its source-preserving transcript
/// sidecar for an unknown REPL slash command.
pub(crate) fn unknown_command_fallback_context(
    command_name: &Utf16JsonProjection,
    catalog: &command_api::HeadlessCommandCatalog,
) -> (
    Vec<Utf16JsonProjection>,
    orchestrator::RequestContextTranscriptAttachment,
) {
    let reminder = command_api::render_unknown_command_reminder(command_name, catalog);
    let available_names = Utf16JsonProjection::array(catalog.prompt_command_names.clone())
        .expect("command catalog names form a JSON array");
    let available_count = Utf16JsonProjection::parse(&catalog.available_command_count.to_string())
        .expect("command count is a JSON number");
    let payload = Utf16JsonProjection::object(vec![
        ("type".to_owned(), "unknown_command_fallback".into()),
        ("commandName".to_owned(), command_name.clone()),
        ("availableCommandNames".to_owned(), available_names),
        ("availableCommandCount".to_owned(), available_count),
    ])
    .expect("unknown command fallback is a JSON object");
    let rendered = Utf16JsonProjection::object(vec![("content".to_owned(), reminder.clone())])
        .and_then(|line| Utf16JsonProjection::array(vec![line]))
        .expect("unknown command rendered view is a content array");
    let attachment = orchestrator::RequestContextTranscriptAttachment {
        message_id: lingxi_core::types::MessageId::new(),
        payload,
        rendered,
        // Native keeps renderedRole=user; isMeta is mapped independently from the payload type.
        rendered_role: "user",
    };
    (vec![reminder], attachment)
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
        Utf16JsonProjection,
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
        None,
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
    skill_batch_fn: Option<SkillBatchTurnFn>,
    policy_refusal: Option<Box<dyn PolicyHelperRefusalSignal>>,
    run_turn_fn: impl Fn(
        Utf16JsonProjection,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> StepOutcome
where
    W: tokio::io::AsyncWrite + Unpin,
{
    step_with_unknown_fallback(
        stdin,
        stderr,
        dispatcher,
        handle,
        sink,
        sigint,
        idle,
        notifications,
        skill_batch_fn,
        policy_refusal,
        None,
        run_turn_fn,
    )
    .await
}

/// Execute one REPL step with a Native-faithful runner for unknown slash
/// commands. Other prompt and slash-command paths keep using `run_turn_fn`.
#[allow(clippy::too_many_arguments)]
pub async fn step_with_notifications_and_unknown_fallback<W>(
    stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
    stderr: &mut W,
    dispatcher: &dyn SlashCommandDispatcher,
    handle: Arc<dyn OrchestratorHandle>,
    sink: Arc<dyn OutputSink>,
    sigint: &SigintSource,
    idle: Option<&dyn IdleNotifier>,
    notifications: Option<&dyn TaskNotificationWake>,
    skill_batch_fn: Option<SkillBatchTurnFn>,
    policy_refusal: Option<Box<dyn PolicyHelperRefusalSignal>>,
    unknown_fallback_turn_fn: &UnknownSlashFallbackTurnFn,
    run_turn_fn: impl Fn(
        Utf16JsonProjection,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> StepOutcome
where
    W: tokio::io::AsyncWrite + Unpin,
{
    step_with_unknown_fallback(
        stdin,
        stderr,
        dispatcher,
        handle,
        sink,
        sigint,
        idle,
        notifications,
        skill_batch_fn,
        policy_refusal,
        Some(unknown_fallback_turn_fn),
        run_turn_fn,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn step_with_unknown_fallback<W>(
    stdin: Arc<Mutex<dyn AsyncBufRead + Send + Unpin>>,
    stderr: &mut W,
    dispatcher: &dyn SlashCommandDispatcher,
    handle: Arc<dyn OrchestratorHandle>,
    sink: Arc<dyn OutputSink>,
    sigint: &SigintSource,
    idle: Option<&dyn IdleNotifier>,
    notifications: Option<&dyn TaskNotificationWake>,
    skill_batch_fn: Option<SkillBatchTurnFn>,
    mut policy_refusal: Option<Box<dyn PolicyHelperRefusalSignal>>,
    unknown_fallback_turn_fn: Option<&UnknownSlashFallbackTurnFn>,
    run_turn_fn: impl Fn(
        Utf16JsonProjection,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>,
) -> StepOutcome
where
    W: tokio::io::AsyncWrite + Unpin,
{
    if let Some(outcome) = signal_outcome(&policy_refusal) {
        return outcome;
    }
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
        PolicyFailure(PolicyHelperSignalKind),
    }
    loop {
        let event = {
            let mut reader = stdin.lock().await;
            tokio::select! {
                biased;
                kind = wait_for_policy_refusal(&mut policy_refusal) => InputEvent::PolicyFailure(kind),
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
            InputEvent::PolicyFailure(kind) => return outcome_for_failure_kind(kind),
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
                let signal = TurnSigintGuard::new(sigint.arm_for_turn(cancel.clone()));
                if let Some(outcome) = signal_outcome(&policy_refusal) {
                    signal.abort();
                    return outcome;
                }
                sink.turn_start().await;
                if let Some(outcome) = signal_outcome(&policy_refusal) {
                    signal.abort();
                    return outcome;
                }
                if let Some(wake) = notifications {
                    let run = wake.run(cancel.clone());
                    tokio::pin!(run);
                    let result = tokio::select! {
                        biased;
                        kind = wait_for_policy_refusal(&mut policy_refusal) => {
                            cancel.cancel();
                            // Keep polling accepted work through its cleanup before
                            // leaving the idle input loop.
                            let _ = run.await;
                            signal.abort();
                            return signal_outcome(&policy_refusal)
                                .unwrap_or_else(|| outcome_for_failure_kind(kind));
                        }
                        result = &mut run => result,
                    };
                    if let Err(error) = result {
                        sink.error("runtime", &error.to_string()).await;
                    }
                }
                signal.abort();
                if let Some(outcome) = signal_outcome(&policy_refusal) {
                    return outcome;
                }
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
    if let Some(outcome) = signal_outcome(&policy_refusal) {
        return outcome;
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
                Ok((task_id, message)) => {
                    let message = Utf16JsonProjection::from(message);
                    host.send_human_task_message(task_id, &message)
                        .await
                        .map(|()| format!("Message accepted for task {task_id}"))
                }
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
        // The slash handler and any turn it expands into are one user
        // operation. Arm SIGINT before dispatch and reuse this token below so
        // cancellation during a handler cannot be hidden by a fresh turn token.
        let token = CancellationToken::new();
        let _sigint_guard = TurnSigintGuard::new(sigint.arm_for_turn(token.clone()));
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
        let source_input = Utf16JsonProjection::from(input);
        let res = command_api::with_mod_command_context(
            context,
            dispatcher.dispatch(source_input.clone(), token.clone()),
        )
        .await;
        if let Some(outcome) = signal_outcome(&policy_refusal) {
            return outcome;
        }
        if token.is_cancelled() {
            _sigint_guard.abort();
            return StepOutcome::Continue;
        }
        enum ModelTurn {
            Prompt(Utf16JsonProjection),
            UnknownSlash {
                prompt: Utf16JsonProjection,
                command_name: Utf16JsonProjection,
            },
            Skill(Vec<orchestrator::QueuedPromptInput>),
        }
        let model_turn = match res {
            SlashDispatchResult::Handled { display } => {
                sink.command_output(input, display.string_at("").unwrap_or_default())
                    .await;
                None
            }
            SlashDispatchResult::Failed { diagnostic } => {
                let display = crate::mode::slash_failure_display(&diagnostic);
                let display_text = String::from_utf16_lossy(
                    &display
                        .string_units("")
                        .expect("slash failure display is a root string"),
                );
                sink.error("slash_command", &display_text).await;
                None
            }
            // A prompt-expanding command (`/loop`, Markdown/Plugin) runs AS a
            // turn: feed the expanded prompt to the model instead of printing it
            // (claude-code `type: "prompt"`). This makes a typed `/loop 5m /foo`
            // actually schedule the cron + execute now.
            SlashDispatchResult::RunAsTurn { prompt } => Some(ModelTurn::Prompt(prompt)),
            SlashDispatchResult::RunSkillAsTurn {
                command,
                body,
                hook_messages,
            } => Some(ModelTurn::Skill(orchestrator::skill_prompt_inputs(
                command,
                body,
                hook_messages,
            ))),
            // Native CLI keeps malformed slash input as the exact user prompt.
            // For an unknown command, the runner separately stages Native's
            // reminder and transcript attachment around that same prompt.
            SlashDispatchResult::ParseFailure { .. } => Some(ModelTurn::Prompt(source_input)),
            SlashDispatchResult::Unknown { name, .. } => Some(ModelTurn::UnknownSlash {
                prompt: source_input,
                command_name: name,
            }),
            SlashDispatchResult::NotASlashCommand => {
                // Dispatcher contract: won't happen for "/" prefixed input.
                None
            }
        };
        if let Some(model_turn) = model_turn {
            if matches!(&model_turn, ModelTurn::Skill(_)) && skill_batch_fn.is_none() {
                sink.error(
                    "slash_command",
                    "source-preserving skill turn runner is unavailable",
                )
                .await;
            } else {
                if matches!(&model_turn, ModelTurn::UnknownSlash { .. })
                    && unknown_fallback_turn_fn.is_none()
                {
                    sink.error(
                        "slash_command",
                        "Native unknown-command fallback runner is unavailable",
                    )
                    .await;
                    _sigint_guard.abort();
                    return StepOutcome::Continue;
                }
                sink.turn_start().await;
                let turn = match model_turn {
                    ModelTurn::Prompt(prompt) => run_turn_fn(prompt, token.clone()),
                    ModelTurn::UnknownSlash {
                        prompt,
                        command_name,
                    } => unknown_fallback_turn_fn.expect("unknown fallback runner was checked")(
                        prompt,
                        command_name,
                        token.clone(),
                    ),
                    ModelTurn::Skill(inputs) => skill_batch_fn
                        .as_ref()
                        .expect("skill runner was checked")(
                        inputs, token.clone()
                    ),
                };
                let outcome =
                    match drive_turn_or_refusal(&mut policy_refusal, token.clone(), turn).await {
                        TurnDrive::Refused => {
                            _sigint_guard.abort();
                            return StepOutcome::PolicyHelperRefusal;
                        }
                        TurnDrive::SemanticWorkerFailure => {
                            _sigint_guard.abort();
                            return StepOutcome::PolicySemanticWorkerFailure;
                        }
                        TurnDrive::Completed(outcome) => outcome,
                    };
                _sigint_guard.abort();
                if let Some(outcome) = signal_outcome(&policy_refusal) {
                    return outcome;
                }
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
                        if signal_outcome(&policy_refusal).is_none() {
                            sink.error("runtime", &e.to_string()).await;
                        }
                    }
                }
            }
        }
        if handle.current_should_exit().await {
            return StepOutcome::ExitCommand { undispatched: None };
        }
        return StepOutcome::Continue;
    }

    // 4. Plain text → orchestrator turn.
    let token = CancellationToken::new();
    // The guard aborts the watcher when this scope exits; the next iteration
    // therefore gets a fresh one.
    let _sigint_guard = TurnSigintGuard::new(sigint.arm_for_turn(token.clone()));
    sink.turn_start().await;
    let outcome = match drive_turn_or_refusal(
        &mut policy_refusal,
        token.clone(),
        run_turn_fn(Utf16JsonProjection::from(input), token),
    )
    .await
    {
        TurnDrive::Refused => {
            _sigint_guard.abort();
            return StepOutcome::PolicyHelperRefusal;
        }
        TurnDrive::SemanticWorkerFailure => {
            _sigint_guard.abort();
            return StepOutcome::PolicySemanticWorkerFailure;
        }
        TurnDrive::Completed(outcome) => outcome,
    };
    _sigint_guard.abort();
    if let Some(outcome) = signal_outcome(&policy_refusal) {
        return outcome;
    }
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
            if signal_outcome(&policy_refusal).is_none() {
                sink.error("runtime", &e.to_string()).await;
            }
        }
    }
    StepOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_context_keeps_exact_name_in_typed_user_sidecar() {
        let command_name = Utf16JsonProjection::root_string(
            "missing\u{fffd}".to_owned(),
            vec![
                0x006d, 0x0069, 0x0073, 0x0073, 0x0069, 0x006e, 0x0067, 0xd800,
            ],
        )
        .unwrap();
        let catalog = command_api::HeadlessCommandCatalog {
            prompt_command_names: vec!["help".into(), "loop".into()],
            available_command_count: 2,
        };

        let (context, attachment) = unknown_command_fallback_context(&command_name, &catalog);

        assert_eq!(context.len(), 1);
        assert!(context[0]
            .string_units("")
            .expect("reminder is a root string")
            .contains(&0xd800));
        assert_eq!(
            attachment.payload.string_at("/type"),
            Some("unknown_command_fallback")
        );
        assert_eq!(
            attachment.payload.string_units("/commandName"),
            Some(vec![
                0x006d, 0x0069, 0x0073, 0x0073, 0x0069, 0x006e, 0x0067, 0xd800
            ])
        );
        assert_eq!(
            attachment.rendered.string_units("/0/content"),
            context[0].string_units("")
        );
        assert_eq!(attachment.rendered_role, "user");
    }

    #[tokio::test]
    async fn turn_sigint_guard_aborts_watcher_when_branch_exits() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();

        struct SignalOnDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for SignalOnDrop {
            fn drop(&mut self) {
                if let Some(signal) = self.0.take() {
                    let _ = signal.send(());
                }
            }
        }

        let watcher = tokio::spawn(async move {
            let _signal_on_drop = SignalOnDrop(Some(stopped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        let guard = TurnSigintGuard::new(watcher);
        started_rx.await.expect("watcher started");

        drop(guard);

        tokio::time::timeout(std::time::Duration::from_secs(1), stopped_rx)
            .await
            .expect("guard should stop the watcher task")
            .expect("watcher task should be dropped");
    }

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
        assert_ne!(StepOutcome::PolicyHelperRefusal, StepOutcome::Continue);
        assert_ne!(
            StepOutcome::PolicySemanticWorkerFailure,
            StepOutcome::PolicyHelperRefusal
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
            StepOutcome::PolicyHelperRefusal,
            StepOutcome::PolicySemanticWorkerFailure,
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
    use tokio::sync::{watch, Mutex, Notify};

    struct TestPolicyRefusalSignal(watch::Receiver<bool>);

    #[async_trait::async_trait]
    impl PolicyHelperRefusalSignal for TestPolicyRefusalSignal {
        fn is_refused(&self) -> bool {
            *self.0.borrow()
        }

        async fn wait(&mut self) {
            loop {
                if self.is_refused() {
                    return;
                }
                if self.0.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    fn test_policy_refusal_signal() -> (Box<dyn PolicyHelperRefusalSignal>, watch::Sender<bool>) {
        let (sender, receiver) = watch::channel(false);
        (Box::new(TestPolicyRefusalSignal(receiver)), sender)
    }

    struct TestSemanticWorkerSignal(watch::Receiver<bool>);

    #[async_trait::async_trait]
    impl PolicyHelperRefusalSignal for TestSemanticWorkerSignal {
        fn is_refused(&self) -> bool {
            false
        }

        fn failure_kind(&self) -> Option<PolicyHelperSignalKind> {
            (*self.0.borrow()).then_some(PolicyHelperSignalKind::SemanticWorkerFailure)
        }

        async fn wait(&mut self) {
            loop {
                if self.failure_kind().is_some() {
                    return;
                }
                if self.0.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    fn test_semantic_worker_signal() -> (Box<dyn PolicyHelperRefusalSignal>, watch::Sender<bool>) {
        let (sender, receiver) = watch::channel(false);
        (Box::new(TestSemanticWorkerSignal(receiver)), sender)
    }

    struct ClearingSemanticWorkerSignal {
        wake: Arc<Notify>,
        current: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl PolicyHelperRefusalSignal for ClearingSemanticWorkerSignal {
        fn is_refused(&self) -> bool {
            false
        }

        fn failure_kind(&self) -> Option<PolicyHelperSignalKind> {
            self.current
                .load(Ordering::SeqCst)
                .then_some(PolicyHelperSignalKind::SemanticWorkerFailure)
        }

        async fn wait(&mut self) {
            self.wake.notified().await;
            self.current.store(false, Ordering::SeqCst);
        }

        async fn wait_kind(&mut self) -> PolicyHelperSignalKind {
            self.wait().await;
            PolicyHelperSignalKind::SemanticWorkerFailure
        }
    }

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
        async fn dispatch(
            &self,
            _raw: Utf16JsonProjection,
            _cancellation: CancellationToken,
        ) -> SlashDispatchResult {
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
    fn end_turn_fn() -> impl Fn(
        Utf16JsonProjection,
        CancellationToken,
    ) -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>> {
        |_prompt, _token| Box::pin(async { Ok(TurnOutcome::EndTurn) })
    }

    #[tokio::test]
    async fn live_policy_refusal_wakes_idle_prompt_without_admitting_a_turn() {
        let (_writer, client) = duplex(64);
        let (signal, refusal) = test_policy_refusal_signal();
        let trigger = tokio::spawn(async move {
            tokio::task::yield_now().await;
            refusal.send(true).unwrap();
        });
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                shared_stdin(client),
                &mut stderr,
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                sink,
                &SigintSource::spawn(),
                None,
                None,
                None,
                Some(signal),
                |_, _| panic!("refusal while idle must not admit a turn"),
            ),
        )
        .await
        .expect("policy refusal must wake an idle input read");
        trigger.await.unwrap();
        assert_eq!(outcome, StepOutcome::PolicyHelperRefusal);
    }

    #[tokio::test]
    async fn semantic_worker_failure_wakes_idle_prompt_as_a_distinct_status() {
        let (_writer, client) = duplex(64);
        let (signal, failure) = test_semantic_worker_signal();
        let trigger = tokio::spawn(async move {
            tokio::task::yield_now().await;
            failure.send(true).unwrap();
        });
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                shared_stdin(client),
                &mut stderr,
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                sink,
                &SigintSource::spawn(),
                None,
                None,
                None,
                Some(signal),
                |_, _| panic!("semantic-worker failure while idle must not admit a turn"),
            ),
        )
        .await
        .expect("semantic-worker failure must wake the idle input read");
        trigger.await.unwrap();
        assert_eq!(outcome, StepOutcome::PolicySemanticWorkerFailure);
    }

    #[tokio::test]
    async fn live_policy_refusal_cancels_and_joins_the_active_turn() {
        let (mut writer, client) = duplex(64);
        writer.write_all(b"hello\n").await.unwrap();
        let (signal, refusal) = test_policy_refusal_signal();
        let turn_started = Arc::new(Notify::new());
        let settled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let trigger_started = turn_started.clone();
        let trigger = tokio::spawn(async move {
            trigger_started.notified().await;
            refusal.send(true).unwrap();
        });
        let turn_started = turn_started.clone();
        let turn_settled = settled.clone();
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                shared_stdin(client),
                &mut stderr,
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                sink,
                &SigintSource::spawn(),
                None,
                None,
                None,
                Some(signal),
                move |_, cancel| {
                    let turn_started = turn_started.clone();
                    let turn_settled = turn_settled.clone();
                    Box::pin(async move {
                        turn_started.notify_one();
                        cancel.cancelled().await;
                        turn_settled.store(true, Ordering::SeqCst);
                        Ok(TurnOutcome::Cancelled)
                    })
                },
            ),
        )
        .await
        .expect("refusal should cancel and settle the active turn");
        trigger.await.unwrap();
        assert_eq!(outcome, StepOutcome::PolicyHelperRefusal);
        assert!(settled.load(Ordering::SeqCst));
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
        let run_turn_fn = move |_prompt: Utf16JsonProjection, _token: CancellationToken| {
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
        async fn dispatch(
            &self,
            _raw: Utf16JsonProjection,
            _cancellation: CancellationToken,
        ) -> SlashDispatchResult {
            self.0.clone()
        }
    }

    struct CancellingDispatcher {
        raw: Arc<Mutex<Option<Utf16JsonProjection>>>,
    }

    #[async_trait::async_trait]
    impl SlashCommandDispatcher for CancellingDispatcher {
        async fn dispatch(
            &self,
            raw: Utf16JsonProjection,
            cancellation: CancellationToken,
        ) -> SlashDispatchResult {
            *self.raw.lock().await = Some(raw);
            cancellation.cancel();
            SlashDispatchResult::RunAsTurn {
                prompt: "expanded prompt".into(),
            }
        }
    }

    /// Run `step` once with the given dispatcher + a `run_turn_fn` that records
    /// every prompt it is invoked with. Returns `(outcome, recorded_prompts)`.
    async fn run_step_slash(
        dispatcher: &dyn SlashCommandDispatcher,
        line: &str,
    ) -> (StepOutcome, Arc<Mutex<Vec<Utf16JsonProjection>>>) {
        let (mut writer, client) = duplex(64);
        let stdin = shared_stdin(client);
        let mut stderr = Vec::new();
        let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let sigint = SigintSource::spawn();

        let prompts = Arc::new(Mutex::new(Vec::<Utf16JsonProjection>::new()));
        let prompts_for_turn = prompts.clone();
        let run_turn_fn = move |prompt: Utf16JsonProjection, _token: CancellationToken| {
            let p = prompts_for_turn.clone();
            Box::pin(async move {
                p.lock().await.push(prompt);
                Ok(TurnOutcome::EndTurn)
            }) as BoxFuture<'static, Result<TurnOutcome, OrchestratorError>>
        };
        let fallback_prompts = prompts.clone();
        let unknown_fallback_turn_fn: UnknownSlashFallbackTurnFn =
            Arc::new(move |prompt, _command_name, _token| {
                let p = fallback_prompts.clone();
                Box::pin(async move {
                    p.lock().await.push(prompt);
                    Ok(TurnOutcome::EndTurn)
                })
            });

        let line_owned = format!("{line}\n");
        let writer_task = tokio::spawn(async move {
            let _ = writer.write_all(line_owned.as_bytes()).await;
            let _ = writer.flush().await;
            writer
        });

        let outcome = step_with_notifications_and_unknown_fallback(
            stdin,
            &mut stderr,
            dispatcher,
            handle,
            sink,
            &sigint,
            None,
            None,
            None,
            None,
            &unknown_fallback_turn_fn,
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
            prompt: "EXPANDED /loop body".into(),
        });
        let (outcome, prompts) = run_step_slash(&dispatcher, "/loop 5m /foo").await;
        assert_eq!(outcome, StepOutcome::Continue);
        let recorded = prompts.lock().await.clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].string_at(""), Some("EXPANDED /loop body"));
    }

    #[tokio::test]
    async fn cancelled_slash_dispatch_does_not_submit_turn_and_preserves_source() {
        let (mut writer, client) = duplex(128);
        writer.write_all("/loop café\n".as_bytes()).await.unwrap();
        let raw = Arc::new(Mutex::new(None));
        let dispatcher = CancellingDispatcher { raw: raw.clone() };
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));
        let turn_calls = Arc::new(AtomicUsize::new(0));
        let turn_calls_for_run = turn_calls.clone();

        let outcome = step(
            shared_stdin(client),
            &mut stderr,
            &dispatcher,
            Arc::new(MockOrchestratorHandle::new()),
            sink,
            &SigintSource::spawn(),
            None,
            move |_prompt, _cancellation| {
                turn_calls_for_run.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(TurnOutcome::EndTurn) })
            },
        )
        .await;

        assert_eq!(outcome, StepOutcome::Continue);
        assert_eq!(turn_calls.load(Ordering::SeqCst), 0);
        let dispatched = raw.lock().await.clone().expect("dispatcher saw the input");
        assert_eq!(dispatched.string_at(""), Some("/loop café"));
        assert_eq!(
            dispatched.string_units(""),
            Some("/loop café".encode_utf16().collect())
        );
    }

    #[tokio::test]
    async fn unknown_fallback_runner_receives_original_prompt_name_and_live_token() {
        let (mut writer, client) = duplex(128);
        writer.write_all(b"/missing caf\xc3\xa9\n").await.unwrap();
        let dispatcher = FixedDispatcher(SlashDispatchResult::Unknown {
            name: "missing".into(),
            display: "Unknown command: /missing".into(),
        });
        let captured = Arc::new(Mutex::new(None));
        let captured_for_fallback = captured.clone();
        let unknown_fallback_turn_fn: UnknownSlashFallbackTurnFn =
            Arc::new(move |prompt, command_name, token| {
                let captured = captured_for_fallback.clone();
                Box::pin(async move {
                    *captured.lock().await = Some((prompt, command_name, token.is_cancelled()));
                    Ok(TurnOutcome::EndTurn)
                })
            });
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));

        let outcome = step_with_notifications_and_unknown_fallback(
            shared_stdin(client),
            &mut stderr,
            &dispatcher,
            Arc::new(MockOrchestratorHandle::new()),
            sink,
            &SigintSource::spawn(),
            None,
            None,
            None,
            None,
            &unknown_fallback_turn_fn,
            |_, _| -> BoxFuture<'static, Result<TurnOutcome, OrchestratorError>> {
                panic!("unknown slash must use its request-context runner")
            },
        )
        .await;

        assert_eq!(outcome, StepOutcome::Continue);
        let (prompt, name, cancelled) = captured
            .lock()
            .await
            .clone()
            .expect("fallback runner should receive the unknown slash turn");
        assert_eq!(prompt.string_at(""), Some("/missing café"));
        assert_eq!(
            prompt.string_units(""),
            Some("/missing café".encode_utf16().collect())
        );
        assert_eq!(name.string_at(""), Some("missing"));
        assert!(
            !cancelled,
            "the same live turn token reaches the fallback runner"
        );
    }

    #[tokio::test]
    async fn malformed_and_unknown_slash_inputs_fall_through_as_user_turns() {
        for (dispatch, input) in [
            (
                SlashDispatchResult::ParseFailure {
                    input: Utf16JsonProjection::from("/"),
                },
                "/",
            ),
            (
                SlashDispatchResult::Unknown {
                    name: "native-unknown".into(),
                    display: "Unknown command".into(),
                },
                "/native-unknown fixed-argument",
            ),
        ] {
            let dispatcher = FixedDispatcher(dispatch);
            let (outcome, prompts) = run_step_slash(&dispatcher, input).await;
            assert_eq!(outcome, StepOutcome::Continue);
            let recorded = prompts.lock().await;
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].string_at(""), Some(input));
        }
    }

    /// A display-only builtin (`/help`, `/model`) stays display-only — it must
    /// NOT run a turn.
    #[tokio::test]
    async fn handled_builtin_stays_display_only() {
        let dispatcher = FixedDispatcher(SlashDispatchResult::Handled {
            display: "Available commands: ...".into(),
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

    struct CancellableTaskWake {
        ready: Notify,
        started: Notify,
        cancelled: std::sync::atomic::AtomicBool,
        settled: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl TaskNotificationWake for CancellableTaskWake {
        async fn wait(&self) {
            self.ready.notified().await;
        }

        async fn run(&self, cancel: CancellationToken) -> Result<TurnOutcome, OrchestratorError> {
            self.started.notify_one();
            cancel.cancelled().await;
            self.cancelled.store(true, Ordering::SeqCst);
            self.settled.store(true, Ordering::SeqCst);
            Ok(TurnOutcome::Cancelled)
        }
    }

    #[tokio::test]
    async fn live_policy_refusal_cancels_and_joins_task_notification_wake() {
        let (_writer, client) = duplex(64);
        let (signal, refusal) = test_policy_refusal_signal();
        let wake = Arc::new(CancellableTaskWake {
            ready: Notify::new(),
            started: Notify::new(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
            settled: std::sync::atomic::AtomicBool::new(false),
        });
        let wake_for_trigger = wake.clone();
        let trigger = tokio::spawn(async move {
            wake_for_trigger.started.notified().await;
            refusal.send(true).unwrap();
        });
        wake.ready.notify_one();
        let mut stderr = Vec::new();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                shared_stdin(client),
                &mut stderr,
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                sink,
                &SigintSource::spawn(),
                None,
                Some(wake.as_ref()),
                None,
                Some(signal),
                |_, _| panic!("task wake cancellation must not admit a human turn"),
            ),
        )
        .await
        .expect("refusal must cancel and settle an active task wake");
        trigger.await.unwrap();
        assert_eq!(outcome, StepOutcome::PolicyHelperRefusal);
        assert!(wake.cancelled.load(Ordering::SeqCst));
        assert!(wake.settled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn semantic_worker_kind_survives_clear_during_task_wake_cleanup() {
        let (_writer, client) = duplex(64);
        let current = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal_wake = Arc::new(Notify::new());
        let signal = Box::new(ClearingSemanticWorkerSignal {
            wake: signal_wake.clone(),
            current: current.clone(),
        });
        let wake = Arc::new(CancellableTaskWake {
            ready: Notify::new(),
            started: Notify::new(),
            cancelled: std::sync::atomic::AtomicBool::new(false),
            settled: std::sync::atomic::AtomicBool::new(false),
        });
        let wake_for_trigger = wake.clone();
        let current_for_trigger = current.clone();
        let trigger = tokio::spawn(async move {
            wake_for_trigger.started.notified().await;
            current_for_trigger.store(true, Ordering::SeqCst);
            signal_wake.notify_one();
        });
        wake.ready.notify_one();
        let sink: Arc<dyn OutputSink> = Arc::new(PlainSink::new(
            crate::headless_host::process_stdout(),
            crate::headless_host::process_stderr(),
        ));

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            step_with_notifications(
                shared_stdin(client),
                &mut Vec::new(),
                &NoopDispatcher,
                Arc::new(MockOrchestratorHandle::new()),
                sink,
                &SigintSource::spawn(),
                None,
                Some(wake.as_ref()),
                None,
                Some(signal),
                |_, _| panic!("task wake failure must not admit a human turn"),
            ),
        )
        .await
        .expect("semantic-worker failure must cancel and settle a task wake");
        trigger.await.unwrap();
        assert_eq!(outcome, StepOutcome::PolicySemanticWorkerFailure);
        assert!(wake.cancelled.load(Ordering::SeqCst));
        assert!(wake.settled.load(Ordering::SeqCst));
        assert!(!current.load(Ordering::SeqCst));
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
                None,
                None,
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
        assert_eq!(*prompts.lock().await, vec!["hello".into()]);
        assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
        let _ = writing.await.unwrap();
    }
    struct HumanTaskHost(std::sync::Mutex<Vec<(String, Utf16JsonProjection)>>);
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
            message: &Utf16JsonProjection,
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
            None,
            None,
            |_, _| panic!("human task message must not become a main model prompt"),
        )
        .await;
        assert_eq!(result, StepOutcome::Continue);
        assert_eq!(
            *host.0.lock().unwrap(),
            vec![("a123".into(), Utf16JsonProjection::from(" continue  "))]
        );
    }
}
