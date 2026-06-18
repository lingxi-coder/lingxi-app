//! On-disk discovery of installed plugins (bootstrap, cache-only).
//!
//! Mirrors claude-code's cache-only loader. At startup claude-code calls
//! `loadAllPluginsCacheOnly()` (`main.tsx:282`), whose shared body is
//! `loadPluginsFromMarketplaces({cacheOnly})` (`pluginLoader.ts:1887`). For
//! each installed plugin it runs `createPluginFromPath()`
//! (`pluginLoader.ts:1348`): read `<pluginPath>/.claude-plugin/plugin.json`
//! (`loadPluginManifest`) and **auto-detect** the optional `commands/`,
//! `agents/`, `skills/`, `output-styles/` directories (Step 3,
//! `pluginLoader.ts:1373-1385`), plus the standard `hooks/hooks.json`
//! (`pluginLoader.ts:1618`).
//!
//! Two entry points:
//!
//! * [`discover_enabled_plugins`] is the BOOTSTRAP-faithful one: it reads the
//!   `settings.enabledPlugins` allowlist and resolves each enabled
//!   `name@marketplace` entry to its versioned cache path
//!   `cache/{marketplace}/{plugin}/{version}/` — the real layout
//!   `loadAllPluginsCacheOnly` consumes. Against a real `~/.claude/plugins`
//!   (which holds `cache/`, `npm-cache/`, `installed_plugins.json` — none with
//!   a direct `.claude-plugin/plugin.json`) this is what discovers the
//!   actually-installed plugins.
//!
//! * [`discover_installed_plugins`] is a flat directory-walk that loads any
//!   directory holding a direct `.claude-plugin/plugin.json` child. It is the
//!   primitive used by the local-path install arm (a pre-fetched plugin dir
//!   passed by path, as with `--add-dir`) — NOT the real cache layout. Against
//!   a real claude-code plugins dir it finds nothing, by design.
//!
//! Both share [`load_plugin_from_path`], which mirrors `createPluginFromPath`
//! (`pluginLoader.ts:1348`): read the manifest (Step 1) and auto-detect the
//! optional component directories (Step 3).
//!
//! [`crate::manager::PluginManager::enable`] then materializes the discovered
//! manifests' commands + hooks into the live registries.
//!
//! Residual (still NOT ported in either path): marketplace-catalog source
//! resolution, enterprise allow/blocklist policy, seed-dir precedence, and
//! reading the exact installed version out of `installed_plugins.json`
//! (`discover_enabled_plugins` probes the single-version case instead).

use crate::manifest::{ComponentPath, PluginComponents, PluginManifest};
use crate::source::PluginSource;
use crate::trust::default_trust_for_source;

use hooks::loader::parse_hooks_from_settings_json;
use hooks::HookSource;
use protocol::PluginId;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Raw shape of `.claude-plugin/plugin.json`.
///
/// Mirrors claude-code's `PluginManifestMetadataSchema`
/// (`schemas.ts:273`): `name` required, the rest optional. Unknown
/// top-level fields are ignored (serde default), matching the schema's
/// "unknown top-level fields are silently stripped" contract
/// (`schemas.ts:879-884`).
#[derive(Debug, Deserialize)]
struct RawManifest {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<RawAuthor>,
    #[serde(default)]
    homepage: Option<String>,
}

/// `author` may be a string or an object (`{ name, email, url }`); claude-code
/// uses the object form (`PluginAuthorSchema`, `schemas.ts:250`). Accept both
/// and reduce to the display name.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAuthor {
    Name(String),
    Object {
        #[serde(default)]
        name: Option<String>,
    },
}

impl RawAuthor {
    fn into_display(self) -> Option<String> {
        match self {
            RawAuthor::Name(s) => Some(s),
            RawAuthor::Object { name } => name,
        }
    }
}

/// Parse a `plugin@marketplace` identifier into `(name, Option<marketplace>)`.
///
/// Faithful to claude-code's `parsePluginIdentifier`
/// (`pluginIdentifier.ts:51`): only the FIRST `@` separates name from
/// marketplace; anything after a second `@` is ignored. A bare `name` (no
/// `@`) yields `marketplace = None`.
fn parse_plugin_identifier(id: &str) -> (&str, Option<&str>) {
    match id.split_once('@') {
        Some((name, rest)) => {
            // `rest` may itself contain another `@`; keep only up to the next.
            let marketplace = rest.split('@').next().unwrap_or(rest);
            (name, Some(marketplace))
        }
        None => (id, None),
    }
}

