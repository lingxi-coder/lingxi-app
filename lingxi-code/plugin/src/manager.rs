//! `PluginManager` — drives the lifecycle state machine and materialises
//! plugin components into the eight engine registries (tools, hooks, MCP,
//! agent, skill, command, output-style, LSP).
//!
//! `enable` materialises commands, hooks, agents (frontmatter-gated),
//! skills, output-styles, LSP servers, and MCP servers (live-connected
//! through the same `McpRegistry::connect_all` path as normal configured
//! `.mcp.json` servers). `disable` symmetrically removes them.
//!
//! `install`'s local-path arm discovers + enables a pre-fetched plugin dir in
//! place — the only arm anything outside this crate's own tests calls. The
//! network arms (`Marketplace`/`Git`/`Mcpb`) deliberately return a
//! "use the CLI installer" error rather than fetching anything: production
//! installs a plugin exclusively through
//! `apps/cli/src/commands/plugin_install.rs`'s own git-clone / npm-install /
//! archive-download materialization, which writes the real V2
//! `installed_plugins.json` shape. An earlier revision of this module
//! duplicated that fetch machinery end-to-end (git clone, marketplace-catalog
//! resolution, `.mcpb` unpack, and its own `installed_plugins.json` writer)
//! with zero production callers; its writer used an INCOMPATIBLE
//! `{version, added}` record shape that would have corrupted the real file
//! had anything ever wired it up (spec §25d). It was deleted rather than
//! kept "just in case" once analysis showed every guard it uniquely had —
//! the marketplace cache-escape check — already has an equivalent in the
//! production path (see `marketplace_entry_source_path` in
//! `plugin_install.rs`).
//!
//! See spec §15.3.

use crate::lifecycle::PluginState;
use crate::loader::resolve_user_config;
use crate::manifest::{ComponentPath, PluginManifest, PluginUserConfig};
use crate::source::PluginSource;
use crate::strict_policy::StrictPluginOnlyPolicy;
use crate::theme_registry::PluginThemeRegistry;
use crate::user_config;
use serde_json::{Map, Value};

use command_api::CommandRegistry;
use hooks::{HookDefinition, HookExecutor, HookRegistry};
use lsp::LspRegistry;
use mcp::{McpRegistry, McpServerConfig};
use outputstyles::{OutputStyle, OutputStyleFrontmatter, OutputStyleRegistry, OutputStyleSource};
use protocol::PluginId;
use secret::CredentialManager;
use skill_api::{parse_skill_markdown, LoadedFrom, SkillRegistry, SkillSource};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use tool_api::ToolRegistry;
use traits::{FileSystem, HttpTransport, RuntimeSpawner};

/// Failure modes for [`PluginManager`] operations.
#[derive(Debug, Clone, Error)]
pub enum PluginManagerError {
    /// No plugin with this id is currently installed.
    #[error("plugin not found: {0}")]
    NotFound(PluginId),
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
/// registry the manager materialises into, and the credential manager (for
/// sensitive user-config values).
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
    /// Persisted non-sensitive `userConfig` state, keyed by plugin identity
    /// (`name@marketplace` for cache-installed plugins, bare `name` for local
    /// ones). Read from the settings `pluginConfigs` scope at construction (via
    /// [`Self::with_plugin_configs`]); empty otherwise. Sensitive values are
    /// NOT here — they come from [`CredentialManager`].
    plugin_configs: RwLock<HashMap<String, PluginUserConfig>>,
    /// Managed marketplace names blocked from add/install/enable. Later
    /// composition-root refreshes replace this set in place.
    blocked_marketplaces: RwLock<HashSet<String>>,

