//! Swarm backend — Windows.
//!
//! claude-code refuses tmux / swarm on Windows: the platform check returns
//! before any swarm code runs and the user sees `--tmux is not supported on
//! Windows`. We match by returning [`SwarmError::Unsupported`] from every
//! fallible method. The previous v0.2.0 doc mentioned a "wezterm / Windows
//! Terminal" fallback — that feature does not exist in claude-code and was
//! invented in M1. M2-01 removes it.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

/// Windows-side [`SwarmBackend`] — always reports unsupported.
#[derive(Default)]
pub struct WindowsSwarmBackend;

impl WindowsSwarmBackend {
    /// Construct a new `WindowsSwarmBackend`. Holds no state.
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
        Err(SwarmError::Unsupported)
    }

    fn is_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::AgentId;
    use lingxi_traits::{PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

    #[test]
    fn is_available_returns_false() {
        assert!(!WindowsSwarmBackend::new().is_available());
    }

    #[tokio::test]
    async fn start_swarm_returns_unsupported() {
        let err = WindowsSwarmBackend::new()
            .start_swarm(SwarmLayout::LeaderFollower)
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }

    #[tokio::test]
    async fn create_teammate_pane_returns_unsupported() {
        let err = WindowsSwarmBackend::new()
            .create_teammate_pane(&AgentId::nil(), PanePosition::Right)
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }

    #[tokio::test]
    async fn destroy_swarm_returns_unsupported() {
        let err = WindowsSwarmBackend::new()
            .destroy_swarm(SwarmHandle {
                session_name: "phantom".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmError::Unsupported));
    }
}
