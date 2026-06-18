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
//! `selectedAgent.agentType ?? 'general-purpose'`), so this handler forwards
//! the value verbatim into [`SubagentSpawnRequest`] with no `AgentId`-to-type
//! derivation. `context_paths` has no source on the variant, so [`Vec::new`] is
//! passed.

use crate::id::TaskType;
use crate::output_manager::TaskOutputManager;
use crate::state::TaskStatus;
use crate::task_trait::{Task, TaskContext, TaskError, TaskHandle, TaskSpawnInput};
use async_trait::async_trait;
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
            tool_invoker,
            budget,
            output_manager,
            status_sink: Arc::new(NoopStatusSink),
            workers: Arc::new(Mutex::new(HashMap::new())),
            pending_kill: Arc::new(Mutex::new(HashMap::new())),
        }
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
            self.status_sink.set_status(&task_id, TaskStatus::Killed).await;
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

        // 3. Build the spawn request, forwarding the already-resolved
        //    `subagent_type` verbatim (parity with TS `agentType`).
        //    `context_paths` has no source on the variant, so an empty vec is
        //    passed.
        let request = SubagentSpawnRequest {
            subagent_type,
            prompt,
            context_paths: Vec::new(),
            // AgentTool spawn-surface parity params — the LocalAgent variant
            // carries no model/name/etc. overrides, so those default to None.
            description: None,
            model: None,
            // The LocalAgent variant IS the background path; wire its real
            // `is_backgrounded` flag onto the spawn request's `run_in_background`.
            run_in_background: is_backgrounded,
            name: None,
            team_name: None,
            mode: None,
            isolation: None,
            cwd: None,
            // Non-fork spawn.
            fork_context_messages: None,
            fork_parent_system_prompt: None,
        };

        // 4. Bundle the inheritance. Cloning the Arcs preserves pointer
        //    identity — required by the recursion-lock + budget-aggregation
        //    invariants (the spawner asserts `Arc::ptr_eq` on these).
        let inherit = SubagentInheritance {
            tool_invoker: self.tool_invoker.clone(),
            budget: self.budget.clone(),
        };

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
        let worker = Box::pin(async move {
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
                Ok(SubagentResult::Failed { reason, .. }) => (reason.clone(), TaskStatus::Failed),
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

            // The subagent has terminated; drop the cancel record so a late
            // kill is a graceful no-op (claude-code `status !== 'running'`).
            workers.lock().await.remove(&worker_task_id);
        });

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
        self.status_sink.set_status(task_id, TaskStatus::Killed).await;
        Ok(())
    }

    fn supports_messages(&self) -> bool {
        // The TS `LocalAgentTask` object exposes no `send_message`; mid-turn
        // messaging there is a separate `queuePendingMessage` path, out of
        // scope for the handler trait.
        false
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
    use traits::{
        BudgetError, SubagentSpawnError, SubagentUsage,
    };

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

    // ---- Recording status sink ---------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        statuses: StdMutex<Vec<(String, TaskStatus)>>,
    }
    #[async_trait]
    impl TaskStatusSink for RecordingSink {
        async fn set_status(&self, task_id: &str, status: TaskStatus) {
            self.statuses
                .lock()
                .unwrap()
                .push((task_id.to_string(), status));
        }
    }
    impl RecordingSink {
        fn last_status(&self) -> Option<TaskStatus> {
            self.statuses.lock().unwrap().last().map(|(_, s)| *s)
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
        LocalAgentHandler::new(
            spawner,
            Arc::new(MockInvoker),
            Arc::new(MockBudget),
            mgr,
        )
        .with_status_sink(sink)
    }

    fn local_agent_input(prompt: &str) -> TaskSpawnInput {
        TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::new(),
            subagent_type: "general-purpose".into(),
            prompt: prompt.into(),
            is_backgrounded: true,
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

    // ---- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn spawn_runs_subagent_and_spools_completed_content() {
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let spawner = MockSpawner::new(CannedResult::Completed(
            json!({ "answer": "42", "ok": true }),
            1234,
        ));
        let dir = tempdir().unwrap();
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
        let sink = Arc::new(RecordingSink::default());

        let handler = make_handler(spawner.clone(), mgr.clone(), sink.clone());

        let handle = handler
            .spawn(local_agent_input("do the thing"), make_ctx(fs))
            .await
            .expect("spawn should succeed");

        assert!(handle.task_id.starts_with('a'), "LocalAgent id prefix is 'a'");
        assert!(handle.cleanup.is_some(), "cleanup seam is present");

        let status = await_terminal(&sink).await;
        assert_eq!(status, TaskStatus::Completed, "Completed result ⇒ Completed");

        // The request carried the resolved subagent_type + prompt, empty context.
        let req = spawner.request().expect("spawner must have been called");
        assert_eq!(req.subagent_type, "general-purpose", "default type fallback");
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
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
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
        let mgr = Arc::new(TaskOutputManager::new(PathBuf::from(dir.path()), fs.clone()));
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
        };

        handler.spawn(input, make_ctx(fs)).await.unwrap();

        await_terminal(&sink).await;
        assert_eq!(
            spawner.request().unwrap().subagent_type,
            "code-reviewer",
            "the variant's subagent_type drives the request verbatim"
        );
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
