# LingXi Core M1 · Plan 06 · Agent & Subagent

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Build the subagent runtime: `StateMachinePool` for sibling slot allocation (effect-delegated, not nested), `SubagentContext` builder with cross-system inheritance, `MultiAgentDispatcher` for parallel spawn, `ForkSpawner` for byte-exact-cache fork mode, plus `AgentColorManager` + `AgentMemorySnapshot` + worktree degradation.

**Architecture:** `lingxi-agent` owns the pool and the cross-system context builder. Subagents are NOT nested state machines — they are sibling slots in a host pool, communicating via channels. This avoids stack growth in deep fork chains and gives unified scheduling/cancellation.

**Depends on:** Plans 01-05.

---

## File Structure

```
crates/agent/
├── Cargo.toml
└── src/{lib, definition, context, pool, runner, multi_dispatch, tool_resolver, permission_mode, color_manager, display, fork, transcript, worktree_policy}.rs

crates/platform-api/src/worktree.rs ← NEW: WorktreeManager trait
```

---

## Task 1: WorktreeManager trait

**Files:** `crates/platform-api/src/worktree.rs`

- [ ] **Step 1: Impl**

```rust
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[async_trait]
pub trait WorktreeManager: Send + Sync {
    async fn create_worktree(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        copy_includes: &[PathBuf],
    ) -> Result<WorktreeHandle, WorktreeError>;
    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError>;
    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError>;
    async fn cleanup_stale(&self, max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError>;
    fn is_supported(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeHandle {
    pub path: PathBuf,
    pub branch_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub created_at: std::time::SystemTime,
}

#[derive(Debug, Clone, Error)]
pub enum WorktreeError {
    #[error("worktree not supported on this platform")]
    Unsupported,
    #[error("git error: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(String),
}
```

- [ ] **Step 2: Re-export + commit**

`crates/platform-api/src/lib.rs`: `pub mod worktree; pub use worktree::*;`

```bash
cargo check -p lingxi-traits
git add crates/traits
git commit -m "feat(traits): WorktreeManager trait"
```

---

## Task 2: AgentDefinition + SubagentContext

