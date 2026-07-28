//! Cross-session agent restore — the read side of [`session::agent_rows`].
//!
//! [`DesktopParkedAgentStore`] records a parked background agent's launch
//! configuration beside its transcript, and [`restore_parked_agents`] rebuilds
//! those agents in a LATER process: it re-spawns each one through the same
//! `BackgroundAgentSpawner` a fresh launch uses, seeded with the conversation
//! recovered from its transcript instead of a prompt.
//!
//! A restore is deliberately narrow. It restores agents that PARKED (a
//! terminal agent has no row), whose transcript still exists (a row without one
//! could only be restarted from scratch, which is not a resume), and — for a
//! forked skill — whose permission scoping still corroborates. The last check
//! is the existing [`crate::fork_resume::DesktopForkResumeGate`], consulted
//! here on its COLD path: there is no live task record in a fresh process, so
//! the on-disk provenance marker is the only witness to the fork's identity.

use std::sync::Arc;

use async_trait::async_trait;
use traits::fork_resume_gate::ForkResumeGate;
use traits::parked_agent_store::ParkedAgentStore;
use traits::subagent_spawn::{SubagentInheritance, SubagentSpawnRequest, SubagentSpawner};

/// Writes and erases parked-agent rows under this session's `subagents/` dir.
pub struct DesktopParkedAgentStore {
    /// Where the rows live, beside each agent's transcript and fork sidecars.
    pub subagents_dir: std::path::PathBuf,
}

#[async_trait]
impl ParkedAgentStore for DesktopParkedAgentStore {
    async fn park(
        &self,
        task_id: &str,
        agent_id: protocol::AgentId,
        description: &str,
        request: &SubagentSpawnRequest,
    ) {
        let row = session::agent_rows::ParkedAgentRow {
            task_id: task_id.to_string(),
            agent_id,
            description: description.to_string(),
            request: request.clone(),
        };
        // Best-effort: failing to record a parked agent costs a later restore,
        // never the run in progress.
        if let Err(e) = session::agent_rows::write_row(&self.subagents_dir, &row).await {
            tracing::debug!("could not record parked agent {agent_id}: {e}");
        }
    }

    async fn unpark(&self, agent_id: protocol::AgentId) {
        if let Err(e) =
            session::agent_rows::remove_row(&self.subagents_dir, &agent_id.to_string()).await
        {
            tracing::debug!("could not clear parked agent {agent_id}: {e}");
        }
    }
}

/// What a restore attempt concluded for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// Re-spawned; carries the NEW agent id (a rebuilt runner is a new pool
    /// instance — its identity on disk is the row's id, which the caller keeps).
    Restored(protocol::AgentId),
    /// The forked-skill resume gate refused it. Carries the refusal, which is
    /// the same byte-exact message a live resume would surface.
    Refused(String),
    /// Its transcript recovered no messages — nothing to resume INTO, so
    /// re-spawning would silently restart the work from scratch.
    EmptyTranscript,
    /// The spawner could not start it.
    Failed(String),
}

