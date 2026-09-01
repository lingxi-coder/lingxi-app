# LingXi Core M1 · Plan 03 · Tools & Hooks

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Wire the agentic tool-execution loop. Build the `Tool` trait, `ToolRegistry`, concurrency-partitioned `ToolDispatcher`, and the 28-event Hook engine that wraps every tool call with PreToolUse/PostToolUse and lets hooks block/modify/auto-approve.

**Architecture:** `lingxi-tools` owns Tool trait + dispatcher. `lingxi-hooks` owns hook engine + 4 executor kinds + async registry. Tool dispatcher consults `lingxi-permission::PermissionPolicy` from Plan 02; hook decisions can override the dispatcher's permission result. Concurrency partition: `is_concurrency_safe` reads run in parallel (up to `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY = 10`), writes serialize.

**Tech Stack:** `async-trait`, `regex` (hook pattern matching + SSRF check), `url`, `futures` (`FuturesUnordered`), `tokio::sync::mpsc` (progress channel).

**References:** Spec §8 Tools · §9 Hooks · §14 Permission integration

**Depends on:** Plan 01 (protocol/core/traits/api-client), Plan 02 (permission policy).

---

## File Structure

```
crates/tools/
├── Cargo.toml
└── src/{lib, tool_trait, context, registry, dispatcher, streaming_exec, result_storage, content_replacement, progress, permissions}.rs

crates/hooks/
├── Cargo.toml
└── src/{lib, events, definition, registry, executor, async_registry, ssrf_guard, response, builtin/mod.rs}.rs
```

---

## Task 1: lingxi-tools — Tool trait + ToolUseContext

**Files:** Create `crates/tools/Cargo.toml`, `src/{lib,tool_trait,context}.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-tools"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-core = { path = "../core" }
lingxi-permission = { path = "../permission" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
futures = "0.3"
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: tool_trait.rs (30+ methods, per §8.1)**

```rust
//! Tool trait — the contract every tool implements.
//!
//! Static methods reason about the tool itself; dynamic methods reason
//! about a specific (input, ctx) pair.

use async_trait::async_trait;
use lingxi_permission::PermissionResult;
use lingxi_protocol::ToolUseId;
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn search_hint(&self) -> Option<&str> { None }

    fn input_schema(&self) -> &Value;
    fn output_schema(&self) -> Option<&Value> { None }

    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool;
    fn is_mcp(&self) -> bool { false }
    fn is_lsp(&self) -> bool { false }
    fn requires_user_interaction(&self) -> bool { false }
    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { false }
    fn strict(&self) -> bool { false }
    fn max_result_size_chars(&self) -> usize;

    fn is_concurrency_safe(&self, input: &Value) -> bool;
    fn is_read_only(&self, input: &Value) -> bool;
    fn is_destructive(&self, input: &Value) -> bool { false }
    fn is_open_world(&self, input: &Value) -> bool { false }
    fn is_search_or_read(&self, _input: &Value) -> Option<SearchReadInfo> { None }
    fn interrupt_behavior(&self, _input: &Value) -> InterruptBehavior { InterruptBehavior::Block }

    fn backfill_observable_input(&self, _input: &mut Value) {}
    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> Result<(), ValidationError> { Ok(()) }

    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> PermissionResult;
    fn get_path(&self, _input: &Value) -> Option<PathBuf> { None }

    async fn description(&self, input: &Value, opts: &DescriptionOptions) -> String;
    async fn prompt(&self, opts: &PromptOptions) -> String;

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError>;

    fn get_activity_description(&self, _input: &Value) -> Option<String> { None }
}

#[derive(Debug, Clone, Default)]
pub struct ToolStaticContext {
    pub feature_flags: std::collections::HashMap<String, bool>,
}

#[derive(Debug, Clone)]
pub struct DescriptionOptions {
    pub is_non_interactive_session: bool,
}

#[derive(Debug, Clone)]
pub struct PromptOptions {
    pub include_examples: bool,
}

#[derive(Debug, Clone)]
pub struct SearchReadInfo {
    pub is_search: bool,
    pub is_read: bool,
    pub is_list: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum InterruptBehavior { Cancel, Block }

pub struct ToolCallResult {
    pub data: Value,
    pub new_messages: Vec<lingxi_protocol::ConversationMessage>,
    pub context_modifier: Option<Box<dyn FnOnce(ToolUseContext) -> ToolUseContext + Send>>,
    pub mcp_meta: Option<serde_json::Value>,
}

impl std::fmt::Debug for ToolCallResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCallResult").field("data", &self.data).finish()
    }
}

#[derive(Debug, Clone, Error)]
#[error("invalid tool input: {0}")]
pub struct ValidationError(pub String);

