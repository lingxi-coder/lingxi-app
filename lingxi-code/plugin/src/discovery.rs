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
//! This module ports the minimal faithful subset: a directory-walk of the
//! plugins directory (`getPluginsDirectory()` =
//! `~/.claude/plugins`, `pluginDirectories.ts:53`), reading each plugin's
//! manifest + auto-detected component subdirectories into a
//! [`PluginManifest`]. Marketplace-catalog resolution, version cache layout,
//! and the `settings.enabledPlugins` allowlist are NOT ported here (residual
//! — see the GAP E note); this pass discovers every plugin directory present
//! under the plugins root.
//!
//! [`crate::manager::PluginManager::enable`] then materializes the discovered
//! manifests' commands + hooks into the live registries.

use crate::manifest::{ComponentPath, PluginComponents, PluginManifest};
use crate::source::PluginSource;
use crate::trust::default_trust_for_source;

use hooks::loader::parse_hooks_from_settings_json;
use hooks::HookSource;
use protocol::PluginId;
use serde::Deserialize;
use std::collections::HashMap;
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
/// `createPluginFromPath`): each of `commands/`, `agents/`, `skills/`,
/// `output-styles/` is globbed for `*.md` when present, and the standard
/// `hooks/hooks.json` is parsed when present.
async fn detect_components(plugin_dir: &Path) -> PluginComponents {
    let commands = glob_md(&plugin_dir.join("commands")).await;
    let agents = glob_md(&plugin_dir.join("agents")).await;
    let skills = glob_md(&plugin_dir.join("skills")).await;
    let output_styles = glob_md(&plugin_dir.join("output-styles")).await;
    let hooks = load_standard_hooks(plugin_dir).await;

    PluginComponents {
        commands,
        agents,
        skills,
        output_styles,
        hooks,
        mcp_servers: HashMap::new(),
        lsp_servers: HashMap::new(),
    }
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
