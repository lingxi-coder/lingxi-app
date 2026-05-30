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

/// Lets `lingxi-sidequery::ForkedAgentRunner` allocate slots without taking
/// a direct dependency on this crate (avoids the
/// `sidequery → agent → memory → sidequery` cycle). The trait is currently a
/// marker so the M1.14 stub compiles; production methods land alongside the
/// real forked-agent runner in a later plan.
impl sidequery::SubagentSlotProvider for StateMachinePool {}

/// Failure modes for [`StateMachinePool`] operations.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// All `max_concurrent` slots are in use.
    #[error("too many agents — pool full")]
    TooManyAgents,
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

    #[tokio::test]
    async fn allocate_and_deallocate() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);

        let ctx = SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
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
            },
            prompt_messages: vec![],
            fork_context_messages: None,
            allowed_tools: vec![],
            worktree_handle: None,
            is_async: false,
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
        };
        let aid = ctx.agent_id;
        let (id, _rx) = pool.allocate(ctx).await.unwrap();
        assert_eq!(id, aid);
        assert_eq!(pool.slot_count().await, 1);
        pool.deallocate(&aid).await.unwrap();
        assert_eq!(pool.slot_count().await, 0);
    }
}
