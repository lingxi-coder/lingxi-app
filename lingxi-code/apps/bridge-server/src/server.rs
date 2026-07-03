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
use std::sync::Arc;

use async_trait::async_trait;
use bridge::wire::Frame;
use bridge::{
    version_compatible, BridgeRequest, BridgeResponse, BridgeWireError, Capabilities, ClientHello,
    FramePump, FrameSink, ServerHello, BRIDGE_PROTOCOL_VERSION,
};
use client_adapter::{ClientEventSink, PermissionRequestSink};
use client_protocol::commands::{ClientCommand, ImageRefDto};
use client_protocol::events::ClientEvent;
use client_protocol::permission::{PermissionKindDto, PermissionRequest, PermissionResponseDto};
use msgqueue::{
    join_prompt_values, MessageQueueManager, QueuePriority, QueueSource, QueuedCommand,
    QueuedCommandContent, TelemetryQueueRecorder,
};
use tokio::sync::Mutex;

use client_adapter::AdapterPermissionGate;

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
}

/// Connection-scoped outbound channel. Holds the per-connection [`FrameSink`]
/// once the transport hands it to us (on the first inbound frame). Before that
/// it is empty and emits are dropped (no client is listening yet); after the
/// connection closes the underlying mpsc is gone and `FrameSink::send` is a
/// silent no-op.
type SharedFrameSink = Arc<Mutex<Option<FrameSink>>>;

/// A [`ClientEventSink`] that forwards each lowered [`ClientEvent`] out as a
/// [`Frame::Event`] on the connection's outbound channel.
struct FrameEventSink {
    out: SharedFrameSink,
}

