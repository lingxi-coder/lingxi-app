//! `FixtureFeed` — a deterministic, scripted [`MultiAgentFeed`] for tests and
//! for any UI surface whose engine source is not yet live. (M9-01)

use crate::multiagent::adapter::MultiAgentFeed;
use crate::multiagent::event::MultiAgentEvent;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Replays a fixed script: each `poll()` pops one step (a `Vec` of events).
/// Once the script is exhausted, every further `poll()` returns empty.
pub struct FixtureFeed {
    steps: Mutex<VecDeque<Vec<MultiAgentEvent>>>,
}

impl FixtureFeed {
    /// Build a feed from an ordered list of per-tick event batches.
    #[must_use]
    pub fn new(steps: Vec<Vec<MultiAgentEvent>>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
        }
    }
}

#[async_trait]
impl MultiAgentFeed for FixtureFeed {
    async fn poll(&self) -> Vec<MultiAgentEvent> {
        self.steps
            .lock()
            .expect("fixture poisoned")
            .pop_front()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiagent::state::TaskRow;

    fn task_step(n: usize) -> Vec<MultiAgentEvent> {
        vec![MultiAgentEvent::TasksRefreshed(
            (0..n)
                .map(|i| TaskRow {
            unread: false, model: None, effort: None,
                    awaiting_plan_approval: false,
                    task_id: format!("b{i:08}"),
                    task_type: "local_bash".into(),
                    status: "running".into(),
                    description: "x".into(),
                    command: None,
                })
                .collect(),
        )]
    }

    #[tokio::test]
    async fn replays_steps_in_order_then_empties() {
        let feed = FixtureFeed::new(vec![task_step(1), task_step(2)]);
        // Step 1.
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(v)] => assert_eq!(v.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
        // Step 2.
        match feed.poll().await.as_slice() {
            [MultiAgentEvent::TasksRefreshed(v)] => assert_eq!(v.len(), 2),
            other => panic!("unexpected: {other:?}"),
        }
        // Exhausted → empty forever.
        assert!(feed.poll().await.is_empty());
        assert!(feed.poll().await.is_empty());
    }
}
