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
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::{mpsc, OwnedSemaphorePermit, RwLock, Semaphore};
use platform_api::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};

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
    pub event_tx: mpsc::Sender<lingxi_core::Event>,
    /// Capacity permit held for the entire lifetime of this slot.
    _capacity_permit: OwnedSemaphorePermit,
}

/// Fixed-capacity table of active subagent slots.
///
/// `max_concurrent` caps how many slots can be alive at once. Allocation
/// hands the caller back an [`mpsc::Receiver`] of [`SubagentEvent`]s the
/// runner emits as it processes the conversation.
pub struct StateMachinePool {
    slots: Arc<RwLock<HashMap<AgentId, StateMachineSlot>>>,
    capacity: Arc<Semaphore>,
    runtime: Arc<dyn RuntimeSpawner>,
    #[cfg(test)]
    post_spawn_wait: Arc<RwLock<Option<Arc<Notify>>>>,
}

/// Cancel-safety guard for [`StateMachinePool::allocate`].
///
/// `allocate` starts the runner before the slot is inserted into `slots`. If the
/// future is dropped in that window, the spawned runner must still be canceled;
/// otherwise a persistent child can outlive its caller with no way to recover
/// its task handle or pool slot.
struct AllocateCancelGuard {
    runtime: Arc<dyn RuntimeSpawner>,
    task: Option<BackgroundTaskHandle>,
}

impl AllocateCancelGuard {
    fn new(runtime: Arc<dyn RuntimeSpawner>, task: BackgroundTaskHandle) -> Self {
        Self {
            runtime,
            task: Some(task),
        }
    }

    fn disarm(&mut self) {
        self.task = None;
    }
}

impl Drop for AllocateCancelGuard {
    fn drop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let runtime = self.runtime.clone();
            handle.spawn(async move {
                let _ = runtime.cancel(&task).await;
            });
        }
    }
}

impl StateMachinePool {
    /// Construct a pool with capacity for `max_concurrent` slots, driven by
    /// the provided [`RuntimeSpawner`].
    #[must_use]
    pub fn new(runtime: Arc<dyn RuntimeSpawner>, max_concurrent: usize) -> Self {
        Self {
            slots: Arc::new(RwLock::new(HashMap::new())),
            capacity: Arc::new(Semaphore::new(max_concurrent)),
            runtime,
            #[cfg(test)]
            post_spawn_wait: Arc::new(RwLock::new(None)),
        }
    }

    #[cfg(test)]
    pub async fn set_post_spawn_wait(&self, notify: Arc<Notify>) {
        *self.post_spawn_wait.write().await = Some(notify);
    }

    /// Allocate a slot. Returns `(agent_id, event_rx)` where `event_rx` is the
    /// channel emitting [`SubagentEvent`]s for the new spawn. The
    /// `event_tx` for inbound `lingxi_core::Event`s is owned by the slot
    /// table and reachable via [`Self::send_event`] in later milestones.
    pub async fn allocate(
        &self,
        ctx: SubagentContext,
    ) -> Result<(AgentId, mpsc::Receiver<SubagentEvent>), PoolError> {
        // Reserve capacity atomically before spawning the runner. A len/read
        // check can race when multiple parallel Agent tool calls allocate at
        // once and let all of them pass the same stale count.
        let capacity_permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| PoolError::TooManyAgents)?;
        let agent_id = ctx.agent_id;
        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(100);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(100);

        let task = self
            .runtime
            .spawn(
                "subagent-state-machine",
                Box::pin(crate::runner::run_subagent(ctx, event_rx, out_tx)),
            )
            .await?;
        let mut cancel_guard = AllocateCancelGuard::new(self.runtime.clone(), task.clone());

        #[cfg(test)]
        if let Some(wait) = self.post_spawn_wait.read().await.clone() {
            wait.notified().await;
        }

        self.slots.write().await.insert(
            agent_id,
            StateMachineSlot {
                agent_id,
                task,
                event_tx,
                _capacity_permit: capacity_permit,
            },
        );
        cancel_guard.disarm();
        Ok((agent_id, out_rx))
    }

    /// Deliver an inbound [`lingxi_core::Event`] into the slot owned by `agent_id`.
    ///
    /// Used by message-driven handlers (e.g. the in-process teammate) to push
    /// a [`lingxi_core::Event::UserMessage`] to a parked subagent, and to deliver
    /// [`lingxi_core::Event::UserExit`] / [`lingxi_core::Event::UserInterrupt`] for
    /// cooperative termination.
    ///
    /// A `read()` lock suffices: we only read the `event_tx` sender, and the
    /// mpsc channel is internally synchronized.
    pub async fn send_event(
        &self,
        agent_id: &AgentId,
        event: lingxi_core::Event,
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
                observer: None,
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            cwd: None,
            is_async: false,
            persistent: false,
            can_show_permission_prompts: true,
            session_interactive: None,
            mcp_clients: vec![],
            transcript_subdir: "/tmp".into(),
            transcript_fs: None,
            resumed_history: None,
            rendered_system_prompt: None,
            mobile_runtime_environment_reminder: None,
            mobile_runtime_workspace_reminder: None,
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
            strict_plugin_only_hooks: false,
            skill_loader: None,
            hook_session_id: protocol::SessionId::nil(),
            hook_cwd: std::path::PathBuf::new(),
            depth: 0,
            observer: None,
            permission_mode_override: None,
            frozen_command_denies: Vec::new(),
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

    /// `send_event` routes an inbound `lingxi_core::Event` into the slot's runner.
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

        pool.send_event(&aid, lingxi_core::Event::UserExit)
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
            .send_event(&AgentId::new(), lingxi_core::Event::UserInterrupt)
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

    #[tokio::test]
    async fn parallel_allocations_cannot_race_past_capacity() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = Arc::new(StateMachinePool::new(runtime, 1));
        let (left, right) = tokio::join!(pool.allocate(make_ctx()), pool.allocate(make_ctx()));
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        assert!(
            matches!(left, Err(PoolError::TooManyAgents))
                || matches!(right, Err(PoolError::TooManyAgents))
        );
        assert_eq!(pool.slot_count().await, 1);
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
            pool.send_event(&id, lingxi_core::Event::UserExit).await.unwrap();
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
            .send_event(
                &aid,
                lingxi_core::Event::UserMessage {
                    message_id: protocol::MessageId::new(),
                    request_id: protocol::RequestId::new(),
                    content: "hello".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::AgentGone), "got {err:?}");
    }
}
