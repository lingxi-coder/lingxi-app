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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use crate::audio_bridge::{AudioRequestSink, AudioResponder};
use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{
    version_compatible, BridgeRequest, BridgeResponse, BridgeWireError, Capabilities, ClientHello,
    FramePump, FrameSink, ServerHello, BRIDGE_PROTOCOL_VERSION,
};
use client_adapter::{
    BridgeAskUserQuestionBroker, ClientEventSink, ComputerAccessRequestSink, PermissionRequestSink,
};
use client_protocol::commands::AudioResultDto;
use client_protocol::commands::{ClientCommand, ImageRefDto};
use client_protocol::computer_access::{ComputerAccessRequestDto, ComputerAccessResponseDto};
use client_protocol::events::{ClientEvent, ErrorKindDto};
use client_protocol::permission::{PermissionKindDto, PermissionRequest, PermissionResponseDto};
use msgqueue::{
    join_prompt_values, MessageQueueManager, QueuePriority, QueueSource, QueuedCommand,
    QueuedCommandContent, TelemetryQueueRecorder,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tui_core::ask_user_question_bridge::AskUserQuestionExchange;
use tui_core::computer_access_bridge::ComputerAccessExchange;

use client_adapter::AdapterPermissionGate;
use client_adapter::BridgeComputerAccessBroker;

use crate::router::CommandRouter;

/// The engine entry a [`BridgeConnection`] calls to drive a turn from an inbound
/// [`ClientCommand::SendPrompt`].
///
/// Abstracting the turn entry behind a trait keeps the connection loop decoupled
/// from HOW the orchestrator was built: the production server builds it via
/// `engine_desktop::build`, while the F2-06 e2e test wires a
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

    /// Drive ONE turn for `prompt` carrying the inline images pasted/attached by
    /// the client (the wire [`ImageRefDto`]s from
    /// [`ClientCommand::SendPrompt`](client_protocol::commands::ClientCommand::SendPrompt)).
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
type SharedComputerAccessBroker = Arc<StdMutex<Option<Weak<BridgeComputerAccessBroker>>>>;
type SharedAskUserQuestionBroker = Arc<StdMutex<Option<Weak<BridgeAskUserQuestionBroker>>>>;

/// A [`ClientEventSink`] that forwards each lowered [`ClientEvent`] out as a
/// [`Frame::Event`] on the connection's outbound channel.
struct FrameEventSink {
    out: SharedFrameSink,
    active_turn: ActiveTurnControl,
    ask_user_question_broker: SharedAskUserQuestionBroker,
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
            | ClientEvent::TextDelta { .. }
            | ClientEvent::AskUserQuestion { .. }
            | ClientEvent::TurnStarted { .. }
            | ClientEvent::TurnEnded { .. }
            | ClientEvent::ToolUseStarted { .. }
            | ClientEvent::ToolHeartbeat { .. }
            | ClientEvent::ToolUseResult { .. }
            | ClientEvent::MessageComplete { .. }
            | ClientEvent::CostUpdate { .. }
            | ClientEvent::CoordinatorStatus { .. }
            | ClientEvent::CoordinatorWorker { .. }
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
                    broker.cancel(request.request_id).await;
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
                    *outcome = client_protocol::events::TurnOutcomeDto::Cancelled;
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
}

#[async_trait]
impl PermissionRequestSink for FramePermissionSink {
    async fn emit_request(&self, request: PermissionRequest) {
        let request_id = request.request_id;
        if !self.active_turn.accepts_interactions() {
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
        let forwarded = self.active_turn.with_accepted_interaction(|| {
            if let Some(sink) = out.as_ref() {
                let _ = sink.send(Frame::PermissionRequest(request));
            }
        });
        drop(out);
        if !forwarded {
            self.tool_names.lock().await.remove(&request_id);
            self.reject(request_id).await;
        } else {
            platform_api::live_sessions::set_process_status(
                "waiting",
                Some(platform_api::live_sessions::PERMISSION_PROMPT_WAITING_FOR),
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
/// need, keyed by `request_id` on the [`BridgeComputerAccessBroker`] itself.
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
    tool_names: Arc<Mutex<HashMap<u64, String>>>,
    gate: Option<Arc<AdapterPermissionGate>>,
    /// The `computer`-tool `request_access` broker (Electron-facing sibling of
    /// `gate`, see [`BridgeComputerAccessBroker`]'s own doc comment). `None`
    /// when no computer-access channel was wired at boot (e.g. a test
    /// connection that never calls [`Self::bind_computer_access`]) — the two
    /// new [`ClientCommand`] variants are then silently dropped, exactly like
    /// an unrouted command with no [`CommandRouter`] bound.
    computer_access_broker: Option<Arc<BridgeComputerAccessBroker>>,
    /// Connection-scoped broker for `AskUserQuestion` UI exchanges.
    ask_user_question_broker: Option<Arc<BridgeAskUserQuestionBroker>>,
    /// The response side of the connection's [`crate::audio_bridge::AudioBridge`]
    /// (the desktop's stand-in for the mobile clients' native
    /// `SpeechToText`/`TextToSpeech`/`VoiceRecorder`). `None` when no audio
    /// bridge was wired at boot — `AudioResponse` is then silently dropped,
    /// exactly like an unrouted command with no [`CommandRouter`] bound.
    audio_responder: Option<AudioResponder>,
    permission_gate_ref: SharedPermissionGate,
    computer_access_broker_ref: SharedComputerAccessBroker,
    ask_user_question_broker_ref: SharedAskUserQuestionBroker,
    driver: Option<Arc<dyn TurnDriver>>,
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
    /// Connection-owned active turn identity and cancellation token. Keeping
    /// this outside the spawned driver closes the race where Cancel arrives
    /// after SendPrompt but before the driver registers with msgqueue.
    active_turn: ActiveTurnControl,
    /// Abort handle for the currently-owned spawned turn-drain loop. On close we
    /// abort it so stale turn work cannot survive into the next reconnect and
    /// emit onto a newly-claimed outbound sink.
    active_turn_task: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>>,
}

#[derive(Clone, Default)]
struct ActiveTurnControl {
    next_generation: Arc<AtomicU64>,
    owner: Arc<StdMutex<Option<ActiveTurnOwner>>>,
}

struct ActiveTurnOwner {
    generation: u64,
    turn_id: Option<u64>,
    cancel: CancellationToken,
    terminal: bool,
}

#[derive(Clone)]
struct TurnInteractions {
    gate: Option<Arc<AdapterPermissionGate>>,
    computer_access_broker: Option<Arc<BridgeComputerAccessBroker>>,
    ask_user_question_broker: Option<Arc<BridgeAskUserQuestionBroker>>,
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
    async fn drain(&self) {
        if let Some(gate) = self.gate.as_ref() {
            gate.drain().await;
        }
        if let Some(broker) = self.computer_access_broker.as_ref() {
            broker.drain().await;
        }
        if let Some(broker) = self.ask_user_question_broker.as_ref() {
            broker.drain().await;
        }
        self.tool_names.lock().await.clear();
    }
}

impl ActiveTurnControl {
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
            *owner = None;
        }
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

/// Drain every queued MAIN-THREAD prompt as a follow-up turn, coalescing
/// consecutive prompts into one turn (twin of `joinPromptValues` /
/// `drainCommandQueue`). Runs until no main-thread command remains.
async fn drain_main_thread(
    driver: &Arc<dyn TurnDriver>,
    queue: &Arc<MessageQueueManager>,
    loop_runtime: &Arc<tool_cron::LoopRuntime>,
    active_turn: &ActiveTurnControl,
    interactions: &TurnInteractions,
) {
    loop {
        // Snapshot the highest-priority main-thread, non-slash prompts so a run
        // of consecutive prompts merges into a single follow-up turn.
        let batch = queue
            .get_by_max_priority(QueuePriority::Later, |c| {
                c.is_main_thread() && !c.is_slash_command()
            })
            .await;
        let Some((joined, consumed)) = join_prompt_values(&batch) else {
            // No batchable (non-slash) prompt left. Pop the next main-thread
            // command and, if it carries prompt text (e.g. a slash command typed
            // mid-turn), run it as its own follow-up turn — IDENTICAL to how the
            // idle SendPrompt path handles the same input — so it is never
            // silently dropped. Text-less commands (bare notifications) are just
            // consumed. (claude-code keeps queued slash commands and routes them
            // post-turn; running it here is the bridge's faithful equivalent
            // since the idle path also runs slash text through run_turn.)
            match queue.dequeue_main_thread().await {
                Some(cmd) => {
                    if let Some(t) = cmd.text() {
                        if !t.is_empty() {
                            tag_loop_tick_in_flight(
                                loop_runtime,
                                cmd.source == QueueSource::Cron,
                                t,
                            );
                            let (generation, cancel) = active_turn.begin(None);
                            driver.run_turn_with_cancel(t.to_string(), cancel).await;
                            interactions.drain().await;
                            active_turn.finish(generation);
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
            .find(|c| c.source == QueueSource::Cron && consumed.contains(&c.uuid))
            .and_then(|c| c.text().map(str::to_string));
        match cron_tick {
            Some(ref t) => tag_loop_tick_in_flight(loop_runtime, true, t),
            None => tag_loop_tick_in_flight(loop_runtime, false, &joined),
        }
        queue
            .consume(&consumed, "drained into follow-up turn")
            .await;
        let (generation, cancel) = active_turn.begin(None);
        driver.run_turn_with_cancel(joined, cancel).await;
        interactions.drain().await;
        active_turn.finish(generation);
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
            tool_names: Arc::new(Mutex::new(HashMap::new())),
            gate: None,
            computer_access_broker: None,
            ask_user_question_broker: None,
            audio_responder: None,
            permission_gate_ref: Arc::new(StdMutex::new(None)),
            computer_access_broker_ref: Arc::new(StdMutex::new(None)),
            ask_user_question_broker_ref: Arc::new(StdMutex::new(None)),
            driver: None,
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
            loop_runtime: Arc::new(tool_cron::LoopRuntime::default()),
            turn_running: Arc::new(AtomicBool::new(false)),
            turn_handoff: Arc::new(tokio::sync::Mutex::new(())),
            active_turn: ActiveTurnControl::default(),
            active_turn_task: Arc::new(StdMutex::new(None)),
        }
    }

    /// The connection-scoped [`ClientEventSink`] to bind into the orchestrator's
    /// [`client_adapter::AdapterOutputStream`]. Every streamed turn event flows
    /// through here as a [`Frame::Event`].
    #[must_use]
    pub fn event_sink(&self) -> Arc<dyn ClientEventSink> {
        Arc::new(FrameEventSink {
            out: self.out.clone(),
            active_turn: self.active_turn.clone(),
            ask_user_question_broker: self.ask_user_question_broker_ref.clone(),
        })
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
        })
    }

    /// The connection-scoped [`ComputerAccessRequestSink`] to bind into a
    /// [`BridgeComputerAccessBroker`]. Every `request_access` exchange the
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
        self.gate = Some(gate);
        self.driver = Some(driver);
        self
    }

    /// The connection-scoped [`AudioRequestSink`] to build an
    /// [`crate::audio_bridge::AudioBridge`] over. Every audio trait call the
    /// engine makes flows through here as a
    /// [`Frame::Event`]`(`[`ClientEvent::AudioRequest`]`)`.
    #[must_use]
    pub fn audio_sink(&self) -> Arc<dyn AudioRequestSink> {
        Arc::new(FrameAudioSink {
            out: self.out.clone(),
        })
    }

    /// Attach the response side of the connection's audio bridge, so an inbound
    /// `AudioResponse` resolves the parked trait call and a disconnect drains
    /// every request still parked. Additive over [`Self::bind`]: a connection
    /// built without this call (e.g. most existing tests) simply never receives
    /// an audio bridge, and `AudioResponse` is a no-op.
    ///
    /// Unlike [`Self::bind_computer_access`] there is no receive loop to spawn:
    /// the audio traits ARE the request source, so the bridge emits directly
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
        broker: Arc<BridgeComputerAccessBroker>,
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
        broker: Arc<BridgeAskUserQuestionBroker>,
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
    pub fn computer_access_broker_handle(&self) -> Arc<BridgeComputerAccessBroker> {
        self.computer_access_broker
            .clone()
            .expect("computer_access_broker_handle called before bind_computer_access()")
    }

    /// Claim the single active-client slot, or confirm that `out` belongs to the
    /// client that already owns it.
    async fn claim_outbound(&self, out: &FrameSink) -> bool {
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
        }
    }

    /// Route one decoded [`ClientCommand`].
    async fn dispatch(&self, command: ClientCommand) {
        match command {
            ClientCommand::SendPrompt {
                text,
                images,
                turn_id,
                ..
            } => {
                self.handle_send_prompt(text, images, turn_id).await;
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
            ClientCommand::AudioResponse { request_id, result } => {
                self.resolve_audio(request_id, result).await;
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
                let _handoff = if is_compact {
                    Some(self.turn_handoff.lock().await)
                } else {
                    None
                };
                if is_compact && self.turn_running.load(Ordering::SeqCst) {
                    self.unscoped_event_sink()
                        .emit(ClientEvent::SlashCommandResult {
                            turn_id,
                            display: "cannot compact the session while a turn is in flight".into(),
                            is_error: true,
                        })
                        .await;
                    return;
                }
                let outcome = match self.router.as_ref() {
                    Some(router) => router.dispatch_slash(&raw).await,
                    None => None,
                };
                let authority_events = outcome
                    .as_ref()
                    .map(|outcome| outcome.authority_events.clone())
                    .unwrap_or_default();
                match outcome.map(|outcome| outcome.result) {
                    Some(platform_api::SlashDispatchResult::RunAsTurn { prompt }) => {
                        // Run the expanded prompt exactly like a direct user
                        // prompt (enqueue-or-spawn; no images).
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
                    Some(platform_api::SlashDispatchResult::Handled { display }) => {
                        self.unscoped_event_sink()
                            .emit(ClientEvent::SlashCommandResult {
                                turn_id,
                                display,
                                is_error: false,
                            })
                            .await;
                    }
                    Some(platform_api::SlashDispatchResult::Unknown { display, .. }) => {
                        self.unscoped_event_sink()
                            .emit(ClientEvent::SlashCommandResult {
                                turn_id,
                                display,
                                is_error: true,
                            })
                            .await;
                    }
                    Some(platform_api::SlashDispatchResult::NotASlashCommand) => {
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
    /// Follow-up turns queued mid-flight are text-only (images ride only on the
    /// directly-dispatched seed prompt) — matching claude-code, where a queued
    /// command's images are carried as `pastedContents`/`ContentBlockParam[]`
    /// but the bridge run loop here drives the text-only `run_turn` entry.
    async fn handle_send_prompt(
        &self,
        text: String,
        images: Vec<ImageRefDto>,
        turn_id: Option<u64>,
    ) {
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
            self.queue.enqueue(prompt_command(text)).await;
            return;
        }

        let queue = self.queue.clone();
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
        let (seed_generation, seed_cancel) = active_turn.begin(turn_id);
        let task = tokio::spawn(async move {
            // Seed turn — the prompt that won the loop (carries its images).
            driver
                .run_turn_with_images_and_cancel(text, images, seed_cancel)
                .await;
            interactions.drain().await;
            active_turn.finish(seed_generation);

            // Between-turn drain: run queued main-thread prompts as follow-up
            // turns until the queue is empty. Re-check after clearing the flag to
            // close the race where a prompt enqueues between the empty-check and
            // the flag clear (twin of print.ts recheckCommandQueue).
            loop {
                drain_main_thread(&driver, &queue, &loop_runtime, &active_turn, &interactions)
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
        if !self.active_turn.cancel(turn_id) {
            tracing::debug!(?turn_id, "bridge-server: ignored stale or idle turn cancel");
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
        .drain()
        .await;
    }

    /// Resolve a parked permission request on the gate (the WS read task side of
    /// the inverted handshake). Looks the recorded tool name back up so an
    /// `AllowAlways` can append the right session rule.
    async fn resolve_permission(&self, request_id: u64, response: PermissionResponseDto) {
        let Some(gate) = self.gate.as_ref() else {
            return;
        };
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
        if resolved && self.active_turn.accepts_interactions() {
            platform_api::live_sessions::set_process_status("busy", None);
        }
        if !resolved {
            tracing::debug!(
                request_id,
                "bridge-server: resolve for unknown / already-resolved permission id"
            );
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
    async fn resolve_audio(&self, request_id: u64, result: AudioResultDto) {
        let Some(responder) = self.audio_responder.as_ref() else {
            return;
        };
        let resolved = responder.resolve(request_id, result).await;
        if !resolved {
            // Unknown, already resolved, or already past its deadline. A safe
            // no-op: the caller has been given an answer either way, and a
            // client is allowed to answer a request we stopped waiting for.
            tracing::debug!(
                request_id,
                "bridge-server: response for unknown / already-resolved audio id"
            );
        }
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
            Frame::Request(BridgeRequest { id, params, .. }) => {
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
    async fn abort_active_turn_task(&self) {
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

    async fn close_connection(&self, closing: Option<&FrameSink>) {
        let mut active = self.out.lock().await;
        if closing.is_some_and(|sink| {
            active
                .as_ref()
                .is_none_or(|current| !current.same_channel(sink))
        }) {
            return;
        }
        *active = None;
        drop(active);
        self.handshaken.store(false, Ordering::SeqCst);
        self.handshake_refused.store(false, Ordering::SeqCst);
        self.abort_active_turn_task().await;
        self.turn_running.store(false, Ordering::SeqCst);
        self.active_turn.clear();
        self.queue.clear_active_turn().await;
        self.queue.clear().await;
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
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use client_adapter::{AdapterPermissionGate, PermissionRequestSink};
    use client_protocol::commands::{ClientCommand, ImageRefDto};
    use client_protocol::events::ClientEvent;
    use client_protocol::permission::{PermissionRequest, PermissionResponseDto};
    use platform_api::{PermissionDecision, PermissionGate};
    use tokio::sync::{Mutex, Notify};
    use tokio_util::sync::CancellationToken;

    use super::{is_owned_turn_event, ActiveTurnControl, BridgeConnection, TurnDriver};

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
        assert!(is_owned_turn_event(&ClientEvent::Attachment {
            attachment: client_protocol::events::AttachmentDto::NestedMemory {
                display_path: "late".to_string(),
            },
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

    /// `bind` requires a gate; this sink drops every request (the dispatch path
    /// under test never emits one).
    struct NoopPermissionSink;

    #[async_trait]
    impl PermissionRequestSink for NoopPermissionSink {
        async fn emit_request(&self, _request: PermissionRequest) {}
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
        result: platform_api::SlashDispatchResult,
        routed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl crate::router::CommandRouter for StubRouter {
        async fn route(
            &self,
            _command: ClientCommand,
            _sink: Arc<dyn client_adapter::ClientEventSink>,
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
            result: platform_api::SlashDispatchResult::RunAsTurn {
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
            result: platform_api::SlashDispatchResult::Handled {
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
        let _serial = crate::driver::LOOP_KA_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = tempfile::tempdir().expect("live-session tempdir");
        let dir = platform_api::live_sessions::LiveSessionDir::at(temp.path().join("sessions"));
        let session_id = "11111111-2222-4333-8444-555555555555";
        let pid = std::process::id();
        dir.upsert_identity(pid, session_id, Some("bridge"), None, None, None)
            .unwrap();
        platform_api::live_sessions::set_process_dir(dir.clone());
        platform_api::live_sessions::set_process_session_id(session_id);

        let connection = BridgeConnection::new();
        let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
        let driver: Arc<dyn TurnDriver> = Arc::new(RecordingDriver {
            captured: Arc::new(Mutex::new(None)),
            notify: Arc::new(Notify::new()),
        });
        let connection = connection.bind(gate.clone(), driver);
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

        let waiting = dir
            .list_live()
            .unwrap()
            .into_iter()
            .find(|record| record.sid() == session_id)
            .expect("live record");
        assert_eq!(waiting.status.as_deref(), Some("waiting"));
        assert_eq!(
            waiting.waiting_for.as_deref(),
            Some(platform_api::live_sessions::PERMISSION_PROMPT_WAITING_FOR)
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
            uuid: format!("drain-ka-{text}"),
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
        )
        .await;
        assert_eq!(
            loop_runtime.in_flight_prompt().as_deref(),
            Some("# Autonomous loop tick"),
            "the batched drain branch must tag a Cron tick as in-flight"
        );
        tool_cron::reset_loop_runtime_state();
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
        )
        .await;
        assert_eq!(
            loop_runtime.in_flight_prompt(),
            None,
            "a user prompt must not be tagged as a loop tick"
        );
    }
}
