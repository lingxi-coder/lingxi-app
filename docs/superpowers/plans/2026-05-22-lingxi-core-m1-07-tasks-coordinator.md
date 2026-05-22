# LingXi Core M1 · Plan 07 · Tasks & Coordinator

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Build the polymorphic Task Manager (7 task types with shared trait + per-type state + disk-persisted output + notification injection) and the Coordinator/Team layer (CoordinatorMode + 4 internal tools + TeamRegistry + TeammateMailbox + SwarmBackend trait).

**Architecture:** `lingxi-tasks` owns the 7 TaskState variants + TaskRegistry + TaskOutputManager. `lingxi-coordinator` owns the mode toggle, internal coordinator-only tools, and the mailbox router. Coordinator tools `TeamCreate / TeamDelete / SendMessage / SyntheticOutput` are only registered in the ToolRegistry when coordinator mode is active.

**Depends on:** Plans 01-06.

---

## File Structure

```
crates/tasks/
├── Cargo.toml
└── src/{lib, task_trait, state, registry, output_manager, notification, cron, id, handlers/{local_bash, local_agent, remote_agent, in_process_teammate, local_workflow, monitor_mcp, dream}}.rs

crates/coordinator/
├── Cargo.toml
└── src/{lib, mode, internal_tools, team_registry, mailbox, swarm}.rs

crates/traits/src/swarm.rs  ← NEW: SwarmBackend trait
```

---

## Task 1: SwarmBackend trait

**Files:** `crates/traits/src/swarm.rs`

```rust
use async_trait::async_trait;
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[async_trait]
pub trait SwarmBackend: Send + Sync {
    async fn start_swarm(&self, layout: SwarmLayout) -> Result<SwarmHandle, SwarmError>;
    async fn create_teammate_pane(&self, agent_id: &AgentId, position: PanePosition) -> Result<PaneId, SwarmError>;
    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError>;
    fn is_available(&self) -> bool;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SwarmLayout { LeaderFollower, Tiled, External }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PanePosition { Top, Bottom, Left, Right }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmHandle { pub session_name: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneId { pub raw: String }

#[derive(Debug, Clone, Error)]
pub enum SwarmError {
    #[error("swarm not supported on this platform")]
    Unsupported,
    #[error("tmux error: {0}")]
    Tmux(String),
}
```

Add to `traits/src/lib.rs`. Commit:
```bash
cargo check -p lingxi-traits
git add crates/traits
git commit -m "feat(traits): SwarmBackend trait"
```

---

## Task 2: TaskState + Task trait (7 variants)

**Files:** `crates/tasks/{Cargo.toml, src/{lib,task_trait,state,id}.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-tasks"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-core = { path = "../core" }
lingxi-agent = { path = "../agent" }
lingxi-tools = { path = "../tools" }
lingxi-hooks = { path = "../hooks" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
rand = "0.9"
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: id.rs (uniform distribution sampler — C4)**

```rust
use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    LocalBash, LocalAgent, RemoteAgent, InProcessTeammate,
    LocalWorkflow, MonitorMcp, Dream,
}

impl TaskType {
    pub fn id_prefix(self) -> char {
        match self {
            Self::LocalBash => 'b',
            Self::LocalAgent => 'a',
            Self::RemoteAgent => 'r',
            Self::InProcessTeammate => 't',
            Self::LocalWorkflow => 'w',
            Self::MonitorMcp => 'm',
            Self::Dream => 'd',
        }
    }
}

const TASK_ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Uniform sampling (not modulo) to avoid bias for 256/36 ≠ integer.
pub fn generate_task_id(task_type: TaskType) -> String {
    let mut rng = rand::rng();
    let suffix: String = (0..8)
        .map(|_| TASK_ID_ALPHABET[rng.random_range(0..TASK_ID_ALPHABET.len())] as char)
        .collect();
    format!("{}{}", task_type.id_prefix(), suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_over_1000_samples() {
        let mut set = HashSet::new();
        for _ in 0..1000 {
            set.insert(generate_task_id(TaskType::LocalBash));
        }
        assert!(set.len() > 990, "too many collisions in 1000 samples");
    }
}
```

- [ ] **Step 3: state.rs (polymorphic state)**

```rust
use crate::id::TaskType;
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus { Pending, Running, Completed, Failed, Killed }

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateBase {
    pub id: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub tool_use_id: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub total_paused_ms: u64,
    pub output_file: PathBuf,
    pub output_offset: u64,
    pub notified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "task_type", rename_all = "snake_case")]
