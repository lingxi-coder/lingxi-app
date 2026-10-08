//! Connection-scoped server loop for the bridge-server WebSocket transport.
//!
//! A [`BridgeConnection`] is the [`bridge::FramePump`] the generalized
//! [`bridge::McpEndpoint`] (F2-03) drives for one client connection. It is the
//! seam between the proven transport (auth + upgrade + framing, owned by the
//! endpoint) and the engine routing (owned here): it routes inbound
//! [`ClientCommand`]s to the engine and pushes the adapter's outbound
//! [`ClientEvent`]s / [`PermissionRequest`]s back as [`Frame`]s.
//!
//! ## The permission round-trip (F2-06)
//!
//! This module's reason to exist is proving the INVERTED, blocking permission
//! handshake crosses the WS boundary without deadlock:
//!
//! 1. A tool dispatch on the ENGINE task calls
//!    [`AdapterPermissionGate::check`], which parks the turn future on a oneshot
//!    and emits a [`PermissionRequest`] through the connection's
//!    [`PermissionRequestSink`] — forwarded out as [`Frame::PermissionRequest`].
//! 2. The WS READ task (a DIFFERENT task) receives an inbound
//!    [`ClientCommand::ApprovePermission`]/[`DenyPermission`] and calls
//!    [`AdapterPermissionGate::resolve`], which fires the parked oneshot.
//!
//! No deadlock is possible because the gate `await`s the oneshot — it never
//! holds a blocking lock across the await — and the read task and the engine
//! turn task are independent (the turn is SPAWNED on `SendPrompt`, so `on_frame`
//! returns promptly and the read loop keeps servicing inbound frames, including
//! the approval).
//!
//! ## Fail-closed on disconnect
//!
//! [`BridgeConnection::on_close`] (the [`bridge::FramePump`] teardown hook) drains
//! the gate's parked-request map: every in-flight `check()` resolves `Deny` the
//! moment the resolving client vanishes, so a turn can never hang waiting for an
//! approval that will never come.
//!
//! ## Single-client model
//!
//! The walking skeleton models ONE client per server (the design's single
//! Electron child). The connection state (gate, turn driver, outbound sink) lives
//! directly on the pump; multi-client fan-out is later work.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use crate::audio_bridge::{AudioRequestSink, AudioResponder};
use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{
    version_compatible, BridgeRequest, BridgeResponse, BridgeWireError, Capabilities, ClientHello,
    FramePump, FrameSink, ServerHello, BRIDGE_PROTOCOL_VERSION,
};
use client::adapter::{
    AskUserQuestionBroker, ClientEventSink, ComputerAccessRequestSink, PermissionRequestSink,
};
use client::protocol::commands::{ClientCommand, ImageRefDto, UiSurfaceDto};
use client::protocol::computer_access::{ComputerAccessRequestDto, ComputerAccessResponseDto};
use client::protocol::events::{ClientEvent, ErrorKindDto};
use client::protocol::permission::{PermissionKindDto, PermissionRequest, PermissionResponseDto};
use msgqueue::{
    join_prompt_values, MessageQueueManager, QueuePriority, QueueSource, QueuedCommand,
    QueuedCommandContent, TelemetryQueueRecorder,
};
use permission::computer_access::ComputerAccessExchange;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tool_api::ask_user_question::AskUserQuestionExchange;

use client::adapter::AdapterPermissionGate;
use client::adapter::ComputerAccessBroker;

use crate::router::CommandRouter;

/// The engine entry a [`BridgeConnection`] calls to drive a turn from an inbound
/// [`ClientCommand::SendPrompt`].
///
/// Abstracting the turn entry behind a trait keeps the connection loop decoupled
/// from HOW the orchestrator was built: the production server builds it via
/// `harness_runtime::desktop::build`, while the F2-06 e2e test wires a
/// `ConversationOrchestrator` with a mock streaming client + a real tool that
/// routes through the same `AdapterPermissionGate`. Both reach the gate the same
/// way — through `orchestrator.perms.check()` during tool dispatch.
#[async_trait]
pub trait TurnDriver: Send + Sync + 'static {
    /// Drive ONE turn for `prompt`. Called on a SPAWNED task so the WS read loop
    /// keeps servicing inbound frames (notably the approval that unblocks a
    /// parked permission `check()`); the implementation streams its events out
    /// through the connection's [`ClientEventSink`] as a side effect.
    async fn run_turn(&self, prompt: String);
    /// Mounted conversation identity for immutable permission owners.
    async fn current_session_id(&self) -> Option<String> {
        None
    }
    /// Attach one explicitly mounted UI client to this live session.
    async fn ui_attach(&self, _client_id: &str, _surface: UiSurfaceDto) -> bool {
        false
    }
    /// Detach one UI client owned by this bridge connection.
    async fn ui_detach(&self, _client_id: &str) -> bool {
        false
    }
    /// Cancel session-local timers before replacing the conversation.
    async fn stop_dynamic_loop(&self) {}

    /// Resolve a scheduled sentinel against this session's working directory.
    fn resolve_loop_prompt(&self, prompt: &str) -> std::io::Result<String> {
        let cwd = std::env::current_dir()?;
        tool_cron::LoopRuntime::default().try_resolve_loop_default_fire(prompt, &cwd, &cwd)
    }
    async fn loop_prompt_failed(&self, error: std::io::Error) {
        tracing::warn!(%error, "could not read scheduled loop instructions");
    }
    async fn run_scheduled_turn(
        &self,
        _prompt: String,
        _model: String,
        _reasoning: client::protocol::controls::ReasoningSelectionDto,
        _cancel: CancellationToken,
    ) -> Result<String, String> {
        Err("paused:Scheduled session execution is unavailable".into())
    }
    async fn run_queued_batch(
        &self,
        inputs: Vec<orchestrator::QueuedPromptInput>,
        cancel: CancellationToken,
    ) {
        let human = inputs.iter().any(|input| !input.is_meta);
        self.run_queued_turn(
            inputs
                .into_iter()
                .map(|input| input.text)
                .collect::<Vec<_>>()
                .join("\n"),
            human,
            cancel,
        )
        .await;
    }
    async fn run_queued_turn(
        &self,
        prompt: String,
        _in_human_turn: bool,
        cancel: CancellationToken,
    ) {
        self.run_turn_with_cancel(prompt, cancel).await;
    }

    /// Drive complete queued input after its predecessor has ended. Production
    /// drivers announce this new turn before streaming or requesting approvals.
    async fn run_queued_turn_with_images(
        &self,
        prompt: String,
        images: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        self.run_turn_with_images_and_cancel(prompt, images, cancel)
            .await;
    }

    /// A registry completion starts a machine turn, without a synthetic prompt.
    async fn run_task_notification_turn(
        &self,
        _registry: Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
        _cancel: CancellationToken,
    ) {
    }

    /// Drive ONE turn for `prompt` carrying the inline images pasted/attached by
    /// the client (the wire [`ImageRefDto`]s from
    /// [`ClientCommand::SendPrompt`](client::protocol::commands::ClientCommand::SendPrompt)).
    ///
    /// ADDITIVE over [`Self::run_turn`]: the DEFAULT body DROPS the images and
    /// delegates to `run_turn`, so every existing impl (including the test drivers)
    /// keeps compiling and behaving exactly as before. Only
    /// [`crate::driver::OrchestratorTurnDriver`] OVERRIDES this to route the images
    /// through to the model. With an empty `images` vector the override is, by
    /// construction, identical to the plain text-only `run_turn` path — so the
    /// dispatch can always call this entry without a special-case for "no images".
    async fn run_turn_with_images(&self, prompt: String, images: Vec<ImageRefDto>) {
        // Default: images are not supported by this driver — drop them and run the
        // text-only turn (byte-identical to the pre-MULTIMODAL.1 behavior).
        let _ = images;
        self.run_turn(prompt).await;
    }

    /// Cancel-aware text-only entry used by the connection-owned turn slot.
    /// Legacy/test drivers keep their existing natural-completion behavior;
    /// the production orchestrator driver overrides this and consumes the
    /// token at its structured cancellation checkpoints.
    async fn run_turn_with_cancel(&self, prompt: String, cancel: CancellationToken) {
        let _ = cancel;
        self.run_turn(prompt).await;
    }

    /// Cancel-aware multimodal sibling of [`Self::run_turn_with_images`].
    async fn run_turn_with_images_and_cancel(
        &self,
        prompt: String,
        images: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        let _ = cancel;
        self.run_turn_with_images(prompt, images).await;
    }
}

/// Connection-scoped outbound channel. Holds the per-connection [`FrameSink`]
/// once the transport hands it to us (on the first inbound frame). Before that
/// it is empty and emits are dropped (no client is listening yet); after the
/// connection closes the underlying mpsc is gone and `FrameSink::send` is a
/// silent no-op.
type SharedFrameSink = Arc<Mutex<Option<FrameSink>>>;
type SharedPermissionGate = Arc<StdMutex<Option<Weak<AdapterPermissionGate>>>>;
type SharedComputerAccessBroker = Arc<StdMutex<Option<Weak<ComputerAccessBroker>>>>;
type SharedAskUserQuestionBroker = Arc<StdMutex<Option<Weak<AskUserQuestionBroker>>>>;
type QueuedPromptPayloads = Arc<Mutex<HashMap<String, QueuedPromptPayload>>>;
type QuestionRejections = Arc<StdMutex<Vec<tokio::task::JoinHandle<()>>>>;

/// The SDK queue intentionally stores text only. Inputs carrying attachments
/// or a client turn identity stay whole until a distinct follow-up turn owns
/// them; they must never be folded into another turn's text-only input.
struct QueuedPromptPayload {
    images: Vec<ImageRefDto>,
    turn_id: Option<u64>,
}

/// A [`ClientEventSink`] that forwards each lowered [`ClientEvent`] out as a
/// [`Frame::Event`] on the connection's outbound channel.
struct FrameEventSink {
    out: SharedFrameSink,
    pending_openai_oauth: Arc<Mutex<Option<ClientEvent>>>,
    cron_requests: Arc<crate::cron_host::HostCronRequests>,
    handshaken: Arc<AtomicBool>,
    active_turn: ActiveTurnControl,
    ask_user_question_broker: SharedAskUserQuestionBroker,
    question_rejections: QuestionRejections,
}

/// Event sink for connection-level command output (currently display-only slash
/// commands). It deliberately bypasses turn ownership because no turn exists.
struct UnscopedFrameEventSink {
    out: SharedFrameSink,
}

async fn forward_event(out: &SharedFrameSink, event: ClientEvent) {
    if let Some(sink) = out.lock().await.as_ref() {
        let _ = sink.send(Frame::Event(event));
    }
}

fn is_owned_turn_event(event: &ClientEvent) -> bool {
    matches!(
        event,
        ClientEvent::SystemNotice { .. }
            | ClientEvent::UiLog { .. }
            | ClientEvent::UiToast { .. }
            | ClientEvent::UiStatus { .. }
            | ClientEvent::TextDelta { .. }
            | ClientEvent::AskUserQuestion { .. }
            | ClientEvent::TurnStarted { .. }
            | ClientEvent::TurnEnded { .. }
            | ClientEvent::ToolUseStarted { .. }
            | ClientEvent::ToolHeartbeat { .. }
            | ClientEvent::ToolUseResult { .. }
            | ClientEvent::MessageComplete { .. }
            | ClientEvent::CostUpdate { .. }
            | ClientEvent::ThinkingDelta { .. }
            | ClientEvent::UsageUpdate { .. }
            | ClientEvent::ApiRetry { .. }
            | ClientEvent::CompactionCompleted { .. }
            | ClientEvent::Attachment { .. }
    )
}

#[async_trait]
impl ClientEventSink for FrameEventSink {
    async fn emit(&self, mut event: ClientEvent) {
        if matches!(&event, ClientEvent::CronRunRequested { .. }) {
            let out = self.out.lock().await;
            self.cron_requests
                .enqueue(event, self.handshaken.load(Ordering::SeqCst), |event| {
                    out.as_ref()
                        .is_some_and(|sink| sink.send(Frame::Event(event)))
                })
                .await;
            return;
        }
        if matches!(&event, ClientEvent::OpenAiOAuthUpdated { .. }) {
            let out = self.out.lock().await;
            let mut pending = self.pending_openai_oauth.lock().await;
            if self.handshaken.load(Ordering::SeqCst) {
                if let Some(sink) = out.as_ref() {
                    if sink.send(Frame::Event(event.clone())) {
                        *pending = None;
                        return;
                    }
                }
            }
            // Boot may rotate an expired token before the host handshakes.
            *pending = Some(event);
            return;
        }

        // AskUserQuestion is parked in the broker before this event reaches
        // the sink. Resolve it fail-closed before applying the generic event
        // filter, otherwise an unowned/terminal request would be dropped while
        // leaving its broker receiver parked until timeout.
        if let ClientEvent::AskUserQuestion { request } = &event {
            let out = self.out.lock().await;
            let forwarded = self.active_turn.with_accepted_interaction(|| {
                if let Some(sink) = out.as_ref() {
                    let _ = sink.send(Frame::Event(event.clone()));
                }
            });
            drop(out);
            if !forwarded {
                let broker = self
                    .ask_user_question_broker
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .as_ref()
                    .and_then(Weak::upgrade);
                if let Some(broker) = broker {
                    // The broker publishes while its request is IN_FLIGHT.
                    // cancel() waits for publication to finish, so awaiting it
                    // inside emit() would wait on ourselves. Register owned
                    // cleanup before returning to the publisher; close joins it.
                    let request_id = request.request_id;
                    let mut rejections = self
                        .question_rejections
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    rejections.retain(|task| !task.is_finished());
                    rejections.push(tokio::spawn(async move {
                        broker.cancel(request_id).await;
                    }));
                }
            }
            return;
        }
        if matches!(event, ClientEvent::TurnEnded { .. }) {
            let Some(cancelled) = self.active_turn.begin_terminal() else {
                tracing::debug!("dropping duplicate or unowned turn terminal");
                return;
            };
            if cancelled {
                if let ClientEvent::TurnEnded { outcome, .. } = &mut event {
                    *outcome = client::protocol::events::TurnOutcomeDto::Cancelled;
                }
            }
        } else if is_owned_turn_event(&event) && !self.active_turn.accepts_turn_events() {
            tracing::debug!(event = ?std::mem::discriminant(&event), "dropping terminal or unowned turn event");
            return;
        }
        if let ClientEvent::TurnStarted { turn_id } = &mut event {
            if turn_id.is_none() {
                *turn_id = self.active_turn.turn_id();
            }
        }
        forward_event(&self.out, event).await;
    }
}

#[async_trait]
impl ClientEventSink for UnscopedFrameEventSink {
    async fn emit(&self, event: ClientEvent) {
        forward_event(&self.out, event).await;
    }
}

/// A [`PermissionRequestSink`] that forwards each [`PermissionRequest`] out as a
/// [`Frame::PermissionRequest`]. It also RECORDS the `request_id → tool_name`
/// mapping so the inbound `ApprovePermission`/`DenyPermission` (which carries
/// only the `request_id`) can supply the tool name back to
/// [`AdapterPermissionGate::resolve`] (needed for the `AllowAlways` session-rule
/// append).
struct FramePermissionSink {
    out: SharedFrameSink,
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
    active_turn: ActiveTurnControl,
    gate: SharedPermissionGate,
    handshaken: Arc<AtomicBool>,
}

#[async_trait]
impl PermissionRequestSink for FramePermissionSink {
    async fn emit_request(&self, request: PermissionRequest) {
        let request_id = request.request_id;
        let gate = self
            .gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        let Some(gate) = gate else {
            return;
        };
        let Some(background_owned) = gate.request_is_background(request_id).await else {
            return;
        };
        let owner_id = gate.request_owner_id(request_id).await;
        if !background_owned && !self.active_turn.accepts_permission_owner(owner_id) {
            self.reject(request_id).await;
            return;
        }
        // Record the tool name for the eventual resolve.
        let tool_name = match &request.kind {
            PermissionKindDto::ToolUseConfirm { tool_name, .. } => Some(tool_name.clone()),
            // Reserved kinds (ExitPlanMode / BypassPermissionsMode) are never
            // live-sourced in the foundation (decision §0.6); no tool name.
            _ => None,
        };
        if let Some(name) = tool_name {
            self.tool_names.lock().await.insert(request_id, name);
        }
        // Cancellation can race the gate's `pending.insert` immediately before
        // this sink is called. Linearize the final ownership check and the send
        // under the turn-owner lock so a request never appears after cancel won.
        let out = self.out.lock().await;
        let mut delivered = false;
        let still_pending = gate.request_is_background(request_id).await == Some(background_owned);
        let send = || {
            if !still_pending {
                return;
            }
            if !self.handshaken.load(Ordering::SeqCst) {
                return;
            }
            if let Some(sink) = out.as_ref() {
                delivered = sink.send(Frame::PermissionRequest(request));
            }
        };
        if background_owned {
            send();
        } else {
            self.active_turn
                .with_accepted_permission_owner(owner_id, send);
        }
        drop(out);
        if !delivered {
            self.tool_names.lock().await.remove(&request_id);
            self.reject(request_id).await;
        } else {
            lingxi_core::host::live_sessions::set_process_status(
                "waiting",
                Some(lingxi_core::host::live_sessions::PERMISSION_PROMPT_WAITING_FOR),
            );
        }
    }
}

impl FramePermissionSink {
    async fn reject(&self, request_id: u64) {
        let gate = self
            .gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(gate) = gate {
            gate.resolve(request_id, PermissionResponseDto::Deny, "")
                .await;
        }
    }
}

/// A [`ComputerAccessRequestSink`] that forwards each [`ComputerAccessRequestDto`]
/// out as a [`Frame::ComputerAccessRequest`]. Unlike [`FramePermissionSink`] it
/// needs no side-table: the DTO already carries everything `resolve`/`deny`
/// need, keyed by `request_id` on the [`ComputerAccessBroker`] itself.
struct FrameComputerAccessSink {
    out: SharedFrameSink,
    active_turn: ActiveTurnControl,
    broker: SharedComputerAccessBroker,
}

#[async_trait]
impl ComputerAccessRequestSink for FrameComputerAccessSink {
    async fn emit_request(&self, request: ComputerAccessRequestDto) {
        let request_id = request.request_id;
        if !self.active_turn.accepts_interactions() {
            self.reject(request_id).await;
            return;
        }
        // The broker parks this request before invoking the sink. Linearize the
        // final ownership check and send so cancellation either drains it or
        // follows a request that was already exposed, never the reverse.
        let out = self.out.lock().await;
        let forwarded = self.active_turn.with_accepted_interaction(|| {
            if let Some(sink) = out.as_ref() {
                let _ = sink.send(Frame::ComputerAccessRequest(request));
            }
        });
        drop(out);
        if !forwarded {
            self.reject(request_id).await;
        }
    }
}

impl FrameComputerAccessSink {
    async fn reject(&self, request_id: u64) {
        let broker = self
            .broker
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(broker) = broker {
            broker.deny(request_id).await;
        }
    }
}

/// An [`AudioRequestSink`] that forwards each [`ClientEvent::AudioRequest`] out
/// as a [`Frame::Event`], reporting whether a client was there to receive it.
///
/// Unlike [`FramePermissionSink`] and [`FrameComputerAccessSink`] it does NOT
/// gate on [`ActiveTurnControl::accepts_interactions`]. Those two ask the USER
/// to authorize something on behalf of a turn, so a request outliving its turn
/// is meaningless and is rejected. An audio op is not a prompt: it is a device
/// operation whose caller is awaiting a VALUE, and it is reachable outside any
/// turn (`is_recording` paints a button). Silently refusing it when no turn is
/// active would make `is_recording` answer `false` while the microphone is
/// open — a lie about the device, not a denied permission. Cancellation is the
/// caller's own concern; a dropped caller drops the receiver.
struct FrameAudioSink {
    out: SharedFrameSink,
}

