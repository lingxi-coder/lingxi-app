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
use std::sync::atomic::{AtomicBool, Ordering};
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
use client_protocol::permission::{
    PermissionKindDto, PermissionRequest, PermissionResponseDto,
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
            self.tool_names.lock().await.insert(request.request_id, name);
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
}

impl Default for BridgeConnection {
    fn default() -> Self {
        Self::new()
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

    /// A clone of the gate handle, for tests that need to observe the parked /
    /// drained request count directly.
    #[must_use]
    pub fn gate_handle(&self) -> Arc<AdapterPermissionGate> {
        self.gate
            .clone()
            .expect("gate_handle called before bind()")
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
                    server_caps.client_protocol_version,
                    hello.capabilities.client_protocol_version,
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
                // Spawn the turn so `on_frame` returns promptly — the read loop
                // must stay free to service the approval that unblocks a parked
                // permission `check()`.
                //
                // MULTIMODAL.1: forward the inline images so the desktop bridge no
                // longer silently drops pasted/attached attachments. When `images`
                // is empty `run_turn_with_images` is identical to the old
                // `run_turn(text)` path (the override and the trait default both
                // degrade to text-only with no images).
                if let Some(driver) = self.driver.clone() {
                    tokio::spawn(async move {
                        driver.run_turn_with_images(text, images).await;
                    });
                }
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
            }) if method == "hello" => {
                match serde_json::from_value::<ClientHello>(params) {
                    Ok(hello) => self.handle_hello(id, hello).await,
                    Err(e) => {
                        tracing::debug!(error = %e, "bridge-server: undecodable ClientHello");
                    }
                }
            }
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
                tracing::debug!(drained, "bridge-server: drained parked permissions on close");
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
