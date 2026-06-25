//! Host-owned slot table that holds concurrent subagent state machines.
//!
//! Subagents are not nested state machines — each spawn becomes a sibling
//! slot in the [`StateMachinePool`] that the host drives via channels. This
//! avoids stack growth in deep fork chains and gives unified
//! scheduling / cancellation. See spec §10.5.

use crate::context::SubagentContext;
use crate::runner::SubagentEvent;
use protocol::AgentId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use traits::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};

/// One slot in the [`StateMachinePool`].
///
/// The slot stores the [`BackgroundTaskHandle`] driving the subagent runner
/// plus the sender side of the slot's event channel; the receiver is moved
/// into [`crate::runner::run_subagent`] at allocation time.
pub struct StateMachineSlot {
    /// Agent id this slot is keyed by.
    pub agent_id: AgentId,
    /// Handle to the background task running the subagent.
    pub task: BackgroundTaskHandle,
    /// Sender used by the host to deliver `Event`s into the slot.
    pub event_tx: mpsc::Sender<engine::Event>,
}

/// Fixed-capacity table of active subagent slots.
///
/// `max_concurrent` caps how many slots can be alive at once. Allocation
/// hands the caller back an [`mpsc::Receiver`] of [`SubagentEvent`]s the
/// runner emits as it processes the conversation.
pub struct StateMachinePool {
    slots: Arc<RwLock<HashMap<AgentId, StateMachineSlot>>>,
    max_concurrent: usize,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl StateMachinePool {
    /// Construct a pool with capacity for `max_concurrent` slots, driven by
    /// the provided [`RuntimeSpawner`].
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeSpawner>, max_concurrent: usize) -> Self {
        Self {
            slots: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent,
            runtime,
        }
    }

    /// Allocate a slot. Returns `(agent_id, event_rx)` where `event_rx` is the
    /// channel emitting [`SubagentEvent`]s for the new spawn. The
    /// `event_tx` for inbound `engine::Event`s is owned by the slot
    /// table and reachable via [`Self::send_event`] in later milestones.
    pub async fn allocate(
        &self,
        ctx: SubagentContext,
    ) -> Result<(AgentId, mpsc::Receiver<SubagentEvent>), PoolError> {
        if self.slots.read().await.len() >= self.max_concurrent {
            return Err(PoolError::TooManyAgents);
        }
        let agent_id = ctx.agent_id;
        let (event_tx, event_rx) = mpsc::channel::<engine::Event>(100);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(100);

        let task = self
            .runtime
            .spawn(
                "subagent-state-machine",
                Box::pin(crate::runner::run_subagent(ctx, event_rx, out_tx)),
            )
            .await?;

        self.slots.write().await.insert(
            agent_id,
            StateMachineSlot {
                agent_id,
                task,
                event_tx,
            },
        );
        Ok((agent_id, out_rx))
    }

    /// Deliver an inbound [`engine::Event`] into the slot owned by `agent_id`.
    ///
    /// Used by message-driven handlers (e.g. the in-process teammate) to push
    /// a [`engine::Event::UserMessage`] to a parked subagent, and to deliver
    /// [`engine::Event::UserExit`] / [`engine::Event::UserInterrupt`] for
    /// cooperative termination.
    ///
    /// A `read()` lock suffices: we only read the `event_tx` sender, and the
    /// mpsc channel is internally synchronized.
    pub async fn send_event(
        &self,
        agent_id: &AgentId,
        event: engine::Event,
    ) -> Result<(), PoolError> {
        let slots = self.slots.read().await;
        let slot = slots.get(agent_id).ok_or(PoolError::NoSuchAgent)?;
        slot.event_tx
            .send(event)
            .await
            .map_err(|_| PoolError::AgentGone)?;
        Ok(())
    }

    /// Remove the slot owned by `agent_id` and cancel its background task.
    pub async fn deallocate(&self, agent_id: &AgentId) -> Result<(), PoolError> {
        let slot = self.slots.write().await.remove(agent_id);
        if let Some(slot) = slot {
            self.runtime.cancel(&slot.task).await?;
        }
        Ok(())
    }

    /// Number of currently active slots.
    pub async fn slot_count(&self) -> usize {
        self.slots.read().await.len()
    }
}

