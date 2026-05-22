//! Swarm backend — Windows (no tmux).
//!
//! Future: fall back to Windows Terminal or wezterm via their CLIs.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

/// Windows `SwarmBackend` — currently Unsupported.
#[derive(Default)]
pub struct WindowsSwarmBackend;

impl WindowsSwarmBackend {
    /// Construct a new `WindowsSwarmBackend`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SwarmBackend for WindowsSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        Err(SwarmError::Unsupported)
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        _position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        Err(SwarmError::Unsupported)
    }

    async fn destroy_swarm(&self, _handle: SwarmHandle) -> Result<(), SwarmError> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        false
    }
}