    // The 8 registries we materialize into:
    command_registry: Arc<RwLock<CommandRegistry>>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    hook_registry: Arc<RwLock<HookRegistry>>,
    output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
    mcp_registry: Arc<McpRegistry>,
    lsp_registry: Arc<LspRegistry>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    /// Optional live agent catalog owned by the host. When wired, plugin
    /// agents are materialized and removed by the same lifecycle transaction
    /// as every other plugin component.
    agent_catalog: Option<Arc<RwLock<Vec<agent::AgentDefinition>>>>,
    plugin_agent_names: RwLock<HashMap<PluginId, Vec<String>>>,
    // Channel registry is part of mcp_registry's agent-scoped pool in M1.
    /// Scoped MCP server names (`plugin:{plugin}:{server}`) each plugin seeded
    /// into `mcp_registry.connections`, so [`Self::unload_plugin`] can remove
    /// exactly those entries (the registry has no plugin-ownership index).
    plugin_mcp_names: RwLock<HashMap<PluginId, Vec<String>>>,
    /// Optional live plugin-workflow registry shared with `tool-workflow`'s
    /// resolver and `tasks::handlers::local_workflow`'s nested `workflow()`
    /// resolver (see [`workflow::PluginWorkflowRegistry`]). `None` until a
    /// composition root wires one via [`Self::with_plugin_workflows`] — every
    /// other component slot still materializes normally either way.
    plugin_workflows: Option<Arc<workflow::PluginWorkflowRegistry>>,
    /// Namespaced workflow names (`{plugin}:{name}`) each plugin seeded into
    /// [`Self::plugin_workflows`], so [`Self::unload_plugin`] can remove
    /// exactly those entries (mirrors [`Self::plugin_mcp_names`]).
    plugin_workflow_names: RwLock<HashMap<PluginId, Vec<String>>>,
    /// Live registry of plugin-declared custom themes (§14,
    /// [`theme_registry::PluginThemeRegistry`]). Unlike `plugin_workflows`,
    /// this port has no OTHER crate that needs to share the same `Arc` yet
    /// (no TUI theme-selection surface exists to consume it — see the
    /// module's deferral note), so `PluginManager` owns it directly rather
    /// than taking it as an optional externally-constructed dependency.
    plugin_themes: Arc<PluginThemeRegistry>,
    /// Namespaced theme slugs (`{plugin}:{name}`) each plugin seeded into
    /// [`Self::plugin_themes`], so [`Self::unload_plugin`] can remove exactly
    /// those entries (mirrors [`Self::plugin_workflow_names`]).
    plugin_theme_slugs: RwLock<HashMap<PluginId, Vec<String>>>,
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
        _strict: Arc<StrictPluginOnlyPolicy>,
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
            plugin_configs: RwLock::new(HashMap::new()),
            blocked_marketplaces: RwLock::new(HashSet::new()),
            command_registry,
            skill_registry,
            hook_registry,
            output_style_registry,
            mcp_registry,
            lsp_registry,
            tool_registry,
            agent_catalog: None,
            plugin_agent_names: RwLock::new(HashMap::new()),
            plugin_mcp_names: RwLock::new(HashMap::new()),
            plugin_workflows: None,
            plugin_workflow_names: RwLock::new(HashMap::new()),
            plugin_themes: Arc::new(PluginThemeRegistry::new()),
            plugin_theme_slugs: RwLock::new(HashMap::new()),
        }
    }

    /// Share the host's live agent catalog with the plugin lifecycle.
    #[must_use]
    pub fn with_agent_catalog(mut self, catalog: Arc<RwLock<Vec<agent::AgentDefinition>>>) -> Self {
        self.agent_catalog = Some(catalog);
        self
    }

    /// Share the host's live plugin-workflow registry with the plugin
    /// lifecycle. The SAME `Arc` must also be handed to
    /// `tool_workflow::WorkflowTool::with_plugin_workflows` and
    /// `tasks::handlers::local_workflow::LocalWorkflowHandler::with_plugin_workflows`
    /// so a plugin's saved workflow, once materialized here, is resolvable by
    /// name through the SAME `Workflow` tool call and `workflow()` nested-call
    /// path as a built-in/project/user workflow.
    #[must_use]
    pub fn with_plugin_workflows(
        mut self,
        registry: Arc<workflow::PluginWorkflowRegistry>,
    ) -> Self {
        self.plugin_workflows = Some(registry);
        self
    }

    /// Composition-test seam for confirming consumers share one live workflow
    /// registry allocation.
    #[doc(hidden)]
    #[must_use]
    pub fn shares_plugin_workflows(
        &self,
        registry: &Arc<workflow::PluginWorkflowRegistry>,
    ) -> bool {
        self.plugin_workflows
            .as_ref()
            .is_some_and(|wired| Arc::ptr_eq(wired, registry))
    }

    /// The live plugin-theme registry (§14, [`theme_registry::PluginThemeRegistry`]).
    /// `PluginManager` is the sole owner today (see the field doc), but hands
    /// out the same `Arc` so a future TUI theme-selection surface can read
    /// it without `PluginManager` growing an `Option`-wrapped setter the way
    /// [`Self::with_plugin_workflows`] needed for a registry built OUTSIDE
    /// this crate.
    ///
    /// ⚠️ **This registry has no production reader yet.** Nothing under
    /// `apps/`, `tui/`, `tui-core/`, or `traits/` calls this accessor or
    /// `theme_registry::resolve_theme`, and `tui_core::ThemeName::ALL` is a
    /// closed set of six palettes with no registry hook — so a plugin theme
    /// is registered and then unreachable. Unlike the workflow registry
    /// (which `engine-desktop::build` wires into four participants), the
    /// theme half CANNOT be made reachable by wiring alone: it needs the
    /// deferred TUI theme-selection feature (a `/theme` picker, a persisted
    /// choice, the oracle's `custom:` wire encoding `IW`/`Lb`, and a real
    /// user-theme store for `Aon`'s other half). Until then `load_plugin`'s
    /// theme block costs one stat + read + parse per declared file per
    /// enable, and can emit `[theme]` warnings for a feature the user cannot
    /// yet see.
    #[must_use]
    pub fn plugin_themes(&self) -> Arc<PluginThemeRegistry> {
        Arc::clone(&self.plugin_themes)
    }

    /// Seed the persisted `userConfig` state (settings `pluginConfigs`) the
    /// loader resolves non-sensitive values from. The composition root reads
    /// this from the active settings scope (see
    /// [`PluginUserConfig::from_settings_map`]) and threads it in; tests and
    /// callers with no persisted config leave it empty.
    #[must_use]
    pub fn with_plugin_configs(mut self, configs: HashMap<String, PluginUserConfig>) -> Self {
        self.plugin_configs = RwLock::new(configs);
        self
    }

    /// Seed the managed marketplace blocklist.
    #[must_use]
    pub fn with_blocked_marketplaces<I, S>(mut self, blocked: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.blocked_marketplaces = RwLock::new(blocked.into_iter().map(Into::into).collect());
        self
    }

    /// Replace the persisted plugin config map used by future loads/reloads.
    pub async fn replace_plugin_configs(&self, configs: HashMap<String, PluginUserConfig>) {
        *self.plugin_configs.write().await = configs;
    }

    /// Replace the managed blocked-marketplaces set used by future loads.
    pub async fn replace_blocked_marketplaces<I, S>(&self, blocked: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *self.blocked_marketplaces.write().await = blocked.into_iter().map(Into::into).collect();
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
                let discovered =
                    crate::discovery::discover_installed_plugins(path.parent().unwrap_or(&path))
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
            // Every arm below is a deliberate stub, not a placeholder for
            // future fetch machinery: production installs a plugin only
            // through `apps/cli/src/commands/plugin_install.rs`, which owns
            // the real git-clone / npm-install / archive-download
            // materialization AND the only writer of the real V2
            // `installed_plugins.json` shape. This method previously
            // duplicated that fetch machinery (see spec §25d) with zero
            // production callers and a writer that used an INCOMPATIBLE
            // record shape — deleted rather than kept, since every guard it
            // uniquely had already exists on the production path (see
            // `marketplace_entry_source_path` in `plugin_install.rs`).
            PluginSource::OfficialMarketplace { name } => Err(PluginManagerError::Io(format!(
                "install of '{name}' from the official marketplace requires the \
                 marketplace fetch loop (HTTP listing + signed-manifest download); \
                 not yet wired — install a pre-fetched plugin directory via \
                 PluginSource::LocalPath"
            ))),
            PluginSource::Marketplace { url, name } => {
                Err(PluginManagerError::Marketplace(format!(
                    "install of '{name}' from marketplace '{url}' is not supported by \
                     PluginManager::install; run the `plugin install` CLI command, which \
                     resolves marketplace sources through the production install pipeline"
                )))
            }
            PluginSource::Git { url, .. } => Err(PluginManagerError::Fetch(format!(
                "install of a plugin from git repository '{url}' is not supported by \
                 PluginManager::install; run the `plugin install` CLI command, which \
                 clones + materializes git plugin sources through the production install \
                 pipeline"
            ))),
            PluginSource::Mcpb { path, .. } => Err(PluginManagerError::Unpack(format!(
                "install of the .mcpb bundle at {} is not supported by \
                 PluginManager::install; run the `plugin install` CLI command instead",
                path.display()
            ))),
            PluginSource::BuiltIn => Err(PluginManagerError::Io(
                "BuiltIn plugins are compiled into the engine and are not \
                 installed via PluginManager::install"
                    .to_string(),
            )),
        }
    }

    /// Mark `id` as `Loaded` and inject its components into the engine
    /// registries.
    pub async fn enable(
        &self,
        id: &PluginId,
        manifest: PluginManifest,
        install_dir: PathBuf,
    ) -> Result<(), PluginManagerError> {
        if let Some(marketplace) = cache_marketplace_name(&install_dir) {
            if self
                .blocked_marketplaces
                .read()
                .await
                .contains(&marketplace)
            {
                return Err(PluginManagerError::Marketplace(format!(
                    "Marketplace '{marketplace}' is blocked by managed settings"
                )));
            }
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

    /// Register an engine-compiled-in plugin whose `(id, manifest,
    /// install_dir)` triple is already resolved and trusted — the second,
    /// narrow door §19.1 requires alongside [`Self::install`]'s continued
    /// rejection of `PluginSource::BuiltIn`.
    ///
    /// [`Self::install`] cannot take a `BuiltIn` source itself: the variant
    /// carries no path/URL for it to fetch from, and that arm keeps
    /// returning [`PluginManagerError::Io`] unconditionally — this method
    /// does not touch `install`'s match at all. Instead the caller (the
    /// composition root, for a plugin compiled into or embedded in the
    /// binary) supplies the manifest and the directory its component files
    /// already live in directly, exactly as `install`'s own `LocalPath` arm
    /// hands a caller-resolved directory straight to [`Self::enable`].
    ///
    /// This is a thin wrapper, not a parallel implementation: every
    /// remaining invariant `enable`/`load_plugin` enforce for a network
    /// install runs here too, unmodified —
    ///   - the blocklist check (`PluginManagerError::Blocked`) — with the
    ///     same caveat it carries on every other path:
    ///     `PluginBlocklist::is_blocked` matches on `PluginId`, and
    ///     `PluginId::new()` is a fresh v4 UUID per process, so the check
    ///     runs but can only match an id the host minted and blocked
    ///     within the same run,
    ///   - the managed-marketplace-block check,
    ///   - `userConfig` resolution (secrets + `pluginConfigs` substitution),
    ///   - and full component materialization: privilege-stripped agents,
    ///     commands, skills, output styles, hooks, MCP servers, LSP servers.
    ///
    /// Three things are stamped unconditionally, overwriting whatever the
    /// caller's `manifest` already carried, so a mistakenly-populated field
    /// on the way in can never survive into the live state:
    ///   - `manifest.source` becomes [`PluginSource::BuiltIn`],
    ///   - `manifest.trust_level` becomes the trust
    ///     [`crate::trust::default_trust_for_source`] assigns `BuiltIn`
    ///     (`AdminTrusted`) — mirroring [`Self::finalize_install`]'s
    ///     stamp-then-`enable` pattern for the network arms, and
    ///   - `manifest.id` becomes `*id`, the value the plugin state map is
    ///     keyed by. Every `install` arm gets `(id, manifest)` from
    ///     `discovery::load_plugin_from_path`, which mints one id and puts
    ///     that same value in `manifest.id`; only this door takes the two
    ///     from a caller that built the manifest by hand, so only here can
    ///     they diverge — and a divergence silently makes the plugin
    ///     impossible to unload (see the stamp's own comment below).
    ///
    /// What this path deliberately skips, because "verified" means the root
    /// never passed through untrusted, attacker-influenced input in the
    /// first place:
    ///   - fetch/download (git clone, marketplace HTTP, `.mcpb` unzip) — the
    ///     `BuiltIn` variant has nothing to fetch from,
    ///   - the `.mcpb` sha256 integrity compare — no archive is unpacked,
    ///   - the marketplace clone's symlink/path-traversal escape guard — the
    ///     directory is not a freshly-cloned untrusted repo,
    ///   - the `installed_plugins.json` durable record `copy_into_cache`
    ///     writes — a compiled-in plugin is deterministically reconstructed
    ///     by the host at every startup, so there is nothing to re-discover
    ///     across a restart, and
    ///   - the "`install_dir` is a real plugin root" check `install`'s
    ///     `LocalPath` arm gets for free from
    ///     `discovery::discover_installed_plugins` (a directory with no
    ///     `plugin.json` yields `no plugin manifest found at ..`). A
    ///     compiled-in plugin need not have a manifest file on disk at all —
    ///     the caller supplies the parsed manifest — so requiring one here
    ///     would defeat the purpose. The cost is that a host packaging bug
    ///     (component files missing from `install_dir`) registers a plugin
    ///     with zero components and returns `Ok`: `load_plugin` skips an
    ///     unreadable component file rather than failing, on this path
    ///     exactly as on every other.
    ///
    /// Not addressed by this method, and not unique to it: `enable`/
    /// `load_plugin` perform no `depends_on`/`dependencies` resolution for
    /// ANY install path today — a pre-existing gap this method inherits
    /// rather than introduces.
    pub async fn register_verified_builtin(
        &self,
        id: &PluginId,
        mut manifest: PluginManifest,
        install_dir: PathBuf,
    ) -> Result<(), PluginManagerError> {
        // `load_plugin` files every registry entry under `manifest.id`
        // (`register_plugin_commands(manifest.id, ..)`, `plugin_mcp_names`,
        // `plugin_agent_names`, …) while `disable`/`unload_plugin` remove
        // them by the `id` argument this state map is keyed by. Every
        // `install` arm gets both from `discovery::load_plugin_from_path`,
        // which mints ONE id and stores that same value in `manifest.id`, so
        // they cannot diverge there. Here the caller hand-builds the manifest
        // and passes the id separately, so they can — and a divergence is
        // silent: registration succeeds, then `disable` reports success while
        // leaving the plugin's commands, hooks, agents, MCP and LSP servers
        // live and permanently unreachable. Stamp it, exactly like `source`
        // and `trust_level` below.
        manifest.id = *id;
        manifest.source = PluginSource::BuiltIn;
        manifest.trust_level = crate::trust::default_trust_for_source(&PluginSource::BuiltIn);
        self.enable(id, manifest, install_dir).await
    }

    /// Transition `id` from `Loaded` to `Disabled` and remove every
    /// registry entry the plugin contributed.
    pub async fn disable(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        let state = self.plugins.read().await.get(id).cloned();
        if let Some((manifest, install_dir)) = match state {
            Some(PluginState::Loaded {
                manifest,
                install_dir,
                ..
            })
            | Some(PluginState::DisablingFailed {
                manifest,
                install_dir,
                ..
            }) => Some((manifest, install_dir)),
            _ => None,
        } {
            if let Err(error) = self.unload_plugin(id).await {
                self.plugins.write().await.insert(
                    *id,
                    PluginState::DisablingFailed {
                        manifest,
                        install_dir,
                        error: error.to_string(),
                    },
                );
                return Err(error);
            }
            self.plugins.write().await.insert(
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

    /// The ids of every plugin currently in the `Loaded` state (its components
    /// are live in the engine registries). A `DisablingFailed` plugin is kept
    /// in this set so reload/disable passes continue converging its partial
    /// unload instead of silently treating it as already disabled.
    pub async fn loaded_plugin_ids(&self) -> Vec<PluginId> {
        self.plugins
            .read()
            .await
            .iter()
            .filter_map(|(id, state)| {
                matches!(
                    state,
                    PluginState::Loaded { .. } | PluginState::DisablingFailed { .. }
                )
                .then_some(*id)
            })
            .collect()
    }

    /// Snapshot one plugin's lifecycle state. Builtin hosts use this to
    /// distinguish an explicitly present `Disabled` plugin from a bundle that
    /// failed before registration and is therefore absent.
    pub async fn plugin_state(&self, id: &PluginId) -> Option<PluginState> {
        self.plugins.read().await.get(id).cloned()
    }

    /// Materialise `manifest`'s components into the 8 registries.
    #[allow(clippy::too_many_lines)] // Wiring layer — validate-then-mutate over 7 component slots.
    async fn load_plugin(
        &self,
        manifest: &PluginManifest,
        install_dir: &Path,
    ) -> Result<(), PluginManagerError> {
        // Resolve the plugin's `userConfig` into the `${user_config.KEY}`
        // substitution map: non-sensitive values from the settings
        // `pluginConfigs[plugin].options` scope, sensitive values live from
        // secure storage (see `resolve_user_config`). The result is CONSUMED
        // below (substituted into the plugin's MCP server configs), not dropped.
        //
        // The plugin identity used for both the secret namespace and the
        // pluginConfigs lookup is the installed `name@marketplace` id for
        // cache-installed plugins, else the bare manifest name.
        let plugin_key = installed_plugin_identity(manifest, install_dir);
        let options = self
            .plugin_configs
            .read()
            .await
            .get(&plugin_key)
            .map(|c| c.options.clone())
            .unwrap_or_default();
        let user_config = resolve_user_config(manifest, &plugin_key, &options, &self.credentials)
            .await
            .map_err(|e| PluginManagerError::Loader(e.to_string()))?;
        // The substitution context keyed by the bare field name.
        let subst_ctx: Map<String, Value> = match user_config {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        // Create the writable plugin-data directory before mutating any live
        // registry. A filesystem failure must not leave a partially loaded
        // plugin behind.
        let plugin_data_dir = if manifest.components.lsp_servers.is_empty() {
            None
        } else {
            Some(ensure_plugin_data_dir(&self.install_dir, &plugin_key).await?)
        };

        // All-or-nothing ordering: VALIDATE every fallible input BEFORE
        // mutating any live registry, so a rejected plugin never leaves an
        // orphaned command / hook behind. claude-code loads a plugin as a
        // single unit. NOTE: a privilege-escalating AGENT is no longer one of
        // those fallible inputs — per §19.1 it is warned about and stripped
        // (see the agent loop directly below), never rejected. The
        // all-or-nothing ordering still governs every other component.

        // (a) Agents — scan the privilege boundary, then parse the same
        //     declared paths discovery returned (including custom/nested agent
        //     directories). Names are plugin-qualified so they cannot shadow a
        //     user/project agent. Registry mutation remains below the complete
        //     validation phase.
        //
        //     §19.1: `permissionMode` / `mcpServers` / `hooks` in a plugin
        //     agent's frontmatter must never reach the agent's runtime
        //     execution state, and all three are handled IDENTICALLY now.
        //     This is *normal* validation (see
        //     `agent_validation::scan_plugin_agent_privileged_fields`): it
        //     only WARNS per privileged field found, never fails — a
        //     malformed or over-privileged agent file must not remove an
        //     otherwise-valid agent from the registry, and must never take
        //     the whole plugin down with it. (Previously `permissionMode` /
        //     `hooks` failed the ENTIRE plugin load via
        //     `validate_plugin_agent_frontmatter`, now the STRICT validator
        //     — unused on this path — while `mcpServers` alone degraded
        //     silently. That inconsistency was the bug.) The three fields
        //     are then unconditionally stripped from `def` below too,
        //     independent of what the scan found, so a future parser change
        //     (a new alias, a nested-field promotion, …) can't smuggle
        //     privilege through a gap in the scan.
        let mut agent_defs = Vec::new();
        for ap in &manifest.components.agents {
            let abs = if ap.path.is_absolute() {
                ap.path.clone()
            } else {
                install_dir.join(&ap.path)
            };
            if let Ok(raw) = tokio::fs::read_to_string(&abs).await {
                if let Some(yaml) = extract_frontmatter(&raw) {
                    match crate::agent_validation::scan_plugin_agent_privileged_fields(yaml) {
                        Ok(fields) => {
                            for field in fields {
                                tracing::warn!(
                                    path = %abs.display(),
                                    plugin = %manifest.name,
                                    field = field.key(),
                                    "plugin agent frontmatter declares a privileged field; \
                                     stripping it before load — it will not reach execution state"
                                );
                            }
                        }
                        // Frontmatter that will not parse as a YAML mapping
                        // cannot be scanned — but it also cannot be parsed
                        // into an `AgentDefinition` by
                        // `parse_agent_markdown` two lines below (its
                        // `Frontmatter` target is strictly less permissive
                        // than `serde_yaml::Value`), so nothing from this
                        // file can reach execution state. Failing the load
                        // here would take the WHOLE plugin down over one
                        // unparseable agent file, which §19.1 forbids: warn
                        // and let the parse below drop just this agent.
                        Err(e) => {
                            tracing::warn!(
                                path = %abs.display(),
                                plugin = %manifest.name,
                                error = %e,
                                "plugin agent frontmatter could not be scanned for privileged \
                                 fields; skipping this agent (the plugin still loads)"
                            );
                        }
                    }
                }
                match agent::parse_agent_markdown(
                    &raw,
                    agent::AgentSource::Plugin,
                    component_root(ap, install_dir.join("agents")),
                    &abs,
                ) {
                    Ok(mut def) => {
                        // §19.1 defense in depth: strip regardless of what the
                        // scan above found (see the loop comment).
                        def.permission_mode = agent::AgentPermissionMode::Bubble;
                        def.mcp_servers.clear();
                        def.frontmatter_hooks.clear();
                        let root = component_root(ap, install_dir.join("agents"));
                        let namespace = abs
                            .parent()
                            .and_then(|parent| parent.strip_prefix(&root).ok())
                            .map(|relative| {
                                relative
                                    .components()
                                    .filter_map(|part| match part {
                                        std::path::Component::Normal(value) => {
                                            Some(value.to_string_lossy().into_owned())
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let mut parts = Vec::with_capacity(namespace.len() + 2);
                        parts.push(manifest.name.clone());
                        parts.extend(namespace);
                        parts.push(def.agent_type);
                        def.agent_type = parts.join(":");
                        if let Some(prompt) = def.system_prompt.take() {
                            def.system_prompt =
                                Some(user_config::substitute_string_field(&prompt, &subst_ctx));
                        }
                        agent_defs.push(def);
                    }
                    Err(agent::AgentLoadError::MissingName(_)) => {}
                    // Invalid name (leading `-` / `:` namespacing collision):
                    // the byte-exact claude error was already logged inside
                    // `parse_agent_markdown` — skip without double-logging.
                    Err(agent::AgentLoadError::InvalidName(_)) => {}
                    Err(error) => tracing::warn!(
                        path = %abs.display(),
                        error = %error,
                        "skipping malformed plugin agent"
                    ),
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
        {
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
                    component_root(cp, install_dir.join("commands")),
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
        {
            let mut sources = Vec::new();
            for sp in &manifest.components.skills {
                let abs = if sp.path.is_absolute() {
                    sp.path.clone()
                } else {
                    install_dir.join(&sp.path)
                };
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                let Ok(skill) = parse_skill_markdown(
                    &raw,
                    abs.clone(),
                    SkillSource::Plugin,
                    LoadedFrom::Plugin,
                ) else {
                    continue;
                };
                sources.push((abs, raw, skill.name));
            }
            let bare_names: HashSet<String> =
                sources.iter().map(|(_, _, name)| name.clone()).collect();
            for (abs, raw, _) in sources {
                // Plugin-authored handoffs commonly use `$other-skill` in the
                // checked-in Markdown. Keep the package bytes source-faithful,
                // but qualify references to sibling skills in the live prompt
                // so the Skill tool cannot miss or resolve a same-name project
                // skill outside this plugin.
                let scoped_raw = scope_plugin_skill_references(&raw, plugin_name, &bare_names);
                let Ok(mut skill) = parse_skill_markdown(
                    &scoped_raw,
                    abs.clone(),
                    SkillSource::Plugin,
                    LoadedFrom::Plugin,
                ) else {
                    continue;
                };
                skill.name = format!("{plugin_name}:{}", skill.name);
                skill.plugin_id = Some(manifest.id);

                // Plugin skills are prompt commands in Claude Code. Register
                // the same parsed file in the shared slash-command catalog so
                // the Skill tool, model skill listing, completion UI, and
                // direct `/plugin:skill` invocation all observe it.
                let skill_root = abs.parent().unwrap_or(install_dir).to_path_buf();
                let file = command_api::parse_skill_command_markdown(
                    &scoped_raw,
                    abs.clone(),
                    skill_root,
                    command_api::CommandSource::Plugin,
                );
                let base =
                    command_api::build_skill_command(&file, command_api::CommandSource::Plugin);
                let (frontmatter, prompt_template) = match &base.kind {
                    command_api::SlashCommandKind::Markdown {
                        frontmatter,
                        prompt_template,
                        ..
                    } => (frontmatter.clone(), prompt_template.clone()),
                    _ => (
                        command_api::CommandFrontmatter::default(),
                        file.content.clone(),
                    ),
                };
                cmds.push(command_api::SlashCommand {
                    name: skill.name.clone(),
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
                skills.push(skill);
            }
        }

        // (d) Output styles — read each `output-styles/*.md`, parse the body as
        //     the system-prompt addendum, namespace the name `{plugin}:{name}`
        //     (`loadPluginOutputStyles.ts:55`). The body becomes
        //     `system_prompt_addendum` (TS `prompt: markdownContent.trim()`).
        let mut styles: Vec<OutputStyle> = Vec::new();
        {
            for op in &manifest.components.output_styles {
                let abs = if op.path.is_absolute() {
                    op.path.clone()
                } else {
                    install_dir.join(&op.path)
                };
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                let disk = outputstyles::parse_output_style(&raw, stem);
                let name = format!("{plugin_name}:{}", disk.name);
                styles.push(OutputStyle {
                    name: name.clone(),
                    description: disk.description.clone(),
                    source: OutputStyleSource::Plugin,
                    frontmatter: OutputStyleFrontmatter {
                        name,
                        description: disk.description,
                        keep_coding_instructions: disk.keep_coding_instructions,
                        force_for_plugin: disk.force_for_plugin,
                        ..Default::default()
                    },
                    system_prompt_addendum: disk.prompt,
                    source_path: Some(abs),
                });
            }
        }

        // (d2) Workflows — read each declared/auto-scanned `.js` file
        //      (`manifest.components.workflows`, §14), extract its own
        //      `meta.name` and namespace `{plugin}:{name}` (oracle plugin-
        //      workflow loader `v()` @169045500: `${pluginName}:${meta.name}`
        //      — the SAME "parse the component's own declared name" rule (d)
        //      applies to output styles, here reading the name from the
        //      script's `export const meta = {…}` block instead of
        //      frontmatter).
        //
        //      `v()` gates a file THREE ways before it may join the table,
        //      and every gate DROPS the file rather than falling back:
        //        `let e = await ZI(c,o,um); if (e===null) return
        //           warn(`Plugin workflow ${o}: not a regular file or exceeds
        //           ${um} bytes — skipping`), null;`
        //        `let r = bf(e,{validateBody:!1}); if ("error" in r) return
        //           warn(`Plugin workflow ${o} has invalid meta: ${r.error}
        //           — skipping`), null;`
        //      There is NO filename fallback on either branch: a shared
        //      helper module dropped in `workflows/` (no `export const meta`)
        //      is simply not a workflow. Registering it under its file stem
        //      would put a name in the `Workflow` tool's `Available:` list
        //      that then dies at `workflow::validate_meta` inside the
        //      launcher — an accept-then-fail the oracle never produces —
        //      so the port applies the same three gates.
        //      `workflow::validate_meta` is this port's `bf(…,{validateBody:
        //      !1})`: it parses the `meta` block only (first-statement, pure
        //      literal, non-empty `name`/`description`) and is the very gate
        //      the launcher already runs, so nothing can pass here and fail
        //      there.
        //
        //      Only collected when a registry is actually wired — the common
        //      case (no composition root has called `with_plugin_workflows`
        //      yet) does zero extra file I/O.
        let mut workflow_entries: Vec<workflow::PluginWorkflowEntry> = Vec::new();
        if self.plugin_workflows.is_some() {
            for wp in &manifest.components.workflows {
                let abs = if wp.path.is_absolute() {
                    wp.path.clone()
                } else {
                    install_dir.join(&wp.path)
                };
                // Oracle `ZI(c,o,um)`: regular file (or a symlink resolving to
                // one — `tokio::fs::metadata` follows links, matching `_()`'s
                // `isFile()||isSymbolicLink()` readdir filter) AND at most
                // `um` = 524288 bytes.
                let Ok(metadata) = tokio::fs::metadata(&abs).await else {
                    continue;
                };
                if !metadata.is_file() || metadata.len() > workflow::MAX_WORKFLOW_SCRIPT_BYTES {
                    tracing::warn!(
                        path = %abs.display(),
                        "Plugin workflow {}: not a regular file or exceeds {} bytes — skipping",
                        abs.display(),
                        workflow::MAX_WORKFLOW_SCRIPT_BYTES
                    );
                    continue;
                }
                let Ok(raw) = tokio::fs::read_to_string(&abs).await else {
                    continue;
                };
                if let Err(e) = workflow::validate_meta(&raw) {
                    tracing::warn!(
                        path = %abs.display(),
                        "Plugin workflow {} has invalid meta: {e} — skipping",
                        abs.display()
                    );
                    continue;
                }
                // `validate_meta` already proved `meta.name` is a non-empty
                // string literal, so this cannot fall through in practice; a
                // `None` here would be a parser disagreement, and dropping
                // the file is the oracle-shaped outcome either way.
                let Some(base_name) =
                    workflow::meta_string_value(&raw, "name").filter(|value| !value.is_empty())
                else {
                    continue;
                };
                workflow_entries.push(workflow::PluginWorkflowEntry {
                    name: format!("{plugin_name}:{base_name}"),
                    script_path: abs,
                });
            }
        }

        // (d3) Themes — read each declared/auto-scanned `.json` file
        //      (`manifest.components.themes`, §14), validate it the way the
        //      oracle's `j(e,t,r)` does (256KB size cap checked BEFORE the
        //      read, JSON validity, `base`/`name`/`overrides` shape — see
        //      `theme_registry`'s module doc for the one place this port's
        //      validation is thinner than the oracle's), and namespace
        //      `{plugin}:{basename}` (oracle `w0e`: `H=${P.name}:` + the
        //      file's basename minus `.json` — the SAME namespacing rule (c)
        //      applies to skills/output-styles/workflows). A file that
        //      cannot be stat'd/read is skipped without a warning (the
        //      common oracle case, a file discovery already verified exists
        //      going missing between discovery and enable); oversized or
        //      invalid-JSON files ARE warned, matching the oracle's two
        //      warning sites.
        let mut theme_paths: Vec<(String, std::path::PathBuf)> = Vec::new();
        for tp in &manifest.components.themes {
            let abs = if tp.path.is_absolute() {
                tp.path.clone()
            } else {
                install_dir.join(&tp.path)
            };
            let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            theme_paths.push((format!("{plugin_name}:{stem}"), abs));
        }

        // (e) MCP servers — scope each `.mcp.json` entry as
        //     `plugin:{plugin}:{server}` so it is keyed identically to a
        //     normal configured server (`addPluginScopeToServers`,
        //     `mcpPluginIntegration.ts:341-359`, uses scope `dynamic`; the
        //     discovery loader already stamps `ConfigScope::Dynamic`). These
        //     scoped configs are connected at enable time through the SAME
        //     `McpRegistry::connect_all` path the engine uses for configured
        //     `.mcp.json` servers (claude-code `getLingXiMcpConfigs`
        //     merges plugin servers into the SAME configs map that the
        //     connection manager dials eagerly at startup — `config.ts:1114`).
        // A strict-plugin-only MCP lock rejects non-plugin sources; this code is
        // the trusted plugin materialization path and therefore remains
        // eligible under the lock.
        let mcp_scoped: Vec<McpServerConfig> = manifest
            .components
            .mcp_servers
            .values()
            .filter_map(|cfg| {
                let scoped_name = format!("plugin:{plugin_name}:{}", cfg.name);
                // Deferred-substitution-site gate (claude `mcp-config-invalid`):
                // a stdio `command` (the shell-executed field) referencing
                // `${user_config.*}` would pass the substituted value to a
                // shell — reject the server (byte-faithful msg). `args` / `env`
                // ARE safe to substitute (discrete argv / env block), so only
                // the `command` field is gated.
                if let traits::McpTransportSpec::Stdio { command, .. } = &cfg.spec {
                    if user_config::references_user_config(command) {
                        tracing::warn!(
                            "{}",
                            user_config::mcp_stdio_reference_rejection(&scoped_name)
                        );
                        return None;
                    }
                }
                let mut scoped = cfg.clone();
                scoped.name = scoped_name;
                // Substitute `${user_config.KEY}` references (command / args
                // / env, and remote url / headers) with the resolved values
                // — the primary consumption path for a plugin's userConfig.
                substitute_mcp_config(&mut scoped, &subst_ctx);
                Some(scoped)
            })
            .collect();

        // ---- All inputs validated; mutate the live registries now. ----

        // 1. Commands and prompt skills share the live command catalog.
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

        // 4. Hooks — apply the resolved userConfig to each Command hook before
        //    registering: inject `LINGXI_PLUGIN_OPTION_<KEY>` into the child env
        //    (claude `CLAUDE_PLUGIN_OPTION_${ye}`), substitute `${user_config.*}`
        //    into exec-form command/args, and REJECT (skip) a shell-form command
        //    that references `${user_config.*}` (the shell would re-parse the
        //    substituted value — an injection hazard). A no-op when the plugin
        //    has no userConfig (`subst_ctx` empty), leaving hooks byte-unchanged.
        let plugin_hooks =
            apply_user_config_to_hooks(plugin_name, &manifest.components.hooks, &subst_ctx);
        self.hook_registry
            .write()
            .await
            .register_plugin_hooks(manifest.id, plugin_hooks);

        // 5. OutputStyles.
        if !styles.is_empty() {
            self.output_style_registry
                .write()
                .await
                .register_plugin_styles(manifest.id, styles);
        }

        // 6. Agents. Their names are plugin-qualified, so removal cannot
        // delete a built-in/user/project agent with the same local name.
        if let Some(catalog) = &self.agent_catalog {
            let names: Vec<String> = agent_defs
                .iter()
                .map(|definition| definition.agent_type.clone())
                .collect();
            if !names.is_empty() {
                let mut live = catalog.write().await;
                for definition in agent_defs {
                    if let Some(existing) = live
                        .iter_mut()
                        .find(|entry| entry.agent_type == definition.agent_type)
                    {
                        *existing = definition;
                    } else {
                        live.push(definition);
                    }
                }
                self.plugin_agent_names
                    .write()
                    .await
                    .insert(manifest.id, names);
            }
        }
        let _ = &self.tool_registry;

        // 7. MCP servers — route each scoped config through the registry's
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
            for name in &names {
                self.mcp_registry
                    .set_headers_helper_plugin_root(name.clone(), install_dir.to_path_buf())
                    .await;
            }
            self.plugin_mcp_names
                .write()
                .await
                .insert(manifest.id, names);
            self.mcp_registry.connect_all(mcp_scoped).await;
        }

        // 8. LSP servers — plugin-only registration path.
        //
        // `LspRegistry::register_plugin_servers` is the ONLY supported way
        // to register LSP servers. The internal `register_config` is
        // `pub(crate)` so user/project settings cannot bypass this gate.
        // Matches claude-code's `getAllLspServers()`
        // (`claude-code/src/services/lsp/config.ts`).
        let configs: Vec<_> = manifest
            .components
            .lsp_servers
            .iter()
            .map(|(local_name, cfg)| {
                let mut scoped = cfg.clone();
                scoped.name = format!("plugin:{plugin_name}:{local_name}");
                substitute_lsp_config(
                    &mut scoped,
                    &subst_ctx,
                    install_dir,
                    plugin_data_dir.as_deref(),
                );
                scoped
            })
            .collect();
        self.lsp_registry
            .register_plugin_servers(manifest.id, configs)
            .await;

        // 9. Workflows — join the saved-workflow search path (§14). Remember
        //    the namespaced names FIRST so `unload_plugin` can remove exactly
        //    these entries regardless of what else the registry holds.
        if let Some(registry) = &self.plugin_workflows {
            if !workflow_entries.is_empty() {
                let names: Vec<String> = workflow_entries.iter().map(|e| e.name.clone()).collect();
                registry.register(workflow_entries);
                self.plugin_workflow_names
                    .write()
                    .await
                    .insert(manifest.id, names);
            }
        }

        // 10. Themes — join the live plugin-theme registry (§14). Remember
        //     the namespaced slugs FIRST so `unload_plugin` can remove
        //     exactly these entries regardless of what else the registry
        //     holds.
        //     Loading is LAZY: only the (slug, path) pairs are recorded here;
        //     the stat + read + parse happen on the registry's first `get` for
        //     a slug. Nothing reads the registry yet, so eager loading would
        //     spend I/O and emit `[theme]` warnings for an invisible feature.
        if !theme_paths.is_empty() {
            let slugs: Vec<String> = theme_paths.iter().map(|(slug, _)| slug.clone()).collect();
            self.plugin_themes.register_paths(theme_paths);
            self.plugin_theme_slugs
                .write()
                .await
                .insert(manifest.id, slugs);
        }

        Ok(())
    }

    /// Symmetric unload — clean up the exact registries we touched.
    async fn unload_plugin(&self, id: &PluginId) -> Result<(), PluginManagerError> {
        // MCP teardown is the only fallible unload step. Run it first so a
        // transport-disconnect failure leaves every other materialized surface
        // intact and retryable.
        let tracked_mcp_names = self.plugin_mcp_names.read().await.get(id).cloned();
        if let Some(names) = tracked_mcp_names {
            let mut remaining = Vec::new();
            for (idx, name) in names.iter().enumerate() {
                match self.mcp_registry.remove_without_revoking_auth(name).await {
                    Ok(()) => {
                        self.mcp_registry
                            .remove_headers_helper_plugin_root(name)
                            .await;
                    }
                    Err(error) => {
                        remaining.extend(names[idx..].iter().cloned());
                        self.plugin_mcp_names.write().await.insert(*id, remaining);
                        return Err(PluginManagerError::Io(format!(
                            "failed to unload plugin MCP server {name}: {error}"
                        )));
                    }
                }
            }
            self.plugin_mcp_names.write().await.remove(id);
        }

        self.command_registry.write().await.unregister_plugin(id);
        self.skill_registry.write().await.unregister_plugin(id);
        self.hook_registry.write().await.unregister_plugin(id);
        self.output_style_registry
            .write()
            .await
            .unregister_plugin(id);
        self.tool_registry.write().await.unregister_plugin(id);
        if let Some(names) = self.plugin_agent_names.write().await.remove(id) {
            if let Some(catalog) = &self.agent_catalog {
                catalog.write().await.retain(|definition| {
                    !names.contains(&definition.agent_type)
                        || definition.source != agent::AgentSource::Plugin
                });
            }
        }
        let _ = self.lsp_registry.unregister_plugin(id).await;
        // Workflow cleanup: remove exactly the namespaced entries this
        // plugin seeded into the shared registry.
        if let Some(names) = self.plugin_workflow_names.write().await.remove(id) {
            if let Some(registry) = &self.plugin_workflows {
                registry.unregister(&names);
            }
        }
        // Theme cleanup: remove exactly the namespaced slugs this plugin
        // seeded into the plugin-theme registry.
        if let Some(slugs) = self.plugin_theme_slugs.write().await.remove(id) {
            self.plugin_themes.unregister(&slugs);
        }
        Ok(())
    }
}

fn scope_plugin_skill_references(
    source: &str,
    plugin_name: &str,
    bare_names: &HashSet<String>,
) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for (dollar, _) in source.match_indices('$') {
        if dollar < cursor {
            continue;
        }
        let token_start = dollar + 1;
        let token_len = source[token_start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .map(char::len_utf8)
            .sum::<usize>();
        let token_end = token_start + token_len;
        let token = &source[token_start..token_end];
        if !token.is_empty() && bare_names.contains(token) {
            output.push_str(&source[cursor..dollar]);
            output.push('$');
            output.push_str(plugin_name);
            output.push(':');
            output.push_str(token);
            cursor = token_end;
        }
    }
    output.push_str(&source[cursor..]);
    output
}

#[cfg(test)]
mod plugin_skill_reference_tests {
    use super::*;

    #[test]
    fn sibling_skill_references_are_qualified_without_rewriting_other_tokens() {
        let names = HashSet::from(["frontend-design".to_string(), "device".to_string()]);
        let source =
            "Use $frontend-design, $device and $ARGUMENTS. Keep $other and $plugin:device.";
        assert_eq!(
            scope_plugin_skill_references(source, "lingxi-local-app", &names),
            "Use $lingxi-local-app:frontend-design, $lingxi-local-app:device and \
             $ARGUMENTS. Keep $other and $plugin:device."
        );
    }
}

fn component_root(component: &ComponentPath, fallback: PathBuf) -> PathBuf {
    component
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("root"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or(fallback)
}

/// Substitute `${user_config.KEY}` references into an MCP server config's
/// transport spec, in place. Covers the substitutable string surfaces: the
/// Stdio `command` / `args` / `env` values, and remote (`Sse` / `Http` /
/// `WebSocket`) `url` + `headers` values. Non-substitutable specs (`InProcess`,
/// `SseIde`, `WsIde`, `SdkControl`) carry no userConfig-derived string and are
/// left untouched. A no-op when the substitution context is empty (the common
/// no-userConfig case), so a plugin without userConfig is byte-unchanged.
fn substitute_mcp_config(cfg: &mut McpServerConfig, ctx: &Map<String, Value>) {
    use traits::McpTransportSpec;
    if ctx.is_empty() {
        return;
    }
    match &mut cfg.spec {
        McpTransportSpec::Stdio { command, args, env } => {
            *command = user_config::substitute_string_field(command, ctx);
            *args = user_config::substitute_args(args, ctx);
            for v in env.values_mut() {
                *v = user_config::substitute_string_field(v, ctx);
            }
        }
        McpTransportSpec::Sse { url, headers, .. } => {
            *url = user_config::substitute_string_field(url, ctx);
            for v in headers.values_mut() {
                *v = user_config::substitute_string_field(v, ctx);
            }
        }
        McpTransportSpec::Http { url, headers, .. } => {
            *url = user_config::substitute_string_field(url, ctx);
            for v in headers.values_mut() {
                *v = user_config::substitute_string_field(v, ctx);
            }
        }
        McpTransportSpec::WebSocket { url, headers, .. } => {
            *url = user_config::substitute_string_field(url, ctx);
            for v in headers.values_mut() {
                *v = user_config::substitute_string_field(v, ctx);
            }
        }
        McpTransportSpec::InProcess { .. }
        | McpTransportSpec::SseIde { .. }
        | McpTransportSpec::WsIde { .. }
        | McpTransportSpec::SdkControl { .. } => {}
    }
}

/// Apply the public plugin-LSP substitution pass before registration.
///
/// Claude Code expands the plugin root, resolved `${user_config.KEY}` values,
/// and ordinary `${ENV_VAR}` references across command/args/env/workspaceFolder,
/// then injects the host paths into the server environment.  Keep the Claude
/// names for third-party plugin compatibility and the LingXi aliases for local
/// plugins authored against this port.
fn substitute_lsp_config(
    cfg: &mut traits::LspServerConfig,
    ctx: &Map<String, Value>,
    install_dir: &Path,
    plugin_data_dir: Option<&Path>,
) {
    let plugin_root = install_dir.to_string_lossy().into_owned();
    let plugin_data = plugin_data_dir
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_dir = project_dir.to_string_lossy().into_owned();
    let expand = |value: &str| {
        let value = value
            .replace("${CLAUDE_PLUGIN_ROOT}", &plugin_root)
            .replace("${LINGXI_PLUGIN_ROOT}", &plugin_root)
            .replace("${CLAUDE_PLUGIN_DATA}", &plugin_data)
            .replace("${LINGXI_PLUGIN_DATA}", &plugin_data)
            .replace("${CLAUDE_PROJECT_DIR}", &project_dir)
            .replace("${LINGXI_PROJECT_DIR}", &project_dir);
        let value = user_config::substitute_string_field(&value, ctx);
        mcp::expand_env_vars_in_string(&value).expanded
    };

    cfg.command = expand(&cfg.command);
    cfg.args = cfg.args.iter().map(|arg| expand(arg)).collect();
    for value in cfg.env.values_mut() {
        *value = expand(value);
    }
    if let Some(workspace_folder) = &mut cfg.workspace_folder {
        *workspace_folder = expand(workspace_folder);
    }
    cfg.env
        .entry("CLAUDE_PLUGIN_ROOT".to_string())
        .or_insert_with(|| plugin_root.clone());
    cfg.env
        .entry("LINGXI_PLUGIN_ROOT".to_string())
        .or_insert_with(|| plugin_root.clone());
    cfg.env
        .entry("CLAUDE_PLUGIN_DATA".to_string())
        .or_insert_with(|| plugin_data.clone());
    cfg.env
        .entry("LINGXI_PLUGIN_DATA".to_string())
        .or_insert_with(|| plugin_data.clone());
    cfg.env
        .entry("CLAUDE_PROJECT_DIR".to_string())
        .or_insert_with(|| project_dir.clone());
    cfg.env
        .entry("LINGXI_PROJECT_DIR".to_string())
        .or_insert_with(|| project_dir);
}

/// Apply a plugin's resolved `userConfig` (`ctx`, keyed by bare field name) to
/// its hook definitions before registration, mirroring claude-code 2.1.207:
///
/// * every **Command** hook's child env gains a `LINGXI_PLUGIN_OPTION_<KEY>`
///   entry per userConfig field (claude `CLAUDE_PLUGIN_OPTION_${ye}=String(me)`,
///   with the established `LINGXI_` prefix — cf. `LINGXI_PLUGIN_ROOT`), so a hook
///   script can read a value (including a sensitive one) from its environment
///   without it ever touching a command line;
/// * an **exec-form** Command hook (with discrete `args`) has `${user_config.*}`
///   substituted into its `command` + `args` (each arg a discrete argv element —
///   no shell re-parse);
/// * a **shell-form** Command hook (a bare `command`, no `args`) that references
///   `${user_config.*}` is REJECTED (skipped) — the substituted value would be
///   re-parsed by the shell (`user_config::shell_form_reference_rejection`).
///
/// Non-Command hooks (Http / Agent / Prompt / Builtin) carry no subprocess env
/// and are passed through unchanged. A no-op (verbatim clone) when `ctx` is empty
/// — the common no-userConfig case stays byte-identical.
fn apply_user_config_to_hooks(
    plugin_name: &str,
    hooks: &[HookDefinition],
    ctx: &Map<String, Value>,
) -> Vec<HookDefinition> {
    if ctx.is_empty() {
        return hooks.to_vec();
    }
    let mut out = Vec::with_capacity(hooks.len());
    for hook in hooks {
        let HookExecutor::Command {
            command,
            args,
            env,
            cwd,
            shell,
        } = &hook.executor
        else {
            out.push(hook.clone());
            continue;
        };
        // Shell-form = a bare command with no discrete exec-form args. Rejecting
        // it (skip) is the safety gate: a substituted secret would hit the shell.
        // The rejection names the OWNING PLUGIN (claude `Hook from plugin ${c}`),
        // not the hook.
        if args.is_empty() && user_config::references_user_config(command) {
            tracing::warn!(
                "{}",
                user_config::shell_form_reference_rejection(plugin_name, command)
            );
            continue;
        }
        // Exec-form (and no-ref shell-form, a no-op): substitute the discrete
        // command + args, then inject the plugin-option env vars.
        let new_command = user_config::substitute_string_field(command, ctx);
        let new_args = user_config::substitute_args(args, ctx);
        let mut new_env = env.clone();
        for (key, value) in ctx {
            new_env.insert(
                user_config::option_env_var(key),
                user_config::value_to_env_string(value),
            );
        }
        let mut hook = hook.clone();
        hook.executor = HookExecutor::Command {
            command: new_command,
            args: new_args,
            env: new_env,
            cwd: cwd.clone(),
            // The `shell` selector is config metadata, not a substitution
            // target — carry it through the user-config rewrite unchanged.
            shell: *shell,
        };
        out.push(hook);
    }
    out
}

/// Extract the YAML frontmatter block (between leading `---` fences) of a
/// markdown agent file, if present. Returns `None` when the file has no
/// frontmatter. Mirrors the `---\n…\n---` convention claude-code's agent
/// loader uses (and the engine's `parse_agent_markdown`).
fn extract_frontmatter(raw: &str) -> Option<&str> {
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    // Find the closing fence at the start of a line.
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    Some(&rest[..end])
}

/// Installed plugin identity used for `pluginConfigs` and plugin-secret
/// namespaces: `name@marketplace` for cache-installed plugins, else bare
/// `manifest.name`.
fn installed_plugin_identity(manifest: &PluginManifest, install_dir: &Path) -> String {
    match cache_marketplace_name(install_dir) {
        Some(marketplace) => format!("{}@{marketplace}", manifest.name),
        None => manifest.name.clone(),
    }
}

/// When `install_dir` is the versioned cache layout
/// `.../cache/<marketplace>/<plugin>/<version>/`, return the `<marketplace>`
/// segment. Local/session plugins that do not live in the cache return `None`.
fn cache_marketplace_name(install_dir: &Path) -> Option<String> {
    let plugin_dir = install_dir.parent()?;
    let marketplace_dir = plugin_dir.parent()?;
    let cache_dir = marketplace_dir.parent()?;
    (cache_dir.file_name()?.to_str()? == "cache")
        .then(|| marketplace_dir.file_name()?.to_str().map(ToOwned::to_owned))
        .flatten()
}

async fn ensure_plugin_data_dir(
    install_root: &Path,
    plugin_key: &str,
) -> Result<PathBuf, PluginManagerError> {
    let data_key = crate::discovery::sanitize_segment(plugin_key, false);
    let path = install_root.join("data").join(data_key);
    tokio::fs::create_dir_all(&path).await.map_err(|err| {
        PluginManagerError::Io(format!(
            "failed to create plugin data dir for {plugin_key}: {err}"
        ))
    })?;
    Ok(path)
}
#[cfg(test)]
mod user_config_tests {
    use super::*;
    use hooks::events::HookEventType;
    use hooks::{HookDefinition, HookExecutor, HookSource};
    use protocol::HookId;
    use serde_json::json;
    use std::collections::HashMap;

    #[tokio::test]
    async fn plugin_data_dir_sanitizes_installed_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let path = ensure_plugin_data_dir(tmp.path(), "hello@mymkt")
            .await
            .unwrap();

        assert_eq!(path, tmp.path().join("data/hello-mymkt"));
        assert!(path.is_dir());
    }

    fn command_hook(name: &str, command: &str, args: &[&str]) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: name.to_string(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: HookExecutor::Command {
                command: command.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                env: HashMap::new(),
                cwd: None,
                shell: None,
            },
            source: HookSource::Plugin,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        }
    }

    fn ctx() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("API_KEY".into(), json!("sk-live"));
        m.insert("PORT".into(), json!(8080));
        m
    }

    /// An exec-form Command hook (with discrete args) has `${user_config.*}`
    /// substituted into command + args, and gains a `LINGXI_PLUGIN_OPTION_<KEY>`
    /// env entry per userConfig field.
    #[test]
    fn exec_form_hook_substitutes_args_and_injects_env() {
        let hooks = vec![command_hook(
            "check",
            "./verify.sh",
            &["--key", "${user_config.API_KEY}"],
        )];
        let out = apply_user_config_to_hooks("weather", &hooks, &ctx());
        assert_eq!(out.len(), 1);
        let HookExecutor::Command { args, env, .. } = &out[0].executor else {
            panic!("expected Command executor");
        };
        assert_eq!(args, &["--key".to_string(), "sk-live".to_string()]);
        // Both userConfig fields exposed as LINGXI_PLUGIN_OPTION_<KEY> (uppercased,
        // sanitized) with String(value) semantics.
        assert_eq!(
            env.get("LINGXI_PLUGIN_OPTION_API_KEY").map(String::as_str),
            Some("sk-live")
        );
        assert_eq!(
            env.get("LINGXI_PLUGIN_OPTION_PORT").map(String::as_str),
            Some("8080")
        );
    }

    /// A shell-form Command hook (bare command, no args) that references
    /// `${user_config.*}` is REJECTED (skipped) — the substituted value would be
    /// re-parsed by the shell.
    #[test]
    fn shell_form_hook_referencing_user_config_is_skipped() {
        let hooks = vec![
            command_hook("safe", "./ok.sh", &["--port", "${user_config.PORT}"]),
            command_hook("danger", "./run.sh ${user_config.API_KEY}", &[]),
        ];
        let out = apply_user_config_to_hooks("weather", &hooks, &ctx());
        // Only the exec-form hook survives; the shell-form one is dropped.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "safe");
    }

    /// A shell-form Command hook with NO userConfig reference is left intact
    /// (env still gains the option vars), so unrelated bare-command hooks keep
    /// working.
    #[test]
    fn shell_form_hook_without_reference_survives() {
        let hooks = vec![command_hook("plain", "./noop.sh", &[])];
        let out = apply_user_config_to_hooks("weather", &hooks, &ctx());
        assert_eq!(out.len(), 1);
        let HookExecutor::Command { command, env, .. } = &out[0].executor else {
            panic!("expected Command executor");
        };
        assert_eq!(command, "./noop.sh");
        assert!(env.contains_key("LINGXI_PLUGIN_OPTION_API_KEY"));
    }

    /// Empty substitution context ⇒ hooks pass through byte-unchanged (the common
    /// no-userConfig case): no env injection, no substitution, no rejection.
    #[test]
    fn empty_ctx_is_noop() {
        let hooks = vec![command_hook(
            "danger",
            "./run.sh ${user_config.API_KEY}",
            &[],
        )];
        let out = apply_user_config_to_hooks("weather", &hooks, &Map::new());
        assert_eq!(out.len(), 1);
        let HookExecutor::Command { command, env, .. } = &out[0].executor else {
            panic!("expected Command executor");
        };
        // Not rejected, not substituted, no injected env.
        assert_eq!(command, "./run.sh ${user_config.API_KEY}");
        assert!(env.is_empty());
    }

    /// The MCP stdio `command` gate: a scoped stdio server whose `command`
    /// references `${user_config.*}` is rejected by the substitution site (the
    /// value would hit a shell); `args` / `env` remain safe substitution
    /// surfaces (covered by the `enable_substitutes_user_config_*` integration
    /// test).
    #[test]
    fn mcp_stdio_command_reference_is_detected() {
        assert!(user_config::references_user_config(
            "mysrv ${user_config.API_KEY}"
        ));
        assert!(!user_config::references_user_config("mysrv --flag"));
    }
}

#[cfg(test)]
mod unload_tests {
    use super::*;
    use async_trait::async_trait;
    use command_api::{CommandRegistry, CommandSource, SlashCommand, SlashCommandKind};
    use hooks::HookRegistry;
    use lsp::LspRegistry;
    use outputstyles::OutputStyleRegistry;
    use platform_posix::{
        PlainTextSecureStorage, PosixClock, PosixFileSystem, PosixHttp, PosixLspTransport,
        PosixRuntime,
    };
    use protocol::McpConnectionId;
    use skill_api::SkillRegistry;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tool_api::ToolRegistry;
    use traits::{
        ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
        McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
        McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
    };

    fn write_minimal_plugin(root: &Path, dir_name: &str, plugin_name: &str) {
        let plugin_dir = root.join(dir_name);
        fs::create_dir_all(plugin_dir.join(".lingxi-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".lingxi-plugin").join("plugin.json"),
            format!(r#"{{"name":"{plugin_name}","version":"1.0.0"}}"#),
        )
        .unwrap();
    }

    struct FailOnceDisconnectTransport {
        disconnect_calls: AtomicUsize,
        fail_on_call: usize,
    }

    impl FailOnceDisconnectTransport {
        fn failing_on(call: usize) -> Self {
            Self {
                disconnect_calls: AtomicUsize::new(0),
                fail_on_call: call,
            }
        }
    }

    #[async_trait]
    impl McpTransport for FailOnceDisconnectTransport {
        async fn connect(&self, _s: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
            Err(McpError::Internal("unused in unload test".into()))
        }

        async fn initialize(
            &self,
            _c: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, McpError> {
            unreachable!("unused in unload test")
        }

        async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
            unreachable!("unused in unload test")
        }

        async fn list_resources(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<McpResourceDto>, McpError> {
            unreachable!("unused in unload test")
        }

        async fn list_resource_templates(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<traits::McpResourceTemplateDto>, McpError> {
            unreachable!("unused in unload test")
        }

        async fn list_prompts(&self, _c: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
            unreachable!("unused in unload test")
        }

        async fn call_tool(
            &self,
            _c: &McpRawConnection,
            _t: &str,
            _i: serde_json::Value,
        ) -> Result<McpToolResultDto, McpError> {
            unreachable!("unused in unload test")
        }

        async fn read_resource(
            &self,
            _c: &McpRawConnection,
            _u: &str,
        ) -> Result<McpResourceContentDto, McpError> {
            unreachable!("unused in unload test")
        }

        async fn ping(&self, _id: McpConnectionId) -> Result<(), McpError> {
            Ok(())
        }

        async fn notifications(
            &self,
            _c: &McpRawConnection,
        ) -> Result<McpNotificationStream, McpError> {
            unreachable!("unused in unload test")
        }

        async fn handle_elicitation(
            &self,
            _c: &McpRawConnection,
            _r: ElicitRequestDto,
        ) -> Result<ElicitResultDto, McpError> {
            unreachable!("unused in unload test")
        }

        async fn disconnect(&self, _id: McpConnectionId) -> Result<(), McpError> {
            let call = self.disconnect_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_on_call {
                Err(McpError::Internal("disconnect failed".into()))
            } else {
                Ok(())
            }
        }

        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::Stdio]
        }
    }

    #[tokio::test]
    async fn disable_records_partial_mcp_unload_failure_and_retries_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        write_minimal_plugin(tmp.path(), "plug", "plug");

        let transport = Arc::new(FailOnceDisconnectTransport::failing_on(2));
        let mcp_registry = Arc::new(McpRegistry::new(transport));
        let command_registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let storage = PlainTextSecureStorage::new(tmp.path().join("secrets"))
            .await
            .unwrap();
        let credentials = Arc::new(CredentialManager::new(
            Arc::new(storage),
            Arc::new(PosixClock::new()),
            Arc::new(PosixHttp::new()),
        ));
        let manager = PluginManager::new(
            tmp.path().to_path_buf(),
            Arc::new(PosixFileSystem::new(tmp.path().to_path_buf())),
            Arc::new(PosixHttp::new()),
            Arc::new(PosixRuntime::new()),
            credentials,
            Arc::new(StrictPluginOnlyPolicy::empty()),
            command_registry.clone(),
            Arc::new(RwLock::new(SkillRegistry::new())),
            Arc::new(RwLock::new(HookRegistry::new())),
            Arc::new(RwLock::new(OutputStyleRegistry::new())),
            mcp_registry.clone(),
            Arc::new(LspRegistry::new(Arc::new(PosixLspTransport::new()))),
            tool_registry.clone(),
        );

        let discovered = crate::discover_installed_plugins(tmp.path()).await;
        let (id, manifest, dir) = discovered.into_iter().next().unwrap();
        let scoped_names = ["plugin:plug:a", "plugin:plug:b", "plugin:plug:c"];
        manager.plugins.write().await.insert(
            id,
            crate::lifecycle::PluginState::Loaded {
                manifest: manifest.clone(),
                install_dir: dir.clone(),
                loaded_at: std::time::SystemTime::now(),
            },
        );
        manager.plugin_mcp_names.write().await.insert(
            id,
            scoped_names
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        );
        command_registry.write().await.register_plugin_commands(
            id,
            vec![SlashCommand {
                name: "plug-cmd".into(),
                description: "plugin command".into(),
                source: CommandSource::Plugin,
                kind: SlashCommandKind::Builtin {
                    handler_id: "plug-cmd".into(),
                },
                ..SlashCommand::default()
            }],
        );
        for scoped_name in scoped_names {
            mcp_registry.connections.write().await.insert(
                scoped_name.into(),
                mcp::McpConnectionState::Connected {
                    config: McpServerConfig {
                        name: scoped_name.into(),
                        spec: McpTransportSpec::Stdio {
                            command: "echo".into(),
                            args: vec![],
                            env: HashMap::new(),
                        },
                        scope: mcp::ConfigScope::Dynamic,
                        disabled: false,
                        timeout_ms: None,
                        always_load: false,
                        discovery_cache: None,
                        tools: Vec::new(),
                        tool_permissions: std::collections::BTreeMap::new(),
                        config_error: None,
                        metadata: mcp::McpServerMetadata::default(),
                    },
                    connection_id: McpConnectionId::new(),
                    capabilities: ServerCapabilitiesDto {
                        tools: true,
                        resources: false,
                        prompts: false,
                        directory_read: false,
                        logging: false,
                        experimental: HashMap::new(),
                        extensions: HashMap::new(),
                    },
                    negotiated: traits::McpNegotiatedProtocol {
                        era: traits::McpProtocolEra::Legacy,
                        version: "2025-11-25".into(),
                    },
                    tools: vec![],
                    resources: vec![],
                    resource_templates: vec![],
                    prompts: vec![],
                    connected_at: std::time::SystemTime::now(),
                },
            );
        }
        let first = manager.disable(&id).await.expect_err("first disable fails");
        assert!(
            matches!(first, PluginManagerError::Io(ref message) if message.contains("failed to unload plugin MCP server")),
            "got: {first}"
        );
        assert!(
            mcp_registry
                .connections
                .read()
                .await
                .contains_key("plugin:plug:b"),
            "the failing MCP server must stay retryable"
        );
        assert!(
            !mcp_registry
                .connections
                .read()
                .await
                .contains_key("plugin:plug:a"),
            "successful teardown before the failure point must stay removed"
        );
        assert_eq!(
            manager.plugin_mcp_names.read().await.get(&id),
            Some(&vec![
                "plugin:plug:b".to_string(),
                "plugin:plug:c".to_string()
            ]),
            "failed unload must preserve only the remaining MCP tracking for retry"
        );
        assert!(
            command_registry.read().await.resolve("plug-cmd").is_some(),
            "failed unload must leave command materialization intact"
        );
        assert!(
            matches!(
                manager.plugins.read().await.get(&id),
                Some(crate::lifecycle::PluginState::DisablingFailed {
                    manifest: failed_manifest,
                    install_dir,
                    error,
                }) if failed_manifest.id == manifest.id
                    && install_dir == &dir
                    && error.contains("failed to unload plugin MCP server plugin:plug:b")
            ),
            "failed disable must record a retriable partial-unload state"
        );
        assert_eq!(
            manager.loaded_plugin_ids().await,
            vec![id],
            "reload/disable loops must keep retrying a partially-unloaded plugin"
        );

        manager.disable(&id).await.expect("retry disable succeeds");
        assert!(
            !mcp_registry
                .connections
                .read()
                .await
                .contains_key("plugin:plug:b"),
            "successful retry removes the failing MCP state"
        );
        assert!(
            !mcp_registry
                .connections
                .read()
                .await
                .contains_key("plugin:plug:c"),
            "successful retry also removes the untouched trailing MCP state"
        );
        assert!(
            manager.plugin_mcp_names.read().await.get(&id).is_none(),
            "successful retry clears MCP tracking"
        );
        assert!(
            command_registry.read().await.resolve("plug-cmd").is_none(),
            "successful retry clears command materialization"
        );
        assert!(
            matches!(
                manager.plugins.read().await.get(&id),
                Some(crate::lifecycle::PluginState::Disabled { manifest: disabled_manifest, install_dir })
                    if disabled_manifest.id == manifest.id && install_dir == &dir
            ),
            "successful retry transitions the plugin to Disabled"
        );
        assert!(
            manager.loaded_plugin_ids().await.is_empty(),
            "Disabled plugins must no longer participate in reload convergence"
        );
    }
}
