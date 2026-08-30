//! On-disk discovery of installed plugins (bootstrap, cache-only).
//!
//! Mirrors claude-code's cache-only loader. At startup claude-code calls
//! `loadAllPluginsCacheOnly()` (`main.tsx:282`), whose shared body is
//! `loadPluginsFromMarketplaces({cacheOnly})` (`pluginLoader.ts:1887`). For
//! each installed plugin it runs `createPluginFromPath()`
//! (`pluginLoader.ts:1348`): read `<pluginPath>/.lingxi-plugin/plugin.json`
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
//!   `loadAllPluginsCacheOnly` consumes. Against a real `~/.lingxi/plugins`
//!   (which holds `cache/`, `npm-cache/`, `installed_plugins.json` — none with
//!   a direct `.lingxi-plugin/plugin.json`) this is what discovers the
//!   actually-installed plugins.
//!
//! * [`discover_installed_plugins`] is a flat directory-walk that loads any
//!   directory holding a direct `.lingxi-plugin/plugin.json` child. It is the
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

use crate::manifest::{
    ComponentPath, PluginChannel, PluginComponents, PluginManifest, UserConfigField,
    UserConfigSchema,
};
use crate::source::PluginSource;
use crate::trust::default_trust_for_source;

use hooks::loader::parse_hooks_from_settings_json;
use hooks::HookSource;
use indexmap::IndexMap;
use protocol::PluginId;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Raw shape of `.lingxi-plugin/plugin.json`.
///
/// Mirrors claude-code's `PluginManifestSchema` (the full union of
/// `PluginManifestMetadataSchema` + component declaration fields):
/// - Identity: `name`, `version`, `description`, `author`, `homepage`.
/// - Component declarations: `skills`, `commands`, `agents`, `outputStyles`,
///   `hooks`, `mcpServers`, `lspServers`, `channels`.
/// - Metadata: `keywords`, `license`, `repository`.
///
/// `name` is required; all other fields are optional. Unknown top-level
/// fields are silently ignored by serde. Component path fields follow Claude
/// Code's field-specific merge rules in [`detect_components`]: commands,
/// agents, and output styles replace their default directories; skills extend
/// the default; hooks, MCP, and LSP declarations merge with their conventional
/// files. Binary: `PluginManifestSchema` in `schemas.ts`.
#[derive(Debug, Deserialize)]
struct RawManifest {
    name: String,
    /// UI-only label; identity and component namespaces continue to use
    /// `name`.
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    /// Activation fallback when no settings scope has made an explicit
    /// decision. Claude Code defaults this field to true.
    #[serde(rename = "defaultEnabled", default = "default_plugin_enabled")]
    default_enabled: bool,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<RawAuthor>,
    #[serde(default)]
    homepage: Option<String>,
    /// Explicitly declared skill directories. These extend the default
    /// `skills/` directory. Binary: `skills` in `PluginManifestSchema`.
    #[serde(default)]
    skills: Option<PathDecl>,
    /// Explicitly declared command directories. Binary: `commands`.
    #[serde(default)]
    commands: Option<PathDecl>,
    /// Explicitly declared agent directories. Binary: `agents`.
    #[serde(default)]
    agents: Option<PathDecl>,
    /// Explicitly declared output-style directories. Binary: `outputStyles`.
    #[serde(rename = "outputStyles", default)]
    output_styles: Option<PathDecl>,
    /// Explicitly declared MCP server configs (manifest-declared MCP servers).
    /// Binary: `mcpServers` in `PluginManifestSchema`.
    #[serde(rename = "mcpServers", default)]
    mcp_servers: Option<serde_json::Value>,
    /// Explicitly declared LSP server configs. Binary: `lspServers`.
    #[serde(rename = "lspServers", default)]
    lsp_servers: Option<serde_json::Value>,
    /// Explicit hook declarations. Binary: `hooks`.
    #[serde(default)]
    hooks: Option<serde_json::Value>,
    /// User-configurable field declarations (`userConfig` in
    /// `PluginManifestSchema`) — each key maps to a `{description, sensitive,
    /// required, default, type}` object. Sensitive fields route through secure
    /// storage; non-sensitive fields through settings `pluginConfigs`.
    #[serde(rename = "userConfig", default)]
    user_config: Option<HashMap<String, UserConfigField>>,
    /// Plugin settings shipped in the manifest.
    #[serde(default)]
    settings: Option<HashMap<String, serde_json::Value>>,
    /// Channel declarations. Binary: `channels`.
    #[serde(default)]
    channels: Option<Vec<RawPluginChannel>>,
    /// Dependency declarations. Binary: `dependencies`.
    #[serde(default)]
    dependencies: Option<serde_json::Value>,
    /// Plugin keywords. Binary: `keywords`.
    #[serde(default)]
    keywords: Option<Vec<String>>,
    /// SPDX license identifier. Binary: `license`.
    #[serde(default)]
    license: Option<String>,
    /// Repository URL or object. Binary: `repository`.
    #[serde(default)]
    repository: Option<serde_json::Value>,
}

fn default_plugin_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PathDecl {
    One(String),
    Many(Vec<String>),
}

impl PathDecl {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(v) => vec![v],
            Self::Many(v) => v,
        }
    }
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

