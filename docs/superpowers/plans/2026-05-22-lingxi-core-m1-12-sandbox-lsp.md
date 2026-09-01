# LingXi Core M1 · Plan 12 · Sandbox + LSP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Two crates wired into the tool execution path: `lingxi-sandbox` (Sandbox trait + `SandboxedCommand` newtype that ProcessRunner only accepts via Sandbox::prepare — A1/A2/D2) and `lingxi-lsp` (LspTransport trait + LspRegistry + LspTool, per-connection state machine).

**Depends on:** Plans 01-11.

---

## File Structure

```
crates/sandbox/
├── Cargo.toml
└── src/{lib, policy, sandbox, decision, sandboxed_command}.rs

crates/lsp/
├── Cargo.toml
└── src/{lib, config, connection, registry, transport, tool, action}.rs

crates/platform-api/src/sandbox.rs ← NEW: Sandbox trait + SandboxedCommand newtype
crates/platform-api/src/lsp.rs     ← NEW: LspTransport trait + DTOs
crates/platform-api/src/process.rs ← MODIFY: ProcessRunner.run accepts only SandboxedCommand
```

---

## Task 1: SandboxedCommand newtype + Sandbox trait (A1)

**Files:** `crates/platform-api/src/{sandbox,process}.rs`

```rust
// sandbox.rs
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[async_trait]
pub trait Sandbox: Send + Sync {
    fn is_available(&self) -> bool;
    fn backend(&self) -> SandboxBackend;

    /// The ONLY way to construct a SandboxedCommand: through this method, which
    /// applies the policy and tags the command as sandboxed.
    fn prepare(&self, cmd: ProcessCommand, policy: &SandboxPolicy) -> Result<SandboxedCommand, SandboxError>;

    /// Audited bypass for cases that need it (e.g. user explicitly disabled
    /// sandbox). Records the reason so the audit log can later confirm intent.
    fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand;

    async fn probe_capability(&self) -> SandboxCapability;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxBackend {
    LinuxNamespaces, LinuxFirejail, MacOsSandboxExec, WindowsJobObject, None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub network: NetworkPolicy,
    pub writable_paths: Vec<PathBuf>,
    pub denied_paths: Vec<PathBuf>,
    pub allow_subprocess: bool,
    pub limits: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkPolicy { Disabled, LoopbackOnly, Allowed }

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_cpu_seconds: Option<u32>,
    pub max_memory_mb: Option<u32>,
    pub max_processes: Option<u32>,
    pub max_open_files: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SandboxCapability {
    pub available: bool,
    pub reason: Option<String>,
    pub features: SandboxFeatures,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxFeatures {
    pub network_isolation: bool,
    pub fs_readonly: bool,
    pub fs_readwrite_paths: bool,
    pub process_limit: bool,
    pub no_new_privileges: bool,
}

#[derive(Debug, Clone, Error)]
pub enum SandboxError {
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("path canonicalization failed: {0}")]
    PathCanonicalize(String),
    #[error("symlink escape detected for {0}")]
    SymlinkEscape(String),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCommand {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: std::collections::HashMap<String, String>,
    pub timeout: Option<std::time::Duration>,
    pub stdin: Option<String>,
}

/// Opaque newtype — only constructible via Sandbox::prepare or Sandbox::bypass_with_audit.
/// `ProcessRunner::run` accepts only this type, making it impossible to bypass the
/// sandbox decision (D2).
#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    pub(crate) inner: ProcessCommand,
    pub(crate) tag: SandboxedTag,
}

#[derive(Debug, Clone)]
pub enum SandboxedTag {
    Wrapped { backend: SandboxBackend },
    BypassAuditedWithReason { reason: String },
}

impl SandboxedCommand {
    /// Access the underlying ProcessCommand (for the runner to actually exec).
    pub fn inner(&self) -> &ProcessCommand { &self.inner }
    pub fn tag(&self) -> &SandboxedTag { &self.tag }
}
```

Modify `process.rs`:

```rust
use async_trait::async_trait;
use crate::sandbox::{SandboxedCommand, ProcessCommand};
use thiserror::Error;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// SandboxedCommand is the only accepted shape.
    async fn run(&self, cmd: &SandboxedCommand) -> Result<ProcessOutput, ProcessError>;
    async fn spawn_background(&self, cmd: &SandboxedCommand) -> Result<ProcessHandle, ProcessError>;
    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError>;
    fn is_available(&self) -> bool;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessHandle { pub task_id: String, pub pid: u32 }

#[derive(Debug, Clone, Error)]
pub enum ProcessError {
    #[error("unsupported on this platform")]
    Unsupported,
    #[error("io: {0}")]
    Io(String),
    #[error("timeout")]
    Timeout,
}
```

Re-export from lib.rs. Commit:
```bash
cargo check -p lingxi-traits
git add crates/traits
git commit -m "feat(traits): Sandbox trait + SandboxedCommand newtype (A1, D2)"
```

---

## Task 2: lingxi-sandbox — policy + decision + (constructor helper)

**Files:** `crates/sandbox/{Cargo.toml, src/{lib,policy,decision}.rs}`

```toml
[package]
name = "lingxi-sandbox"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-permission = { path = "../permission" }
serde.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

```rust
// policy.rs
pub use lingxi_platform_api::{NetworkPolicy, ResourceLimits, SandboxPolicy};

/// Conservative default: no network, project-writable only, subprocess allowed.
pub fn default_policy(workspace: std::path::PathBuf) -> SandboxPolicy {
    SandboxPolicy {
        network: NetworkPolicy::Disabled,
        writable_paths: vec![workspace],
        denied_paths: vec!["/etc".into(), "/var".into(), "/sys".into(), "/proc".into()],
        allow_subprocess: true,
        limits: ResourceLimits {
            max_cpu_seconds: Some(300),
            max_memory_mb: Some(2048),
            max_processes: Some(50),
            max_open_files: Some(1024),
        },
    }
}
```

```rust
// decision.rs
use lingxi_permission::PermissionMode;
use lingxi_platform_api::SandboxPolicy;

