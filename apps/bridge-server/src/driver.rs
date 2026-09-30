//! S1 — the production [`TurnDriver`] over a real [`ConversationOrchestrator`].
//!
//! This is the engine entry the [`crate::server::BridgeConnection`] calls when an
//! inbound [`client::protocol::commands::ClientCommand::SendPrompt`] arrives. It is
//! the production counterpart of the test-only driver that lived inside the F2-06
//! e2e suite (`tests/e2e_permission_test.rs`): a thin wrapper that turns one
//! `run_turn(prompt)` call into one streaming turn on the orchestrator.
//!
//! ## How the events reach the client
//!
//! The driver does NOT hold the [`client::adapter::AdapterOutputStream`] directly —
//! the orchestrator already owns it as its [`lingxi_core::host::OutputStream`] (wired at
//! construction, by `harness_runtime::desktop::build` in production or by the test harness).
//! Because that output stream lowers every callback into a
//! [`client::protocol::events::ClientEvent`] and forwards it through the
//! connection-scoped [`client::adapter::ClientEventSink`]
//! ([`crate::server::BridgeConnection::event_sink`]), simply driving the streaming
//! turn is enough: `TextDelta`, `ToolUseStarted`/`Result`, the per-turn
//! `CostUpdate`, and the terminal `TurnEnded` all flow out as `Frame::Event`s as a
//! side effect. The driver's only job is to START the turn (on a spawned task, per
//! the [`TurnDriver`] contract) and to surface a turn-level FAILURE as a
//! [`ClientEvent::Error`] so the client is never left waiting silently.
//!
//! ## Cancellation
//!
//! Each turn gets a fresh [`CancellationToken`]; the driver uses the orchestrator's
//! cancelable streaming entry ([`ConversationOrchestrator::run_turn_streaming_with_cancel`])
//! so a future per-turn cancel command (an `AbortTurn`-style control) has a hook to
//! fire it. Today nothing cancels mid-driver, so the token is never tripped and the
//! turn runs to completion — behavior identical to the plain streaming entry.

use std::sync::Arc;

use async_trait::async_trait;
use client::adapter::{lowering::lower_cost_snapshot, ClientEventSink};
use client::protocol::commands::ImageRefDto;
use client::protocol::events::{ClientEvent, ErrorKindDto, TurnOutcomeDto};
// `ImageSource` is re-exported from the orchestrator (the canonical, FROZEN
// `protocol` shape) so this library code can name it without taking a direct
// `protocol` dependency.
use orchestrator::conversation::ImageSource;
use orchestrator::{ConversationOrchestrator, OrchestratorError};
use tokio_util::sync::CancellationToken;

use crate::server::TurnDriver;

/// A msgqueue-backed [`orchestrator::prompt::mid_turn_input::MidTurnInputSource`].
///
/// Bridges the orchestrator's queue-agnostic mid-turn drain seam to the
/// connection's [`msgqueue::MessageQueueManager`]: each
/// [`MidTurnInputSource::take_mid_turn_input`] snapshots the `Next`-priority
/// MAIN-THREAD, NON-slash prompts, joins the consecutive ones via
/// [`msgqueue::join_prompt_values`], REMOVES the consumed commands from the
/// queue (so the between-turn drain doesn't re-run them), and returns the joined
/// text for injection as a meta user message. Returns `None` when nothing
/// batchable is queued.
///
/// `Now`-priority commands are deliberately EXCLUDED: a `Now` enqueue aborts the
/// in-flight turn (via the queue's now-abort hook) and must survive to the
/// between-turn drain so it runs as its own interrupting turn, rather than being
/// silently folded into the running turn as injected mid-turn text.
///
/// This is the composition-root half of the seam — it lives in the bridge (which
/// owns the per-connection queue) so the `orchestrator` crate keeps NO
/// dependency on `msgqueue`. Twin of claude-code's query.ts ~1570-1580
/// snapshot+`joinPromptValues`+inject path.
pub struct MsgQueueMidTurnInput {
    queue: Arc<msgqueue::MessageQueueManager>,
    /// How many mid-turn prompts a human has folded into turns on this
    /// connection (see [`Self::foreign_input_counter`]).
    foreign_inputs: Arc<std::sync::atomic::AtomicU32>,
}

impl MsgQueueMidTurnInput {
    /// Build the adapter over the connection's queue.
    #[must_use]
    pub fn new(queue: Arc<msgqueue::MessageQueueManager>) -> Self {
        Self {
            queue,
            foreign_inputs: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }
}

impl MsgQueueMidTurnInput {
    /// A clone of the counter this source bumps every time a human's prompt is
    /// folded into a turn already in flight.
    ///
    /// PARITY the `/loop` fold's `foreign_user_input` veto: the oracle spots a
    /// real user message inside the span; here that message arrives through
    /// exactly this seam, so counting it is the same fact.
    #[must_use]
    pub fn foreign_input_counter(&self) -> Arc<std::sync::atomic::AtomicU32> {
        self.foreign_inputs.clone()
    }
}

#[async_trait]
impl orchestrator::prompt::mid_turn_input::MidTurnInputSource for MsgQueueMidTurnInput {
    fn supports_goal_retries(&self) -> bool {
        true
    }
    async fn has_queued_goal_work(&self) -> bool {
        self.queue.has_main_thread_commands().await
    }
    async fn enqueue_goal_retry(&self, id: String, body: String, cancel: CancellationToken) {
        self.queue.enqueue_goal_retry(id, body, cancel).await;
    }

    async fn take_mid_turn_input(&self) -> Option<String> {
        let taken = self.queue.take_mid_turn_prompt().await;
        if taken.is_some() {
            self.foreign_inputs
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        taken
    }
}

struct PendingWakeup {
    handle: lingxi_core::host::BackgroundTaskHandle,
    prompt: String,
    command_id: String,
    cancel: CancellationToken,
}

/// A msgqueue-backed [`tool_cron::WakeupScheduler`] — the composition-root impl
/// of the `/loop` dynamic-mode one-shot self-wakeup seam (Phase 2).
///
/// Twin of [`MsgQueueMidTurnInput`]: it lives in the bridge (which owns the
/// per-connection queue + the [`lingxi_core::host::RuntimeSpawner`]) so the `tool-cron`
/// crate — and the orchestrator — keep NO knowledge of how a wakeup is delivered.
/// [`WakeupScheduler::schedule`] spawns ONE background task that
/// [`RuntimeSpawner::sleep`]s for `delay`, resolves the autonomous sentinel via
/// [`tool_cron::resolve_wakeup_prompt`], and ENQUEUEs the resolved prompt at
/// [`msgqueue::QueuePriority::Later`] so it runs only after the current turn.
/// This matches the upstream scheduled notification's `later` priority,
/// `isMeta: true`, and `skipSlashCommands: true`.
///
/// WIRING: attached at `boot::assemble`. The `ScheduleWakeupTool` is built deep
/// inside `harness_runtime::desktop::build` (via `tool_cron::register_all_with_auth`)
/// BEFORE the per-connection queue + spawner exist, so it holds an empty
/// set-once `WakeupSchedulerCell` surfaced on `DesktopRuntime`; `assemble` fills
/// that cell with this adapter once the queue + `runtime_spawner` are available.
/// Hosts without a per-connection queue (CLI / offline / mobile) leave the cell
/// empty → the tool is an honest no-op.
pub struct MsgQueueWakeupScheduler {
    queue: Arc<msgqueue::MessageQueueManager>,
    runtime: Arc<dyn lingxi_core::host::RuntimeSpawner>,
    loop_runtime: Arc<tool_cron::LoopRuntime>,
    /// Wakeups armed but not yet fired (the binary's `kind:"loop"` cron
    /// entries), each paired with the prompt it will re-inject. A new schedule
    /// supersedes them; `stop: true` and a user abort cancel them AND forget
    /// those prompts' loop records (`Ort`), which is why the prompt is kept.
    pending: Arc<std::sync::Mutex<Vec<PendingWakeup>>>,
    /// The connection's event sink, used at fire time to announce the wakeup
    /// (binary `onFireTask`'s transcript append) and, after quiet ticks, the
    /// no-op fold's streak line. `None` ⇒ the wakeup fires silently (tests, and
    /// any host assembled without a sink).
    events: Option<Arc<dyn ClientEventSink>>,
    /// Persist fire boundaries alongside conversation history before publishing.
    orchestrator: Option<std::sync::Weak<orchestrator::ConversationOrchestrator>>,
}

impl MsgQueueWakeupScheduler {
    /// Build the adapter over the connection's queue + the host runtime spawner.
    #[must_use]
    pub fn new(
        queue: Arc<msgqueue::MessageQueueManager>,
        runtime: Arc<dyn lingxi_core::host::RuntimeSpawner>,
    ) -> Self {
        Self {
            queue,
            runtime,
            loop_runtime: Arc::new(tool_cron::LoopRuntime::default()),
            pending: Arc::new(std::sync::Mutex::new(Vec::new())),
            events: None,
            orchestrator: None,
        }
    }

    /// Build the adapter over an existing connection-scoped loop runtime.
    #[must_use]
    pub fn with_loop_runtime(
        queue: Arc<msgqueue::MessageQueueManager>,
        runtime: Arc<dyn lingxi_core::host::RuntimeSpawner>,
        loop_runtime: Arc<tool_cron::LoopRuntime>,
    ) -> Self {
        Self {
            queue,
            runtime,
            loop_runtime,
            pending: Arc::new(std::sync::Mutex::new(Vec::new())),
            events: None,
            orchestrator: None,
        }
    }