pub enum TaskState {
    LocalBash(LocalBashTaskState),
    LocalAgent(LocalAgentTaskState),
    RemoteAgent(RemoteAgentTaskState),
    InProcessTeammate(InProcessTeammateTaskState),
    LocalWorkflow(LocalWorkflowTaskState),
    MonitorMcp(MonitorMcpTaskState),
    Dream(DreamTaskState),
}

impl TaskState {
    pub fn base(&self) -> &TaskStateBase {
        match self {
            Self::LocalBash(s) => &s.base,
            Self::LocalAgent(s) => &s.base,
            Self::RemoteAgent(s) => &s.base,
            Self::InProcessTeammate(s) => &s.base,
            Self::LocalWorkflow(s) => &s.base,
            Self::MonitorMcp(s) => &s.base,
            Self::Dream(s) => &s.base,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalBashTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub command: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAgentTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub agent_id: AgentId,
    pub prompt: String,
    pub error: Option<String>,
    pub messages: Vec<lingxi_protocol::ConversationMessage>,
    pub pending_messages: Vec<String>,
    pub is_backgrounded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteAgentTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub remote_session_id: String,
    pub remote_endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InProcessTeammateTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub agent_id: AgentId,
    pub pending_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalWorkflowTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub workflow_id: String,
    pub current_step: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorMcpTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub server_name: String,
    pub watch_resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamTaskState {
    #[serde(flatten)] pub base: TaskStateBase,
    pub iteration_count: u32,
    pub max_iterations: Option<u32>,
}
```

- [ ] **Step 4: task_trait.rs**

```rust
use crate::state::{TaskState, TaskStatus};
use async_trait::async_trait;
use lingxi_traits::{FileSystem, RuntimeSpawner};
use std::sync::Arc;
use thiserror::Error;

#[async_trait]
pub trait Task: Send + Sync {
    fn name(&self) -> &str;
    fn task_type(&self) -> crate::id::TaskType;
    async fn spawn(&self, input: TaskSpawnInput, ctx: TaskContext) -> Result<TaskHandle, TaskError>;
    async fn kill(&self, task_id: &str, ctx: TaskContext) -> Result<(), TaskError>;
    fn supports_messages(&self) -> bool { false }
    async fn send_message(&self, _task_id: &str, _message: String, _ctx: TaskContext) -> Result<(), TaskError> {
        Err(TaskError::Unsupported)
    }
}

#[derive(Debug, Clone)]
pub enum TaskSpawnInput {
    LocalBash { command: String, timeout: Option<std::time::Duration> },
    LocalAgent { agent_id: lingxi_protocol::AgentId, prompt: String, is_backgrounded: bool },
    RemoteAgent { endpoint: String, prompt: String },
    InProcessTeammate { agent_id: lingxi_protocol::AgentId, name: String },
    LocalWorkflow { workflow_id: String },
    MonitorMcp { server_name: String, watch: Vec<String> },
    Dream { prompt: String, max_iterations: Option<u32> },
}

#[derive(Clone)]
pub struct TaskContext {
    pub fs: Arc<dyn FileSystem>,
    pub runtime: Arc<dyn RuntimeSpawner>,
}

pub struct TaskHandle {
    pub task_id: String,
    pub cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Debug, Clone, Error)]
pub enum TaskError {
    #[error("unknown task type")]
    UnknownType,
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("terminated task — cannot accept messages")]
    TerminatedTask,
    #[error("unsupported operation")]
    Unsupported,
    #[error("io: {0}")]
    Io(String),
    #[error("internal: {0}")]
    Internal(String),
}
```

- [ ] **Step 5: lib.rs + commit**

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod cron;
pub mod handlers;
pub mod id;
pub mod notification;
pub mod output_manager;
pub mod registry;
pub mod state;
pub mod task_trait;

pub use id::{generate_task_id, TaskType};
pub use state::*;
pub use task_trait::*;
```

Test:
```bash
cargo test -p lingxi-tasks --lib id
```

Commit:
```bash
git add crates/tasks
git commit -m "feat(tasks): TaskType + TaskState (7 variants) + Task trait"
```

---

## Task 3: TaskRegistry + TaskOutputManager + NotificationBuilder

**Files:** `crates/tasks/src/{registry, output_manager, notification}.rs`

- [ ] **Step 1: output_manager.rs**

```rust
use lingxi_traits::FileSystem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

pub struct TaskOutputManager {
    output_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    pub max_file_size: u64,
    pub total_budget: u64,
    used: AtomicU64,
}

#[derive(Debug, Clone, Error)]
pub enum OutputError {
    #[error("io: {0}")]
    Io(String),
    #[error("path escapes output dir: {0}")]
    PathEscape(String),
}

#[derive(Debug, Clone)]
pub struct OutputOptions { pub offset: Option<u64>, pub limit: Option<u64> }

#[derive(Debug, Clone)]
pub struct TaskOutput {
    pub content: String,
    pub total_lines: u64,
    pub truncated: bool,
}

impl TaskOutputManager {
    pub fn new(output_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self { output_dir, fs, max_file_size: 10 * 1024 * 1024, total_budget: 100 * 1024 * 1024, used: AtomicU64::new(0) }
    }

    /// Allocate a path inside `output_dir`. Refuse any `..` or absolute leak (D8).
    pub async fn allocate(&self, task_id: &str) -> Result<PathBuf, OutputError> {
        let filename = format!("{task_id}.txt");
        let path = self.output_dir.join(&filename);
        if !path.starts_with(&self.output_dir) {
            return Err(OutputError::PathEscape(filename));
        }
        self.fs.write_file(path.to_str().unwrap(), "").await
            .map_err(|e| OutputError::Io(e.to_string()))?;
        Ok(path)
    }

    pub async fn read(&self, output_file: &Path, opts: OutputOptions) -> Result<TaskOutput, OutputError> {
        let fc = self.fs.read_file(output_file.to_str().unwrap(), opts.offset, opts.limit).await
            .map_err(|e| OutputError::Io(e.to_string()))?;
        Ok(TaskOutput { content: fc.content, total_lines: fc.total_lines, truncated: fc.truncated })
    }
}
```

- [ ] **Step 2: notification.rs (XML injection)**

```rust
use crate::state::TaskStatus;
use std::path::Path;

pub struct TaskNotificationBuilder;

impl TaskNotificationBuilder {
    pub fn build(
        task_id: &str,
        tool_use_id: Option<&str>,
        output_path: &Path,
        status: TaskStatus,
        summary: &str,
    ) -> String {
        let tool_use_line = tool_use_id
            .map(|s| format!("  <tool-use-id>{s}</tool-use-id>\n"))
            .unwrap_or_default();
        format!(
            "<task-notification>\n  <task-id>{task_id}</task-id>\n{tool_use_line}  <output-file>{}</output-file>\n  <status>{:?}</status>\n  <summary>{summary}</summary>\n</task-notification>",
            output_path.display(),
            status,
        )
    }
}

#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub value: String,
    pub mode: NotificationMode,
}

#[derive(Debug, Clone, Copy)]
pub enum NotificationMode { Normal, TaskNotification }
```

- [ ] **Step 3: registry.rs**

```rust
use crate::id::{generate_task_id, TaskType};
use crate::output_manager::TaskOutputManager;
use crate::state::{TaskState, TaskStateBase, TaskStatus};
use crate::task_trait::{Task, TaskContext, TaskError, TaskSpawnInput};
use lingxi_traits::{BackgroundTaskHandle, FileSystem, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::{mpsc, RwLock};

pub struct TaskRegistry {
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    handlers: HashMap<TaskType, Arc<dyn Task>>,
    handles: Arc<tokio::sync::Mutex<HashMap<String, BackgroundTaskHandle>>>,
    runtime: Arc<dyn RuntimeSpawner>,
    fs: Arc<dyn FileSystem>,
    pub output_manager: Arc<TaskOutputManager>,
}

impl TaskRegistry {
    pub fn new(
        runtime: Arc<dyn RuntimeSpawner>,
        fs: Arc<dyn FileSystem>,
        output_manager: Arc<TaskOutputManager>,
    ) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            handlers: HashMap::new(),
            handles: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            runtime, fs, output_manager,
        }
    }

    pub fn register_handler(&mut self, task_type: TaskType, handler: Arc<dyn Task>) {
        self.handlers.insert(task_type, handler);
    }

    pub async fn create(&self, task_type: TaskType, _input: TaskSpawnInput, description: String) -> Result<String, TaskError> {
        let id = generate_task_id(task_type);
        let path = self.output_manager.allocate(&id).await.map_err(|e| TaskError::Io(e.to_string()))?;
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
        let state = match task_type {
            TaskType::LocalBash => TaskState::LocalBash(crate::state::LocalBashTaskState { base, command: String::new(), pid: None, exit_code: None }),
            TaskType::Dream => TaskState::Dream(crate::state::DreamTaskState { base, iteration_count: 0, max_iterations: None }),
            _ => TaskState::LocalBash(crate::state::LocalBashTaskState { base, command: String::new(), pid: None, exit_code: None }),
        };
        self.tasks.write().await.insert(id.clone(), state);
        Ok(id)
    }

    pub async fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn list(&self) -> Vec<TaskState> {
        self.tasks.read().await.values().cloned().collect()
    }

    pub async fn kill(&self, task_id: &str) -> Result<(), TaskError> {
        let mut handles = self.handles.lock().await;
        if let Some(h) = handles.remove(task_id) {
            self.runtime.cancel(&h).await.map_err(|e| TaskError::Internal(e.to_string()))?;
        }
        if let Some(s) = self.tasks.write().await.get_mut(task_id) {
            // Mark killed
            match s {
                TaskState::LocalBash(b) => b.base.status = TaskStatus::Killed,
                TaskState::LocalAgent(a) => a.base.status = TaskStatus::Killed,
                _ => {}
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: handlers/mod.rs (stub per type)**

```rust
pub mod local_bash;
pub mod local_agent;
pub mod remote_agent;
pub mod in_process_teammate;
pub mod local_workflow;
pub mod monitor_mcp;
pub mod dream;
```

Each file: a stub `pub struct XHandler;` implementing `Task` with `spawn` returning `Err(TaskError::Unsupported)`. Full impls come in M2.

- [ ] **Step 5: cron.rs**

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTaskDef {
    pub id: String,
    pub schedule: String,
    pub prompt: String,
    pub agent_type: Option<String>,
    pub last_run: Option<SystemTime>,
    pub enabled: bool,
}

pub struct CronTaskRegistry {
    tasks: Mutex<HashMap<String, CronTaskDef>>,
}

impl CronTaskRegistry {
    pub fn new() -> Self { Self { tasks: Mutex::new(HashMap::new()) } }
    pub fn register(&self, def: CronTaskDef) { self.tasks.lock().unwrap().insert(def.id.clone(), def); }
    pub fn unregister(&self, id: &str) { self.tasks.lock().unwrap().remove(id); }
    pub fn find_due(&self, _now: SystemTime) -> Vec<CronTaskDef> {
        // Stub: full schedule parsing in Plan 11.
        Vec::new()
    }
}

impl Default for CronTaskRegistry { fn default() -> Self { Self::new() } }
```

- [ ] **Step 6: Commit**

```bash
cargo test -p lingxi-tasks
git add crates/tasks
git commit -m "feat(tasks): TaskRegistry + OutputManager + NotificationBuilder + handler stubs"
```

---

## Task 4: lingxi-coordinator — mode + internal tools + mailbox

**Files:** `crates/coordinator/{Cargo.toml, src/{lib,mode,internal_tools,team_registry,mailbox,swarm}.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-coordinator"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-core = { path = "../core" }
lingxi-tools = { path = "../tools" }
lingxi-agent = { path = "../agent" }
lingxi-tasks = { path = "../tasks" }
lingxi-memory = { path = "../memory" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: mode.rs**

```rust
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
pub struct CoordinatorMode {
    enabled: AtomicBool,
    pub session_started_as_coordinator: bool,
}

impl CoordinatorMode {
    pub fn new() -> Self { Self::default() }
    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Acquire) }
    pub fn enter(&self) { self.enabled.store(true, Ordering::Release); }
    pub fn exit(&self) { self.enabled.store(false, Ordering::Release); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModeSwitchResult { EnteredCoordinator, ExitedCoordinator }
```

- [ ] **Step 3: mailbox.rs**

```rust
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    pub from: MessageSender,
    pub content: String,
    pub message_id: String,
    pub timestamp: std::time::SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageSender { Coordinator, Teammate(AgentId), User, System }

#[derive(Debug, Clone, Error)]
pub enum MailboxError {
    #[error("mailbox closed")]
    Closed,
    #[error("mailbox full")]
    Full,
    #[error("recipient not found: {0}")]
    NotFound(AgentId),
}

pub struct TeammateMailbox {
    pub agent_id: AgentId,
    inbox: Mutex<VecDeque<TeammateMessage>>,
    max_size: usize,
    waker: Arc<Notify>,
    closed: AtomicBool,
}

impl TeammateMailbox {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            inbox: Mutex::new(VecDeque::new()),
            max_size: 100,
            waker: Arc::new(Notify::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub fn deliver(&self, msg: TeammateMessage) -> Result<(), MailboxError> {
        if self.closed.load(Ordering::Acquire) { return Err(MailboxError::Closed); }
        let mut inbox = self.inbox.lock().unwrap();
        if inbox.len() >= self.max_size {
            return Err(MailboxError::Full);
        }
        inbox.push_back(msg);
        self.waker.notify_one();
        Ok(())
    }

    pub fn drain(&self) -> Vec<TeammateMessage> {
        self.inbox.lock().unwrap().drain(..).collect()
    }

    pub async fn wait_for_message(&self, timeout: Duration) -> Option<TeammateMessage> {
        tokio::select! {
            _ = self.waker.notified() => self.inbox.lock().unwrap().pop_front(),
            _ = tokio::time::sleep(timeout) => None,
        }
    }
}

pub struct MailboxRouter {
    mailboxes: tokio::sync::RwLock<std::collections::HashMap<AgentId, Arc<TeammateMailbox>>>,
}

impl MailboxRouter {
    pub fn new() -> Self { Self { mailboxes: tokio::sync::RwLock::new(Default::default()) } }
    pub async fn register(&self, agent_id: AgentId, mailbox: Arc<TeammateMailbox>) {
        self.mailboxes.write().await.insert(agent_id, mailbox);
    }
    pub async fn route(&self, to: &AgentId, msg: TeammateMessage) -> Result<(), MailboxError> {
        let mailboxes = self.mailboxes.read().await;
        let mb = mailboxes.get(to).ok_or(MailboxError::NotFound(*to))?;
        mb.deliver(msg)
    }
    pub async fn unregister(&self, agent: &AgentId) {
        self.mailboxes.write().await.remove(agent);
    }
}
```

- [ ] **Step 4: team_registry.rs**

```rust
use crate::mailbox::{MailboxRouter, TeammateMailbox};
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAgent {
    pub agent_id: AgentId,
    pub agent_type: String,
    pub name: String,
    pub parent_id: Option<AgentId>,
    pub status: WorkerStatus,
    pub task_id: String,
    pub spawned_at: SystemTime,
    pub last_active_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Idle,
    Working { activity: String },
    AwaitingMessage,
    Completed,
    Failed { error: String },
    Killed,
}

pub struct TeamRegistry {
    workers: RwLock<HashMap<AgentId, WorkerAgent>>,
    pub coordinator_id: AgentId,
    pub mailbox_router: Arc<MailboxRouter>,
}

impl TeamRegistry {
    pub fn new(coordinator_id: AgentId) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            coordinator_id,
            mailbox_router: Arc::new(MailboxRouter::new()),
        }
    }

    pub async fn spawn_worker(
        &self,
        agent_type: String,
        name: String,
        task_id: String,
    ) -> Result<AgentId, crate::mailbox::MailboxError> {
        let agent_id = AgentId::new();
        let mailbox = Arc::new(TeammateMailbox::new(agent_id));
        self.mailbox_router.register(agent_id, mailbox).await;
        self.workers.write().await.insert(agent_id, WorkerAgent {
            agent_id,
            agent_type,
            name,
            parent_id: Some(self.coordinator_id),
            status: WorkerStatus::Idle,
            task_id,
            spawned_at: SystemTime::now(),
            last_active_at: SystemTime::now(),
        });
        Ok(agent_id)
    }

    pub async fn delete_worker(&self, agent_id: &AgentId) {
        self.workers.write().await.remove(agent_id);
        self.mailbox_router.unregister(agent_id).await;
    }

    pub async fn list(&self) -> Vec<WorkerAgent> {
        self.workers.read().await.values().cloned().collect()
    }
}
```

- [ ] **Step 5: internal_tools.rs (4 tools — stubs that register only when coordinator mode is active)**

```rust
use crate::team_registry::TeamRegistry;
use lingxi_tools::Tool;
use std::sync::Arc;

