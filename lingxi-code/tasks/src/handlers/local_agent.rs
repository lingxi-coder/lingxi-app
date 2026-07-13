//! Local-agent (one-shot subagent) task handler — M2 implementation.
//!
//! Spawns a child subagent through the [`SubagentSpawner`] seam, drives it to
//! completion on the engine's [`RuntimeSpawner`] (never `tokio::spawn` directly
//! — D17), spools the terminal [`SubagentResult`] payload into the task's spool
//! file, and reports the terminal [`TaskStatus`] through a narrow status sink.
//!
//! This is the Rust analogue of the claude-code `LocalAgentTask` /
//! `registerAsyncAgent` lifecycle: the agent runs as a backgrounded one-shot,
//! the transcript/result is spooled to a per-agent disk file, and `kill`
//! aborts the in-flight worker (the analogue of TS `abortController.abort()` +
//! `unregisterCleanup()`).
//!
//! ## Why kill rides on the runtime handle, not a process handle
//!
//! Unlike [`crate::handlers::local_bash::LocalBashHandler`] (which can recover
//! a [`traits::ProcessHandle`] for a true OS kill), [`SubagentSpawner::spawn`]
//! is *await-to-completion*: it returns the terminal [`SubagentResult`] and
//! hands back no live handle while the subagent runs. The only cancellation
//! primitive available is therefore cooperative cancellation of the worker
//! future via [`RuntimeSpawner::cancel`] on the [`traits::BackgroundTaskHandle`]
//! returned by [`RuntimeSpawner::spawn`]. The handler records that handle (plus
//! the `runtime` Arc that minted it, since [`TaskContext`] is per-call and the
//! synchronous cleanup path has no `ctx`) in a map keyed by `task_id`, so both
//! [`Task::kill`] and [`LocalAgentHandler::drain_pending_kills`] can cancel the
//! worker. Dropping the worker future drops the pending `spawner.spawn(..)`
//! await, which is the cooperative-cancel contract the runtime documents.
//!
//! ## `agent_id` and `subagent_type` are independent sibling fields
//!
//! [`TaskSpawnInput::LocalAgent`] carries BOTH an [`protocol::AgentId`] (a
//! per-instance identity UUID) AND a resolved `subagent_type: String` (one of
//! the built-in subagent-type *names*), exactly mirroring the TS
//! `LocalAgentTaskState` shape (`agentId` + `agentType`). The caller that
//! already resolved the `AgentDefinition` stamps `subagent_type` (applying the
//! `'general-purpose'` fallback when the definition has none, matching the TS
//! `selectedAgent.agentType ?? 'general-purpose'`). Modern background Agent
//! launches also carry the complete already-resolved request and inheritance
//! handles; legacy direct-task callers retain the compact fallback path.

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use agent::{StreamingSubagentSpawner, SubagentEvent};
use async_trait::async_trait;
use protocol::AgentId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use traits::{
    BackgroundTaskHandle, BudgetEnforcerHandle, RuntimeSpawner, SubagentInheritance,
    SubagentResult, SubagentSpawnRequest, SubagentSpawner,
};

// Re-export the status-sink seam from the bash handler so callers wire a single
// implementation across handlers (the wire step plugs in one adapter over the
// registry). Defined once in `local_bash` to avoid divergence.
pub use crate::handlers::local_bash::{NoopStatusSink, TaskStatusSink};

/// Handler name reported by [`Task::name`] / used as the runtime task-name
/// prefix. Byte-aligned with the `"local_agent"` wire string the registration
/// step uses.
const HANDLER_NAME: &str = "local_agent";

/// A worker-cancel record: the [`BackgroundTaskHandle`] returned by
/// [`RuntimeSpawner::spawn`] plus the `runtime` Arc that minted it.
///
/// [`TaskContext`] (which carries `runtime`) is per-call and the synchronous
/// [`TaskHandle::cleanup`] closure receives no `ctx`, so the handler captures
/// the `runtime` alongside the handle at spawn time. That lets both
/// [`Task::kill`] and [`LocalAgentHandler::drain_pending_kills`] issue
/// [`RuntimeSpawner::cancel`] without a fresh `ctx`.
///
/// Public because it appears in the signature of the public
/// [`LocalAgentHandler::workers_map`] accessor (parity with
/// `LocalBashHandler::children_map`); fields stay private.
pub struct WorkerCancel {
    handle: BackgroundTaskHandle,
    runtime: Arc<dyn RuntimeSpawner>,
}

/// Local-agent task handler.
///
/// Holds the constructor-injected dependencies that are *not* on
/// [`TaskContext`] — the subagent spawner, the parent's `tool_invoker` +
/// `budget` (bundled into a per-spawn [`SubagentInheritance`]), the spool
/// manager, and the terminal-status sink — plus the shared worker-handle map
/// keyed by `task_id` so [`Task::kill`] can cancel the in-flight worker future.
/// The `subagent_type` is no longer derived here: it arrives already resolved
/// on the [`TaskSpawnInput::LocalAgent`] variant and is forwarded verbatim.
pub struct LocalAgentHandler {
    /// Allocates a subagent slot and pumps it to a terminal [`SubagentResult`].
    spawner: Arc<dyn SubagentSpawner>,
    /// The persistent/resumable spawn seam (local_agent resume). When wired AND
    /// the spawn is backgrounded, the agent runs PERSISTENT — it "comes to rest"
    /// after each turn-set and can be resumed via [`Task::send_message`] —
    /// instead of the one-shot `spawner`. Production passes the same
    /// `PoolSubagentSpawner` (it impls both `SubagentSpawner` and this).
    streaming_spawner: Option<Arc<dyn StreamingSubagentSpawner>>,
    /// `task_id` → the resting agent's id, for `send_message` resume routing.
    /// Populated for a live persistent agent; removed when it terminates.
    agent_ids: Arc<Mutex<HashMap<String, AgentId>>>,
    /// Parent's tool invoker — passed through *unchanged* in
    /// [`SubagentInheritance`] (the recursion lock relies on `Arc::ptr_eq`).
    tool_invoker: Arc<dyn traits::ToolInvoker>,
    /// Parent's budget enforcer — passed through *unchanged* so budget charges
    /// aggregate across the whole agent tree (`Arc::ptr_eq` invariant).
    budget: Arc<dyn BudgetEnforcerHandle>,
    /// Owns the spool directory + path allocation for the result payload.
    output_manager: Arc<TaskOutputManager>,
    /// Where terminal status transitions / token usage are reported.
    status_sink: Arc<dyn TaskStatusSink>,
    /// `task_id` → live worker-cancel record. Populated for the duration of the
    /// spawn; the worker removes its own entry on exit, and [`Task::kill`]
    /// removes + cancels it if still present.
    workers: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    /// `task_id` → record queued for teardown by the synchronous
    /// [`TaskHandle::cleanup`] closure (which cannot await). Drained by
    /// [`LocalAgentHandler::drain_pending_kills`].
    pending_kill: Arc<Mutex<HashMap<String, WorkerCancel>>>,
    /// Runs the terminal keep/cleanup judgment on a background agent's
    /// isolation worktree (`SubagentSpawnRequest::worktree`) — claude-code
    /// hands its `getWorktreeResult` closure to the detached async lifecycle,
    /// so the worker here judges via
    /// [`traits::worktree::agent_worktree_result`] when the agent reaches a
    /// terminal state (keep when dirty/ahead, else auto-remove). `None`
    /// (default) ⇒ no worktree handling: a carried worktree is left in place,
    /// the conservative direction.
    worktree_manager: Option<Arc<dyn traits::worktree::WorktreeManager>>,
}