#[async_trait]
impl ClientEventSink for FrameEventSink {
    async fn emit(&self, event: ClientEvent) {
        if let Some(sink) = self.out.lock().await.as_ref() {
            let _ = sink.send(Frame::Event(event));
        }
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
}

#[async_trait]
impl PermissionRequestSink for FramePermissionSink {
    async fn emit_request(&self, request: PermissionRequest) {
        // Record the tool name for the eventual resolve.
        let tool_name = match &request.kind {
            PermissionKindDto::ToolUseConfirm { tool_name, .. } => Some(tool_name.clone()),
            // Reserved kinds (ExitPlanMode / BypassPermissionsMode) are never
            // live-sourced in the foundation (decision §0.6); no tool name.
            _ => None,
        };
        if let Some(name) = tool_name {
            self.tool_names
                .lock()
                .await
                .insert(request.request_id, name);
        }
        if let Some(sink) = self.out.lock().await.as_ref() {
            let _ = sink.send(Frame::PermissionRequest(request));
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
    driver: Option<Arc<dyn TurnDriver>>,
    /// The full command-routing seam (F2-08). When bound, every
    /// non-turn/non-permission [`ClientCommand`] (model, listings, slash, tasks,
    /// session control) is delegated here, with replies pushed out through the
    /// connection's [`ClientEventSink`]. `None` in the F2-05/06/07 skeleton (only
    /// the turn + permission path was routed there).
    router: Option<Arc<dyn CommandRouter>>,
    /// Set once the opening `hello` handshake is ACCEPTED (compatible versions).
    handshaken: Arc<AtomicBool>,
    /// Set once a `hello` handshake is REFUSED (a breaking-version mismatch,
    /// governing decision §0.10 / F2-07). A refused connection's subsequent
    /// commands are dropped — a peer disagreeing on a breaking version never
    /// drives the engine. (Distinct from "no `hello` yet": the F2-05/F2-06
    /// single-client skeleton predates a mandatory handshake, so a client that
    /// never sent a `hello` is the trusted local child and still routes.)
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
    /// Whether the single turn-drain loop is currently running. Twin of
    /// print.ts run()'s `running` flag (L1866): the first `SendPrompt` that wins
    /// this flag OWNS the drain loop; concurrent prompts enqueue and the owner
    /// drains them before clearing the flag.
    turn_running: Arc<AtomicBool>,
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
async fn drain_main_thread(driver: &Arc<dyn TurnDriver>, queue: &Arc<MessageQueueManager>) {
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
                            tag_loop_tick_in_flight(cmd.source == QueueSource::Cron, t);
                            driver.run_turn(t.to_string()).await;
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
            Some(ref t) => tag_loop_tick_in_flight(true, t),
            None => tag_loop_tick_in_flight(false, &joined),
        }
        queue.remove(&consumed, "drained into follow-up turn").await;
        driver.run_turn(joined).await;
    }
}

/// Record (or clear) the in-flight `/loop` tick so the driver's turn-completion
/// edge can arm the keepalive fallback. A `QueueSource::Cron` command IS a loop
/// tick (binary `d.kind==="loop"`); any other turn clears a stale tag so a user
/// turn never inherits one.
fn tag_loop_tick_in_flight(is_cron: bool, text: &str) {
    if is_cron {
        tool_cron::begin_loop_tick(text.to_string());
    } else {
        tool_cron::take_loop_tick_in_flight_prompt();
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
            turn_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The connection-scoped [`ClientEventSink`] to bind into the orchestrator's
    /// [`client_adapter::AdapterOutputStream`]. Every streamed turn event flows
    /// through here as a [`Frame::Event`].
    #[must_use]
    pub fn event_sink(&self) -> Arc<dyn ClientEventSink> {
        Arc::new(FrameEventSink {
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
        })
    }

    /// Attach the connection's permission gate (whose `resolve` the read task
    /// calls on an inbound approval) and the [`TurnDriver`] that drives
    /// `SendPrompt`. Yields the fully-bound pump.
    #[must_use]
    pub fn bind(mut self, gate: Arc<AdapterPermissionGate>, driver: Arc<dyn TurnDriver>) -> Self {
        self.gate = Some(gate);
        self.driver = Some(driver);
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

    /// A clone of the gate handle, for tests that need to observe the parked /
    /// drained request count directly.
    #[must_use]
    pub fn gate_handle(&self) -> Arc<AdapterPermissionGate> {
        self.gate.clone().expect("gate_handle called before bind()")
    }

    /// Ensure the connection's outbound cell points at the live [`FrameSink`].
    /// Idempotent: the transport hands a fresh `FrameSink` clone on every
    /// `on_frame`, but they all feed the same per-connection write task, so we
    /// only need to populate the cell once (and re-populating with a clone is
    /// harmless).
    async fn ensure_outbound(&self, out: &FrameSink) {
        let mut cell = self.out.lock().await;
        if cell.is_none() {
            *cell = Some(out.clone());
        }
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
            ClientCommand::SendPrompt { text, images, .. } => {
                self.handle_send_prompt(text, images).await;
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
            // A slash command may be a `type: "prompt"` command (`/loop`,
            // Markdown/Plugin): claude-code injects its expanded prompt as the
            // user turn. Pre-dispatch via the router's dispatcher (which the
            // connection cannot reach otherwise) so a `RunAsTurn` result is fed
            // through the SAME enqueue-or-spawn turn path as a `SendPrompt` (the
            // connection owns the driver + queue + turn-running flag). Display-
            // only / unknown / no-dispatcher cases fall back to the router's
            // text-surface path.
            ClientCommand::RunSlashCommand { raw } => {
                let disposition = match self.router.as_ref() {
                    Some(router) => router.dispatch_slash(&raw).await,
                    None => None,
                };
                match disposition {
                    Some(traits::SlashDispatchResult::RunAsTurn { prompt }) => {
                        // Run the expanded prompt exactly like a direct user
                        // prompt (enqueue-or-spawn; no images).
                        self.handle_send_prompt(prompt, Vec::new()).await;
                    }
                    // Display-only / unknown result: surface the SAME dispatch
                    // result's text directly. We must NOT re-`route()` here —
                    // `route(RunSlashCommand)` calls `dispatcher.dispatch()` a
                    // SECOND time (router.rs), which would re-run the builtin's
                    // `handle()` (and any of its `EmitEffects`/`InjectMessage`
                    // side effects) twice and discard the first result. Reuse
                    // the already-computed disposition instead.
                    Some(traits::SlashDispatchResult::Handled { display })
                    | Some(traits::SlashDispatchResult::Unknown { display, .. }) => {
                        self.event_sink()
                            .emit(ClientEvent::TextDelta { text: display })
                            .await;
                    }
                    Some(traits::SlashDispatchResult::NotASlashCommand) => {
                        self.event_sink()
                            .emit(ClientEvent::TextDelta {
                                text: format!("not a slash command: {raw}"),
                            })
                            .await;
                    }
                    // No dispatcher wired: delegate to the router's text-surface
                    // path, which emits the "no slash-command dispatcher wired"
                    // error (unchanged behavior). It dispatches at most once.
                    None => {
                        if let Some(router) = self.router.clone() {
                            router
                                .route(ClientCommand::RunSlashCommand { raw }, self.event_sink())
                                .await;
                        }
                    }
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
    async fn handle_send_prompt(&self, text: String, images: Vec<ImageRefDto>) {
        let Some(driver) = self.driver.clone() else {
            return;
        };

        // Try to win the run-loop ownership. compare_exchange fails if a turn is
        // already running, in which case we enqueue instead of spawning.
        if self
            .turn_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.queue.enqueue(prompt_command(text)).await;
            return;
        }

        let queue = self.queue.clone();
        let turn_running = self.turn_running.clone();
        tokio::spawn(async move {
            // Seed turn — the prompt that won the loop (carries its images).
            driver.run_turn_with_images(text, images).await;

            // Between-turn drain: run queued main-thread prompts as follow-up
            // turns until the queue is empty. Re-check after clearing the flag to
            // close the race where a prompt enqueues between the empty-check and
            // the flag clear (twin of print.ts recheckCommandQueue).
            loop {
                drain_main_thread(&driver, &queue).await;
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
        if !resolved {
            tracing::debug!(
                request_id,
                "bridge-server: resolve for unknown / already-resolved permission id"
            );
        }
    }
}

#[async_trait]
impl FramePump for BridgeConnection {
    async fn on_frame(&self, frame: Frame, out: FrameSink) {
        self.ensure_outbound(&out).await;
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
                }
            },
            Frame::Request(BridgeRequest { params, .. }) => {
                // A peer that disagreed on a breaking version was REFUSED at the
                // handshake (`handshake_refused` set); drop its commands so a
                // mismatched client can never drive the engine (the load-bearing
                // F2-07 guarantee, governing decision §0.10). (The F2-05/F2-06
                // single-client skeleton predates a mandatory handshake: a client
                // that never sent a `hello` is the trusted local Electron child and
                // still routes — the version guard only ENGAGES once a `hello`
                // arrives and is refused.)
                if self.handshake_refused.load(Ordering::SeqCst) {
                    tracing::debug!(
                        "bridge-server: dropping command on a version-refused connection"
                    );
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
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use client_adapter::{AdapterPermissionGate, PermissionRequestSink};
    use client_protocol::commands::{ClientCommand, ImageRefDto};
    use client_protocol::permission::PermissionRequest;
    use tokio::sync::{Mutex, Notify};

    use super::{BridgeConnection, TurnDriver};

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
        result: traits::SlashDispatchResult,
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
        async fn dispatch_slash(&self, _raw: &str) -> Option<traits::SlashDispatchResult> {
            Some(self.result.clone())
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
            result: traits::SlashDispatchResult::RunAsTurn {
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
            result: traits::SlashDispatchResult::Handled {
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
        super::drain_main_thread(&driver, &queue).await;
        assert_eq!(
            tool_cron::loop_tick_in_flight_prompt().as_deref(),
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
        super::drain_main_thread(&driver, &queue).await;
        assert_eq!(
            tool_cron::loop_tick_in_flight_prompt(),
            None,
            "a user prompt must not be tagged as a loop tick"
        );
    }
}