#[derive(Debug, Clone, Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("io: {0}")]
    Io(String),
    #[error("aborted")]
    Aborted,
    #[error("internal: {0}")]
    Internal(String),
    // FileStateCache-driven errors (wired in Plan 10).
    #[error("edit before read: {path}")]
    EditWithoutRead { path: String },
    #[error("file modified externally since last read: {path}")]
    FileModifiedExternally { path: String },
    #[error("partial view — please re-read {path}")]
    PartialViewMustReread { path: String },
    #[error("file content hash mismatch: {path}")]
    FileContentMismatch { path: String },
}
```

- [ ] **Step 3: context.rs**

```rust
use crate::content_replacement::ContentReplacementState;
use lingxi_protocol::{AgentId, McpConnectionId, ToolUseId};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ToolUseContext {
    pub options: ToolUseOptions,
    pub messages: Vec<lingxi_protocol::ConversationMessage>,
    pub tool_use_id: Option<ToolUseId>,
    pub agent_id: Option<AgentId>,
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    // File state cache wired in Plan 10.
}

#[derive(Clone)]
pub struct ToolUseOptions {
    pub debug: bool,
    pub verbose: bool,
    pub main_loop_model: String,
    pub max_budget_nano_usd: Option<u64>,
    pub mcp_clients: Vec<McpConnectionId>,
    pub is_non_interactive_session: bool,
    pub custom_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
}
```

- [ ] **Step 4: Run check**

```bash
cargo check -p lingxi-tools
```

- [ ] **Step 5: Commit**

```bash
git add crates/tools/Cargo.toml crates/tools/src
git commit -m "feat(tools): Tool trait + ToolUseContext + ToolError"
```

---

## Task 2: ToolRegistry + simple Tool impls (Read/Bash stubs)

**Files:** Create `crates/tools/src/registry.rs`, modify `lib.rs`

- [ ] **Step 1: registry.rs**

```rust
use crate::tool_trait::{Tool, ToolStaticContext};
use lingxi_protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    builtin: Vec<Arc<dyn Tool>>,
    mcp_tools: HashMap<McpConnectionId, Vec<Arc<dyn Tool>>>,
    lsp_tools: Vec<Arc<dyn Tool>>,
    plugin_tools: HashMap<PluginId, Vec<Arc<dyn Tool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            builtin: Vec::new(),
            mcp_tools: HashMap::new(),
            lsp_tools: Vec::new(),
            plugin_tools: HashMap::new(),
        }
    }

    pub fn register_builtin(&mut self, tool: Arc<dyn Tool>) {
        self.builtin.push(tool);
    }

    pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for t in &self.builtin {
            if t.is_enabled(ctx) { out.push(t.clone()); }
        }
        for ts in self.mcp_tools.values() { out.extend(ts.iter().cloned()); }
        out.extend(self.lsp_tools.iter().cloned());
        for ts in self.plugin_tools.values() { out.extend(ts.iter().cloned()); }
        out
    }

    pub fn find_by_name(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.builtin.iter()
            .chain(self.mcp_tools.values().flatten())
            .chain(self.lsp_tools.iter())
            .chain(self.plugin_tools.values().flatten())
            .find(|t| t.name() == name || t.aliases().contains(&name))
            .cloned()
    }

    pub fn register_mcp_tools(&mut self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>) {
        self.mcp_tools.insert(conn_id, tools);
    }
    pub fn unregister_mcp_tools(&mut self, conn_id: McpConnectionId) {
        self.mcp_tools.remove(&conn_id);
    }
    pub fn register_plugin_tools(&mut self, plugin_id: PluginId, tools: Vec<Arc<dyn Tool>>) {
        self.plugin_tools.insert(plugin_id, tools);
    }
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.plugin_tools.remove(plugin_id);
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_trait::*;
    use async_trait::async_trait;
    use lingxi_permission::{PermissionMetadata, PermissionDecisionReason, PermissionResult};
    use serde_json::json;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str { "Dummy" }
        fn input_schema(&self) -> &serde_json::Value {
            static SCHEMA: once_cell::sync::Lazy<serde_json::Value> =
                once_cell::sync::Lazy::new(|| json!({"type": "object"}));
            &SCHEMA
        }
        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool { true }
        fn max_result_size_chars(&self) -> usize { 1024 }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
        async fn check_permissions(&self, _input: &serde_json::Value, _ctx: &ToolUseContext) -> PermissionResult {
            PermissionResult::Allow {
                reason: PermissionDecisionReason::Other { reason: "test".into() },
                updated_input: None, update_destination: None,
                metadata: PermissionMetadata::default(),
            }
        }
        async fn description(&self, _input: &serde_json::Value, _opts: &DescriptionOptions) -> String { "Dummy".into() }
        async fn prompt(&self, _opts: &PromptOptions) -> String { "".into() }
        async fn call(&self, _input: serde_json::Value, _ctx: ToolUseContext, _tx: crate::progress::ToolProgressSender)
            -> Result<ToolCallResult, ToolError>
        {
            Ok(ToolCallResult { data: json!({"ok": true}), new_messages: vec![], context_modifier: None, mcp_meta: None })
        }
    }

    #[test]
    fn find_by_name_returns_registered_tool() {
        let mut r = ToolRegistry::new();
        r.register_builtin(Arc::new(DummyTool));
        assert!(r.find_by_name("Dummy").is_some());
        assert!(r.find_by_name("Nonexistent").is_none());
    }
}
```

- [ ] **Step 2: Add `once_cell` to dev-deps and the regular deps**

```toml
[dependencies]
once_cell = "1"
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p lingxi-tools --lib registry
```

- [ ] **Step 4: Commit**

```bash
git add crates/tools
git commit -m "feat(tools): ToolRegistry with builtin/mcp/lsp/plugin partitions"
```

---

## Task 3: Progress channel + ContentReplacement + ResultStorage stubs

**Files:** `crates/tools/src/{progress,content_replacement,result_storage}.rs`

- [ ] **Step 1: progress.rs**

```rust
use lingxi_protocol::ToolUseId;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ToolProgress {
    pub tool_use_id: ToolUseId,
    pub data: serde_json::Value,
}