impl LocalAgentHandler {
    /// Construct a handler with the injected dependencies.
    ///
    /// `fs` and `runtime` arrive per-call via [`TaskContext`] and are
    /// deliberately *not* injected here. `tool_invoker` + `budget` are stored
    /// so each spawn can bundle them into a [`SubagentInheritance`] (cloning the
    /// `Arc` preserves pointer identity — required by the recursion-lock +
    /// budget-aggregation invariants). The `subagent_type` is supplied per-spawn
    /// on the [`TaskSpawnInput::LocalAgent`] variant, so there is nothing to
    /// inject here.
    #[must_use]
    pub fn new(
        spawner: Arc<dyn SubagentSpawner>,
        tool_invoker: Arc<dyn traits::ToolInvoker>,
        budget: Arc<dyn BudgetEnforcerHandle>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            spawner,
            streaming_spawner: None,
            agent_ids: Arc::new(Mutex::new(HashMap::new())),
            tool_invoker,
            budget,
            output_manager,
            status_sink: Arc::new(NoopStatusSink),
            workers: Arc::new(Mutex::new(HashMap::new())),
            pending_kill: Arc::new(Mutex::new(HashMap::new())),
            worktree_manager: None,
        }
    }

    /// Wire the worktree manager so a BACKGROUND agent's `isolation:"worktree"`
    /// worktree (resolved by `AgentTool` before dispatch and carried on
    /// [`SubagentSpawnRequest::worktree`]) is auto-cleaned — kept when
    /// dirty/ahead — when the agent reaches a terminal state. This is the
    /// async-path owner of the same keep/cleanup judgment the sync path runs
    /// inside `AgentTool` (claude-code's `getWorktreeResult` closure).
    #[must_use]
    pub fn with_worktree_manager(
        mut self,
        manager: Arc<dyn traits::worktree::WorktreeManager>,
    ) -> Self {
        self.worktree_manager = Some(manager);
        self
    }

    /// Wire the persistent/resumable spawn seam (local_agent resume). When set,
    /// a backgrounded spawn runs PERSISTENT (comes to rest + resumable) instead
    /// of one-shot. Production passes the same `PoolSubagentSpawner`.
    #[must_use]
    pub fn with_streaming_spawner(mut self, streaming: Arc<dyn StreamingSubagentSpawner>) -> Self {
        self.streaming_spawner = Some(streaming);
        self
    }

    /// Attach a [`TaskStatusSink`] so terminal transitions / token usage are
    /// reported (e.g. an adapter over [`crate::registry::TaskRegistry`]).
    #[must_use]
    pub fn with_status_sink(mut self, sink: Arc<dyn TaskStatusSink>) -> Self {
        self.status_sink = sink;
        self
    }

    /// Share the same `workers` map with an external owner (e.g. the registry
    /// wiring) so a [`TaskHandle::cleanup`] closure and [`Task::kill`] observe
    /// the same handles.
    #[must_use]
    pub fn workers_map(&self) -> Arc<Mutex<HashMap<String, WorkerCancel>>> {
        self.workers.clone()
    }

    /// Drain records queued by [`TaskHandle::cleanup`] and cancel each worker
    /// future for real. This is the async counterpart of the synchronous
    /// cleanup closure: the registry/cleanup-registry calls it on agent
    /// teardown so a subagent outliving its parent is aborted (claude-code
    /// `killTask`-on-cleanup parity). Status is flipped to `Killed` regardless.
    pub async fn drain_pending_kills(&self) {
        let pending: Vec<(String, WorkerCancel)> = self.pending_kill.lock().await.drain().collect();
        for (task_id, rec) in pending {
            let _ = rec.runtime.cancel(&rec.handle).await;
            self.status_sink
                .set_status(&task_id, TaskStatus::Killed)
                .await;
        }
    }
}

#[async_trait]
impl Task for LocalAgentHandler {
    fn name(&self) -> &str {
        HANDLER_NAME
    }

    fn task_type(&self) -> TaskType {
        TaskType::LocalAgent
    }