    /// Retain structured wakeup metadata so resume reconstructs no-op folds.
    #[must_use]
    pub fn with_orchestrator(
        mut self,
        orchestrator: Arc<orchestrator::ConversationOrchestrator>,
    ) -> Self {
        self.orchestrator = Some(Arc::downgrade(&orchestrator));
        self
    }

    /// Announce each firing wakeup on `events` (binary `onFireTask`). Additive
    /// over [`Self::new`] / [`Self::with_loop_runtime`]; without it a wakeup is
    /// delivered silently, as it was before the no-op fold landed.
    #[must_use]
    pub fn with_event_sink(mut self, events: Arc<dyn ClientEventSink>) -> Self {
        self.events = Some(events);
        self
    }
}

#[async_trait]
impl tool_cron::WakeupScheduler for MsgQueueWakeupScheduler {
    async fn schedule(&self, delay: std::time::Duration, prompt: String, _reason: String) {
        let queue = self.queue.clone();
        let runtime = self.runtime.clone();
        let pending = self.pending.clone();
        let events = self.events.clone();
        let orchestrator = self.orchestrator.clone();
        let loop_runtime = self.loop_runtime.clone();
        // The task body consumes `prompt`; keep the un-resolved text for the
        // pending list so `cancel_pending` can report it back for `Ort`.
        let prompt_for_pending = prompt.clone();
        let task = tool_cron::WakeupTask::new(delay, &prompt);
        let command_id = task.command_id();
        let task_command_id = command_id.clone();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (registered, registration) = tokio::sync::oneshot::channel();
        // Spawn a detached one-shot timer (engine code must not call
        // `tokio::spawn` directly — D17 — so go through the runtime seam).
        let spawned = runtime
            .clone()
            .spawn(
                "loop-wakeup",
                Box::pin(async move {
                    tokio::select! {
                        biased;
                        () = task_cancel.cancelled() => {},
                        () = async {
                    // Even a zero-delay test timer must remain discoverable by
                    // cancellation until persistence/publication/enqueue finish.
                    if registration.await.is_err() { return; }
                    runtime.sleep(delay).await;
                    // The upstream scheduler skips its tick while loading. Keep
                    // this timer cancellable until the current turn has settled.
                    while queue.has_active_turn().await {
                        runtime.sleep(std::time::Duration::from_millis(100)).await;
                    }
                    // PARITY `onFireTask`'s loop branch (`s.replace(f => D(f,
                    // u, U(t), l))`): announce the resume, carrying the no-op
                    // streak the ticks before this one accumulated. Read HERE,
                    // after the sleep — the streak is settled at the turn edge
                    // of the tick that armed this wakeup, which is long past by
                    // the time it fires.
                    if let Some(sink) = &events {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                        let streak = loop_runtime.noop_streak();
                        let (message, companion) = tool_cron::loop_wakeup_lines(now_ms, streak);
                        // EVERY wakeup carries this event, streak or not, so the
                        // client can mark the group boundary without reading the
                        // copy. A non-zero streak tells it how many preceding
                        // groups to collapse — the oracle's `foldedUuids`,
                        // expressed as a count because a LingXi wakeup is one
                        // turn.
                        let since_ms = streak.map_or(0, |(_, since)| {
                            since.duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
                        });
                        if let Some(orchestrator) = orchestrator.as_ref().and_then(std::sync::Weak::upgrade) {
                            if let Err(error) = orchestrator.append_scheduled_loop_wakeup(
                                message.clone(), companion.clone(), streak.map_or(0, |(count, _)| count), since_ms,
                                orchestrator::ScheduledLoopFire { fire_id: task.fire_id, task_id: task.task_id.clone(), cron: task.cron.clone(), prompt: task.display_prompt.clone(), task_kind_loop: true },
                            ).await {
                                tracing::warn!(%error, "could not persist /loop wakeup boundary");
                            }
                        }
                        sink.emit(ClientEvent::LoopWakeup {
                            message,
                            companion,
                            streak: streak.map_or(0, |(streak, _)| streak),
                            since_ms,
                        })
                        .await;
                    }
                    // Preserve the sentinel as the loop identity. The drain
                    // resolves it for this turn; keepalive must re-read loop.md.
                    queue
                        .enqueue(msgqueue::QueuedCommand {
                            scheduled_task_id: Some(task.task_id.clone()),
                            scheduled_fire_id: Some(task.fire_id.as_uuid().to_string()),
                            uuid: task_command_id.clone(),
                            content: msgqueue::QueuedCommandContent::UserInput { text: prompt },
                            priority: msgqueue::QueuePriority::Later,
                            queued_at: std::time::SystemTime::now(),
                            source: msgqueue::QueueSource::Cron,
                            agent_id: None,
                            // Scheduled prompts are model instructions, including
                            // slash-like text, and never become mid-turn user input.
                            skip_slash_commands: true,
                            is_meta: true,
                        })
                        .await;
                    pending.lock().unwrap_or_else(|e| e.into_inner())
                        .retain(|entry| entry.command_id != task_command_id);
                        } => {},
                    }
                }),
            )
            .await;
        if let Ok(handle) = spawned {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(PendingWakeup {
                    handle,
                    prompt: prompt_for_pending,
                    command_id,
                    cancel,
                });
            let _ = registered.send(());
        }
    }

    async fn cancel_pending(&self) -> Vec<String> {
        let armed: Vec<_> =
            std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        // Logical cancellation also covers a timer blocked in persistence, and
        // hosts whose RuntimeSpawner cannot abort a task that has begun firing.
        for entry in &armed {
            entry.cancel.cancel();
        }
        let mut cancelled = Vec::with_capacity(armed.len());
        for entry in armed {
            let _ = self.runtime.cancel(&entry.handle).await;
            self.queue
                .remove(&[entry.command_id], "dynamic loop cancelled")
                .await;
            cancelled.push(entry.prompt);
        }
        // A wakeup that ALREADY fired retired itself from `pending` the moment
        // it enqueued, so the loop above sees nothing to remove while its
        // `Later`-priority command still sits in the queue — and the drain
        // between turns then runs the tick the user just stopped. Sweep the
        // queue for those too. `tool_cron::RuntimeWakeupScheduler::cancel_pending`
        // does the same through `delivery.cancel_queued()`; this is the msgqueue
        // spelling of it.
        let stranded: Vec<_> = self
            .queue
            .snapshot()
            .await
            .into_iter()
            .filter(|command| {
                command.source == msgqueue::QueueSource::Cron
                    && command.uuid.starts_with("loop-wakeup-")
            })
            .collect();
        if !stranded.is_empty() {
            let ids: Vec<_> = stranded
                .iter()
                .map(|command| command.uuid.clone())
                .collect();
            self.queue.remove(&ids, "dynamic loop cancelled").await;
            for command in stranded {
                if let Some(text) = command.text() {
                    // One pending wakeup per prompt: count the
                    // enqueue -> pending-retirement handoff once.
                    if !cancelled.iter().any(|prompt| prompt == text) {
                        cancelled.push(text.to_string());
                    }
                }
            }
        }
        cancelled
    }

    fn loop_runtime(&self) -> Option<Arc<tool_cron::LoopRuntime>> {
        Some(self.loop_runtime.clone())
    }
}

#[async_trait]
impl cron::scheduler::SessionCronDelivery for MsgQueueWakeupScheduler {
    fn loop_runtime(&self) -> Option<Arc<tool_cron::LoopRuntime>> {
        Some(self.loop_runtime.clone())
    }