#[async_trait]
impl AudioRequestSink for FrameAudioSink {
    async fn emit_request(&self, request: ClientEvent) -> bool {
        // `FrameSink::send` is false once the connection's write task has ended,
        // and the cell itself is `None` before a client attaches or after it
        // detaches — both are "nobody is listening", which the bridge turns into
        // an immediate failure instead of a parked request.
        match self.out.lock().await.as_ref() {
            Some(sink) => sink.send(Frame::Event(request)),
            None => false,
        }
    }
}

/// A bound connection: the [`bridge::FramePump`] the endpoint drives, holding the
/// connection-scoped permission gate, the outbound sink cell, the turn driver,
/// and the `request_id → tool_name` map.
///
/// Construct in two phases: [`BridgeConnection::new`] creates the outbound cell +
/// the sink handles ([`BridgeConnection::event_sink`] /
/// [`BridgeConnection::permission_sink`]) the caller wires into the orchestrator
/// it builds; then [`BridgeConnection::bind`] attaches the resulting gate +
/// [`TurnDriver`] and yields the pump.
pub struct BridgeConnection {
    out: SharedFrameSink,
    /// Serialize socket admission with teardown without holding the send lock
    /// across brokers whose cleanup emits an event back through that lock.
    connection_admission: Mutex<()>,
    pending_openai_oauth: Arc<Mutex<Option<ClientEvent>>>,
    cron_requests: Arc<crate::cron_host::HostCronRequests>,
    queued_prompt_payloads: QueuedPromptPayloads,
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
    gate: Option<Arc<AdapterPermissionGate>>,
    /// The `computer`-tool `request_access` broker (Electron-facing sibling of
    /// `gate`, see [`ComputerAccessBroker`]'s own doc comment). `None`
    /// when no computer-access channel was wired at boot (e.g. a test
    /// connection that never calls [`Self::bind_computer_access`]) — the two
    /// new [`ClientCommand`] variants are then silently dropped, exactly like
    /// an unrouted command with no [`CommandRouter`] bound.
    computer_access_broker: Option<Arc<ComputerAccessBroker>>,
    /// Connection-scoped broker for `AskUserQuestion` UI exchanges.
    ask_user_question_broker: Option<Arc<AskUserQuestionBroker>>,
    /// The response side of the connection's [`crate::audio_bridge::AudioBridge`]
    /// (the desktop service's response half). `None` when no audio
    /// bridge was wired at boot — `AudioResponse` is then silently dropped,
    /// exactly like an unrouted command with no [`CommandRouter`] bound.
    audio_responder: Option<AudioResponder>,
    permission_gate_ref: SharedPermissionGate,
    computer_access_broker_ref: SharedComputerAccessBroker,
    ask_user_question_broker_ref: SharedAskUserQuestionBroker,
    question_rejections: QuestionRejections,
    driver: Option<Arc<dyn TurnDriver>>,
    /// Client ids explicitly attached through this socket, serialized with
    /// attach/detach and close cleanup. A hello alone never populates this set.
    ui_attached_clients: Arc<Mutex<HashSet<String>>>,
    ui_lifecycle_gate: Arc<tokio::sync::Mutex<()>>,
    /// The full command-routing seam (F2-08). When bound, every
    /// non-turn/non-permission [`ClientCommand`] (model, listings, slash, tasks,
    /// session control) is delegated here, with replies pushed out through the
    /// connection's [`ClientEventSink`]. `None` in the F2-05/06/07 skeleton (only
    /// the turn + permission path was routed there).
    router: Option<Arc<dyn CommandRouter>>,
    /// Set once the opening `hello` handshake is ACCEPTED (compatible versions).
    handshaken: Arc<AtomicBool>,
    /// Set once a `hello` handshake is refused. A refused connection's
    /// subsequent commands are rejected and can never drive the engine.
    handshake_refused: Arc<AtomicBool>,
    /// Input message queue (spec §27). A `SendPrompt` that arrives while a turn
    /// is in flight is ENQUEUED here instead of spawning a second concurrent
    /// turn (which would race on the shared `session.history`); the single
    /// drain loop runs it as a follow-up turn once the in-flight turn ends.
    ///
    /// This is the parity twin of claude-code's module-level command queue. The
    /// guard is on `SendPrompt` ONLY — `ApprovePermission`/`DenyPermission` are
    /// never queued, so the inverted permission handshake that unblocks a parked
    /// tool `check()` keeps dispatching immediately (server design above).
    queue: Arc<MessageQueueManager>,
    /// Plugin-origin slash calls waiting for the queue consumer's result.
    mod_commands: PendingModCommands,
    mod_prompts: PendingModPrompts,
    /// Dynamic-loop state scoped to this connection/session.
    loop_runtime: Arc<tool_cron::LoopRuntime>,
    /// Whether the single turn-drain loop is currently running. Twin of
    /// print.ts run()'s `running` flag (L1866): the first `SendPrompt` that wins
    /// this flag OWNS the drain loop; concurrent prompts enqueue and the owner
    /// drains them before clearing the flag.
    turn_running: Arc<AtomicBool>,
    /// Serialize idle command admission with the drain loop's release/reclaim
    /// window. Never hold this mutex while driving a model turn.
    turn_handoff: Arc<tokio::sync::Mutex<()>>,
    /// True while a session transition owns `turn_handoff` and may await a
    /// plugin command queued by its own session.start hook.
    transition_active: Arc<AtomicBool>,
    /// Connection-owned active turn identity and cancellation token. Keeping
    /// this outside the spawned driver closes the race where Cancel arrives
    /// after SendPrompt but before the driver registers with msgqueue.
    active_turn: ActiveTurnControl,
    /// Abort handle for the currently-owned spawned turn-drain loop. On close we
    /// abort it so stale turn work cannot survive into the next reconnect and
    /// emit onto a newly-claimed outbound sink.
    active_turn_task: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>>,
    queue_wakeup_task: Option<tokio::task::AbortHandle>,
    task_notification_registry:
        Option<Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>>,
}

#[derive(Clone, Default)]
struct ActiveTurnControl {
    next_generation: Arc<AtomicU64>,
    owner: Arc<StdMutex<Option<ActiveTurnOwner>>>,
    permission_gate: SharedPermissionGate,
}

struct ActiveTurnOwner {
    generation: u64,
    turn_id: Option<u64>,
    cancel: CancellationToken,
    terminal: bool,
    permission_owner_id: Option<u64>,
}

#[derive(Clone)]
struct TurnInteractions {
    gate: Option<Arc<AdapterPermissionGate>>,
    computer_access_broker: Option<Arc<ComputerAccessBroker>>,
    ask_user_question_broker: Option<Arc<AskUserQuestionBroker>>,
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
}

impl Default for TurnInteractions {
    fn default() -> Self {
        Self {
            gate: None,
            computer_access_broker: None,
            ask_user_question_broker: None,
            tool_names: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TurnInteractions {
    async fn drain(&self, permission_owner_id: Option<u64>) {
        if let Some(gate) = self.gate.as_ref() {
            if let Some(owner_id) = permission_owner_id {
                gate.cancel_owner(owner_id).await;
            }
        }
        if let Some(broker) = self.computer_access_broker.as_ref() {
            broker.drain().await;
        }
        if let Some(broker) = self.ask_user_question_broker.as_ref() {
            broker.drain().await;
        }
        // Background permissions keep their tool names for later AllowAlways.
        let ids = self
            .tool_names
            .lock()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            let background = match &self.gate {
                Some(gate) => gate.request_is_background(id).await == Some(true),
                None => false,
            };
            if !background {
                self.tool_names.lock().await.remove(&id);
            }
        }
    }
}

/// Interaction cleanup and owner retirement are one handoff. A cancellation
/// can await publication/resolution callbacks while the driver finishes; its
/// owned interaction cleanup must complete before the next owner starts.
async fn finish_turn(
    active_turn: &ActiveTurnControl,
    generation: u64,
    interactions: &TurnInteractions,
    turn_handoff: &Arc<Mutex<()>>,
) {
    let _handoff = turn_handoff.lock().await;
    if active_turn.owns_generation(generation) {
        interactions
            .drain(active_turn.permission_owner_id(generation))
            .await;
        active_turn.finish(generation);
    }
}

async fn begin_owned_turn(
    active_turn: &ActiveTurnControl,
    driver: &Arc<dyn TurnDriver>,
    turn_id: Option<u64>,
) -> (u64, CancellationToken) {
    if let Some(gate) = active_turn.gate() {
        gate.set_session_id(driver.current_session_id().await);
    }
    active_turn.begin(turn_id)
}

impl ActiveTurnControl {
    fn gate(&self) -> Option<Arc<AdapterPermissionGate>> {
        self.permission_gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
    }

    fn begin(&self, turn_id: Option<u64>) -> (u64, CancellationToken) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = CancellationToken::new();
        *self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(ActiveTurnOwner {
            generation,
            turn_id,
            cancel: cancel.clone(),
            terminal: false,
            permission_owner_id: self.gate().map(|gate| gate.begin_main_turn(None, turn_id)),
        });
        (generation, cancel)
    }

    fn finish(&self, generation: u64) {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if owner
            .as_ref()
            .is_some_and(|active| active.generation == generation)
        {
            let permission_owner_id = owner.as_ref().and_then(|owner| owner.permission_owner_id);
            *owner = None;
            if let (Some(gate), Some(id)) = (self.gate(), permission_owner_id) {
                gate.end_main_turn(id);
            }
        }
    }

    fn permission_owner_id(&self, generation: u64) -> Option<u64> {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .filter(|owner| owner.generation == generation)
            .and_then(|owner| owner.permission_owner_id)
    }

    fn accepts_permission_owner(&self, permission_owner_id: Option<u64>) -> bool {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|owner| {
                !owner.terminal
                    && !owner.cancel.is_cancelled()
                    && permission_owner_id.is_some()
                    && owner.permission_owner_id == permission_owner_id
            })
    }

    fn with_accepted_permission_owner(
        &self,
        permission_owner_id: Option<u64>,
        send: impl FnOnce(),
    ) {
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if owner.as_ref().is_some_and(|owner| {
            !owner.terminal
                && !owner.cancel.is_cancelled()
                && permission_owner_id.is_some()
                && owner.permission_owner_id == permission_owner_id
        }) {
            send();
        }
    }

    fn owns_generation(&self, generation: u64) -> bool {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|owner| owner.generation == generation)
    }

    fn cancellation_target(&self, turn_id: Option<u64>) -> Option<(u64, CancellationToken)> {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .filter(|active| !active.terminal && (turn_id.is_none() || active.turn_id == turn_id))
            .map(|active| (active.generation, active.cancel.clone()))
    }

    fn cancel(&self, turn_id: Option<u64>) -> bool {
        let token = {
            let owner = self
                .owner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            owner.as_ref().and_then(|active| {
                (!active.terminal && (turn_id.is_none() || active.turn_id == turn_id))
                    .then(|| active.cancel.clone())
            })
        };
        if let Some(token) = token {
            token.cancel();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_some()
    }

    fn accepts_turn_events(&self) -> bool {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|owner| !owner.terminal)
    }

    /// Atomically closes the event stream while retaining the active slot until
    /// the driver future naturally returns. Returns whether cancellation had
    /// already been requested so the terminal outcome can be normalized.
    fn begin_terminal(&self) -> Option<bool> {
        let mut owner = self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let owner = owner.as_mut()?;
        if owner.terminal {
            return None;
        }
        owner.terminal = true;
        Some(owner.cancel.is_cancelled())
    }

    fn accepts_interactions(&self) -> bool {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|owner| !owner.terminal && !owner.cancel.is_cancelled())
    }

    /// Run `action` while holding the turn-owner lock only when interactive
    /// requests are still accepted. This is the linearization point shared by
    /// prompt emission and cancellation: once `cancel` or `begin_terminal`
    /// wins the lock, no later permission/question request can reach a client.
    fn with_accepted_interaction(&self, action: impl FnOnce()) -> bool {
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if owner
            .as_ref()
            .is_some_and(|owner| !owner.terminal && !owner.cancel.is_cancelled())
        {
            action();
            true
        } else {
            false
        }
    }

    fn turn_id(&self) -> Option<u64> {
        self.owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(|owner| owner.turn_id)
    }

    fn clear(&self) {
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(owner) = owner {
            owner.cancel.cancel();
            if let (Some(gate), Some(id)) = (self.gate(), owner.permission_owner_id) {
                gate.end_main_turn(id);
            }
        }
    }
}

impl Default for BridgeConnection {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a `Next`-priority main-thread [`QueuedCommand`] for a user prompt that
/// arrived while a turn was in flight (twin of claude-code's `enqueue` defaulting
/// a direct prompt to `'next'`). `agent_id` is `None` (main thread) so the
/// between-turn drain picks it up.
fn prompt_command(text: String) -> QueuedCommand {
    // Process-unique monotonic counter — the drain loop removes consumed
    // commands by uuid, so each queued prompt needs a distinct id.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    QueuedCommand {
        scheduled_task_id: None,
        scheduled_fire_id: None,
        uuid: format!("prompt-{}", SEQ.fetch_add(1, Ordering::Relaxed)),
        content: QueuedCommandContent::UserInput { text },
        priority: QueuePriority::Next,
        queued_at: std::time::SystemTime::now(),
        source: QueueSource::PromptInput,
        agent_id: None,
        skip_slash_commands: false,
        is_meta: false,
    }
}

struct PendingModCommand {
    plugin: String,
    settle: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
}

type PendingModCommands = Arc<StdMutex<HashMap<String, PendingModCommand>>>;

struct PendingModPrompt {
    plugin: String,
    as_user: bool,
    settle: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
}

type PendingModPrompts = Arc<StdMutex<HashMap<String, PendingModPrompt>>>;

struct BridgeModCommandQueue {
    queue: Arc<MessageQueueManager>,
    pending: PendingModCommands,
    prompts: PendingModPrompts,
}

#[async_trait]
impl command_api::ModCommandQueue for BridgeModCommandQueue {
    async fn enqueue(
        &self,
        plugin: &str,
        command: &str,
        args: &str,
    ) -> Result<serde_json::Value, String> {
        let text = if args.is_empty() {
            format!("/{command}")
        } else {
            format!("/{command} {args}")
        };
        let mut queued = prompt_command(text);
        queued.uuid = format!("mod-command-{}", uuid::Uuid::new_v4());
        queued.source = QueueSource::Plugin;
        queued.priority = QueuePriority::Later;
        queued.is_meta = true;
        let (settle, receive) = tokio::sync::oneshot::channel();
        self.pending.lock().unwrap().insert(
            queued.uuid.clone(),
            PendingModCommand {
                plugin: plugin.to_owned(),
                settle,
            },
        );
        self.queue.enqueue(queued).await;
        receive
            .await
            .map_err(|_| "the command did not run".to_owned())?
    }

    async fn enqueue_prompt(
        &self,
        plugin: &str,
        text: &str,
        as_user: bool,
    ) -> Result<serde_json::Value, String> {
        let mut queued = prompt_command(text.to_owned());
        queued.uuid = format!("mod-prompt-{}", uuid::Uuid::new_v4());
        queued.source = QueueSource::Plugin;
        queued.priority = QueuePriority::Later;
        queued.is_meta = !as_user;
        let (settle, receive) = tokio::sync::oneshot::channel();
        self.prompts.lock().unwrap().insert(
            queued.uuid.clone(),
            PendingModPrompt {
                plugin: plugin.to_owned(),
                as_user,
                settle,
            },
        );
        self.queue.enqueue(queued).await;
        receive
            .await
            .map_err(|_| "the prompt did not run".to_owned())?
    }
}

#[derive(Clone)]
struct ModQueueDrain {
    pending: PendingModCommands,
    prompts: PendingModPrompts,
    router: Option<Arc<dyn CommandRouter>>,
    events: Arc<dyn ClientEventSink>,
}

struct QueuedModTurn {
    prompt: String,
    as_user: bool,
    origin: serde_json::Value,
    generation: u64,
    cancel: CancellationToken,
}

struct TransitionScope(Arc<AtomicBool>);

impl TransitionScope {
    fn begin(flag: &Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag.clone())
    }
}