/// Public `plugin.json` channel shape. `userConfig` is a direct field map,
/// just like top-level `userConfig`; [`UserConfigSchema`] is the engine's
/// internal wrapper.
#[derive(Debug, Clone, Deserialize)]
struct RawPluginChannel {
    server: String,
    #[serde(rename = "userConfig", default)]
    user_config: Option<HashMap<String, UserConfigField>>,
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
pub(crate) fn sanitize_segment(s: &str, allow_dot: bool) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            let keep = c.is_ascii_alphanumeric() || c == '-' || c == '_' || (allow_dot && c == '.');
            if keep {
                c
            } else {
                '-'
            }
        })
        .collect();
    // A pure-dot or empty segment would resolve to the parent (`..`) or current
    // (`.`) directory, letting an attacker-controlled name/version escape its
    // cache subdir (e.g. a malicious plugin.json `"version": ".."` would make a
    // join resolve to the SIBLING cache dir, which an unconditional
    // remove_dir_all would then wipe). Collapse these to a safe token — no real
    // semver / plugin / marketplace segment is ever exactly "", ".", or "..".
    if mapped.is_empty() || mapped == "." || mapped == ".." {
        "-".to_string()
    } else {
        mapped
    }
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
/// whose `.lingxi-plugin/plugin.json` is then read by `createPluginFromPath`
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

/// Re-discover every plugin recorded in `installed_plugins.json`, resolving each
/// to its exact versioned cache dir `cache/<marketplace>/<plugin>/<version>/`.
///
/// This is the durable counterpart to [`crate::manager::PluginManager`]'s
/// record-on-install: the records carry the authoritative `(marketplace,
/// plugin, version)` triple, so resolution is exact (no single-version probe).
/// Each segment is sanitized identically to the install-time path so the read
/// path matches the write path. A record whose cache dir is missing / has no
/// manifest is skipped (resilient to a hand-deleted cache). Returned tuples are
/// `(freshly-minted id, manifest, install dir)`, sorted by plugin name.
async fn discover_recorded_plugins_identified(
    plugins_dir: &Path,
) -> Vec<(String, PluginId, PluginManifest, PathBuf)> {
    let mut out = Vec::new();

    // v2 schema (claude-code 2.1.201): `plugins["<plugin>@<market>"] = [ {scope,
    // installPath, version, installedAt, lastUpdated} ]`. Each record carries the
    // exact `installPath` cache dir, so resolution is direct. Read the raw JSON so
    // the v2 array shape and the legacy `plugins[market][plugin]` object shape can
    // coexist during migration.
    let raw = tokio::fs::read_to_string(crate::installed::path(plugins_dir))
        .await
        .ok();
    if let Some(records) = raw
        .as_deref()
        .and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok())
        .and_then(|v| v.get("plugins").and_then(|p| p.as_object()).cloned())
    {
        let cache_root = plugins_dir.join("cache");
        for (key, value) in &records {
            match value {
                // v2: array of per-scope records; use each record's installPath.
                serde_json::Value::Array(recs) => {
                    let mut seen: Option<PathBuf> = None;
                    for rec in recs {
                        if let Some(path) = rec.get("installPath").and_then(|p| p.as_str()) {
                            let dir = PathBuf::from(path);
                            if seen.as_ref() == Some(&dir) {
                                continue; // same cache dir across scopes — load once
                            }
                            if let Some((id, manifest)) = load_plugin_from_path(&dir).await {
                                out.push((key.clone(), id, manifest, dir.clone()));
                                seen = Some(dir);
                            }
                        }
                    }
                }
                // Legacy: `{ "<plugin>": {version, added} }` under a marketplace key.
                serde_json::Value::Object(plugins) => {
                    for (name, record) in plugins {
                        let version = record
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        let dir = cache_root
                            .join(sanitize_segment(key, false))
                            .join(sanitize_segment(name, false))
                            .join(sanitize_segment(version, true));
                        if let Some((id, manifest)) = load_plugin_from_path(&dir).await {
                            out.push((format!("{name}@{key}"), id, manifest, dir));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    out.sort_by(|a, b| a.2.name.cmp(&b.2.name));
    out
}

/// Re-discover every plugin recorded in `installed_plugins.json`.
///
/// Listing and management callers intentionally receive every installed
/// plugin regardless of its activation state. Runtime startup should use
/// [`discover_effective_plugins`] instead.
pub async fn discover_recorded_plugins(
    plugins_dir: &Path,
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    discover_recorded_plugins_identified(plugins_dir)
        .await
        .into_iter()
        .map(|(_, id, manifest, path)| (id, manifest, path))
        .collect()
}

/// Resolve the effective runtime plugin set.
///
/// Explicit `enabledPlugins` entries win. Installed plugins with no explicit
/// setting fall back to their manifest's `defaultEnabled` value (defaulting to
/// true), while explicitly disabled plugins remain unloaded. The explicit
/// cache probe is retained for old/cache-only installations that have no
/// `installed_plugins.json` record.
pub async fn discover_effective_plugins(
    plugins_dir: &Path,
    enabled: &BTreeMap<String, bool>,
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    let mut out = discover_enabled_plugins(plugins_dir, enabled).await;
    let mut seen_paths: BTreeSet<PathBuf> = out.iter().map(|(_, _, path)| path.clone()).collect();

    for (identifier, id, manifest, path) in discover_recorded_plugins_identified(plugins_dir).await
    {
        let active = enabled
            .get(&identifier)
            .copied()
            .unwrap_or(manifest.default_enabled);
        if active && seen_paths.insert(path.clone()) {
            out.push((id, manifest, path));
        }
    }

    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    out
}

/// Walk `plugins_dir` and return every installed plugin discovered on disk.
///
/// Each returned tuple is `(freshly-minted id, manifest, install dir)`. A
/// missing `plugins_dir` yields an empty vec (zero-cost on a fresh install,
/// matching claude-code's resilient boot). Directories without a readable
/// `.lingxi-plugin/plugin.json` are skipped.
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

/// (M4 cc2.1.198) Load the `--plugin-dir <path>` session-only plugins — the
/// path arm of the binary's inline-plugin loader `EBm` (2.1.198):
///
/// * a missing path is a WARN + skip, never a boot failure
///   (`C(\`Plugin path does not exist: ${l} (${errno}), skipping\`,
///   {level:"warn"})`);
/// * a `.zip` is extracted into a fresh per-session temp dir
///   (`PXt()` = `join(os.tmpdir(), "claude-plugin-session-<hex>")`;
///   `inline-{i}-{name}` child), then wrapper-dir–unwrapped (`Yor`: a single
///   directory child holding the manifest dir becomes the plugin root);
/// * the resulting directory loads exactly like an installed plugin
///   (`zor` ≙ [`load_plugin_from_path`]); success logs
///   `Loaded inline plugin from path: {name}`;
/// * a summary `Loaded {n} session-only plugins from --plugin-dir` follows.
pub async fn discover_cli_plugin_dirs(
    paths: &[PathBuf],
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    let mut out = Vec::new();
    for (i, raw) in paths.iter().enumerate() {
        let path = match tokio::fs::canonicalize(raw).await {
            Ok(p) => p,
            // `Cd.stat(l)` failed → warn + `path-not-found` record, skip.
            Err(e) => {
                tracing::warn!(
                    "Plugin path does not exist: {} ({}), skipping",
                    raw.display(),
                    e.raw_os_error()
                        .map_or_else(|| "UNKNOWN".to_string(), errno_name)
                );
                continue;
            }
        };
        let is_zip = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("zip"));
        let plugin_root = if is_zip {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("download")
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            let dest = session_temp_dir().join(format!("inline-{i}-{stem}"));
            let _ = tokio::fs::remove_dir_all(&dest).await;
            if let Err(e) = tokio::fs::create_dir_all(&dest).await {
                tracing::warn!("Failed to load session plugin from {}: {e}", raw.display());
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Failed to load session plugin from {}: {e}", raw.display());
                    continue;
                }
            };
            // Reuse the guarded zip extractor (`.mcpb` IS a zip; the same
            // path-traversal / zip-bomb limits protect inline plugin zips).
            if let Err(e) = crate::mcpb::unpack_mcpb(&bytes, &dest) {
                tracing::warn!("Failed to load session plugin from {}: {e}", raw.display());
                continue;
            }
            tracing::debug!("Extracted inline plugin zip to {}", dest.display());
            // `Yor`: unwrap a single wrapper directory holding the manifest.
            unwrap_zip_root(&dest).await
        } else {
            path
        };
        match load_plugin_from_path(&plugin_root).await {
            Some((id, manifest)) => {
                tracing::debug!("Loaded inline plugin from path: {}", manifest.name);
                out.push((id, manifest, plugin_root));
            }
            None => {
                tracing::warn!(
                    "Failed to load session plugin from {}: no readable {}/plugin.json",
                    raw.display(),
                    branding::PLUGIN_MANIFEST_DIR
                );
            }
        }
    }
    if !out.is_empty() {
        tracing::debug!(
            "Loaded {} session-only plugins from --plugin-dir",
            out.len()
        );
    }
    out
}

/// The per-process inline-plugin extraction dir (`PXt()` port: a temp-root
/// session dir, created lazily and reused for every `--plugin-dir` zip).
fn session_temp_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        std::env::temp_dir().join(format!("lingxi-plugin-session-{}", std::process::id()))
    })
    .clone()
}

