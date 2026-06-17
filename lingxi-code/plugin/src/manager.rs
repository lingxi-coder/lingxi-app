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

use command_api::CommandRegistry;
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
    /// The **local-path** arm is wired: the plugin directory is discovered in
    /// place (no copy — claude-code's `--add-dir` local plugins are loaded
    /// from their source location), its manifest + components are read via
    /// [`crate::discovery::discover_installed_plugins`]'s per-directory loader,
    /// and a fresh [`PluginId`] is minted and returned. The remaining
    /// network-backed arms (git clone / marketplace download / `.mcpb` unzip)
    /// require the marketplace + fetch machinery that is not yet ported and so
    /// return a typed error rather than panicking.
    pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginManagerError> {
        match source {
            PluginSource::LocalPath { path } => {
                let discovered = crate::discovery::discover_installed_plugins(
                    path.parent().unwrap_or(&path),
                )
                .await;
                // Match by directory: the discovery walk returns siblings of
                // `path`'s parent; pick the one whose install dir is `path`.
                let found = discovered.into_iter().find(|(_, _, dir)| dir == &path);
                if let Some((id, manifest, dir)) = found {
                    self.enable(&id, manifest, dir).await?;
                    Ok(id)
                } else {
                    Err(PluginManagerError::Io(format!(
                        "no plugin manifest found at {}",
                        path.display()
                    )))
                }
            }
            other => Err(PluginManagerError::Io(format!(
                "install from {other:?} requires marketplace/git fetch (not yet wired); \
                 install a pre-fetched plugin directory via PluginSource::LocalPath"
            ))),
        }
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
        install_dir: &Path,
    ) -> Result<(), PluginManagerError> {
        let _user_config = resolve_user_config(manifest, &self.credentials)
            .await
            .map_err(|e| PluginManagerError::Loader(e.to_string()))?;

        // 1. Commands.
        if !self.strict.is_locked(PluginComponent::Commands) {
            let cmds: Vec<command_api::SlashCommand> = manifest
                .components
                .commands
                .iter()
                .map(|cp| command_api::SlashCommand {
                    name: cp
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string(),
                    description: String::new(),
                    source: command_api::CommandSource::Plugin,
                    kind: command_api::SlashCommandKind::Plugin {
                        plugin_id: manifest.id,
                        file_path: cp.path.clone(),
                        frontmatter: command_api::CommandFrontmatter::default(),
                        prompt_template: String::new(),
                    },
                    loaded_from: Some("plugin".to_string()),
                    ..command_api::SlashCommand::default()
                })
                .collect();
            self.command_registry
                .write()
                .await
                .register_plugin_commands(manifest.id, cmds);
        }

        // 2. Agents — frontmatter validated against D2 (the privilege gate).
        //    The agent *catalog* materialisation happens at the composition
        //    root via `agent::load_agents_from_dirs([(…/agents,
        //    AgentSource::Plugin)])` (the manager holds no agent-catalog ref,
        //    faithful to the dir-scan catalog design). Here we gate each
        //    plugin agent file's YAML frontmatter so a plugin cannot smuggle
        //    `permission_mode` / `hooks:` / `mcpServers` escalations
        //    (`validate_plugin_agent_frontmatter`, agent_validation.rs:29).
        for ap in &manifest.components.agents {
            let abs = if ap.path.is_absolute() {
                ap.path.clone()
            } else {
                install_dir.join(&ap.path)
            };
            if let Ok(raw) = tokio::fs::read_to_string(&abs).await {
                if let Some(yaml) = extract_frontmatter(&raw) {
                    if let Err(e) = crate::validate_plugin_agent_frontmatter(yaml) {
                        return Err(PluginManagerError::Validation(format!(
                            "agent {}: {e}",
                            abs.display()
                        )));
                    }
                }
            }
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

/// Extract the YAML frontmatter block (between leading `---` fences) of a
/// markdown agent file, if present. Returns `None` when the file has no
/// frontmatter. Mirrors the `---\n…\n---` convention claude-code's agent
/// loader uses (and the engine's `parse_agent_markdown`).
fn extract_frontmatter(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix("---\n").or_else(|| raw.strip_prefix("---\r\n"))?;
    // Find the closing fence at the start of a line.
    let end = rest
        .find("\n---")
        .or_else(|| rest.find("\r\n---"))?;
    Some(&rest[..end])
}