impl Drop for TransitionScope {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Snapshot a text-only batch under the same handoff that publishes complete
/// queued prompts and cancels their identities. Taking metadata and queue
/// snapshots separately lets a newly published attachment slip into a text batch.
async fn batchable_prompt_snapshot(
    queue: &Arc<MessageQueueManager>,
    queued_prompt_payloads: &QueuedPromptPayloads,
    turn_handoff: &Arc<tokio::sync::Mutex<()>>,
) -> Vec<QueuedCommand> {
    let _handoff = turn_handoff.lock().await;
    let payload_ids = queued_prompt_payloads
        .lock()
        .await
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if queue
        .peek(|c| c.is_main_thread())
        .await
        .is_some_and(|head| {
            head.source != QueueSource::Plugin
                && !head.is_slash_command()
                && !payload_ids.contains(&head.uuid)
        })
    {
        queue
            .snapshot()
            .await
            .into_iter()
            .filter(|c| {
                c.is_main_thread()
                    && c.source != QueueSource::Plugin
                    && !c.is_slash_command()
                    && !payload_ids.contains(&c.uuid)
            })
            .collect()
    } else {
        Vec::new()
    }
}

async fn run_queued_mod_command(
    command: &QueuedCommand,
    mod_drain: &ModQueueDrain,
    driver: &Arc<dyn TurnDriver>,
    active_turn: &ActiveTurnControl,
    turn_handoff: &Arc<tokio::sync::Mutex<()>>,
    transition_owns_handoff: bool,
) -> Option<QueuedModTurn> {
    let queued_prompt = mod_drain.prompts.lock().unwrap().remove(&command.uuid);
    if let Some(pending) = queued_prompt {
        let (generation, cancel) = if transition_owns_handoff {
            begin_owned_turn(active_turn, driver, None).await
        } else {
            let _handoff = turn_handoff.lock().await;
            begin_owned_turn(active_turn, driver, None).await
        };
        let prompt = command.text().unwrap_or_default().to_owned();
        let _ = pending.settle.send(Ok(serde_json::json!({"text":prompt})));
        return Some(QueuedModTurn {
            prompt,
            as_user: pending.as_user,
            origin: if pending.as_user {
                serde_json::json!({"kind":"plugin","name":pending.plugin,"asUser":true})
            } else {
                serde_json::json!({"kind":"plugin","name":pending.plugin})
            },
            generation,
            cancel,
        });
    }
    let pending = mod_drain.pending.lock().unwrap().remove(&command.uuid);
    let Some(PendingModCommand { plugin, settle }) = pending else {
        return None;
    };
    let Some(router) = mod_drain.router.as_ref() else {
        let _ = settle.send(Err("Mod command router is unavailable".into()));
        return None;
    };
    let context = command_api::ModCommandRunContext {
        origin: serde_json::json!({"kind":"plugin","name":plugin}),
        is_fullscreen: false,
        columns: 80,
    };
    let (outcome, settlement) = command_api::with_mod_command_capture(
        context,
        router.dispatch_slash(command.text().unwrap_or_default()),
    )
    .await;
    let Some(outcome) = outcome else {
        let _ = settle.send(Err("Mod command dispatcher is unavailable".into()));
        return None;
    };
    let mut queued_turn = None;
    let result = match outcome.result {
        lingxi_core::host::SlashDispatchResult::Handled { display } => {
            mod_drain
                .events
                .emit(ClientEvent::SlashCommandResult {
                    turn_id: None,
                    display: display.clone(),
                    is_error: false,
                })
                .await;
            Ok(settlement.unwrap_or_else(|| serde_json::json!({"text":display})))
        }
        lingxi_core::host::SlashDispatchResult::RunAsTurn { prompt } => {
            let (generation, cancel) = if transition_owns_handoff {
                begin_owned_turn(active_turn, driver, None).await
            } else {
                let _handoff = turn_handoff.lock().await;
                begin_owned_turn(active_turn, driver, None).await
            };
            queued_turn = Some(QueuedModTurn {
                prompt,
                as_user: false,
                origin: serde_json::json!({"kind":"plugin","name":plugin}),
                generation,
                cancel,
            });
            Ok(serde_json::json!({}))
        }
        lingxi_core::host::SlashDispatchResult::Unknown { display, .. } => {
            mod_drain
                .events
                .emit(ClientEvent::SlashCommandResult {
                    turn_id: None,
                    display: display.clone(),
                    is_error: true,
                })
                .await;
            Err(display)
        }
        lingxi_core::host::SlashDispatchResult::NotASlashCommand => {
            Err(format!("{plugin}: queued command was not a slash command"))
        }
    };
    for event in outcome.authority_events {
        mod_drain.events.emit(event).await;
    }
    let _ = settle.send(result);
    queued_turn
}

/// Drain every queued MAIN-THREAD prompt as a follow-up turn, coalescing
/// consecutive prompts into one turn (twin of `joinPromptValues` /
/// `drainCommandQueue`). Runs until no main-thread command remains.
async fn drain_main_thread(
    driver: &Arc<dyn TurnDriver>,
    queue: &Arc<MessageQueueManager>,
    loop_runtime: &Arc<tool_cron::LoopRuntime>,
    active_turn: &ActiveTurnControl,
    interactions: &TurnInteractions,
    queued_prompt_payloads: &QueuedPromptPayloads,
    turn_handoff: &Arc<tokio::sync::Mutex<()>>,
    mod_drain: Option<&ModQueueDrain>,
) {
    loop {
        // Native hJe selects the head by priority, then Rr collects compatible
        // messages in insertion order. Each retains its own origin in the batch.
        let batch = batchable_prompt_snapshot(queue, queued_prompt_payloads, turn_handoff).await;
        let Some((joined, mut consumed)) = join_prompt_values(&batch) else {
            // No batchable (non-slash) prompt left. Pop the next main-thread
            // command and, if it carries prompt text (e.g. a slash command typed
            // mid-turn), run it as its own follow-up turn — IDENTICAL to how the
            // idle SendPrompt path handles the same input — so it is never
            // silently dropped. Text-less commands (bare notifications) are just
            // consumed. (claude-code keeps queued slash commands and routes them
            // post-turn; running it here is the bridge's faithful equivalent
            // since the idle path also runs slash text through run_turn.)
            // Claim metadata-bearing input and its cancellation owner under
            // the same handoff used by queued cancellation. There is no gap
            // where an ID belongs to neither the queue nor an active turn.
            let (command, payload, owned_turn) = {
                let _handoff = turn_handoff.lock().await;
                let command = queue.dequeue_main_thread().await;
                let payload = match command.as_ref() {
                    Some(command) => queued_prompt_payloads.lock().await.remove(&command.uuid),
                    None => None,
                };
                let owned_turn = if command.as_ref().is_some_and(|command| {
                    command.source != QueueSource::Plugin
                        && (payload.is_some()
                            || command.text().is_some_and(|text| !text.is_empty()))
                }) {
                    Some(
                        begin_owned_turn(
                            active_turn,
                            driver,
                            payload.as_ref().and_then(|payload| payload.turn_id),
                        )
                        .await,
                    )
                } else {
                    None
                };
                (command, payload, owned_turn)
            };
            match command {
                Some(cmd) => {
                    if cmd.source == QueueSource::Plugin {
                        if let Some(mod_drain) = mod_drain {
                            if let Some(turn) = run_queued_mod_command(
                                &cmd,
                                mod_drain,
                                driver,
                                active_turn,
                                turn_handoff,
                                false,
                            )
                            .await
                            {
                                orchestrator::mod_prompt_origin::with_origin(
                                    turn.origin,
                                    driver.run_queued_turn(turn.prompt, turn.as_user, turn.cancel),
                                )
                                .await;
                                finish_turn(
                                    active_turn,
                                    turn.generation,
                                    interactions,
                                    turn_handoff,
                                )
                                .await;
                            }
                        }
                        continue;
                    }
                    if let Some(payload) = payload {
                        let (generation, cancel) = owned_turn.expect("complete prompt owns a turn");
                        tag_loop_tick_in_flight(
                            loop_runtime,
                            false,
                            cmd.text().unwrap_or_default(),
                        );
                        orchestrator::mod_prompt_origin::with_origin(
                            serde_json::json!({"kind":"bridge"}),
                            driver.run_queued_turn_with_images(
                                cmd.text().unwrap_or_default().to_string(),
                                payload.images,
                                cancel,
                            ),
                        )
                        .await;
                        finish_turn(active_turn, generation, interactions, turn_handoff).await;
                        continue;
                    }
                    if let Some(t) = cmd.text() {
                        if !t.is_empty() {
                            tag_loop_tick_in_flight(
                                loop_runtime,
                                cmd.source == QueueSource::Cron
                                    && cmd.uuid.starts_with("loop-wakeup-"),
                                t,
                            );
                            let (generation, cancel) =
                                owned_turn.expect("nonempty queued prompt owns a turn");
                            let origin = match cmd.source {
                                QueueSource::PromptInput => serde_json::json!({"kind":"bridge"}),
                                QueueSource::Cron => {
                                    serde_json::json!({"kind":"scheduled-trigger"})
                                }
                                QueueSource::AgentSendMessage => serde_json::json!({"kind":"peer"}),
                                _ => serde_json::json!({"kind":"unclassified"}),
                            };
                            orchestrator::mod_prompt_origin::with_origin(
                                origin,
                                driver.run_queued_turn(
                                    t.to_string(),
                                    cmd.source == QueueSource::PromptInput && !cmd.is_meta,
                                    cancel,
                                ),
                            )
                            .await;
                            finish_turn(active_turn, generation, interactions, turn_handoff).await;
                        }
                    }
                    continue;
                }
                None => break,
            }
        };
        // KEEPALIVE (binary `onFireTask` `if(d.kind==="loop")I7e(d.prompt)`): tag on
        // the command SOURCE, not the slash-vs-non-slash branch — a dynamic loop
        // tick whose sentinel resolved to non-slash instruction text lands in THIS
        // batched branch, so the in-flight tag must be set here too. Use the first
        // consumed `QueueSource::Cron` command's text as the in-flight prompt.
        let cron_tick = batch
            .iter()
            .find(|c| {
                c.source == QueueSource::Cron
                    && c.uuid.starts_with("loop-wakeup-")
                    && consumed.contains(&c.uuid)
            })
            .and_then(|c| c.text().map(str::to_string));
        match cron_tick {
            Some(ref t) => tag_loop_tick_in_flight(loop_runtime, true, t),
            None => tag_loop_tick_in_flight(loop_runtime, false, &joined),
        }
        // Resolve only consumed prompts: touching a later sentinel early would
        // incorrectly mark its preamble/loop.md as already delivered.
        // A batch mixes origins (a `/loop` wakeup and a typed prompt can be
        // drained together), so a `loop.md` read failure must retire ONLY the
        // sentinel that could not be resolved. Consuming the whole batch here
        // silently destroyed the user's typed message.
        let mut inputs = Vec::new();
        let mut loop_failure = None;
        let mut failed_uuids = Vec::new();
        for command in batch
            .iter()
            .filter(|command| consumed.contains(&command.uuid))
        {
            let Some(text) = command.text() else { continue };
            let text = if command.source == QueueSource::Cron {
                match driver.resolve_loop_prompt(text) {
                    Ok(text) => text,
                    Err(error) => {
                        failed_uuids.push(command.uuid.clone());
                        loop_failure.get_or_insert(error);
                        continue;
                    }
                }
            } else {
                text.to_string()
            };
            inputs.push(orchestrator::QueuedPromptInput {
                goal_retry_id: command
                    .uuid
                    .starts_with("goal-retry-")
                    .then(|| command.uuid.clone()),
                text,
                is_meta: command.is_meta || command.source != QueueSource::PromptInput,
                mod_origin: Some(match command.source {
                    QueueSource::PromptInput => serde_json::json!({"kind":"bridge"}),
                    QueueSource::Cron => serde_json::json!({"kind":"scheduled-trigger"}),
                    QueueSource::AgentSendMessage => serde_json::json!({"kind":"peer"}),
                    _ => serde_json::json!({"kind":"unclassified"}),
                }),
                message_id: None,
                transcript_row_token: None,
                queue_priority: (command.priority == QueuePriority::Later)
                    .then(|| "later".to_string()),
                scheduled_task_id: command.scheduled_task_id.clone(),
                scheduled_fire_id: command.scheduled_fire_id.clone(),
            });
        }
        if let Some(error) = loop_failure {
            queue
                .consume(&failed_uuids, "loop instruction read failed")
                .await;
            consumed.retain(|uuid| !failed_uuids.contains(uuid));
            loop_runtime.take_in_flight_prompt();
            driver.loop_prompt_failed(error).await;
            if inputs.is_empty() {
                continue;
            }
        }
        let (generation, cancel) = {
            let _handoff = turn_handoff.lock().await;
            queue
                .consume(&consumed, "drained into follow-up turn")
                .await;
            begin_owned_turn(active_turn, driver, None).await
        };
        let in_human_turn = batch.iter().any(|cmd| {
            consumed.contains(&cmd.uuid) && cmd.source == QueueSource::PromptInput && !cmd.is_meta
        });
        if cron_tick.is_some() && in_human_turn {
            loop_runtime.veto_tick(tool_cron::LoopFoldVeto::ForeignUserInput);
        }
        driver.run_queued_batch(inputs, cancel).await;
        finish_turn(active_turn, generation, interactions, turn_handoff).await;
    }
}

/// Record (or clear) the in-flight `/loop` tick so the driver's turn-completion
/// edge can arm the keepalive fallback. A `QueueSource::Cron` command IS a loop
/// tick (binary `d.kind==="loop"`); any other turn clears a stale tag so a user
/// turn never inherits one.
fn tag_loop_tick_in_flight(loop_runtime: &tool_cron::LoopRuntime, is_cron: bool, text: &str) {
    if is_cron {
        loop_runtime.begin_tick(text.to_string());
    } else {
        loop_runtime.take_in_flight_prompt();
        loop_runtime.invalidate_noop_streak();
    }
}

impl Drop for BridgeConnection {
    fn drop(&mut self) {
        if let Some(handle) = &self.queue_wakeup_task {
            handle.abort();
        }
        self.abort_active_turn_task();
        for task in self
            .question_rejections
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .drain(..)
        {
            task.abort();
        }
        self.active_turn.clear();
    }
}

impl BridgeConnection {
    /// Create an UNBOUND connection: the outbound sink cell and the tool-name map
    /// exist, but no gate / driver is attached yet. Call
    /// [`Self::event_sink`]/[`Self::permission_sink`] to obtain the sinks to wire
    /// into the orchestrator, then [`Self::bind`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Arc::new(Mutex::new(None)),
            connection_admission: Mutex::new(()),
            pending_openai_oauth: Arc::new(Mutex::new(None)),
            cron_requests: Arc::new(crate::cron_host::HostCronRequests::default()),
            queued_prompt_payloads: Arc::new(Mutex::new(HashMap::new())),
            tool_names: Arc::new(Mutex::new(HashMap::new())),
            gate: None,
            computer_access_broker: None,
            ask_user_question_broker: None,
            audio_responder: None,
            permission_gate_ref: Arc::new(StdMutex::new(None)),
            computer_access_broker_ref: Arc::new(StdMutex::new(None)),
            ask_user_question_broker_ref: Arc::new(StdMutex::new(None)),
            question_rejections: Arc::new(StdMutex::new(Vec::new())),
            driver: None,
            ui_attached_clients: Arc::new(Mutex::new(HashSet::new())),
            ui_lifecycle_gate: Arc::new(tokio::sync::Mutex::new(())),
            router: None,
            handshaken: Arc::new(AtomicBool::new(false)),
            handshake_refused: Arc::new(AtomicBool::new(false)),
            // Install the telemetry recorder synchronously at construction so
            // every queue mutation (enqueue/dequeue/remove/clear) is forwarded
            // to the tracing observability sink — the Rust twin of claude-code
            // wiring `recordQueueOperation` (messageQueueManager.ts). Per-
            // connection isolation is preserved: each connection owns its own
            // queue + recorder.
            queue: Arc::new(MessageQueueManager::with_recorder(Arc::new(
                TelemetryQueueRecorder::new(),
            ))),
            mod_commands: Arc::new(StdMutex::new(HashMap::new())),
            mod_prompts: Arc::new(StdMutex::new(HashMap::new())),
            loop_runtime: Arc::new(tool_cron::LoopRuntime::default()),
            turn_running: Arc::new(AtomicBool::new(false)),
            turn_handoff: Arc::new(tokio::sync::Mutex::new(())),
            transition_active: Arc::new(AtomicBool::new(false)),
            active_turn: ActiveTurnControl::default(),
            active_turn_task: Arc::new(StdMutex::new(None)),
            queue_wakeup_task: None,
            task_notification_registry: None,
        }
    }

    /// Wake an idle main loop when teammates enqueue a noninterrupting prompt.
    pub fn with_task_notification_registry(
        mut self,
        registry: Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
    ) -> Self {
        self.task_notification_registry = Some(registry);
        self
    }

    pub fn with_queue_wakeup(mut self) -> Self {
        let Some(driver) = self.driver.clone() else {
            return self;
        };
        let mod_drain = self.mod_queue_drain();
        let queue = self.queue.clone();
        let running = self.turn_running.clone();
        let turn_handoff = self.turn_handoff.clone();
        let transition_active = self.transition_active.clone();
        let handshaken = self.handshaken.clone();
        let active_turn = self.active_turn.clone();
        let active_task = self.active_turn_task.clone();
        let queued_prompt_payloads = self.queued_prompt_payloads.clone();
        let loop_runtime = self.loop_runtime.clone();
        let interactions = TurnInteractions {
            gate: self.gate.clone(),
            computer_access_broker: self.computer_access_broker.clone(),
            ask_user_question_broker: self.ask_user_question_broker.clone(),
            tool_names: self.tool_names.clone(),
        };
        let registry = self.task_notification_registry.clone();
        let watcher = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                // A transition may be waiting inside session.start for its
                // own queued plugin command. The parent owns `turn_handoff`,
                // so consume only that plugin item through its reentrant lane.
                if transition_active.load(Ordering::SeqCst)
                    && handshaken.load(Ordering::SeqCst)
                    && queue
                        .peek(|command| {
                            command.is_main_thread() && command.source == QueueSource::Plugin
                        })
                        .await
                        .is_some()
                    && running
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    if let Some(command) = queue
                        .dequeue_filtered(|command| {
                            command.is_main_thread() && command.source == QueueSource::Plugin
                        })
                        .await
                    {
                        let turn = run_queued_mod_command(
                            &command,
                            &mod_drain,
                            &driver,
                            &active_turn,
                            &turn_handoff,
                            true,
                        )
                        .await;
                        if let Some(turn) = turn {
                            // The plugin's command has run and its API promise
                            // is settled. Drive the resulting model prompt on
                            // a separate owner so this socket can still read
                            // permission replies while the turn is in flight.
                            let driver = driver.clone();
                            let queue = queue.clone();
                            let running = running.clone();
                            let loop_runtime = loop_runtime.clone();
                            let active_turn = active_turn.clone();
                            let interactions = interactions.clone();
                            let queued_prompt_payloads = queued_prompt_payloads.clone();
                            let turn_handoff = turn_handoff.clone();
                            let mod_drain = mod_drain.clone();
                            let task = tokio::spawn(async move {
                                orchestrator::mod_prompt_origin::with_origin(
                                    turn.origin,
                                    driver.run_queued_turn(turn.prompt, turn.as_user, turn.cancel),
                                )
                                .await;
                                finish_turn(
                                    &active_turn,
                                    turn.generation,
                                    &interactions,
                                    &turn_handoff,
                                )
                                .await;
                                loop {
                                    drain_main_thread(
                                        &driver,
                                        &queue,
                                        &loop_runtime,
                                        &active_turn,
                                        &interactions,
                                        &queued_prompt_payloads,
                                        &turn_handoff,
                                        Some(&mod_drain),
                                    )
                                    .await;
                                    let _handoff = turn_handoff.lock().await;
                                    running.store(false, Ordering::SeqCst);
                                    if queue.has_main_thread_commands().await
                                        && running
                                            .compare_exchange(
                                                false,
                                                true,
                                                Ordering::SeqCst,
                                                Ordering::SeqCst,
                                            )
                                            .is_ok()
                                    {
                                        continue;
                                    }
                                    break;
                                }
                            });
                            *active_task
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner()) = Some(task);
                            continue;
                        }
                    }
                    running.store(false, Ordering::SeqCst);
                    continue;
                }
                if handshaken.load(Ordering::SeqCst)
                    && running.load(Ordering::SeqCst)
                    && !queue
                        .get_by_max_priority(QueuePriority::Next, |command| {
                            command.is_main_thread()
                        })
                        .await
                        .is_empty()
                    && !lingxi_core::host::env::background_tasks_disabled()
                {
                    if let Some(registry) = &registry {
                        registry
                            .background_all_tasks_with_reason(
                                lingxi_core::host::task_registry::TaskBackgroundReason::DeliverMessage,
                            )
                            .await;
                    }
                    continue;
                }
                // Serialize the idle claim with clear/new/resume, without
                // marking a transition as an active model turn.
                let Ok(_handoff) = turn_handoff.try_lock() else {
                    continue;
                };
                let pending_notifications = match &registry {
                    Some(registry) => registry.has_pending_task_notifications_for(None).await,
                    None => false,
                };
                if !handshaken.load(Ordering::SeqCst)
                    || (!queue.has_main_thread_commands().await && !pending_notifications)
                    || running
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                {
                    continue;
                }
                let mut task_slot = active_task
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if !handshaken.load(Ordering::SeqCst) {
                    running.store(false, Ordering::SeqCst);
                    continue;
                }
                let (
                    driver,
                    queue,
                    running,
                    active_turn,
                    loop_runtime,
                    interactions,
                    queued_prompt_payloads,
                    turn_handoff,
                ) = (
                    driver.clone(),
                    queue.clone(),
                    running.clone(),
                    active_turn.clone(),
                    loop_runtime.clone(),
                    interactions.clone(),
                    queued_prompt_payloads.clone(),
                    turn_handoff.clone(),
                );
                let registry = registry.clone();
                let mod_drain = mod_drain.clone();
                let task = tokio::spawn(async move {
                    if let Some(registry) = registry {
                        if registry.has_pending_task_notifications_for(None).await {
                            let (generation, cancel) = {
                                let _handoff = turn_handoff.lock().await;
                                begin_owned_turn(&active_turn, &driver, None).await
                            };
                            driver.run_task_notification_turn(registry, cancel).await;
                            finish_turn(&active_turn, generation, &interactions, &turn_handoff)
                                .await;
                        }
                    }
                    drain_main_thread(
                        &driver,
                        &queue,
                        &loop_runtime,
                        &active_turn,
                        &interactions,
                        &queued_prompt_payloads,
                        &turn_handoff,
                        Some(&mod_drain),
                    )
                    .await;
                    running.store(false, Ordering::SeqCst);
                });
                *task_slot = Some(task);
            }
        });
        self.queue_wakeup_task = Some(watcher.abort_handle());
        self
    }

    /// The connection-scoped [`ClientEventSink`] to bind into the orchestrator's
    /// [`client::adapter::AdapterOutputStream`]. Every streamed turn event flows
    /// through here as a [`Frame::Event`].
    #[must_use]
    pub fn event_sink(&self) -> Arc<dyn ClientEventSink> {
        Arc::new(FrameEventSink {
            out: self.out.clone(),
            pending_openai_oauth: self.pending_openai_oauth.clone(),
            cron_requests: self.cron_requests.clone(),
            handshaken: self.handshaken.clone(),
            active_turn: self.active_turn.clone(),
            ask_user_question_broker: self.ask_user_question_broker_ref.clone(),
            question_rejections: self.question_rejections.clone(),
        })
    }

    /// Sink for a `/loop` wakeup's own announcement.
    ///
    /// A wakeup fires BETWEEN turns by construction — it is what starts the
    /// next turn — so no turn owns it and [`Self::event_sink`] would drop the
    /// announcement on the floor (`is_owned_turn_event` covers `SystemNotice`).
    /// Narrow on purpose: the general unscoped sink stays private so
    /// orchestrator events cannot bypass turn ownership.
    #[must_use]
    pub fn loop_wakeup_event_sink(&self) -> Arc<dyn ClientEventSink> {
        self.unscoped_event_sink()
    }

    /// Sink reserved for command output that is not part of a turn lifecycle.
    /// Keeping it private prevents orchestrator events from bypassing ownership.
    fn unscoped_event_sink(&self) -> Arc<dyn ClientEventSink> {
        Arc::new(UnscopedFrameEventSink {
            out: self.out.clone(),
        })
    }

    /// The connection-scoped [`PermissionRequestSink`] to bind into the
    /// [`AdapterPermissionGate`]. Every `check()` request flows through here as a
    /// [`Frame::PermissionRequest`].
    #[must_use]
    pub fn permission_sink(&self) -> Arc<dyn PermissionRequestSink> {
        Arc::new(FramePermissionSink {
            out: self.out.clone(),
            tool_names: self.tool_names.clone(),
            active_turn: self.active_turn.clone(),
            gate: self.permission_gate_ref.clone(),
            handshaken: self.handshaken.clone(),
        })
    }

    /// The connection-scoped [`ComputerAccessRequestSink`] to bind into a
    /// [`ComputerAccessBroker`]. Every `request_access` exchange the
    /// broker drains flows through here as a [`Frame::ComputerAccessRequest`].
    #[must_use]
    pub fn computer_access_sink(&self) -> Arc<dyn ComputerAccessRequestSink> {
        Arc::new(FrameComputerAccessSink {
            out: self.out.clone(),
            active_turn: self.active_turn.clone(),
            broker: self.computer_access_broker_ref.clone(),
        })
    }

    /// Attach the connection's permission gate (whose `resolve` the read task
    /// calls on an inbound approval) and the [`TurnDriver`] that drives
    /// `SendPrompt`. Yields the fully-bound pump.
    #[must_use]
    pub fn bind(mut self, gate: Arc<AdapterPermissionGate>, driver: Arc<dyn TurnDriver>) -> Self {
        *self
            .permission_gate_ref
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::downgrade(&gate));
        *self
            .active_turn
            .permission_gate
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::downgrade(&gate));
        self.gate = Some(gate);
        self.driver = Some(driver);
        self
    }

    /// The connection-scoped [`AudioRequestSink`] to build an
    /// [`crate::audio_bridge::AudioBridge`] over. Every audio operation the
    /// engine makes flows through here as a
    /// [`Frame::Event`]`(`[`ClientEvent::AudioRequest`]`)`.
    #[must_use]
    pub fn audio_sink(&self) -> Arc<dyn AudioRequestSink> {
        Arc::new(FrameAudioSink {
            out: self.out.clone(),
        })
    }

    /// Attach the response side of the connection's audio bridge, so an inbound
    /// `AudioResponse` resolves the parked operation and a disconnect drains
    /// every request still parked. Additive over [`Self::bind`]: a connection
    /// built without this call (e.g. most existing tests) simply never receives
    /// an audio bridge, and `AudioResponse` is a no-op.
    ///
    /// Unlike [`Self::bind_computer_access`] there is no receive loop to spawn:
    /// audio operations ARE the request source, so the bridge emits directly
    /// through [`Self::audio_sink`] rather than draining a channel.
    #[must_use]
    pub fn bind_audio(mut self, responder: AudioResponder) -> Self {
        self.audio_responder = Some(responder);
        self
    }

    /// Attach the connection's `computer`-tool `request_access` broker and
    /// SPAWN its receive loop over `rx` (the receiving end of the SAME channel
    /// whose sender was wired into `DesktopConfig::computer_access_tx`, which
    /// drives the generic `tool_computer_use::TuiBridgeResolver` on the engine
    /// side). Additive over [`Self::bind`]: a connection built without this
    /// call (e.g. most existing tests) simply never receives a computer-access
    /// channel, and `ApproveComputerAccess`/`DenyComputerAccess` are no-ops.
    #[must_use]
    pub fn bind_computer_access(
        mut self,
        broker: Arc<ComputerAccessBroker>,
        rx: tokio::sync::mpsc::Receiver<ComputerAccessExchange>,
    ) -> Self {
        *self
            .computer_access_broker_ref
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::downgrade(&broker));
        let run_loop = broker.clone();
        tokio::spawn(async move { run_loop.run(rx).await });
        self.computer_access_broker = Some(broker);
        self
    }

    /// Attach the connection's interactive `AskUserQuestion` broker and start
    /// draining the same channel whose sender is installed in `DesktopConfig`.
    #[must_use]
    pub fn bind_ask_user_question(
        mut self,
        broker: Arc<AskUserQuestionBroker>,
        rx: tokio::sync::mpsc::Receiver<AskUserQuestionExchange>,
    ) -> Self {
        *self
            .ask_user_question_broker_ref
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::downgrade(&broker));
        let run_loop = broker.clone();
        tokio::spawn(async move { run_loop.run(rx).await });
        self.ask_user_question_broker = Some(broker);
        self
    }

    /// Attach the full command-routing seam (F2-08). Additive over [`Self::bind`]:
    /// once bound, every non-turn/non-permission [`ClientCommand`] is delegated
    /// to the [`CommandRouter`], with replies pushed out through the connection's
    /// [`ClientEventSink`]. A connection with no router (the F2-05/06/07
    /// skeleton) silently drops those commands.
    #[must_use]
    pub fn bind_router(mut self, router: Arc<dyn CommandRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// A clone of this connection's message queue, so the composition root can
    /// wire the SAME per-connection queue into the turn driver (for active-turn
    /// registration) and into the orchestrator's mid-turn input adapter. The
    /// queue is `Arc`-shared, so all three see the same items.
    #[must_use]
    pub fn queue_handle(&self) -> Arc<MessageQueueManager> {
        self.queue.clone()
    }

    /// Queue adapter bound to the desktop Mod catalog after the connection is
    /// assembled. It shares this connection's queue and result correlation.
    pub(crate) fn mod_command_queue(&self) -> Arc<dyn command_api::ModCommandQueue> {
        Arc::new(BridgeModCommandQueue {
            queue: self.queue.clone(),
            pending: self.mod_commands.clone(),
            prompts: self.mod_prompts.clone(),
        })
    }

    fn mod_queue_drain(&self) -> ModQueueDrain {
        ModQueueDrain {
            pending: self.mod_commands.clone(),
            prompts: self.mod_prompts.clone(),
            router: self.router.clone(),
            events: self.unscoped_event_sink(),
        }
    }

    pub(crate) fn cron_requests_handle(&self) -> Arc<crate::cron_host::HostCronRequests> {
        self.cron_requests.clone()
    }

    /// A clone of this connection's dynamic-loop state.
    #[must_use]
    pub fn loop_runtime_handle(&self) -> Arc<tool_cron::LoopRuntime> {
        self.loop_runtime.clone()
    }

    /// A clone of the gate handle, for tests that need to observe the parked /
    /// drained request count directly.
    #[must_use]
    pub fn gate_handle(&self) -> Arc<AdapterPermissionGate> {
        self.gate.clone().expect("gate_handle called before bind()")
    }

    /// A clone of the computer-access broker handle, for tests that need to
    /// observe the parked / drained request count directly.
    #[must_use]
    pub fn computer_access_broker_handle(&self) -> Arc<ComputerAccessBroker> {
        self.computer_access_broker
            .clone()
            .expect("computer_access_broker_handle called before bind_computer_access()")
    }

    /// Claim the single active-client slot, or confirm that `out` belongs to the
    /// client that already owns it.
    async fn claim_outbound(&self, out: &FrameSink) -> bool {
        let _admission = self.connection_admission.lock().await;
        let mut cell = self.out.lock().await;
        match cell.as_ref() {
            Some(active) => active.same_channel(out),
            None => {
                *cell = Some(out.clone());
                true
            }
        }
    }

    fn error_response(id: u64, message: impl Into<String>) -> Frame {
        Frame::Response(BridgeResponse {
            id,
            result: None,
            error: Some(BridgeWireError {
                code: -32600,
                message: message.into(),
            }),
        })
    }

    /// Handle the opening `hello` handshake frame (F2-07).
    ///
    /// The handshake exchanges TWO independently-versioned numbers (governing
    /// decision §0.10): the wire-envelope [`BRIDGE_PROTOCOL_VERSION`]
    /// ([`ClientHello::protocol_version`]) and the DTO-contract
    /// `CLIENT_PROTOCOL_VERSION` ([`Capabilities::client_protocol_version`]). A
    /// MAJOR-version mismatch in EITHER refuses the connection: we reply with a
    /// [`BridgeResponse`] error (no [`ServerHello`]) and leave `handshaken`
    /// false, so no subsequent command is routed. A compatible handshake replies
    /// with our [`ServerHello`] and flips `handshaken`.
    async fn handle_hello(&self, request_id: u64, hello: ClientHello) {
        if self.handshake_refused.load(Ordering::SeqCst) {
            if let Some(sink) = self.out.lock().await.as_ref() {
                let _ = sink.send(Self::error_response(
                    request_id,
                    "connection handshake was already refused",
                ));
            }
            return;
        }
        let bridge_ok = version_compatible(BRIDGE_PROTOCOL_VERSION, &hello.protocol_version);
        let server_caps = Capabilities::default();
        let client_ok = version_compatible(
            &server_caps.client_protocol_version,
            &hello.capabilities.client_protocol_version,
        );

        let response = if bridge_ok && client_ok {
            if let Some(responder) = self.audio_responder.as_ref() {
                responder.update_capabilities(hello.capabilities.audio.clone());
            }
            self.handshaken.store(true, Ordering::SeqCst);
            BridgeResponse {
                id: request_id,
                result: Some(
                    serde_json::to_value(ServerHello {
                        protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
                        server_name: concat!("lingxi-bridge-server/", env!("CARGO_PKG_VERSION"))
                            .to_string(),
                        capabilities: server_caps,
                    })
                    .expect("ServerHello serializes"),
                ),
                error: None,
            }
        } else {
            // Mark the connection refused so every SUBSEQUENT command is dropped
            // (the load-bearing F2-07 guarantee, governing decision §0.10): a
            // peer that disagrees on a breaking version is not merely told "no",
            // it is barred from driving the engine.
            self.handshake_refused.store(true, Ordering::SeqCst);
            // Name WHICH version refused (both may be incompatible at once).
            let message = match (bridge_ok, client_ok) {
                (false, false) => format!(
                    "incompatible bridge protocol version (server {BRIDGE_PROTOCOL_VERSION}, \
                     client {}) and client-protocol version (server {}, client {})",
                    hello.protocol_version,
                    server_caps.client_protocol_version,
                    hello.capabilities.client_protocol_version,
                ),
                (false, true) => format!(
                    "incompatible bridge protocol version (server {BRIDGE_PROTOCOL_VERSION}, \
                     client {})",
                    hello.protocol_version,
                ),
                (true, false) => format!(
                    "incompatible client-protocol version (server {}, client {})",
                    server_caps.client_protocol_version, hello.capabilities.client_protocol_version,
                ),
                (true, true) => unreachable!("the accept branch already handled both-compatible"),
            };
            BridgeResponse {
                id: request_id,
                result: None,
                error: Some(BridgeWireError {
                    // JSON-RPC "invalid request" range; the version disagreement
                    // is a structural rejection of the opening frame.
                    code: -32600,
                    message,
                }),
            }
        };

        if let Some(sink) = self.out.lock().await.as_ref() {
            let _ = sink.send(Frame::Response(response));
            if self.handshaken.load(Ordering::SeqCst) {
                let mut pending = self.pending_openai_oauth.lock().await;
                if let Some(event) = pending.take() {
                    if !sink.send(Frame::Event(event.clone())) {
                        *pending = Some(event);
                    }
                }
                self.cron_requests
                    .replay(|event| sink.send(Frame::Event(event)))
                    .await;
            }
        }
    }

    /// Route one decoded [`ClientCommand`].
    async fn dispatch(&self, command: ClientCommand) {
        if matches!(
            &command,
            ClientCommand::SendPrompt { .. }
                | ClientCommand::RunSlashCommand { .. }
                | ClientCommand::TaskMessage { .. }
        ) {
            if let Some(registry) = &self.task_notification_registry {
                registry.update_shell_session_activity(
                    true,
                    self.turn_running.load(Ordering::SeqCst),
                    true,
                );
            }
        }
        match command {
            ClientCommand::UiAttach { surface, client_id } => {
                let _lifecycle = self.ui_lifecycle_gate.lock().await;
                let mut owned = self.ui_attached_clients.lock().await;
                if !owned.contains(&client_id) {
                    if let Some(driver) = self.driver.as_ref() {
                        if driver.ui_attach(&client_id, surface).await {
                            owned.insert(client_id);
                        }
                    }
                }
            }
            ClientCommand::UiDetach { client_id } => {
                let _lifecycle = self.ui_lifecycle_gate.lock().await;
                let was_owned = self.ui_attached_clients.lock().await.remove(&client_id);
                if was_owned {
                    if let Some(driver) = self.driver.as_ref() {
                        driver.ui_detach(&client_id).await;
                    }
                }
            }
            command @ (ClientCommand::UiRender { .. }
            | ClientCommand::UiClientModule { .. }
            | ClientCommand::UiMessage { .. }
            | ClientCommand::UiClientFault { .. }
            | ClientCommand::UiClientPress { .. }
            | ClientCommand::UiPress { .. }
            | ClientCommand::UiInput { .. }
            | ClientCommand::UiSelect { .. }
            | ClientCommand::UiClientOperation { .. }) => {
                let request_id = match &command {
                    ClientCommand::UiRender { request_id, .. }
                    | ClientCommand::UiClientModule { request_id, .. }
                    | ClientCommand::UiMessage { request_id, .. }
                    | ClientCommand::UiClientFault { request_id, .. }
                    | ClientCommand::UiClientPress { request_id, .. }
                    | ClientCommand::UiPress { request_id, .. }
                    | ClientCommand::UiInput { request_id, .. }
                    | ClientCommand::UiSelect { request_id, .. }
                    | ClientCommand::UiClientOperation { request_id, .. } => request_id.clone(),
                    _ => unreachable!("the UI request match is exhaustive"),
                };
                if let Some(router) = self.router.clone() {
                    // UI controls are session-scoped but not owned by the
                    // assistant turn. Route them even while a model turn is
                    // active, and publish the correlated response unscoped.
                    router.route(command, self.unscoped_event_sink()).await;
                } else {
                    self.unscoped_event_sink()
                        .emit(ClientEvent::UiControlResult {
                            request_id,
                            response_json: None,
                            metadata_json: None,
                            error: Some("Mod UI requests need a mounted session".into()),
                        })
                        .await;
                }
            }
            ClientCommand::SendPrompt {
                text,
                images,
                turn_id,
                ..
            } => {
                self.handle_send_prompt(text, images, turn_id).await;
            }
            ClientCommand::ScheduledRunTurn {
                run_id,
                prompt,
                model,
                reasoning,
            } => {
                self.handle_prompt_seed(prompt, Vec::new(), None, Some((run_id, model, reasoning)))
                    .await;
            }
            ClientCommand::Cancel { turn_id } => {
                self.cancel_active_turn(turn_id).await;
            }
            ClientCommand::ApprovePermission {
                request_id,
                response,
            } => {
                self.resolve_permission(request_id, response).await;
            }
            ClientCommand::DenyPermission { request_id } => {
                self.resolve_permission(request_id, PermissionResponseDto::Deny)
                    .await;
            }
            ClientCommand::ApproveComputerAccess {
                request_id,
                response,
            } => {
                self.resolve_computer_access(request_id, response).await;
            }
            ClientCommand::DenyComputerAccess { request_id } => {
                self.deny_computer_access(request_id).await;
            }
            ClientCommand::AudioResponse { identity, result } => {
                self.resolve_audio(identity, result).await;
            }
            ClientCommand::UpdateAudioCapabilities { capabilities } => {
                self.update_audio_capabilities(capabilities).await;
            }
            ClientCommand::AnswerAskUserQuestion {
                request_id,
                answers,
            } => {
                self.resolve_ask_user_question(request_id, answers).await;
            }
            ClientCommand::CancelAskUserQuestion { request_id } => {
                self.cancel_ask_user_question(request_id).await;
            }
            // Manual compaction is a connection-scoped command and is valid
            // while no turn owns the event stream. Its completion must bypass
            // the late-turn filter; automatic in-turn compaction continues to
            // flow through `FrameEventSink` and remains owner-scoped.
            command @ ClientCommand::ForceCompact => {
                let _handoff = self.turn_handoff.lock().await;
                // Do not queue on the orchestrator's turn gate in this input
                // loop: its owner may need a permission reply from this socket.
                if self.turn_running.load(Ordering::SeqCst) {
                    self.unscoped_event_sink()
                        .emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: "cannot compact the session while a turn is in flight".into(),
                        })
                        .await;
                    return;
                }
                let _transition = TransitionScope::begin(&self.transition_active);
                if let Some(router) = self.router.clone() {
                    router.route(command, self.unscoped_event_sink()).await;
                } else {
                    tracing::debug!(
                        "bridge-server: force compact not routed (no CommandRouter bound)"
                    );
                }
            }
            // A slash command may be a `type: "prompt"` command (`/loop`,
            // Markdown/Plugin): claude-code injects its expanded prompt as the
            // user turn. Pre-dispatch via the router's dispatcher (which the
            // connection cannot reach otherwise) so a `RunAsTurn` result is fed
            // through the SAME enqueue-or-spawn turn path as a `SendPrompt` (the
            // connection owns the driver + queue + turn-running flag). Display-
            // only / unknown / no-dispatcher cases fall back to the router's
            // text-surface path.
            ClientCommand::RunSlashCommand { raw, turn_id } => {
                // Check before dispatch_slash: builtins execute there, not in
                // the later display-only routing fallback. Use connection run
                // ownership, which also covers queued follow-up turns.
                let is_compact = command_api::parser::parse_slash_command(&raw)
                    .is_some_and(|parsed| parsed.name.eq_ignore_ascii_case("compact"));
                // Match the ALIAS SPELLINGS the registry accepts, not just the
                // canonical builtin: `commands/core/src/register.rs` maps both
                // `new` and `reset` onto `clear`, and `dispatch_slash` resolves
                // through that map. Missing a spelling here does not stop the
                // conversation from being replaced — it only skips
                // `stop_loop_for_session_transition`, so the armed `/loop`
                // wakeup and its already-queued `loop-wakeup-*` command leak
                // into the new session.
                let changes_session =
                    command_api::parser::parse_slash_command(&raw).is_some_and(|parsed| {
                        matches!(
                            parsed.name.to_ascii_lowercase().as_str(),
                            "clear" | "new" | "reset"
                        )
                    });
                let _handoff = if is_compact || changes_session {
                    Some(self.turn_handoff.lock().await)
                } else {
                    None
                };
                if (is_compact || changes_session) && self.turn_running.load(Ordering::SeqCst) {
                    self.unscoped_event_sink()
                        .emit(ClientEvent::SlashCommandResult {
                            turn_id,
                            display: if is_compact {
                                "cannot compact the session while a turn is in flight"
                            } else {
                                "cannot replace the session while a turn is in flight"
                            }
                            .into(),
                            is_error: true,
                        })
                        .await;
                    return;
                }
                let _transition = (is_compact || changes_session)
                    .then(|| TransitionScope::begin(&self.transition_active));
                if changes_session {
                    self.stop_loop_for_session_transition().await;
                }
                if is_compact {
                    self.loop_runtime.reset_autonomous_loop_delivered();
                }
                let context = command_api::ModCommandRunContext {
                    origin: serde_json::json!({"kind":"bridge"}),
                    is_fullscreen: false,
                    columns: 80,
                };
                let outcome = match self.router.as_ref() {
                    Some(router) => {
                        command_api::with_mod_command_context(context, router.dispatch_slash(&raw))
                            .await
                    }
                    None => None,
                };
                let authority_events = outcome
                    .as_ref()
                    .map(|outcome| outcome.authority_events.clone())
                    .unwrap_or_default();
                match outcome.map(|outcome| outcome.result) {
                    Some(lingxi_core::host::SlashDispatchResult::RunAsTurn { prompt }) => {
                        // Run the expanded prompt exactly like a direct user
                        // prompt (enqueue-or-spawn; no images).
                        drop(_transition);
                        drop(_handoff);
                        self.handle_send_prompt(prompt, Vec::new(), turn_id).await;
                    }
                    // Display-only / unknown result: surface the SAME dispatch
                    // result's text directly. We must NOT re-`route()` here —
                    // `route(RunSlashCommand)` calls `dispatcher.dispatch()` a
                    // SECOND time (router.rs), which would re-run the builtin's
                    // `handle()` (and any of its `EmitEffects`/`InjectMessage`
                    // side effects) twice and discard the first result. Reuse
                    // the already-computed disposition instead.
                    Some(lingxi_core::host::SlashDispatchResult::Handled { display }) => {
                        self.unscoped_event_sink()
                            .emit(ClientEvent::SlashCommandResult {
                                turn_id,
                                display,
                                is_error: false,
                            })
                            .await;
                    }
                    Some(lingxi_core::host::SlashDispatchResult::Unknown { display, .. }) => {
                        self.unscoped_event_sink()
                            .emit(ClientEvent::SlashCommandResult {
                                turn_id,
                                display,
                                is_error: true,
                            })
                            .await;
                    }
                    Some(lingxi_core::host::SlashDispatchResult::NotASlashCommand) => {
                        self.unscoped_event_sink()
                            .emit(ClientEvent::SlashCommandResult {
                                turn_id,
                                display: format!("not a slash command: {raw}"),
                                is_error: true,
                            })
                            .await;
                    }
                    // No dispatcher wired: delegate to the router's text-surface
                    // path, which emits the "no slash-command dispatcher wired"
                    // error (unchanged behavior). It dispatches at most once.
                    None => {
                        if let Some(router) = self.router.clone() {
                            router
                                .route(
                                    ClientCommand::RunSlashCommand { raw, turn_id },
                                    self.unscoped_event_sink(),
                                )
                                .await;
                        }
                    }
                }
                for event in authority_events {
                    self.unscoped_event_sink().emit(event).await;
                }
                self.refresh_permission_session().await;
            }
            command @ (ClientCommand::ClearSession
            | ClientCommand::NewSession { .. }
            | ClientCommand::ResumeSession { .. }) => {
                let _handoff = self.turn_handoff.lock().await;
                // The connection owns the actual run loop, including follow-ups.
                // Never wait for the SDK turn gate from an ordinary callback:
                // disconnect must finish that callback before draining prompts.
                if self.turn_running.load(Ordering::SeqCst) {
                    self.unscoped_event_sink()
                        .emit(ClientEvent::Error {
                            kind: ErrorKindDto::Protocol,
                            message: "cannot replace the session while a turn is in flight".into(),
                        })
                        .await;
                    return;
                }
                let _transition = TransitionScope::begin(&self.transition_active);
                // Only an admitted replacement owns timer teardown.
                self.stop_loop_for_session_transition().await;
                if let Some(router) = &self.router {
                    // Session transitions have no active turn. Their restored
                    // usage and notices are connection snapshots, not live
                    // model output. Keep the normal event sink owner-scoped so
                    // late output from an old turn is still dropped.
                    router.route(command, self.unscoped_event_sink()).await;
                }
                self.refresh_permission_session().await;
            }
            // The FULL command surface (model, listings, slash, tasks, session
            // control) is delegated to the bound [`CommandRouter`] (F2-08), which
            // reaches the engine handles and pushes replies out through the
            // connection's event sink. A skeleton connection with no router
            // (F2-05/06/07) silently drops these.
            other => {
                if let Some(router) = self.router.clone() {
                    router.route(other, self.event_sink()).await;
                } else {
                    tracing::debug!("bridge-server: command not routed (no CommandRouter bound)");
                }
            }
        }
    }

    /// Handle an inbound [`ClientCommand::SendPrompt`] (enqueue-or-spawn).
    ///
    /// Parity twin of print.ts run()'s `running` guard (L1866) + queue path:
    ///
    /// - If the single turn-drain loop is ALREADY running, the prompt is
    ///   ENQUEUED (priority `Next`, the default for direct prompt input) rather
    ///   than spawning a SECOND concurrent turn — two `run_turn`s would race on
    ///   the shared `session.history`. The owning loop drains it as a follow-up.
    /// - Otherwise this prompt WINS the `turn_running` flag and OWNS the loop:
    ///   it runs the seed prompt, then drains every queued main-thread command
    ///   as a follow-up turn before clearing the flag (port of print.ts:2371-2406
    ///   `do { drainCommandQueue() } while(...)`).
    ///
    /// The loop is SPAWNED so `on_frame` returns promptly and the read loop
    /// stays free to service the approval that unblocks a parked permission
    /// `check()`. The guard is on `SendPrompt` only; permission frames are never
    /// queued (see [`Self::resolve_permission`]).
    ///
    /// Follow-up input carrying images or a turn ID is retained as one whole
    /// prompt. Plain uncorrelated text can still enter the SDK's mid-turn batch;
    /// metadata-bearing input runs independently after the current turn.
    async fn stop_loop_for_session_transition(&self) {
        if let Some(driver) = &self.driver {
            driver.stop_dynamic_loop().await;
        }
        let queued: Vec<_> = self
            .queue
            .snapshot()
            .await
            .into_iter()
            .filter(|command| {
                command.source == QueueSource::Plugin
                    || command.source == QueueSource::Cron
                        && command.uuid.starts_with("loop-wakeup-")
            })
            .map(|command| command.uuid)
            .collect();
        for id in &queued {
            if let Some(pending) = self.mod_commands.lock().unwrap().remove(id) {
                let _ = pending.settle.send(Err(
                    "the command was removed from the queue before it ran".into(),
                ));
            }
            if let Some(pending) = self.mod_prompts.lock().unwrap().remove(id) {
                let _ = pending.settle.send(Ok(serde_json::json!({
                    "drop":"the prompt was removed from the queue before it ran"
                })));
            }
        }
        self.queue.remove(&queued, "conversation replaced").await;
        self.loop_runtime.reset();
    }

    async fn handle_send_prompt(
        &self,
        text: String,
        images: Vec<ImageRefDto>,
        turn_id: Option<u64>,
    ) {
        self.handle_prompt_seed(text, images, turn_id, None).await;
    }

    async fn handle_prompt_seed(
        &self,
        text: String,
        images: Vec<ImageRefDto>,
        turn_id: Option<u64>,
        scheduled: Option<(
            String,
            String,
            client::protocol::controls::ReasoningSelectionDto,
        )>,
    ) {
        // A new seed turn is intervening work, even if the prior loop tick
        // already finished and its keepalive consumed the in-flight marker.
        self.loop_runtime.invalidate_noop_streak();
        let Some(driver) = self.driver.clone() else {
            return;
        };

        // Try to win the run-loop ownership. compare_exchange fails if a turn is
        // already running, in which case we enqueue instead of spawning.
        let _handoff = self.turn_handoff.lock().await;
        if self
            .turn_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            if let Some((run_id, _, _)) = scheduled {
                self.unscoped_event_sink()
                    .emit(ClientEvent::ScheduledRunFinished {
                        run_id,
                        summary: None,
                        error: Some("busy:Target session is running".into()),
                    })
                    .await;
                return;
            }
            let mut command = prompt_command(text);
            if !images.is_empty() || turn_id.is_some() {
                // `Later` is not consumed by the SDK's text-only mid-turn
                // adapter. The connection restores this payload at follow-up
                // admission instead of silently dropping images or correlation.
                command.priority = QueuePriority::Later;
                self.queued_prompt_payloads.lock().await.insert(
                    command.uuid.clone(),
                    QueuedPromptPayload { images, turn_id },
                );
            }
            self.queue.enqueue(command).await;
            if !lingxi_core::host::env::background_tasks_disabled() {
                if let Some(registry) = &self.task_notification_registry {
                    registry
                        .background_all_tasks_with_reason(
                            lingxi_core::host::task_registry::TaskBackgroundReason::DeliverMessage,
                        )
                        .await;
                }
            }
            return;
        }

        let queue = self.queue.clone();
        let queued_prompt_payloads = self.queued_prompt_payloads.clone();
        let loop_runtime = self.loop_runtime.clone();
        let turn_running = self.turn_running.clone();
        let turn_handoff = self.turn_handoff.clone();
        let active_turn = self.active_turn.clone();
        let interactions = TurnInteractions {
            gate: self.gate.clone(),
            computer_access_broker: self.computer_access_broker.clone(),
            ask_user_question_broker: self.ask_user_question_broker.clone(),
            tool_names: self.tool_names.clone(),
        };
        // Claim the active owner before spawning. Cancel can now safely arrive
        // immediately after SendPrompt without missing the driver's token.
        let (seed_generation, seed_cancel) = begin_owned_turn(&active_turn, &driver, turn_id).await;
        let scheduled_sink = self.unscoped_event_sink();
        let mod_drain = self.mod_queue_drain();
        let task = tokio::spawn(async move {
            // Seed turn — the prompt that won the loop (carries its images).
            if let Some((run_id, model, reasoning)) = scheduled {
                let result = driver
                    .run_scheduled_turn(text, model, reasoning, seed_cancel)
                    .await;
                let (summary, error) = match result {
                    Ok(summary) => (Some(summary), None),
                    Err(error) => (None, Some(error)),
                };
                scheduled_sink
                    .emit(ClientEvent::ScheduledRunFinished {
                        run_id,
                        summary,
                        error,
                    })
                    .await;
            } else {
                orchestrator::mod_prompt_origin::with_origin(
                    serde_json::json!({"kind":"bridge"}),
                    driver.run_turn_with_images_and_cancel(text, images, seed_cancel),
                )
                .await;
            }
            finish_turn(&active_turn, seed_generation, &interactions, &turn_handoff).await;

            // Between-turn drain: run queued main-thread prompts as follow-up
            // turns until the queue is empty. Re-check after clearing the flag to
            // close the race where a prompt enqueues between the empty-check and
            // the flag clear (twin of print.ts recheckCommandQueue).
            loop {
                drain_main_thread(
                    &driver,
                    &queue,
                    &loop_runtime,
                    &active_turn,
                    &interactions,
                    &queued_prompt_payloads,
                    &turn_handoff,
                    Some(&mod_drain),
                )
                .await;
                let _handoff = turn_handoff.lock().await;
                turn_running.store(false, Ordering::SeqCst);
                // If a prompt slipped in after the last drain but before the
                // store, re-claim the loop and drain again; otherwise we're done.
                if queue.has_main_thread_commands().await
                    && turn_running
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    continue;
                }
                break;
            }
        });
        *self
            .active_turn_task
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(task);
    }

    async fn cancel_active_turn(&self, turn_id: Option<u64>) {
        // The active token can be cancelled without waiting for an ordinary
        // control's handoff. Queue lookup must re-check after acquiring that
        // lock because the input may just have been promoted to its own turn;
        // the matched owner's broker cleanup is fenced separately below.
        let mut target = self.active_turn.cancellation_target(turn_id);
        if target.is_none() {
            if let Some(turn_id) = turn_id {
                let _handoff = self.turn_handoff.lock().await;
                target = self.active_turn.cancellation_target(Some(turn_id));
                if target.is_none() {
                    let mut payloads = self.queued_prompt_payloads.lock().await;
                    let queued = payloads
                        .iter()
                        .filter(|(_, payload)| payload.turn_id == Some(turn_id))
                        .map(|(uuid, _)| uuid.clone())
                        .collect::<Vec<_>>();
                    for uuid in &queued {
                        payloads.remove(uuid);
                    }
                    drop(payloads);
                    self.queue
                        .remove(&queued, "queued client turn cancelled")
                        .await;
                }
            }
        }
        let Some((generation, cancel)) = target else {
            tracing::debug!(?turn_id, "bridge-server: ignored stale or idle turn cancel");
            return;
        };
        if !lingxi_core::host::env::background_tasks_disabled() {
            if let Some(registry) = &self.task_notification_registry {
                registry
                    .background_all_tasks_with_reason(
                        lingxi_core::host::task_registry::TaskBackgroundReason::TurnAbort,
                    )
                    .await;
            }
        }
        cancel.cancel();
        // Deliver cancellation before waiting for the handoff, so it still
        // bypasses an ordinary control. Cleanup shares owner retirement and
        // admission: none of these awaited broker drains can reach a successor.
        let _handoff = self.turn_handoff.lock().await;
        if self
            .active_turn
            .cancellation_target(None)
            .is_none_or(|(current, _)| current != generation)
        {
            return;
        }

        // Interaction state belongs to the matched turn. Drain it only after a
        // successful identity match; a stale Cancel must have zero effect on a
        // newer turn's prompts or durable permission decisions.
        TurnInteractions {
            gate: self.gate.clone(),
            computer_access_broker: self.computer_access_broker.clone(),
            ask_user_question_broker: self.ask_user_question_broker.clone(),
            tool_names: self.tool_names.clone(),
        }
        .drain(self.active_turn.permission_owner_id(generation))
        .await;
    }

    /// Resolve a parked permission request on the gate (the WS read task side of
    /// the inverted handshake). Looks the recorded tool name back up so an
    /// `AllowAlways` can append the right session rule.
    async fn resolve_permission(&self, request_id: u64, response: PermissionResponseDto) {
        let Some(gate) = self.gate.as_ref() else {
            return;
        };
        let background_owned = gate.request_is_background(request_id).await == Some(true);
        let tool_name = self
            .tool_names
            .lock()
            .await
            .remove(&request_id)
            .unwrap_or_default();
        let resolved = gate.resolve(request_id, response, &tool_name).await;
        // A stale approval must not resurrect a turn that is already idle or
        // disconnected. Only the gate's successful removal proves that this
        // response owned a pending request, and the active-turn check preserves
        // terminal/disconnected state when cancellation won the race.
        if resolved
            && self.handshaken.load(Ordering::SeqCst)
            && (background_owned || self.active_turn.accepts_interactions())
        {
            lingxi_core::host::live_sessions::set_process_status("busy", None);
        }
        if !resolved {
            tracing::debug!(
                request_id,
                "bridge-server: resolve for unknown / already-resolved permission id"
            );
        }
    }

    async fn refresh_permission_session(&self) {
        if let (Some(gate), Some(driver)) = (&self.gate, &self.driver) {
            gate.set_session_id(driver.current_session_id().await);
        }
    }

    /// Resolve a parked `computer`-tool `request_access` prompt on the broker
    /// (the WS read task side of the SAME inverted handshake shape the
    /// permission gate uses).
    async fn resolve_computer_access(&self, request_id: u64, response: ComputerAccessResponseDto) {
        let Some(broker) = self.computer_access_broker.as_ref() else {
            return;
        };
        let resolved = broker.resolve(request_id, response).await;
        if !resolved {
            tracing::debug!(
                request_id,
                "bridge-server: resolve for unknown / already-resolved computer-access id"
            );
        }
    }

    /// Deny a parked `computer`-tool `request_access` prompt on the broker.
    async fn deny_computer_access(&self, request_id: u64) {
        let Some(broker) = self.computer_access_broker.as_ref() else {
            return;
        };
        let resolved = broker.deny(request_id).await;
        if !resolved {
            tracing::debug!(
                request_id,
                "bridge-server: deny for unknown / already-resolved computer-access id"
            );
        }
    }

    /// Resolve a parked audio trait call with the client's outcome (the WS read
    /// task side of the SAME inverted handshake the permission gate and the
    /// computer-access broker use).
    async fn resolve_audio(
        &self,
        identity: client::protocol::audio::AudioOperationIdDto,
        result: client::protocol::audio::AudioOperationResultDto,
    ) {
        let Some(responder) = self.audio_responder.as_ref() else {
            return;
        };
        let resolved = responder.resolve(identity.clone(), result).await;
        if !resolved {
            // Unknown, already resolved, or already past its deadline. A safe
            // no-op: the caller has been given an answer either way, and a
            // client is allowed to answer a request we stopped waiting for.
            tracing::debug!(
                operation_id = %identity.id,
                "bridge-server: response for unknown / already-resolved audio id"
            );
        }
    }

    async fn update_audio_capabilities(
        &self,
        capabilities: client::protocol::audio::AudioCapabilitySnapshotDto,
    ) {
        let Some(responder) = self.audio_responder.as_ref() else {
            return;
        };
        responder.update_capabilities(Some(capabilities.clone()));
        self.unscoped_event_sink()
            .emit(ClientEvent::AudioCapabilitiesChanged { capabilities })
            .await;
    }

    async fn resolve_ask_user_question(&self, request_id: u64, answers: HashMap<String, String>) {
        let Some(broker) = self.ask_user_question_broker.as_ref() else {
            return;
        };
        if !broker.resolve(request_id, answers).await {
            tracing::debug!(
                request_id,
                "bridge-server: resolve for unknown / already-resolved AskUserQuestion id"
            );
        }
    }

    async fn cancel_ask_user_question(&self, request_id: u64) {
        let Some(broker) = self.ask_user_question_broker.as_ref() else {
            return;
        };
        if !broker.cancel(request_id).await {
            tracing::debug!(
                request_id,
                "bridge-server: cancel for unknown / already-resolved AskUserQuestion id"
            );
        }
    }
}