/// Sanitize one path segment exactly as claude-code's `getVersionedCachePathIn`
/// (`pluginLoader.ts:139`) does: marketplace/plugin replace any char outside
/// `[A-Za-z0-9\-_]` with `-`; version additionally keeps `.`.
fn sanitize_segment(s: &str, allow_dot: bool) -> String {
    s.chars()
        .map(|c| {
            let keep = c.is_ascii_alphanumeric()
                || c == '-'
                || c == '_'
                || (allow_dot && c == '.');
            if keep {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Discover the plugins enabled by the `enabledPlugins` allowlist against the
/// REAL claude-code on-disk layout.
///
/// claude-code never flat-walks the plugins directory for manifests. Its
/// cache-only loader `loadAllPluginsCacheOnly()` (`main.tsx:282`) delegates to
/// `loadPluginsFromMarketplaces({cacheOnly})` (`pluginLoader.ts:1888`), which
/// is driven by `settings.enabledPlugins` — a map of `plugin@marketplace` →
/// enabled (`pluginLoader.ts:1898-1906`). Each enabled `name@marketplace`
/// entry resolves through `getVersionedCachePath` (`pluginLoader.ts:139`) to
/// the versioned cache directory
/// `<plugins>/cache/{marketplace}/{plugin}/{version}/`
/// whose `.claude-plugin/plugin.json` is then read by `createPluginFromPath`
/// (`pluginLoader.ts:1348`).
///
/// This port reads the `enabled` allowlist, skips disabled entries, resolves
/// each remaining `name@marketplace` to `cache/{marketplace}/{plugin}/`, and
/// probes that directory for an installed version dir (the version normally
/// comes from `installed_plugins.json`; with a single installed version we
/// pick it, mirroring `probeSeedCacheAnyVersion`,
/// `pluginLoader.ts:217`). Bare `name` entries (no marketplace) and
/// uninstalled/missing entries are skipped — never a flat walk.
///
/// What is still NOT ported (residual): marketplace-catalog source resolution,
/// enterprise allow/blocklist policy (`getStrictKnownMarketplaces` /
/// `getBlockedMarketplaces`), seed-dir precedence, and reading the exact
/// version out of `installed_plugins.json` (we probe instead).
pub async fn discover_enabled_plugins(
    plugins_dir: &Path,
    enabled: &BTreeMap<String, bool>,
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    let cache_root = plugins_dir.join("cache");
    let mut out = Vec::new();
    for (entry_id, is_enabled) in enabled {
        if !is_enabled {
            continue;
        }
        let (name, marketplace) = parse_plugin_identifier(entry_id);
        // Marketplace-qualified entries only — a bare name has no resolvable
        // cache path (claude-code skips non-`plugin@marketplace` keys via the
        // `PluginIdSchema` filter, `pluginLoader.ts:1909`).
        let Some(marketplace) = marketplace else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let plugin_cache_dir = cache_root
            .join(sanitize_segment(marketplace, false))
            .join(sanitize_segment(name, false));
        let Some(versioned) = resolve_installed_version_dir(&plugin_cache_dir).await else {
            continue;
        };
        if let Some((id, manifest)) = load_plugin_from_path(&versioned).await {
            out.push((id, manifest, versioned));
        }
    }
    // Stable ordering by plugin name for deterministic bootstrap.
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    out
}

/// Probe `cache/{marketplace}/{plugin}/` for an installed version directory.
///
/// claude-code knows the exact version from `installed_plugins.json` /
/// marketplace catalog and joins it directly (`getVersionedCachePath`). We
/// don't carry that metadata here, so we probe: if the plugin dir holds
/// exactly one version subdirectory with content, use it (mirroring
/// `probeSeedCacheAnyVersion`'s single-version rule, `pluginLoader.ts:217`).
/// Zero or multiple versions → ambiguous → skip (returns `None`).
async fn resolve_installed_version_dir(plugin_dir: &Path) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(plugin_dir).await.ok()?;
    let mut version_dirs = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            version_dirs.push(entry.path());
        }
    }
    if version_dirs.len() == 1 {
        Some(version_dirs.into_iter().next().unwrap())
    } else {
        None
    }
}

/// Walk `plugins_dir` and return every installed plugin discovered on disk.
///
/// Each returned tuple is `(freshly-minted id, manifest, install dir)`. A
/// missing `plugins_dir` yields an empty vec (zero-cost on a fresh install,
/// matching claude-code's resilient boot). Directories without a readable
/// `.claude-plugin/plugin.json` are skipped.
///
/// `PluginId` is a UUID newtype with no string-stable derivation, so a fresh
/// id is minted per plugin and stamped onto the returned manifest; callers
/// keep the `id` to drive [`crate::manager::PluginManager::enable`] / unload.
pub async fn discover_installed_plugins(
    plugins_dir: &Path,
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    let mut out = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(plugins_dir).await else {
        // Missing / unreadable plugins dir = no plugins (fresh install).
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let entry_path = entry.path();
        if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Some((id, manifest)) = load_plugin_from_path(&entry_path).await {
            out.push((id, manifest, entry_path));
        }
    }
    // Stable ordering by plugin name for deterministic bootstrap.
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    out
}

/// Read + auto-detect a single plugin directory. Returns `None` when there is
/// no readable manifest (the directory is not a plugin).
///
/// Mirrors `createPluginFromPath` (`pluginLoader.ts:1348`): Step 1 loads the
/// manifest, Step 3 auto-detects the optional component directories.
async fn load_plugin_from_path(plugin_dir: &Path) -> Option<(PluginId, PluginManifest)> {
    let manifest_path = plugin_dir.join(".claude-plugin").join("plugin.json");
    let raw = tokio::fs::read_to_string(&manifest_path).await.ok()?;
    let parsed: RawManifest = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %manifest_path.display(),
                "skipping plugin with malformed plugin.json"
            );
            return None;
        }
    };

    let id = PluginId::new();
    let source = PluginSource::LocalPath {
        path: plugin_dir.to_path_buf(),
    };
    let trust_level = default_trust_for_source(&source);

    let components = detect_components(plugin_dir).await;

    let manifest = PluginManifest {
        id,
        name: parsed.name,
        version: parsed.version.unwrap_or_default(),
        description: parsed.description.unwrap_or_default(),
        author: parsed.author.and_then(RawAuthor::into_display),
        homepage: parsed.homepage,
        source,
        components,
        trust_level,
        depends_on: Vec::new(),
        user_config: None,
        channels: Vec::new(),
        settings: HashMap::new(),
    };
    Some((id, manifest))
}

