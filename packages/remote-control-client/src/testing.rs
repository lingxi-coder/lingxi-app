//! Test doubles for code built on this crate: an in-memory WebRTC network.
//!
//! [`FakePeerNetwork`] hands out a [`PeerConnector`] for the viewer and lets a fake
//! host `accept_offer` the SDP it finds in an offer, getting back the host's end of
//! the DataChannel as a [`Link`]. [`MemoryRtdb`](crate::rtdb::MemoryRtdb) and
//! [`MemoryViewerKeys`](crate::device::MemoryViewerKeys) are the other doubles.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use crate::peer::{
    PeerAnswer, PeerConfig, PeerConnectionState, PeerConnector, PeerError, PeerEvent, PeerHandle,
    PeerOfferer,
};
use crate::transport::{FragmentLink, Link, LinkClosed, LinkEvent};

/// What a [`FakePeerNetwork`] does with the next peer connection that is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerBehavior {
    /// Open once both the answer has been applied and the host has accepted the offer.
    #[default]
    Connect,
    /// Never open (a firewall in the way).
    Hang,
    /// Report `Failed` after the answer was applied.
    IceFailed,
    /// Creating the peer connection fails.
    CreateFails,
    /// Creating the offer fails.
    OfferFails,
}

struct FakePeer {
    id: u64,
    behavior: PeerBehavior,
    events: mpsc::UnboundedSender<PeerEvent>,
    host_events: mpsc::UnboundedSender<LinkEvent>,
    host_receiver: Mutex<Option<mpsc::UnboundedReceiver<LinkEvent>>>,
    state: Mutex<PeerState>,
    ready: watch::Sender<bool>,
}

#[derive(Default)]
struct PeerState {
    closed: bool,
    answer_applied: bool,
    accepted: bool,
    opened: bool,
}

impl FakePeer {
    fn maybe_open(&self, state: &mut PeerState) {
        if state.closed || state.opened || !(state.answer_applied && state.accepted) {
            return;
        }
        match self.behavior {
            PeerBehavior::Connect => {
                state.opened = true;
                self.ready.send_replace(true);
                let _ = self
                    .events
                    .send(PeerEvent::State(PeerConnectionState::Connected));
                let _ = self.events.send(PeerEvent::Open);
            }
            PeerBehavior::IceFailed => {
                let _ = self
                    .events
                    .send(PeerEvent::State(PeerConnectionState::Failed));
            }
            _ => {}
        }
    }

    fn end(&self, viewer_reason: &str, host_reason: &str) {
        let mut state = self.state.lock().expect("peer");
        if state.closed {
            return;
        }
        state.closed = true;
        drop(state);
        let _ = self
            .events
            .send(PeerEvent::Closed(viewer_reason.to_owned()));
        let _ = self
            .host_events
            .send(LinkEvent::Closed(host_reason.to_owned()));
    }
}

struct ViewerEnd(Arc<FakePeer>);

#[async_trait]
impl PeerHandle for ViewerEnd {
    async fn offer_sdp(&self) -> Result<String, PeerError> {
        if self.0.behavior == PeerBehavior::OfferFails {
            return Err(PeerError("createOffer failed".into()));
        }
        Ok(format!("v=0 fake-offer {}", self.0.id))
    }

    async fn apply_answer(&self, sdp: &str) -> Result<PeerAnswer, PeerError> {
        if sdp != format!("v=0 fake-answer {}", self.0.id) {
            return Err(PeerError("unexpected answer".into()));
        }
        let mut ready = self.0.ready.subscribe();
        {
            let mut state = self.0.state.lock().expect("peer");
            state.answer_applied = true;
            self.0.maybe_open(&mut state);
        }
        // Like the real session: wait for the channel, at most 15 s.
        let opened = matches!(
            tokio::time::timeout(std::time::Duration::from_secs(15), ready.wait_for(|r| *r)).await,
            Ok(Ok(_))
        );
        Ok(if opened {
            PeerAnswer::Ready
        } else {
            PeerAnswer::TimedOut
        })
    }

    async fn send(&self, data: Vec<u8>) -> Result<(), PeerError> {
        let state = self.0.state.lock().expect("peer");
        if state.closed || !state.opened {
            return Err(PeerError("channel is not open".into()));
        }
        drop(state);
        let _ = self.0.host_events.send(LinkEvent::Fragment(data));
        Ok(())
    }