    async fn clear_queued(&self) {
        let ids: Vec<_> = self
            .queue
            .snapshot()
            .await
            .into_iter()
            .filter(|command| {
                command.source == msgqueue::QueueSource::Cron
                    && command.uuid.starts_with("cron-fire-")
            })
            .map(|command| command.uuid)
            .collect();
        self.queue.remove(&ids, "session changed").await;
    }
    async fn is_loading(&self) -> bool {
        self.queue.has_active_turn().await
    }
    async fn enqueue(&self, fire: cron::scheduler::SessionCronFire) -> Result<(), String> {
        if fire.cron.is_empty() {
            self.queue
                .enqueue(msgqueue::QueuedCommand {
                    scheduled_task_id: None,
                    scheduled_fire_id: None,
                    uuid: format!("cron-fire-{}", fire.id),
                    content: msgqueue::QueuedCommandContent::UserInput { text: fire.prompt },
                    priority: msgqueue::QueuePriority::Later,
                    queued_at: std::time::SystemTime::now(),
                    source: msgqueue::QueueSource::Cron,
                    agent_id: None,
                    skip_slash_commands: true,
                    is_meta: true,
                })
                .await;
            return Ok(());
        }
        let task = tool_cron::WakeupTask::scheduled(&fire);
        let now = std::time::SystemTime::now();
        let now_ms = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let (message, _) = task.lines(now_ms, None);
        if let Some(orch) = self
            .orchestrator
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
        {
            orch.append_scheduled_loop_wakeup(
                message.clone(),
                None,
                0,
                0,
                orchestrator::ScheduledLoopFire {
                    fire_id: task.fire_id,
                    task_id: task.task_id.clone(),
                    cron: task.cron.clone(),
                    prompt: task.display_prompt.clone(),
                    task_kind_loop: false,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        if let Some(sink) = &self.events {
            sink.emit(ClientEvent::ScheduledTaskFire { message }).await;
        }
        self.queue
            .enqueue(msgqueue::QueuedCommand {
                scheduled_task_id: Some(task.task_id.clone()),
                scheduled_fire_id: Some(task.fire_id.as_uuid().to_string()),
                uuid: task.command_id(),
                content: msgqueue::QueuedCommandContent::UserInput { text: fire.prompt },
                priority: msgqueue::QueuePriority::Later,
                queued_at: now,
                source: msgqueue::QueueSource::Cron,
                agent_id: None,
                skip_slash_commands: true,
                is_meta: true,
            })
            .await;
        Ok(())
    }
}

/// A production [`TurnDriver`] backed by a real [`ConversationOrchestrator`].
///
/// Wraps the orchestrator (whose [`client::adapter::AdapterOutputStream`] is already
/// wired to the connection's event sink) plus a clone of that same
/// [`ClientEventSink`] — held ONLY so a turn-level error (an `Err` out of the
/// streaming entry) can be surfaced as a [`ClientEvent::Error`]. Successful events
/// flow through the orchestrator's own output stream, not through this handle.
pub struct OrchestratorTurnDriver {
    orchestrator: Arc<ConversationOrchestrator>,
    /// The connection's event sink, shared with the orchestrator's output stream.
    /// Used solely to emit a terminal [`ClientEvent::Error`] on turn failure;
    /// `None` to drop errors silently (e.g. tests that only assert success).
    error_sink: Option<Arc<dyn ClientEventSink>>,
    /// Optional response accumulator paired with this orchestrator. Production
    /// wiring supplies it so a hard failure cannot leak partial blocks into the
    /// next turn; older/test drivers may omit it.
    message_output: Option<client::adapter::AdapterOutputStream>,
    /// The connection's message queue, wired so each turn's fresh
    /// [`CancellationToken`] is REGISTERED with the queue at turn start (so a
    /// `Now`-priority enqueue aborts the in-flight turn) and CLEARED at turn end.
    /// `None` ⇒ no registration; the turn runs uninterruptibly by the queue
    /// (the test drivers and any caller that builds the driver without a queue).
    queue: Option<Arc<msgqueue::MessageQueueManager>>,
    /// The abort-reason flag the orchestrator reads to tell a `Now`-command abort
    /// from a user interrupt. RESET to `UserInterrupt` at each turn start so a
    /// stale `QueueNowCommand` from a prior turn cannot mislabel this one. `None`
    /// when no queue is wired. The queue's now-abort hook sets it to
    /// `QueueNowCommand` right before firing the token.
    cancel_reason: Option<orchestrator::prompt::mid_turn_input::CancelReasonFlag>,
    /// The self-wakeup scheduler, used at each turn's completion edge to arm the
    /// `/loop` keepalive fallback (binary `lKi`) when a dynamic loop tick ended
    /// without the model rescheduling. `None` ⇒ no keepalive (CLI / tests / hosts
    /// with no per-connection queue). Wired at `boot::assemble` with the SAME
    /// [`MsgQueueWakeupScheduler`] filling the tool's `WakeupSchedulerCell`.
    wakeup_scheduler: Option<Arc<dyn tool_cron::WakeupScheduler>>,
    /// Session-scoped dynamic-loop bookkeeping obtained from the scheduler.
    loop_runtime: Option<Arc<tool_cron::LoopRuntime>>,
    /// Mid-turn prompts a human folded into a turn — the `/loop` fold's
    /// `foreign_user_input` veto. `None` ⇒ that arm never fires.
    foreign_inputs: Option<Arc<std::sync::atomic::AtomicU32>>,
}

impl OrchestratorTurnDriver {
    /// Construct a driver that drops turn-level errors silently.
    ///
    /// Use this when the caller does not need failures surfaced as
    /// [`ClientEvent::Error`] (e.g. a test wired to a mock that never errors).
    /// The production server uses [`Self::with_error_sink`] so an error reaches
    /// the client.
    #[must_use]
    pub fn new(orchestrator: Arc<ConversationOrchestrator>) -> Self {
        Self {
            orchestrator,
            error_sink: None,
            message_output: None,
            queue: None,
            cancel_reason: None,
            wakeup_scheduler: None,
            loop_runtime: None,
            foreign_inputs: None,
        }
    }

    /// Wire the connection's message queue + abort-reason flag so each turn's
    /// cancel token is registered with the queue (a `Now` enqueue aborts the
    /// in-flight turn) and the reason flag is reset at turn start. Additive over
    /// [`Self::new`] / [`Self::with_error_sink`]: a driver built without this
    /// behaves exactly as before (no queue-driven abort). The composition root
    /// (`boot::assemble`) calls this with the [`crate::server::BridgeConnection`]'s
    /// per-connection queue and the SAME flag wired into the orchestrator.
    #[must_use]
    pub fn with_queue(
        mut self,
        queue: Arc<msgqueue::MessageQueueManager>,
        cancel_reason: orchestrator::prompt::mid_turn_input::CancelReasonFlag,
    ) -> Self {
        self.queue = Some(queue);
        self.cancel_reason = Some(cancel_reason);
        self
    }

    /// Construct a driver that surfaces a turn-level failure as a
    /// [`ClientEvent::Error`] on `error_sink`.
    ///
    /// `error_sink` must be the SAME connection-scoped
    /// [`crate::server::BridgeConnection::event_sink`] the orchestrator's
    /// [`client::adapter::AdapterOutputStream`] was built from, so the error frame
    /// rides the one outbound channel in order behind any events the failed turn
    /// already streamed.
    #[must_use]
    pub fn with_error_sink(
        orchestrator: Arc<ConversationOrchestrator>,
        error_sink: Arc<dyn ClientEventSink>,
    ) -> Self {
        Self {
            orchestrator,
            error_sink: Some(error_sink),
            message_output: None,
            queue: None,
            cancel_reason: None,
            wakeup_scheduler: None,
            loop_runtime: None,
            foreign_inputs: None,
        }
    }

    /// Attach the connection's concrete output adapter so response state can
    /// be reset at turn boundaries and after hard failures.
    #[must_use]
    pub fn with_message_output(mut self, output: client::adapter::AdapterOutputStream) -> Self {
        self.message_output = Some(output);
        self
    }

    /// Wire the self-wakeup scheduler so each turn's completion edge can arm the
    /// `/loop` keepalive fallback (binary `lKi`). Additive over [`Self::new`] /
    /// [`Self::with_error_sink`] / [`Self::with_queue`]; a driver built without it
    /// never arms a keepalive. `boot::assemble` passes the SAME
    /// [`MsgQueueWakeupScheduler`] it uses to fill the tool's `WakeupSchedulerCell`.
    #[must_use]
    pub fn with_wakeup_scheduler(mut self, scheduler: Arc<dyn tool_cron::WakeupScheduler>) -> Self {
        self.loop_runtime = scheduler.loop_runtime();
        self.wakeup_scheduler = Some(scheduler);
        self
    }

    /// Wire the mid-turn-input counter so the `/loop` fold can veto a tick a
    /// human interrupted with a prompt (`foreign_user_input`). Additive; without
    /// it that arm simply never fires.
    #[must_use]
    pub fn with_foreign_input_counter(
        mut self,
        counter: Arc<std::sync::atomic::AtomicU32>,
    ) -> Self {
        self.foreign_inputs = Some(counter);
        self
    }

    /// Map an [`OrchestratorError`] to a wire [`ClientEvent::Error`].
    ///
    /// The coarse [`ErrorKindDto`] mirrors the streaming output stream's own
    /// classification: API/transport failures are `Transport`, a stream-protocol
    /// violation is `Protocol`, a `max_turns` budget hit is `MaxTurns`, and any
    /// other internal failure is `Internal`.
    fn error_event(err: &OrchestratorError) -> ClientEvent {
        let kind = match err {
            // RateLimitRejected is an enriched ApiCall(RateLimited) — same
            // Transport class as the error it replaces (matches client-adapter).
            OrchestratorError::ApiCall(_)
            | OrchestratorError::Streaming(_)
            | OrchestratorError::RateLimitRejected { .. } => ErrorKindDto::Transport,
            OrchestratorError::StreamingProtocol(_) | OrchestratorError::StreamEndedWithoutStop => {
                ErrorKindDto::Protocol
            }
            OrchestratorError::MaxTurnsReached { .. } => ErrorKindDto::MaxTurns,
            _ => ErrorKindDto::Internal,
        };
        ClientEvent::Error {
            kind,
            message: err.to_string(),
        }
    }

    /// Convert the wire [`ImageRefDto`]s (uniform inline `{media_type, base64}`,
    /// decision §0.8) into the canonical [`ImageSource::Base64`]. The media type
    /// and base64 bytes are taken STRAIGHT from the DTO — no content sniffing and
    /// no temp-file round-trip; the inline bytes ride directly onto the outgoing
    /// user message.
    fn to_image_sources(images: Vec<ImageRefDto>) -> Vec<ImageSource> {
        images
            .into_iter()
            .map(|dto| ImageSource::Base64 {
                media_type: dto.media_type,
                data: dto.base64,
            })
            .collect()
    }

    /// Announce a turn the CLIENT did not submit, before it starts producing.
    ///
    /// [`crate::server::drain_main_thread`] runs whatever is left on the queue
    /// as its OWN follow-up turn — a `/loop` tick, a `Now`-priority interrupt, a
    /// slash command typed mid-turn. claude-code drains that same queue into the
    /// ordinary submit path, so the turn it produces is indistinguishable from a
    /// typed one: spinner, transcript, permission prompts. A host that learns
    /// about turns only from the prompts IT sent would render none of it, and
    /// would keep its composer unlocked and its Stop button hidden while the
    /// engine works.
    ///
    /// The task-notification rewake is announced one layer up instead — the
    /// orchestrator's own `emit_turn_started` (`conversation::drivers`) reaches
    /// the client through this same [`client::adapter::AdapterOutputStream`], so
    /// announcing it here as well would double-emit.
    async fn announce_engine_initiated_turn(&self) {
        use lingxi_core::host::OutputStream;
        if let Some(output) = &self.message_output {
            output.emit_turn_started().await;
        }
    }

    /// Drive ONE streaming turn with already-decoded image `sources`, surfacing a
    /// turn-level failure as a terminal [`ClientEvent::Error`] when an error sink
    /// is wired. Shared by [`TurnDriver::run_turn`] (no images) and
    /// [`TurnDriver::run_turn_with_images`]; an empty `sources` vector is
    /// byte-identical to the pre-MULTIMODAL.1 text-only turn.
    async fn drive_turn(
        &self,
        prompt: String,
        sources: Vec<ImageSource>,
        cancel: CancellationToken,
        notification_registry: Option<
            Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
        >,
        in_human_turn: bool,
    ) {
        self.drive_turn_with_inputs(
            prompt,
            sources,
            cancel,
            notification_registry,
            in_human_turn,
            None,
        )
        .await;
    }

    async fn drive_turn_with_inputs(
        &self,
        prompt: String,
        sources: Vec<ImageSource>,
        cancel: CancellationToken,
        notification_registry: Option<
            Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
        >,
        in_human_turn: bool,
        inputs: Option<Vec<orchestrator::QueuedPromptInput>>,
    ) {
        lingxi_core::host::live_sessions::set_process_status("busy", None);
        if let Some(output) = &self.message_output {
            output.reset_message_buffer().await;
        }
        // NOW-ABORT wiring: register this turn's token with the queue so a `Now`
        // enqueue aborts it, and RESET the abort-reason flag to `UserInterrupt`
        // so a stale `QueueNowCommand` from the previous turn can't mislabel this
        // one. Both no-ops when no queue is wired (the test drivers).
        if let Some(reason) = self.cancel_reason.as_ref() {
            reason.reset();
        }
        // Fresh `/loop` fold span. Reset for EVERY turn, not just loop ticks —
        // a count carried over from a working turn would veto the next quiet
        // tick (or, worse, inflate its telemetry).
        self.orchestrator.turn_span().reset();
        let foreign_inputs_before = self
            .foreign_inputs
            .as_ref()
            .map_or(0, |c| c.load(std::sync::atomic::Ordering::Relaxed));
        if let Some(queue) = self.queue.as_ref() {
            queue.register_active_turn(cancel.clone()).await;
        }
        let cancel_probe = cancel.clone();
        let result = if let Some(registry) = notification_registry {
            self.orchestrator
                .run_task_notification_rewake(registry.as_ref(), cancel)
                .await
        } else if let Some(inputs) = inputs {
            self.orchestrator
                .run_queued_prompt_batch(inputs, cancel)
                .await
        } else {
            self.orchestrator
                .run_turn_streaming_with_origin(&prompt, sources, cancel, None, in_human_turn)
                .await
        };
        // USER ABORT (binary `t3t`): the user interrupted this turn, so every
        // pending dynamic-loop wakeup is cancelled, the in-flight tick is
        // dropped, their chain-start records are forgotten and the loop ends
        // with `tengu_loop_ended{user_abort}`. A `Now`-command abort is NOT a
        // user abort — the queue is interrupting to run something else, and the
        // loop must survive it — so it falls through to the keepalive edge.
        //
        // Otherwise: KEEPALIVE (binary loading→idle `useEffect`). If the
        // just-completed turn was a dynamic `/loop` tick that did NOT reschedule,
        // arm one fallback heartbeat (`lKi`). `maybe_arm_keepalive` is a no-op
        // for non-loop-tick turns (no in-flight prompt) and when the keepalive
        // gate is off. A hard turn failure is terminal for automatic work: it
        // must end the loop instead of scheduling the same failing work again.
        let user_aborted = cancel_probe.is_cancelled()
            && self.cancel_reason.as_ref().is_none_or(|reason| {
                reason.get() == orchestrator::prompt::mid_turn_input::CancelReason::UserInterrupt
            });
        // NO-OP FOLD (binary `D()` → `v()`): settle the tick that just ended
        // into the `/loop` no-op streak. The oracle decides this at the NEXT
        // fire by walking the transcript span since the last one; LingXi
        // delivers a wakeup as one queued command, so the span IS this turn and
        // the verdict is taken here. Runs BEFORE the two edges below because
        // both consume the in-flight tick marker `settle_loop_tick` reads.
        //
        // The veto arms, in the order the oracle would meet them walking the
        // span: the blocking system message first, then per-message
        // disturbances. Only ORDER is approximated — the oracle returns on the
        // first veto in transcript POSITION, which a tally cannot reconstruct;
        // every arm's trigger is the same fact it reads.
        //
        // There is no `split_tool_pair` arm, and there cannot be one: it fires
        // when a `tool_result` in the span has no matching `tool_use`, which
        // happens in the oracle because the span STARTS at a fire anchor that
        // can fall between the two. LingXi's span is a whole turn, and a turn
        // always holds the assistant message before its own tool results.
        // `blocking_system_before_anchor` is unreachable for the same reason.
        if let Some(runtime) = self.loop_runtime.as_ref() {
            let span = self.orchestrator.turn_span().snapshot();
            let foreign_inputs = self
                .foreign_inputs
                .as_ref()
                .map_or(0, |c| c.load(std::sync::atomic::Ordering::Relaxed))
                .saturating_sub(foreign_inputs_before);
            if span.compactions > 0 {
                runtime.reset_autonomous_loop_delivered();
                runtime.veto_tick(tool_cron::LoopFoldVeto::BlockingSystemInSpan);
            }
            if user_aborted || span.aborts > 0 {
                runtime.veto_tick(tool_cron::LoopFoldVeto::ToolAbort);
            }
            if span.denials > 0 {
                runtime.veto_tick(tool_cron::LoopFoldVeto::ToolDenial);
            }
            if foreign_inputs > 0 {
                runtime.veto_tick(tool_cron::LoopFoldVeto::ForeignUserInput);
            }
            if cancel_probe.is_cancelled() && !user_aborted {
                runtime.veto_tick(tool_cron::LoopFoldVeto::QueuedCommand);
            }
            if runtime.in_flight_prompt().is_none()
                && (in_human_turn || span.compactions > 0 || span.denials > 0 || span.aborts > 0)
            {
                runtime.invalidate_noop_streak();
            }
            tool_cron::settle_loop_tick(
                runtime,
                tool_cron::LoopSpanCounts {
                    tool_uses: span.tool_uses,
                    span_len: span.messages,
                },
            );
        }
        let hard_failure = result.is_err();
        if let Some(scheduler) = self.wakeup_scheduler.as_ref() {
            if user_aborted {
                tool_cron::cancel_dynamic_loop_on_user_abort(scheduler).await;
            } else if hard_failure {
                tool_cron::stop_dynamic_loop(Some(scheduler)).await;
            } else if let Some(runtime) = self.loop_runtime.as_ref() {
                tool_cron::maybe_arm_keepalive_with_runtime(scheduler, runtime).await;
            } else {
                tool_cron::maybe_arm_keepalive(scheduler).await;
            }
        }
        // Release the scheduler only after no-op/abort/keepalive bookkeeping:
        // a due wakeup must observe the settled streak, never the prior tick.
        if let Some(queue) = self.queue.as_ref() {
            queue.clear_active_turn().await;
        }
        match result {
            // Success / cancellation / max-turns all already produced their
            // terminal events through the orchestrator's output stream
            // (`TurnEnded`, etc.) — nothing more to push here.
            Ok(_) => {}
            // A hard failure never reached `emit_end_turn`, so surface it
            // explicitly as a terminal `Error` event (when an error sink is wired)
            // rather than letting the client hang.
            Err(err) => {
                if let Some(output) = &self.message_output {
                    output.reset_message_buffer().await;
                }
                if let Some(sink) = &self.error_sink {
                    sink.emit(Self::error_event(&err)).await;
                    // `Error` is also used by non-turn commands, so clients
                    // cannot safely treat every error as a turn terminal. Make
                    // the hard-turn failure explicit with the same terminal
                    // event shape normal orchestrator completion emits.
                    let outcome = if cancel_probe.is_cancelled() {
                        TurnOutcomeDto::Cancelled
                    } else {
                        TurnOutcomeDto::EndTurn
                    };
                    sink.emit(ClientEvent::TurnEnded {
                        outcome,
                        stop_reason: Some("error".to_string()),
                        cost: lower_cost_snapshot(&self.orchestrator.snapshot_cost_real().await),
                    })
                    .await;
                } else {
                    tracing::debug!(error = %err, "bridge-server: turn failed (no error sink)");
                }
            }
        }
        lingxi_core::host::live_sessions::set_process_status("idle", None);
    }
}

#[async_trait]
impl TurnDriver for OrchestratorTurnDriver {
    async fn stop_dynamic_loop(&self) {
        tool_cron::stop_dynamic_loop(self.wakeup_scheduler.as_ref()).await;
        if let Some(runtime) = &self.loop_runtime {
            runtime.reset();
        }
    }

    fn resolve_loop_prompt(&self, prompt: &str) -> std::io::Result<String> {
        let fallback = tool_cron::LoopRuntime::default();
        let runtime = self.loop_runtime.as_deref().unwrap_or(&fallback);
        runtime.try_resolve_loop_default_fire(
            prompt,
            &self.orchestrator.project_root(),
            &self.orchestrator.current_cwd(),
        )
    }

    async fn loop_prompt_failed(&self, error: std::io::Error) {
        if let Some(sink) = &self.error_sink {
            sink.emit(Self::error_event(
                &orchestrator::OrchestratorError::Internal(error.to_string()),
            ))
            .await;
        } else {
            tracing::warn!(%error, "could not read scheduled loop instructions");
        }
    }

    async fn run_scheduled_turn(
        &self,
        prompt: String,
        model: String,
        reasoning: client::protocol::controls::ReasoningSelectionDto,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        use lingxi_core::host::OrchestratorHandle;
        if !self.orchestrator.workspace_trusted().await {
            return Err("paused:Trust this workspace before running scheduled tasks".into());
        }
        let reasoning = crate::router::decode_reasoning_selection(reasoning);
        if let Some(output) = &self.message_output {
            output.reset_message_buffer().await;
        }
        if let Some(queue) = &self.queue {
            queue.register_active_turn(cancel.clone()).await;
        }
        let cancel_probe = cancel.clone();
        let result = self
            .orchestrator
            .run_scheduled_turn(&prompt, &model, reasoning, cancel)
            .await;
        if let Some(queue) = &self.queue {
            queue.clear_active_turn().await;
        }
        // `run_scheduled_turn_locked` emits `TurnStarted` before it can fail, and
        // a client releases `activeTurn` only on `TurnEnded`/`SessionEnded` —
        // `ScheduledRunFinished` is host-private. Propagating the error with `?`
        // therefore left the composer locked and the Stop button live forever.
        // Same terminal shape `drive_turn_with_inputs` emits on a hard failure.
        let result = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Some(sink) = &self.error_sink {
                    sink.emit(ClientEvent::TurnEnded {
                        outcome: if cancel_probe.is_cancelled() {
                            TurnOutcomeDto::Cancelled
                        } else {
                            TurnOutcomeDto::EndTurn
                        },
                        stop_reason: Some("error".to_string()),
                        cost: lower_cost_snapshot(&self.orchestrator.snapshot_cost_real().await),
                    })
                    .await;
                }
                return Err(error);
            }
        };
        match result {
            orchestrator::conversation::TurnOutcome::EndTurn => Ok(self
                .orchestrator
                .snapshot_history()
                .await
                .iter()
                .rev()
                .find(|message| {
                    matches!(
                        message,
                        lingxi_core::types::ConversationMessage::Assistant { .. }
                    )
                })
                .map(lingxi_core::types::ConversationMessage::text_content)
                .unwrap_or_default()),
            orchestrator::conversation::TurnOutcome::Cancelled => {
                Err("cancelled:Scheduled run cancelled".into())
            }
            _ => Err("Scheduled run reached its turn limit".into()),
        }
    }

    async fn run_queued_batch(
        &self,
        inputs: Vec<orchestrator::QueuedPromptInput>,
        cancel: CancellationToken,
    ) {
        let human = inputs.iter().any(|input| !input.is_meta);
        self.announce_engine_initiated_turn().await;
        self.drive_turn_with_inputs(String::new(), Vec::new(), cancel, None, human, Some(inputs))
            .await;
    }

    async fn run_queued_turn(
        &self,
        prompt: String,
        in_human_turn: bool,
        cancel: CancellationToken,
    ) {
        self.announce_engine_initiated_turn().await;
        self.drive_turn(prompt, Vec::new(), cancel, None, in_human_turn)
            .await;
    }

    async fn run_task_notification_turn(
        &self,
        registry: Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
        cancel: CancellationToken,
    ) {
        self.drive_turn(String::new(), Vec::new(), cancel, Some(registry), false)
            .await;
    }

    async fn run_turn(&self, prompt: String) {
        // No images: drive with an empty source set — identical to routing through
        // `run_turn_streaming_with_cancel` (which decodes `&[]` to an empty vec).
        self.drive_turn(prompt, Vec::new(), CancellationToken::new(), None, true)
            .await;
    }

    /// MULTIMODAL.1: route pasted/attached images through to the model instead of
    /// dropping them. Each inline `ImageRefDto` becomes an
    /// [`ImageSource::Base64`], which the orchestrator appends to the outgoing
    /// user message via `ConversationMessage::user_with_images`.
    async fn run_turn_with_images(&self, prompt: String, images: Vec<ImageRefDto>) {
        let sources = Self::to_image_sources(images);
        self.drive_turn(prompt, sources, CancellationToken::new(), None, true)
            .await;
    }

    async fn run_turn_with_cancel(&self, prompt: String, cancel: CancellationToken) {
        self.drive_turn(prompt, Vec::new(), cancel, None, true)
            .await;
    }

    async fn run_turn_with_images_and_cancel(
        &self,
        prompt: String,
        images: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        self.drive_turn(prompt, Self::to_image_sources(images), cancel, None, true)
            .await;
    }
}

/// Crate-shared serialization lock for tests that touch process-global runtime
/// state (loop runtime, telemetry overrides, or live-session/UDS registration)
/// across the `driver`, `server`, and `boot` test modules.
#[cfg(test)]
pub(crate) static LOOP_KA_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use client::adapter::{AdapterOutputStream, ClientEventSink, MockSink};
    use client::protocol::commands::ImageRefDto;
    use lingxi_core::host::{BudgetError, WorkflowOutputScope, WorkflowOutputScopes};
    use lingxi_core::types::{ContentBlock, ConversationMessage};
    use orchestrator::conversation::ImageSource;
    use orchestrator::test_support::{
        content_block_start_text, content_block_stop, message_delta_stop, message_start,
        message_stop, noop_hook_executor, text_delta, MockApiClient, MockStreamingApiClient,
        NoOpPermissionGate, StaticMemoryProvider,
    };
    use orchestrator::{scripted, ConversationOrchestrator, OrchestratorConfig};
    use permission::gate::PermissionGate;