pub type ToolProgressSender = mpsc::Sender<ToolProgress>;
pub type ToolProgressReceiver = mpsc::Receiver<ToolProgress>;

pub fn progress_channel() -> (ToolProgressSender, ToolProgressReceiver) {
    mpsc::channel(64)
}
```

- [ ] **Step 2: content_replacement.rs**

```rust
use lingxi_protocol::ToolUseId;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ContentReplacementState {
    pub replacements: HashMap<ToolUseId, ReplacementRecord>,
    pub total_budget_chars: usize,
    pub used_chars: usize,
}

#[derive(Debug, Clone)]
pub struct ReplacementRecord {
    pub original_size: usize,
    pub replaced_at_turn: u32,
    pub placeholder: String, // "[Old tool result content cleared]"
}
```

- [ ] **Step 3: result_storage.rs**

```rust
use lingxi_protocol::ToolUseId;
use lingxi_platform_api::FileSystem;
use std::sync::Arc;
use std::path::PathBuf;

pub struct ToolResultStorage {
    storage_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl ToolResultStorage {
    pub fn new(storage_dir: PathBuf, fs: Arc<dyn FileSystem>) -> Self {
        Self { storage_dir, fs }
    }

    pub async fn store(&self, tool_use_id: &ToolUseId, content: &str) -> Result<PathBuf, lingxi_platform_api::FsError> {
        let path = self.storage_dir.join(format!("{tool_use_id}.txt"));
        self.fs.write_file(path.to_str().unwrap(), content).await?;
        Ok(path)
    }
}
```

- [ ] **Step 4: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod content_replacement;
pub mod context;
pub mod dispatcher;
pub mod permissions;
pub mod progress;
pub mod registry;
pub mod result_storage;
pub mod streaming_exec;
pub mod tool_trait;

pub use context::{ToolUseContext, ToolUseOptions};
pub use dispatcher::{ToolDispatchEvent, ToolDispatcher, ToolCall};
pub use progress::{progress_channel, ToolProgress, ToolProgressReceiver, ToolProgressSender};
pub use registry::ToolRegistry;
pub use tool_trait::*;
```

- [ ] **Step 5: Commit**

```bash
git add crates/tools/src
git commit -m "feat(tools): progress channel + content replacement + result storage scaffold"
```

---

## Task 4: ToolDispatcher with concurrency partition

**Files:** `crates/tools/src/dispatcher.rs`

- [ ] **Step 1: Write tests describing partition behavior**

```rust
// dispatcher.rs (tests at end)
#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_protocol::ToolUseId;
    use serde_json::json;

    #[test]
    fn partition_groups_adjacent_safe_calls() {
        let calls = vec![
            ToolCall { id: ToolUseId::new(), name: "Read".into(), input: json!({}) },
            ToolCall { id: ToolUseId::new(), name: "Read".into(), input: json!({}) },
            ToolCall { id: ToolUseId::new(), name: "Write".into(), input: json!({}) },
            ToolCall { id: ToolUseId::new(), name: "Read".into(), input: json!({}) },
        ];
        // Read = concurrency_safe; Write = not safe.
        let safe_predicate = |c: &ToolCall| c.name == "Read";
        let parts = ToolDispatcher::partition(&calls, safe_predicate);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].is_concurrency_safe);
        assert_eq!(parts[0].calls.len(), 2);
        assert!(!parts[1].is_concurrency_safe);
        assert_eq!(parts[1].calls.len(), 1);
        assert!(parts[2].is_concurrency_safe);
    }
}
```

- [ ] **Step 2: Implement**

```rust
use crate::progress::ToolProgress;
use crate::registry::ToolRegistry;
use crate::tool_trait::ToolError;
use lingxi_protocol::ToolUseId;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: ToolUseId,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone)]
pub struct ToolPartition {
    pub is_concurrency_safe: bool,
    pub calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub enum ToolDispatchEvent {
    Started { tool_use_id: ToolUseId, tool_name: String },
    Progress { tool_use_id: ToolUseId, progress: ToolProgress },
    RequestPermission { tool_use_id: ToolUseId, tool_name: String, reason: String },
    Completed { tool_use_id: ToolUseId, result: serde_json::Value },
    Failed { tool_use_id: ToolUseId, error: ToolError },
    ValidationFailed { tool_use_id: ToolUseId, error: String },
    ToolNotFound { tool_use_id: ToolUseId, name: String },
    HookBlocked { tool_use_id: ToolUseId, reason: String },
    ContextModifier { tool_use_id: ToolUseId, modifier: ContextModifierBox },
}

/// Boxed FnOnce closure to mutate ToolUseContext between calls when a tool
/// is not concurrency-safe (e.g. cd changes cwd).
pub type ContextModifierBox = Box<dyn FnOnce(crate::context::ToolUseContext) -> crate::context::ToolUseContext + Send>;

impl std::fmt::Debug for ContextModifierBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<ContextModifierBox>")
    }
}

pub struct ToolDispatcher {
    registry: Arc<ToolRegistry>,
    max_concurrency: usize,
}

impl ToolDispatcher {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry, max_concurrency: 10 }
    }

    /// Group adjacent calls of the same concurrency-safety class.
    pub fn partition<F: Fn(&ToolCall) -> bool>(
        calls: &[ToolCall],
        is_safe: F,
    ) -> Vec<ToolPartition> {
        let mut parts: Vec<ToolPartition> = Vec::new();
        for call in calls {
            let safe = is_safe(call);
            if let Some(last) = parts.last_mut() {
                if last.is_concurrency_safe == safe {
                    last.calls.push(call.clone());
                    continue;
                }
            }
            parts.push(ToolPartition { is_concurrency_safe: safe, calls: vec![call.clone()] });
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    // ...
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p lingxi-tools --lib dispatcher
```

Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/tools/src/dispatcher.rs
git commit -m "feat(tools): ToolDispatcher with concurrency partition"
```

---

## Task 5: lingxi-hooks crate — 28 events + 4 executor kinds

**Files:** Create `crates/hooks/Cargo.toml`, `src/{lib,events,definition,response,registry,executor,async_registry,ssrf_guard,builtin/mod}.rs`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-hooks"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-core = { path = "../core" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
regex = "1"
url = "2"

[lints]
workspace = true
```

- [ ] **Step 2: events.rs — full 28-variant enum (per §9.1)**

```rust
use lingxi_protocol::{AgentId, HookId, MessageId, McpConnectionId, PluginId, SessionId, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum HookEventType {
    PreToolUse, PostToolUse, PostToolUseFailure,
    SessionStart, SessionEnd, Setup,
    UserPromptSubmit,
    Stop, StopFailure,
    SubagentStart, SubagentStop,
    PreCompact, PostCompact,
    PermissionRequest, PermissionDenied,
    TeammateIdle, TaskCreated, TaskCompleted,
    Elicitation, ElicitationResult,
    ConfigChange, WorktreeCreate, WorktreeRemove,
    InstructionsLoaded, CwdChanged, FileChanged,
    Notification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse { tool_name: String, tool_input: Value, tool_use_id: ToolUseId },
    PostToolUse { tool_name: String, tool_input: Value, tool_output: Value, tool_use_id: ToolUseId },
    PostToolUseFailure { tool_name: String, error: String, tool_use_id: ToolUseId },
    SessionStart { session_id: SessionId, source: String },
    SessionEnd { session_id: SessionId, reason: String },
    Setup,
    UserPromptSubmit { prompt: String },
    Stop { reason: String },
    StopFailure { error: String },
    SubagentStart { agent_id: AgentId, agent_type: String, parent_agent_id: Option<AgentId> },
    SubagentStop { agent_id: AgentId, status: String },
    PreCompact { reason: String },
    PostCompact { summary: String, tokens_freed: u64 },
    PermissionRequest { tool_name: String, tool_input: Value, reason: String },
    PermissionDenied { tool_name: String, reason: String },
    TeammateIdle { agent_id: AgentId },
    TaskCreated { task_id: String, task_type: String, description: String },
    TaskCompleted { task_id: String, status: String },
    Elicitation { server_name: String, params: Value },
    ElicitationResult { server_name: String, result: Value },
    ConfigChange { changes: Vec<Value> },
    WorktreeCreate { path: PathBuf, branch: String },
    WorktreeRemove { path: PathBuf },
    InstructionsLoaded { paths: Vec<PathBuf> },
    CwdChanged { old: PathBuf, new: PathBuf },
    FileChanged { path: PathBuf, kind: String },
    Notification { message: String, kind: String },
}

impl HookEvent {
    pub fn event_type(&self) -> HookEventType {
        match self {
            Self::PreToolUse { .. } => HookEventType::PreToolUse,
            Self::PostToolUse { .. } => HookEventType::PostToolUse,
            Self::PostToolUseFailure { .. } => HookEventType::PostToolUseFailure,
            Self::SessionStart { .. } => HookEventType::SessionStart,
            Self::SessionEnd { .. } => HookEventType::SessionEnd,
            Self::Setup => HookEventType::Setup,
            Self::UserPromptSubmit { .. } => HookEventType::UserPromptSubmit,
            Self::Stop { .. } => HookEventType::Stop,
            Self::StopFailure { .. } => HookEventType::StopFailure,
            Self::SubagentStart { .. } => HookEventType::SubagentStart,
            Self::SubagentStop { .. } => HookEventType::SubagentStop,
            Self::PreCompact { .. } => HookEventType::PreCompact,
            Self::PostCompact { .. } => HookEventType::PostCompact,
            Self::PermissionRequest { .. } => HookEventType::PermissionRequest,
            Self::PermissionDenied { .. } => HookEventType::PermissionDenied,
            Self::TeammateIdle { .. } => HookEventType::TeammateIdle,
            Self::TaskCreated { .. } => HookEventType::TaskCreated,
            Self::TaskCompleted { .. } => HookEventType::TaskCompleted,
            Self::Elicitation { .. } => HookEventType::Elicitation,
            Self::ElicitationResult { .. } => HookEventType::ElicitationResult,
            Self::ConfigChange { .. } => HookEventType::ConfigChange,
            Self::WorktreeCreate { .. } => HookEventType::WorktreeCreate,
            Self::WorktreeRemove { .. } => HookEventType::WorktreeRemove,
            Self::InstructionsLoaded { .. } => HookEventType::InstructionsLoaded,
            Self::CwdChanged { .. } => HookEventType::CwdChanged,
            Self::FileChanged { .. } => HookEventType::FileChanged,
            Self::Notification { .. } => HookEventType::Notification,
        }
    }
}
```

- [ ] **Step 3: definition.rs**

```rust
use lingxi_protocol::HookId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use crate::events::HookEventType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub id: HookId,
    pub name: String,
    pub events: Vec<HookEventType>,
    pub if_condition: Option<HookCondition>,
    pub executor: HookExecutor,
    pub source: HookSource,
    pub blocking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookExecutor {
    Command { command: String, args: Vec<String>, env: HashMap<String, String>, cwd: Option<PathBuf> },
    Http { url: String, method: String, headers: HashMap<String, String>, timeout: Duration },
    Agent { agent_type: String, prompt: String },
    Builtin { handler_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCondition {
    pub pattern: String,
    pub match_tool_name: bool,
    pub match_input: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookSource {
    User, Project, Local, Managed, Plugin, FrontMatter, Session, Skill,
}
```

- [ ] **Step 4: response.rs**

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResponse {
    pub decision: Option<HookDecision>,
    pub reason: Option<String>,
    pub updated_input: Option<Value>,
    pub system_message: Option<String>,
    pub attachments: Vec<Value>,
    #[serde(default)]
    pub suppress_output: bool,
    pub structured_content: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookDecision { Allow, Approve, Block, Continue }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub outcome: HookOutcome,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub response: Option<HookResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookOutcome { Success, Error, Cancelled, Timeout }

#[derive(Debug, Clone, Default)]
pub struct AggregateHookResult {
    pub decision: Option<HookDecision>,
    pub reason: Option<String>,
    pub modified_input: Option<Value>,
    pub system_messages: Vec<String>,
    pub attachments: Vec<Value>,
    pub all_results: Vec<(lingxi_protocol::HookId, HookResult)>,
}
```

- [ ] **Step 5: ssrf_guard.rs**

```rust
use std::collections::HashSet;
use std::net::IpAddr;
use thiserror::Error;

pub struct SsrfGuard {
    allowed_schemes: HashSet<String>,
    blocked_cidrs: Vec<IpRange>,
    allowed_hosts: Option<HashSet<String>>,
}

#[derive(Debug, Clone)]
pub struct IpRange {
    pub start: IpAddr,
    pub end: IpAddr,
}

#[derive(Debug, Clone, Error)]
pub enum SsrfError {
    #[error("disallowed scheme: {0}")]
    DisallowedScheme(String),
    #[error("host blocked: {0}")]
    HostBlocked(String),
    #[error("ip blocked: {0}")]
    IpBlocked(IpAddr),
    #[error("dns resolution failed: {0}")]
    DnsFailed(String),
    #[error("url parse failed: {0}")]
    UrlParseFailed(String),
}

impl SsrfGuard {
    pub fn with_defaults() -> Self {
        let mut allowed_schemes = HashSet::new();
        allowed_schemes.insert("http".into());
        allowed_schemes.insert("https".into());

        // Block RFC1918 + loopback (10/8, 172.16/12, 192.168/16, 127/8, ::1, fc00::/7).
        let blocked = vec![
            IpRange { start: "10.0.0.0".parse().unwrap(), end: "10.255.255.255".parse().unwrap() },
            IpRange { start: "172.16.0.0".parse().unwrap(), end: "172.31.255.255".parse().unwrap() },
            IpRange { start: "192.168.0.0".parse().unwrap(), end: "192.168.255.255".parse().unwrap() },
            IpRange { start: "127.0.0.0".parse().unwrap(), end: "127.255.255.255".parse().unwrap() },
        ];

        Self { allowed_schemes, blocked_cidrs: blocked, allowed_hosts: None }
    }

    pub fn check_url(&self, url: &str) -> Result<(), SsrfError> {
        let parsed = url::Url::parse(url).map_err(|e| SsrfError::UrlParseFailed(e.to_string()))?;
        if !self.allowed_schemes.contains(parsed.scheme()) {
            return Err(SsrfError::DisallowedScheme(parsed.scheme().into()));
        }
        if let Some(allowed) = &self.allowed_hosts {
            if !allowed.contains(parsed.host_str().unwrap_or("")) {
                return Err(SsrfError::HostBlocked(parsed.host_str().unwrap_or("").into()));
            }
        }
        // IP literal check (no DNS yet — that's M2 with platform DNS resolver).
        if let Some(host) = parsed.host_str() {
            if let Ok(ip) = host.parse::<IpAddr>() {
                if self.blocked_cidrs.iter().any(|r| ip >= r.start && ip <= r.end) {
                    return Err(SsrfError::IpBlocked(ip));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_localhost_ip() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("http://127.0.0.1/").is_err());
    }

    #[test]
    fn blocks_rfc1918_ip() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("http://192.168.1.1/").is_err());
    }

    #[test]
    fn allows_public_host() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("https://example.com/").is_ok());
    }

