//! `TaskRegistryHandle` trait impl for `TaskRegistry`.
//!
//! Bridges the `lingxi-traits` seam (string-typed wire format) to the
//! concrete `TaskRegistry` (typed `TaskType` + `TaskStatus`) so the six
//! `Task*` tools in `lingxi-tools` can dispatch without taking a cyclic dep
//! on `lingxi-tasks`.
//!
//! Task-type and task-status wire strings here are byte-aligned with the
//! locked constants in `lingxi-tools::builtin::task` (`TASK_TYPES` /
//! `TASK_STATUSES`).

use crate::id::TaskType;
use crate::registry::TaskRegistry;
use crate::state::{TaskState, TaskStatus};
use crate::task_trait::{TaskError, TaskSpawnInput};
use async_trait::async_trait;
use traits::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};

fn task_type_from_wire(s: &str) -> Result<TaskType, TaskRegistryError> {
    Ok(match s {
        "local_bash" => TaskType::LocalBash,
        "local_agent" => TaskType::LocalAgent,
        "remote_agent" => TaskType::RemoteAgent,
        "in_process_teammate" => TaskType::InProcessTeammate,
        "local_workflow" => TaskType::LocalWorkflow,
        "monitor_mcp" => TaskType::MonitorMcp,
        "dream" => TaskType::Dream,
        other => {
            return Err(TaskRegistryError::InvalidInput(format!(
                "unknown task_type '{other}'"
            )))
        }
    })
}

pub(crate) fn task_type_to_wire(t: TaskType) -> &'static str {
    match t {
        TaskType::LocalBash => "local_bash",
        TaskType::LocalAgent => "local_agent",
        TaskType::RemoteAgent => "remote_agent",
        TaskType::InProcessTeammate => "in_process_teammate",
        TaskType::LocalWorkflow => "local_workflow",
        TaskType::MonitorMcp => "monitor_mcp",
        TaskType::Dream => "dream",
    }
}

fn status_from_wire(s: &str) -> Result<TaskStatus, TaskRegistryError> {
    Ok(match s {
        "pending" => TaskStatus::Pending,
        "running" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "killed" => TaskStatus::Killed,
        other => {
            return Err(TaskRegistryError::InvalidInput(format!(
                "unknown status '{other}'"
            )))
        }
    })
}

pub(crate) fn status_to_wire(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Killed => "killed",
    }
}

fn state_to_record(s: &TaskState) -> TaskRecord {
    let b = s.base();
    // Mirror claude-code `LocalShellTaskState.command`: only `local_bash` tasks
    // carry a shell command. `TaskStop` prefers `command` over `description` for
    // `local_bash` (`stopTask.ts:97`); every other task type reports `None`.
    let command = match s {
        TaskState::LocalBash(bash) => Some(bash.command.clone()),
        _ => None,
    };
    TaskRecord {
        task_id: b.id.clone(),
        task_type: task_type_to_wire(b.task_type).to_string(),
        status: status_to_wire(b.status).to_string(),
        description: b.description.clone(),
        command,
    }
}