**Files:** `crates/agent/{Cargo.toml, src/{lib,definition,context}.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-agent"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-core = { path = "../core" }
lingxi-tools = { path = "../tools" }
lingxi-mcp = { path = "../mcp" }
lingxi-memory = { path = "../memory" }
lingxi-hooks = { path = "../hooks" }
lingxi-permission = { path = "../permission" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: definition.rs**

```rust
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub when_to_use: String,
    pub tools: AgentToolPolicy,
    pub max_turns: u32,
    pub model: AgentModel,
    pub permission_mode: AgentPermissionMode,
    pub source: AgentSource,
    pub base_dir: PathBuf,
    pub system_prompt: Option<String>,
    pub mcp_servers: Vec<AgentMcpServerSpec>,
    pub frontmatter_hooks: Vec<lingxi_hooks::HookDefinition>,
    pub icon: Option<String>,
    pub allowed_tools: Vec<String>,
    pub worktree_requirement: Option<WorktreeRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToolPolicy {
    All { use_exact_tools: bool },
    Explicit(Vec<String>),
    Except(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentModel {
    Inherit,
    Alias(String),
    Explicit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPermissionMode { Bubble, Isolated, Auto, Plan }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource { BuiltIn, UserDefined, Project, Plugin, PolicySettings }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMcpServerSpec {
    ByName(String),
    Inline { name: String, config: lingxi_mcp::McpServerConfig },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeRequirement { Required, Optional, None }

impl AgentDefinition {
    pub fn is_fork(&self) -> bool {
        self.agent_type == "fork"
    }
}
```

- [ ] **Step 3: context.rs**

```rust
use crate::definition::AgentDefinition;
use crate::display::AgentDisplay;
use lingxi_memory::AgentMemorySnapshot;
use lingxi_protocol::{AgentId, McpConnectionId, ConversationMessage};
use lingxi_platform_api::WorktreeHandle;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use lingxi_tools::ContentReplacementState;

#[derive(Clone)]
pub struct SubagentContext {
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub agent_definition: AgentDefinition,
    pub prompt_messages: Vec<ConversationMessage>,
    pub fork_context_messages: Option<Vec<ConversationMessage>>,
    pub allowed_tools: Vec<String>,
    pub worktree_handle: Option<WorktreeHandle>,
    pub is_async: bool,
    pub can_show_permission_prompts: bool,
    pub mcp_clients: Vec<McpConnectionId>,
    pub transcript_subdir: PathBuf,
    pub rendered_system_prompt: Option<Arc<str>>,
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    pub agent_memory: Option<AgentMemorySnapshot>,
    pub display: AgentDisplay,
}
```

- [ ] **Step 4: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod color_manager;
pub mod context;
pub mod definition;
pub mod display;
pub mod fork;
pub mod multi_dispatch;
pub mod permission_mode;
pub mod pool;
pub mod runner;
pub mod tool_resolver;
pub mod transcript;
pub mod worktree_policy;

pub use color_manager::AgentColorManager;
pub use context::SubagentContext;
pub use definition::*;
pub use display::AgentDisplay;
pub use pool::{StateMachinePool, StateMachineSlot};
pub use multi_dispatch::{MultiAgentDispatcher, MultiAgentSpawnSpec};
pub use runner::SubagentEvent;
pub use tool_resolver::AgentToolResolver;
pub use worktree_policy::create_worktree_or_degrade;
```

- [ ] **Step 5: Stub remaining files (color/display/fork/etc.)** — each contains a `pub struct X;` with TODO-by-plan markers so the crate compiles.

```rust
// display.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDisplay {
    pub color: AgentColor,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentColor { Cyan, Magenta, Yellow, Green, Blue, Red, Orange, Purple, Pink, Teal }
```

```rust
// color_manager.rs
use crate::display::AgentColor;
use lingxi_protocol::AgentId;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

pub struct AgentColorManager {
    available: Vec<AgentColor>,
    assigned: Mutex<HashMap<AgentId, AgentColor>>,
    recent: Mutex<VecDeque<AgentColor>>,
}

impl AgentColorManager {
    pub fn new() -> Self {
        Self {
            available: vec![
                AgentColor::Cyan, AgentColor::Magenta, AgentColor::Yellow,
                AgentColor::Green, AgentColor::Blue, AgentColor::Red,
                AgentColor::Orange, AgentColor::Purple, AgentColor::Pink, AgentColor::Teal,
            ],
            assigned: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    pub fn assign(&self, agent: &AgentId) -> AgentColor {
        let mut assigned = self.assigned.lock().unwrap();
        if let Some(c) = assigned.get(agent) { return *c; }
        let recent = self.recent.lock().unwrap();
        let color = self.available.iter()
            .find(|c| !recent.contains(c))
            .copied()
            .unwrap_or(AgentColor::Cyan);
        assigned.insert(*agent, color);
        drop(recent);
        let mut recent = self.recent.lock().unwrap();
        recent.push_back(color);
        if recent.len() > 5 { recent.pop_front(); }
        color
    }

    pub fn release(&self, agent: &AgentId) {
        self.assigned.lock().unwrap().remove(agent);
    }
}

impl Default for AgentColorManager { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_agents_get_distinct_colors() {
        let m = AgentColorManager::new();
        let a = AgentId::new();
        let b = AgentId::new();
        assert_ne!(m.assign(&a), m.assign(&b));
    }
}
```

```rust
// fork.rs
pub struct ForkSpawner; // full impl in Plan 08 (SideQuery & Forked Agent)

pub const FORK_BOILERPLATE_TAG: &str = "<fork-boilerplate>";
pub const FORK_DIRECTIVE_PREFIX: &str = "<fork-directive>";
```

```rust
// permission_mode.rs — already covered by AgentPermissionMode in definition.rs
```

```rust
// tool_resolver.rs
use crate::definition::{AgentDefinition, AgentPermissionMode, AgentToolPolicy};
use lingxi_tools::Tool;
use std::sync::Arc;

pub struct AgentToolResolver;

impl AgentToolResolver {
    pub fn resolve(
        agent_def: &AgentDefinition,
        parent_tools: &[Arc<dyn Tool>],
        agent_mcp_tools: &[Arc<dyn Tool>],
        _coordinator_mode: bool,
    ) -> Vec<Arc<dyn Tool>> {
        let mut tools = match &agent_def.tools {
            AgentToolPolicy::All { use_exact_tools } => {
                if *use_exact_tools { parent_tools.to_vec() }
                else { parent_tools.to_vec() }
            }
            AgentToolPolicy::Explicit(names) => parent_tools.iter()
                .filter(|t| names.contains(&t.name().to_string()))
                .cloned().collect(),
            AgentToolPolicy::Except(names) => parent_tools.iter()
                .filter(|t| !names.contains(&t.name().to_string()))
                .cloned().collect(),
        };
        tools.extend(agent_mcp_tools.iter().cloned());
        if agent_def.permission_mode == AgentPermissionMode::Plan {
            tools.retain(|t| matches!(t.name(), "Read" | "Grep" | "Glob" | "WebSearch" | "WebFetch"));
        }
        tools
    }
}
```

```rust
// transcript.rs
use lingxi_protocol::{AgentId, ConversationMessage};
use lingxi_platform_api::FileSystem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub agent_id: AgentId,
    pub timestamp: SystemTime,
    pub message: ConversationMessage,
}

pub struct AgentTranscriptWriter {
    pub transcript_path: PathBuf,
    pub agent_id: AgentId,
    fs: Arc<dyn FileSystem>,
}

impl AgentTranscriptWriter {
    pub fn new(transcript_path: PathBuf, agent_id: AgentId, fs: Arc<dyn FileSystem>) -> Self {
        Self { transcript_path, agent_id, fs }
    }

    pub async fn record(&self, message: &ConversationMessage) -> Result<(), lingxi_platform_api::FsError> {
        let entry = TranscriptEntry { agent_id: self.agent_id, timestamp: SystemTime::now(), message: message.clone() };
        let line = format!("{}\n", serde_json::to_string(&entry).unwrap());
        // FileSystem.append_file added in Plan 10. For now use write+read concat:
        let existing = self.fs.read_file(self.transcript_path.to_str().unwrap(), None, None).await
            .map(|fc| fc.content).unwrap_or_default();
        self.fs.write_file(self.transcript_path.to_str().unwrap(), &(existing + &line)).await
    }
}
```

```rust
// worktree_policy.rs
use crate::definition::WorktreeRequirement;
use lingxi_platform_api::{WorktreeError, WorktreeHandle, WorktreeManager};

pub async fn create_worktree_or_degrade(
    manager: &dyn WorktreeManager,
    requirement: WorktreeRequirement,
    slug: &str,
) -> Result<Option<WorktreeHandle>, WorktreeError> {
    match (requirement, manager.is_supported()) {
        (WorktreeRequirement::Required, false) => Err(WorktreeError::Unsupported),
        (WorktreeRequirement::Required, true) => Ok(Some(manager.create_worktree(slug, None, &[]).await?)),
        (WorktreeRequirement::Optional, true) => {
            match manager.create_worktree(slug, None, &[]).await {
                Ok(h) => Ok(Some(h)),
                Err(e) => {
                    tracing::warn!("worktree degraded: {e}");
                    Ok(None)
                }
            }
        }
        (WorktreeRequirement::Optional, false) | (WorktreeRequirement::None, _) => Ok(None),
    }
}
```

- [ ] **Step 6: Commit**

```bash
cargo check -p lingxi-agent
git add crates/agent
git commit -m "feat(agent): AgentDefinition, SubagentContext, ColorManager, ToolResolver, Worktree policy"
```

---

## Task 3: StateMachinePool (effect-delegated)

**Files:** `crates/agent/src/{pool,runner}.rs`

- [ ] **Step 1: pool.rs**

```rust
use crate::context::SubagentContext;
use crate::runner::SubagentEvent;
use lingxi_protocol::AgentId;
use lingxi_platform_api::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub struct StateMachineSlot {
    pub agent_id: AgentId,
    pub task: BackgroundTaskHandle,
    pub event_tx: mpsc::Sender<lingxi_core::Event>,
}

pub struct StateMachinePool {
    slots: Arc<RwLock<HashMap<AgentId, StateMachineSlot>>>,
    max_concurrent: usize,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl StateMachinePool {
    pub fn new(runtime: Arc<dyn RuntimeSpawner>, max_concurrent: usize) -> Self {
        Self { slots: Arc::new(RwLock::new(HashMap::new())), max_concurrent, runtime }
    }

    /// Allocate a slot. Returns (agent_id, event-source receiver).
    /// The host drives `event_tx` (channels are owned by the slot table).
    pub async fn allocate(
        &self,
        ctx: SubagentContext,
    ) -> Result<(AgentId, mpsc::Receiver<SubagentEvent>), PoolError> {
        if self.slots.read().await.len() >= self.max_concurrent {
            return Err(PoolError::TooManyAgents);
        }
        let agent_id = ctx.agent_id;
        let (event_tx, event_rx) = mpsc::channel::<lingxi_core::Event>(100);
        let (out_tx, out_rx) = mpsc::channel::<SubagentEvent>(100);

        let task = self.runtime.spawn(
            "subagent-state-machine",
            Box::pin(crate::runner::run_subagent(ctx, event_rx, out_tx)),
        ).await.map_err(PoolError::Runtime)?;

        self.slots.write().await.insert(agent_id, StateMachineSlot { agent_id, task, event_tx });
        Ok((agent_id, out_rx))
    }

    pub async fn deallocate(&self, agent_id: &AgentId) -> Result<(), PoolError> {
        let slot = self.slots.write().await.remove(agent_id);
        if let Some(slot) = slot {
            self.runtime.cancel(&slot.task).await.map_err(PoolError::Runtime)?;
        }
        Ok(())
    }

    pub async fn slot_count(&self) -> usize { self.slots.read().await.len() }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("too many agents — pool full")]
    TooManyAgents,
    #[error(transparent)]
    Runtime(RuntimeError),
}
```

- [ ] **Step 2: runner.rs**

```rust
use crate::context::SubagentContext;
use lingxi_protocol::AgentId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentEvent {
    Progress { agent_id: AgentId, tool_use_count: u32, token_count: u64 },
    Completed { agent_id: AgentId, result: serde_json::Value },
    Failed { agent_id: AgentId, error: String },
    Killed { agent_id: AgentId },
    Message { agent_id: AgentId, message: serde_json::Value },
}

/// The actual subagent state machine loop. Drives `lingxi_core::reduce` over
/// `event_rx` and emits `out_tx` SubagentEvents. M1.11 ships a stub completion
/// after the first event; full agentic loop in Plan 09+ uses §22 SessionStorage
/// and §23 FileStateCache.
pub async fn run_subagent(
    ctx: SubagentContext,
    mut event_rx: mpsc::Receiver<lingxi_core::Event>,
    out_tx: mpsc::Sender<SubagentEvent>,
) {
    let _ = event_rx.recv().await;
    let _ = out_tx.send(SubagentEvent::Completed {
        agent_id: ctx.agent_id,
        result: serde_json::json!({"stub": true}),
    }).await;
}
```

- [ ] **Step 3: multi_dispatch.rs**

```rust
use crate::definition::AgentDefinition;
use crate::pool::{PoolError, StateMachinePool};
use crate::runner::SubagentEvent;
use lingxi_protocol::AgentId;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct MultiAgentSpawnSpec {
    pub agent_type: String,
    pub name: String,
    pub initial_prompt: String,
    pub agent_def_override: Option<AgentDefinition>,
}

pub struct MultiAgentDispatcher {
    pool: Arc<StateMachinePool>,
}

impl MultiAgentDispatcher {
    pub fn new(pool: Arc<StateMachinePool>) -> Self { Self { pool } }

    pub async fn spawn_multi(
        &self,
        specs: Vec<MultiAgentSpawnSpec>,
        _coordinator: AgentId,
        contexts: Vec<crate::context::SubagentContext>,
    ) -> Result<Vec<(AgentId, mpsc::Receiver<SubagentEvent>)>, PoolError> {
        let mut out = Vec::with_capacity(specs.len());
        for ctx in contexts {
            out.push(self.pool.allocate(ctx).await?);
        }
        Ok(out)
    }
}
```

- [ ] **Step 4: Test allocation**

```rust
// add to pool.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SubagentContext;
    use crate::definition::{AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy};
    use crate::display::{AgentColor, AgentDisplay};
    use lingxi_test_harness::mocks::MockRuntimeSpawner;
    use std::sync::Arc;

    #[tokio::test]
    async fn allocate_and_deallocate() {
        let runtime = Arc::new(MockRuntimeSpawner::default());
        let pool = StateMachinePool::new(runtime, 5);

        let ctx = SubagentContext {
            agent_id: AgentId::new(),
            parent_agent_id: None,
            agent_definition: AgentDefinition {
                agent_type: "test".into(),
                when_to_use: "".into(),
                tools: AgentToolPolicy::All { use_exact_tools: true },
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
            display: AgentDisplay { color: AgentColor::Cyan, icon: None },
        };
        let aid = ctx.agent_id;
        let (id, _rx) = pool.allocate(ctx).await.unwrap();
        assert_eq!(id, aid);
        assert_eq!(pool.slot_count().await, 1);
        pool.deallocate(&aid).await.unwrap();
        assert_eq!(pool.slot_count().await, 0);
    }
}
```

- [ ] **Step 5: Add `lingxi-test-harness` to agent dev-deps**

```toml
[dev-dependencies]
lingxi-test-harness = { path = "../test-harness" }
```

- [ ] **Step 6: Run + commit**

```bash
cargo test -p lingxi-agent
git add crates/agent
git commit -m "feat(agent): StateMachinePool + SubagentEvent + MultiAgentDispatcher"
```

---

## Task 4: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.11-agent -m "Plan 06 complete"
```

## Self-Review

- §10.2 AgentDefinition → definition.rs ✓
- §10.3 SubagentContext → context.rs ✓
- §10.5 StateMachinePool (effect-delegated) → pool.rs ✓
- §10.6 MultiAgentDispatcher → multi_dispatch.rs ✓
- §10.7 Worktree degradation → worktree_policy.rs ✓
- §10.8 AgentToolResolver → tool_resolver.rs ✓
- §10.10 AgentColorManager → color_manager.rs ✓

## Execution Handoff

Next: **Plan 07 — Tasks & Coordinator** (`2026-05-22-lingxi-core-m1-07-tasks-coordinator.md`).
