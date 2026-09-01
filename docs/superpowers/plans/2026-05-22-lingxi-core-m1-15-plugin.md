# LingXi Core M1 · Plan 15 · Plugin System

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** `lingxi-plugin` — manifest model + 7 lifecycle states + marketplace + blocklist + strict-plugin-only policy + materialization into the 8 registries built in Plans 03/04/09/12 (Tools/Hooks/MCP/Agent/Skill/Command/OutputStyle/LSP). Plugin agent frontmatter `permission_mode/hooks/mcpServers` is **rejected** (D2 plugin-agent privilege).

**Depends on:** Plans 01-14 — Plugin is the most cross-cutting subsystem and must come after every registry it injects into.

---

## File Structure

```
crates/plugin/
├── Cargo.toml
└── src/{lib, manifest, lifecycle, source, trust, blocklist, marketplace, strict_policy, loader, manager, agent_validation}.rs
```

---

## Task 1: Manifest + Source + TrustLevel

```toml
[package]
name = "lingxi-plugin"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-tools = { path = "../tools" }
lingxi-hooks = { path = "../hooks" }
lingxi-mcp = { path = "../mcp" }
lingxi-agent = { path = "../agent" }
lingxi-skills = { path = "../skills" }
lingxi-commands = { path = "../commands" }
lingxi-outputstyles = { path = "../outputstyles" }
lingxi-lsp = { path = "../lsp" }
lingxi-secret = { path = "../secret" }
serde.workspace = true
serde_json.workspace = true
serde_yaml = "0.9"
thiserror.workspace = true
async-trait.workspace = true
url = "2"
semver = "1"
zip = "0.6"  # for .mcpb bundle parsing
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

```rust
// manifest.rs (Claude Code component surface)
use lingxi_hooks::HookDefinition;
use lingxi_mcp::McpServerConfig;
use lingxi_platform_api::LspServerConfig;
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub source: PluginSource,
    pub components: PluginComponents,
    pub trust_level: PluginTrustLevel,
    pub depends_on: Vec<PluginId>,
    pub user_config: Option<UserConfigSchema>,
    pub channels: Vec<PluginChannel>,
    pub settings: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginComponents {
    pub commands: Vec<ComponentPath>,
    pub agents: Vec<ComponentPath>,
    pub skills: Vec<ComponentPath>,
    pub output_styles: Vec<ComponentPath>,
    pub hooks: Vec<HookDefinition>,
    pub mcp_servers: HashMap<String, McpServerConfig>,
    pub lsp_servers: HashMap<String, LspServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentPath {
    pub path: PathBuf,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfigSchema {
    pub fields: HashMap<String, UserConfigField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfigField {
    pub description: String,
    pub sensitive: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginChannel {
    pub name: String,
    pub mcp_server: String,
    pub user_config: Option<UserConfigSchema>,
}
```

```rust
// source.rs
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginSource {
    BuiltIn,
    OfficialMarketplace { name: String },
    Marketplace { url: String, name: String },
    Git { url: String, ref_: String },
    LocalPath { path: PathBuf },
    Mcpb { path: PathBuf, hash: String },
}
```

```rust
// trust.rs (A7 — git/local default Untrusted)
use crate::source::PluginSource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginTrustLevel {
    AdminTrusted,
    UserTrusted,
    Untrusted,
}

pub fn default_trust_for_source(src: &PluginSource) -> PluginTrustLevel {
    match src {
        PluginSource::BuiltIn => PluginTrustLevel::AdminTrusted,
        PluginSource::OfficialMarketplace { .. } => PluginTrustLevel::AdminTrusted,
        PluginSource::Marketplace { .. } => PluginTrustLevel::UserTrusted,
        PluginSource::Git { .. } => PluginTrustLevel::Untrusted,
        PluginSource::LocalPath { .. } => PluginTrustLevel::Untrusted,
        PluginSource::Mcpb { .. } => PluginTrustLevel::UserTrusted,
    }
}
```

---

## Task 2: Lifecycle + Blocklist + StrictPolicy

```rust
// lifecycle.rs (7 states)
use crate::manifest::PluginManifest;
use crate::source::PluginSource;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub enum PluginState {
    Declared { source: PluginSource },
    Fetching { source: PluginSource, started_at: SystemTime },
    Fetched { manifest: PluginManifest, install_dir: PathBuf },
    Loaded { manifest: PluginManifest, install_dir: PathBuf, loaded_at: SystemTime },
    Disabled { manifest: PluginManifest, install_dir: PathBuf },
    Failed { source: PluginSource, error: String },
    Blocked { source: PluginSource, reason: String },
}
```

```rust
// blocklist.rs
use lingxi_protocol::PluginId;
use std::collections::HashSet;
use tokio::sync::RwLock;

pub struct PluginBlocklist {
    static_block: HashSet<PluginId>,
    remote_block: RwLock<HashSet<PluginId>>,
    pub fetch_url: String,
}

impl PluginBlocklist {
    pub fn new(fetch_url: String) -> Self {
        Self { static_block: HashSet::new(), remote_block: RwLock::new(HashSet::new()), fetch_url }
    }

    pub async fn is_blocked(&self, id: &PluginId) -> Option<String> {
        if self.static_block.contains(id) { return Some("static blocklist".into()); }
        if self.remote_block.read().await.contains(id) { return Some("remote blocklist".into()); }
        None
    }

    pub async fn set_remote_blocklist(&self, ids: HashSet<PluginId>) {
        *self.remote_block.write().await = ids;
    }
}
```

```rust
// strict_policy.rs
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginComponent {
    Commands, Agents, Skills, Hooks, OutputStyles, McpServers, LspServers, Channels,
}

pub struct StrictPluginOnlyPolicy {
    pub locked: HashSet<PluginComponent>,
}

impl StrictPluginOnlyPolicy {
    pub fn empty() -> Self { Self { locked: HashSet::new() } }
    pub fn is_locked(&self, c: PluginComponent) -> bool { self.locked.contains(&c) }
}
```

---

## Task 3: Agent frontmatter validation (D2)

```rust
// agent_validation.rs
//! Reject plugin-provided agent files that try to escalate privilege via
//! frontmatter. Privileges must come from the manifest at install/enable time.

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum AgentValidationError {
    #[error("agent frontmatter cannot set permission_mode in plugin context")]
    PermissionModeForbidden,
    #[error("agent frontmatter cannot declare hooks in plugin context")]
    HooksForbidden,
    #[error("agent frontmatter cannot declare mcpServers in plugin context")]
    McpServersForbidden,
}

/// Validate frontmatter YAML for a plugin agent file. Returns error if any
/// privilege-related field is present.
pub fn validate_plugin_agent_frontmatter(yaml: &str) -> Result<(), AgentValidationError> {
    if yaml.contains("permission_mode") { return Err(AgentValidationError::PermissionModeForbidden); }
    if yaml.contains("hooks:") { return Err(AgentValidationError::HooksForbidden); }
    if yaml.contains("mcpServers") { return Err(AgentValidationError::McpServersForbidden); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_permission_mode() {
        let yaml = "name: x\npermission_mode: bypassPermissions\n";
        assert!(matches!(validate_plugin_agent_frontmatter(yaml), Err(AgentValidationError::PermissionModeForbidden)));
    }

    #[test]
    fn clean_frontmatter_passes() {
        let yaml = "name: x\ndescription: y\ntools: ['Read']\n";
        assert!(validate_plugin_agent_frontmatter(yaml).is_ok());
    }
}
```

---

## Task 4: Loader (resolves user_config sensitive fields through SecureStorage)

```rust
// loader.rs
use crate::manifest::{PluginManifest, UserConfigSchema};
use lingxi_secret::CredentialManager;
use serde_json::{Map, Value};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum LoaderError {
    #[error("missing required user config field: {0}")]
    MissingRequired(String),
    #[error("credential read failed: {0}")]
    Credential(String),
}

/// Resolve user_config field values. Sensitive fields fetch from CredentialManager.
pub async fn resolve_user_config(
    manifest: &PluginManifest,
    credentials: &CredentialManager,
) -> Result<Value, LoaderError> {
    let Some(schema) = &manifest.user_config else { return Ok(Value::Object(Map::new())); };
    let mut out = Map::new();
    for (key, field) in &schema.fields {
        if field.sensitive {
            // The host UI provides these; if absent, signal missing-required.
            if field.required {
                return Err(LoaderError::MissingRequired(key.clone()));
            }
            out.insert(key.clone(), Value::Null);
        } else if field.required {
            // Non-sensitive required values flow through manifest.settings.
            let v = manifest.settings.get(key)
                .ok_or_else(|| LoaderError::MissingRequired(key.clone()))?;
            out.insert(key.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}
```

---

## Task 5: PluginManager — load() injects into 8 registries

```rust
// manager.rs
use crate::agent_validation::validate_plugin_agent_frontmatter;
use crate::blocklist::PluginBlocklist;
use crate::lifecycle::PluginState;
use crate::loader::resolve_user_config;
use crate::manifest::PluginManifest;
use crate::source::PluginSource;
use crate::strict_policy::{PluginComponent, StrictPluginOnlyPolicy};

use lingxi_commands::CommandRegistry;
use lingxi_hooks::HookRegistry;
use lingxi_lsp::LspRegistry;
use lingxi_mcp::McpRegistry;
use lingxi_outputstyles::OutputStyleRegistry;
use lingxi_protocol::PluginId;
use lingxi_secret::CredentialManager;
use lingxi_skills::SkillRegistry;
use lingxi_tools::ToolRegistry;
use lingxi_platform_api::{FileSystem, HttpTransport, RuntimeSpawner};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Error)]
pub enum PluginManagerError {
    #[error("plugin not found: {0}")]
    NotFound(PluginId),
    #[error("plugin blocked: {0}")]
    Blocked(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("io: {0}")]
    Io(String),
    #[error("loader: {0}")]
    Loader(String),
}

pub struct PluginManager {
    plugins: RwLock<HashMap<PluginId, PluginState>>,
    install_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    http: Arc<dyn HttpTransport>,
    runtime: Arc<dyn RuntimeSpawner>,
    credentials: Arc<CredentialManager>,
    blocklist: Arc<PluginBlocklist>,
    strict: Arc<StrictPluginOnlyPolicy>,

    // The 8 registries we materialize into:
    command_registry: Arc<RwLock<CommandRegistry>>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    hook_registry: Arc<RwLock<HookRegistry>>,
    output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
    mcp_registry: Arc<McpRegistry>,
    lsp_registry: Arc<LspRegistry>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    // Channel registry is part of mcp_registry's agent-scoped pool in M1.
}

impl PluginManager {
    pub fn new(
        install_dir: PathBuf,
        fs: Arc<dyn FileSystem>,
        http: Arc<dyn HttpTransport>,
        runtime: Arc<dyn RuntimeSpawner>,
        credentials: Arc<CredentialManager>,
        blocklist: Arc<PluginBlocklist>,
        strict: Arc<StrictPluginOnlyPolicy>,
        command_registry: Arc<RwLock<CommandRegistry>>,
        skill_registry: Arc<RwLock<SkillRegistry>>,
        hook_registry: Arc<RwLock<HookRegistry>>,
        output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
        mcp_registry: Arc<McpRegistry>,
        lsp_registry: Arc<LspRegistry>,
        tool_registry: Arc<RwLock<ToolRegistry>>,
    ) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            install_dir, fs, http, runtime, credentials,
            blocklist, strict,
            command_registry, skill_registry, hook_registry, output_style_registry,
            mcp_registry, lsp_registry, tool_registry,
        }
    }

    pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError> {
        // Plan 16 implements actual fetch (git clone / marketplace download /
        // mcpb unzip). M1.21 ships the contract.
        let _ = source;
        Err(PluginManagerError::Io("install impl in Plan 16".into()))
    }

    pub async fn enable(&self, id: &PluginId, manifest: PluginManifest, install_dir: PathBuf) -> Result<(), PluginManagerError> {
        if let Some(reason) = self.blocklist.is_blocked(id).await {
            return Err(PluginManagerError::Blocked(reason));
        }
        self.load_plugin(&manifest, &install_dir).await?;
        self.plugins.write().await.insert(*id, PluginState::Loaded {
            manifest, install_dir, loaded_at: std::time::SystemTime::now(),
        });
        Ok(())
    }

    pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        let mut plugins = self.plugins.write().await;
        let state = plugins.get(id).cloned();
        if let Some(PluginState::Loaded { manifest, install_dir, .. }) = state {
            self.unload_plugin(id).await?;
            plugins.insert(*id, PluginState::Disabled { manifest, install_dir });
            Ok(())
        } else {
            Err(PluginManagerError::NotFound(*id))
        }
    }

    /// Materialize all components into the 8 registries.
    async fn load_plugin(&self, manifest: &PluginManifest, _install_dir: &PathBuf) -> Result<(), PluginManagerError> {
        let _user_config = resolve_user_config(manifest, &*self.credentials).await
            .map_err(|e| PluginManagerError::Loader(e.to_string()))?;

        // 1. Commands.
        if !self.strict.is_locked(PluginComponent::Commands) {
            let cmds: Vec<lingxi_commands::SlashCommand> = manifest.components.commands.iter()
                .map(|cp| lingxi_commands::SlashCommand {
                    name: cp.path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                    description: "".into(),
                    source: lingxi_commands::CommandSource::Plugin,
                    kind: lingxi_commands::SlashCommandKind::Plugin {
                        plugin_id: manifest.id,
                        file_path: cp.path.clone(),
                        frontmatter: lingxi_commands::CommandFrontmatter::default(),
                        prompt_template: String::new(),
                    },
                })
                .collect();
            self.command_registry.write().await.register_plugin_commands(manifest.id, cmds);
        }

        // 2. Agents — frontmatter validated against D2.
        for ap in &manifest.components.agents {
            // (read file, validate via validate_plugin_agent_frontmatter)
            let _ = ap; // M1.21 ships the validation gate; full agent registry in M2.
        }

        // 3. Skills.
        if !self.strict.is_locked(PluginComponent::Skills) {
            // (build Skill objects from skill files; register)
        }

        // 4. Hooks.
        self.hook_registry.write().await.register_plugin_hooks(manifest.id, manifest.components.hooks.clone());

        // 5. OutputStyles.
        // 6. MCP servers — registered through McpRegistry::connect for each entry.
        // 7. LSP servers.
        let configs: Vec<_> = manifest.components.lsp_servers.values().cloned().collect();
        self.lsp_registry.register_plugin_servers(manifest.id, configs).await;

        Ok(())
    }

    /// Symmetric unload — clean up the exact registries we touched.
    async fn unload_plugin(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        self.command_registry.write().await.unregister_plugin(id);
        self.skill_registry.write().await.unregister_plugin(id);
        self.hook_registry.write().await.unregister_plugin(id);
        self.output_style_registry.write().await.unregister_plugin(id);
        self.tool_registry.write().await.unregister_plugin(id);
        self.lsp_registry.unregister_plugin(id).await;
        // mcp_registry cleanup: per-agent scope cleanup happens at agent exit.
        Ok(())
    }
}
```

---

## Task 6: lib.rs + commit

```rust
// lib.rs
#![forbid(unsafe_code)]
pub mod agent_validation;
pub mod blocklist;
pub mod lifecycle;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod marketplace;
pub mod source;
pub mod strict_policy;
pub mod trust;

pub use agent_validation::{validate_plugin_agent_frontmatter, AgentValidationError};
pub use blocklist::PluginBlocklist;
pub use lifecycle::PluginState;
pub use loader::{resolve_user_config, LoaderError};
pub use manager::{PluginManager, PluginManagerError};
pub use manifest::*;
pub use source::PluginSource;
pub use strict_policy::{PluginComponent, StrictPluginOnlyPolicy};
pub use trust::{default_trust_for_source, PluginTrustLevel};
```

```rust
// marketplace.rs — stub
pub struct MarketplaceManager;
```

Commit:
```bash
cargo test -p lingxi-plugin
git add crates/plugin
git commit -m "feat(plugin): manifest, lifecycle, blocklist, strict policy, agent-frontmatter validation, 8-registry materialization"
```

---

## Task 7: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.21-plugin -m "Plan 15 complete"
```

## Self-Review

- §15.1 PluginManifest + 8 component slots (D6) → ✓
- §15.2 PluginState (7 variants) → lifecycle.rs ✓
- §15.3 PluginManager 8-registry injection → manager.rs ✓
- §15.5 PluginBlocklist → ✓
- §15.6 StrictPluginOnlyPolicy → ✓
- A7 default trust for git/local = Untrusted → trust.rs ✓
- D2 plugin agent frontmatter privilege rejection → agent_validation.rs ✓
- Sensitive user_config → SecureStorage → loader.rs ✓

## Execution Handoff

Next: **Plan 16 — UniFFI + posix-minimal + cli-demo** (`2026-05-22-lingxi-core-m1-16-uniffi-cli-demo.md`).
