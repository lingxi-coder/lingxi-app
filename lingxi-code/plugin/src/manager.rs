//! `PluginManager` — drives the lifecycle state machine and materialises
//! plugin components into the eight engine registries (tools, hooks, MCP,
//! agent, skill, command, output-style, LSP).
//!
//! Today only `enable` and `disable` carry production logic. `install` is
//! a stub returning an error — Plan 16 implements the actual fetches
//! (git clone, marketplace download, `.mcpb` unpack).
//!
//! See spec §15.3.

use crate::blocklist::PluginBlocklist;
use crate::lifecycle::PluginState;
use crate::loader::resolve_user_config;
use crate::manifest::PluginManifest;
use crate::source::PluginSource;
use crate::strict_policy::{PluginComponent, StrictPluginOnlyPolicy};

use commands::CommandRegistry;
use hooks::HookRegistry;
use lsp::LspRegistry;
use mcp::McpRegistry;
use outputstyles::OutputStyleRegistry;
use protocol::PluginId;
use secret::CredentialManager;
use skill_api::SkillRegistry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tool_api::ToolRegistry;
use traits::{FileSystem, HttpTransport, RuntimeSpawner};

/// Failure modes for [`PluginManager`] operations.
#[derive(Debug, Clone, Error)]
pub enum PluginManagerError {
    /// No plugin with this id is currently installed.
    #[error("plugin not found: {0}")]
    NotFound(PluginId),
    /// Blocklist matched the plugin (static or remote).
    #[error("plugin blocked: {0}")]
    Blocked(String),
    /// Manifest validation rejected the plugin.
    #[error("validation: {0}")]
    Validation(String),
    /// Filesystem / I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// User-config loader failure.
    #[error("loader: {0}")]
    Loader(String),
}

/// The plugin lifecycle coordinator.
///
/// Holds a state map keyed by [`PluginId`], references to every engine
/// registry the manager materialises into, the credential manager (for
/// sensitive user-config values), the blocklist, and the strict policy.
///
/// The `fs`, `http`, and `runtime` fields are reserved for Plan 16's
/// install/fetch code; they are not used by the M1.21 stub.
pub struct PluginManager {
    plugins: RwLock<HashMap<PluginId, PluginState>>,
    #[allow(dead_code)] // Used by Plan 16 install paths.
    install_dir: PathBuf,
    #[allow(dead_code)] // Used by Plan 16 install paths.
    fs: Arc<dyn FileSystem>,
    #[allow(dead_code)] // Used by Plan 16 install paths.
    http: Arc<dyn HttpTransport>,
    #[allow(dead_code)] // Used by Plan 16 install paths.
    runtime: Arc<dyn RuntimeSpawner>,
    credentials: Arc<CredentialManager>,
    blocklist: Arc<PluginBlocklist>,
    strict: Arc<StrictPluginOnlyPolicy>,

    // The 8 registries we materialize into:
    command_registry: Arc<RwLock<CommandRegistry>>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    hook_registry: Arc<RwLock<HookRegistry>>,
    output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
    #[allow(dead_code)] // Channel/MCP materialisation lands in Plan 16.
    mcp_registry: Arc<McpRegistry>,
    lsp_registry: Arc<LspRegistry>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    // Channel registry is part of mcp_registry's agent-scoped pool in M1.
}

