//! `PluginManager` — drives the lifecycle state machine and materialises
//! plugin components into the eight engine registries (tools, hooks, MCP,
//! agent, skill, command, output-style, LSP).
//!
//! `enable` materialises commands, hooks, agents (frontmatter-gated),
//! skills, output-styles, LSP servers, and MCP servers (live-connected
//! through the same `McpRegistry::connect_all` path as normal configured
//! `.mcp.json` servers). `disable` symmetrically removes them. `install`'s local-path arm discovers + enables a
//! pre-fetched plugin dir; the network arms (git clone, marketplace
//! download, `.mcpb` unpack) return a typed, capability-named error until
//! the fetch + marketplace-policy machinery is ported.
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
use mcp::{McpRegistry, McpServerConfig};
use outputstyles::{OutputStyle, OutputStyleFrontmatter, OutputStyleRegistry, OutputStyleSource};
use protocol::PluginId;
use secret::CredentialManager;
use skill_api::{parse_skill_markdown, LoadedFrom, SkillRegistry, SkillSource};
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
    /// Network fetch failure (git clone / HTTP download). The body carries the
    /// byte-faithful claude-code failure detail.
    #[error("fetch: {0}")]
    Fetch(String),
    /// Archive-unpack failure (`.mcpb` zip extract).
    #[error("unpack: {0}")]
    Unpack(String),
    /// Marketplace catalog / policy failure.
    #[error("marketplace: {0}")]
    Marketplace(String),
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
    mcp_registry: Arc<McpRegistry>,
    lsp_registry: Arc<LspRegistry>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    // Channel registry is part of mcp_registry's agent-scoped pool in M1.
    /// Scoped MCP server names (`plugin:{plugin}:{server}`) each plugin seeded
    /// into `mcp_registry.connections`, so [`Self::unload_plugin`] can remove
    /// exactly those entries (the registry has no plugin-ownership index).
    plugin_mcp_names: RwLock<HashMap<PluginId, Vec<String>>>,
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
            plugin_mcp_names: RwLock::new(HashMap::new()),
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
            // The network-backed arms each name the specific fetch capability
            // that is not yet ported, so the error is actionable. The actual
            // machinery (`marketplace.rs` is a placeholder with no HTTP/clone/
            // unzip code) plus the marketplace-policy gates
            // (`getStrictKnownMarketplaces` / blocklist) are residual — until
            // then, install a pre-fetched plugin directory via
            // `PluginSource::LocalPath`.
            PluginSource::OfficialMarketplace { name } => Err(PluginManagerError::Io(format!(
                "install of '{name}' from the official marketplace requires the \
                 marketplace fetch loop (HTTP listing + signed-manifest download); \
                 not yet wired — install a pre-fetched plugin directory via \
                 PluginSource::LocalPath"
            ))),
            PluginSource::Marketplace { url, name } => {
                let source = PluginSource::Marketplace {
                    url: url.clone(),
                    name: name.clone(),
                };
                let mkt = crate::marketplace::MarketplaceManager::new(self.install_dir.clone());
                // 1. Clone + parse the marketplace catalog (keyed by the marketplace
                //    repo identity so distinct marketplaces don't collide).
                let mkt_name = repo_dir_for_url(&url);
                let (index, clone_dir) = mkt
                    .resolve_index_via_git(&url, &mkt_name)
                    .await
                    .map_err(PluginManagerError::Marketplace)?;
                // 2. Find the plugin entry by name (byte-exact not-found message).
                let entry = index.plugins.iter().find(|p| p.name == name).ok_or_else(|| {
                    let avail = index
                        .plugins
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    PluginManagerError::Marketplace(format!(
                        "Marketplace '{name}' not found. Available marketplaces: {avail}"
                    ))
                })?;
                // 3. Resolve the plugin dir inside the clone (lexical guard), then
                //    canonicalize and assert it is STILL inside the clone — a
                //    120000 symlink in the untrusted repo (e.g. `path` pointing at
                //    `~/.ssh`) would otherwise let the copy follow it out of the
                //    clone and exfiltrate host files into the cache.
                let src_dir = crate::marketplace::MarketplaceManager::plugin_dir_in_clone(
                    &clone_dir, entry,
                )
                .map_err(PluginManagerError::Marketplace)?;
                let real_src = tokio::fs::canonicalize(&src_dir).await.map_err(|_| {
                    PluginManagerError::Marketplace(format!(
                        "Marketplace name '{name}' resolves to a path outside the cache directory"
                    ))
                })?;
                let real_clone = tokio::fs::canonicalize(&clone_dir).await.map_err(|e| {
                    PluginManagerError::Marketplace(format!("marketplace clone unreadable: {e}"))
                })?;
                if !real_src.starts_with(&real_clone) {
                    return Err(PluginManagerError::Marketplace(format!(
                        "Marketplace name '{name}' resolves to a path outside the cache directory"
                    )));
                }
                // 4. Materialize under the catalog's DECLARED name (the segment
                //    reboot discovery resolves `plugin@<marketplace-name>` to),
                //    not the URL slug.
                let landed = self.copy_into_cache(&real_src, &index.name).await?;
                // 5. Finalize (load manifest + components, stamp source, enable).
                self.finalize_install(source, landed).await
            }
            PluginSource::Git { url, ref_ } => {
                let source = PluginSource::Git {
                    url: url.clone(),
                    ref_: ref_.clone(),
                };
                // Clone under `repos/<host>/<owner>/<repo>/` (a fresh checkout —
                // remove any stale clone first, matching re-install semantics).
                let repo_subpath = repo_dir_for_url(&url);
                let clone_dir = self.install_dir.join("repos").join(&repo_subpath);
                if clone_dir.exists() {
                    tokio::fs::remove_dir_all(&clone_dir).await.ok();
                }
                if let Some(parent) = clone_dir.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        PluginManagerError::Fetch(format!("Failed to clone repository: {e}"))
                    })?;
                }
                // git2 is synchronous + blocks on the network → spawn_blocking.
                let (u, r, cd) = (url.clone(), ref_.clone(), clone_dir.clone());
                tokio::task::spawn_blocking(move || crate::git::clone_plugin_git(&u, &r, &cd))
                    .await
                    .map_err(|e| {
                        PluginManagerError::Fetch(format!("Failed to clone repository: {e}"))
                    })?
                    .map_err(PluginManagerError::Fetch)?;
                // The clone IS the plugin dir (single-plugin repo). Materialize it
                // into the versioned cache, then finalize like the local arm.
                let landed = self.copy_into_cache(&clone_dir, &repo_subpath).await?;
                self.finalize_install(source, landed).await
            }
            PluginSource::Mcpb { path, hash } => {
                let source = PluginSource::Mcpb {
                    path: path.clone(),
                    hash: hash.clone(),
                };
                // 1. Read the bundle bytes (local file; remote download deferred).
                let bytes = tokio::fs::read(&path).await.map_err(|e| {
                    PluginManagerError::Fetch(format!(
                        "Failed to download MCPB {}: {e}",
                        path.display()
                    ))
                })?;
                // 2. Integrity: the content hash is the ONLY tamper check (claude-
                //    code has no signature). Verified before extraction.
                if !hash.is_empty() && crate::mcpb::sha256_hex(&bytes) != *hash {
                    return Err(PluginManagerError::Unpack(format!(
                        "MCPB manifest invalid at {} (hash mismatch)",
                        path.display()
                    )));
                }
                // 3. mkdtemp → extract (path-traversal / too-many-files / zip-bomb
                //    guarded), on a blocking thread.
                let tmp = tempfile::tempdir().map_err(|e| {
                    PluginManagerError::Unpack(format!(
                        "Failed to extract MCPB {}: {e}",
                        path.display()
                    ))
                })?;
                let tmp_path = tmp.path().to_path_buf();
                tokio::task::spawn_blocking(move || {
                    crate::mcpb::unpack_mcpb(&bytes, &tmp_path)?;
                    // 4. Normalize: ensure a `.claude-plugin/plugin.json` exists
                    //    (translate a root `manifest.json` if needed).
                    crate::mcpb::ensure_plugin_manifest(&tmp_path)
                })
                .await
                .map_err(|e| PluginManagerError::Unpack(e.to_string()))?
                .map_err(PluginManagerError::Unpack)?;
                // 5. Land into the versioned cache + finalize.
                let bundle = mcpb_bundle_name(&path);
                let landed = self.copy_into_cache(tmp.path(), &bundle).await?;
                self.finalize_install(source, landed).await
            }
            PluginSource::BuiltIn => Err(PluginManagerError::Io(
                "BuiltIn plugins are compiled into the engine and are not \
                 installed via PluginManager::install"
                    .to_string(),
            )),
        }
    }

    /// Shared tail for every network install arm: a valid plugin directory is
    /// now on disk at `landed_dir`. Load its manifest + auto-detected components
    /// (reusing the local-path loader), stamp the REAL fetch `source` (so trust
    /// + provenance match the origin rather than defaulting to `LocalPath`), and
    /// enable it.
    async fn finalize_install(
        &self,
        source: PluginSource,
        landed_dir: PathBuf,
    ) -> Result<PluginId, PluginManagerError> {
        let Some((id, mut manifest)) = crate::discovery::load_plugin_from_path(&landed_dir).await
        else {
            return Err(PluginManagerError::Io(format!(
                "no plugin manifest found at {}",
                landed_dir.display()
            )));
        };
        manifest.source = source.clone();
        manifest.trust_level = crate::trust::default_trust_for_source(&source);
        self.enable(&id, manifest, landed_dir).await?;
        Ok(id)
    }

    /// Materialize a freshly-fetched plugin tree at `src_dir` into the versioned
    /// cache layout `cache/<marketplace>/<plugin>/<version>/` that
    /// [`crate::discovery::discover_enabled_plugins`] resolves. `<marketplace>`
    /// is the sanitized source identity (`repo_subpath`); `<plugin>`/`<version>`
    /// come from the just-fetched `.claude-plugin/plugin.json` (version falls
    /// back to `"unknown"` when absent). Returns the landed `<version>/` dir.
    async fn copy_into_cache(
        &self,
        src_dir: &Path,
        repo_subpath: &str,
    ) -> Result<PathBuf, PluginManagerError> {
        // Read name + version from the fetched manifest to compute the path.
        let manifest_path = src_dir.join(".claude-plugin").join("plugin.json");
        let raw = tokio::fs::read_to_string(&manifest_path).await.map_err(|_| {
            PluginManagerError::Io(format!(
                "no plugin manifest found at {}",
                src_dir.display()
            ))
        })?;
        let json: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| PluginManagerError::Validation(format!("invalid plugin.json: {e}")))?;
        let name = json
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| PluginManagerError::Validation("plugin.json missing name".into()))?;
        let version = json
            .get("version")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown");

        let dest = self
            .install_dir
            .join("cache")
            .join(crate::discovery::sanitize_segment(repo_subpath, false))
            .join(crate::discovery::sanitize_segment(name, false))
            .join(crate::discovery::sanitize_segment(version, true));
        if dest.exists() {
            tokio::fs::remove_dir_all(&dest).await.ok();
        }
        copy_dir_recursive(src_dir, &dest)
            .await
            .map_err(|e| PluginManagerError::Io(format!("failed to materialize plugin cache: {e}")))?;
        // Durably record the install (marketplace → plugin → version) so a later
        // launch can re-discover the exact cache dir. Best-effort: a record-write
        // failure must not fail an otherwise-successful install.
        if let Err(e) = crate::installed::record(
            &self.install_dir,
            repo_subpath,
            name,
            version,
            crate::installed::now_ms(),
        )
        .await
        {
            tracing::warn!(error = %e, "failed to write installed_plugins.json record");
        }
        Ok(dest)
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
    #[allow(clippy::too_many_lines)] // Wiring layer — validate-then-mutate over 7 component slots.
    async fn load_plugin(
        &self,
        manifest: &PluginManifest,
        install_dir: &Path,
    ) -> Result<(), PluginManagerError> {
        let _user_config = resolve_user_config(manifest, &self.credentials)
            .await
            .map_err(|e| PluginManagerError::Loader(e.to_string()))?;

        // All-or-nothing ordering: VALIDATE every fallible input BEFORE
        // mutating any live registry, so a rejected plugin never leaves an
        // orphaned command / hook behind. claude-code loads a plugin as a
        // single unit; a privilege-escalating agent rejects the whole plugin,
        // not just the agent.

        // (a) Agents — frontmatter validated against D2 (the privilege gate)
        //     FIRST. The agent *catalog* materialisation happens at the
        //     composition root via `agent::load_agents_from_dirs([(…/agents,
        //     AgentSource::Plugin)])` (the manager holds no agent-catalog ref,
        //     faithful to the dir-scan catalog design). Here we gate each
        //     plugin agent file's YAML frontmatter so a plugin cannot smuggle
        //     `permission_mode` / `hooks:` / `mcpServers` escalations
        //     (`validate_plugin_agent_frontmatter`, agent_validation.rs:29).
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

        // (b) Commands — read each command markdown file's BODY + frontmatter
        //     (NOT empty strings). `createPluginFromPath` (`pluginLoader.ts`)
        //     loads each command file's content as the prompt; an empty
        //     prompt_template would expand to an inert prompt
        //     (`command-api/expand.rs:74` substitutes over prompt_template).
        //     Build a faithful `MarkdownCommandFile` via the command-api
        //     primitive, then re-stamp it as a `Plugin`-kind command carrying
        //     the plugin id (so unload can target it). A file that cannot be
        //     read is skipped (TS returns null + filters).
        let mut cmds: Vec<command_api::SlashCommand> = Vec::new();
        if !self.strict.is_locked(PluginComponent::Commands) {
            let commands_dir = install_dir.join("commands");
            for cp in &manifest.components.commands {
                let abs = if cp.path.is_absolute() {
                    cp.path.clone()
                } else {
                    install_dir.join(&cp.path)
                };
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                let file = command_api::parse_command_markdown(
                    &raw,
                    abs.clone(),
                    commands_dir.clone(),
                    command_api::CommandSource::Plugin,
                );
                // Build the faithful Markdown command (name/description/body/
                // frontmatter/argument metadata), then convert to Plugin kind.
                let base =
                    command_api::build_markdown_command(&file, command_api::CommandSource::Plugin);
                // Namespace the command `{plugin}:{namespace}:{base}` to mirror
                // `getCommandNameFromFile` (`loadPluginCommands.ts:60-97`), which
                // ALWAYS prefixes `${pluginName}:` (consistent with skills /
                // output-styles below). `base.name` already carries the
                // subdirectory namespace from `command_name_from_path`, so the
                // single `{plugin}:` prefix completes the canonical name.
                let namespaced_name = format!("{}:{}", manifest.name, base.name);
                let (frontmatter, prompt_template) = match base.kind {
                    command_api::SlashCommandKind::Markdown {
                        frontmatter,
                        prompt_template,
                        ..
                    } => (frontmatter, prompt_template),
                    _ => (
                        command_api::CommandFrontmatter::default(),
                        file.content.clone(),
                    ),
                };
                cmds.push(command_api::SlashCommand {
                    name: namespaced_name,
                    source: command_api::CommandSource::Plugin,
                    kind: command_api::SlashCommandKind::Plugin {
                        plugin_id: manifest.id,
                        file_path: abs,
                        frontmatter,
                        prompt_template,
                    },
                    loaded_from: Some("plugin".to_string()),
                    ..base
                });
            }
        }

        // (c) Skills — read each `skills/<name>/SKILL.md`, parse its frontmatter
        //     body, namespace the name as `{plugin}:{skill}` (consistent with
        //     commands / agents / output-styles — `loadPluginOutputStyles.ts:55`)
        //     and stamp the owning plugin id so unload can target it. A file
        //     that cannot be read or parsed is skipped (TS filters nulls).
        let plugin_name = &manifest.name;
        let mut skills: Vec<skill_api::Skill> = Vec::new();
        if !self.strict.is_locked(PluginComponent::Skills) {
            for sp in &manifest.components.skills {
                let abs = if sp.path.is_absolute() {
                    sp.path.clone()
                } else {
                    install_dir.join(&sp.path)
                };
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                let Ok(mut skill) =
                    parse_skill_markdown(&raw, abs.clone(), SkillSource::Plugin, LoadedFrom::Plugin)
                else {
                    continue;
                };
                skill.name = format!("{plugin_name}:{}", skill.name);
                skill.plugin_id = Some(manifest.id);
                skills.push(skill);
            }
        }

        // (d) Output styles — read each `output-styles/*.md`, parse the body as
        //     the system-prompt addendum, namespace the name `{plugin}:{name}`
        //     (`loadPluginOutputStyles.ts:55`). The body becomes
        //     `system_prompt_addendum` (TS `prompt: markdownContent.trim()`).
        let mut styles: Vec<OutputStyle> = Vec::new();
        if !self.strict.is_locked(PluginComponent::OutputStyles) {
            for op in &manifest.components.output_styles {
                let abs = if op.path.is_absolute() {
                    op.path.clone()
                } else {
                    install_dir.join(&op.path)
                };
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                let stem = abs
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                let disk = outputstyles::parse_output_style(&raw, stem);
                let name = format!("{plugin_name}:{}", disk.name);
                styles.push(OutputStyle {
                    name: name.clone(),
                    description: disk.description.clone(),
                    source: OutputStyleSource::Plugin,
                    frontmatter: OutputStyleFrontmatter {
                        name,
                        description: disk.description,
                        ..Default::default()
                    },
                    system_prompt_addendum: disk.prompt,
                    source_path: Some(abs),
                });
            }
        }

        // (e) MCP servers — scope each `.mcp.json` entry as
        //     `plugin:{plugin}:{server}` so it is keyed identically to a
        //     normal configured server (`addPluginScopeToServers`,
        //     `mcpPluginIntegration.ts:341-359`, uses scope `dynamic`; the
        //     discovery loader already stamps `ConfigScope::Dynamic`). These
        //     scoped configs are connected at enable time through the SAME
        //     `McpRegistry::connect_all` path the engine uses for configured
        //     `.mcp.json` servers (claude-code `getClaudeCodeMcpConfigs`
        //     merges plugin servers into the SAME configs map that the
        //     connection manager dials eagerly at startup — `config.ts:1114`).
        let mcp_scoped: Vec<McpServerConfig> = if self
            .strict
            .is_locked(PluginComponent::McpServers)
        {
            Vec::new()
        } else {
            manifest
                .components
                .mcp_servers
                .values()
                .map(|cfg| {
                    let mut scoped = cfg.clone();
                    scoped.name = format!("plugin:{plugin_name}:{}", cfg.name);
                    scoped
                })
                .collect()
        };

        // ---- All inputs validated; mutate the live registries now. ----

        // 1. Commands.
        if !cmds.is_empty() {
            self.command_registry
                .write()
                .await
                .register_plugin_commands(manifest.id, cmds);
        }

        // 3. Skills.
        if !skills.is_empty() {
            self.skill_registry
                .write()
                .await
                .register_plugin_skills(manifest.id, skills);
        }

        // 4. Hooks.
        self.hook_registry
            .write()
            .await
            .register_plugin_hooks(manifest.id, manifest.components.hooks.clone());

        // 5. OutputStyles.
        if !styles.is_empty() {
            self.output_style_registry
                .write()
                .await
                .register_plugin_styles(manifest.id, styles);
        }
        let _ = &self.tool_registry;

        // 6. MCP servers — route each scoped config through the registry's
        //    live `connect_all` path, the SAME path engine-desktop uses for
        //    normal configured `.mcp.json` servers (so plugin servers auto-dial
        //    at bootstrap, transitioning Connecting→Connected, or to a
        //    loop-eligible `Disconnected{last_error}` on failure that the
        //    already-spawned reconnect loop retries). `connect_all` honors the
        //    same `disabled` gating as configured servers. Remember the scoped
        //    names FIRST so `unload_plugin` can remove exactly these entries
        //    regardless of the state `connect_all` leaves them in.
        if !mcp_scoped.is_empty() {
            let names: Vec<String> = mcp_scoped.iter().map(|cfg| cfg.name.clone()).collect();
            self.plugin_mcp_names.write().await.insert(manifest.id, names);
            self.mcp_registry.connect_all(mcp_scoped).await;
        }

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
        // MCP cleanup: remove exactly the scoped `plugin:{plugin}:*` entries
        // this plugin seeded into the registry's connection map.
        if let Some(names) = self.plugin_mcp_names.write().await.remove(id) {
            let mut conns = self.mcp_registry.connections.write().await;
            for n in &names {
                conns.remove(n);
            }
        }
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

/// Derive a stable, sanitized `host/owner/repo` sub-path from a git URL, used as
/// both the `repos/<…>/` clone destination and the cache `<marketplace>`
/// identity. Strips a trailing `.git`, the `git@host:owner/repo` SSH form, and
/// any URL scheme; each path segment is sanitized to `[A-Za-z0-9._-]`.
fn repo_dir_for_url(url: &str) -> String {
    // Normalize the SSH `git@host:owner/repo` form to `host/owner/repo`.
    let stripped = if let Some(rest) = url.strip_prefix("git@") {
        rest.replacen(':', "/", 1)
    } else {
        // Drop the scheme (`https://`, `file://`, `ssh://`, …).
        url.split("://").last().unwrap_or(url).to_string()
    };
    let stripped = stripped.trim_end_matches('/').trim_end_matches(".git");
    let joined: Vec<String> = stripped
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(|seg| crate::discovery::sanitize_segment(seg, true))
        .collect();
    if joined.is_empty() {
        "repo".to_string()
    } else {
        joined.join("/")
    }
}

/// Derive a stable cache `<marketplace>` segment for a `.mcpb` bundle from its
/// file name (the stem, sanitized). e.g. `/x/my-plugin.mcpb` → `my-plugin`.
fn mcpb_bundle_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("mcpb");
    crate::discovery::sanitize_segment(stem, true)
}