    fn close(&self) {
        self.0.end("closed", "peer_closed");
    }
}

struct HostEnd(Arc<FakePeer>);

#[async_trait]
impl FragmentLink for HostEnd {
    async fn send_fragment(&self, fragment: Vec<u8>) -> Result<(), LinkClosed> {
        let state = self.0.state.lock().expect("peer");
        if state.closed || !state.opened {
            return Err(LinkClosed("channel is not open".into()));
        }
        drop(state);
        let _ = self.0.events.send(PeerEvent::Message(fragment));
        Ok(())
    }

    fn close(&self, reason: &str) {
        self.0.end("peer_closed", reason);
    }
}

/// An in-memory WebRTC. Cheap to clone.
#[derive(Clone, Default)]
pub struct FakePeerNetwork {
    shared: Arc<NetworkShared>,
}

#[derive(Default)]
struct NetworkShared {
    next: AtomicU64,
    peers: Mutex<HashMap<u64, Arc<FakePeer>>>,
    behavior: Mutex<Vec<PeerBehavior>>,
    created: AtomicU64,
}

impl std::fmt::Debug for FakePeerNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakePeerNetwork").finish_non_exhaustive()
    }
}

impl FakePeerNetwork {
    /// A network where every peer connects.
    pub fn new() -> Self {
        Self::default()
    }

    /// Script what the next peer connections do, in order; later ones connect.
    pub fn script(&self, behaviors: impl IntoIterator<Item = PeerBehavior>) {
        *self.shared.behavior.lock().expect("behavior") = behaviors.into_iter().collect();
    }

    /// The viewer-side factory.
    pub fn connector(&self) -> Arc<dyn PeerConnector> {
        Arc::new(self.clone())
    }

    /// How many peer connections were created so far.
    pub fn created(&self) -> u64 {
        self.shared.created.load(Ordering::SeqCst)
    }

    /// The host side: accept the SDP of an offer. Returns the answer SDP and the
    /// host's end of the DataChannel (which carries data once the viewer applied the answer).
    pub fn accept_offer(&self, sdp: &str) -> Result<(String, Link), PeerError> {
        let id: u64 = sdp
            .strip_prefix("v=0 fake-offer ")
            .and_then(|rest| rest.parse().ok())
            .ok_or_else(|| PeerError("not an offer of this network".into()))?;
        let peer = self
            .shared
            .peers
            .lock()
            .expect("peers")
            .get(&id)
            .cloned()
            .ok_or_else(|| PeerError("unknown peer".into()))?;
        let events = peer
            .host_receiver
            .lock()
            .expect("peer")
            .take()
            .ok_or_else(|| PeerError("offer already accepted".into()))?;
        let mut state = peer.state.lock().expect("peer");
        state.accepted = true;
        peer.maybe_open(&mut state);
        drop(state);
        Ok((
            format!("v=0 fake-answer {id}"),
            Link {
                link: Arc::new(HostEnd(peer)),
                events,
            },
        ))
    }
}

#[async_trait]
impl PeerConnector for FakePeerNetwork {
    async fn create_offerer(&self, _config: PeerConfig) -> Result<PeerOfferer, PeerError> {
        let behavior = {
            let mut script = self.shared.behavior.lock().expect("behavior");
            if script.is_empty() {
                PeerBehavior::Connect
            } else {
                script.remove(0)
            }
        };
        if behavior == PeerBehavior::CreateFails {
            return Err(PeerError("cannot create a peer connection".into()));
        }
        self.shared.created.fetch_add(1, Ordering::SeqCst);
        let id = self.shared.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (events_tx, events) = mpsc::unbounded_channel();
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        let peer = Arc::new(FakePeer {
            id,
            behavior,
            events: events_tx,
            host_events: host_tx,
            host_receiver: Mutex::new(Some(host_rx)),
            state: Mutex::new(PeerState::default()),
            ready: watch::channel(false).0,
        });
        self.shared
            .peers
            .lock()
            .expect("peers")
            .insert(id, Arc::clone(&peer));
        Ok(PeerOfferer {
            peer: Arc::new(ViewerEnd(peer)),
            events,
        })
    }
}