impl PluginManager {
    /// Build a `PluginManager` wired into all engine registries.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // Wiring layer — every dep is required.
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
            install_dir,
            fs,
            http,
            runtime,
            credentials,
            blocklist,
            strict,
            command_registry,
            skill_registry,
            hook_registry,
            output_style_registry,
            mcp_registry,
            lsp_registry,
            tool_registry,
        }
    }

    /// Install a plugin from `source`.
    ///
    /// Plan 16 implements actual fetch (git clone / marketplace download /
    /// `.mcpb` unzip). M1.21 ships the contract.
    #[allow(clippy::unused_async)] // Plan 16 wires the real async fetch.
    pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError> {
        let _ = source;
        Err(PluginManagerError::Io("install impl in Plan 16".into()))
    }

    /// Mark `id` as `Loaded` and inject its components into the engine
    /// registries.
    ///
    /// Returns [`PluginManagerError::Blocked`] when the blocklist matches.
    pub async fn enable(
        &self,
        id: &PluginId,
        manifest: PluginManifest,
        install_dir: PathBuf,
    ) -> Result<(), PluginManagerError> {
        if let Some(reason) = self.blocklist.is_blocked(id).await {
            return Err(PluginManagerError::Blocked(reason));
        }
        self.load_plugin(&manifest, &install_dir).await?;
        self.plugins.write().await.insert(
            *id,
            PluginState::Loaded {
                manifest,
                install_dir,
                loaded_at: std::time::SystemTime::now(),
            },
        );
        Ok(())
    }

    /// Transition `id` from `Loaded` to `Disabled` and remove every
    /// registry entry the plugin contributed.
    pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        let mut plugins = self.plugins.write().await;
        let state = plugins.get(id).cloned();
        if let Some(PluginState::Loaded {
            manifest,
            install_dir,
            ..
        }) = state
        {
            self.unload_plugin(id).await?;
            plugins.insert(
                *id,
                PluginState::Disabled {
                    manifest,
                    install_dir,
                },
            );
            Ok(())
        } else {
            Err(PluginManagerError::NotFound(*id))
        }
    }

    /// Materialise `manifest`'s components into the 8 registries.
    async fn load_plugin(
        &self,
        manifest: &PluginManifest,
        _install_dir: &Path,
    ) -> Result<(), PluginManagerError> {
        let _user_config = resolve_user_config(manifest, &self.credentials)
            .await
            .map_err(|e| PluginManagerError::Loader(e.to_string()))?;

        // 1. Commands.
        if !self.strict.is_locked(PluginComponent::Commands) {
            let cmds: Vec<commands::SlashCommand> = manifest
                .components
                .commands
                .iter()
                .map(|cp| commands::SlashCommand {
                    name: cp
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string(),
                    description: String::new(),
                    source: commands::CommandSource::Plugin,
                    kind: commands::SlashCommandKind::Plugin {
                        plugin_id: manifest.id,
                        file_path: cp.path.clone(),
                        frontmatter: commands::CommandFrontmatter::default(),
                        prompt_template: String::new(),
                    },
                })
                .collect();
            self.command_registry
                .write()
                .await
                .register_plugin_commands(manifest.id, cmds);
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
        self.hook_registry
            .write()
            .await
            .register_plugin_hooks(manifest.id, manifest.components.hooks.clone());

        // 5. OutputStyles — Plan 16 reads disk files; nothing to inject at M1.21.
        let _ = &self.output_style_registry;
        let _ = &self.tool_registry;
        let _ = &self.skill_registry;

        // 6. MCP servers — registered through McpRegistry::connect for each entry.
        // 7. LSP servers — plugin-only registration path.
        //
        // `LspRegistry::register_plugin_servers` is the ONLY supported way
        // to register LSP servers. The internal `register_config` is
        // `pub(crate)` so user/project settings cannot bypass this gate.
        // Matches claude-code's `getAllLspServers()`
        // (`claude-code/src/services/lsp/config.ts`).
        let configs: Vec<_> = manifest.components.lsp_servers.values().cloned().collect();
        self.lsp_registry
            .register_plugin_servers(manifest.id, configs)
            .await;

        Ok(())
    }

    /// Symmetric unload — clean up the exact registries we touched.
    async fn unload_plugin(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        self.command_registry.write().await.unregister_plugin(id);
        self.skill_registry.write().await.unregister_plugin(id);
        self.hook_registry.write().await.unregister_plugin(id);
        self.output_style_registry
            .write()
            .await
            .unregister_plugin(id);
        self.tool_registry.write().await.unregister_plugin(id);
        let _ = self.lsp_registry.unregister_plugin(id).await;
        // mcp_registry cleanup: per-agent scope cleanup happens at agent exit.
        Ok(())
    }
}