#[derive(Debug, Clone)]
pub enum SandboxDecision {
    NoSandbox,
    Sandbox { policy: SandboxPolicy },
    RefuseBecauseSandboxUnavailable { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub enum ProjectTrustLevel { Trusted, Untrusted }

pub fn should_use_sandbox(
    cmd: &str,
    mode: PermissionMode,
    trust: ProjectTrustLevel,
    classifier_safe: Option<bool>,
    sandbox_available: bool,
    workspace: std::path::PathBuf,
) -> SandboxDecision {
    if matches!(mode, PermissionMode::BypassPermissions) { return SandboxDecision::NoSandbox; }
    if matches!(mode, PermissionMode::Plan) { return SandboxDecision::NoSandbox; }
    let dangerous = is_obviously_dangerous(cmd);
    if dangerous && !sandbox_available {
        return SandboxDecision::RefuseBecauseSandboxUnavailable {
            reason: "command flagged dangerous and no sandbox backend available".into(),
        };
    }
    if matches!(trust, ProjectTrustLevel::Trusted) && classifier_safe == Some(true) && !dangerous {
        return SandboxDecision::NoSandbox;
    }
    if !sandbox_available {
        return SandboxDecision::NoSandbox;
    }
    SandboxDecision::Sandbox { policy: crate::policy::default_policy(workspace) }
}

fn is_obviously_dangerous(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    ["rm -rf /", "sudo ", "chmod 777", "curl", "fork bomb"].iter().any(|p| lower.contains(p))
}
```

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod decision;
pub mod policy;

pub use decision::{should_use_sandbox, ProjectTrustLevel, SandboxDecision};
pub use policy::default_policy;
pub use lingxi_platform_api::{
    NetworkPolicy, ResourceLimits, Sandbox, SandboxBackend, SandboxError, SandboxPolicy, SandboxedCommand, SandboxedTag,
};
```

```rust
// canonicalize check helper — used by platform impls (A2)
pub fn canonicalize_safely(path: &std::path::Path, workspace: &std::path::Path) -> Result<std::path::PathBuf, lingxi_platform_api::SandboxError> {
    let canon = path.canonicalize().map_err(|e| lingxi_platform_api::SandboxError::PathCanonicalize(e.to_string()))?;
    if !canon.starts_with(workspace) {
        return Err(lingxi_platform_api::SandboxError::SymlinkEscape(path.display().to_string()));
    }
    Ok(canon)
}
```

Commit:
```bash
cargo test -p lingxi-sandbox
git add crates/sandbox
git commit -m "feat(sandbox): policy + should_use_sandbox decision + symlink escape check"
```

---

## Task 3: LspTransport trait + DTOs

**Files:** `crates/platform-api/src/lsp.rs`

```rust
use async_trait::async_trait;
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub trigger_languages: Vec<String>,
    pub root_dir_markers: Vec<String>,
    pub initialization_options: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspRawConnection { pub connection_id: lingxi_protocol::McpConnectionId }
// Reuse McpConnectionId type to avoid yet another id newtype; documents that
// the id namespace is shared for connection-level routing.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerCapabilities {
    pub text_document_sync: Option<String>,
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub diagnostics: bool,
    pub symbols: bool,
    pub formatting: bool,
    pub rename: bool,
    pub code_action: bool,
}

#[async_trait]
pub trait LspTransport: Send + Sync {
    async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError>;
    async fn initialize(&self, conn: &LspRawConnection, root_uri: &str) -> Result<LspServerCapabilities, LspError>;
    async fn request(&self, conn: &LspRawConnection, method: &str, params: Value) -> Result<Value, LspError>;
    async fn notify(&self, conn: &LspRawConnection, method: &str, params: Value) -> Result<(), LspError>;
    async fn shutdown(&self, conn_id: lingxi_protocol::McpConnectionId) -> Result<(), LspError>;
    fn is_available(&self) -> bool;
}

#[derive(Debug, Clone, Error)]
pub enum LspError {
    #[error("unavailable")]
    Unavailable,
    #[error("transport error: {0}")]
    Transport(String),
    #[error("server response: {0}")]
    ServerError(String),
}
```

Re-export from `traits/lib.rs`.

---

## Task 4: lingxi-lsp — registry + LspTool

**Files:** `crates/lsp/{Cargo.toml, src/*.rs}`

```toml
[package]
name = "lingxi-lsp"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-tools = { path = "../tools" }
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

```rust
// connection.rs
use lingxi_protocol::McpConnectionId;
use lingxi_platform_api::{LspServerCapabilities, LspServerConfig};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum LspConnectionState {
    Disconnected { config: LspServerConfig },
    Starting { config: LspServerConfig, started_at: SystemTime, pid: u32 },
    Initialized {
        config: LspServerConfig,
        connection_id: McpConnectionId,
        server_capabilities: LspServerCapabilities,
        pid: u32,
    },
    Failed { config: LspServerConfig, error: String },
    Stopped { config: LspServerConfig },
}
```

```rust
// action.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LspAction {
    Hover { line: u32, character: u32 },
    Definition { line: u32, character: u32 },
    References { line: u32, character: u32 },
    Diagnostics,
    Symbols { query: Option<String> },
    Completion { line: u32, character: u32 },
    Formatting,
    Rename { line: u32, character: u32, new_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspResponse(pub serde_json::Value);
```

```rust
// registry.rs
use crate::connection::LspConnectionState;
use lingxi_protocol::{McpConnectionId, PluginId};
use lingxi_platform_api::{LspError, LspServerConfig, LspTransport};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct LspRegistry {
    servers: RwLock<HashMap<String, LspConnectionState>>,
    file_route_cache: RwLock<HashMap<PathBuf, String>>,
    transport: Arc<dyn LspTransport>,
    plugin_servers: RwLock<HashMap<PluginId, Vec<String>>>,
}

impl LspRegistry {
    pub fn new(transport: Arc<dyn LspTransport>) -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
            file_route_cache: RwLock::new(HashMap::new()),
            transport,
            plugin_servers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register_config(&self, config: LspServerConfig) {
        self.servers.write().await.insert(config.name.clone(), LspConnectionState::Disconnected { config });
    }

    pub async fn ensure_server_for_file(&self, _path: &std::path::Path) -> Result<McpConnectionId, LspError> {
        Err(LspError::Unavailable) // Plan 16 wires production posix-minimal LspTransport.
    }

    pub async fn register_plugin_servers(&self, plugin_id: PluginId, configs: Vec<LspServerConfig>) {
        let names: Vec<String> = configs.iter().map(|c| c.name.clone()).collect();
        for c in configs { self.servers.write().await.insert(c.name.clone(), LspConnectionState::Disconnected { config: c }); }
        self.plugin_servers.write().await.insert(plugin_id, names);
    }

    pub async fn unregister_plugin(&self, plugin_id: &PluginId) -> Vec<String> {
        self.plugin_servers.write().await.remove(plugin_id).unwrap_or_default()
    }
}
```

```rust
// tool.rs (LspTool exposing dispatch())
use crate::action::{LspAction, LspResponse};
use crate::registry::LspRegistry;
use async_trait::async_trait;
use lingxi_permission::{PermissionDecisionReason, PermissionMetadata, PermissionResult};
use lingxi_tools::{DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext, ToolUseContext};
use serde_json::Value;
use std::sync::Arc;

pub struct LspTool {
    registry: Arc<LspRegistry>,
}

impl LspTool {
    pub fn new(registry: Arc<LspRegistry>) -> Self { Self { registry } }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str { "LSP" }
    fn input_schema(&self) -> &Value {
        static S: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| serde_json::json!({"type":"object"}));
        &S
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool { true }
    fn max_result_size_chars(&self) -> usize { 65_536 }
    fn is_concurrency_safe(&self, _: &Value) -> bool { true }
    fn is_read_only(&self, _: &Value) -> bool { true }
    fn is_lsp(&self) -> bool { true }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other { reason: "lsp tool".into() },
            updated_input: None, update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }
    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String { "Invoke LSP".into() }
    async fn prompt(&self, _: &PromptOptions) -> String { "".into() }
    async fn call(&self, input: Value, _ctx: ToolUseContext, _: lingxi_tools::ToolProgressSender)
        -> Result<ToolCallResult, ToolError>
    {
        let _action: LspAction = serde_json::from_value(input.clone())
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        // M1.18 ships the contract; production impl in Plan 16 with posix-minimal.
        Ok(ToolCallResult {
            data: serde_json::json!({"stub": true}),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}
```

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod action;
pub mod connection;
pub mod registry;
pub mod tool;

pub use action::{LspAction, LspResponse};
pub use connection::LspConnectionState;
pub use registry::LspRegistry;
pub use tool::LspTool;
```

Add `once_cell = "1"` to deps.

Commit:
```bash
cargo test -p lingxi-lsp
git add crates/lsp
git commit -m "feat(lsp): registry + LspTool + action enum"
```

---

## Task 5: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.18-sandbox-lsp -m "Plan 12 complete"
```

## Self-Review

- §24.1 Sandbox trait + SandboxedCommand (A1) → ✓
- §24.2 SandboxPolicy → ✓
- §24.3 should_use_sandbox decision → ✓
- §24 symlink escape check (A2) → canonicalize_safely ✓
- §25.1 LspServerConfig → ✓
- §25.2 LspConnectionState → ✓
- §25.3 LspTransport trait + LspRegistry → ✓
- §25.4 LspTool → ✓

## Execution Handoff

Next: **Plan 13 — Telemetry + Anthropic OAuth** (`2026-05-22-lingxi-core-m1-13-telemetry-oauth.md`).