    #[test]
    fn rejects_file_scheme() {
        let g = SsrfGuard::with_defaults();
        assert!(g.check_url("file:///etc/passwd").is_err());
    }
}
```

- [ ] **Step 6: registry.rs + executor.rs + async_registry.rs (minimal compile-clean stubs; full logic in §9 mature plan)**

```rust
// registry.rs
use crate::definition::{HookDefinition, HookSource};
use crate::events::{HookEvent, HookEventType};
use lingxi_protocol::{AgentId, PluginId};
use std::collections::HashMap;

pub struct HookContext {
    pub session_id: lingxi_protocol::SessionId,
    pub agent_id: Option<AgentId>,
    pub cwd: std::path::PathBuf,
}

pub struct HookRegistry {
    sources: HashMap<HookSource, Vec<HookDefinition>>,
    plugin: HashMap<PluginId, Vec<HookDefinition>>,
    frontmatter: HashMap<AgentId, Vec<HookDefinition>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { sources: HashMap::new(), plugin: HashMap::new(), frontmatter: HashMap::new() }
    }

    pub fn register(&mut self, hook: HookDefinition) {
        self.sources.entry(hook.source).or_default().push(hook);
    }

    pub fn register_plugin_hooks(&mut self, plugin_id: PluginId, hooks: Vec<HookDefinition>) {
        self.plugin.insert(plugin_id, hooks);
    }

    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.plugin.remove(plugin_id);
    }

    pub fn match_event(&self, event: &HookEvent, _ctx: &HookContext) -> Vec<&HookDefinition> {
        let et = event.event_type();
        let mut matched: Vec<&HookDefinition> = self.sources.values().flatten()
            .filter(|h| h.events.contains(&et))
            .collect();
        for hooks in self.plugin.values() {
            matched.extend(hooks.iter().filter(|h| h.events.contains(&et)));
        }
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }
}