#[async_trait]
impl FramePump for BridgeConnection {
    fn is_priority_frame(&self, frame: &Frame) -> bool {
        let Frame::Request(request) = frame else {
            return false;
        };
        if request.method == "hello" {
            return false;
        }
        if request.method == "permission_request_scope" {
            return self.handshaken.load(Ordering::SeqCst);
        }
        match serde_json::from_value::<ClientCommand>(request.params.clone()) {
            // A cancel for a prompt still queued behind a slow control must
            // remain behind that prompt. Otherwise it is ignored before the
            // prompt has claimed an owner and the cancelled work starts later.
            Ok(ClientCommand::Cancel { turn_id }) => {
                self.active_turn.cancellation_target(turn_id).is_some()
            }
            // Keep epoch changes ordered with audio responses even when an
            // unrelated ordinary command is stalled. Before hello completes,
            // the update must remain behind the handshake.
            Ok(ClientCommand::UpdateAudioCapabilities { .. }) => {
                self.handshaken.load(Ordering::SeqCst)
            }
            Ok(
                ClientCommand::ApprovePermission { .. }
                | ClientCommand::DenyPermission { .. }
                | ClientCommand::ApproveComputerAccess { .. }
                | ClientCommand::DenyComputerAccess { .. }
                | ClientCommand::AudioResponse { .. }
                | ClientCommand::AnswerAskUserQuestion { .. }
                | ClientCommand::CancelAskUserQuestion { .. },
            ) => true,
            _ => false,
        }
    }