/// Port of `extractTextContent(blocks, '\n')` (claude-code
/// `utils/messages.ts:2893-2901`): filter the content blocks whose `type` is
/// `"text"` and join their `text` fields with `\n`.
///
/// The agent's final-message `content` arrives here as a `serde_json::Value`
/// (the spooled `SubagentResult::Completed.content`). A non-array `content`
/// (e.g. a bare string the spawner returned) yields no `text` blocks → `""`,
/// exactly mirroring the TS `.filter(...).map(...).join(...)` over a typed
/// block array.
fn extract_text_content(content: &serde_json::Value) -> String {
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Recover the agent's final-message content `Value` from a `local_agent`
/// spool. The `local_agent` handler writes
/// `to_string_pretty(content)` + `"\n<usage><total_tokens>N</total_tokens></usage>\n"`
/// (`handlers/local_agent.rs`). Strip the trailing `<usage>…</usage>` footer
/// (if present) and JSON-parse the remainder back into the original `content`
/// `Value`, so `extract_text_content` can pull the clean answer out of it.
/// Returns `None` when the spool is empty or does not parse (e.g. a `Failed`
/// reason string), in which case the caller falls back to the raw output.
fn agent_content_from_spool(spool: &str) -> Option<serde_json::Value> {
    let trimmed = spool.trim_end();
    // Drop the appended `<usage>…</usage>` footer if it is present, keeping the
    // pretty-printed JSON body that precedes it.
    let body = match trimmed.rfind("\n<usage>") {
        Some(idx) => &trimmed[..idx],
        None => trimmed,
    };
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(body).ok()
}

fn task_err_to_registry_err(e: TaskError) -> TaskRegistryError {
    match e {
        TaskError::NotFound(id) => TaskRegistryError::NotFound(id),
        TaskError::UnknownType => TaskRegistryError::InvalidInput("unknown task type".into()),
        TaskError::TerminatedTask => TaskRegistryError::Internal("task already terminated".into()),
        TaskError::Unsupported => TaskRegistryError::Internal("unsupported".into()),
        TaskError::Io(s) => TaskRegistryError::Internal(format!("io: {s}")),
        TaskError::Internal(s) => TaskRegistryError::Internal(s),
    }
}

fn placeholder_input(task_type: TaskType) -> TaskSpawnInput {
    // Most spawn inputs require fields a `TaskCreate` JSON call cannot supply
    // (e.g. `LocalBash` needs `command`). The registry's `create` only
    // allocates the spool file + state slot, so the input shape is unused
    // in production for the M4-05 tool dispatch path. We pick a minimal
    // variant that matches the task_type so the create call type-checks.
    match task_type {
        TaskType::LocalBash => TaskSpawnInput::LocalBash {
            command: String::new(),
            timeout: None,
        },
        TaskType::LocalAgent => TaskSpawnInput::LocalAgent {
            agent_id: protocol::AgentId::nil(),
            subagent_type: String::new(),
            prompt: String::new(),
            is_backgrounded: false,
        },
        TaskType::RemoteAgent => TaskSpawnInput::RemoteAgent {
            endpoint: String::new(),
            prompt: String::new(),
        },
        TaskType::InProcessTeammate => TaskSpawnInput::InProcessTeammate {
            agent_id: protocol::AgentId::nil(),
            name: String::new(),
            team_name: String::new(),
        },
        TaskType::LocalWorkflow => TaskSpawnInput::LocalWorkflow {
            workflow_id: String::new(),
            script: String::new(),
            resume_from_run_id: None,
            args: None,
            run_id: None,
        },
        TaskType::MonitorMcp => TaskSpawnInput::MonitorMcp {
            server_name: String::new(),
            watch: vec![],
        },
        TaskType::Dream => TaskSpawnInput::Dream {
            prompt: String::new(),
            max_iterations: None,
        },
    }
}

#[async_trait]
impl TaskRegistryHandle for TaskRegistry {
    async fn create(&self, input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        let task_type = task_type_from_wire(&input.task_type)?;
        let id = self
            .create(
                task_type,
                placeholder_input(task_type),
                input.description.clone(),
            )
            .await
            .map_err(task_err_to_registry_err)?;
        Ok(TaskRecord {
            task_id: id,
            task_type: input.task_type,
            status: "pending".into(),
            description: input.description,
            // `create` allocates a placeholder spawn input with an empty command
            // (`placeholder_input`), so no real command is available at this point;
            // a `local_bash` record gets its command once `state_to_record` reads
            // the populated bash state. `None` here matches the pre-spawn shape.
            command: None,
        })
    }

    async fn get(&self, id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(self.get(id).await.as_ref().map(state_to_record))
    }

    async fn list(&self, filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        let want_status = match filter.status.as_deref() {
            Some(s) => Some(status_from_wire(s)?),
            None => None,
        };
        let mut out = Vec::new();
        for state in self.list().await {
            if let Some(want) = want_status {
                if state.base().status != want {
                    continue;
                }
            }
            out.push(state_to_record(&state));
        }
        Ok(out)
    }

    async fn update(
        &self,
        id: &str,
        patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        if let Some(ref s) = patch.status {
            return <Self as TaskRegistryHandle>::set_status(self, id, s).await;
        }
        // No-op patch — just return current state.
        match self.get(id).await {
            Some(state) => Ok(state_to_record(&state)),
            None => Err(TaskRegistryError::NotFound(id.into())),
        }
    }

    async fn set_status(&self, id: &str, status: &str) -> Result<TaskRecord, TaskRegistryError> {
        let s = status_from_wire(status)?;
        let state = self
            .set_status(id, s)
            .await
            .map_err(task_err_to_registry_err)?;
        Ok(state_to_record(&state))
    }

    async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
        self.kill(id).await.map_err(task_err_to_registry_err)?;
        // After kill, fetch the (now-killed) state for the record.
        match self.get(id).await {
            Some(state) => Ok(state_to_record(&state)),
            None => Err(TaskRegistryError::NotFound(id.into())),
        }
    }

    async fn output(
        &self,
        id: &str,
        offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        let state = self
            .get(id)
            .await
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        let status = state.base().status;
        // Mirror TS `bashTask.result?.code ?? null`: only local-bash tasks carry
        // a process exit code; other task types report `None`.
        let exit_code = match &state {
            TaskState::LocalBash(s) => s.exit_code,
            _ => None,
        };
        // Mirror the TS poll predicate `status !== 'running' && status !==
        // 'pending'` — terminal means the task is "done" for retrieval.
        let done = status.is_terminal();
        let output_file = state.base().output_file.clone();
        let opts = crate::output_manager::OutputOptions {
            offset,
            limit: None,
        };
        let out = self
            .output_manager
            .read(&output_file, opts)
            .await
            .map_err(|e| match e {
                crate::output_manager::OutputError::Io(s) => {
                    TaskRegistryError::Internal(format!("io: {s}"))
                }
                crate::output_manager::OutputError::PathEscape(p) => {
                    TaskRegistryError::Internal(format!("path escape: {p}"))
                }
                // Unreachable on the read path (exclusive-create only fires in
                // `allocate`), but the match must stay exhaustive.
                crate::output_manager::OutputError::AlreadyExists(p) => {
                    TaskRegistryError::Internal(format!("spool already allocated: {p}"))
                }
            })?;
        // Agent-specific fields (TS `getTaskOutputData` `local_agent` branch,
        // `TaskOutputTool.tsx:91-106`): carry the agent's `error` + `prompt`
        // through, and extract the CLEAN final answer from the spooled
        // transcript so the model sees the answer (not the raw JSON blob).
        // `error`/`prompt`/`result` stay `None` for non-agent task types
        // (matching the TS shape, where only `local_agent` adds these keys).
        let (error, prompt, result) = match &state {
            TaskState::LocalAgent(a) => {
                let clean = agent_content_from_spool(&out.content)
                    .map(|content| extract_text_content(&content))
                    .filter(|s| !s.is_empty());
                (a.error.clone(), Some(a.prompt.clone()), clean)
            }
            _ => (None, None, None),
        };
        Ok(TaskOutputChunk {
            task_id: state.base().id.clone(),
            content: out.content,
            total_lines: out.total_lines,
            truncated: out.truncated,
            status: Some(status_to_wire(status).to_string()),
            exit_code,
            done,
            error,
            prompt,
            result,
            // The absolute on-disk spool path (claude-code
            // `getTaskOutputPath(taskId)`), surfaced so `TaskOutputTool` can show
            // the real path in its `[Truncated. Full output: <path>]` header.
            output_path: output_file.to_str().map(str::to_string),
        })
    }

    async fn mark_notified(&self, id: &str) -> Result<(), TaskRegistryError> {
        // Dispatch to the inherent `TaskRegistry::mark_notified`, which sets the
        // `notified` flag and eagerly evicts the task if it is now terminal.
        TaskRegistry::mark_notified(self, id)
            .await
            .map_err(task_err_to_registry_err)
    }

    async fn mark_rested(
        &self,
        id: &str,
        result: Option<String>,
        usage: Option<traits::task_registry::AgentRunUsage>,
    ) {
        // Dispatch to the inherent arm-rest path (no-op for unknown/terminal).
        TaskRegistry::mark_task_rested(self, id, result, usage).await;
    }

    async fn take_pending_task_notifications(
        &self,
    ) -> Result<Vec<traits::task_registry::TaskNotification>, TaskRegistryError> {
        // Dispatch to the inherent drain, which snapshots + marks-notified +
        // evicts the terminal-not-notified tasks. Infallible at the registry
        // level (the lock is always acquirable), so the seam result is always
        // `Ok`.
        Ok(TaskRegistry::take_pending_task_notifications(self).await)
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use crate::output_manager::TaskOutputManager;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    use test_harness::mocks::MockRuntimeSpawner;
    use traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};

    use std::collections::HashMap;
    use tokio::sync::Mutex as TokioMutex;

    /// In-memory `FileSystem` that actually preserves writes — used by the
    /// M5-01 Task 6 `output_returns_real_content_after_spool_write` test.
    struct InMemoryFs {
        files: TokioMutex<HashMap<String, String>>,
    }

    impl InMemoryFs {
        fn new() -> Self {
            Self {
                files: TokioMutex::new(HashMap::new()),
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
            let off = offset.unwrap_or(0) as usize;
            let body: String = content.chars().skip(off).collect();
            let truncated = if let Some(lim) = limit {
                body.len() as u64 > lim
            } else {
                false
            };
            let trimmed = if let Some(lim) = limit {
                body.chars().take(lim as usize).collect::<String>()
            } else {
                body
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
            let entry = map.entry(path.to_string()).or_default();
            entry.push_str(body);
            Ok(())
        }
        async fn truncate(&self, path: &str, len: u64) -> Result<(), FsError> {
            let mut map = self.files.lock().await;
            if let Some(s) = map.get_mut(path) {
                s.truncate(len as usize);
            }
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            let map = self.files.lock().await;
            Ok(map.get(path).map(|s| s.len() as u64).unwrap_or(0))
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

    fn make_registry() -> (tempfile::TempDir, Arc<TaskRegistry>) {
        let dir = tempdir().unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(InMemoryFs::new());
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let out_mgr = Arc::new(TaskOutputManager::new(
            PathBuf::from(dir.path()),
            fs.clone(),
        ));
        let registry = Arc::new(TaskRegistry::new(runtime, fs, out_mgr));
        (dir, registry)
    }

    // ── agent-specific output helpers (T3) ───────────────────────────────

    #[test]
    fn extract_text_content_joins_text_blocks_with_newline() {
        // Port of `extractTextContent(blocks, '\n')`: only `type:"text"` blocks,
        // joined by `\n`; non-text blocks (e.g. tool_use) are dropped.
        let content = serde_json::json!([
            { "type": "text", "text": "first" },
            { "type": "tool_use", "name": "Bash", "input": {} },
            { "type": "text", "text": "second" }
        ]);
        assert_eq!(extract_text_content(&content), "first\nsecond");
    }

    #[test]
    fn extract_text_content_non_array_is_empty() {
        // A non-array content (bare string / object) has no text blocks ⇒ "".
        assert_eq!(extract_text_content(&serde_json::json!("hello")), "");
        assert_eq!(extract_text_content(&serde_json::json!({ "x": 1 })), "");
    }

    #[test]
    fn agent_content_from_spool_strips_usage_footer_and_parses() {
        // The local_agent handler spools `to_string_pretty(content)` + a
        // `<usage>…</usage>` footer; recover the original content Value.
        let spool = "[\n  {\n    \"type\": \"text\",\n    \"text\": \"the answer\"\n  }\n]\n\
                     <usage><total_tokens>9</total_tokens></usage>\n";
        let content = agent_content_from_spool(spool).expect("parses");
        assert_eq!(extract_text_content(&content), "the answer");
    }

    #[test]
    fn agent_content_from_spool_unparseable_is_none() {
        // A `Failed` reason string (not JSON) ⇒ None, so the caller keeps the
        // raw output.
        assert!(agent_content_from_spool("model refused to continue").is_none());
        assert!(agent_content_from_spool("").is_none());
    }

    #[tokio::test]
    async fn output_local_agent_surfaces_clean_result_prompt_and_error() {
        // Drive a real local_agent task through the registry: insert a spawned
        // LocalAgent state, write its spool (the pretty-JSON transcript blob),
        // stamp an error, and verify `output()` returns the CLEAN extracted
        // text in `result` plus the agent's `prompt` + `error`.
        let (_d, registry) = make_registry();

        // Build a LocalAgent state directly and inject it (the registry's task
        // map is private, so use the spawn-state builder + the test setter).
        let chunk = {
            // Allocate a spool slot under a real LocalAgent id.
            let task_id = crate::id::generate_task_id(crate::id::TaskType::LocalAgent);
            let spool = registry
                .output_manager
                .allocate(&task_id)
                .await
                .unwrap();
            let base = crate::state::TaskStateBase {
                id: task_id.clone(),
                task_type: crate::id::TaskType::LocalAgent,
                status: TaskStatus::Failed,
                description: "run the agent".into(),
                tool_use_id: None,
                start_time: std::time::SystemTime::UNIX_EPOCH,
                end_time: None,
                total_paused_ms: 0,
                output_file: spool.clone(),
                output_offset: 0,
                notified: false,
            };
            let state = TaskState::LocalAgent(crate::state::LocalAgentTaskState {
                base,
                agent_id: protocol::AgentId::nil(),
                prompt: "do the thing".into(),
                error: Some("model refused".into()),
                messages: vec![],
                pending_messages: vec![],
                is_backgrounded: true,
            });
            registry.insert_state_for_test(state).await;

            // Spool the raw pretty-JSON transcript + usage footer.
            let blob = "[\n  {\n    \"type\": \"text\",\n    \"text\": \"final answer\"\n  }\n]\n\
                        <usage><total_tokens>3</total_tokens></usage>\n";
            registry
                .output_manager
                .fs_for_test()
                .write_file(spool.to_str().unwrap(), blob)
                .await
                .unwrap();

            let h: &dyn TaskRegistryHandle = registry.as_ref();
            h.output(&task_id, None).await.unwrap()
        };

        assert_eq!(chunk.result.as_deref(), Some("final answer"));
        assert_eq!(chunk.prompt.as_deref(), Some("do the thing"));
        assert_eq!(chunk.error.as_deref(), Some("model refused"));
        assert!(chunk.done, "failed task is terminal");
    }

    #[tokio::test]
    async fn get_local_bash_record_carries_command() {
        // T10: a `local_bash` record surfaces its shell COMMAND (claude-code
        // `LocalShellTaskState.command`), distinct from its description, so
        // `TaskStop` can prefer it (`stopTask.ts:97`).
        let (_d, registry) = make_registry();
        let task_id = crate::id::generate_task_id(crate::id::TaskType::LocalBash);
        let spool = registry.output_manager.allocate(&task_id).await.unwrap();
        let base = crate::state::TaskStateBase {
            id: task_id.clone(),
            task_type: crate::id::TaskType::LocalBash,
            status: TaskStatus::Running,
            description: "build the workspace".into(),
            tool_use_id: None,
            start_time: std::time::SystemTime::UNIX_EPOCH,
            end_time: None,
            total_paused_ms: 0,
            output_file: spool,
            output_offset: 0,
            notified: false,
        };
        let state = TaskState::LocalBash(crate::state::LocalBashTaskState {
            base,
            command: "cargo build --release".into(),
            pid: None,
            exit_code: None,
        });
        registry.insert_state_for_test(state).await;

        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h.get(&task_id).await.unwrap().expect("record present");
        assert_eq!(
            rec.command.as_deref(),
            Some("cargo build --release"),
            "local_bash record carries the shell command"
        );
        assert_eq!(rec.description, "build the workspace");
    }

    #[tokio::test]
    async fn get_local_agent_record_has_no_command() {
        // T10: non-bash task types report `command: None` so `TaskStop` falls back
        // to `description` (`stopTask.ts:97` `: task.description`).
        let (_d, registry) = make_registry();
        let task_id = crate::id::generate_task_id(crate::id::TaskType::LocalAgent);
        let spool = registry.output_manager.allocate(&task_id).await.unwrap();
        let base = crate::state::TaskStateBase {
            id: task_id.clone(),
            task_type: crate::id::TaskType::LocalAgent,
            status: TaskStatus::Running,
            description: "review the PR".into(),
            tool_use_id: None,
            start_time: std::time::SystemTime::UNIX_EPOCH,
            end_time: None,
            total_paused_ms: 0,
            output_file: spool,
            output_offset: 0,
            notified: false,
        };
        let state = TaskState::LocalAgent(crate::state::LocalAgentTaskState {
            base,
            agent_id: protocol::AgentId::nil(),
            prompt: "do it".into(),
            error: None,
            messages: vec![],
            pending_messages: vec![],
            is_backgrounded: true,
        });
        registry.insert_state_for_test(state).await;

        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h.get(&task_id).await.unwrap().expect("record present");
        assert_eq!(rec.command, None, "non-bash record has no command");
    }

    #[tokio::test]
    async fn create_via_handle_returns_record_with_pending_status() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "hello".into(),
            })
            .await
            .unwrap();
        assert!(rec.task_id.starts_with('b'));
        assert_eq!(rec.status, "pending");
        assert_eq!(rec.task_type, "local_bash");
    }

    #[tokio::test]
    async fn list_filters_by_status() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        h.create(TaskCreateInput {
            task_type: "local_bash".into(),
            description: "a".into(),
        })
        .await
        .unwrap();
        let pending = h
            .list(TaskListFilter {
                status: Some("pending".into()),
            })
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        let running = h
            .list(TaskListFilter {
                status: Some("running".into()),
            })
            .await
            .unwrap();
        assert!(running.is_empty());
    }

    #[tokio::test]
    async fn set_status_via_handle_transitions() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "a".into(),
            })
            .await
            .unwrap();
        let updated = h.set_status(&rec.task_id, "running").await.unwrap();
        assert_eq!(updated.status, "running");
    }

    #[tokio::test]
    async fn unknown_task_type_is_invalid_input() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let err = h
            .create(TaskCreateInput {
                task_type: "bogus".into(),
                description: "x".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, TaskRegistryError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn output_returns_empty_for_freshly_created_task_with_zero_byte_spool() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "a".into(),
            })
            .await
            .unwrap();
        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.task_id, rec.task_id);
        assert_eq!(chunk.content, "");
        assert!(!chunk.truncated);
    }

    #[tokio::test]
    async fn output_returns_real_content_after_spool_write() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "echo hi".into(),
            })
            .await
            .unwrap();

        // Pull the spool path from the typed state, then write content
        // directly through the registry's filesystem (simulates a handler
        // producing output).
        let state = registry.get(&rec.task_id).await.unwrap();
        let path = state.base().output_file.clone();
        let path_str = path.to_str().unwrap().to_string();
        let fs = registry.output_manager.fs_for_test();
        fs.write_file(&path_str, "line1\nline2\nline3\n")
            .await
            .unwrap();

        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.task_id, rec.task_id);
        assert_eq!(
            chunk.content, "line1\nline2\nline3\n",
            "spool content surfaces verbatim"
        );
        assert_eq!(chunk.total_lines, 3, "total_lines reflects the spool");
        assert!(!chunk.truncated, "no truncation on unlimited read");
    }

    #[tokio::test]
    async fn output_running_task_is_not_done_and_carries_no_exit_code() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "sleep".into(),
            })
            .await
            .unwrap();
        registry.force_bash_terminal_for_test(&rec.task_id, TaskStatus::Running, None).await;

        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.status.as_deref(), Some("running"));
        assert!(!chunk.done, "running task is not terminal");
        assert_eq!(chunk.exit_code, None, "no exit code while running");
    }

    #[tokio::test]
    async fn output_completed_task_is_done_with_exit_code_zero() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "echo".into(),
            })
            .await
            .unwrap();
        registry.force_bash_terminal_for_test(&rec.task_id, TaskStatus::Completed, Some(0)).await;

        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.status.as_deref(), Some("completed"));
        assert!(chunk.done, "completed task is terminal");
        assert_eq!(chunk.exit_code, Some(0));
    }

    #[tokio::test]
    async fn output_failed_task_is_done_with_nonzero_exit_code() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "false".into(),
            })
            .await
            .unwrap();
        registry.force_bash_terminal_for_test(&rec.task_id, TaskStatus::Failed, Some(1)).await;

        let chunk = h.output(&rec.task_id, None).await.unwrap();
        assert_eq!(chunk.status.as_deref(), Some("failed"));
        assert!(chunk.done, "failed task is terminal");
        assert_eq!(chunk.exit_code, Some(1));
    }

    #[tokio::test]
    async fn output_threads_offset_into_output_manager() {
        let (_d, registry) = make_registry();
        let h: &dyn TaskRegistryHandle = registry.as_ref();
        let rec = h
            .create(TaskCreateInput {
                task_type: "local_bash".into(),
                description: "echo hi".into(),
            })
            .await
            .unwrap();

        let state = registry.get(&rec.task_id).await.unwrap();
        let path_str = state.base().output_file.to_str().unwrap().to_string();
        let fs = registry.output_manager.fs_for_test();
        fs.write_file(&path_str, "abcdef").await.unwrap();

        // offset=3 should drop the first 3 chars.
        let chunk = h.output(&rec.task_id, Some(3)).await.unwrap();
        assert_eq!(chunk.content, "def", "offset honoured");
    }
}