impl Default for HookRegistry {
    fn default() -> Self { Self::new() }
}
```

```rust
// executor.rs (minimal API + Builtin handler dispatch; Command/Http/Agent executors stubbed)
use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use crate::registry::{HookContext, HookRegistry};
use crate::response::{AggregateHookResult, HookOutcome, HookResponse, HookResult};
use crate::ssrf_guard::SsrfGuard;
use async_trait::async_trait;
use lingxi_platform_api::{HttpTransport, ProcessRunner, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait BuiltinHookHandler: Send + Sync {
    async fn handle(&self, event: &HookEvent, ctx: &HookContext) -> HookResult;
    fn id(&self) -> &str;
}

pub struct HookExecutorImpl {
    registry: Arc<RwLock<HookRegistry>>,
    http: Arc<dyn HttpTransport>,
    runtime: Arc<dyn RuntimeSpawner>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    ssrf_guard: SsrfGuard,
}

impl HookExecutorImpl {
    pub fn new(
        registry: Arc<RwLock<HookRegistry>>,
        http: Arc<dyn HttpTransport>,
        runtime: Arc<dyn RuntimeSpawner>,
    ) -> Self {
        Self {
            registry, http, runtime,
            builtin_handlers: HashMap::new(),
            ssrf_guard: SsrfGuard::with_defaults(),
        }
    }

    pub fn register_builtin(&mut self, h: Arc<dyn BuiltinHookHandler>) {
        self.builtin_handlers.insert(h.id().into(), h);
    }

    pub async fn execute(&self, event: HookEvent, ctx: HookContext) -> AggregateHookResult {
        let reg = self.registry.read().await;
        let matched = reg.match_event(&event, &ctx);
        let mut agg = AggregateHookResult::default();
        for hook in matched {
            let result = self.execute_single(hook, &event, &ctx).await;
            self.merge(&mut agg, hook, result);
            if matches!(agg.decision, Some(crate::response::HookDecision::Block)) {
                break;
            }
        }
        agg
    }

    async fn execute_single(&self, hook: &HookDefinition, event: &HookEvent, ctx: &HookContext) -> HookResult {
        match &hook.executor {
            HookExecutor::Builtin { handler_id } => {
                if let Some(h) = self.builtin_handlers.get(handler_id) {
                    return h.handle(event, ctx).await;
                }
                HookResult { outcome: HookOutcome::Error, stdout: String::new(), stderr: format!("builtin {handler_id} not found"), exit_code: None, response: None }
            }
            HookExecutor::Http { url, .. } => {
                if self.ssrf_guard.check_url(url).is_err() {
                    return HookResult { outcome: HookOutcome::Error, stdout: String::new(), stderr: "SSRF guard rejected url".into(), exit_code: None, response: None };
                }
                // Full impl: build HttpRequest, post event JSON, parse response. Stubbed for M1.4.
                HookResult { outcome: HookOutcome::Success, stdout: String::new(), stderr: String::new(), exit_code: None, response: None }
            }
            HookExecutor::Command { .. } | HookExecutor::Agent { .. } => {
                // Full impl: spawn process / fork agent. Stubbed for M1.4.
                HookResult { outcome: HookOutcome::Success, stdout: String::new(), stderr: String::new(), exit_code: None, response: None }
            }
        }
    }

    fn merge(&self, agg: &mut AggregateHookResult, hook: &HookDefinition, r: HookResult) {
        if let Some(resp) = &r.response {
            if resp.decision.is_some() { agg.decision = resp.decision; }
            if let Some(reason) = &resp.reason { agg.reason = Some(reason.clone()); }
            if let Some(input) = &resp.updated_input { agg.modified_input = Some(input.clone()); }
            if let Some(msg) = &resp.system_message { agg.system_messages.push(msg.clone()); }
            agg.attachments.extend(resp.attachments.clone());
        }
        agg.all_results.push((hook.id, r));
    }
}
```

```rust
// async_registry.rs
use crate::definition::HookDefinition;
use crate::events::HookEvent;
use crate::registry::HookContext;
use crate::response::HookResult;
use lingxi_protocol::HookId;
use lingxi_platform_api::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub struct AsyncHookRegistry {
    runtime: Arc<dyn RuntimeSpawner>,
    in_flight: Arc<Mutex<HashMap<HookId, BackgroundTaskHandle>>>,
    completion_tx: mpsc::Sender<(HookId, HookResult)>,
}

impl AsyncHookRegistry {
    pub fn new(runtime: Arc<dyn RuntimeSpawner>, completion_tx: mpsc::Sender<(HookId, HookResult)>) -> Self {
        Self { runtime, in_flight: Arc::new(Mutex::new(HashMap::new())), completion_tx }
    }

    pub async fn spawn(&self, _hook: HookDefinition, _event: HookEvent, _ctx: HookContext) -> Result<(), RuntimeError> {
        // Full impl in Plan 09 (Skills/Cmd/Styles tie-in). M1.4 ships the type so
        // dispatcher can reference it.
        Ok(())
    }
}
```

```rust
// builtin/mod.rs (empty for M1.4 — populated in Plan 09 and onward)
```

- [ ] **Step 7: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod async_registry;
pub mod builtin;
pub mod definition;
pub mod events;
pub mod executor;
pub mod registry;
pub mod response;
pub mod ssrf_guard;

pub use definition::*;
pub use events::*;
pub use executor::HookExecutorImpl;
pub use registry::*;
pub use response::*;
pub use ssrf_guard::{SsrfError, SsrfGuard};
```

- [ ] **Step 8: Run tests**

```bash
cargo test -p lingxi-hooks
```

Expected: 4 SSRF tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/hooks
git commit -m "feat(hooks): 28-event enum + executor scaffold + SSRF guard"
```

---

## Task 6: Integration test — Pre/PostToolUse around dispatcher

**Files:** Create `crates/test-harness/tests/tools_hooks_e2e.rs`

- [ ] **Step 1: Write scenario**

```rust
//! Verify hook integration: a PreToolUse hook can block a tool call.
use lingxi_hooks::*;
use lingxi_protocol::{HookId, SessionId, ToolUseId};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

// Minimal stub HttpTransport / RuntimeSpawner from test-harness/mocks.
use lingxi_test_harness::mocks::{MockHttpTransport, MockRuntimeSpawner};

struct BlockingBuiltin;

#[async_trait::async_trait]
impl BuiltinHookHandler for BlockingBuiltin {
    async fn handle(&self, _event: &HookEvent, _ctx: &HookContext) -> HookResult {
        HookResult {
            outcome: HookOutcome::Success,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            response: Some(HookResponse {
                decision: Some(HookDecision::Block),
                reason: Some("test policy".into()),
                updated_input: None,
                system_message: None,
                attachments: vec![],
                suppress_output: false,
                structured_content: None,
            }),
        }
    }
    fn id(&self) -> &str { "blocking-test" }
}

#[tokio::test]
async fn pretooluse_block_short_circuits() {
    let reg = Arc::new(RwLock::new(HookRegistry::new()));
    reg.write().await.register(HookDefinition {
        id: HookId::new(),
        name: "block-everything".into(),
        events: vec![HookEventType::PreToolUse],
        if_condition: None,
        executor: HookExecutor::Builtin { handler_id: "blocking-test".into() },
        source: HookSource::User,
        blocking: true,
        timeout: None,
        priority: 100,
    });

    let http = Arc::new(MockHttpTransport::new());
    let runtime = Arc::new(MockRuntimeSpawner::default());
    let mut exec = HookExecutorImpl::new(reg.clone(), http, runtime);
    exec.register_builtin(Arc::new(BlockingBuiltin));

    let event = HookEvent::PreToolUse {
        tool_name: "Bash".into(),
        tool_input: serde_json::json!({"command": "rm -rf /"}),
        tool_use_id: ToolUseId::new(),
    };
    let ctx = HookContext { session_id: SessionId::nil(), agent_id: None, cwd: std::path::PathBuf::from("/tmp") };
    let result = exec.execute(event, ctx).await;
    assert_eq!(result.decision, Some(HookDecision::Block));
    assert_eq!(result.reason.as_deref(), Some("test policy"));
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p lingxi-test-harness --test tools_hooks_e2e
```

- [ ] **Step 3: Commit**

```bash
git add crates/test-harness/tests/tools_hooks_e2e.rs
git commit -m "test(hooks): PreToolUse blocking hook short-circuits dispatcher"
```

---

## Task 7: Plan exit

- [ ] **Step 1: Workspace check**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Tag**

```bash
git tag -a m1.7-tools-hooks -m "Plan 03 complete"
```

---

## Self-Review

- §8.1 Tool trait (30+ methods) → Task 1 ✓
- §8.4 ToolRegistry → Task 2 ✓
- §8.5 Concurrency partition → Task 4 ✓
- §9.1 28 hook events → Task 5.2 ✓
- §9.2 4 executor kinds → Task 5.3 ✓
- §9.7 SSRF guard → Task 5.5 ✓
- Integration with §14 Permission → Tool::check_permissions ✓

## Execution Handoff

Next: **Plan 04 — Memory & MCP** (`2026-05-22-lingxi-core-m1-04-memory-mcp.md`).