    async fn spawn(
        &self,
        input: TaskSpawnInput,
        ctx: TaskContext,
    ) -> Result<TaskHandle, TaskError> {
        // 1. Only the LocalAgent variant is accepted; reject the other six.
        let TaskSpawnInput::LocalAgent {
            agent_id: _agent_id,
            subagent_type,
            prompt,
            is_backgrounded,
            // The registry's `state_for_spawn` stamps this onto `TaskStateBase`;
            // the handler itself doesn't consume it.
            tool_use_id: _,
            spawn_request,
            inheritance,
        } = input
        else {
            return Err(TaskError::Internal(
                "local_agent handler received a non-LocalAgent spawn input".into(),
            ));
        };
        // `is_backgrounded` drives surfacing/eviction (the registry layer
        // records it on `LocalAgentTaskState`) and is also threaded onto the
        // spawn request's `run_in_background` below for spawn-surface parity; the
        // one-shot subagent's local run mechanics are otherwise identical.

        // 2. Generate the task id (prefix 'a') and allocate its spool file.
        let task_id = crate::id::generate_task_id(TaskType::LocalAgent);
        let spool_path = self
            .output_manager
            .allocate(&task_id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        // Validate UTF-8 once up front (the output manager's `append` assumes a
        // UTF-8 spool path); spool paths under the manager's dir always are.
        if spool_path.to_str().is_none() {
            return Err(TaskError::Internal("spool path is not valid UTF-8".into()));
        }

        // 3. Preserve the complete background Agent request (model/cwd/context/
        //    isolation/schema/depth and more). Direct TaskCreate-style callers
        //    lack that payload, so only they use the compact legacy fallback.
        let request = spawn_request.unwrap_or_else(|| SubagentSpawnRequest {
            subagent_type,
            prompt,
            context_paths: Vec::new(),
            description: None,
            model: None,
            model_profile: None,
            run_in_background: is_backgrounded,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            worktree: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: None,
            effort: None,
            tool_use_id: None,
            system_prompt_override: None,
            system_prompt_addendum: None,
            additional_disallowed_tools: Vec::new(),
            depth: 0,
            parent_model_override: None,
        });

        // 4. Preserve the immediate parent's registry/budget handles. Root
        //    handles are only the correct fallback for legacy direct tasks.
        let inherit = inheritance.unwrap_or_else(|| SubagentInheritance {
            tool_invoker: self.tool_invoker.clone(),
            budget: self.budget.clone(),
        });

        // The isolation worktree `AgentTool` resolved for this agent (if any).
        // The BACKGROUND lifecycle owns the terminal keep/cleanup judgment —
        // claude-code hands `getWorktreeResult` to the detached task
        // (`AgentTool` returns `async_launched` immediately and must NOT clean
        // up at launch) — so the worker below runs it once the agent reaches a
        // terminal state. Requires the injected manager; without it the
        // worktree is left in place (conservative).
        let agent_worktree = request.worktree.clone();
        let worktree_manager = self.worktree_manager.clone();

        // 5. Drive the subagent to completion inside a runtime-spawned worker
        //    (engine code must not call tokio::spawn directly — D17). The worker
        //    awaits `spawner.spawn`, spools the terminal payload, reports the
        //    terminal status, and removes its own cancel record on exit.
        let spawner = self.spawner.clone();
        let status_sink = self.status_sink.clone();
        let workers = self.workers.clone();
        let output_manager = self.output_manager.clone();
        let worker_spool_path = spool_path.clone();
        let worker_task_id = task_id.clone();
        let streaming = self.streaming_spawner.clone();
        let agent_ids = self.agent_ids.clone();
        let worker: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            if is_backgrounded && streaming.is_some() {
                // ── PERSISTENT / resumable path (local_agent "comes to rest"). ──
                // The agent emits ONE Completed per turn-set, then the runner
                // PARKS awaiting the next message (delivered by `send_message` →
                // `StreamingSubagentSpawner::resume`). Each rest appends to the
                // spool and KEEPS the task alive (status stays Running → not
                // evicted). Terminal only on Failed / Killed / channel-close.
                let streaming = streaming.expect("is_some checked");
                Box::pin(async move {
                    status_sink
                        .set_status(&worker_task_id, TaskStatus::Running)
                        .await;
                    let (agent_id, mut rx) =
                        match streaming.spawn_persistent(request, inherit).await {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = output_manager
                                    .append(&worker_spool_path, &e.to_string())
                                    .await;
                                status_sink
                                    .set_status(&worker_task_id, TaskStatus::Failed)
                                    .await;
                                // Terminal (spawn never ran): judge the carried
                                // isolation worktree so it never leaks.
                                if let (Some(mgr), Some(handle)) =
                                    (&worktree_manager, &agent_worktree)
                                {
                                    let _ = traits::worktree::agent_worktree_result(
                                        mgr.as_ref(),
                                        handle,
                                    )
                                    .await;
                                }
                                workers.lock().await.remove(&worker_task_id);
                                return;
                            }
                        };
                    // Register the live agent id so `send_message` can resume it.
                    agent_ids
                        .lock()
                        .await
                        .insert(worker_task_id.clone(), agent_id);
                    loop {
                        match rx.recv().await {
                            Some(SubagentEvent::Completed {
                                result,
                                usage,
                                total_tool_use_count,
                                total_duration_ms,
                                ..
                            }) => {
                                let body = serde_json::to_string_pretty(&result)
                                    .unwrap_or_else(|_| result.to_string());
                                let bt = usage.billable_tokens;
                                let total = bt
                                    .input
                                    .saturating_add(bt.cache_write)
                                    .saturating_add(bt.cache_read)
                                    .saturating_add(bt.output);
                                let body = format!(
                                    "{body}\n<usage><total_tokens>{total}</total_tokens></usage>\n"
                                );
                                let _ = output_manager.append(&worker_spool_path, &body).await;
                                // The `<result>` is the agent's final-text response
                                // — binary `wc(ne.content,"\n")`, which the runner
                                // already exposes as the result's `text` field
                                // (`blocks.join("\n")`); NOT the JSON-pretty spool.
                                let rest_result = result
                                    .get("text")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned);
                                let rest_usage = Some(traits::task_registry::AgentRunUsage {
                                    subagent_tokens: total,
                                    tool_uses: total_tool_use_count,
                                    duration_ms: total_duration_ms,
                                });
                                // Came to rest — alive, awaiting the next message.
                                // Keep status non-terminal (Running) so the
                                // registry never evicts the resting agent, then
                                // arm a one-shot "came to rest" notification so
                                // the model learns a turn-set finished (the
                                // `run_in_background` "you will be notified"
                                // promise; re-armed on each subsequent rest).
                                status_sink
                                    .set_status(&worker_task_id, TaskStatus::Running)
                                    .await;
                                status_sink
                                    .notify_rest(&worker_task_id, rest_result, rest_usage)
                                    .await;
                            }
                            Some(SubagentEvent::Failed { error, .. }) => {
                                let _ = output_manager.append(&worker_spool_path, &error).await;
                                status_sink
                                    .set_status(&worker_task_id, TaskStatus::Failed)
                                    .await;
                                break;
                            }
                            Some(SubagentEvent::Killed { .. }) => {
                                status_sink
                                    .set_status(&worker_task_id, TaskStatus::Killed)
                                    .await;
                                break;
                            }
                            // Progress / Message: live streaming, not spooled here.
                            Some(_) => {}
                            None => {
                                status_sink
                                    .set_status(&worker_task_id, TaskStatus::Completed)
                                    .await;
                                break;
                            }
                        }
                    }
                    // Terminal (Failed / Killed / channel-close — NOT a rest):
                    // run the worktree keep/cleanup judgment (claude-code
                    // `getWorktreeResult`): keep when dirty/ahead, else remove.
                    if let (Some(mgr), Some(handle)) = (&worktree_manager, &agent_worktree) {
                        let _ =
                            traits::worktree::agent_worktree_result(mgr.as_ref(), handle).await;
                    }
                    agent_ids.lock().await.remove(&worker_task_id);
                    workers.lock().await.remove(&worker_task_id);
                })
            } else {
                Box::pin(async move {
                    status_sink
                        .set_status(&worker_task_id, TaskStatus::Running)
                        .await;

                    let result = spawner.spawn(request, inherit).await;

                    // Map the terminal SubagentResult onto a spool payload + status.
                    // Spool I/O is best-effort — a spool write failure must not mask
                    // the subagent result (mirrors local_bash). The shared
                    // `TaskStatusSink` has no token-usage method (it is defined in
                    // `local_bash`, which this handler must not modify), so token usage
                    // is surfaced by spooling a `<usage><total_tokens>…` footer —
                    // byte-aligned with the TS `registerAsyncAgent` notification shape.
                    let (payload, status) = match &result {
                        Ok(SubagentResult::Completed { content, usage, .. }) => {
                            // Pretty-print the JSON payload; fall back to the compact
                            // Display form if serialization somehow fails.
                            let body = serde_json::to_string_pretty(content)
                                .unwrap_or_else(|_| content.to_string());
                            let body = format!(
                                "{body}\n<usage><total_tokens>{}</total_tokens></usage>\n",
                                usage.total_tokens
                            );
                            (body, TaskStatus::Completed)
                        }
                        Ok(SubagentResult::Failed { reason, .. }) => {
                            (reason.clone(), TaskStatus::Failed)
                        }
                        Ok(SubagentResult::Killed { .. }) => (String::new(), TaskStatus::Killed),
                        Err(e) => (e.to_string(), TaskStatus::Failed),
                    };

                    // Routed through the output manager's `append` so the per-file 5GB
                    // disk cap is enforced (T17). The write uses O_NOFOLLOW (claude-code
                    // `diskOutput.ts`) so a symlink planted at the spool path from inside
                    // the sandbox cannot redirect the write (T18).
                    if !payload.is_empty() {
                        let _ = output_manager.append(&worker_spool_path, &payload).await;
                    }

                    status_sink.set_status(&worker_task_id, status).await;

                    // Terminal: run the worktree keep/cleanup judgment on the
                    // carried isolation worktree (claude-code `getWorktreeResult`
                    // — keep when dirty/ahead, else auto-remove). Runs for ANY
                    // outcome so a worktree never leaks on a failed/killed agent.
                    if let (Some(mgr), Some(handle)) = (&worktree_manager, &agent_worktree) {
                        let _ =
                            traits::worktree::agent_worktree_result(mgr.as_ref(), handle).await;
                    }

                    // The subagent has terminated; drop the cancel record so a late
                    // kill is a graceful no-op (claude-code `status !== 'running'`).
                    workers.lock().await.remove(&worker_task_id);
                })
            };

