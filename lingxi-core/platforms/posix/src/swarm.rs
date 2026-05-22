//! `tmux` `SwarmBackend` — POSIX.
//!
//! M2.02 ships the type surface only. Actually shelling out to `tmux` for
//! session creation and pane management is deferred to M2 phase 3 (real
//! pane choreography requires probing the host for `tmux` and avoiding
//! fork-bombs of leftover panes on crash). `is_available()` returns
//! `false` until then so the engine knows to bypass swarm orchestration.

use async_trait::async_trait;
use lingxi_protocol::AgentId;
use lingxi_traits::{PaneId, PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};

/// POSIX `SwarmBackend` using `tmux` (stub).
#[derive(Default)]
pub struct TmuxSwarmBackend;

impl TmuxSwarmBackend {
    /// Construct a new `TmuxSwarmBackend`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SwarmBackend for TmuxSwarmBackend {
    async fn start_swarm(&self, _layout: SwarmLayout) -> Result<SwarmHandle, SwarmError> {
        // M2 follow-up: shell out to `tmux new-session -d -s lingxi-swarm`.
        Err(SwarmError::Tmux(
            "M2 follow-up: tmux session creation".into(),
        ))
    }

    async fn create_teammate_pane(
        &self,
        _agent_id: &AgentId,
        _position: PanePosition,
    ) -> Result<PaneId, SwarmError> {
        Err(SwarmError::Tmux("M2 follow-up".into()))
    }

    async fn destroy_swarm(&self, _handle: SwarmHandle) -> Result<(), SwarmError> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        // M2 follow-up: actually probe `which tmux`.
        false
    }
}
