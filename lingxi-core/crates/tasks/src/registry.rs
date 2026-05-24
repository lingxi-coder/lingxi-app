//! Central registry tracking running tasks.
//!
//! The registry owns the in-memory map of task IDs to [`TaskState`] and to
//! the [`BackgroundTaskHandle`]s returned by the [`RuntimeSpawner`].

use crate::id::{generate_task_id, TaskType};
use crate::output_manager::TaskOutputManager;
use crate::state::{TaskState, TaskStateBase, TaskStatus};
use crate::task_trait::{Task, TaskError, TaskSpawnInput};
use lingxi_traits::{BackgroundTaskHandle, FileSystem, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// Tracks running tasks and dispatches lifecycle operations to handlers.
pub struct TaskRegistry {
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    handlers: HashMap<TaskType, Arc<dyn Task>>,
    handles: Arc<tokio::sync::Mutex<HashMap<String, BackgroundTaskHandle>>>,
    #[allow(dead_code)]
    runtime: Arc<dyn RuntimeSpawner>,
    #[allow(dead_code)]
    fs: Arc<dyn FileSystem>,
    /// Owner of task spool files.
    pub output_manager: Arc<TaskOutputManager>,
}

impl TaskRegistry {
    /// Construct an empty registry with no handlers yet registered.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimeSpawner>,
        fs: Arc<dyn FileSystem>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            handlers: HashMap::new(),
            handles: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            runtime,
            fs,
            output_manager,
        }
    }

    /// Register a per-type handler.
    pub fn register_handler(&mut self, task_type: TaskType, handler: Arc<dyn Task>) {
        self.handlers.insert(task_type, handler);
    }

    /// Create a new task entry. Allocates the output file and returns the
    /// generated task ID. Spawning the actual worker is done by handlers.
    pub async fn create(
        &self,
        task_type: TaskType,
        _input: TaskSpawnInput,
        description: String,
    ) -> Result<String, TaskError> {
        let id = generate_task_id(task_type);
        let path = self
            .output_manager
            .allocate(&id)
            .await
            .map_err(|e| TaskError::Io(e.to_string()))?;
        let base = TaskStateBase {
            id: id.clone(),
            task_type,
            status: TaskStatus::Pending,
            description,
            tool_use_id: None,
            start_time: SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: path,
            output_offset: 0,
            notified: false,
        };
        // Build a default state per type; production stores real fields.
        #[allow(clippy::match_same_arms)]
        let state = match task_type {
            TaskType::LocalBash => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
            }),
            TaskType::Dream => TaskState::Dream(crate::state::DreamTaskState {
                base,
                iteration_count: 0,
                max_iterations: None,
            }),
            _ => TaskState::LocalBash(crate::state::LocalBashTaskState {
                base,
                command: String::new(),
                pid: None,
                exit_code: None,
            }),
        };
        self.tasks.write().await.insert(id.clone(), state);
        Ok(id)
    }

    /// Look up a task by ID.
    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(task_id).cloned()
    }

    /// Return all known tasks.
    pub async fn list(&self) -> Vec<TaskState> {
        self.tasks.read().await.values().cloned().collect()
    }

    /// Force `task_id`'s status to `status`. Returns
    /// [`TaskError::NotFound`] if the id is unknown. Only Bash and Agent
    /// variants currently carry a writable `status` field in the M1 surface;
    /// other variants are no-ops on the variant but still return the
    /// (possibly unchanged) state for the caller's consumption.
    pub async fn set_status(
        &self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<TaskState, TaskError> {
        let mut map = self.tasks.write().await;
        let entry = map
            .get_mut(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
        match entry {
            TaskState::LocalBash(b) => b.base.status = status,
            TaskState::LocalAgent(a) => a.base.status = status,
            TaskState::RemoteAgent(r) => r.base.status = status,
            TaskState::InProcessTeammate(t) => t.base.status = status,
            TaskState::LocalWorkflow(w) => w.base.status = status,
            TaskState::MonitorMcp(m) => m.base.status = status,
            TaskState::Dream(d) => d.base.status = status,
        }
        Ok(entry.clone())
    }

    /// Kill a task, cancelling its background handle if any.
    ///
    /// Note: only Bash and Agent states currently carry a writable `status`
    /// field in the M1 surface; other variants are no-ops on cancel.
    pub async fn kill(&self, task_id: &str) -> Result<(), TaskError> {
        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.remove(task_id) {
            self.runtime
                .cancel(&h)
                .await
                .map_err(|e| TaskError::Internal(e.to_string()))?;
        }
        if let Some(s) = self.tasks.write().await.get_mut(task_id) {
            // Mark killed for the variants whose status is exposed here.
            match s {
                TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                // Other variants intentionally fall through in M1.
                _ => {}
            }
        }
        Ok(())
    }
}