/// Auto-detect the component directories of a plugin (Step 3 of
/// `createPluginFromPath`): `commands/`, `agents/`, `output-styles/` are
/// globbed for `*.md` when present; `skills/` uses the `<name>/SKILL.md`
/// one-level layout (`validatePlugin.ts:731-739`); the standard
/// `hooks/hooks.json` is parsed when present; and MCP / LSP server configs are
/// read from the plugin-root `.mcp.json` / `.lsp.json` files
/// (`mcpPluginIntegration.ts:137`, `lspPluginIntegration.ts:64`).
async fn detect_components(plugin_dir: &Path) -> PluginComponents {
    let commands = glob_md(&plugin_dir.join("commands")).await;
    let agents = glob_md(&plugin_dir.join("agents")).await;
    let skills = glob_skill_dirs(&plugin_dir.join("skills")).await;
    let output_styles = glob_md(&plugin_dir.join("output-styles")).await;
    let hooks = load_standard_hooks(plugin_dir).await;
    let mcp_servers = load_mcp_servers(plugin_dir).await;
    let lsp_servers = load_lsp_servers(plugin_dir).await;

    PluginComponents {
        commands,
        agents,
        skills,
        output_styles,
        hooks,
        mcp_servers,
        lsp_servers,
    }
}

/// Collect plugin skills using claude-code's `<name>/SKILL.md` layout: descend
/// ONE level into `skills/` and collect each subdirectory's `SKILL.md`
/// (`validatePlugin.ts:735-739` — single `.md` files directly in `skills/` are
/// NOT loaded, and a subdir without a `SKILL.md` is skipped). Sorted by path
/// for deterministic ordering; a missing `skills/` dir yields an empty vec.
async fn glob_skill_dirs(skills_dir: &Path) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(skills_dir).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if tokio::fs::try_exists(&skill_md).await.unwrap_or(false) {
            out.push(ComponentPath {
                path: skill_md,
                metadata: None,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Read the plugin-root `.mcp.json` into `{ server name → McpServerConfig }`.
///
/// Mirrors `loadPluginMcpServers` (`mcpPluginIntegration.ts:137`): the file is
/// the standard `.mcp.json` shape `{ "mcpServers": { … } }`, but the shared
/// parser also accepts a bare top-level map of `{name: serverConfig}` via the
/// `parsed.mcpServers || parsed` fallback (`mcpPluginIntegration.ts:243`), so
/// plugin `.mcp.json` files in either form resolve here. A missing / malformed
/// file yields an empty map (non-fatal — claude-code logs and continues), and
/// an individual invalid entry is skipped while valid siblings are kept.
/// Manifest-declared `mcpServers` is NOT read here (residual: `RawManifest`
/// doesn't carry it).
async fn load_mcp_servers(plugin_dir: &Path) -> HashMap<String, mcp::McpServerConfig> {
    let path = plugin_dir.join(".mcp.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return HashMap::new();
    };
    // Plugin MCP servers are dynamic-scoped (`addPluginScopeToServers` uses
    // `scope: 'dynamic'`, `mcpPluginIntegration.ts:353`).
    match mcp::parse_mcp_json_string(&raw, mcp::ConfigScope::Dynamic) {
        Ok(configs) => configs
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "skipping malformed plugin .mcp.json");
            HashMap::new()
        }
    }
}

/// Read the plugin-root `.lsp.json` into `{ server name → LspServerConfig }`.
///
/// Mirrors `loadPluginLspServers` (`lspPluginIntegration.ts:64`): the file is a
/// `Record<name, LspServerConfig>`. The record key is the server name; if an
/// entry omits its own `name`, the key is stamped onto the config (the
/// registry keys by `config.name`). A missing / malformed file yields an empty
/// map. Manifest-declared `lspServers` is NOT read here (residual).
async fn load_lsp_servers(plugin_dir: &Path) -> HashMap<String, traits::LspServerConfig> {
    let path = plugin_dir.join(".lsp.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return HashMap::new();
    };
    let parsed: HashMap<String, traits::LspServerConfig> = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "skipping malformed plugin .lsp.json");
            return HashMap::new();
        }
    };
    parsed
        .into_iter()
        .map(|(key, mut cfg)| {
            if cfg.name.is_empty() {
                cfg.name.clone_from(&key);
            }
            (cfg.name.clone(), cfg)
        })
        .collect()
}