/// Rebuild every restorable parked agent found under `subagents_dir`.
///
/// Returns one outcome per row, in the deterministic order
/// [`session::agent_rows::list_restorable`] yields, so a caller can report
/// exactly what happened rather than a count.
///
/// A restored agent's row is REWRITTEN by the handler as soon as it parks
/// again; a refused or failed one keeps its row, so the next process can try
/// again once (for example) the skill it needs is back.
pub async fn restore_parked_agents(
    subagents_dir: &std::path::Path,
    spawner: &dyn SubagentSpawner,
    gate: &dyn ForkResumeGate,
    inherit: &SubagentInheritance,
) -> Vec<(protocol::AgentId, RestoreOutcome)> {
    let rows = session::agent_rows::list_restorable(subagents_dir).await;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.agent_id;
        // COLD gate: a fresh process has no live task record, so the fork's
        // identity is corroborated against the on-disk provenance marker.
        // Passing `None` is what selects that path — see
        // `session::forked_skill::check_scoping_provenance`.
        if let Err(refusal) = gate.check_resume(id, None).await {
            out.push((id, RestoreOutcome::Refused(refusal)));
            continue;
        }
        let history =
            session::agent_rows::read_transcript_messages(subagents_dir, &id.to_string()).await;
        if history.is_empty() {
            out.push((id, RestoreOutcome::EmptyTranscript));
            continue;
        }
        let mut request = row.request.clone();
        // The recovered conversation REPLACES the seeding — prompt, fork
        // context and preload alike. Re-sending the original prompt would make
        // the agent redo work its own transcript already records, and reusing
        // `fork_context_messages` (a PREFIX the runner adds ahead of the prompt
        // and the `SubagentStart` preload) would additionally re-fire start
        // hooks for a run that began in another process.
        request.resumed_history = Some(history);
        request.prompt = String::new();
        match spawner.spawn_async(request, inherit.clone()).await {
            Ok(launch) => out.push((id, RestoreOutcome::Restored(launch.agent_id))),
            Err(e) => out.push((id, RestoreOutcome::Failed(e.to_string()))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use session::agent_rows::{write_row, ParkedAgentRow};
    use std::sync::Mutex as StdMutex;
    use traits::subagent_spawn::{
        AsyncLaunch, SelectedAgentMeta, SubagentListingEntry, SubagentResult, SubagentSpawnError,
    };

    fn request() -> SubagentSpawnRequest {
        SubagentSpawnRequest {
            subagent_type: "general-purpose".into(),
            prompt: "the original prompt".into(),
            observer: None,
            context_paths: Vec::new(),
            description: Some("research".into()),
            model: Some("claude-opus-5".into()),
            model_profile: None,
            run_in_background: true,
            name: None,
            team_name: None,
            creator_teammate_name: None,
            creator_team_name: None,
            mode: None,
            isolation: None,
            cwd: Some("/repo".into()),
            worktree: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 1,
            parent_model_override: None,
            forked_skill_name: None,
            forked_skill_attribution: None,
            forked_skill_effort: None,
            frozen_command_denies: Vec::new(),
            resumed_history: None,
        }
    }

    #[derive(Default)]
    struct RecordingSpawner {
        seen: StdMutex<Vec<SubagentSpawnRequest>>,
        fail: bool,
    }
    #[async_trait]
    impl SubagentSpawner for RecordingSpawner {
        async fn spawn(
            &self,
            _r: SubagentSpawnRequest,
            _i: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            unreachable!("restore uses the async path")
        }
        async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
            vec![]
        }
        async fn resolve_selection(&self, t: &str, _m: Option<&str>) -> SelectedAgentMeta {
            SelectedAgentMeta {
                agent_type: t.to_string(),
                ..Default::default()
            }
        }
        async fn spawn_async(
            &self,
            request: SubagentSpawnRequest,
            _i: SubagentInheritance,
        ) -> Result<AsyncLaunch, SubagentSpawnError> {
            self.seen.lock().unwrap().push(request);
            if self.fail {
                return Err(SubagentSpawnError::Runtime("pool full".into()));
            }
            Ok(AsyncLaunch {
                agent_id: protocol::AgentId::new(),
                output_file: "/tmp/a.output".into(),
            })
        }
    }

    struct Gate(Option<&'static str>);
    #[async_trait]
    impl ForkResumeGate for Gate {
        async fn check_resume(
            &self,
            _agent_id: protocol::AgentId,
            _task_forked_skill_name: Option<&str>,
        ) -> Result<(), String> {
            match self.0 {
                Some(msg) => Err(msg.to_string()),
                None => Ok(()),
            }
        }
    }

    struct NoInvoker;
    #[async_trait]
    impl traits::tool_invoker::ToolInvoker for NoInvoker {
        async fn invoke(
            &self,
            _n: &str,
            _i: serde_json::Value,
            _c: traits::tool_invoker::SubagentInvocationContext,
        ) -> Result<serde_json::Value, traits::tool_invoker::ToolInvokerError> {
            Ok(serde_json::Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    struct NoBudget;
    #[async_trait]
    impl traits::budget::BudgetEnforcerHandle for NoBudget {
        async fn check_and_charge(&self, _n: u64) -> Result<(), traits::budget::BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }
    fn inherit() -> SubagentInheritance {
        SubagentInheritance {
            tool_invoker: Arc::new(NoInvoker),
            budget: Arc::new(NoBudget),
        }
    }

    async fn seed(dir: &std::path::Path, id: protocol::AgentId, transcript: &[&str]) {
        write_row(
            dir,
            &ParkedAgentRow {
                task_id: "a00000001".into(),
                agent_id: id,
                description: "research".into(),
                request: request(),
            },
        )
        .await
        .unwrap();
        if transcript.is_empty() {
            // Still create the file — an EMPTY transcript is distinct from a
            // missing one (the row is listed, then rejected for having nothing
            // to resume into).
            tokio::fs::write(
                session::forked_skill::agent_transcript_path(dir, &id.to_string()),
                "",
            )
            .await
            .unwrap();
            return;
        }
        let mut body = String::new();
        for t in transcript {
            let msg =
                protocol::ConversationMessage::user(protocol::MessageId::new(), (*t).to_string());
            body.push_str(
                &serde_json::to_string(&serde_json::json!({
                    "agent_id": id.to_string(),
                    "timestamp": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
                    "message": msg,
                }))
                .unwrap(),
            );
            body.push('\n');
        }
        tokio::fs::write(
            session::forked_skill::agent_transcript_path(dir, &id.to_string()),
            body,
        )
        .await
        .unwrap();
    }

    /// The end-to-end point of the whole layer: an agent parked in one process
    /// is rebuilt in the next, seeded with its RECOVERED conversation rather
    /// than its original prompt — otherwise it would redo work its own
    /// transcript already records.
    #[tokio::test]
    async fn a_parked_agent_is_rebuilt_from_its_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let id = protocol::AgentId::new();
        seed(dir.path(), id, &["first", "second"]).await;

        let spawner = RecordingSpawner::default();
        let outcomes =
            restore_parked_agents(dir.path(), &spawner, &Gate(None), &inherit()).await;

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0].1, RestoreOutcome::Restored(_)));
        let seen = spawner.seen.lock().unwrap();
        let req = seen.first().expect("spawned");
        assert_eq!(
            req.resumed_history.as_ref().map(Vec::len),
            Some(2),
            "seeded with the recovered conversation"
        );
        assert!(
            req.fork_context_messages.is_none(),
            "a restore must NOT ride the fork-context prefix — that would \
             re-run the preload and re-fire SubagentStart"
        );
        assert!(
            req.prompt.is_empty(),
            "the original prompt is NOT re-sent: {:?}",
            req.prompt
        );
        // The launch configuration is the row's, not a default.
        assert_eq!(req.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(req.cwd.as_deref(), Some("/repo"));
        assert_eq!(req.depth, 1);
    }

    /// The forked-skill gate is consulted on its COLD path and its refusal is
    /// surfaced verbatim — a restore must not become a way to resume a fork
    /// whose scoping no longer corroborates.
    #[tokio::test]
    async fn a_refused_fork_is_not_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let id = protocol::AgentId::new();
        seed(dir.path(), id, &["first"]).await;

        let spawner = RecordingSpawner::default();
        let outcomes = restore_parked_agents(
            dir.path(),
            &spawner,
            &Gate(Some("refusing to resume it without the skill's permission scoping.")),
            &inherit(),
        )
        .await;

        assert_eq!(
            outcomes[0].1,
            RestoreOutcome::Refused(
                "refusing to resume it without the skill's permission scoping.".into()
            )
        );
        assert!(
            spawner.seen.lock().unwrap().is_empty(),
            "a refused agent is never spawned"
        );
    }

    /// An empty transcript means there is nothing to resume INTO; re-spawning
    /// would silently restart the work from scratch.
    #[tokio::test]
    async fn an_empty_transcript_is_not_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let id = protocol::AgentId::new();
        seed(dir.path(), id, &[]).await;

        let spawner = RecordingSpawner::default();
        let outcomes =
            restore_parked_agents(dir.path(), &spawner, &Gate(None), &inherit()).await;
        assert_eq!(outcomes[0].1, RestoreOutcome::EmptyTranscript);
        assert!(spawner.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_spawn_failure_is_reported_not_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), protocol::AgentId::new(), &["first"]).await;
        let spawner = RecordingSpawner {
            fail: true,
            ..Default::default()
        };
        let outcomes =
            restore_parked_agents(dir.path(), &spawner, &Gate(None), &inherit()).await;
        assert!(matches!(outcomes[0].1, RestoreOutcome::Failed(_)));
    }

    /// park writes a row, unpark erases it — and the ABSENCE is what stops a
    /// terminated agent from being revived.
    #[tokio::test]
    async fn park_then_unpark_leaves_nothing_to_restore() {
        let dir = tempfile::tempdir().unwrap();
        let id = protocol::AgentId::new();
        let store = DesktopParkedAgentStore {
            subagents_dir: dir.path().to_path_buf(),
        };
        store.park("a00000001", id, "research", &request()).await;
        assert!(session::agent_rows::read_row(dir.path(), &id.to_string())
            .await
            .is_some());

        store.unpark(id).await;
        assert!(session::agent_rows::read_row(dir.path(), &id.to_string())
            .await
            .is_none());

        let spawner = RecordingSpawner::default();
        assert!(
            restore_parked_agents(dir.path(), &spawner, &Gate(None), &inherit())
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn nothing_parked_restores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = RecordingSpawner::default();
        assert!(
            restore_parked_agents(dir.path(), &spawner, &Gate(None), &inherit())
                .await
                .is_empty()
        );
    }
}
