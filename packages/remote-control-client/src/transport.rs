//! The seam between byte-level links (a DataChannel, the RTDB tunnel) and the
//! message-level RPC: a [`FragmentLink`] moves fragments, the [`RpcPeer`](crate::rpc::RpcPeer)
//! turns them into whole JSON messages and back.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::version::FRAGMENT_MAX_BYTES;

/// What a link reports to its single consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// The peer's next fragment, in order.
    Fragment(Vec<u8>),
    /// The link ended, whoever ended it; `reason` is a short machine-readable code.
    /// Always the last event.
    Closed(String),
}

/// The link refused a send because it is closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("link is closed: {0}")]
pub struct LinkClosed(pub String);

/// The sending half of an ordered, reliable stream of fragments (see [`crate::framing`]).
#[async_trait]
pub trait FragmentLink: Send + Sync {
    /// Largest fragment this link wants, at most 16384 (the tunnel prefers 16374
    /// so that three fit one frame).
    fn max_fragment_bytes(&self) -> usize {
        FRAGMENT_MAX_BYTES
    }

    /// Queue one fragment for the peer, first waiting while the link is
    /// backpressured. Fails if the link is closed.
    async fn send_fragment(&self, fragment: Vec<u8>) -> Result<(), LinkClosed>;

    /// End the link. Idempotent.
    fn close(&self, reason: &str);
}

/// A link together with its inbound events.
pub struct Link {
    /// The sending half.
    pub link: Arc<dyn FragmentLink>,
    /// Inbound fragments and the final close. Single consumer.
    pub events: mpsc::UnboundedReceiver<LinkEvent>,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").finish_non_exhaustive()
    }
}