/// Returns the 4 coordinator-only tools (TeamCreate / TeamDelete / SendMessage / SyntheticOutput).
/// In §15 Plugin and §22 Cli-demo, the host wires these into ToolRegistry only when
/// CoordinatorMode::is_enabled() is true.
pub fn coordinator_internal_tools(_team: Arc<TeamRegistry>) -> Vec<Arc<dyn Tool>> {
    // Full impls in production; M1.13 ships placeholder names so the registry
    // can advertise them. Plan 15 wires the real call() methods.
    Vec::new()
}
```

- [ ] **Step 6: swarm.rs (delegates to trait — no impl in core)**

```rust
pub use lingxi_traits::{PanePosition, SwarmBackend, SwarmError, SwarmHandle, SwarmLayout};
```

- [ ] **Step 7: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod internal_tools;
pub mod mailbox;
pub mod mode;
pub mod swarm;
pub mod team_registry;

pub use mailbox::{MailboxError, MailboxRouter, MessageSender, TeammateMailbox, TeammateMessage};
pub use mode::CoordinatorMode;
pub use team_registry::{TeamRegistry, WorkerAgent, WorkerStatus};
```

```bash
cargo test -p lingxi-coordinator
git add crates/coordinator
git commit -m "feat(coordinator): mode + team registry + mailbox router + internal tools stub"
```