        let bg_handle = ctx
            .runtime
            .spawn(&format!("{HANDLER_NAME}:{task_id}"), worker)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))?;

        // Record the worker-cancel handle (+ the runtime that minted it) so
        // kill / drain can cancel the in-flight worker without a fresh ctx.
        self.workers.lock().await.insert(
            task_id.clone(),
            WorkerCancel {
                handle: bg_handle,
                runtime: ctx.runtime.clone(),
            },
        );

        // 6. Build the synchronous cleanup seam (claude-code `registerCleanup`
        //    parity). The closure cannot await, so it moves any live cancel
        //    record into `pending_kill`; the async `drain_pending_kills`
        //    (called by the registry on agent teardown) performs the real
        //    `RuntimeSpawner::cancel`. Authoritative cancellation also flows
        //    through `Task::kill`.
        let cleanup_workers = self.workers.clone();
        let cleanup_pending = self.pending_kill.clone();
        let cleanup_task_id = task_id.clone();
        let cleanup: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let (Ok(mut workers), Ok(mut pending)) =
                (cleanup_workers.try_lock(), cleanup_pending.try_lock())
            {
                if let Some(rec) = workers.remove(&cleanup_task_id) {
                    pending.insert(cleanup_task_id.clone(), rec);
                }
                // If no live record remains the worker already exited; nothing
                // to queue (the terminal status was already reported).
            }
        });

        Ok(TaskHandle {
            task_id,
            cleanup: Some(cleanup),
        })
    }

    async fn kill(&self, task_id: &str, _ctx: TaskContext) -> Result<(), TaskError> {
        // Recover the live worker-cancel record (if any) and cancel the
        // in-flight worker future — the analogue of TS `abortController.abort()`
        // + `unregisterCleanup()`. An absent record ⇒ the subagent already
        // terminated ⇒ graceful no-op (claude-code `status !== 'running'`).
        let rec = self.workers.lock().await.remove(task_id);
        if let Some(rec) = rec {
            rec.runtime
                .cancel(&rec.handle)
                .await
                .map_err(|e| TaskError::Io(e.to_string()))?;
        }
        // Flip status to Killed regardless (best-effort; a worker that already
        // reported a terminal status simply gets a redundant Killed).
        self.status_sink
            .set_status(task_id, TaskStatus::Killed)
            .await;
        Ok(())
    }

    fn supports_messages(&self) -> bool {
        // A PERSISTENT (backgrounded + resumable) local_agent accepts messages:
        // `send_message` resumes a resting agent (claude-code `resumeAgentBackground`
        // / `injectUserMessageToTeammate`). Without the streaming seam wired, the
        // one-shot agent has no inbound channel → not supported.
        self.streaming_spawner.is_some()
    }

    /// Resume a resting persistent local_agent: deliver `message` to its parked
    /// runner (via [`StreamingSubagentSpawner::resume`] → `pool.send_event`),
    /// which appends it to history and runs the next turn-set — the
    /// `injectUserMessageToTeammate` / `resumeAgentBackground` analogue. Errors
    /// when the streaming seam is unwired or the agent has terminated/evicted.
    async fn send_message(
        &self,
        task_id: &str,
        message: String,
        _ctx: TaskContext,
    ) -> Result<(), TaskError> {
        let Some(streaming) = &self.streaming_spawner else {
            return Err(TaskError::Unsupported);
        };
        let agent_id = self
            .agent_ids
            .lock()
            .await
            .get(task_id)
            .copied()
            .ok_or(TaskError::TerminatedTask)?;
        streaming
            .resume(&agent_id, message)
            .await
            .map_err(|e| TaskError::Internal(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::TaskStatus;
    use serde_json::json;
    use std::any::Any;
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;
    use test_harness::mocks::MockRuntimeSpawner;
    use tokio::sync::Mutex as TokioMutex;
    use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use traits::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
    use traits::{BudgetError, SubagentSpawnError, SubagentUsage};

    // ---- In-memory FileSystem (mirrors local_bash test fixture) ------------

    struct InMemoryFs {
        files: TokioMutex<StdHashMap<String, String>>,
    }
    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(StdHashMap::new()),
            }
        }
    }
    #[async_trait]
    impl FileSystem for InMemoryFs {
        async fn read_file(
            &self,
            path: &str,
            offset: Option<u64>,
            limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            let map = self.files.lock().await;
            let content = map.get(path).cloned().unwrap_or_default();
            let off = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
            let body: String = content.chars().skip(off).collect();
            let truncated = limit.is_some_and(|lim| body.len() as u64 > lim);
            let trimmed = match limit {
                Some(lim) => body
                    .chars()
                    .take(usize::try_from(lim).unwrap_or(usize::MAX))
                    .collect(),
                None => body,
            };
            let total_lines = content.lines().count() as u64;
            Ok(FileContent {
                content: trimmed,
                truncated,
                total_lines,
            })
        }
        async fn write_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), body.to_string());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = FileEvent> + Send>>, FsError>
        {
            Err(FsError::Io("not supported".into()))
        }
        async fn append_file(&self, path: &str, body: &str) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            map.entry(path.to_string()).or_default().push_str(body);
            Ok(())
        }
        async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            if let Some(s) = map.get_mut(path) {
                s.truncate(usize::try_from(len).unwrap_or(usize::MAX));
            }
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("not supported".into()))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    // ---- Mock SubagentSpawner (returns a canned result; records inputs) ----

    enum CannedResult {
        Completed(serde_json::Value, u64),
        Failed(String),
        Killed,
        Err(String),
        /// Never resolves — used to keep the worker alive for the kill test.
        Pending,
    }

    struct MockSpawner {
        canned: StdMutex<Option<CannedResult>>,
        seen_request: StdMutex<Option<SubagentSpawnRequest>>,
        // Pointer identity of the inherited Arcs, captured for ptr_eq checks.
        seen_invoker: StdMutex<Option<Arc<dyn ToolInvoker>>>,
        seen_budget: StdMutex<Option<Arc<dyn BudgetEnforcerHandle>>>,
    }
    impl MockSpawner {
        fn new(canned: CannedResult) -> Arc<Self> {
            Arc::new(Self {
                canned: StdMutex::new(Some(canned)),
                seen_request: StdMutex::new(None),
                seen_invoker: StdMutex::new(None),
                seen_budget: StdMutex::new(None),
            })
        }
        fn request(&self) -> Option<SubagentSpawnRequest> {
            self.seen_request.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl SubagentSpawner for MockSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            *self.seen_request.lock().unwrap() = Some(request);
            *self.seen_invoker.lock().unwrap() = Some(inherit.tool_invoker.clone());
            *self.seen_budget.lock().unwrap() = Some(inherit.budget.clone());
            // Take the canned value out and DROP the guard before any await:
            // a `std::sync::MutexGuard` held across `.await` makes the future
            // `!Send`, but `SubagentSpawner::spawn` must return a `Send` future.
            let canned = self.canned.lock().unwrap().take();
            match canned {
                Some(CannedResult::Completed(content, total_tokens)) => {
                    Ok(SubagentResult::Completed {
                        agent_id: protocol::AgentId::new(),
                        content,
                        usage: SubagentUsage {
                            total_tokens,
                            ..Default::default()
                        },
                        total_tool_use_count: 0,
                        total_duration_ms: 0,
                        total_tokens,
                        assistant_message_count: 0,
                        response_char_count: 0,
                        last_request_id: None,
                    })
                }
                Some(CannedResult::Failed(reason)) => Ok(SubagentResult::Failed {
                    agent_id: protocol::AgentId::new(),
                    reason,
                }),
                Some(CannedResult::Killed) => Ok(SubagentResult::Killed {
                    agent_id: protocol::AgentId::new(),
                }),
                Some(CannedResult::Err(msg)) => Err(SubagentSpawnError::Runtime(msg)),
                Some(CannedResult::Pending) | None => {
                    // Park forever — the test cancels via Task::kill.
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }
    }

    // ---- Mock ToolInvoker / BudgetEnforcerHandle (inert) -------------------

    struct MockInvoker;
    #[async_trait]
    impl ToolInvoker for MockInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: serde_json::Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<serde_json::Value, ToolInvokerError> {
            Ok(json!(null))
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct MockBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for MockBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    // ---- Mock WorktreeManager (terminal keep/cleanup judgment) --------------

    /// Judgment-only mock: scripted change summary + a removal recorder.
    /// `create_worktree` is unreachable here — the handler never CREATES
    /// worktrees (AgentTool does, before dispatch); it only judges the one
    /// carried on `SubagentSpawnRequest::worktree`.
    struct RecordingWorktree {
        summary: Option<traits::worktree::WorktreeChangeSummary>,
        removed: StdMutex<Vec<traits::worktree::WorktreeHandle>>,
    }
    impl RecordingWorktree {
        fn new(summary: Option<traits::worktree::WorktreeChangeSummary>) -> Arc<Self> {
            Arc::new(Self {
                summary,
                removed: StdMutex::new(Vec::new()),
            })
        }
        fn removed_count(&self) -> usize {
            self.removed.lock().unwrap().len()
        }
    }
    #[async_trait]
    impl traits::worktree::WorktreeManager for RecordingWorktree {
        async fn create_worktree(
            &self,
            _slug: &str,
            _base_branch: Option<&str>,
            _copy_includes: &[PathBuf],
        ) -> Result<traits::worktree::WorktreeHandle, traits::worktree::WorktreeError> {
            Err(traits::worktree::WorktreeError::Unsupported)
        }
        async fn remove_worktree(
            &self,
            handle: &traits::worktree::WorktreeHandle,
        ) -> Result<(), traits::worktree::WorktreeError> {
            self.removed.lock().unwrap().push(handle.clone());
            Ok(())
        }
        async fn list_worktrees(
            &self,
        ) -> Result<Vec<traits::worktree::WorktreeInfo>, traits::worktree::WorktreeError> {
            Ok(Vec::new())
        }
        async fn cleanup_stale(
            &self,
            _max_age: std::time::Duration,
        ) -> Result<Vec<PathBuf>, traits::worktree::WorktreeError> {
            Ok(Vec::new())
        }
        fn is_supported(&self) -> bool {
            true
        }
        async fn worktree_change_summary(
            &self,
            _handle: &traits::worktree::WorktreeHandle,
        ) -> Result<Option<traits::worktree::WorktreeChangeSummary>, traits::worktree::WorktreeError>
        {
            Ok(self.summary)
        }
    }

    fn isolation_worktree_handle() -> traits::worktree::WorktreeHandle {
        traits::worktree::WorktreeHandle {
            path: PathBuf::from("/repo/.lingxi/worktrees/agent-1"),
            branch_name: "worktree-agent-1".into(),
            base_commit: None,
        }
    }

    /// A background Agent request carrying the isolation worktree the tool
    /// resolved before dispatch (the P1-01 ownership transfer).
    fn request_with_worktree(prompt: &str) -> SubagentSpawnRequest {
        SubagentSpawnRequest {
            subagent_type: "general-purpose".into(),
            prompt: prompt.into(),
            context_paths: Vec::new(),
            description: None,
            model: None,
            model_profile: None,
            run_in_background: true,
            name: None,
            team_name: None,
            mode: None,
            isolation: Some("worktree".into()),
            cwd: Some("/repo/.lingxi/worktrees/agent-1".into()),
            worktree: Some(isolation_worktree_handle()),
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
        }
    }

    fn input_with_worktree(prompt: &str) -> TaskSpawnInput {
        TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            subagent_type: "general-purpose".into(),
            prompt: prompt.into(),
            is_backgrounded: true,
            tool_use_id: None,
            spawn_request: Some(request_with_worktree(prompt)),
            inheritance: None,
        }
    }

    // ---- Recording status sink ---------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<(String, TaskStatus)>>,
        rest_count: StdMutex<usize>,
        last_rest: StdMutex<Option<(Option<String>, Option<traits::task_registry::AgentRunUsage>)>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, task_id: &str, status: TaskStatus) {
            self.statuses
                .lock()
                .unwrap()
                .push((task_id.to_string(), status));
        }
        async fn notify_rest(
            &self,
            _task_id: &str,
            result: Option<String>,
            usage: Option<traits::task_registry::AgentRunUsage>,
        ) {
            *self.rest_count.lock().unwrap() += 1;
            *self.last_rest.lock().unwrap() = Some((result, usage));
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().map(|(_, s)| *s)
        }
        fn rest_count(&self) -> usize {
            *self.rest_count.lock().unwrap()
        }
    }

    // ---- Helpers ------------------------------------------------------------

    fn make_ctx(fs: Arc<dyn FileSystem>) -> TaskContext {
        TaskContext {
            fs,
            runtime: Arc::new(MockRuntimeSpawner::default()),
        }
    }

    fn make_output_manager(fs: Arc<dyn FileSystem>) -> (tempfile::TempDir, Arc<TaskOutputManager>) {
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs));
        (dir, mgr)
    }

    fn make_handler(
        spawner: Arc<dyn SubagentSpawner>,
        mgr: Arc<TaskOutputManager>,
        sink: Arc<dyn TaskStatusSink>,
    ) -> LocalAgentHandler {
        LocalAgentHandler::new(spawner, Arc::new(MockInvoker), Arc::new(MockBudget), mgr)
            .with_status_sink(sink)
    }

    fn local_agent_input(prompt: &str) -> TaskSpawnInput {
        TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            subagent_type: "general-purpose".into(),
            prompt: prompt.into(),
            is_backgrounded: true,
            tool_use_id: None,
            spawn_request: None,
            inheritance: None,
        }
    }

    /// Poll the sink until it reports a terminal status (the worker runs on the
    /// `MockRuntimeSpawner`'s tokio task, so yields let it finish deterministically).
    async fn await_terminal(sink: &Arc<RecordingSink>) -> TaskStatus {
        for _ in 0..200 {
            if let Some(s) = sink.last_status() {
                if s.is_terminal() {
                    return s;
                }
            }
            tokio::task::yield_now().await;
        }
        sink.last_status().expect("worker never reported a status")
    }

    // ---- Mock StreamingSubagentSpawner (persistent / resume seam) -----------

    /// A persistent-spawner mock whose outbound event channel the TEST drives:
    /// `spawn_persistent` stashes the `Sender` in `tx_slot` (so the test can
    /// push per-turn-set `Completed`/`Failed`/… events), and `resume` bumps a
    /// counter the test asserts on.
    struct MockStreamingSpawner {
        tx_slot: Arc<StdMutex<Option<tokio::sync::mpsc::Sender<SubagentEvent>>>>,
        resume_count: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl StreamingSubagentSpawner for MockStreamingSpawner {
        async fn spawn_persistent(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<(AgentId, tokio::sync::mpsc::Receiver<SubagentEvent>), SubagentSpawnError>
        {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            *self.tx_slot.lock().unwrap() = Some(tx);
            Ok((AgentId::new(), rx))
        }
        async fn resume(
            &self,
            _agent_id: &AgentId,
            _message: String,
        ) -> Result<(), SubagentSpawnError> {
            self.resume_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    fn completed_event(marker: &str) -> SubagentEvent {
        SubagentEvent::Completed {
            agent_id: AgentId::new(),
            result: json!({ "marker": marker }),
            usage: llm_client::Usage::default(),
            total_tool_use_count: 0,
            total_duration_ms: 0,
            assistant_message_count: 0,
            last_request_id: None,
        }
    }

    async fn await_spool_contains(
        mgr: &Arc<TaskOutputManager>,
        path: &std::path::Path,
        needle: &str,
    ) {
        for _ in 0..200 {
            if let Ok(read) = mgr
                .read(path, crate::output_manager::OutputOptions::default())
                .await
            {
                if read.content.contains(needle) {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("spool never contained {needle:?}");
    }

    // ---- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn persistent_agent_rests_after_each_turn_set_and_resumes_on_message() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());
        let tx_slot: Arc<StdMutex<Option<tokio::sync::mpsc::Sender<SubagentEvent>>>> =
            Arc::new(StdMutex::new(None));
        let resume_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let streaming = Arc::new(MockStreamingSpawner {
            tx_slot: tx_slot.clone(),
            resume_count: resume_count.clone(),
        });

        // The one-shot spawner is present but UNUSED on the persistent path.
        let handler = make_handler(
            MockSpawner::new(CannedResult::Pending),
            mgr.clone(),
            sink.clone(),
        )
        .with_streaming_spawner(streaming);
        assert!(
            handler.supports_messages(),
            "streaming seam ⇒ messages supported"
        );

        let ctx = make_ctx(fs);
        let handle = handler
            .spawn(local_agent_input("start"), ctx.clone())
            .await
            .expect("spawn should succeed");
        let task_id = handle.task_id.clone();

        // Wait for the persistent worker to call spawn_persistent (stashes tx).
        let tx = {
            let mut got = None;
            for _ in 0..200 {
                if let Some(t) = tx_slot.lock().unwrap().clone() {
                    got = Some(t);
                    break;
                }
                tokio::task::yield_now().await;
            }
            got.expect("spawn_persistent should have run")
        };

        // ── Turn-set 1: Completed ⇒ spool + COME TO REST (status Running). ──
        tx.send(completed_event("first")).await.unwrap();
        let spool_path = dir.path().join(format!("{task_id}.output"));
        await_spool_contains(&mgr, &spool_path, "first").await;
        // Rested, NOT terminal — still alive awaiting the next message.
        assert_eq!(
            sink.last_status(),
            Some(TaskStatus::Running),
            "agent comes to rest (alive)"
        );
        assert_eq!(sink.rest_count(), 1, "first rest armed a notification");

        // ── Resume via send_message → the runner wakes for turn-set 2. ──
        handler
            .send_message(&task_id, "again".into(), ctx)
            .await
            .expect("resume ok");
        assert_eq!(
            resume_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "send_message routed to resume"
        );

        // ── Turn-set 2: another Completed ⇒ second spool + rest again. ──
        tx.send(completed_event("second")).await.unwrap();
        await_spool_contains(&mgr, &spool_path, "second").await;
        assert_eq!(
            sink.last_status(),
            Some(TaskStatus::Running),
            "rests again after the second turn-set"
        );
        assert_eq!(
            sink.rest_count(),
            2,
            "second rest re-armed the notification"
        );

        // ── Channel close ⇒ the agent terminates (final Completed). ──
        // Drop EVERY Sender — the one the slot still holds plus our handle.
        tx_slot.lock().unwrap().take();
        drop(tx);
        let status = await_terminal(&sink).await;
        assert_eq!(
            status,
            TaskStatus::Completed,
            "channel close ⇒ terminal Completed"
        );
    }

    #[tokio::test]
    async fn send_message_requires_seam_and_live_agent() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());
        let ctx = make_ctx(fs);

        // No streaming seam ⇒ one-shot ⇒ messaging unsupported.
        let one_shot = make_handler(
            MockSpawner::new(CannedResult::Pending),
            mgr.clone(),
            sink.clone(),
        );
        assert!(!one_shot.supports_messages());
        assert!(matches!(
            one_shot.send_message("any", "hi".into(), ctx.clone()).await,
            Err(TaskError::Unsupported)
        ));

        // Seam wired but no live agent for that id ⇒ TerminatedTask.
        let streaming = Arc::new(MockStreamingSpawner {
            tx_slot: Arc::new(StdMutex::new(None)),
            resume_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let persistent = make_handler(MockSpawner::new(CannedResult::Pending), mgr, sink)
            .with_streaming_spawner(streaming);
        assert!(persistent.supports_messages());
        assert!(matches!(
            persistent.send_message("ghost", "hi".into(), ctx).await,
            Err(TaskError::TerminatedTask)
        ));
    }

    #[tokio::test]
    async fn spawn_runs_subagent_and_spools_completed_content() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Completed(
            json!({ "answer": "42", "ok": true }),
            1234,
        ));
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner.clone(), mgr.clone(), sink.clone());

        let handle = handler
            .spawn(local_agent_input("do the thing"), make_ctx(fs))
            .await
            .expect("spawn should succeed");

        assert!(
            handle.task_id.starts_with('a'),
            "LocalAgent id prefix is 'a'"
        );
        assert!(handle.cleanup.is_some(), "cleanup seam is present");

        let status = await_terminal(&sink).await;
        assert_eq!(
            status,
            TaskStatus::Completed,
            "Completed result ⇒ Completed"
        );

        // The request carried the resolved subagent_type + prompt, empty context.
        let req = spawner.request().expect("spawner must have been called");
        assert_eq!(
            req.subagent_type, "general-purpose",
            "default type fallback"
        );
        assert_eq!(req.prompt, "do the thing");
        assert!(req.context_paths.is_empty());

        // The Completed content was spooled (pretty JSON ⇒ contains the keys)
        // along with the token-usage footer.
        let spool_path = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(read.content.contains("\"answer\""), "content key spooled");
        assert!(read.content.contains("42"), "content value spooled");
        assert!(
            read.content.contains("<total_tokens>1234</total_tokens>"),
            "token usage spooled in the usage footer"
        );
    }

    #[tokio::test]
    async fn failed_result_maps_to_failed_and_spools_reason() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Failed("model refused".into()));
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr.clone(), sink.clone());

        let handle = handler
            .spawn(local_agent_input("p"), make_ctx(fs))
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);

        let spool_path = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(read.content.contains("model refused"), "reason spooled");

        // No usage footer on the failed path.
        assert!(!read.content.contains("<total_tokens>"));
    }

    #[tokio::test]
    async fn killed_result_maps_to_killed() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Killed);
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr, sink.clone());

        handler
            .spawn(local_agent_input("p"), make_ctx(fs))
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Killed);
    }

    #[tokio::test]
    async fn spawn_error_maps_to_failed_and_spools_error() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Err("pool full".into()));
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr.clone(), sink.clone());

        let handle = handler
            .spawn(local_agent_input("p"), make_ctx(fs))
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);

        let spool_path = dir.path().join(format!("{}.output", handle.task_id));
        let read = mgr
            .read(&spool_path, crate::output_manager::OutputOptions::default())
            .await
            .unwrap();
        assert!(read.content.contains("pool full"), "spawn error spooled");
    }

    /// Poll until the handler's live worker map is empty — the worker removes
    /// its own record LAST (after the terminal status + worktree judgment), so
    /// an empty map means the judgment definitely ran (or was skipped).
    async fn await_workers_drained(workers: &Arc<TokioMutex<StdHashMap<String, WorkerCancel>>>) {
        for _ in 0..400 {
            if workers.lock().await.is_empty() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("worker never drained its cancel record");
    }

    /// P1-01 (parity 2.1.207): the BACKGROUND lifecycle owns the isolation
    /// worktree's terminal keep/cleanup judgment (claude hands its
    /// `getWorktreeResult` closure to the detached task). A CLEAN worktree is
    /// auto-removed once the worker reaches a terminal state.
    #[tokio::test]
    async fn terminal_background_agent_removes_clean_worktree() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Completed(json!("ok"), 0));
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());
        let wt = RecordingWorktree::new(Some(traits::worktree::WorktreeChangeSummary {
            changed_files: 0,
            commits: 0,
        }));
        let handler = make_handler(spawner, mgr, sink.clone())
            .with_worktree_manager(wt.clone() as Arc<dyn traits::worktree::WorktreeManager>);
        let workers = handler.workers_map();

        handler
            .spawn(input_with_worktree("p"), make_ctx(fs))
            .await
            .unwrap();

        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);
        await_workers_drained(&workers).await;
        assert_eq!(
            wt.removed_count(),
            1,
            "clean worktree auto-removed at terminal"
        );
    }

    /// P1-01: a DIRTY worktree (uncommitted files or commits ahead) is KEPT —
    /// the terminal judgment must never discard work.
    #[tokio::test]
    async fn terminal_background_agent_keeps_dirty_worktree() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Failed("model refused".into()));
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());
        let wt = RecordingWorktree::new(Some(traits::worktree::WorktreeChangeSummary {
            changed_files: 2,
            commits: 1,
        }));
        let handler = make_handler(spawner, mgr, sink.clone())
            .with_worktree_manager(wt.clone() as Arc<dyn traits::worktree::WorktreeManager>);
        let workers = handler.workers_map();

        handler
            .spawn(input_with_worktree("p"), make_ctx(fs))
            .await
            .unwrap();

        // Runs for ANY terminal outcome (here: Failed) — but a dirty worktree
        // is kept, not removed.
        assert_eq!(await_terminal(&sink).await, TaskStatus::Failed);
        await_workers_drained(&workers).await;
        assert_eq!(wt.removed_count(), 0, "dirty worktree KEPT at terminal");
    }

    /// P1-01: on the PERSISTENT path the judgment runs ONLY at a terminal
    /// state — a "comes to rest" turn-set completion must NOT remove the
    /// worktree (the agent is still alive and resumable in it).
    #[tokio::test]
    async fn persistent_agent_worktree_judged_only_at_terminal() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());
        let tx_slot: Arc<StdMutex<Option<tokio::sync::mpsc::Sender<SubagentEvent>>>> =
            Arc::new(StdMutex::new(None));
        let streaming = Arc::new(MockStreamingSpawner {
            tx_slot: tx_slot.clone(),
            resume_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let wt = RecordingWorktree::new(Some(traits::worktree::WorktreeChangeSummary {
            changed_files: 0,
            commits: 0,
        }));
        let handler = make_handler(
            MockSpawner::new(CannedResult::Pending),
            mgr,
            sink.clone(),
        )
        .with_streaming_spawner(streaming)
        .with_worktree_manager(wt.clone() as Arc<dyn traits::worktree::WorktreeManager>);
        let workers = handler.workers_map();

        handler
            .spawn(input_with_worktree("p"), make_ctx(fs))
            .await
            .unwrap();

        // Wait for spawn_persistent to stash the event sender.
        let tx = {
            let mut got = None;
            for _ in 0..200 {
                if let Some(t) = tx_slot.lock().unwrap().clone() {
                    got = Some(t);
                    break;
                }
                tokio::task::yield_now().await;
            }
            got.expect("spawn_persistent should have run")
        };

        // Turn-set completes → the agent comes to REST (not terminal): the
        // worktree must survive (the resting agent still works in it).
        tx.send(completed_event("rest")).await.unwrap();
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(wt.removed_count(), 0, "no judgment at a rest");

        // Channel close ⇒ terminal ⇒ the clean worktree is auto-removed.
        tx_slot.lock().unwrap().take();
        drop(tx);
        assert_eq!(await_terminal(&sink).await, TaskStatus::Completed);
        await_workers_drained(&workers).await;
        assert_eq!(
            wt.removed_count(),
            1,
            "clean worktree auto-removed at terminal"
        );
    }

    #[tokio::test]
    async fn kill_cancels_inflight_worker_and_flips_to_killed() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        // The spawner parks forever, so the worker stays alive until kill.
        let spawner = MockSpawner::new(CannedResult::Pending);
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr, sink.clone());
        let workers = handler.workers_map();

        let handle = handler
            .spawn(local_agent_input("p"), make_ctx(fs.clone()))
            .await
            .unwrap();

        // Let the worker reach its parked await and the Running status land.
        for _ in 0..50 {
            if !workers.lock().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            workers.lock().await.contains_key(&handle.task_id),
            "worker-cancel record present while subagent runs"
        );

        handler
            .kill(&handle.task_id, make_ctx(fs))
            .await
            .expect("kill should succeed");

        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
        assert!(
            !workers.lock().await.contains_key(&handle.task_id),
            "cancel record removed by kill"
        );
    }

    #[tokio::test]
    async fn kill_absent_task_is_graceful_noop() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Killed);
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr, sink.clone());

        // No spawn ⇒ no worker record. kill must still succeed and flip Killed.
        handler
            .kill("adeadbeef", make_ctx(fs))
            .await
            .expect("kill of unknown task is a no-op success");
        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
    }

    #[tokio::test]
    async fn cleanup_then_drain_cancels_inflight_worker() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Pending);
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner, mgr, sink.clone());
        let workers = handler.workers_map();

        let handle = handler
            .spawn(local_agent_input("p"), make_ctx(fs))
            .await
            .unwrap();

        // Wait for the worker-cancel record to be present.
        for _ in 0..50 {
            if !workers.lock().await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        // Fire the synchronous cleanup closure (moves record to pending_kill).
        (handle.cleanup.as_ref().unwrap())();

        // The record left the live map.
        assert!(
            !workers.lock().await.contains_key(&handle.task_id),
            "cleanup moved the record out of the live map"
        );

        // Drain performs the real async cancel + flips status.
        handler.drain_pending_kills().await;
        assert_eq!(sink.last_status(), Some(TaskStatus::Killed));
    }

    #[tokio::test]
    async fn variant_subagent_type_drives_the_request() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Completed(json!("ok"), 0));
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner.clone(), mgr, sink.clone());

        // A non-default subagent_type on the variant must flow through verbatim
        // (parity with TS `agentType` — no AgentId-to-type derivation).
        let input = TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            subagent_type: "code-reviewer".into(),
            prompt: "p".into(),
            is_backgrounded: true,
            tool_use_id: None,
            spawn_request: None,
            inheritance: None,
        };

        handler.spawn(input, make_ctx(fs)).await.unwrap();

        await_terminal(&sink).await;
        assert_eq!(
            spawner.request().unwrap().subagent_type,
            "code-reviewer",
            "the variant's subagent_type drives the request verbatim"
        );
    }

    /// Background Agent invocations arrive through `TaskSpawnInput` after the
    /// Agent tool has already resolved every override. The task boundary must
    /// not reconstruct a stripped request or replace the immediate parent's
    /// inheritance handles with composition-root defaults.
    #[tokio::test]
    async fn full_background_request_and_inheritance_survive_task_boundary() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Completed(json!("ok"), 0));
        let (_dir, mgr) = make_output_manager(fs.clone());
        let sink = Arc::new(RecordingSink::default());
        let handler = make_handler(spawner.clone(), mgr, sink.clone());

        let inherited_invoker: Arc<dyn ToolInvoker> = Arc::new(MockInvoker);
        let inherited_budget: Arc<dyn BudgetEnforcerHandle> = Arc::new(MockBudget);
        let expected = SubagentSpawnRequest {
            subagent_type: "code-reviewer".into(),
            prompt: "inspect the background request".into(),
            context_paths: vec![PathBuf::from("/workspace/CONTEXT.md")],
            description: Some("review request".into()),
            model: Some("opus".into()),
            model_profile: Some("preferred-provider".into()),
            run_in_background: true,
            name: Some("reviewer".into()),
            team_name: Some("team-a".into()),
            mode: Some("plan".into()),
            isolation: Some("worktree".into()),
            cwd: Some("/workspace/subdir".into()),
            worktree: None,
            fork_context_messages: None,
            fork_parent_system_prompt: None,
            schema: Some(r#"{\"type\":\"object\"}"#.into()),
            effort: Some(json!("high")),
            tool_use_id: Some("toolu_background".into()),
            system_prompt_override: Some("override".into()),
            system_prompt_addendum: Some("addendum".into()),
            additional_disallowed_tools: vec!["Bash".into()],
            depth: 3,
            parent_model_override: Some("claude-opus-4-6".into()),
        };
        let input = TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            // These compact task-index fields deliberately disagree with the
            // full request so forwarding the stripped legacy reconstruction is
            // observable.
            subagent_type: "general-purpose".into(),
            prompt: "legacy prompt".into(),
            is_backgrounded: true,
            tool_use_id: Some("toolu_background".into()),
            spawn_request: Some(expected.clone()),
            inheritance: Some(SubagentInheritance {
                tool_invoker: inherited_invoker.clone(),
                budget: inherited_budget.clone(),
            }),
        };

        handler.spawn(input, make_ctx(fs)).await.unwrap();
        await_terminal(&sink).await;

        let observed = spawner.request().expect("worker received a request");
        assert_eq!(observed, expected, "all Agent spawn overrides survive");
        assert!(Arc::ptr_eq(
            spawner
                .seen_invoker
                .lock()
                .unwrap()
                .as_ref()
                .expect("worker received inheritance"),
            &inherited_invoker,
        ));
        assert!(Arc::ptr_eq(
            spawner
                .seen_budget
                .lock()
                .unwrap()
                .as_ref()
                .expect("worker received inheritance"),
            &inherited_budget,
        ));
    }

    #[tokio::test]
    async fn non_localagent_input_is_rejected() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Killed);
        let (_dir, mgr) = make_output_manager(fs.clone());

        let handler = make_handler(spawner, mgr, Arc::new(RecordingSink::default()));

        let result = handler
            .spawn(
                TaskSpawnInput::LocalWorkflow {
                    workflow_id: "wf".into(),
                    script: String::new(),
                    resume_from_run_id: None,
                    args: None,
                    run_id: None,
                    invocation_mode: None,
                    workflow_source: None,
                    launched_from_subagent: false,
                },
                make_ctx(fs),
            )
            .await;
        match result {
            Err(TaskError::Internal(_)) => {}
            Err(other) => panic!("expected Internal, got {other:?}"),
            Ok(_) => panic!("non-LocalAgent input must be rejected"),
        }
    }

    #[test]
    fn name_and_type_are_local_agent() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Killed);
        let (_dir, mgr) = make_output_manager(fs);
        let handler = make_handler(spawner, mgr, Arc::new(RecordingSink::default()));
        assert_eq!(handler.name(), "local_agent");
        assert_eq!(handler.task_type(), TaskType::LocalAgent);
        assert!(!handler.supports_messages());
    }
}
