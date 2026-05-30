//! `MultiAgentFeed` — the swappable source of [`MultiAgentEvent`]s. (M9-01)
//!
//! Two impls share one output type: `crate::multiagent::fixture::FixtureFeed`
//! (deterministic, tests) and `crate::multiagent::poller::PollerFeed` (live,
//! over `TaskRegistryHandle`). The single output type is the contract that lets
//! the UI be built against fixtures and light up against the real engine with
//! no UI changes.

use crate::multiagent::event::MultiAgentEvent;
use async_trait::async_trait;

/// A source of multi-agent updates. One `poll()` returns the events for one
/// tick (a poller read, or one scripted fixture step).
#[async_trait]
pub trait MultiAgentFeed: Send + Sync {
    /// Produce the events for the next tick. An empty `Vec` means "no change".
    async fn poll(&self) -> Vec<MultiAgentEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn MultiAgentFeed>> = None;
    }
}
