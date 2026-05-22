# LingXi Core M1 · Plan 04 · Memory & MCP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Add the 4-tier Memory system (project / user / session / team) with LLM-driven selection and prefetch, plus the MCP Lifecycle covering all 7 transport variants, per-connection state machines, OAuth handshake, and agent-scoped server pools.

**Architecture:** `lingxi-memory` runs a side query (via Plan 08 SideQueryClient, stubbed here) to pick relevant memory files in parallel with the main API call. `lingxi-mcp` manages connections with a state machine per server, supporting stdio (desktop only), SSE, HTTP, WebSocket, InProcess, SseIde, SdkControl.

**Tech Stack:** `serde_yaml` (frontmatter parse), `regex`, `url`, `async-trait`.

**References:** Spec §6 Memory · §7 MCP

**Depends on:** Plans 01-03.

---

## File Structure

```
crates/memory/
├── Cargo.toml
└── src/{lib, tier, file, selector, prefetch, session_memory, team_memory, snapshot}.rs

crates/mcp/
├── Cargo.toml
└── src/{lib, registry, connection, capabilities, oauth, approval, transport_spec, agent_scope}.rs

crates/traits/src/mcp.rs       ← NEW: McpTransport trait
crates/traits/src/filesystem.rs ← MODIFY: add watch method
```

---

## Task 1: McpTransport trait (in lingxi-traits)

**Files:** Create `crates/traits/src/mcp.rs`, modify `lib.rs`.

- [ ] **Step 1: mcp.rs**

```rust
use async_trait::async_trait;
use futures_core::stream::Stream;
use lingxi_protocol::McpConnectionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransportSpec {
    Stdio { command: String, args: Vec<String>, env: std::collections::HashMap<String, String> },
    Sse { url: String, headers: std::collections::HashMap<String, String>, headers_helper: Option<String>, oauth: Option<McpOAuthConfigDto> },
    Http { url: String, headers: std::collections::HashMap<String, String>, oauth: Option<McpOAuthConfigDto> },
    WebSocket { url: String, headers: std::collections::HashMap<String, String> },
    InProcess { registry_key: String },
    SseIde { url: String, ide_name: String, ide_running_in_windows: bool },
    SdkControl { control_channel_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransportKind {
    Stdio, Sse, Http, WebSocket, InProcess, SseIde, SdkControl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpOAuthConfigDto {
    pub client_id: Option<String>,
    pub callback_port: Option<u16>,
    pub auth_server_metadata_url: Option<String>,
    pub xaa: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRawConnection {
    pub connection_id: McpConnectionId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilitiesDto {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub logging: bool,
    pub experimental: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDto { pub server_name: String, pub tool_name: String, pub description: String, pub input_schema: Value, pub full_name: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDto { pub uri: String, pub name: String, pub mime_type: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptDto { pub name: String, pub description: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResultDto { pub content: Value, pub is_error: bool }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotificationDto { pub method: String, pub params: Value }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestDto { pub params: Value }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitResultDto { pub data: Value }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContentDto { pub uri: String, pub content: String }

pub type McpNotificationStream = Pin<Box<dyn Stream<Item = McpNotificationDto> + Send>>;

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError>;
    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError>;
    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError>;
    async fn list_resources(&self, conn: &McpRawConnection) -> Result<Vec<McpResourceDto>, McpError>;
    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError>;
    async fn call_tool(&self, conn: &McpRawConnection, tool: &str, input: Value) -> Result<McpToolResultDto, McpError>;
    async fn read_resource(&self, conn: &McpRawConnection, uri: &str) -> Result<McpResourceContentDto, McpError>;
    async fn ping(&self, conn_id: McpConnectionId) -> Result<(), McpError>;
    async fn notifications(&self, conn: &McpRawConnection) -> Result<McpNotificationStream, McpError>;
    async fn handle_elicitation(&self, conn: &McpRawConnection, req: ElicitRequestDto) -> Result<ElicitResultDto, McpError>;
    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError>;
    fn supported_transports(&self) -> Vec<McpTransportKind>;
}

#[derive(Debug, Clone, Error)]
pub enum McpError {
    #[error("transport {0:?} not supported on this platform")]
    UnsupportedTransport(McpTransportKind),
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("oauth flow failed: {0}")]
    OAuth(String),
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}
```

- [ ] **Step 2: Extend FileSystem with `watch`**

In `crates/traits/src/filesystem.rs`, add to the trait:

```rust
#[async_trait]
pub trait FileSystem: Send + Sync {
    // ... existing ...
    async fn watch(&self, dir: &str) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError>;
}

#[derive(Debug, Clone)]
pub struct FileEvent {
    pub path: std::path::PathBuf,
    pub kind: FileEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventKind { Created, Modified, Deleted }
```

- [ ] **Step 3: Update lib.rs**

```rust
pub mod mcp;
pub use mcp::*;
```

- [ ] **Step 4: Commit**

```bash
git add crates/traits
git commit -m "feat(traits): McpTransport trait + FileSystem.watch"
```

---

## Task 2: lingxi-memory — Tier model + frontmatter parsing

**Files:** Create `crates/memory/{Cargo.toml, src/{lib,tier,file}.rs}`.

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-memory"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-api-client = { path = "../api-client" }
serde.workspace = true
serde_json.workspace = true
serde_yaml = "0.9"
thiserror.workspace = true
regex = "1"
tracing.workspace = true
tokio = { version = "1", features = ["sync"] }

[lints]
workspace = true
```

- [ ] **Step 2: tier.rs**

```rust
use lingxi_protocol::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryTier {
    Project { repo_root: PathBuf },
    User { user_memory_dir: PathBuf },
    Session { session_id: SessionId },
    Team { team_dir: PathBuf, watcher_enabled: bool },
}
```

- [ ] **Step 3: file.rs (frontmatter parsing)**

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub frontmatter: MemoryFrontmatter,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MemoryFrontmatter {
    pub memory_type: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub tags: Vec<String>,
    pub related_tools: Vec<String>,
}

#[derive(Debug, Clone, Error)]
pub enum MemoryError {
    #[error("parse failed: {0}")]
    Parse(String),
    #[error("io failed: {0}")]
    Io(String),
    #[error("selector unavailable: {0}")]
    SelectorUnavailable(String),
}

pub const MAX_ENTRYPOINT_LINES: usize = 200;
pub const MAX_ENTRYPOINT_BYTES: usize = 25_000;

/// Parse a `---\n<yaml>\n---\n<body>` markdown file.
pub fn parse_markdown_with_frontmatter(input: &str) -> Result<(MemoryFrontmatter, String), MemoryError> {
    if !input.starts_with("---") {
        return Ok((MemoryFrontmatter::default(), input.to_string()));
    }
    let rest = &input[3..];
    let end = rest.find("\n---\n").ok_or(MemoryError::Parse("unterminated frontmatter".into()))?;
    let yaml = &rest[..end];
    let body = &rest[end + 5..];
    let fm: MemoryFrontmatter = serde_yaml::from_str(yaml).map_err(|e| MemoryError::Parse(e.to_string()))?;
    Ok((fm, body.trim_start().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_frontmatter() {
        let raw = "---\nmemory_type: tool_usage\ndescription: bash tips\n---\nuse fd not find\n";
        let (fm, body) = parse_markdown_with_frontmatter(raw).unwrap();
        assert_eq!(fm.memory_type, "tool_usage");
        assert_eq!(fm.description, "bash tips");
        assert!(body.contains("fd"));
    }

    #[test]
    fn no_frontmatter_returns_default() {
        let raw = "just markdown\n";
        let (fm, body) = parse_markdown_with_frontmatter(raw).unwrap();
        assert!(fm.description.is_empty());
        assert_eq!(body, raw);
    }
}
```

- [ ] **Step 4: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod file;
pub mod prefetch;
pub mod selector;
pub mod session_memory;
pub mod snapshot;
pub mod team_memory;
pub mod tier;

pub use file::{MemoryError, MemoryFile, MemoryFrontmatter, parse_markdown_with_frontmatter};
pub use tier::MemoryTier;
```

- [ ] **Step 5: Stub remaining modules**

```rust
// selector.rs
use crate::file::{MemoryError, MemoryFile};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct MemorySelector {
    pub selector_model: String,
    pub max_selected: usize,
}

impl MemorySelector {
    pub fn new() -> Self {
        Self { selector_model: "claude-haiku-4-5".into(), max_selected: 5 }
    }

    /// Sketch — in Plan 08 this delegates to SideQueryClient.
    pub async fn select_relevant(
        &self,
        _query: &str,
        available: &[MemoryFile],
        _recent_tools: &[String],
        already: &HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, MemoryError> {
        // Stub: pick any not already surfaced, up to max_selected.
        Ok(available.iter()
            .filter(|m| !already.contains(&m.path))
            .take(self.max_selected)
            .map(|m| m.path.clone())
            .collect())
    }
}
```

```rust
// prefetch.rs
use crate::selector::MemorySelector;
use lingxi_traits::RuntimeSpawner;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct MemoryPrefetch {
    selector: Arc<MemorySelector>,
    runtime: Arc<dyn RuntimeSpawner>,
}

pub struct PendingMemoryPrefetch {
    rx: tokio::sync::Mutex<Option<oneshot::Receiver<Vec<PathBuf>>>>,
}

impl MemoryPrefetch {
    pub fn new(selector: Arc<MemorySelector>, runtime: Arc<dyn RuntimeSpawner>) -> Self {
        Self { selector, runtime }
    }

