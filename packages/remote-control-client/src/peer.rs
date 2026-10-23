//! The WebRTC seam. The real implementation (over `lingxi-webrtc-core`'s `RtcSession`
//! in data-only mode) is wired in by the app layer; this crate only needs what is
//! described here, and tests use the in-memory fake in [`crate::testing`].
//!
//! The mapping onto `lingxi_webrtc_core::RtcSession` is one to one:
//!
//! | here                              | `lingxi-webrtc-core`                                                        |
//! |-----------------------------------|-----------------------------------------------------------------------------|
//! | [`PeerConnector::create_offerer`] | `RtcSession::with_config(SessionConfig::data_only(IceConfig{servers, relay_only: false}))`, `gather_timeout = Some(config.gather_timeout)` |
//! | [`PeerHandle::offer_sdp`]         | `offer()` (bounded gathering, complete SDP)                                 |
//! | [`PeerHandle::apply_answer`]      | `apply_answer(sdp)`; `AnswerOutcome::{Ready, TimedOut}` is [`PeerAnswer`]   |
//! | [`PeerHandle::send`]              | `send_bytes(&[u8])` (at most 16 KiB; awaits buffer space)                   |
//! | [`PeerEvent::Message`]            | `receive()` (a pump task owns the `&mut` session part)                      |
//! | [`PeerEvent::Open`]               | `subscribe_ready()` turning `true`                                          |
//! | [`PeerEvent::State`]              | `subscribe_connection_state()`                                              |
//! | [`PeerEvent::Closed`]             | `receive()` returning `None` (`receive_overflowed()` says `receive_overflow`), `close()` |
//! | [`PeerHandle::close`]             | `close()` (async there: the adapter spawns it)                              |
//!
//! `apply_answer` of the real session waits for the channel to open (at most 15 s),
//! so it is awaited together with the [`PeerEvent::Open`] event and bounded by the
//! same 15 s.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

/// What a viewer asks of a new peer connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConfig {
    /// ICE servers (STUN `urls`, no TURN in v1).
    pub stun_urls: Vec<String>,
    /// Label of the reliable, ordered DataChannel to create (`lingxi-agent`).
    pub channel_label: String,
    /// Longest wait for ICE candidates before offering with what has arrived.
    pub gather_timeout: Duration,
}

/// State of the underlying connection, as WebRTC reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerConnectionState {
    /// Created.
    New,
    /// Negotiating.
    Connecting,
    /// Connected.
    Connected,
    /// Temporarily lost; may recover.
    Disconnected,
    /// ICE gave up.
    Failed,
    /// Closed.
    Closed,
}

/// What a peer reports, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    /// The DataChannel is open.
    Open,
    /// One binary DataChannel message (one fragment). Adapters must not pass text
    /// messages through: they end the peer with `Closed("protocol_error")`.
    Message(Vec<u8>),
    /// The connection state changed.
    State(PeerConnectionState),
    /// The DataChannel or connection ended, whoever ended it. Always the last event.
    Closed(String),
}

/// How applying the answer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAnswer {
    /// The DataChannel is open.
    Ready,
    /// The channel did not open in time.
    TimedOut,
}

/// A WebRTC failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PeerError(pub String);

/// Creates data-only offerer peer connections.
#[async_trait]
pub trait PeerConnector: Send + Sync {
    /// Build a data-only peer connection (no audio or video) with the given STUN
    /// servers, as the offerer, with the DataChannel created and events flowing.
    async fn create_offerer(&self, config: PeerConfig) -> Result<PeerOfferer, PeerError>;
}

/// A new offerer and its event stream.
pub struct PeerOfferer {
    /// The control surface.
    pub peer: Arc<dyn PeerHandle>,
    /// Events, starting with whatever happens during negotiation.
    pub events: mpsc::UnboundedReceiver<PeerEvent>,
}

impl std::fmt::Debug for PeerOfferer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerOfferer").finish_non_exhaustive()
    }
}

/// One peer connection.
#[async_trait]
pub trait PeerHandle: Send + Sync {
    /// Create the offer, set it locally and wait (at most the configured gather
    /// timeout) for ICE gathering; returns the complete SDP with its candidates.
    async fn offer_sdp(&self) -> Result<String, PeerError>;

    /// Apply the host's answer SDP and wait (at most 15 s) for the DataChannel to
    /// open, like `RtcSession::apply_answer`. An adapter whose session returns
    /// earlier may return [`PeerAnswer::Ready`] at once; [`PeerEvent::Open`] is what
    /// the connection finally waits for.
    async fn apply_answer(&self, sdp: &str) -> Result<PeerAnswer, PeerError>;

    /// Send one binary message (one fragment), waiting while the send buffer holds
    /// more than about 1 MiB.
    async fn send(&self, data: Vec<u8>) -> Result<(), PeerError>;

    /// Close the channel and the connection. Idempotent.
    fn close(&self);
}