    async fn on_frame(&self, frame: Frame, out: FrameSink) {
        if !self.claim_outbound(&out).await {
            if let Frame::Request(request) = frame {
                let _ = out.send(Self::error_response(
                    request.id,
                    "bridge-server already has an active client",
                ));
            }
            return;
        }
        match frame {
            // The opening handshake (F2-07) rides the request/response envelope:
            // `method == "hello"` with a `ClientHello` payload. It is answered
            // synchronously with an accept (`ServerHello`) or a version-refusal
            // error, and is the ONLY frame routed before `handshaken` is set.
            Frame::Request(BridgeRequest {
                id, method, params, ..
            }) if method == "hello" => match serde_json::from_value::<ClientHello>(params) {
                Ok(hello) => self.handle_hello(id, hello).await,
                Err(e) => {
                    tracing::debug!(error = %e, "bridge-server: undecodable ClientHello");
                    if let Some(sink) = self.out.lock().await.as_ref() {
                        let _ = sink.send(Self::error_response(id, "invalid hello request"));
                    }
                }
            },
            Frame::Request(BridgeRequest { id, method, params }) => {
                if !self.handshaken.load(Ordering::SeqCst) {
                    let message = if self.handshake_refused.load(Ordering::SeqCst) {
                        "connection handshake was refused"
                    } else {
                        "successful hello required before commands"
                    };
                    if let Some(sink) = self.out.lock().await.as_ref() {
                        let _ = sink.send(Self::error_response(id, message));
                    }
                    return;
                }
                if method == "permission_request_scope" {
                    let request_id = params
                        .as_object()
                        .filter(|params| params.len() == 1)
                        .and_then(|params| params.get("request_id"))
                        .and_then(serde_json::Value::as_u64);
                    let response = match request_id {
                        Some(request_id) => {
                            let scope = match &self.gate {
                                Some(gate) => gate.request_is_background(request_id).await,
                                None => None,
                            };
                            Frame::Response(BridgeResponse {
                                id,
                                result: Some(scope.map_or(serde_json::Value::Null, |background_owned| {
                                    serde_json::json!({"request_id":request_id,"background_owned":background_owned})
                                })),
                                error: None,
                            })
                        }
                        None => Self::error_response(id, "invalid permission request scope query"),
                    };
                    if let Some(sink) = self.out.lock().await.as_ref() {
                        let _ = sink.send(response);
                    }
                    return;
                }
                if method == "desktop_runtime_snapshot" {
                    // This router seam only reads state into a collector. Fence
                    // its read and reply against live outbound pushes: a worker
                    // mutation is either in the snapshot or delivered after it,
                    // never sent before a stale snapshot can replace that fact.
                    // No broker drain, turn-gate wait, or outbound callback is
                    // allowed inside runtime_snapshot.
                    let out = self.out.lock().await;
                    let response = if params != serde_json::json!({"type":"list_session_agents"}) {
                        Self::error_response(id, "invalid desktop runtime snapshot request")
                    } else {
                        let snapshot = match &self.router {
                            Some(router) => router.runtime_snapshot().await,
                            None => Err("desktop runtime snapshot is unavailable".into()),
                        };
                        match snapshot {
                            Ok(events) => Frame::Response(BridgeResponse {
                                id,
                                result: Some(serde_json::json!({"events":events})),
                                error: None,
                            }),
                            Err(error) => Self::error_response(id, error),
                        }
                    };
                    if let Some(sink) = out.as_ref() {
                        let _ = sink.send(response);
                    }
                    return;
                }
                match serde_json::from_value::<ClientCommand>(params) {
                    Ok(command) => self.dispatch(command).await,
                    Err(e) => {
                        tracing::debug!(error = %e, "bridge-server: undecodable command params");
                    }
                }
            }
            // Clients only send `Frame::Request`; a `Frame::Event` /
            // `Frame::Response` / `Frame::PermissionRequest` arriving inbound is
            // a protocol violation we simply ignore (server-push directions).
            _ => {
                tracing::debug!("bridge-server: ignoring non-request inbound frame");
            }
        }
    }