/// Failure modes for [`StateMachinePool`] operations.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// All `max_concurrent` slots are in use.
    #[error("too many agents — pool full")]
    TooManyAgents,
    /// No slot is currently allocated for the requested `agent_id`.
    #[error("no agent with the requested id")]
    NoSuchAgent,
    /// The slot exists but its inbound event channel is closed (the subagent
    /// runner future has dropped its receiver — effectively terminated).
    #[error("agent channel closed")]
    AgentGone,
    /// The underlying [`RuntimeSpawner`] rejected the spawn / cancel call.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SubagentContext;
    use crate::definition::{
        AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
    };
    use crate::display::{AgentColor, AgentDisplay};
    use std::sync::Arc;
    use test_harness::mocks::MockRuntimeSpawner;

    /// Build a minimal [`SubagentContext`] with `api_client = None`, so the
    /// allocated slot runs the legacy reducer-driven stub (no API calls).
    fn make_ctx() -> SubagentContext {
        SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_name: None,
            team_name: None,
            agent_definition: AgentDefinition {
                agent_type: "test".into(),
                when_to_use: String::new(),
                tools: AgentToolPolicy::All {
                    use_exact_tools: true,
                },
                max_turns: 1,
                model: AgentModel::Inherit,
                permission_mode: AgentPermissionMode::Bubble,
                source: AgentSource::BuiltIn,
                base_dir: "/tmp".into(),
                system_prompt: None,
                mcp_servers: vec![],
                frontmatter_hooks: vec![],
                icon: None,
                allowed_tools: vec![],
                worktree_requirement: None,
                disallowed_tools: vec![],
                skills: vec![],
                required_mcp_servers: vec![],
                background: false,
                isolation: None,
                memory: None,
                effort: None,
                initial_prompt: None,
                color: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            cwd: None,
            is_async: false,
            persistent: false,
            can_show_permission_prompts: true,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            rendered_system_prompt: None,
            content_replacement_state: None,
            agent_memory: None,
            display: AgentDisplay {
                color: AgentColor::Cyan,
                icon: None,
            },
            model_profile: None,
            api_client: None,
            tool_invoker: None,
            tool_schemas: vec![],
            schema: None,
            budget: None,
            hook_executor: None,
            skill_loader: None,
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
        }
    }

    #[tokio::test]
    async fn allocate_and_deallocate() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);

        let ctx = make_ctx();
        let aid = ctx.agent_id;
        let (id, _rx) = pool.allocate(ctx).await.unwrap();
        assert_eq!(id, aid);
        assert_eq!(pool.slot_count().await, 1);
        pool.deallocate(&aid).await.unwrap();
        assert_eq!(pool.slot_count().await, 0);
    }

    /// `send_event` routes an inbound `engine::Event` into the slot's runner.
    /// We drive the stub runner (api_client = None): a `UserExit` delivered via
    /// `send_event` makes it emit `SubagentEvent::Killed`, proving the event
    /// reached the slot's `event_tx`.
    #[tokio::test]
    async fn send_event_delivers_to_slot() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);

        let ctx = make_ctx();
        let aid = ctx.agent_id;
        let (_id, mut out_rx) = pool.allocate(ctx).await.unwrap();

        pool.send_event(&aid, engine::Event::UserExit)
            .await
            .expect("send_event delivers to the live slot");

        // The stub runner's fast-path turns UserExit into Killed.
        let ev = out_rx.recv().await.expect("a SubagentEvent");
        assert!(
            matches!(ev, SubagentEvent::Killed { agent_id } if agent_id == aid),
            "UserExit routed through send_event yields Killed; got {ev:?}"
        );
    }

    /// `send_event` to an id with no slot is `NoSuchAgent`.
    #[tokio::test]
    async fn send_event_unknown_agent_errors() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);
        let err = pool
            .send_event(&AgentId::new(), engine::Event::UserInterrupt)
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::NoSuchAgent), "got {err:?}");
    }

    /// Allocating beyond `max_concurrent` is rejected with `TooManyAgents`. A
    /// core invariant of the fixed-capacity slot table.
    #[tokio::test]
    async fn allocate_over_capacity_rejects() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 1);

        // First slot fits.
        let (_id, _rx) = pool.allocate(make_ctx()).await.unwrap();
        assert_eq!(pool.slot_count().await, 1);

        // Second slot exceeds the cap.
        let err = pool.allocate(make_ctx()).await.unwrap_err();
        assert!(matches!(err, PoolError::TooManyAgents), "got {err:?}");
        assert_eq!(pool.slot_count().await, 1, "rejected spawn left no slot");
    }

    /// `send_event` to a slot whose runner has dropped its inbound receiver
    /// (the run future returned, e.g. after a terminal event) surfaces
    /// `AgentGone` — distinct from `NoSuchAgent` (slot absent). The slot is
    /// still in the map, so this exercises the closed-channel arm specifically.
    #[tokio::test]
    async fn send_event_to_dropped_runner_is_agent_gone() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);

        let aid = {
            let ctx = make_ctx();
            let id = ctx.agent_id;
            let (_id, mut out_rx) = pool.allocate(ctx).await.unwrap();
            // Drive the stub runner to termination: UserExit makes it emit
            // Killed and return, dropping its `event_rx`. We never deallocate,
            // so the slot stays in the map with a now-closed inbound channel.
            pool.send_event(&id, engine::Event::UserExit).await.unwrap();
            // Wait for the runner to actually surface Killed and return so its
            // receiver is dropped before we probe the closed channel.
            let ev = out_rx.recv().await.expect("Killed event");
            assert!(matches!(ev, SubagentEvent::Killed { .. }), "got {ev:?}");
            // The out channel closes once the runner returns.
            while out_rx.recv().await.is_some() {}
            id
        };

        // The slot is still present (not deallocated) but the runner's
        // receiver is gone -> send fails with AgentGone.
        let err = pool
            .send_event(&aid, engine::Event::UserMessage {
                message_id: protocol::MessageId::new(),
                request_id: protocol::RequestId::new(),
                content: "hello".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::AgentGone), "got {err:?}");
    }
}