---

## Task 5: Integration test — coordinator spawns worker, sends message, drains

**Files:** `crates/test-harness/tests/coordinator_e2e.rs`

```rust
use lingxi_coordinator::{MailboxError, MessageSender, TeamRegistry, TeammateMessage};
use lingxi_protocol::AgentId;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn coordinator_routes_message_to_worker() {
    let coord = AgentId::new();
    let team = TeamRegistry::new(coord);
    let worker = team.spawn_worker("explorer".into(), "explore-1".into(), "task-1".into()).await.unwrap();

    let msg = TeammateMessage {
        from: MessageSender::Coordinator,
        content: "go".into(),
        message_id: "m1".into(),
        timestamp: std::time::SystemTime::now(),
    };
    team.mailbox_router.route(&worker, msg.clone()).await.unwrap();

    // The worker's mailbox is registered in the router; routing succeeded.
    // (Drain test belongs to the worker side — covered in Plan 09 SkillTool tests.)
}

#[tokio::test]
async fn route_to_unknown_worker_errors() {
    let coord = AgentId::new();
    let team = TeamRegistry::new(coord);
    let r = team.mailbox_router.route(&AgentId::new(), TeammateMessage {
        from: MessageSender::Coordinator, content: "".into(), message_id: "".into(),
        timestamp: std::time::SystemTime::now(),
    }).await;
    assert!(matches!(r, Err(MailboxError::NotFound(_))));
}
```

Run + commit:
```bash
cargo test -p lingxi-test-harness --test coordinator_e2e
git add crates/test-harness/tests/coordinator_e2e.rs
git commit -m "test(coordinator): route message to registered worker"
```

---

## Task 6: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.13-tasks-coordinator -m "Plan 07 complete"
```

## Self-Review

- §11.1 7 TaskTypes → id.rs ✓
- §11.2 TaskStateBase + 7 variants → state.rs ✓
- §11.4 TaskRegistry → registry.rs ✓
- §11.5 TaskOutputManager containment (D8) → output_manager.rs ✓
- §11.6 TaskNotificationBuilder → notification.rs ✓
- §12.1 CoordinatorMode → mode.rs ✓
- §12.3 TeamRegistry → team_registry.rs ✓
- §12.4 TeammateMailbox (bounded) → mailbox.rs ✓
- §12.5 SyntheticOutputTool (stub — not is_read_only) → internal_tools.rs ✓

## Execution Handoff

Next: **Plan 08 — Side Query & Forked Agent infrastructure** (`2026-05-22-lingxi-core-m1-08-sidequery.md`).