    use super::OrchestratorTurnDriver;
    use crate::server::TurnDriver;

    /// A mock streaming client scripting ONE minimal turn (assistant text, then
    /// `end_turn`) so a `run_turn*` call drives to completion and captures the
    /// outgoing request via `captured_calls()`.
    fn streaming_one_turn() -> Arc<MockStreamingApiClient> {
        Arc::new(MockStreamingApiClient::with_turns(vec![scripted![
            message_start("m1", "claude-sonnet-4-20250514"),
            content_block_start_text(0),
            text_delta(0, "ok"),
            content_block_stop(0),
            message_delta_stop("end_turn"),
            message_stop(),
        ]]))
    }

    /// Build a production driver over an orchestrator wired to `streaming` (whose
    /// `captured_calls()` records the outgoing request messages). No network, no
    /// API key — the mock streaming client scripts the whole turn.
    fn build_driver(streaming: Arc<MockStreamingApiClient>) -> OrchestratorTurnDriver {
        let batched = Arc::new(MockApiClient::new(Vec::new()));
        let sink = MockSink::arc();
        let output: Arc<dyn lingxi_core::host::OutputStream> =
            Arc::new(AdapterOutputStream::new(sink as Arc<dyn ClientEventSink>));
        let tools = Arc::new(tool_api::registry::ToolRegistry::new());
        let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched,
            streaming,
            tools,
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate) as Arc<dyn PermissionGate>,
            output,
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        ));
        OrchestratorTurnDriver::new(orchestrator)
    }

    struct RejectingOutputScopes;

    #[async_trait::async_trait]
    impl WorkflowOutputScopes for RejectingOutputScopes {
        async fn begin_turn(
            &self,
            _: lingxi_core::types::SessionId,
            _: lingxi_core::types::MessageId,
            _: Option<u64>,
        ) -> Result<WorkflowOutputScope, BudgetError> {
            Err(BudgetError::Internal(
                "output persistence is unavailable".into(),
            ))
        }

        fn capture(
            &self,
            _: lingxi_core::types::SessionId,
        ) -> Result<WorkflowOutputScope, BudgetError> {
            Err(BudgetError::Internal(
                "output persistence is unavailable".into(),
            ))
        }
    }

    fn build_driver_with_rejected_output_scope() -> OrchestratorTurnDriver {
        let driver = build_driver(streaming_one_turn());
        let orchestrator = Arc::try_unwrap(driver.orchestrator)
            .ok()
            .expect("test driver is the sole orchestrator owner")
            .with_workflow_output_scopes(Arc::new(RejectingOutputScopes));
        OrchestratorTurnDriver::new(Arc::new(orchestrator))
    }

    #[test]
    fn loop_file_resolution_uses_session_directory_and_rereads_edits() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tool_cron::reset_autonomous_loop_delivered();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("loop.md"), "First session task").unwrap();
        let driver = build_driver(streaming_one_turn());
        let orchestrator = Arc::try_unwrap(driver.orchestrator)
            .ok()
            .unwrap()
            .with_current_cwd(Arc::new(std::sync::Mutex::new(
                directory.path().to_path_buf(),
            )));
        let driver = OrchestratorTurnDriver::new(Arc::new(orchestrator));
        assert!(driver
            .resolve_loop_prompt("<<loop.md-dynamic>>")
            .unwrap()
            .contains("First session task"));
        std::fs::write(directory.path().join("loop.md"), "Updated session task").unwrap();
        assert!(driver
            .resolve_loop_prompt("<<loop.md-dynamic>>")
            .unwrap()
            .contains("Updated session task"));
        tool_cron::reset_autonomous_loop_delivered();
    }

    /// [`build_driver`] plus the orchestrator it wraps, for tests that read the
    /// per-turn tally back off it.
    fn build_driver_with_orchestrator(
        streaming: Arc<MockStreamingApiClient>,
    ) -> (OrchestratorTurnDriver, Arc<ConversationOrchestrator>) {
        let driver = build_driver(streaming);
        let orchestrator = driver.orchestrator.clone();
        (driver, orchestrator)
    }

    /// Extract the content blocks of the FIRST user message in a captured request.
    fn first_user_content(messages: &[ConversationMessage]) -> Vec<ContentBlock> {
        messages
            .iter()
            .find_map(|m| match m {
                // R-P1: skip the leading additional-context `<system-reminder>`
                // meta (is_meta: true); the real prompt + images ride on the
                // first NON-meta user message.
                ConversationMessage::User {
                    content,
                    is_meta: false,
                    ..
                } => Some(content.clone()),
                _ => None,
            })
            .expect("a non-meta user message must be in the outgoing request")
    }

    /// The pure DTO→source conversion preserves the media type and base64 bytes
    /// verbatim (no sniffing, order preserved) — the load-bearing MULTIMODAL.1 map.
    #[test]
    fn image_ref_dto_converts_to_base64_source_preserving_fields() {
        let dtos = vec![
            ImageRefDto {
                media_type: "image/png".to_string(),
                base64: "iVBORw0KGgoAAAA".to_string(),
            },
            ImageRefDto {
                media_type: "image/jpeg".to_string(),
                base64: "/9j/4AAQSkZJRg".to_string(),
            },
        ];

        let sources = OrchestratorTurnDriver::to_image_sources(dtos);

        assert_eq!(
            sources,
            vec![
                ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgoAAAA".to_string(),
                },
                ImageSource::Base64 {
                    media_type: "image/jpeg".to_string(),
                    data: "/9j/4AAQSkZJRg".to_string(),
                },
            ]
        );
    }

    /// End-to-end on the BRIDGE path: a turn driven with inline images lands those
    /// bytes on the OUTGOING user message as a `ContentBlock::Image` carrying the
    /// exact `ImageSource::Base64` — proving the image reaches the model instead of
    /// being silently dropped.
    #[tokio::test]
    async fn run_turn_with_images_reaches_outgoing_user_message() {
        let streaming = streaming_one_turn();
        let driver = build_driver(streaming.clone());

        let dto = ImageRefDto {
            media_type: "image/png".to_string(),
            base64: "iVBORw0KGgoAAAA".to_string(),
        };
        driver
            .run_turn_with_images("describe this".to_string(), vec![dto])
            .await;

        let calls = streaming.captured_calls().await;
        assert_eq!(calls.len(), 1, "exactly one stream request");
        let content = first_user_content(&calls[0].messages);

        assert!(
            content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text == "describe this")),
            "the prompt text must precede the image: {content:?}"
        );
        let source = content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Image { source } => Some(source.clone()),
                _ => None,
            })
            .expect("an image block must reach the outgoing user message");
        assert_eq!(
            source,
            ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgoAAAA".to_string(),
            }
        );
    }

    /// With an empty image set, `run_turn_with_images` builds the SAME outgoing
    /// user message as the plain text-only `run_turn` — no image blocks, identical
    /// content — proving the additive path is byte-identical when there are no
    /// images.
    #[tokio::test]
    async fn empty_images_matches_text_only_run_turn() {
        let streaming_plain = streaming_one_turn();
        build_driver(streaming_plain.clone())
            .run_turn("hello".to_string())
            .await;

        let streaming_empty = streaming_one_turn();
        build_driver(streaming_empty.clone())
            .run_turn_with_images("hello".to_string(), Vec::new())
            .await;

        let plain = first_user_content(&streaming_plain.captured_calls().await[0].messages);
        let empty = first_user_content(&streaming_empty.captured_calls().await[0].messages);

        assert_eq!(
            plain, empty,
            "empty-images user content must equal the text-only path"
        );
        assert!(
            plain
                .iter()
                .all(|b| !matches!(b, ContentBlock::Image { .. })),
            "the text-only path carries no image blocks: {plain:?}"
        );
    }

    /// Build a driver whose emitted `ClientEvent`s stay observable, with the
    /// SAME `AdapterOutputStream` wired both into the orchestrator and into the
    /// driver's `message_output` — the production shape (`boot::assemble`).
    fn build_driver_with_sink(
        streaming: Arc<MockStreamingApiClient>,
    ) -> (OrchestratorTurnDriver, Arc<MockSink>) {
        let batched = Arc::new(MockApiClient::new(Vec::new()));
        let sink = MockSink::arc();
        let message_output = AdapterOutputStream::new(sink.clone() as Arc<dyn ClientEventSink>);
        let output: Arc<dyn lingxi_core::host::OutputStream> = Arc::new(message_output.clone());
        let tools = Arc::new(tool_api::registry::ToolRegistry::new());
        let orchestrator = Arc::new(ConversationOrchestrator::new_with_streaming(
            OrchestratorConfig::default(),
            batched,
            streaming,
            tools,
            noop_hook_executor(),
            Arc::new(NoOpPermissionGate) as Arc<dyn PermissionGate>,
            output,
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from("/tmp"),
        ));
        (
            OrchestratorTurnDriver::new(orchestrator).with_message_output(message_output),
            sink,
        )
    }

    /// A turn the client did not submit must ANNOUNCE itself, and must do so
    /// BEFORE it starts producing.
    ///
    /// `drain_main_thread` runs the leftover queue as its own follow-up turn
    /// (`run_queued_turn` / `run_queued_batch`) after the previous turn's
    /// `TurnEnded` already told the client the connection went idle. Without a
    /// `TurnStarted` in front of it, a host that mirrors turn liveness sees a
    /// stream of events belonging to no turn it knows about — Electron's
    /// `BridgeManager` drops exactly those, along with the permission requests
    /// the turn parks on, so the turn runs invisibly and its tools die at the
    /// 300s permission timeout.
    ///
    /// Asserting on the FIRST event (not merely on presence) is what makes this
    /// honest: an announcement that arrives after the first `TextDelta` has
    /// already been dropped is no announcement at all.
    #[tokio::test]
    async fn a_queued_turn_announces_itself_before_it_streams() {
        let (driver, sink) = build_driver_with_sink(streaming_one_turn());

        driver
            .run_queued_turn(
                "drained from the queue".to_string(),
                false,
                CancellationToken::new(),
            )
            .await;

        let events = sink.events().await;
        assert_eq!(
            events.first(),
            Some(&client::protocol::events::ClientEvent::TurnStarted { turn_id: None }),
            "a queue-drained turn must open with TurnStarted; got {events:?}",
        );
    }

    /// The batched sibling of the test above: several queued commands joined
    /// into ONE follow-up turn is still one turn, and still announces once.
    #[tokio::test]
    async fn a_queued_batch_announces_itself_once() {
        let (driver, sink) = build_driver_with_sink(streaming_one_turn());

        driver
            .run_queued_batch(
                vec![
                    orchestrator::QueuedPromptInput {
                        goal_retry_id: None,
                        text: "first".to_string(),
                        is_meta: false,
                        message_id: None,
                        queue_priority: None,
                        scheduled_task_id: None,
                        scheduled_fire_id: None,
                    },
                    orchestrator::QueuedPromptInput {
                        goal_retry_id: None,
                        text: "second".to_string(),
                        is_meta: false,
                        message_id: None,
                        queue_priority: None,
                        scheduled_task_id: None,
                        scheduled_fire_id: None,
                    },
                ],
                CancellationToken::new(),
            )
            .await;

        let events = sink.events().await;
        assert_eq!(
            events.first(),
            Some(&client::protocol::events::ClientEvent::TurnStarted { turn_id: None }),
            "a queue-drained batch must open with TurnStarted; got {events:?}",
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    client::protocol::events::ClientEvent::TurnStarted { .. }
                ))
                .count(),
            1,
            "one turn announces once, however many commands were folded into it",
        );
    }

    // ========================================================================
    // §27 mid-turn drain adapter (`MsgQueueMidTurnInput`) + Now-abort wiring.
    // ========================================================================

    use msgqueue::{
        MessageQueueManager, QueuePriority, QueueSource, QueuedCommand, QueuedCommandContent,
    };
    use orchestrator::prompt::mid_turn_input::MidTurnInputSource;
    use std::time::SystemTime;
    use tokio_util::sync::CancellationToken;

    fn user_cmd(uuid: &str, prio: QueuePriority, text: &str) -> QueuedCommand {
        QueuedCommand {
            scheduled_task_id: None,
            scheduled_fire_id: None,
            uuid: uuid.to_string(),
            content: QueuedCommandContent::UserInput {
                text: text.to_string(),
            },
            priority: prio,
            queued_at: SystemTime::now(),
            source: QueueSource::PromptInput,
            agent_id: None,
            skip_slash_commands: false,
            is_meta: false,
        }
    }

    /// The adapter joins consecutive `Next` main-thread prompts, returns the
    /// joined text, and REMOVES the consumed commands from the queue (consume-once
    /// so the between-turn drain never re-runs them).
    #[tokio::test]
    async fn mid_turn_adapter_joins_and_consumes_queued_prompts() {
        let queue = Arc::new(MessageQueueManager::new());
        queue
            .enqueue(user_cmd("a", QueuePriority::Next, "first"))
            .await;
        queue
            .enqueue(user_cmd("b", QueuePriority::Next, "second"))
            .await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());

        let joined = adapter.take_mid_turn_input().await;
        assert_eq!(joined.as_deref(), Some("first\nsecond"));
        // Consumed — the queue is now empty, so a second drain yields None.
        assert!(queue.is_empty().await);
        assert_eq!(adapter.take_mid_turn_input().await, None);
    }

    /// A `Now`-priority command is EXCLUDED from the mid-turn drain: it has
    /// already aborted the in-flight turn and must be PRESERVED in the queue for
    /// the between-turn drain to run as its own interrupting turn, not folded
    /// into the running turn as injected text. The drain still consumes the
    /// `Next` prompt that precedes it.
    #[tokio::test]
    async fn mid_turn_adapter_excludes_now_and_preserves_it() {
        let queue = Arc::new(MessageQueueManager::new());
        queue
            .enqueue(user_cmd("a", QueuePriority::Next, "first"))
            .await;
        queue
            .enqueue(user_cmd("urgent", QueuePriority::Now, "do it now"))
            .await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());

        // Only the `Next` prompt is drained mid-turn; the `Now` command is left.
        let joined = adapter.take_mid_turn_input().await;
        assert_eq!(joined.as_deref(), Some("first"));

        // The `Now` command survives for the between-turn drain.
        assert_eq!(queue.len().await, 1);
        let remaining = queue
            .get_by_max_priority(QueuePriority::Now, |_| true)
            .await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].uuid, "urgent");
        assert_eq!(remaining[0].priority, QueuePriority::Now);

        // A second mid-turn drain finds nothing batchable (Now stays excluded).
        assert_eq!(adapter.take_mid_turn_input().await, None);
        assert_eq!(queue.len().await, 1);
    }

    /// A slash command is EXCLUDED from the mid-turn drain (it is routed
    /// post-turn), so the adapter returns `None` when only a slash command waits.
    #[tokio::test]
    async fn mid_turn_adapter_excludes_slash_commands() {
        let queue = Arc::new(MessageQueueManager::new());
        queue
            .enqueue(user_cmd("s", QueuePriority::Next, "/clear"))
            .await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());
        assert_eq!(adapter.take_mid_turn_input().await, None);
        // The slash command is left in the queue for the post-turn path.
        assert_eq!(queue.len().await, 1);
    }

    /// A subagent-scoped command (has an `agent_id`) is EXCLUDED by the
    /// main-thread filter, so it never leaks into the coordinator's mid-turn drain.
    #[tokio::test]
    async fn mid_turn_adapter_scopes_to_main_thread() {
        let queue = Arc::new(MessageQueueManager::new());
        let mut sub = user_cmd("sub", QueuePriority::Next, "subagent input");
        sub.agent_id = Some(lingxi_core::types::AgentId::new());
        queue.enqueue(sub).await;
        let adapter = super::MsgQueueMidTurnInput::new(queue.clone());
        assert_eq!(adapter.take_mid_turn_input().await, None);
        assert_eq!(queue.len().await, 1);
    }

    /// A `Now`-priority enqueue, with the driver having registered the turn's
    /// cancel token via `with_queue`, fires the token AND sets the reason flag —
    /// proving the end-to-end Now-abort wiring (the driver registers, the queue
    /// hook records the reason, the token trips).
    #[tokio::test]
    async fn with_queue_registers_token_and_now_enqueue_aborts_with_reason() {
        use orchestrator::prompt::mid_turn_input::{CancelReason, CancelReasonFlag};

        let queue = Arc::new(MessageQueueManager::new());
        let reason = CancelReasonFlag::new();
        // Install the now-abort hook exactly as boot::assemble does.
        {
            let r = reason.clone();
            queue
                .set_now_abort_hook(Arc::new(move || r.set(CancelReason::QueueNowCommand)))
                .await;
        }

        // Register a turn token (what drive_turn does at turn start).
        let token = CancellationToken::new();
        queue.register_active_turn(token.clone()).await;
        assert!(!token.is_cancelled());
        assert_eq!(reason.get(), CancelReason::UserInterrupt);

        // A Now enqueue trips the token AND records the reason.
        queue
            .enqueue(user_cmd("urgent", QueuePriority::Now, "do it now"))
            .await;
        assert!(
            token.is_cancelled(),
            "Now enqueue must abort the active turn"
        );
        assert_eq!(
            reason.get(),
            CancelReason::QueueNowCommand,
            "the now-abort hook must record QueueNowCommand before firing"
        );

        // Build a driver wired with the queue + reason to prove the API compiles
        // and the seam is reachable (smoke).
        let _driver = build_driver(streaming_one_turn()).with_queue(queue, reason);
    }

    /// A minimal real-spawning `RuntimeSpawner` for the wakeup-adapter test: it
    /// spawns the future on the current tokio runtime and uses real `sleep`.
    struct TestRuntime;
    #[async_trait::async_trait]
    impl lingxi_core::host::RuntimeSpawner for TestRuntime {
        async fn spawn(
            &self,
            name: &str,
            task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<lingxi_core::host::BackgroundTaskHandle, lingxi_core::host::runtime::RuntimeError>
        {
            tokio::spawn(task);
            Ok(lingxi_core::host::BackgroundTaskHandle {
                task_name: name.to_string(),
                task_id: 0,
            })
        }
        async fn sleep(&self, duration: std::time::Duration) {
            tokio::time::sleep(duration).await;
        }
        async fn cancel(
            &self,
            _handle: &lingxi_core::host::BackgroundTaskHandle,
        ) -> Result<(), lingxi_core::host::runtime::RuntimeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn wakeup_scheduler_preserves_sentinel_until_between_turn_drain() {
        use super::MsgQueueWakeupScheduler;
        use tool_cron::WakeupScheduler;

        // PARITY 2.1.263: the sentinel resolver has NO gate any more
        // (`tengu_kairos_loop_prompt` is absent from the binary), so a sentinel
        // always resolves. Clear the shared delivery state so the FIRST-delivery
        // branch fires.
        tool_cron::reset_autonomous_loop_delivered();

        let queue = Arc::new(MessageQueueManager::new());
        let runtime: Arc<dyn lingxi_core::host::RuntimeSpawner> = Arc::new(TestRuntime);
        let sched = MsgQueueWakeupScheduler::new(queue.clone(), runtime);

        // Schedule a 0-delay wakeup carrying the autonomous sentinel — it must be
        // resolved to the instruction block before being enqueued.
        sched
            .schedule(
                std::time::Duration::from_millis(0),
                "<<autonomous-loop-dynamic>>".to_string(),
                "idle tick".to_string(),
            )
            .await;

        // Give the spawned timer a moment to fire + enqueue.
        for _ in 0..50 {
            if queue.len().await > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(
            queue.take_mid_turn_prompt().await.is_none(),
            "scheduled input must wait until the active turn ends"
        );
        let cmd = queue
            .dequeue()
            .await
            .expect("wakeup must have enqueued one command");
        assert_eq!(cmd.priority, QueuePriority::Later);
        assert!(cmd.is_meta);
        assert!(cmd.skip_slash_commands);
        assert_eq!(cmd.source, msgqueue::QueueSource::Cron);
        assert_eq!(cmd.scheduled_task_id.as_deref().unwrap().len(), 8);
        assert!(lingxi_core::types::MessageId::parse_prefixed(
            cmd.scheduled_fire_id.as_deref().unwrap()
        )
        .is_some());
        assert!(cmd
            .uuid
            .ends_with(cmd.scheduled_fire_id.as_deref().unwrap()));
        let text = cmd.text().expect("user-input text");
        assert_eq!(
            text, "<<autonomous-loop-dynamic>>",
            "the keepalive identity must retain the sentinel, not frozen task text"
        );
    }

    #[tokio::test]
    async fn due_wakeup_waits_for_busy_turn_without_mid_turn_injection() {
        use tool_cron::WakeupScheduler;
        let queue = Arc::new(MessageQueueManager::new());
        queue.register_active_turn(CancellationToken::new()).await;
        let sched = super::MsgQueueWakeupScheduler::new(queue.clone(), Arc::new(TestRuntime));
        sched
            .schedule(
                std::time::Duration::ZERO,
                "check again".into(),
                "tick".into(),
            )
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        assert_eq!(
            queue.len().await,
            0,
            "busy turns must not receive wakeup announcements or prompts"
        );
        assert!(queue.take_mid_turn_prompt().await.is_none());
        queue.clear_active_turn().await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while queue.len().await == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the due wakeup fires once after idle");
        assert!(queue.take_mid_turn_prompt().await.is_none());
        let command = queue.dequeue().await.unwrap();
        assert_eq!(command.text(), Some("check again"));
        assert!(command.is_meta && command.skip_slash_commands);
        assert_eq!(command.priority, QueuePriority::Later);
        assert_eq!(queue.len().await, 0);
    }

    #[tokio::test]
    async fn stop_cancels_wakeup_while_announcement_is_blocked() {
        use tool_cron::WakeupScheduler;
        struct BlockedSink {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        #[async_trait::async_trait]
        impl client::adapter::ClientEventSink for BlockedSink {
            async fn emit(&self, _: client::protocol::events::ClientEvent) {
                self.entered.notify_one();
                self.release.notified().await;
            }
        }
        let queue = Arc::new(MessageQueueManager::new());
        let sink = Arc::new(BlockedSink {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let scheduler = super::MsgQueueWakeupScheduler::new(queue.clone(), Arc::new(TestRuntime))
            .with_event_sink(sink.clone());
        scheduler
            .schedule(
                std::time::Duration::ZERO,
                "stopped prompt".into(),
                "tick".into(),
            )
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(1), sink.entered.notified())
            .await
            .unwrap();
        assert_eq!(scheduler.cancel_pending().await, vec!["stopped prompt"]);
        sink.release.notify_one();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            queue.len().await,
            0,
            "stop must cancel even after the timer starts publication"
        );
    }

    #[tokio::test]
    async fn wakeup_scheduler_passthrough_prompt() {
        use super::MsgQueueWakeupScheduler;
        use tool_cron::WakeupScheduler;

        let queue = Arc::new(MessageQueueManager::new());
        let runtime: Arc<dyn lingxi_core::host::RuntimeSpawner> = Arc::new(TestRuntime);
        let sched = MsgQueueWakeupScheduler::new(queue.clone(), runtime);
        sched
            .schedule(
                std::time::Duration::from_millis(0),
                "5m /babysit-prs".to_string(),
                "repeat loop".to_string(),
            )
            .await;
        for _ in 0..50 {
            if queue.len().await > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let cmd = queue.dequeue().await.expect("enqueued");
        assert_eq!(cmd.text(), Some("5m /babysit-prs"));
    }

    use super::LOOP_KA_TEST_SERIAL;

    /// A keepalive recorder scheduler for the turn-completion trigger tests.
    struct KaRec {
        calls: std::sync::Mutex<Vec<std::time::Duration>>,
        runtime: Arc<tool_cron::LoopRuntime>,
    }
    #[async_trait::async_trait]
    impl tool_cron::WakeupScheduler for KaRec {
        async fn schedule(&self, delay: std::time::Duration, _p: String, _r: String) {
            self.calls.lock().unwrap().push(delay);
        }

        fn loop_runtime(&self) -> Option<Arc<tool_cron::LoopRuntime>> {
            Some(self.runtime.clone())
        }
    }

    /// A dynamic /loop tick that completes WITHOUT the model rescheduling arms one
    /// 1200s keepalive fallback at the turn-completion edge (binary `lKi` via the
    /// loading→idle `useEffect`).
    #[tokio::test]
    async fn keepalive_arms_after_silent_loop_tick() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 2.1.263: dynamic mode has no flag; only the keepalive gate is a flag.
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        // The drain tags a Cron-sourced command as the in-flight loop tick.
        loop_runtime.begin_tick("<<autonomous-loop-dynamic>>".to_string());

        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime.clone(),
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver(streaming_one_turn()).with_wakeup_scheduler(sched);
        // The scripted turn emits text + end_turn — NO ScheduleWakeup call.
        driver.run_turn("loop tick".to_string()).await;

        let calls = rec.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "a silent loop tick must arm one keepalive (keepalive={}, in_flight={:?}, consecutive={})",
            tool_cron::is_loop_keepalive_enabled(),
            loop_runtime.in_flight_prompt(),
            loop_runtime.consecutive_keepalives(),
        );
        // 2.1.263 minute-aligns the target (`P(m)`), so the sleep is 1200s
        // rounded up to the next whole minute.
        assert!(
            calls[0] >= std::time::Duration::from_secs(1200)
                && calls[0] <= std::time::Duration::from_secs(1260),
            "keepalive delay {:?} must be 1200s ceil'd to a minute",
            calls[0]
        );
        drop(calls);
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        loop_runtime.reset();
    }

    /// PARITY `t3t`: a loop tick the USER aborted arms no keepalive — it cancels
    /// the pending wakeups, drops the in-flight tick and ends the loop. Without
    /// the abort branch this turn takes the keepalive edge and arms 1200s.
    #[tokio::test]
    async fn user_abort_of_a_loop_tick_ends_the_loop_instead_of_arming_a_keepalive() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        loop_runtime.begin_tick("<<autonomous-loop-dynamic>>".to_string());

        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime.clone(),
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver(streaming_one_turn()).with_wakeup_scheduler(sched);

        // A pre-cancelled token with no `CancelReasonFlag` wired reads as the
        // default `UserInterrupt` — exactly the Ctrl+C / ESC path.
        let cancel = CancellationToken::new();
        cancel.cancel();
        driver
            .run_turn_with_cancel("loop tick".to_string(), cancel)
            .await;

        assert!(
            rec.calls.lock().unwrap().is_empty(),
            "a user-aborted loop tick must NOT arm a keepalive"
        );
        assert_eq!(
            loop_runtime.in_flight_prompt(),
            None,
            "the aborted tick must be dropped"
        );
        assert!(loop_runtime.loop_ended(), "user abort ends the loop");
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        loop_runtime.reset();
    }

    #[tokio::test]
    async fn failed_loop_tick_ends_the_loop_instead_of_retrying_forever() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        loop_runtime.begin_tick("<<autonomous-loop-dynamic>>".to_string());
        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime.clone(),
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver_with_rejected_output_scope().with_wakeup_scheduler(sched);

        driver.run_turn("loop tick".to_string()).await;

        assert!(
            rec.calls.lock().unwrap().is_empty(),
            "a failed loop tick must not schedule another attempt"
        );
        assert!(
            loop_runtime.loop_ended(),
            "a hard failure must end the loop"
        );
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        loop_runtime.reset();
    }

    /// The `/loop` fold's span tally has to be BUMPED BY THE TURN LOOP, not
    /// merely defined: `turn_span`'s own unit tests call the counter directly
    /// and pass whether or not production ever does.
    ///
    /// A text-only turn is the sharpest probe available — it calls
    /// `note_assistant_response(0)`, so `tool_uses` stays 0 while `messages`
    /// goes to exactly 1. An unwired call site leaves BOTH at 0, and reporting
    /// `span_len: 0` in `loop_noop_fold` is precisely the "reported as zero"
    /// this counter exists to avoid.
    #[tokio::test]
    async fn a_turn_bumps_the_fold_span_tally() {
        let (driver, orchestrator) = build_driver_with_orchestrator(streaming_one_turn());
        assert_eq!(
            orchestrator.turn_span().snapshot(),
            orchestrator::turn_span::TurnSpanCounts::default(),
            "a fresh orchestrator starts at zero",
        );
        driver.run_turn("hello".to_string()).await;
        let span = orchestrator.turn_span().snapshot();
        assert_eq!(
            span.messages, 1,
            "the assistant response must be counted by the turn loop (0 = call site missing)",
        );
        assert_eq!(span.tool_uses, 0, "a text-only turn calls no tools");
    }

    /// The tally is reset at the START of every turn, not only loop ticks: a
    /// count carried over from a working turn would veto the next quiet tick,
    /// or inflate its `loop_noop_fold`.
    ///
    /// Poking the counters before the turn (rather than running two turns) also
    /// keeps this honest with a single-turn mock — a second `run_turn` against
    /// an exhausted script never reaches the model, so it would read 0 whether
    /// or not the reset happened.
    #[tokio::test]
    async fn a_turn_resets_the_fold_span_tally_it_inherits() {
        let (driver, orchestrator) = build_driver_with_orchestrator(streaming_one_turn());
        let tally = orchestrator.turn_span();
        tally.note_assistant_response(9);
        tally.note_denial("user-rejected");
        tally.note_compaction();
        assert_eq!(
            tally.snapshot().tool_uses,
            9,
            "the leftovers are really there"
        );

        driver.run_turn("hello".to_string()).await;

        let span = orchestrator.turn_span().snapshot();
        assert_eq!(
            span,
            orchestrator::turn_span::TurnSpanCounts {
                tool_uses: 0,
                messages: 1,
                ..Default::default()
            },
            "only this turn's own assistant response survives the reset",
        );
    }

    /// PARITY `D()` + `v()`: a tick the model closed with `noop: true` folds
    /// into the streak, and the next wakeup renders it.
    #[tokio::test]
    async fn a_quiet_loop_tick_folds_into_the_streak() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        loop_runtime.begin_tick("<<autonomous-loop-dynamic>>".to_string());
        // What `ScheduleWakeup({noop:true})` records during the tick.
        loop_runtime.mark_noop_reported(true);

        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime.clone(),
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver(streaming_one_turn()).with_wakeup_scheduler(sched);
        driver.run_turn("loop tick".to_string()).await;

        let (streak, _) = loop_runtime
            .noop_streak()
            .expect("a quiet tick must fold into the streak");
        assert_eq!(streak, 1);
        let (head, companion) = tool_cron::loop_wakeup_lines(0, loop_runtime.noop_streak());
        assert!(head.contains("1 no-op tick since"), "{head}");
        assert!(companion.is_some());
        loop_runtime.reset();
    }

    /// PARITY the `tool_abort` veto: a tick the user interrupted is not quiet,
    /// whatever the model claimed before the interrupt.
    #[tokio::test]
    async fn a_user_aborted_loop_tick_vetoes_the_fold() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        loop_runtime.begin_tick("<<autonomous-loop-dynamic>>".to_string());
        loop_runtime.mark_noop_reported(true);

        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime.clone(),
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver(streaming_one_turn()).with_wakeup_scheduler(sched);
        let cancel = CancellationToken::new();
        cancel.cancel();
        driver
            .run_turn_with_cancel("loop tick".to_string(), cancel)
            .await;

        assert_eq!(
            loop_runtime.noop_streak(),
            None,
            "an interrupted tick must not count as quiet"
        );
        loop_runtime.reset();
    }

    /// A NON-loop turn (no in-flight tick) never arms a keepalive, even with a
    /// scheduler wired and the flags on.
    #[tokio::test]
    async fn human_turn_between_fires_invalidates_settled_noop_streak() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let runtime = Arc::new(tool_cron::LoopRuntime::default());
        runtime.begin_tick("quiet tick".into());
        runtime.mark_noop_reported(true);
        runtime.settle_tick(
            std::time::SystemTime::now(),
            tool_cron::LoopSpanCounts {
                tool_uses: 1,
                span_len: 2,
            },
        );
        runtime.take_in_flight_prompt();
        assert!(runtime.noop_streak().is_some());
        let scheduler = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: runtime.clone(),
        });
        build_driver(streaming_one_turn())
            .with_wakeup_scheduler(scheduler)
            .run_turn("new user work".into())
            .await;
        assert_eq!(
            runtime.noop_streak(),
            None,
            "the next fire must not fold intervening user work"
        );
    }

    #[tokio::test]
    async fn keepalive_not_armed_for_user_turn() {
        let _serial = LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 2.1.263: dynamic mode has no flag; only the keepalive gate is a flag.
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());

        let rec = Arc::new(KaRec {
            calls: std::sync::Mutex::new(Vec::new()),
            runtime: loop_runtime,
        });
        let sched: Arc<dyn tool_cron::WakeupScheduler> = rec.clone();
        let driver = build_driver(streaming_one_turn()).with_wakeup_scheduler(sched);
        driver.run_turn("just a normal user turn".to_string()).await;

        assert!(
            rec.calls.lock().unwrap().is_empty(),
            "a non-loop turn must NOT arm a keepalive"
        );
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
    }
}