    async fn on_close(&self) {
        self.close_connection(None).await;
    }

    async fn on_close_with_sink(&self, sink: FrameSink) {
        self.close_connection(Some(&sink)).await;
    }
}

impl BridgeConnection {
    /// Abort without waiting. `Drop` cannot await, and a caller that is
    /// tearing the connection down anyway has nothing to learn from the join.
    fn abort_active_turn_task(&self) {
        let active_turn_task = {
            self.active_turn_task
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take()
        };
        if let Some(handle) = active_turn_task {
            handle.abort();
        }
    }

    /// Abort and wait for the task to actually be destroyed. Acknowledging a
    /// cancellation is not proof the future released what it held.
    async fn abort_and_join_active_turn_task(&self) {
        let active_turn_task = {
            self.active_turn_task
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take()
        };
        if let Some(handle) = active_turn_task {
            handle.abort();
            let _ = handle.await;
        }
    }

    async fn join_question_rejections(&self) {
        loop {
            let tasks = std::mem::take(
                &mut *self
                    .question_rejections
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
            );
            if tasks.is_empty() {
                break;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    async fn close_connection(&self, closing: Option<&FrameSink>) {
        let _admission = self.connection_admission.lock().await;
        let mut active = self.out.lock().await;
        if closing.is_some_and(|sink| {
            active
                .as_ref()
                .is_none_or(|current| !current.same_channel(sink))
        }) {
            return;
        }
        // New sockets wait for the admission barrier through all cleanup.
        // Release the send lock first: broker.drain() emits resolved events and
        // must be able to reacquire it. Those events cannot reach a new socket.
        self.handshaken.store(false, Ordering::SeqCst);
        self.handshake_refused.store(false, Ordering::SeqCst);
        self.cron_requests.disconnected().await;
        *active = None;
        drop(active);
        {
            let _lifecycle = self.ui_lifecycle_gate.lock().await;
            let attached = std::mem::take(&mut *self.ui_attached_clients.lock().await);
            if let Some(driver) = self.driver.as_ref() {
                for client_id in attached {
                    driver.ui_detach(&client_id).await;
                }
            }
        }
        self.abort_and_join_active_turn_task().await;
        self.turn_running.store(false, Ordering::SeqCst);
        self.active_turn.clear();
        self.queue.clear_active_turn().await;
        self.queue.clear().await;
        for (_, pending) in self.mod_commands.lock().unwrap().drain() {
            let _ = pending.settle.send(Err(
                "the command was removed from the queue before it ran".into(),
            ));
        }
        for (_, pending) in self.mod_prompts.lock().unwrap().drain() {
            let _ = pending.settle.send(Ok(serde_json::json!({
                "drop":"the prompt was removed from the queue before it ran"
            })));
        }
        self.queued_prompt_payloads.lock().await.clear();
        self.loop_runtime.take_in_flight_prompt();
        self.tool_names.lock().await.clear();

        // Fail-closed: drop every parked permission sender so any in-flight
        // `check()` resolves `Deny` (the client that would approve is gone).
        if let Some(gate) = self.gate.as_ref() {
            let drained = gate.drain().await;
            if drained > 0 {
                tracing::debug!(
                    drained,
                    "bridge-server: drained parked permissions on close"
                );
            }
        }

        // Same fail-closed guarantee for the computer-access broker: drop
        // every parked oneshot so an in-flight `TuiBridgeResolver::resolve`
        // await resolves to the fully-denied default.
        if let Some(broker) = self.computer_access_broker.as_ref() {
            let drained = broker.drain().await;
            if drained > 0 {
                tracing::debug!(
                    drained,
                    "bridge-server: drained parked computer-access requests on close"
                );
            }
        }
        if let Some(broker) = self.ask_user_question_broker.as_ref() {
            let drained = broker.drain().await;
            if drained > 0 {
                tracing::debug!(
                    drained,
                    "bridge-server: drained parked AskUserQuestion requests on close"
                );
            }
        }

        // Publication is now complete and the broker's pending map is empty.
        // Join every deferred rejection before letting a new socket claim.
        self.join_question_rejections().await;

        // Audio requests are drained on DISCONNECT only, never at end-of-turn
        // (they are not in `TurnInteractions`): an audio op is a device
        // operation, not a per-turn user interaction, and `is_recording` is
        // reachable with no turn active at all. Without this the caller would
        // wait out its full deadline for an answer the departed client can
        // never send.
        if let Some(responder) = self.audio_responder.as_ref() {
            let drained = responder.drain().await;
            if drained > 0 {
                tracing::debug!(
                    drained,
                    "bridge-server: drained parked audio requests on close"
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "server/compact_admission_test.rs"]
mod compact_admission_test;

#[cfg(test)]
#[path = "server/connection_regression_test.rs"]
mod connection_regression_test;

#[cfg(test)]
#[path = "server/turn_handoff_regression_test.rs"]
mod turn_handoff_regression_test;

#[cfg(test)]
#[path = "server/permission_owner_regression_test.rs"]
mod permission_owner_regression_test;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;
    use client::adapter::{AdapterPermissionGate, PermissionRequestSink};
    use client::protocol::commands::{ClientCommand, ImageRefDto, UiSurfaceDto};
    use client::protocol::events::ClientEvent;
    use client::protocol::permission::{PermissionRequest, PermissionResponseDto};
    use lingxi_core::host::{PermissionDecision, PermissionGate};
    use msgqueue::{MessageQueueManager, QueuePriority, QueueSource};
    use tokio::sync::{Mutex, Notify};
    use tokio_util::sync::CancellationToken;

    use super::{
        batchable_prompt_snapshot, drain_main_thread, is_owned_turn_event, run_queued_mod_command,
        ActiveTurnControl, BridgeConnection, BridgeModCommandQueue, ModQueueDrain,
        PendingModCommands, TurnDriver, TurnInteractions, UnscopedFrameEventSink,
    };

    #[test]
    fn terminal_closes_all_turn_events_without_releasing_the_driver_slot() {
        let active = ActiveTurnControl::default();
        let (generation, _) = active.begin(Some(7));
        assert!(active.accepts_turn_events());
        assert_eq!(active.begin_terminal(), Some(false));
        assert!(!active.accepts_turn_events());
        assert!(!active.accepts_interactions());
        assert!(
            !active.cancel(Some(7)),
            "a terminal turn is closed and cannot be cancelled again"
        );
        assert!(
            active.is_active(),
            "driver still owns the slot after terminal emission"
        );
        assert_eq!(
            active.begin_terminal(),
            None,
            "a duplicate terminal is rejected"
        );
        active.finish(generation);
        assert!(!active.is_active());

        assert!(is_owned_turn_event(&ClientEvent::TextDelta {
            text: "late".to_string(),
        }));
        // Manual /compact runs while idle; progress must bypass turn ownership.
        assert!(!is_owned_turn_event(&ClientEvent::CompactionStatus {
            phase: "summarizing".to_string(),
            error: None,
        }));
        assert!(is_owned_turn_event(&ClientEvent::CompactionCompleted {
            messages_before: 2,
            messages_after: 1,
            bytes_saved: 10,
            summary: "kept context".to_string(),
        }));
        assert!(is_owned_turn_event(&ClientEvent::SystemNotice {
            message: "late notice".to_string(),
            is_error: false,
        }));
        assert!(is_owned_turn_event(&ClientEvent::UiLog {
            plugin: "review".to_string(),
            text: "late log".to_string(),
        }));
        assert!(is_owned_turn_event(&ClientEvent::UiToast {
            plugin: "review".to_string(),
            text: "late toast".to_string(),
            timeout_ms: 4000,
        }));
        assert!(is_owned_turn_event(&ClientEvent::UiStatus {
            plugin: "review".to_string(),
            text: Some("late status".to_string()),
        }));
        assert!(is_owned_turn_event(&ClientEvent::Attachment {
            attachment: client::protocol::events::AttachmentDto::NestedMemory {
                display_path: "late".to_string(),
            },
        }));
        // A `/loop` fold announces the wakeup that is about to START a turn, so
        // no turn owns it. If it were owned, the ownership filter would drop
        // every one of them — which is exactly what happened to the plain
        // `SystemNotice` resume line until it moved to the unscoped sink.
        assert!(!is_owned_turn_event(&ClientEvent::LoopWakeup {
            message: "Claude resuming /loop wakeup (Sep 7 3:04pm)".to_string(),
            companion: Some(
                "[2 prior /loop wakeups found nothing actionable; loop is healthy.]".to_string(),
            ),
            streak: 2,
            since_ms: 1_788_790_449_000,
        }));
    }

    /// Shared cell capturing the `(prompt, images)` a driver was driven with.
    type CapturedTurn = Arc<Mutex<Option<(String, Vec<ImageRefDto>)>>>;

    /// A [`TurnDriver`] that records the `(prompt, images)` it was driven with, so a
    /// test can assert the `SendPrompt` dispatch forwarded the wire `images`.
    struct RecordingDriver {
        captured: CapturedTurn,
        notify: Arc<Notify>,
    }

    #[derive(Default)]
    struct RecordingSurfaceDriver {
        calls: StdMutex<Vec<String>>,
    }

    #[async_trait]
    impl TurnDriver for RecordingSurfaceDriver {
        async fn run_turn(&self, _prompt: String) {}

        async fn ui_attach(&self, client_id: &str, surface: UiSurfaceDto) -> bool {
            self.calls.lock().unwrap().push(format!(
                "attach:{client_id}:{}",
                match surface {
                    UiSurfaceDto::Desktop => "desktop",
                    UiSurfaceDto::Mobile => "mobile",
                    UiSurfaceDto::Vscode => "vscode",
                }
            ));
            true
        }

        async fn ui_detach(&self, client_id: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("detach:{client_id}"));
            true
        }
    }

    #[async_trait]
    impl TurnDriver for RecordingDriver {
        async fn run_turn(&self, prompt: String) {
            // Record an EMPTY image set so a regression (dispatch taking the old
            // text-only `run_turn` path despite carried images) shows up as a
            // mismatch against the sent images.
            *self.captured.lock().await = Some((prompt, Vec::new()));
            self.notify.notify_one();
        }

        async fn run_turn_with_images(&self, prompt: String, images: Vec<ImageRefDto>) {
            *self.captured.lock().await = Some((prompt, images));
            self.notify.notify_one();
        }
    }

    #[tokio::test]
    async fn ui_surface_commands_are_explicit_owned_and_closed_once() {
        let connection = BridgeConnection::new();
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let driver = Arc::new(RecordingSurfaceDriver::default());
        let connection = connection.bind(gate, driver.clone());

        connection
            .handle_hello(
                1,
                super::ClientHello {
                    protocol_version: super::BRIDGE_PROTOCOL_VERSION.to_owned(),
                    client_name: "electron-window".to_owned(),
                    capabilities: super::Capabilities::default(),
                },
            )
            .await;
        assert!(
            driver.calls.lock().unwrap().is_empty(),
            "hello is not attach"
        );

        connection
            .dispatch(ClientCommand::UiAttach {
                surface: UiSurfaceDto::Desktop,
                client_id: "renderer-a".to_owned(),
            })
            .await;
        connection
            .dispatch(ClientCommand::UiAttach {
                surface: UiSurfaceDto::Desktop,
                client_id: "renderer-a".to_owned(),
            })
            .await;
        connection
            .dispatch(ClientCommand::UiDetach {
                client_id: "other-connection-client".to_owned(),
            })
            .await;
        connection
            .dispatch(ClientCommand::UiAttach {
                surface: UiSurfaceDto::Mobile,
                client_id: "renderer-b".to_owned(),
            })
            .await;
        connection
            .dispatch(ClientCommand::UiDetach {
                client_id: "renderer-a".to_owned(),
            })
            .await;

        connection.close_connection(None).await;
        connection.close_connection(None).await;
        assert_eq!(
            *driver.calls.lock().unwrap(),
            vec![
                "attach:renderer-a:desktop",
                "attach:renderer-b:mobile",
                "detach:renderer-a",
                "detach:renderer-b",
            ]
        );
    }

    /// `bind` requires a gate; this sink drops every request (the dispatch path
    /// under test never emits one).
    struct NoopPermissionSink;

    #[async_trait]
    impl PermissionRequestSink for NoopPermissionSink {
        async fn emit_request(&self, _request: PermissionRequest) {}
    }

    #[tokio::test]
    async fn codex_rotation_before_host_connect_is_retained_without_turn() {
        let connection = BridgeConnection::new();
        let session = client::protocol::events::OpenAiOAuthSessionDto {
            access_token: "rotated".into(),
            refresh_token: Some("next-refresh".into()),
            expires_at: 2_000_000_000,
            account_id: Some("acct".into()),
            fedramp: false,
            email: Some("acct@example.com".into()),
        };
        connection
            .event_sink()
            .emit(ClientEvent::OpenAiOAuthUpdated {
                session: session.clone(),
            })
            .await;
        assert_eq!(
            *connection.pending_openai_oauth.lock().await,
            Some(ClientEvent::OpenAiOAuthUpdated { session })
        );
    }

    #[tokio::test]
    async fn queued_teammate_message_wakes_idle_driver_without_interrupting_busy_turn() {
        let captured = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: notify.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new()
            .bind(gate, driver)
            .with_queue_wakeup();
        connection.handshaken.store(true, Ordering::SeqCst);
        connection.turn_running.store(true, Ordering::SeqCst);
        let active = CancellationToken::new();
        connection.queue.register_active_turn(active.clone()).await;
        let mut command = super::prompt_command(
            "<teammate-message teammate_id=\"researcher\">\nDone\n</teammate-message>".into(),
        );
        command.source = msgqueue::QueueSource::AgentSendMessage;
        command.skip_slash_commands = true;
        command.is_meta = true;
        connection.queue.enqueue(command).await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!active.is_cancelled());
        assert!(captured.lock().await.is_none());
        connection.turn_running.store(false, Ordering::SeqCst);
        tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
            .await
            .unwrap();
        assert_eq!(
            captured.lock().await.as_ref().unwrap().0,
            "<teammate-message teammate_id=\"researcher\">\nDone\n</teammate-message>"
        );
        assert!(connection.queue.is_empty().await);
    }

    struct PendingNotificationRegistry(AtomicBool, AtomicBool, Notify);
    #[async_trait]
    impl lingxi_core::host::task_registry::TaskRegistryHandle for PendingNotificationRegistry {
        async fn background_all_tasks_with_reason(
            &self,
            reason: lingxi_core::host::task_registry::TaskBackgroundReason,
        ) -> usize {
            if self.1.swap(false, Ordering::SeqCst) {
                assert_eq!(
                    reason,
                    lingxi_core::host::task_registry::TaskBackgroundReason::DeliverMessage
                );
                self.2.notify_one();
                1
            } else {
                0
            }
        }
        async fn has_pending_task_notifications_for(
            &self,
            recipient: Option<lingxi_core::types::AgentId>,
        ) -> bool {
            recipient.is_none() && self.0.load(Ordering::SeqCst)
        }
        async fn create(
            &self,
            _i: lingxi_core::host::task_registry::TaskCreateInput,
        ) -> Result<
            lingxi_core::host::task_registry::TaskRecord,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn get(
            &self,
            _id: &str,
        ) -> Result<
            Option<lingxi_core::host::task_registry::TaskRecord>,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn list(
            &self,
            _f: lingxi_core::host::task_registry::TaskListFilter,
        ) -> Result<
            Vec<lingxi_core::host::task_registry::TaskRecord>,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn update(
            &self,
            _id: &str,
            _p: lingxi_core::host::task_registry::TaskUpdatePatch,
        ) -> Result<
            lingxi_core::host::task_registry::TaskRecord,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn set_status(
            &self,
            _id: &str,
            _s: &str,
        ) -> Result<
            lingxi_core::host::task_registry::TaskRecord,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn kill(
            &self,
            _id: &str,
        ) -> Result<
            lingxi_core::host::task_registry::TaskRecord,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
        async fn output(
            &self,
            _id: &str,
            _o: Option<u64>,
        ) -> Result<
            lingxi_core::host::task_registry::TaskOutputChunk,
            lingxi_core::host::task_registry::TaskRegistryError,
        > {
            unreachable!()
        }
    }

    struct NotificationDriver {
        started: Notify,
        cancelled: Notify,
    }
    #[async_trait]
    impl TurnDriver for NotificationDriver {
        async fn run_turn(&self, _: String) {
            panic!("completion must never become a human prompt");
        }
        async fn run_task_notification_turn(
            &self,
            _: Arc<dyn lingxi_core::host::task_registry::TaskRegistryHandle>,
            cancel: CancellationToken,
        ) {
            self.started.notify_one();
            cancel.cancelled().await;
            self.cancelled.notify_one();
        }
    }

    #[tokio::test]
    async fn task_completion_wakeup_owns_bridge_cancellation_and_waits_for_idle_handshake() {
        let registry = Arc::new(PendingNotificationRegistry(
            AtomicBool::new(true),
            AtomicBool::new(false),
            Notify::new(),
        ));
        let driver = Arc::new(NotificationDriver {
            started: Notify::new(),
            cancelled: Notify::new(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new()
            .bind(gate, driver.clone())
            .with_task_notification_registry(registry.clone())
            .with_queue_wakeup();
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !connection.turn_running.load(Ordering::SeqCst),
            "no model turn before handshake"
        );
        connection.turn_running.store(true, Ordering::SeqCst);
        connection.handshaken.store(true, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            driver.started.notified()
        )
        .await
        .is_err());
        connection.turn_running.store(false, Ordering::SeqCst);
        tokio::time::timeout(std::time::Duration::from_secs(2), driver.started.notified())
            .await
            .unwrap();
        assert!(
            connection.active_turn.accepts_interactions(),
            "wake tools must own permission requests"
        );
        registry.0.store(false, Ordering::SeqCst);
        connection.cancel_active_turn(None).await;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            driver.cancelled.notified(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn queued_message_backgrounds_a_shell_that_arms_after_the_message_arrives() {
        let registry = Arc::new(PendingNotificationRegistry(
            AtomicBool::new(false),
            AtomicBool::new(false),
            Notify::new(),
        ));
        let captured = Arc::new(Mutex::new(None));
        let driver = Arc::new(RecordingDriver {
            captured,
            notify: Arc::new(Notify::new()),
        });
        let connection = BridgeConnection::new()
            .bind(
                Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink))),
                driver,
            )
            .with_task_notification_registry(registry.clone())
            .with_queue_wakeup();
        connection.handshaken.store(true, Ordering::SeqCst);
        connection.turn_running.store(true, Ordering::SeqCst);
        let (_, cancel) = connection.active_turn.begin(None);
        connection
            .queue
            .enqueue(super::prompt_command(
                "arrived while shell was young".into(),
            ))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        registry.1.store(true, Ordering::SeqCst); // foreground arming after queued input
        tokio::time::timeout(std::time::Duration::from_secs(2), registry.2.notified())
            .await
            .unwrap();
        assert!(
            !cancel.is_cancelled(),
            "delivering a queued message must not cancel the old model turn"
        );
    }

    /// A `SendPrompt` carrying inline images must dispatch through
    /// `run_turn_with_images` with those images intact — no longer dropping them
    /// (the MULTIMODAL.1 bridge wiring). An empty-image regression would record an
    /// empty set via the default `run_turn` and fail the equality below.
    #[tokio::test]
    async fn send_prompt_forwards_images_to_run_turn_with_images() {
        let captured = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: notify.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new().bind(gate, driver);

        let images = vec![
            ImageRefDto {
                media_type: "image/png".to_string(),
                base64: "iVBORw0KGgoAAAA".to_string(),
            },
            ImageRefDto {
                media_type: "image/gif".to_string(),
                base64: "R0lGODlhAQAB".to_string(),
            },
        ];
        connection
            .dispatch(ClientCommand::SendPrompt {
                text: "look at these".to_string(),
                prompt_mode: None,
                images: images.clone(),
                turn_id: None,
            })
            .await;

        // `dispatch` SPAWNS the turn; wait for the recording driver to fire.
        notify.notified().await;

        let got = captured
            .lock()
            .await
            .clone()
            .expect("the bound driver must have been driven");
        assert_eq!(got.0, "look at these");
        assert_eq!(
            got.1, images,
            "the SendPrompt images must reach run_turn_with_images, not be dropped"
        );
    }

    /// A [`CommandRouter`] that returns a fixed dispatch result for
    /// `dispatch_slash` and records whether the display-only `route` fallback was
    /// taken. Lets a test prove a `type: "prompt"` slash command (`RunAsTurn`)
    /// reaches `handle_send_prompt` (driving the [`TurnDriver`]) while a display-
    /// only result instead goes through `route`.
    struct StubRouter {
        result: lingxi_core::host::SlashDispatchResult,
        routed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl crate::router::CommandRouter for StubRouter {
        async fn route(
            &self,
            _command: ClientCommand,
            _sink: Arc<dyn client::adapter::ClientEventSink>,
        ) {
            self.routed.store(true, Ordering::SeqCst);
        }
        async fn dispatch_slash(&self, _raw: &str) -> Option<crate::router::SlashDispatchOutcome> {
            Some(crate::router::SlashDispatchOutcome {
                result: self.result.clone(),
                authority_events: Vec::new(),
            })
        }
    }

    struct ModContextExecutor;

    #[async_trait]
    impl command_api::ModCommandExecutor for ModContextExecutor {
        async fn run(
            &self,
            _plugin: &str,
            _command: &str,
            args: &str,
            context: command_api::ModCommandRunContext,
        ) -> Result<String, String> {
            let raw = format!(
                "{}:{}:{}:{}:{args}",
                context.origin["kind"].as_str().unwrap(),
                context.origin["name"].as_str().unwrap_or("-"),
                context.is_fullscreen,
                context.columns
            );
            command_api::record_mod_command_settlement(serde_json::json!({"text":raw}));
            Ok(format!("display: {raw}"))
        }
    }

    struct ModContextRouter(command_api::RegistrySlashDispatcher);

    struct RecordingModContext(Arc<StdMutex<Option<command_api::ModCommandRunContext>>>);

    #[async_trait]
    impl command_api::ModCommandExecutor for RecordingModContext {
        async fn run(
            &self,
            _plugin: &str,
            _command: &str,
            _args: &str,
            context: command_api::ModCommandRunContext,
        ) -> Result<String, String> {
            *self.0.lock().unwrap() = Some(context);
            Ok("recorded".into())
        }
    }

    #[async_trait]
    impl crate::router::CommandRouter for ModContextRouter {
        async fn route(
            &self,
            _command: ClientCommand,
            _sink: Arc<dyn client::adapter::ClientEventSink>,
        ) {
        }

        async fn dispatch_slash(&self, raw: &str) -> Option<crate::router::SlashDispatchOutcome> {
            use lingxi_core::host::SlashCommandDispatcher as _;
            Some(crate::router::SlashDispatchOutcome {
                result: self.0.dispatch(raw).await,
                authority_events: Vec::new(),
            })
        }
    }

    struct ReentrantModRouter {
        queue: Arc<dyn command_api::ModCommandQueue>,
        settled: Arc<Mutex<Option<Result<serde_json::Value, String>>>>,
        queued_result: lingxi_core::host::SlashDispatchResult,
    }

    struct BlockingQueuedDriver {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl TurnDriver for BlockingQueuedDriver {
        async fn run_turn(&self, _prompt: String) {
            panic!("queued turn must use its queue entry");
        }

        async fn run_queued_turn(
            &self,
            _prompt: String,
            _in_human_turn: bool,
            _cancel: CancellationToken,
        ) {
            self.started.notify_one();
            self.release.notified().await;
        }
    }

    #[async_trait]
    impl crate::router::CommandRouter for ReentrantModRouter {
        async fn route(
            &self,
            command: ClientCommand,
            _sink: Arc<dyn client::adapter::ClientEventSink>,
        ) {
            if matches!(command, ClientCommand::ClearSession) {
                let result = self.queue.enqueue("demo", "hello", "Ada").await;
                *self.settled.lock().await = Some(result);
            }
        }

        async fn dispatch_slash(&self, raw: &str) -> Option<crate::router::SlashDispatchOutcome> {
            if raw == "/clear" {
                let result = self.queue.enqueue("demo", "hello", "Ada").await;
                *self.settled.lock().await = Some(result);
                return Some(crate::router::SlashDispatchOutcome {
                    result: lingxi_core::host::SlashDispatchResult::Handled {
                        display: "cleared".into(),
                    },
                    authority_events: Vec::new(),
                });
            }
            Some(crate::router::SlashDispatchOutcome {
                result: self.queued_result.clone(),
                authority_events: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn mod_command_run_uses_bridge_queue_and_settles_local_result() {
        use command_api::ModCommandQueue as _;
        use hooks::mods::ModCommandCatalog as _;

        let queue = Arc::new(MessageQueueManager::new());
        let pending: PendingModCommands = Arc::new(StdMutex::new(HashMap::new()));
        let runner = BridgeModCommandQueue {
            queue: queue.clone(),
            pending: pending.clone(),
            prompts: Arc::new(StdMutex::new(HashMap::new())),
        };
        let call = tokio::spawn(async move { runner.enqueue("demo", "hello", "Ada").await });
        let queued = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(command) = queue.peek(|_| true).await {
                    break command;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(queued.source, QueueSource::Plugin);
        assert_eq!(queued.priority, QueuePriority::Later);
        assert_eq!(queued.text(), Some("/hello Ada"));

        let registry = Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new()));
        let catalog = command_api::RegistryModCommandCatalog::new(registry.clone());
        catalog.bind_executor(Arc::new(ModContextExecutor));
        catalog
            .register(
                "demo",
                serde_json::json!({"name":"hello","description":"Greet"}),
            )
            .await
            .unwrap();
        let drain = ModQueueDrain {
            pending,
            prompts: Arc::new(StdMutex::new(HashMap::new())),
            router: Some(Arc::new(ModContextRouter(
                command_api::RegistrySlashDispatcher::new(registry),
            ))),
            events: Arc::new(UnscopedFrameEventSink {
                out: Arc::new(Mutex::new(None)),
            }),
        };
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        drain_main_thread(
            &driver,
            &queue,
            &Arc::new(tool_cron::LoopRuntime::default()),
            &ActiveTurnControl::default(),
            &TurnInteractions::default(),
            &Arc::new(Mutex::new(HashMap::new())),
            &Arc::new(Mutex::new(())),
            Some(&drain),
        )
        .await;
        assert_eq!(
            call.await.unwrap().unwrap(),
            serde_json::json!({"text":"plugin:demo:false:80:Ada"})
        );
    }

    #[tokio::test]
    async fn mod_prompt_submit_uses_later_queue_and_settles_on_admission() {
        let connection = BridgeConnection::new();
        let queue = connection.queue_handle();
        let runner = connection.mod_command_queue();
        let call =
            tokio::spawn(async move { runner.enqueue_prompt("demo", "follow up", true).await });
        let queued = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(command) = queue.peek(|_| true).await {
                    break command;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(queued.text(), Some("follow up"));
        assert_eq!(queued.source, QueueSource::Plugin);
        assert_eq!(queued.priority, QueuePriority::Later);
        assert!(!queued.is_meta);
        assert!(batchable_prompt_snapshot(
            &queue,
            &connection.queued_prompt_payloads,
            &connection.turn_handoff,
        )
        .await
        .is_empty());

        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let turn = run_queued_mod_command(
            &queued,
            &connection.mod_queue_drain(),
            &driver,
            &connection.active_turn,
            &connection.turn_handoff,
            false,
        )
        .await
        .unwrap();
        assert_eq!(turn.prompt, "follow up");
        assert!(turn.as_user);
        assert_eq!(
            turn.origin,
            serde_json::json!({"kind":"plugin","name":"demo","asUser":true})
        );
        assert_eq!(
            call.await.unwrap().unwrap(),
            serde_json::json!({"text":"follow up"})
        );
    }

    #[tokio::test]
    async fn bridge_plugin_prompt_origin_reaches_the_driven_turn() {
        struct OriginDriver(Arc<Mutex<Option<serde_json::Value>>>);

        #[async_trait]
        impl TurnDriver for OriginDriver {
            async fn run_turn(&self, _prompt: String) {
                *self.0.lock().await = Some(orchestrator::mod_prompt_origin::current());
            }
        }

        let connection = BridgeConnection::new();
        let queue = connection.queue_handle();
        let runner = connection.mod_command_queue();
        let call =
            tokio::spawn(async move { runner.enqueue_prompt("demo", "follow up", true).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while queue.peek(|_| true).await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let seen = Arc::new(Mutex::new(None));
        let driver: Arc<dyn TurnDriver> = Arc::new(OriginDriver(seen.clone()));
        drain_main_thread(
            &driver,
            &queue,
            &Arc::new(tool_cron::LoopRuntime::default()),
            &connection.active_turn,
            &TurnInteractions::default(),
            &connection.queued_prompt_payloads,
            &connection.turn_handoff,
            Some(&connection.mod_queue_drain()),
        )
        .await;
        assert_eq!(
            call.await.unwrap().unwrap(),
            serde_json::json!({"text":"follow up"})
        );
        assert_eq!(
            *seen.lock().await,
            Some(serde_json::json!({"kind":"plugin","name":"demo","asUser":true}))
        );
    }

    #[tokio::test]
    async fn bridge_session_transition_retires_queued_mod_command() {
        let connection = BridgeConnection::new();
        let queue = connection.queue_handle();
        let runner = connection.mod_command_queue();
        let call = tokio::spawn(async move { runner.enqueue("demo", "hello", "Ada").await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while queue.peek(|_| true).await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        connection.stop_loop_for_session_transition().await;
        assert_eq!(
            call.await.unwrap().unwrap_err(),
            "the command was removed from the queue before it ran"
        );
        assert!(queue.snapshot().await.is_empty());

        let prompt_runner = connection.mod_command_queue();
        let prompt_call = tokio::spawn(async move {
            prompt_runner
                .enqueue_prompt("demo", "follow up", false)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while queue.peek(|_| true).await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        connection.stop_loop_for_session_transition().await;
        assert_eq!(
            prompt_call.await.unwrap().unwrap(),
            serde_json::json!({"drop":"the prompt was removed from the queue before it ran"})
        );
        assert!(queue.snapshot().await.is_empty());

        let runner = connection.mod_command_queue();
        let disconnect_call =
            tokio::spawn(async move { runner.enqueue("demo", "hello", "again").await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while queue.peek(|_| true).await.is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        connection.close_connection(None).await;
        assert_eq!(
            disconnect_call.await.unwrap().unwrap_err(),
            "the command was removed from the queue before it ran"
        );
        assert!(queue.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn bridge_slash_input_stamps_bridge_origin_on_mod_command() {
        use hooks::mods::ModCommandCatalog as _;

        let registry = Arc::new(tokio::sync::RwLock::new(command_api::CommandRegistry::new()));
        let catalog = command_api::RegistryModCommandCatalog::new(registry.clone());
        let seen = Arc::new(StdMutex::new(None));
        catalog.bind_executor(Arc::new(RecordingModContext(seen.clone())));
        catalog
            .register(
                "demo",
                serde_json::json!({"name":"origin","description":"Inspect"}),
            )
            .await
            .unwrap();
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new()
            .bind(gate, driver)
            .bind_router(Arc::new(ModContextRouter(
                command_api::RegistrySlashDispatcher::new(registry),
            )));
        connection
            .dispatch(ClientCommand::RunSlashCommand {
                raw: "/origin".into(),
                turn_id: None,
            })
            .await;
        let context = seen.lock().unwrap().clone().expect("Mod handler ran");
        assert_eq!(context.origin, serde_json::json!({"kind":"bridge"}));
        assert!(!context.is_fullscreen);
        assert_eq!(context.columns, 80);
    }

    #[tokio::test]
    async fn bridge_transition_can_await_its_own_mod_command_without_deadlock() {
        let captured = Arc::new(Mutex::new(None));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: Arc::new(Notify::new()),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new();
        let settled = Arc::new(Mutex::new(None));
        let router = Arc::new(ReentrantModRouter {
            queue: connection.mod_command_queue(),
            settled: settled.clone(),
            queued_result: lingxi_core::host::SlashDispatchResult::Handled {
                display: "nested".into(),
            },
        });
        let connection = connection
            .bind(gate, driver)
            .bind_router(router)
            .with_queue_wakeup();
        connection.handshaken.store(true, Ordering::SeqCst);

        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connection.dispatch(ClientCommand::ClearSession),
        )
        .await
        .expect("ClearSession must settle its nested Mod command");
        assert_eq!(
            settled.lock().await.take().unwrap().unwrap(),
            serde_json::json!({"text":"nested"})
        );
        assert!(!connection.transition_active.load(Ordering::SeqCst));

        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connection.dispatch(ClientCommand::RunSlashCommand {
                raw: "/clear".into(),
                turn_id: None,
            }),
        )
        .await
        .expect("/clear must settle its nested Mod command");
        assert_eq!(
            settled.lock().await.take().unwrap().unwrap(),
            serde_json::json!({"text":"nested"})
        );
        assert!(!connection.transition_active.load(Ordering::SeqCst));
        assert!(captured.lock().await.is_none());
    }

    #[tokio::test]
    async fn bridge_transition_can_await_plugin_prompt_command_without_deadlock() {
        let captured = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: notify.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new();
        let settled = Arc::new(Mutex::new(None));
        let router = Arc::new(ReentrantModRouter {
            queue: connection.mod_command_queue(),
            settled: settled.clone(),
            queued_result: lingxi_core::host::SlashDispatchResult::RunAsTurn {
                prompt: "expanded plugin prompt".into(),
            },
        });
        let connection = connection
            .bind(gate, driver)
            .bind_router(router)
            .with_queue_wakeup();
        connection.handshaken.store(true, Ordering::SeqCst);

        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connection.dispatch(ClientCommand::ClearSession),
        )
        .await
        .expect("prompt command must settle under a parent transition");
        assert_eq!(
            settled.lock().await.take().unwrap().unwrap(),
            serde_json::json!({})
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), notify.notified())
            .await
            .expect("queued model turn must run after command admission");
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while connection.active_turn.is_active() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued model turn must retire its owner");
        assert_eq!(
            captured
                .lock()
                .await
                .as_ref()
                .map(|(prompt, _)| prompt.as_str()),
            Some("expanded plugin prompt")
        );
        assert!(!connection.active_turn.is_active());
        assert!(!connection.transition_active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn bridge_plugin_prompt_settles_before_its_model_turn_finishes() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(BlockingQueuedDriver {
            started: started.clone(),
            release: release.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new();
        let settled = Arc::new(Mutex::new(None));
        let router = Arc::new(ReentrantModRouter {
            queue: connection.mod_command_queue(),
            settled: settled.clone(),
            queued_result: lingxi_core::host::SlashDispatchResult::RunAsTurn {
                prompt: "expanded plugin prompt".into(),
            },
        });
        let connection = connection
            .bind(gate, driver)
            .bind_router(router)
            .with_queue_wakeup();
        connection.handshaken.store(true, Ordering::SeqCst);

        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connection.dispatch(ClientCommand::ClearSession),
        )
        .await
        .expect("command admission must leave the socket loop free");
        assert_eq!(
            settled.lock().await.take().unwrap().unwrap(),
            serde_json::json!({})
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
            .await
            .expect("model turn must start separately");
        assert!(connection.active_turn.is_active());
        assert!(connection.turn_running.load(Ordering::SeqCst));
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while connection.active_turn.is_active()
                || connection.turn_running.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued turn must release its owner");
    }

    /// A `RunSlashCommand` whose dispatcher yields `RunAsTurn` (a `type: "prompt"`
    /// command like `/loop`) must be fed through the SAME enqueue-or-spawn turn
    /// path as a `SendPrompt` — driving the bound `TurnDriver` with the EXPANDED
    /// prompt — and must NOT take the display-only `route` fallback.
    #[tokio::test]
    async fn run_slash_command_run_as_turn_drives_turn() {
        let captured = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: notify.clone(),
        });
        let routed = Arc::new(AtomicBool::new(false));
        let router: Arc<dyn crate::router::CommandRouter> = Arc::new(StubRouter {
            result: lingxi_core::host::SlashDispatchResult::RunAsTurn {
                prompt: "EXPANDED /loop prompt".to_string(),
            },
            routed: routed.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new()
            .bind(gate, driver)
            .bind_router(router);

        connection
            .dispatch(ClientCommand::RunSlashCommand {
                raw: "/loop 5m /babysit-prs".to_string(),
                turn_id: Some(41),
            })
            .await;

        // `dispatch` SPAWNS the turn; wait for the recording driver to fire.
        notify.notified().await;
        let got = captured.lock().await.clone().expect("driver must run");
        assert_eq!(got.0, "EXPANDED /loop prompt");
        assert!(
            !routed.load(Ordering::SeqCst),
            "a RunAsTurn result must NOT take the display-only route fallback"
        );
    }

    /// A `RunSlashCommand` whose dispatcher yields a display-only `Handled` result
    /// (a `type: "local"` command like `/help`) must surface the dispatch result's
    /// text DIRECTLY and must NOT drive a turn — and crucially must NOT re-`route()`
    /// (which would call `dispatcher.dispatch()` a SECOND time, re-running the
    /// builtin's `handle()` and any of its side effects). Reusing the already-
    /// computed `dispatch_slash` disposition is the no-double-dispatch fix.
    #[tokio::test]
    async fn run_slash_command_handled_surfaces_text_without_redispatch() {
        let captured = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify,
        });
        let routed = Arc::new(AtomicBool::new(false));
        let router: Arc<dyn crate::router::CommandRouter> = Arc::new(StubRouter {
            result: lingxi_core::host::SlashDispatchResult::Handled {
                display: "help text".to_string(),
            },
            routed: routed.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new()
            .bind(gate, driver)
            .bind_router(router);

        connection
            .dispatch(ClientCommand::RunSlashCommand {
                raw: "/help".to_string(),
                turn_id: Some(42),
            })
            .await;

        assert!(
            !routed.load(Ordering::SeqCst),
            "a display-only Handled result must NOT re-route (no double-dispatch); \
             the already-computed disposition's text is surfaced directly"
        );
        assert!(
            captured.lock().await.is_none(),
            "a display-only command must NOT drive a turn"
        );
    }

    struct CancelAwareBlockDriver {
        started: Arc<Notify>,
        cancel_observed: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl TurnDriver for CancelAwareBlockDriver {
        async fn run_turn(&self, _prompt: String) {
            panic!("connection must use the cancel-aware entry");
        }

        async fn run_turn_with_images_and_cancel(
            &self,
            _prompt: String,
            _images: Vec<ImageRefDto>,
            cancel: CancellationToken,
        ) {
            self.started.notify_one();
            cancel.cancelled().await;
            self.cancel_observed.notify_one();
            // Models an InterruptBehavior::Block tool: cancellation has been
            // observed, but the owner does not return until the safe boundary.
            self.release.notified().await;
        }
    }

    #[test]
    fn cancellation_priority_requires_an_existing_matching_turn() {
        use bridge::wire::{BridgeRequest, Frame};
        use bridge::FramePump;
        let connection = BridgeConnection::new();
        let cancel = |turn_id| {
            Frame::Request(BridgeRequest {
                id: 1,
                method: "command".into(),
                params: serde_json::to_value(ClientCommand::Cancel { turn_id }).unwrap(),
            })
        };
        // Slow SetModel -> queued SendPrompt(42) -> Cancel(42): cancellation
        // must stay behind the prompt until it has claimed the active slot.
        assert!(!connection.is_priority_frame(&cancel(Some(42))));
        assert!(!connection.is_priority_frame(&cancel(None)));
        let (generation, _) = connection.active_turn.begin(Some(41));
        assert!(!connection.is_priority_frame(&cancel(Some(42))));
        assert!(connection.is_priority_frame(&cancel(Some(41))));
        assert!(connection.is_priority_frame(&cancel(None)));
        connection.active_turn.finish(generation);
        let (generation, _) = connection.active_turn.begin(Some(42));
        assert!(connection.is_priority_frame(&cancel(Some(42))));
        connection.active_turn.finish(generation);
        assert!(!connection.is_priority_frame(&cancel(Some(42))));
    }

    #[test]
    fn audio_capability_updates_share_the_response_lane_after_handshake() {
        use bridge::wire::{BridgeRequest, Frame};
        use bridge::FramePump;

        let connection = BridgeConnection::new();
        let update = Frame::Request(BridgeRequest {
            id: 1,
            method: "command".into(),
            params: serde_json::to_value(ClientCommand::UpdateAudioCapabilities {
                capabilities: client::protocol::audio::AudioCapabilitySnapshotDto {
                    service_epoch: 8,
                    support_revision: 1,
                    supported_operations: vec![],
                    readiness: vec![],
                    max_payload_bytes: 1024,
                },
            })
            .unwrap(),
        });

        assert!(!connection.is_priority_frame(&update));
        connection.handshaken.store(true, Ordering::SeqCst);
        assert!(connection.is_priority_frame(&update));
    }

    #[tokio::test]
    async fn cancel_matches_turn_id_and_keeps_slot_until_block_owner_finishes() {
        let started = Arc::new(Notify::new());
        let cancel_observed = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(CancelAwareBlockDriver {
            started: started.clone(),
            cancel_observed: cancel_observed.clone(),
            release: release.clone(),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = BridgeConnection::new().bind(gate, driver);

        connection
            .dispatch(ClientCommand::SendPrompt {
                text: "block".to_string(),
                prompt_mode: None,
                images: Vec::new(),
                turn_id: Some(41),
            })
            .await;
        started.notified().await;

        connection
            .dispatch(ClientCommand::Cancel { turn_id: Some(99) })
            .await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                cancel_observed.notified()
            )
            .await
            .is_err(),
            "stale turn id must not cancel the active owner"
        );

        connection
            .dispatch(ClientCommand::Cancel { turn_id: Some(41) })
            .await;
        cancel_observed.notified().await;
        assert!(connection.active_turn.is_active());
        assert!(connection.turn_running.load(Ordering::SeqCst));

        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while connection.active_turn.is_active()
                || connection.turn_running.load(Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("turn slot released after Block owner completed");
    }

    #[tokio::test]
    async fn permission_registered_after_cancel_is_denied_without_waiting_for_timeout() {
        let connection = BridgeConnection::new();
        let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let connection = connection.bind(gate.clone(), driver);
        let (generation, _) = connection.active_turn.begin(Some(73));
        assert!(connection.active_turn.cancel(Some(73)));

        let decision = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            gate.check("Bash", &serde_json::json!({ "command": "echo late" })),
        )
        .await
        .expect("a post-cancel permission must not wait for the normal prompt timeout");

        assert!(matches!(decision, PermissionDecision::Deny { .. }));
        assert_eq!(gate.pending_count().await, 0);
        connection.active_turn.finish(generation);
    }

    #[tokio::test]
    async fn permission_live_status_uses_cli_reason_and_ignores_stale_resolution() {
        use super::connection_regression_test::{captured_sink, next_frame};
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().expect("live-session tempdir");
        let dir =
            lingxi_core::host::live_sessions::LiveSessionDir::at(temp.path().join("sessions"));
        let session_id = "11111111-2222-4333-8444-555555555555";
        let pid = std::process::id();
        dir.upsert_identity(pid, session_id, Some("bridge"), None, None, None)
            .unwrap();
        lingxi_core::host::live_sessions::set_process_dir(dir.clone());
        lingxi_core::host::live_sessions::set_process_session_id(session_id);

        let connection = BridgeConnection::new();
        let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let connection = connection.bind(gate.clone(), driver);
        let (endpoint, mut socket, sink) = captured_sink().await;
        assert!(connection.claim_outbound(&sink).await);
        connection.handshaken.store(true, Ordering::SeqCst);
        let (generation, _) = connection.active_turn.begin(Some(7));

        let check = tokio::spawn({
            let gate = gate.clone();
            async move {
                gate.check("Bash", &serde_json::json!({"command": "echo hi"}))
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while gate.pending_count().await == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("permission request is parked");
        assert!(matches!(
            next_frame(&mut socket).await,
            bridge::wire::Frame::PermissionRequest(_)
        ));

        let waiting = dir
            .list_live()
            .unwrap()
            .into_iter()
            .find(|record| record.sid() == session_id)
            .expect("live record");
        assert_eq!(waiting.status.as_deref(), Some("waiting"));
        assert_eq!(
            waiting.waiting_for.as_deref(),
            Some(lingxi_core::host::live_sessions::PERMISSION_PROMPT_WAITING_FOR)
        );

        connection
            .resolve_permission(1, PermissionResponseDto::AllowOnce)
            .await;
        let busy = dir
            .list_live()
            .unwrap()
            .into_iter()
            .find(|record| record.sid() == session_id)
            .expect("live record after resolve");
        assert_eq!(busy.status.as_deref(), Some("busy"));
        assert_eq!(busy.waiting_for, None);
        assert!(matches!(check.await.unwrap(), PermissionDecision::Allow));

        // A duplicate/stale approval must not turn an already-idle session
        // back to busy.
        dir.set_status(pid, "idle", None).unwrap();
        connection
            .resolve_permission(1, PermissionResponseDto::Deny)
            .await;
        let idle = dir
            .list_live()
            .unwrap()
            .into_iter()
            .find(|record| record.sid() == session_id)
            .expect("live record after stale resolve");
        assert_eq!(idle.status.as_deref(), Some("idle"));
        assert_eq!(idle.waiting_for, None);
        connection.active_turn.finish(generation);
        endpoint.shutdown().await;
    }

    /// A driver whose FIRST turn parks on a barrier until the test releases it,
    /// recording every prompt (in order) it is driven with. Lets a test prove a
    /// second `SendPrompt` arriving mid-turn does NOT spawn a concurrent turn but
    /// is enqueued and run as a follow-up once the first turn ends.
    struct GatedDriver {
        prompts: Arc<Mutex<Vec<String>>>,
        /// Released by the test to let the FIRST turn complete.
        release: Arc<Notify>,
        /// Notifies the test each time a turn STARTS.
        started: Arc<Notify>,
        /// Notifies the test each time a turn COMPLETES.
        completed: Arc<Notify>,
        first_seen: AtomicBool,
    }

    #[async_trait]
    impl TurnDriver for GatedDriver {
        async fn run_turn(&self, prompt: String) {
            self.prompts.lock().await.push(prompt);
            self.started.notify_one();
            // Only the FIRST turn parks; follow-up turns run straight through.
            if self
                .first_seen
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.release.notified().await;
            }
            self.completed.notify_one();
        }
    }

    /// Two `SendPrompt`s while a turn is in flight: the second must be ENQUEUED
    /// (not spawned concurrently) and run as a follow-up turn after the first
    /// ends — the parity twin of print.ts run()'s `running` guard + drain loop.
    #[tokio::test]
    async fn second_prompt_is_queued_and_drained_after_first_turn() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Notify::new());
        let started = Arc::new(Notify::new());
        let completed = Arc::new(Notify::new());
        let driver: Arc<dyn TurnDriver> = Arc::new(GatedDriver {
            prompts: prompts.clone(),
            release: release.clone(),
            started: started.clone(),
            completed: completed.clone(),
            first_seen: AtomicBool::new(false),
        });
        let gate = Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink)));
        let connection = Arc::new(BridgeConnection::new().bind(gate, driver));

        // First prompt — wins the run loop and parks on the barrier.
        connection
            .dispatch(ClientCommand::SendPrompt {
                text: "first".to_string(),
                prompt_mode: None,
                images: Vec::new(),
                turn_id: None,
            })
            .await;
        started.notified().await;

        // While the first turn is parked, the loop owns `turn_running`; a second
        // prompt must be ENQUEUED, not spawned. Nothing new starts.
        assert!(connection.turn_running.load(Ordering::SeqCst));
        connection
            .dispatch(ClientCommand::SendPrompt {
                text: "second".to_string(),
                prompt_mode: None,
                images: Vec::new(),
                turn_id: None,
            })
            .await;
        assert_eq!(
            connection.queue.len().await,
            1,
            "the mid-turn prompt must be queued, not concurrently spawned"
        );
        // Still only ONE turn has started.
        assert_eq!(prompts.lock().await.len(), 1);

        // Release the first turn; the loop then drains the queued prompt as a
        // follow-up turn and finally clears `turn_running`.
        release.notify_one();
        completed.notified().await; // first turn done
        started.notified().await; // follow-up turn started
        completed.notified().await; // follow-up turn done

        // Drain settles: queue empty, flag cleared, both prompts ran in order.
        // (Yield until the spawned loop finishes its post-drain bookkeeping.)
        for _ in 0..100 {
            if !connection.turn_running.load(Ordering::SeqCst) && connection.queue.is_empty().await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!connection.turn_running.load(Ordering::SeqCst));
        assert!(connection.queue.is_empty().await);
        assert_eq!(*prompts.lock().await, vec!["first", "second"]);
    }

    // ── Keepalive: drain tags the in-flight loop tick on SOURCE, not text shape ──

    fn drain_test_command(text: &str, source: super::QueueSource) -> super::QueuedCommand {
        super::QueuedCommand {
            scheduled_task_id: None,
            scheduled_fire_id: None,
            uuid: if source == super::QueueSource::Cron {
                format!("loop-wakeup-{text}")
            } else {
                format!("drain-ka-{text}")
            },
            content: super::QueuedCommandContent::UserInput {
                text: text.to_string(),
            },
            priority: super::QueuePriority::Next,
            queued_at: std::time::SystemTime::now(),
            source,
            agent_id: None,
            skip_slash_commands: false,
            is_meta: false,
        }
    }

    #[tokio::test]
    async fn session_transition_cancels_dynamic_work_and_preserves_other_queue_entries() {
        struct StopRecorder(Arc<std::sync::atomic::AtomicUsize>);
        #[async_trait]
        impl TurnDriver for StopRecorder {
            async fn run_turn(&self, _: String) {}
            async fn stop_dynamic_loop(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let stops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut connection = BridgeConnection::new();
        connection.driver = Some(Arc::new(StopRecorder(stops.clone())));
        let mut wakeup = drain_test_command("old loop", super::QueueSource::Cron);
        wakeup.uuid = "loop-wakeup-old-session".into();
        connection.queue.enqueue(wakeup).await;
        connection
            .queue
            .enqueue(drain_test_command(
                "keep user input",
                super::QueueSource::PromptInput,
            ))
            .await;
        connection.loop_runtime.begin_tick("old loop".into());
        connection.stop_loop_for_session_transition().await;
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(connection.loop_runtime.in_flight_prompt(), None);
        assert_eq!(connection.queue.len().await, 1);
        assert_eq!(
            connection.queue.dequeue().await.unwrap().text(),
            Some("keep user input")
        );
    }

    #[test]
    fn loop_runtime_is_isolated_between_connections() {
        let first = super::BridgeConnection::new();
        let second = super::BridgeConnection::new();
        first
            .loop_runtime
            .begin_tick("first connection tick".to_string());

        // A normal prompt on another connection clears only its own state.
        second.loop_runtime.take_in_flight_prompt();
        assert_eq!(
            first.loop_runtime.in_flight_prompt().as_deref(),
            Some("first connection tick")
        );
        assert_eq!(second.loop_runtime.in_flight_prompt(), None);
    }

    /// REGRESSION (verify-wf HIGH): a dynamic loop tick whose sentinel resolved to
    /// NON-slash instruction text takes the BATCHED drain branch. The drain must
    /// still tag it as the in-flight loop tick (binary `if(d.kind==="loop")
    /// I7e(d.prompt)`) so the driver's turn-end edge can arm the keepalive. The
    /// `RecordingDriver` does not consume the in-flight tag, so it remains set after
    /// the drain — proving the tag was written on the batched path.
    #[tokio::test]
    async fn drain_tags_non_slash_cron_tick_in_flight() {
        let _g = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tool_cron::reset_loop_runtime_state();
        let queue = Arc::new(super::MessageQueueManager::new());
        queue
            .enqueue(drain_test_command(
                "# Autonomous loop tick",
                super::QueueSource::Cron,
            ))
            .await;
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        super::drain_main_thread(
            &driver,
            &queue,
            &loop_runtime,
            &super::ActiveTurnControl::default(),
            &super::TurnInteractions::default(),
            &Arc::new(Mutex::new(std::collections::HashMap::new())),
            &Arc::new(Mutex::new(())),
            None,
        )
        .await;
        assert_eq!(
            loop_runtime.in_flight_prompt().as_deref(),
            Some("# Autonomous loop tick"),
            "the batched drain branch must tag a Cron tick as in-flight"
        );
        tool_cron::reset_loop_runtime_state();
    }

    #[tokio::test]
    async fn mixed_queue_batch_retains_insertion_order_and_each_origin() {
        struct BatchRecorder(Arc<Mutex<Vec<Vec<(String, bool)>>>>);
        #[async_trait]
        impl TurnDriver for BatchRecorder {
            async fn run_turn(&self, _: String) {
                panic!("expected a batch");
            }
            async fn run_queued_batch(
                &self,
                inputs: Vec<orchestrator::QueuedPromptInput>,
                _: CancellationToken,
            ) {
                self.0.lock().await.push(
                    inputs
                        .into_iter()
                        .map(|input| (input.text, input.is_meta))
                        .collect(),
                );
            }
        }
        let queue = Arc::new(super::MessageQueueManager::new());
        let mut scheduled = drain_test_command("scheduled first", super::QueueSource::Cron);
        scheduled.priority = super::QueuePriority::Later;
        scheduled.is_meta = true;
        scheduled.skip_slash_commands = true;
        queue.enqueue(scheduled).await;
        let mut human = drain_test_command("human second", super::QueueSource::PromptInput);
        human.priority = super::QueuePriority::Now;
        queue.enqueue(human).await;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let driver: Arc<dyn TurnDriver> = Arc::new(BatchRecorder(captured.clone()));
        let runtime = Arc::new(tool_cron::LoopRuntime::default());
        super::drain_main_thread(
            &driver,
            &queue,
            &runtime,
            &super::ActiveTurnControl::default(),
            &super::TurnInteractions::default(),
            &Arc::new(Mutex::new(std::collections::HashMap::new())),
            &Arc::new(Mutex::new(())),
            None,
        )
        .await;
        assert_eq!(
            *captured.lock().await,
            vec![vec![
                ("scheduled first".into(), true),
                ("human second".into(), false)
            ]]
        );
    }

    #[tokio::test]
    async fn fixed_cron_resolves_in_main_turn_without_dynamic_keepalive() {
        let queue = Arc::new(super::MessageQueueManager::new());
        let mut command =
            drain_test_command("/scheduled raw instruction", super::QueueSource::Cron);
        command.uuid = "cron-fire-01234567".into();
        command.priority = super::QueuePriority::Later;
        command.skip_slash_commands = true;
        command.is_meta = true;
        queue.enqueue(command).await;
        let captured = Arc::new(Mutex::new(None));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: Arc::new(Notify::new()),
        });
        let runtime = Arc::new(tool_cron::LoopRuntime::default());
        super::drain_main_thread(
            &driver,
            &queue,
            &runtime,
            &super::ActiveTurnControl::default(),
            &super::TurnInteractions::default(),
            &Arc::new(Mutex::new(std::collections::HashMap::new())),
            &Arc::new(Mutex::new(())),
            None,
        )
        .await;
        assert_eq!(runtime.in_flight_prompt(), None);
        assert_eq!(
            captured.lock().await.as_ref().unwrap().0,
            "/scheduled raw instruction"
        );
    }

    #[tokio::test]
    async fn drain_resolves_loop_sentinel_but_keeps_original_keepalive_identity() {
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let queue = Arc::new(super::MessageQueueManager::new());
        queue
            .enqueue(drain_test_command(
                "<<autonomous-loop-dynamic>>",
                super::QueueSource::Cron,
            ))
            .await;
        let captured = Arc::new(Mutex::new(None));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: captured.clone(),
            notify: Arc::new(Notify::new()),
        });
        let runtime = Arc::new(tool_cron::LoopRuntime::default());
        super::drain_main_thread(
            &driver,
            &queue,
            &runtime,
            &super::ActiveTurnControl::default(),
            &super::TurnInteractions::default(),
            &Arc::new(Mutex::new(std::collections::HashMap::new())),
            &Arc::new(Mutex::new(())),
            None,
        )
        .await;
        assert_eq!(
            runtime.in_flight_prompt().as_deref(),
            Some("<<autonomous-loop-dynamic>>")
        );
        let result = captured.lock().await;
        let text = &result.as_ref().unwrap().0;
        assert_ne!(text, "<<autonomous-loop-dynamic>>");
        assert!(text.contains("ScheduleWakeup"));
    }

    /// A normal user prompt (non-Cron) through the same batched drain branch must
    /// NOT leave an in-flight loop-tick tag.
    #[tokio::test]
    async fn drain_does_not_tag_user_prompt_in_flight() {
        let _g = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        tool_cron::reset_loop_runtime_state();
        let queue = Arc::new(super::MessageQueueManager::new());
        queue
            .enqueue(drain_test_command(
                "just a user prompt",
                super::QueueSource::PromptInput,
            ))
            .await;
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let loop_runtime = Arc::new(tool_cron::LoopRuntime::default());
        super::drain_main_thread(
            &driver,
            &queue,
            &loop_runtime,
            &super::ActiveTurnControl::default(),
            &super::TurnInteractions::default(),
            &Arc::new(Mutex::new(std::collections::HashMap::new())),
            &Arc::new(Mutex::new(())),
            None,
        )
        .await;
        assert_eq!(
            loop_runtime.in_flight_prompt(),
            None,
            "a user prompt must not be tagged as a loop tick"
        );
    }
}
