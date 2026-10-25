//! A WebRTC DataChannel as a [`FragmentLink`], over a [`PeerHandle`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

use crate::peer::{PeerConnectionState, PeerEvent, PeerHandle};
use crate::transport::{FragmentLink, LinkClosed, LinkEvent};
use crate::version::FRAGMENT_MAX_BYTES;

/// How the DataChannel stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectState {
    /// Negotiating.
    Pending,
    /// Open.
    Open,
    /// Ended, with the reason.
    Closed(String),
}

/// One fragment per binary message. Sending waits inside the peer while its
/// buffer is full, so a large snapshot cannot pile up in memory.
pub(crate) struct DirectLink {
    peer: Arc<dyn PeerHandle>,
    closed: AtomicBool,
    events: mpsc::UnboundedSender<LinkEvent>,
    state: watch::Sender<DirectState>,
}

impl DirectLink {
    /// Start pumping a peer's events. Returns the link and its inbound events.
    pub(crate) fn start(
        peer: Arc<dyn PeerHandle>,
        mut peer_events: mpsc::UnboundedReceiver<PeerEvent>,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<LinkEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (state, _) = watch::channel(DirectState::Pending);
        let link = Arc::new(Self {
            peer,
            closed: AtomicBool::new(false),
            events,
            state,
        });
        let pump = Arc::clone(&link);
        tokio::spawn(async move {
            while let Some(event) = peer_events.recv().await {
                match event {
                    PeerEvent::Open => {
                        pump.state.send_if_modified(|state| {
                            if *state == DirectState::Pending {
                                *state = DirectState::Open;
                                true
                            } else {
                                false
                            }
                        });
                    }
                    PeerEvent::Message(data) => {
                        if !pump.closed.load(Ordering::SeqCst) {
                            let _ = pump.events.send(LinkEvent::Fragment(data));
                        }
                    }
                    // ICE gave up. (`Disconnected` can still recover, and the keepalive
                    // judges a link that stays silent.)
                    PeerEvent::State(PeerConnectionState::Failed) => pump.close("ice_failed"),
                    PeerEvent::State(_) => {}
                    PeerEvent::Closed(reason) => {
                        pump.ended(&reason);
                        return;
                    }
                }
            }
            pump.ended("channel_closed");
        });
        (link, receiver)
    }

    /// Wait until the channel opens (`Ok`) or ends (`Err(reason)`).
    pub(crate) async fn wait_open(&self) -> Result<(), String> {
        let mut receiver = self.state.subscribe();
        loop {
            match receiver.borrow_and_update().clone() {
                DirectState::Open => return Ok(()),
                DirectState::Closed(reason) => return Err(reason),
                DirectState::Pending => {}
            }
            if receiver.changed().await.is_err() {
                return Err("channel_closed".into());
            }
        }
    }

    fn ended(&self, reason: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.events.send(LinkEvent::Closed(reason.to_owned()));
        self.state
            .send_replace(DirectState::Closed(reason.to_owned()));
    }
}

#[async_trait]
impl FragmentLink for DirectLink {
    fn max_fragment_bytes(&self) -> usize {
        FRAGMENT_MAX_BYTES
    }

    async fn send_fragment(&self, fragment: Vec<u8>) -> Result<(), LinkClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(LinkClosed("closed".into()));
        }
        self.peer.send(fragment).await.map_err(|e| LinkClosed(e.0))
    }

    fn close(&self, reason: &str) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.peer.close();
        self.ended(reason);
    }
}
