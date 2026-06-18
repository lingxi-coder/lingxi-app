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

fn task_type_to_wire(t: TaskType) -> &'static str {
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

fn status_to_wire(s: TaskStatus) -> &'static str {
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
    TaskRecord {
        task_id: b.id.clone(),
        task_type: task_type_to_wire(b.task_type).to_string(),
        status: status_to_wire(b.status).to_string(),
        description: b.description.clone(),
    }
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
        },
        TaskType::LocalWorkflow => TaskSpawnInput::LocalWorkflow {
            workflow_id: String::new(),
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
        Ok(TaskOutputChunk {
            task_id: state.base().id.clone(),
            content: out.content,
            total_lines: out.total_lines,
            truncated: out.truncated,
            status: Some(status_to_wire(status).to_string()),
            exit_code,
            done,
        })
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