/// `Yor` port: when the extracted zip root holds EXACTLY ONE entry, a
/// directory that itself contains the plugin manifest dir, descend into it
/// (zips often wrap the plugin in a single top-level folder).
async fn unwrap_zip_root(root: &Path) -> PathBuf {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return root.to_path_buf();
    };
    let mut only: Option<PathBuf> = None;
    let mut count = 0usize;
    while let Ok(Some(e)) = entries.next_entry().await {
        count += 1;
        if count > 1 {
            return root.to_path_buf();
        }
        if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            only = Some(e.path());
        }
    }
    if let Some(inner) = only {
        if tokio::fs::metadata(inner.join(branding::PLUGIN_MANIFEST_DIR))
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            tracing::debug!(
                "Inline plugin zip had wrapper directory; using {}",
                inner.display()
            );
            return inner;
        }
    }
    root.to_path_buf()
}

/// Best-effort errno → name mapping for the `path does not exist` warn line
/// (the binary logs node's errno code, e.g. `ENOENT`).
fn errno_name(code: i32) -> String {
    match code {
        2 => "ENOENT".to_string(),
        13 => "EACCES".to_string(),
        20 => "ENOTDIR".to_string(),
        other => format!("errno {other}"),
    }
}

/// Read + auto-detect a single plugin directory. Returns `None` when there is
/// no readable manifest (the directory is not a plugin).
///
/// Mirrors `createPluginFromPath` (`pluginLoader.ts:1348`): Step 1 loads the
/// manifest, Step 3 auto-detects the optional component directories.
pub(crate) async fn load_plugin_from_path(plugin_dir: &Path) -> Option<(PluginId, PluginManifest)> {
    let manifest_path = plugin_dir
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
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

    let components = detect_components(plugin_dir, &parsed).await;
    let settings = load_plugin_settings(plugin_dir, parsed.settings.as_ref()).await;
    let channels = validate_plugin_channels(
        parsed.channels.as_deref().unwrap_or_default(),
        &components.mcp_servers,
        &manifest_path,
    );
    let dependencies = match crate::parse_dependencies(parsed.dependencies.as_ref()) {
        Ok(dependencies) => dependencies,
        Err(error) => {
            tracing::warn!(
                path = %manifest_path.display(),
                %error,
                "ignoring invalid plugin dependency declarations"
            );
            Vec::new()
        }
    };

    let manifest = PluginManifest {
        id,
        name: parsed.name,
        display_name: parsed.display_name,
        default_enabled: parsed.default_enabled,
        version: parsed.version.unwrap_or_default(),
        description: parsed.description.unwrap_or_default(),
        author: parsed.author.and_then(RawAuthor::into_display),
        homepage: parsed.homepage,
        source,
        components,
        trust_level,
        depends_on: Vec::new(),
        dependencies,
        user_config: parsed.user_config.map(|fields| UserConfigSchema { fields }),
        channels,
        settings,
    };
    Some((id, manifest))
}