    pub async fn start(&self, _query: String, _memory_dir: PathBuf) -> PendingMemoryPrefetch {
        let (tx, rx) = oneshot::channel();
        let _ = self.runtime.spawn(
            "memory-prefetch",
            Box::pin(async move {
                // Plan 08 will wire to actual selector. M1.5 ships the channel plumbing.
                let _ = tx.send(Vec::<PathBuf>::new());
            }),
        ).await;
        PendingMemoryPrefetch { rx: tokio::sync::Mutex::new(Some(rx)) }
    }
}
```

```rust
// session_memory.rs
pub struct SessionMemoryExtractor { /* impl in Plan 10 */ }

// team_memory.rs
pub struct TeamMemoryWatcher { /* impl in Plan 10 */ }
pub struct SecretScannerStub; // delegates to lingxi-secret in Plan 10

// snapshot.rs
use lingxi_protocol::SnapshotId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemorySnapshot {
    pub snapshot_id: SnapshotId,
    pub agent_type: String,
    pub included_paths: Vec<PathBuf>,
    pub created_at: SystemTime,
}
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p lingxi-memory --lib
```

Expected: 2 tests pass (frontmatter).

- [ ] **Step 7: Commit**

```bash
git add crates/memory
git commit -m "feat(memory): tier model, frontmatter parser, selector + prefetch scaffold"
```

---

## Task 3: lingxi-mcp — Connection state machine + Registry

**Files:** Create `crates/mcp/{Cargo.toml, src/{lib,registry,connection,capabilities,oauth,approval,agent_scope}.rs}`.

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-mcp"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: connection.rs**

```rust
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{McpTransportSpec, McpToolDto, McpResourceDto, McpPromptDto, ServerCapabilitiesDto};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub spec: McpTransportSpec,
    pub scope: ConfigScope,
    pub disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfigScope {
    Local, User, Project, Dynamic, Enterprise, ClaudeAi, Managed,
}

#[derive(Debug, Clone)]
pub enum McpConnectionState {
    Disconnected { config: McpServerConfig, last_error: Option<String> },
    Connecting { config: McpServerConfig, started_at: SystemTime },
    AwaitingOAuth { config: McpServerConfig, callback_port: u16 },
    Connected {
        config: McpServerConfig,
        connection_id: McpConnectionId,
        capabilities: ServerCapabilitiesDto,
        tools: Vec<McpToolDto>,
        resources: Vec<McpResourceDto>,
        prompts: Vec<McpPromptDto>,
        connected_at: SystemTime,
    },
    HealthChecking { connection_id: McpConnectionId, config: McpServerConfig },
    Reconnecting { config: McpServerConfig, retry_count: u32, next_retry_at: SystemTime },
    Failed { config: McpServerConfig, error: String, attempts: u32 },
    Stopped { config: McpServerConfig },
}

impl McpConnectionState {
    pub fn name(&self) -> &str {
        match self {
            Self::Disconnected { config, .. }
            | Self::Connecting { config, .. }
            | Self::AwaitingOAuth { config, .. }
            | Self::Connected { config, .. }
            | Self::HealthChecking { config, .. }
            | Self::Reconnecting { config, .. }
            | Self::Failed { config, .. }
            | Self::Stopped { config } => &config.name,
        }
    }
}
```

- [ ] **Step 3: registry.rs**

```rust
use crate::connection::*;
use lingxi_protocol::{AgentId, McpConnectionId};
use lingxi_traits::{McpError, McpTransport};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

pub struct McpRegistry {
    connections: RwLock<HashMap<String, McpConnectionState>>,
    agent_scoped: RwLock<HashMap<AgentId, HashMap<String, McpConnectionId>>>,
    transport: Arc<dyn McpTransport>,
    pub health_check_interval: Duration,
    pub max_retry_count: u32,
}

impl McpRegistry {
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
            agent_scoped: RwLock::new(HashMap::new()),
            transport,
            health_check_interval: Duration::from_secs(30),
            max_retry_count: 5,
        }
    }

    pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError> {
        // Reuse if already Connected.
        if let Some(McpConnectionState::Connected { connection_id, .. }) =
            self.connections.read().await.get(&config.name)
        {
            return Ok(*connection_id);
        }

        // Disconnected → Connecting.
        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::Connecting { config: config.clone(), started_at: SystemTime::now() },
        );

        let conn = self.transport.connect(&config.spec).await?;
        let caps = self.transport.initialize(&conn).await?;
        let tools = self.transport.list_tools(&conn).await?;
        let resources = self.transport.list_resources(&conn).await?;
        let prompts = self.transport.list_prompts(&conn).await?;

        let connection_id = conn.connection_id;
        self.connections.write().await.insert(
            config.name.clone(),
            McpConnectionState::Connected {
                config,
                connection_id,
                capabilities: caps,
                tools,
                resources,
                prompts,
                connected_at: SystemTime::now(),
            },
        );
        Ok(connection_id)
    }

    pub async fn disconnect(&self, name: &str) -> Result<(), McpError> {
        let mut conns = self.connections.write().await;
        if let Some(state) = conns.remove(name) {
            if let McpConnectionState::Connected { connection_id, config, .. } = state {
                self.transport.disconnect(connection_id).await?;
                conns.insert(name.into(), McpConnectionState::Stopped { config });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Full mock McpTransport test in test-harness/tests/mcp_lifecycle.rs.
}
```

- [ ] **Step 4: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod agent_scope;
pub mod approval;
pub mod capabilities;
pub mod connection;
pub mod oauth;
pub mod registry;

pub use connection::*;
pub use registry::McpRegistry;
```

- [ ] **Step 5: Stub `oauth.rs`, `approval.rs`, `capabilities.rs`, `agent_scope.rs` (full impl deferred)**

```rust
// oauth.rs
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum OAuthState {
    Initiated { callback_port: u16, code_verifier: String, state_token: String },
    AwaitingCallback { callback_port: u16, code_verifier: String, state_token: String, auth_url: String },
    ExchangingCode { code: String },
    Authenticated { access_token: lingxi_protocol::Secret<String>, refresh_token: Option<lingxi_protocol::Secret<String>>, expires_at: SystemTime },
    Refreshing { refresh_token: lingxi_protocol::Secret<String> },
}
```

```rust
// approval.rs
use crate::connection::ConfigScope;
use std::collections::HashSet;

pub struct McpApprovalPolicy {
    pub project_servers_require_approval: bool,
    pub approved: HashSet<String>,
    pub rejected: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus { Approved, Rejected, PendingApproval }

impl McpApprovalPolicy {
    pub fn is_approved(&self, name: &str, scope: ConfigScope) -> ApprovalStatus {
        if self.rejected.contains(name) { return ApprovalStatus::Rejected; }
        if self.approved.contains(name) { return ApprovalStatus::Approved; }
        match scope {
            ConfigScope::Local | ConfigScope::User | ConfigScope::Enterprise | ConfigScope::Managed => ApprovalStatus::Approved,
            ConfigScope::Project => ApprovalStatus::PendingApproval,
            _ => ApprovalStatus::PendingApproval,
        }
    }
}
```

```rust
// capabilities.rs — re-export trait DTOs for ergonomics
pub use lingxi_traits::{McpToolDto, McpResourceDto, McpPromptDto, ServerCapabilitiesDto};
```

```rust
// agent_scope.rs
use lingxi_protocol::{AgentId, McpConnectionId};
use std::collections::HashMap;

pub struct AgentScopedConnections {
    inner: HashMap<AgentId, HashMap<String, McpConnectionId>>,
}

impl AgentScopedConnections {
    pub fn new() -> Self { Self { inner: HashMap::new() } }
    pub fn register(&mut self, agent: AgentId, server: String, conn: McpConnectionId) {
        self.inner.entry(agent).or_default().insert(server, conn);
    }
    pub fn cleanup(&mut self, agent: &AgentId) -> Vec<McpConnectionId> {
        self.inner.remove(agent).map(|m| m.into_values().collect()).unwrap_or_default()
    }
}
```

- [ ] **Step 6: Compile**

```bash
cargo check -p lingxi-mcp
```

- [ ] **Step 7: Commit**

```bash
git add crates/mcp
git commit -m "feat(mcp): connection state machine + registry + oauth/approval stubs"
```

---

## Task 4: Integration test — Mock MCP server lifecycle

**Files:** `crates/test-harness/src/mocks/mock_mcp.rs`, `crates/test-harness/tests/mcp_lifecycle.rs`

- [ ] **Step 1: mock_mcp.rs**

```rust
use async_trait::async_trait;
use lingxi_protocol::McpConnectionId;
use lingxi_traits::*;
use serde_json::Value;
use std::sync::Mutex;

pub struct MockMcpTransport {
    tools: Mutex<Vec<McpToolDto>>,
}

impl MockMcpTransport {
    pub fn new() -> Self { Self { tools: Mutex::new(Vec::new()) } }

    pub fn add_tool(&self, name: &str) {
        self.tools.lock().unwrap().push(McpToolDto {
            server_name: "mock".into(),
            tool_name: name.into(),
            description: format!("{name} test tool"),
            input_schema: serde_json::json!({"type": "object"}),
            full_name: format!("mcp__mock__{name}"),
        });
    }
}

#[async_trait]
impl McpTransport for MockMcpTransport {
    async fn connect(&self, _spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        Ok(McpRawConnection { connection_id: McpConnectionId::new() })
    }
    async fn initialize(&self, _conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        Ok(ServerCapabilitiesDto {
            tools: true, resources: false, prompts: false, logging: false,
            experimental: Default::default(),
        })
    }
    async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Ok(self.tools.lock().unwrap().clone())
    }
    async fn list_resources(&self, _conn: &McpRawConnection) -> Result<Vec<McpResourceDto>, McpError> { Ok(Vec::new()) }
    async fn list_prompts(&self, _conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> { Ok(Vec::new()) }
    async fn call_tool(&self, _conn: &McpRawConnection, _tool: &str, _input: Value) -> Result<McpToolResultDto, McpError> {
        Ok(McpToolResultDto { content: serde_json::json!("ok"), is_error: false })
    }
    async fn read_resource(&self, _conn: &McpRawConnection, _uri: &str) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal("not implemented".into()))
    }
    async fn ping(&self, _conn_id: McpConnectionId) -> Result<(), McpError> { Ok(()) }
    async fn notifications(&self, _conn: &McpRawConnection) -> Result<McpNotificationStream, McpError> {
        use futures::stream::empty;
        Ok(Box::pin(empty()))
    }
    async fn handle_elicitation(&self, _c: &McpRawConnection, _r: ElicitRequestDto) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal("not implemented".into()))
    }
    async fn disconnect(&self, _conn_id: McpConnectionId) -> Result<(), McpError> { Ok(()) }
    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![McpTransportKind::Stdio, McpTransportKind::InProcess]
    }
}
```

- [ ] **Step 2: tests/mcp_lifecycle.rs**

```rust
use lingxi_mcp::{McpRegistry, McpServerConfig, ConfigScope};
use lingxi_test_harness::mocks::MockMcpTransport;
use lingxi_traits::McpTransportSpec;
use std::sync::Arc;

#[tokio::test]
async fn connect_initializes_and_lists_tools() {
    let transport = Arc::new(MockMcpTransport::new());
    transport.add_tool("hello");
    let registry = McpRegistry::new(transport.clone());

    let config = McpServerConfig {
        name: "mock".into(),
        spec: McpTransportSpec::InProcess { registry_key: "mock".into() },
        scope: ConfigScope::User,
        disabled: false,
    };
    let conn_id = registry.connect(config).await.unwrap();
    assert!(!conn_id.as_uuid().is_nil());
}
```

- [ ] **Step 3: Add `futures` to test-harness deps**

In `crates/test-harness/Cargo.toml`:

```toml
futures = "0.3"
```

- [ ] **Step 4: Add MockMcpTransport export**

`crates/test-harness/src/mocks/mod.rs`:

```rust
pub mod mock_mcp;
pub use mock_mcp::MockMcpTransport;
```

- [ ] **Step 5: Run**

```bash
cargo test -p lingxi-test-harness --test mcp_lifecycle
```

- [ ] **Step 6: Commit**

```bash
git add crates/test-harness
git commit -m "test(mcp): MockMcpTransport + connect→init→list_tools lifecycle"
```

---

## Task 5: Plan exit

- [ ] **Step 1: Workspace check**

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Tag**

```bash
git tag -a m1.9-memory-mcp -m "Plan 04 complete: memory + mcp"
```

---

## Self-Review

- §6 Memory 4-tier model → Task 2.2 ✓
- §6.2 frontmatter parse → Task 2.3 ✓
- §6.3 LLM selector (stubbed, full in Plan 08) → Task 2.5 ✓
- §7.1 7 transport variants → Task 1 ✓
- §7.2 per-connection state machine → Task 3.2 ✓
- §7.4 McpRegistry connect with handshake → Task 3.3 ✓
- §7.6 approval policy → Task 3.5 ✓

## Execution Handoff

Next: **Plan 05 — Compaction Engine** (`2026-05-22-lingxi-core-m1-05-compaction.md`).
