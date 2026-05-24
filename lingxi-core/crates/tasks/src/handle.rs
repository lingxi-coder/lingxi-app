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
use lingxi_traits::task_registry::{
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
            agent_id: lingxi_protocol::AgentId::nil(),
            prompt: String::new(),
            is_backgrounded: false,
        },
        TaskType::RemoteAgent => TaskSpawnInput::RemoteAgent {
            endpoint: String::new(),
            prompt: String::new(),
        },
        TaskType::InProcessTeammate => TaskSpawnInput::InProcessTeammate {
            agent_id: lingxi_protocol::AgentId::nil(),
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
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        // M4-05 surface returns an empty spool for tasks that have not yet
        // started accumulating output. The output_manager's spool reader
        // lands as a follow-up — the registry currently exposes only the
        // file path through `TaskStateBase::output_file`.
        let state = self
            .get(id)
            .await
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        Ok(TaskOutputChunk {
            task_id: state.base().id.clone(),
            content: String::new(),
            total_lines: 0,
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_manager::TaskOutputManager;
    use async_trait::async_trait;
    use lingxi_test_harness::mocks::MockRuntimeSpawner;
    use lingxi_traits::filesystem::{FileContent, FileEvent, FileSystem, FlockGuard, FsError};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    struct NoopFs;

    #[async_trait]
    impl FileSystem for NoopFs {
        async fn read_file(
            &self,
            _: &str,
            _: Option<u64>,
            _: Option<u64>,
        ) -> Result<FileContent, FsError> {
            Ok(FileContent {
                content: String::new(),
                truncated: false,
                total_lines: 0,
            })
        }
        async fn write_file(&self, _: &str, _: &str) -> Result<(), FsError> {
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
        async fn append_file(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<std::time::SystemTime, FsError> {
            Ok(std::time::SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, _: &str) -> Result<u64, FsError> {
            Ok(0)
        }
        async fn delete_file(&self, _: &str) -> Result<(), FsError> {
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
        let fs: Arc<dyn FileSystem> = Arc::new(NoopFs);
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
    async fn output_returns_empty_for_freshly_created_task() {
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
}