/// Retain only channel declarations that can bind to a server contributed by
/// this plugin. A malformed reference must not silently become a connection to
/// a same-named user/project MCP server.
fn validate_plugin_channels(
    declared: &[RawPluginChannel],
    mcp_servers: &HashMap<String, mcp::McpServerConfig>,
    manifest_path: &Path,
) -> Vec<PluginChannel> {
    let mut seen = BTreeSet::new();
    let mut channels = Vec::with_capacity(declared.len());

    for channel in declared {
        if channel.server.is_empty() || !mcp_servers.contains_key(&channel.server) {
            tracing::warn!(
                path = %manifest_path.display(),
                server = %channel.server,
                "skipping plugin channel whose server is not declared by this plugin"
            );
            continue;
        }
        if !seen.insert(channel.server.clone()) {
            tracing::warn!(
                path = %manifest_path.display(),
                server = %channel.server,
                "skipping duplicate plugin channel declaration"
            );
            continue;
        }
        channels.push(PluginChannel {
            server: channel.server.clone(),
            user_config: channel
                .user_config
                .clone()
                .map(|fields| UserConfigSchema { fields }),
        });
    }

    channels
}

/// Auto-detect the component directories of a plugin (Step 3 of
/// `createPluginFromPath`): `commands/`, `agents/`, `output-styles/` are
/// globbed for `*.md` when present; `skills/` uses the `<name>/SKILL.md`
/// one-level layout (`validatePlugin.ts:731-739`); the standard
/// `hooks/hooks.json` is parsed when present; and MCP / LSP server configs are
/// read from the plugin-root `.mcp.json` / `.lsp.json` files
/// (`mcpPluginIntegration.ts:137`, `lspPluginIntegration.ts:64`).
async fn detect_components(plugin_dir: &Path, parsed: &RawManifest) -> PluginComponents {
    let default_commands = glob_md(&plugin_dir.join("commands")).await;
    let default_agents = glob_md(&plugin_dir.join("agents")).await;
    let default_skills_dir = plugin_dir.join("skills");
    let mut default_skills = glob_skill_dirs(&default_skills_dir).await;
    if parsed.skills.is_none()
        && !tokio::fs::try_exists(&default_skills_dir)
            .await
            .unwrap_or(false)
    {
        let root_skill = plugin_dir.join("SKILL.md");
        if tokio::fs::try_exists(&root_skill).await.unwrap_or(false) {
            default_skills.push(ComponentPath {
                path: root_skill,
                metadata: component_root_metadata(plugin_dir),
            });
        }
    }
    let default_output_styles = glob_md(&plugin_dir.join("output-styles")).await;
    let default_hooks = load_standard_hooks(plugin_dir).await;
    let default_mcp_servers = load_mcp_servers(plugin_dir).await;
    let default_lsp_servers = load_lsp_servers(plugin_dir).await;

    let commands = match &parsed.commands {
        Some(paths) => resolve_markdown_declared_paths(plugin_dir, paths.clone()).await,
        None => default_commands,
    };
    let agents = match &parsed.agents {
        Some(paths) => resolve_markdown_declared_paths(plugin_dir, paths.clone()).await,
        None => default_agents,
    };
    let mut skills = default_skills;
    if let Some(paths) = &parsed.skills {
        skills.extend(resolve_skill_declared_paths(plugin_dir, paths.clone()).await);
        dedup_component_paths(&mut skills);
    }
    let output_styles = match &parsed.output_styles {
        Some(paths) => resolve_markdown_declared_paths(plugin_dir, paths.clone()).await,
        None => default_output_styles,
    };
    let mut hooks = default_hooks;
    hooks.extend(load_declared_hooks(plugin_dir, parsed.hooks.clone()).await);
    let mut mcp_servers = default_mcp_servers;
    mcp_servers.extend(load_declared_mcp_servers(plugin_dir, parsed.mcp_servers.clone()).await);
    let mut lsp_servers = default_lsp_servers;
    lsp_servers.extend(load_declared_lsp_servers(plugin_dir, parsed.lsp_servers.clone()).await);

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

fn resolve_declared_relative_path(plugin_dir: &Path, raw: &str) -> Option<PathBuf> {
    let rel = raw.strip_prefix("./")?;
    if rel.is_empty() {
        return None;
    }
    let path = Path::new(rel);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(plugin_dir.join(path))
}

fn dedup_component_paths(paths: &mut Vec<ComponentPath>) {
    let mut seen = BTreeSet::new();
    paths.retain(|cp| seen.insert(cp.path.clone()));
}

fn component_root_metadata(root: &Path) -> Option<Value> {
    Some(serde_json::json!({ "root": root }))
}

fn stamp_component_root(paths: &mut [ComponentPath], root: &Path) {
    for path in paths {
        path.metadata = component_root_metadata(root);
    }
}

async fn resolve_markdown_declared_paths(plugin_dir: &Path, paths: PathDecl) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    for raw in paths.into_vec() {
        let Some(abs) = resolve_declared_relative_path(plugin_dir, &raw) else {
            tracing::warn!(path = %raw, "skipping invalid plugin manifest markdown path");
            continue;
        };
        let Ok(meta) = tokio::fs::metadata(&abs).await else {
            tracing::warn!(path = %abs.display(), "skipping missing plugin manifest markdown path");
            continue;
        };
        if meta.is_dir() {
            let mut found = glob_md(&abs).await;
            stamp_component_root(&mut found, &abs);
            out.extend(found);
        } else if abs
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            let root = abs.parent().unwrap_or(plugin_dir).to_path_buf();
            out.push(ComponentPath {
                path: abs,
                metadata: component_root_metadata(&root),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    dedup_component_paths(&mut out);
    out
}

async fn resolve_skill_declared_paths(plugin_dir: &Path, paths: PathDecl) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    for raw in paths.into_vec() {
        let Some(abs) = resolve_declared_relative_path(plugin_dir, &raw) else {
            tracing::warn!(path = %raw, "skipping invalid plugin manifest skill path");
            continue;
        };
        let Ok(meta) = tokio::fs::metadata(&abs).await else {
            tracing::warn!(path = %abs.display(), "skipping missing plugin manifest skill path");
            continue;
        };
        if meta.is_dir() {
            let direct_skill = abs.join("SKILL.md");
            if tokio::fs::try_exists(&direct_skill).await.unwrap_or(false) {
                let root = abs.parent().unwrap_or(plugin_dir);
                out.push(ComponentPath {
                    path: direct_skill,
                    metadata: component_root_metadata(root),
                });
            } else {
                let mut found = glob_skill_dirs(&abs).await;
                stamp_component_root(&mut found, &abs);
                out.extend(found);
            }
        } else if abs.file_name().and_then(|s| s.to_str()) == Some("SKILL.md") {
            let root = abs
                .parent()
                .and_then(Path::parent)
                .unwrap_or(plugin_dir)
                .to_path_buf();
            out.push(ComponentPath {
                path: abs,
                metadata: component_root_metadata(&root),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    dedup_component_paths(&mut out);
    out
}

/// Collect plugin skills recursively, stopping at every directory that owns a
/// `SKILL.md`. Nested grouping directories are allowed, but content below a
/// discovered skill root is not scanned as another independent skill.
async fn glob_skill_dirs(skills_dir: &Path) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    let mut stack = vec![skills_dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if current != skills_dir {
            let skill_md = current.join("SKILL.md");
            if tokio::fs::try_exists(&skill_md).await.unwrap_or(false) {
                out.push(ComponentPath {
                    path: skill_md,
                    metadata: component_root_metadata(skills_dir),
                });
                continue;
            }
        }
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(entry.path());
            }
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
/// Manifest-declared `mcpServers` is merged separately by
/// [`load_declared_mcp_servers`].
async fn load_mcp_servers(plugin_dir: &Path) -> HashMap<String, mcp::McpServerConfig> {
    let path = plugin_dir.join(".mcp.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return HashMap::new();
    };
    // Plugin MCP servers are dynamic-scoped (`addPluginScopeToServers` uses
    // `scope: 'dynamic'`, `mcpPluginIntegration.ts:353`).
    match mcp::parse_mcp_json_string(&raw, mcp::ConfigScope::Dynamic) {
        Ok(configs) => configs.into_iter().map(|c| (c.name.clone(), c)).collect(),
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
/// map. Manifest-declared `lspServers` is merged separately by
/// [`load_declared_lsp_servers`].
async fn load_lsp_servers(plugin_dir: &Path) -> IndexMap<String, traits::LspServerConfig> {
    let path = plugin_dir.join(".lsp.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return IndexMap::new();
    };
    let parsed: IndexMap<String, Value> = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "skipping malformed plugin .lsp.json");
            return IndexMap::new();
        }
    };
    parse_lsp_records(parsed)
}

/// Collect every `*.md` file under `dir` **recursively** as a [`ComponentPath`],
/// sorted by path for deterministic ordering. Returns an empty vec when `dir`
/// does not exist (the auto-detect "directory absent" case).
///
/// Mirrors `walkPluginMarkdown` (`walkPluginMarkdown.ts:21-69`): it descends
/// into subdirectories so nested files carry their directory namespace
/// (`commands/git/commit.md` → command `git:commit`, then `{plugin}:git:commit`
/// via `command_name_from_path` + the manager's `{plugin}:` prefix), and it
/// matches the `.md` extension case-insensitively (TS `toLowerCase()`).
/// (`stopAtSkillDir` is not modeled here: LingXi discovers plugin skills only
/// from the dedicated `skills/` directory via [`glob_skill_dirs`], so a
/// `SKILL.md` nested under `commands/` is not a supported LingXi layout.)
async fn glob_md(dir: &Path) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    // Iterative DFS (avoids boxing for async recursion). A directory that
    // cannot be read is skipped, matching TS's swallowed readdir errors.
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                out.push(ComponentPath {
                    path: p,
                    metadata: component_root_metadata(dir),
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Load the plugin settings understood by Claude Code's plugin runtime. A
/// valid plugin-root `settings.json` takes precedence over manifest `settings`;
/// an absent or malformed file falls back to the manifest. Unknown keys are
/// discarded instead of merging arbitrary plugin data into host settings.
async fn load_plugin_settings(
    plugin_dir: &Path,
    manifest_settings: Option<&HashMap<String, Value>>,
) -> HashMap<String, Value> {
    const ALLOWED: [&str; 2] = ["agent", "subagentStatusLine"];

    let settings_path = plugin_dir.join("settings.json");
    let selected = match tokio::fs::read_to_string(&settings_path).await {
        Ok(raw) => match serde_json::from_str::<HashMap<String, Value>>(&raw) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(
                    path = %settings_path.display(),
                    error = %error,
                    "ignoring malformed plugin settings.json"
                );
                manifest_settings.cloned().unwrap_or_default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            manifest_settings.cloned().unwrap_or_default()
        }
        Err(error) => {
            tracing::warn!(
                path = %settings_path.display(),
                error = %error,
                "unable to read plugin settings.json"
            );
            manifest_settings.cloned().unwrap_or_default()
        }
    };

    selected
        .into_iter()
        .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
        .collect()
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
    parse_hooks_value(&wrapper, &path)
}

fn parse_hooks_value(value: &Value, path: &Path) -> Vec<hooks::HookDefinition> {
    let inner = value.get("hooks").unwrap_or(value);
    let settings = serde_json::json!({ "hooks": inner });
    match parse_hooks_from_settings_json(&settings.to_string(), HookSource::Plugin) {
        Ok(hooks) => hooks,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to parse plugin hooks");
            Vec::new()
        }
    }
}

async fn load_declared_hooks(
    plugin_dir: &Path,
    value: Option<Value>,
) -> Vec<hooks::HookDefinition> {
    let Some(value) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match value {
        Value::String(path) => {
            out.extend(load_declared_hooks_from_path(plugin_dir, &path).await);
        }
        Value::Array(items) if items.iter().all(|v| matches!(v, Value::String(_))) => {
            for item in items {
                if let Value::String(path) = item {
                    out.extend(load_declared_hooks_from_path(plugin_dir, &path).await);
                }
            }
        }
        other => out.extend(parse_hooks_value(&other, plugin_dir)),
    }
    out
}

async fn load_declared_hooks_from_path(plugin_dir: &Path, raw: &str) -> Vec<hooks::HookDefinition> {
    let Some(path) = resolve_declared_relative_path(plugin_dir, raw) else {
        tracing::warn!(path = %raw, "skipping invalid plugin manifest hooks path");
        return Vec::new();
    };
    let Ok(raw_json) = tokio::fs::read_to_string(&path).await else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw_json) else {
        return Vec::new();
    };
    parse_hooks_value(&value, &path)
}

async fn load_declared_mcp_servers(
    plugin_dir: &Path,
    value: Option<Value>,
) -> HashMap<String, mcp::McpServerConfig> {
    load_declared_json_records(plugin_dir, value, |raw| {
        mcp::parse_mcp_json_string(raw, mcp::ConfigScope::Dynamic).map(|v| {
            v.into_iter()
                .map(|cfg| (cfg.name.clone(), cfg))
                .collect::<HashMap<_, _>>()
        })
    })
    .await
}

async fn load_declared_lsp_servers(
    plugin_dir: &Path,
    value: Option<Value>,
) -> IndexMap<String, traits::LspServerConfig> {
    let Some(value) = value else {
        return IndexMap::new();
    };
    let items = match value {
        Value::Array(items) => items,
        one => vec![one],
    };
    let mut out = IndexMap::new();
    for item in items {
        let parsed = match item {
            Value::String(raw_path) => {
                let Some(path) = resolve_declared_relative_path(plugin_dir, &raw_path) else {
                    tracing::warn!(path = %raw_path, "skipping invalid plugin manifest json path");
                    continue;
                };
                let Ok(raw) = tokio::fs::read_to_string(&path).await else {
                    continue;
                };
                serde_json::from_str::<IndexMap<String, Value>>(&raw)
            }
            inline @ Value::Object(_) => serde_json::from_value::<IndexMap<String, Value>>(inline),
            _ => continue,
        };
        if let Ok(parsed) = parsed {
            out.extend(parse_lsp_records(parsed));
        }
    }
    out
}

fn parse_lsp_records(
    records: IndexMap<String, Value>,
) -> IndexMap<String, traits::LspServerConfig> {
    records
        .into_iter()
        .filter_map(|(key, value)| {
            let mut config = match serde_json::from_value::<traits::LspServerConfig>(value) {
                Ok(config) => config,
                Err(error) => {
                    tracing::warn!(server = %key, %error, "skipping malformed plugin LSP server configuration");
                    return None;
                }
            };
            // The record key is authoritative in Claude Code's public shape;
            // an extra legacy `name` field must not rename or collide with a
            // sibling server.
            config.name.clone_from(&key);
            validate_lsp_config(&config).then(|| (config.name.clone(), config))
        })
        .collect()
}

fn validate_lsp_config(config: &traits::LspServerConfig) -> bool {
    let valid = !config.command.trim().is_empty()
        && !config.extension_to_language.is_empty()
        && matches!(config.transport.as_str(), "stdio" | "socket")
        && config.startup_timeout.is_none_or(|timeout| timeout > 0)
        && config.shutdown_timeout.is_none_or(|timeout| timeout > 0);
    if !valid {
        tracing::warn!(
            server = %config.name,
            "skipping invalid plugin LSP server configuration"
        );
    }
    valid
}

async fn load_declared_json_records<T, E>(
    plugin_dir: &Path,
    value: Option<Value>,
    parse: impl Fn(&str) -> Result<HashMap<String, T>, E>,
) -> HashMap<String, T> {
    let Some(value) = value else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    match value {
        Value::String(path) => {
            merge_declared_json_records(plugin_dir, &path, &parse, &mut out).await;
        }
        Value::Array(items) if items.iter().all(|v| matches!(v, Value::String(_))) => {
            for item in items {
                if let Value::String(path) = item {
                    merge_declared_json_records(plugin_dir, &path, &parse, &mut out).await;
                }
            }
        }
        other => {
            if let Ok(parsed) = parse(&other.to_string()) {
                out.extend(parsed);
            }
        }
    }
    out
}

async fn merge_declared_json_records<T, E>(
    plugin_dir: &Path,
    raw_path: &str,
    parse: &impl Fn(&str) -> Result<HashMap<String, T>, E>,
    out: &mut HashMap<String, T>,
) {
    let Some(path) = resolve_declared_relative_path(plugin_dir, raw_path) else {
        tracing::warn!(path = %raw_path, "skipping invalid plugin manifest json path");
        return;
    };
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return;
    };
    if let Ok(parsed) = parse(&raw) {
        out.extend(parsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn declared_commands_replace_defaults_and_declared_skills_extend_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin.join("commands")).unwrap();
        fs::create_dir_all(plugin.join("custom-commands")).unwrap();
        fs::create_dir_all(plugin.join("skills/base")).unwrap();
        fs::create_dir_all(plugin.join("extra-skills/extra")).unwrap();

        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"demo",
                "commands":"./custom-commands",
                "skills":"./extra-skills"
            }"#,
        )
        .unwrap();
        fs::write(plugin.join("commands/default.md"), "default").unwrap();
        fs::write(plugin.join("custom-commands/custom.md"), "custom").unwrap();
        fs::write(plugin.join("skills/base/SKILL.md"), "base").unwrap();
        fs::write(plugin.join("extra-skills/extra/SKILL.md"), "extra").unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        let commands: Vec<_> = manifest
            .components
            .commands
            .iter()
            .map(|c| c.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(commands, vec!["custom.md"]);

        let skills: Vec<_> = manifest
            .components
            .skills
            .iter()
            .map(|c| {
                c.path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(skills, vec!["base", "extra"]);
    }

    #[tokio::test]
    async fn root_skill_is_discovered_for_single_skill_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"single-skill"}"#,
        )
        .unwrap();
        fs::write(plugin.join("SKILL.md"), "root skill").unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(manifest.components.skills.len(), 1);
        assert_eq!(manifest.components.skills[0].path, plugin.join("SKILL.md"));
    }

    #[tokio::test]
    async fn manifest_preserves_display_name_and_default_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"deployment-tools","displayName":"Deployment Tools","defaultEnabled":false}"#,
        )
        .unwrap();

        let (_, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(manifest.display_name.as_deref(), Some("Deployment Tools"));
        assert!(!manifest.default_enabled);
    }

    #[tokio::test]
    async fn effective_discovery_applies_explicit_state_before_manifest_default() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        let enabled_path = plugins_dir.join("cache/mkt/enabled/1.0.0");
        let disabled_path = plugins_dir.join("cache/mkt/disabled/1.0.0");
        for path in [&enabled_path, &disabled_path] {
            fs::create_dir_all(path.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        }
        fs::write(
            enabled_path
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"enabled"}"#,
        )
        .unwrap();
        fs::write(
            disabled_path
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"disabled","defaultEnabled":false}"#,
        )
        .unwrap();
        fs::write(
            crate::installed::path(plugins_dir),
            serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "plugins": {
                    "enabled@mkt": [{
                        "scope": "user",
                        "installPath": enabled_path,
                        "version": "1.0.0"
                    }],
                    "disabled@mkt": [{
                        "scope": "user",
                        "installPath": disabled_path,
                        "version": "1.0.0"
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let defaults = discover_effective_plugins(plugins_dir, &BTreeMap::new()).await;
        assert_eq!(
            defaults
                .iter()
                .map(|(_, manifest, _)| manifest.name.as_str())
                .collect::<Vec<_>>(),
            vec!["enabled"]
        );

        let overrides = BTreeMap::from([
            ("enabled@mkt".to_string(), false),
            ("disabled@mkt".to_string(), true),
        ]);
        let explicit = discover_effective_plugins(plugins_dir, &overrides).await;
        assert_eq!(
            explicit
                .iter()
                .map(|(_, manifest, _)| manifest.name.as_str())
                .collect::<Vec<_>>(),
            vec!["disabled"]
        );
    }

    #[tokio::test]
    async fn declared_skill_directory_may_contain_skill_md_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin.join("custom-skill")).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"demo","skills":"./custom-skill"}"#,
        )
        .unwrap();
        fs::write(plugin.join("custom-skill/SKILL.md"), "custom skill").unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(manifest.components.skills.len(), 1);
        assert_eq!(
            manifest.components.skills[0].path,
            plugin.join("custom-skill/SKILL.md")
        );
    }

    #[tokio::test]
    async fn public_lsp_schema_loads_without_redundant_internal_fields() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(".lsp.json"),
            r#"{
                "typescript": {
                    "command": "typescript-language-server",
                    "args": ["--stdio"],
                    "extensionToLanguage": {".ts": "typescript"},
                    "transport": "socket",
                    "initializationOptions": {"hostInfo": "claude"},
                    "settings": {"typescript": {"format": {"enable": true}}},
                    "workspaceFolder": "./workspace",
                    "startupTimeout": 1234,
                    "shutdownTimeout": 4321,
                    "restartOnCrash": false,
                    "maxRestarts": 7,
                    "diagnostics": false
                }
            }"#,
        )
        .unwrap();

        let configs = load_lsp_servers(tmp.path()).await;
        let cfg = configs.get("typescript").expect("public config loads");
        assert_eq!(cfg.name, "typescript");
        assert_eq!(cfg.transport, "socket");
        assert_eq!(cfg.extension_to_language[".ts"], "typescript");
        assert_eq!(cfg.workspace_folder.as_deref(), Some("./workspace"));
        assert_eq!(cfg.startup_timeout, Some(1234));
        assert_eq!(cfg.shutdown_timeout, Some(4321));
        assert_eq!(cfg.restart_on_crash, Some(false));
        assert_eq!(cfg.max_restarts, Some(7));
        assert_eq!(cfg.diagnostics, Some(false));
    }

    #[tokio::test]
    async fn declared_lsp_array_preserves_path_and_inline_order() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("from-file.json"),
            r#"{"first":{"command":"one","extensionToLanguage":{".a":"a"}}}"#,
        )
        .unwrap();
        let configs = load_declared_lsp_servers(
            tmp.path(),
            Some(serde_json::json!([
                "./from-file.json",
                {"second":{"command":"two","extensionToLanguage":{".b":"b"}}}
            ])),
        )
        .await;
        assert_eq!(
            configs.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[tokio::test]
    async fn malformed_lsp_server_does_not_hide_valid_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(".lsp.json"),
            r#"{
                "broken": {"args": ["--stdio"], "extensionToLanguage": {".bad": "bad"}},
                "working": {"name": "spoofed", "command": "good-lsp", "extensionToLanguage": {".ok": "ok"}}
            }"#,
        )
        .unwrap();

        let configs = load_lsp_servers(tmp.path()).await;
        assert!(!configs.contains_key("broken"));
        assert!(!configs.contains_key("spoofed"));
        assert_eq!(configs["working"].name, "working");
        assert_eq!(configs["working"].command, "good-lsp");
    }

    #[tokio::test]
    async fn declared_relative_paths_are_confined_and_inline_mcp_lsp_are_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{
                "name":"demo",
                "commands":"../escape",
                "mcpServers":{"inline":{"type":"stdio","command":"echo"}},
                "lspServers":{"rust":{"command":"rust-analyzer","extensionToLanguage":{".rs":"rust"}}}
            }"#,
        )
        .unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert!(manifest.components.commands.is_empty());
        assert!(manifest.components.mcp_servers.contains_key("inline"));
        assert!(manifest.components.lsp_servers.contains_key("rust"));
    }

    #[tokio::test]
    async fn channels_bind_declared_plugin_mcp_servers_and_preserve_user_config() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"channel-demo",
                "mcpServers":{"telegram":{"type":"stdio","command":"telegram-channel"}},
                "channels":[{
                    "server":"telegram",
                    "userConfig":{
                        "bot_token":{
                            "type":"string",
                            "title":"Bot token",
                            "description":"Telegram token",
                            "sensitive":true
                        }
                    }
                }]
            }"#,
        )
        .unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(manifest.channels.len(), 1);
        assert_eq!(manifest.channels[0].server, "telegram");
        let field = manifest.channels[0]
            .user_config
            .as_ref()
            .unwrap()
            .fields
            .get("bot_token")
            .unwrap();
        assert_eq!(field.value_type.as_deref(), Some("string"));
        assert_eq!(field.title.as_deref(), Some("Bot token"));
        assert!(field.sensitive);
    }

    #[tokio::test]
    async fn channels_cannot_bind_undeclared_or_duplicate_mcp_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"channel-demo",
                "mcpServers":{"telegram":{"type":"stdio","command":"telegram-channel"}},
                "channels":[
                    {"server":"telegram"},
                    {"server":"telegram"},
                    {"server":"user-server"}
                ]
            }"#,
        )
        .unwrap();

        let (_id, manifest) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(manifest.channels.len(), 1);
        assert_eq!(manifest.channels[0].server, "telegram");
    }
}