/// Recursively copy the directory tree at `src` to `dst` (creating `dst`).
/// Symlink-safe: only regular files and directories are copied (matching
/// claude-code's `copyDir`, which skips special entries); symlinks and other
/// non-regular entries are silently skipped so a malicious clone cannot plant a
/// dangling/escaping link in the cache.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Refuse to copy a symlinked ROOT: `read_dir` follows it to the target
    // (potentially outside the source tree), which an untrusted clone could
    // abuse to exfiltrate arbitrary host files into the cache. Entries
    // discovered INSIDE a directory are already skipped if they are symlinks,
    // but the walk's own root is not covered by that check.
    if tokio::fs::symlink_metadata(src)
        .await?
        .file_type()
        .is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to copy a symlinked directory",
        ));
    }
    tokio::fs::create_dir_all(dst).await?;
    // Iterative DFS over (src, dst) pairs to avoid boxing for async recursion.
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            // `file_type()` does NOT follow symlinks → a symlink reports neither
            // is_dir nor is_file here and is skipped.
            let ft = entry.file_type().await?;
            let child_from = entry.path();
            let child_to = to.join(entry.file_name());
            if ft.is_dir() {
                tokio::fs::create_dir_all(&child_to).await?;
                stack.push((child_from, child_to));
            } else if ft.is_file() {
                tokio::fs::copy(&child_from, &child_to).await?;
            }
            // else: symlink / device / fifo → skipped.
        }
    }
    Ok(())
}
