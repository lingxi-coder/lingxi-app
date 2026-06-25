//! `MultiAgentFeed` — the swappable source of [`MultiAgentEvent`]s. (M9-01)
//!
//! Two impls share one output type: `crate::multiagent::fixture::FixtureFeed`
//! (deterministic, tests) and `crate::multiagent::poller::PollerFeed` (live,
//! over `TaskRegistryHandle`). The single output type is the contract that lets
//! the UI be built against fixtures and light up against the real engine with
//! no UI changes.

use crate::multiagent::event::MultiAgentEvent;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

/// A source of multi-agent updates. One `poll()` returns the events for one
/// tick (a poller read, or one scripted fixture step).
#[async_trait]
pub trait MultiAgentFeed: Send + Sync {
    /// Produce the events for the next tick. An empty `Vec` means "no change".
    async fn poll(&self) -> Vec<MultiAgentEvent>;

    /// (BGTASK-3) Stop a running task. Default: unsupported — feeds that
    /// don't back a real task registry (fixtures, tests) inherit this rather
    /// than each having to stub it out.
    async fn kill(&self, _task_id: &str) -> Result<(), String> {
        Err("this feed does not support stopping tasks".to_string())
    }
}

/// Poll `feed` once and forward every produced event to `tx`. Returns the
/// number of events sent. The M9-05 `root.rs` pump calls this on a tick; here
/// it is a standalone, fully-testable unit. A closed channel is treated as a
/// no-op (events are dropped) — the caller owns shutdown.
pub async fn pump_once(feed: &dyn MultiAgentFeed, tx: &UnboundedSender<MultiAgentEvent>) -> usize {
    let events = feed.poll().await;
    let mut sent = 0;
    for ev in events {
        if tx.send(ev).is_ok() {
            sent += 1;
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::fixture::FixtureFeed;
    use crate::multiagent::state::TaskRow;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn MultiAgentFeed>> = None;
    }

    #[tokio::test]
    async fn pump_once_forwards_fixture_events_to_channel() {
        let feed = FixtureFeed::new(vec![vec![MultiAgentEvent::TasksRefreshed(vec![TaskRow {
            task_id: "b00000001".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "x".into(),
            command: None,
        }])]]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sent = pump_once(&feed, &tx).await;
        assert_eq!(sent, 1);
        match rx.recv().await.unwrap() {
            MultiAgentEvent::TasksRefreshed(rows) => assert_eq!(rows.len(), 1),
            other @ MultiAgentEvent::WorkersRefreshed(_) => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pump_once_on_exhausted_feed_sends_nothing() {
        let feed = FixtureFeed::new(vec![]); // empty script
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sent = pump_once(&feed, &tx).await;
        assert_eq!(sent, 0);
        assert!(rx.try_recv().is_err());
    }
}
