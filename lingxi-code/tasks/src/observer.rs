//! Independent observer task creation and non-interrupting activity delivery.

use crate::id::TaskType;
use crate::registry::TaskRegistry;
use crate::state::{TaskState, TaskStatus};
use crate::task_trait::{TaskContext, TaskError, TaskSpawnInput};
use platform_api::{SubagentInheritance, SubagentSpawnRequest};
use protocol::AgentId;

impl TaskRegistry {
    pub(crate) async fn deliver_observer_digest(
        &self,
        task_id: &str,
        digest: String,
    ) -> Result<(), TaskError> {
        let handler = self
            .handlers
            .get(&TaskType::LocalAgent)
            .ok_or(TaskError::UnknownType)?;
        handler
            .send_message(
                task_id,
                digest,
                TaskContext {
                    fs: self.fs.clone(),
                    runtime: self.runtime.clone(),
                },
            )
            .await
    }

    pub(crate) async fn observe_agent_activity(
        &self,
        mut request: SubagentSpawnRequest,
        inheritance: SubagentInheritance,
        observed_agent_id: AgentId,
        digest: String,
    ) -> Result<(), TaskError> {
        // Pair lookup and first publication form one transaction, including
        // concurrent lifecycle taps restored for the same observed agent.
        let _pairing = self.observer_activity_lock.lock().await;
        let resume = {
            let mut tasks = self.tasks.write().await;
            let existing = tasks.values_mut().find_map(|state| match state {
                TaskState::LocalAgent(agent)
                    if agent.is_observer
                        && agent.observed_agent_id == Some(observed_agent_id)
                        && agent.subagent_type == request.subagent_type =>
                {
                    Some(agent)
                }
                _ => None,
            });
            if let Some(agent) = existing {
                if agent.base.status.is_terminal() && !agent.is_parked {
                    // An explicitly stopped observer is never silently relaunched.
                    return Err(TaskError::TerminatedTask);
                }
                agent.pending_messages.push(digest.clone());
                if !agent.is_parked {
                    return Ok(());
                }
                agent.is_parked = false;
                agent.base.status = TaskStatus::Running;
                Some((
                    agent.base.id.clone(),
                    std::mem::take(&mut agent.pending_messages).join("\n\n"),
                ))
            } else {
                None
            }
        };
        if let Some((id, digest)) = resume {
            return self.deliver_observer_digest(&id, digest).await;
        }
        request.observer = None;
        request.run_in_background = true;
        request.query_source_label =
            Some(platform_api::subagent_spawn::OBSERVER_QUERY_SOURCE.into());
        request.creator_agent_id = None;
        request.prompt = format!("{}\n\n{}", request.prompt, digest);
        let description = request
            .description
            .clone()
            .unwrap_or_else(|| format!("Observer {}", request.subagent_type));
        let input = TaskSpawnInput::LocalAgent {
            agent_id: AgentId::new(),
            subagent_type: request.subagent_type.clone(),
            prompt: request.prompt.clone(),
            is_backgrounded: true,
            tool_use_id: None,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: Some(observed_agent_id),
            spawn_request: Some(request),
            inheritance: Some(inheritance),
        };
        self.spawn(TaskType::LocalAgent, input, description).await?;
        Ok(())
    }
}