/// Collect every `*.md` file directly under `dir` as a [`ComponentPath`],
/// sorted by path for deterministic ordering. Returns an empty vec when `dir`
/// does not exist (the auto-detect "directory absent" case).
async fn glob_md(dir: &Path) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(ComponentPath {
                path: p,
                metadata: None,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Parse `hooks/hooks.json` into [`hooks::HookDefinition`]s, if present.
///
/// claude-code's `loadPluginHooks` (`pluginLoader.ts:1224`) validates the file
/// against `PluginHooksSchema` — a wrapper `{ description?, hooks }` — and
/// returns the inner `hooks` (`pluginLoader.ts:1238-1241`), whose shape is the
/// same `HooksSettings` used by settings files. We extract that inner `hooks`
/// object and feed it to the engine's settings-hook parser as
/// `{ "hooks": <inner> }`, stamping [`HookSource::Plugin`].
async fn load_standard_hooks(plugin_dir: &Path) -> Vec<hooks::HookDefinition> {
    let path = plugin_dir.join("hooks").join("hooks.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return Vec::new();
    };
    let wrapper: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "skipping malformed hooks.json");
            return Vec::new();
        }
    };
    // The file wraps the settings-shaped hooks under a `hooks` key.
    let Some(inner) = wrapper.get("hooks") else {
        return Vec::new();
    };
    let settings = serde_json::json!({ "hooks": inner });
    match parse_hooks_from_settings_json(&settings.to_string(), HookSource::Plugin) {
        Ok(hooks) => hooks,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to parse plugin hooks");
            Vec::new()
        }
    }
}
