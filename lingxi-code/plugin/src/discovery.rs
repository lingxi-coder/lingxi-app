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
use protocol::PluginId;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// A leading UTF-8 byte-order mark, as some editors on Windows write it.
/// `serde_json` does not treat U+FEFF as whitespace, so every manifest/config
/// JSON string read in this file must have it stripped before parsing (same
/// convention as `command-api::markdown_loader::UTF8_BOM`,
/// `skill-api::frontmatter`, `agent::catalog`).
const UTF8_BOM: char = '\u{feff}';

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
    /// Explicitly declared command directories, OR (commands-only) an object
    /// map of command name to `{source|content, description?, argumentHint?,
    /// model?, allowedTools?}`. Binary: `commands` (`Rs`, `union([path,
    /// path[], record(string, Ds)])` — the record form is commands-only;
    /// `skills`/`agents`/`outputStyles` stay `union([path, path[]])`, hence
    /// the separate [`CommandsDecl`] type instead of widening [`PathDecl`].
    #[serde(default)]
    commands: Option<CommandsDecl>,
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

/// The `commands` manifest field only: `union([path, path[], record(string,
/// Ds)])` (oracle `Rs`). Forked from [`PathDecl`] rather than widening it
/// because `skills`/`agents`/`outputStyles` stay `union([path, path[]])` —
/// serde tries an `untagged` enum's variants in declaration order, and `Map`
/// must come after `One`/`Many` since a JSON array/string can never satisfy
/// `Map`'s object shape, so this ordering is unambiguous either way.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CommandsDecl {
    One(String),
    Many(Vec<String>),
    Map(BTreeMap<String, RawCommandEntry>),
}

/// One value of the `commands` object-map form (oracle `Ds`). Exactly one of
/// `source` (a markdown file path) / `content` (inline markdown) must be
/// present — enforced in [`RawCommandEntry`]'s `Deserialize` impl below so a
/// violation fails the surrounding `serde_json::from_str::<RawManifest>` call
/// the same way any other malformed `plugin.json` does (byte-matching the
/// oracle's `.refine()`, which fails the WHOLE `PluginManifestSchema.parse`
/// on one bad command entry — not just that entry).
#[derive(Debug, Clone)]
struct RawCommandEntry {
    source: Option<String>,
    /// Inline markdown body. Recognized here to accept the oracle-valid
    /// shape (so it no longer sinks the whole plugin), but not yet
    /// materialized into a command: `PluginManager::load_plugin`
    /// (`plugin/src/manager.rs`, not owned by this change) always reads a
    /// component's markdown off disk via `ComponentPath::path` and has no
    /// slot for a file-less inline body.
    #[allow(dead_code)]
    content: Option<String>,
    /// Command description override. Not yet threaded to the materialized
    /// command (same manager.rs limitation as `content`); parsed so a
    /// declaring plugin still loads instead of vanishing.
    #[allow(dead_code)]
    description: Option<String>,
    #[allow(dead_code)]
    argument_hint: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    #[allow(dead_code)]
    allowed_tools: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for RawCommandEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            source: Option<String>,
            #[serde(default)]
            content: Option<String>,
            #[serde(default)]
            description: Option<String>,
            #[serde(rename = "argumentHint", default)]
            argument_hint: Option<String>,
            #[serde(default)]
            model: Option<String>,
            #[serde(rename = "allowedTools", default)]
            allowed_tools: Option<Vec<String>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        match (&raw.source, &raw.content) {
            (Some(_), None) | (None, Some(_)) => {}
            _ => {
                return Err(serde::de::Error::custom(
                    "Command must have either \"source\" (file path) or \"content\" \
                     (inline markdown), but not both",
                ))
            }
        }
        Ok(RawCommandEntry {
            source: raw.source,
            content: raw.content,
            description: raw.description,
            argument_hint: raw.argument_hint,
            model: raw.model,
            allowed_tools: raw.allowed_tools,
        })
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
        if let Some((id, manifest)) =
            load_plugin_from_path_with_mcp_gate(&versioned, false, Some(entry_id.as_str())).await
        {
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
                            if let Some((id, manifest)) =
                                load_plugin_from_path_with_mcp_gate(&dir, false, Some(key.as_str()))
                                    .await
                            {
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
                        let identifier = format!("{name}@{key}");
                        if let Some((id, manifest)) =
                            load_plugin_from_path_with_mcp_gate(&dir, false, Some(&identifier))
                                .await
                        {
                            out.push((identifier, id, manifest, dir));
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
    discover_cli_plugin_dirs_impl(paths, false).await
}

/// Sibling of [`discover_cli_plugin_dirs`] for plugin directories whose MCP
/// connections the CALLER already owns — oracle SDK-host
/// `extensionsConfig.inlinePluginsNoMcp()` (`M4()`), the `skipMcpDiscovery`
/// twin of `inlinePlugins()` (`L4()`, i.e. plain `discover_cli_plugin_dirs`).
/// This crate's stand-in entry point is the parsed-but-unwired
/// `--plugin-dir-no-mcp` flag (`apps/cli/src/argv.rs`'s
/// `plugin_dir_no_mcp` / `apps/cli/src/commands/agents.rs`'s twin) — wiring
/// it from `DesktopConfig` through to this function is a companion change,
/// out of this file's scope. Every plugin loaded through this path gets
/// [`PluginComponents::skip_mcp_discovery`] stamped `true`, so its
/// `.mcp.json` and manifest `mcpServers` are never read.
pub async fn discover_cli_plugin_dirs_no_mcp(
    paths: &[PathBuf],
) -> Vec<(PluginId, PluginManifest, PathBuf)> {
    discover_cli_plugin_dirs_impl(paths, true).await
}

async fn discover_cli_plugin_dirs_impl(
    paths: &[PathBuf],
    sdk_skip_mcp_discovery: bool,
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
        match load_plugin_from_path_with_mcp_gate(&plugin_root, sdk_skip_mcp_discovery, None).await
        {
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
/// Thin wrapper over [`load_plugin_from_path_with_mcp_gate`] for the common
/// case: no SDK-host `skipMcpDiscovery` request and no known
/// `name@marketplace` install-source identity. Kept so every existing call
/// site (production and test) is unaffected by the MCP-discovery gate added
/// for §3/§4.
pub(crate) async fn load_plugin_from_path(plugin_dir: &Path) -> Option<(PluginId, PluginManifest)> {
    load_plugin_from_path_with_mcp_gate(plugin_dir, false, None).await
}

/// Read + auto-detect a single plugin directory. Returns `None` when there is
/// no readable manifest (the directory is not a plugin).
///
/// Mirrors `createPluginFromPath` (`pluginLoader.ts:1348`): Step 1 loads the
/// manifest, Step 3 auto-detects the optional component directories.
///
/// `sdk_skip_mcp_discovery` is the oracle's per-plugin SDK-host
/// `skipMcpDiscovery` request (see [`PluginComponents::skip_mcp_discovery`]);
/// `install_source_id` is this plugin's `name@marketplace` install-source
/// identity when the caller resolved one (`discover_enabled_plugins` /
/// `discover_recorded_plugins_identified` always have one; an ad-hoc
/// directory load — `discover_installed_plugins`, `discover_cli_plugin_dirs`
/// — never does). Both feed [`resolve_skip_mcp_discovery`], which also
/// consults the process-wide `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` /
/// `_EXCEPT` env pair.
pub(crate) async fn load_plugin_from_path_with_mcp_gate(
    plugin_dir: &Path,
    sdk_skip_mcp_discovery: bool,
    install_source_id: Option<&str>,
) -> Option<(PluginId, PluginManifest)> {
    let manifest_path = plugin_dir
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
    let raw = tokio::fs::read_to_string(&manifest_path).await.ok()?;
    let raw = raw.strip_prefix(UTF8_BOM).unwrap_or(raw.as_str());
    let parsed: RawManifest = match serde_json::from_str(raw) {
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

    let skip_mcp_discovery =
        resolve_skip_mcp_discovery(&parsed.name, sdk_skip_mcp_discovery, install_source_id);
    let components = detect_components(plugin_dir, &parsed, skip_mcp_discovery).await;
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
///
/// `skip_mcp_discovery` mirrors oracle `OL` (`pluginLoader.ts` @160861000):
/// when set, NEITHER the plugin-root `.mcp.json` NOR the manifest's declared
/// `mcpServers` is read — every other component slot (commands, agents,
/// skills, output styles, hooks, LSP servers) still auto-detects/resolves
/// normally, matching the oracle's "leaves other components enabled". See
/// [`resolve_skip_mcp_discovery`] for how the caller decides this bit.
async fn detect_components(
    plugin_dir: &Path,
    parsed: &RawManifest,
    skip_mcp_discovery: bool,
) -> PluginComponents {
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
    let default_mcp_servers = if skip_mcp_discovery {
        HashMap::new()
    } else {
        load_mcp_servers(plugin_dir).await
    };
    let default_lsp_servers = load_lsp_servers(plugin_dir).await;

    let commands = match &parsed.commands {
        Some(decl) => resolve_commands_declared(plugin_dir, decl.clone()).await,
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
    if !skip_mcp_discovery {
        mcp_servers.extend(load_declared_mcp_servers(plugin_dir, parsed.mcp_servers.clone()).await);
    }
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
        skip_mcp_discovery,
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

/// Resolve the `commands` field for all three shapes `Rs` permits: a single
/// path / path array behaves exactly like the other markdown component
/// fields; the commands-only object-map form is handled by
/// [`resolve_command_entries`].
async fn resolve_commands_declared(plugin_dir: &Path, decl: CommandsDecl) -> Vec<ComponentPath> {
    match decl {
        CommandsDecl::One(raw) => {
            resolve_markdown_declared_paths(plugin_dir, PathDecl::One(raw)).await
        }
        CommandsDecl::Many(raws) => {
            resolve_markdown_declared_paths(plugin_dir, PathDecl::Many(raws)).await
        }
        CommandsDecl::Map(entries) => resolve_command_entries(plugin_dir, entries).await,
    }
}

/// Resolve the object-map form of `commands` (oracle `record(string, Ds)`):
/// each key names a slash command and its value carries either an on-disk
/// `source` markdown file or inline `content`.
///
/// `source` entries resolve to a real [`ComponentPath`] exactly like an
/// array entry, so they load correctly (the actual §6 fix — previously ANY
/// object-form `commands` value made deserialization of the whole
/// `plugin.json` fail, vanishing the plugin entirely regardless of shape).
///
/// `content` entries have no file for `PluginManager::load_plugin`
/// (`plugin/src/manager.rs`) to read — it always reads `ComponentPath::path`
/// off disk — so they are skipped with a diagnostic rather than silently
/// dropped or (worse) faked into an empty file. Wiring inline command bodies
/// (and honoring the map key as the resulting slash-command name, per the
/// oracle's `"about" → "/plugin:about"` behavior) requires a change to that
/// file and is deferred, not fixed here.
async fn resolve_command_entries(
    plugin_dir: &Path,
    entries: BTreeMap<String, RawCommandEntry>,
) -> Vec<ComponentPath> {
    let mut out = Vec::new();
    for (name, entry) in entries {
        match entry.source {
            Some(source) => {
                let Some(abs) = resolve_declared_relative_path(plugin_dir, &source) else {
                    tracing::warn!(
                        command = %name,
                        path = %source,
                        "skipping invalid plugin manifest command source path"
                    );
                    continue;
                };
                let root = abs.parent().unwrap_or(plugin_dir).to_path_buf();
                out.push(ComponentPath {
                    path: abs,
                    metadata: component_root_metadata(&root),
                });
            }
            None => {
                // The source/content refinement guarantees `content` is
                // `Some` here.
                tracing::warn!(
                    command = %name,
                    "skipping plugin command with inline `content` -- inline \
                     command bodies are not yet materialized"
                );
            }
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

// ---------- §3 + §4: plugin MCP-discovery suppression ----------
//
// Oracle `OL` (`pluginLoader.ts` @160861000) gates plugin MCP-server
// discovery in this order (built-ins aside — `if(e.isBuiltin)return` is not
// modeled here: nothing in this file's discovery paths ever produces a
// `PluginSource::BuiltIn` plugin, so the check has no reachable call site to
// guard):
//
//   1. an SDK-host per-plugin `skipMcpDiscovery:true` request always wins
//      (`PluginConfigSchema`'s `local` variant, @155779385) — the SDK host
//      owns this plugin's MCP connections itself, so the engine must not open
//      a second copy;
//   2. otherwise, `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` (raw JS-string
//      truthiness: any non-empty value, not `isEnvTruthy`'s enumerated set —
//      confirmed against the oracle's own `if(a.CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS)`
//      bare property check) suppresses discovery for every non-built-in
//      plugin;
//   3. UNLESS `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT` (oracle `SCn`, same
//      offset) names this plugin: comma-split, each trimmed entry containing
//      `@` matches the plugin's `name@marketplace` install-source identity
//      case-insensitively (oracle `qy`: `e===n||e.toLowerCase()===n.toLowerCase()`);
//      an entry WITHOUT `@` matches the bare plugin name, but only when this
//      plugin has a known install-source identity at all (oracle `!pM(e)` —
//      `pM` gates the same "directory-loaded plugin" confinement checks a few
//      lines later in `OL`, i.e. a raw `--plugin-dir`/ad-hoc directory load
//      has no marketplace pedigree to match a bare name against).
//
// LINGXI_-prefixed aliases are accepted first, per this crate's env
// convention (`tools/ui/src/brief.rs`'s `LINGXI_BRIEF`/`CLAUDE_CODE_BRIEF`
// pair is the precedent this mirrors byte-for-byte on the truthiness rule).

/// Case-insensitive exact match — oracle `qy(e,n)`:
/// `e===n||e.toLowerCase()===n.toLowerCase()`.
fn qy_eq(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// Raw JS-string truthiness for an env var: unset or empty is falsy, any
/// other value (including `"0"`/`"false"`) is truthy. Distinct from
/// `traits::env::is_env_truthy`'s stricter `1|true|yes|on` allowlist — the
/// oracle reads `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` as a bare
/// `process.env` property, not through `isEnvTruthy`.
fn env_set_nonempty(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.is_empty())
}

fn env_value_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` (+ `LINGXI_` alias) presence check.
fn skip_plugin_mcp_servers_env_set() -> bool {
    env_set_nonempty("LINGXI_SKIP_PLUGIN_MCP_SERVERS")
        || env_set_nonempty("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS")
}

/// `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT` (+ `LINGXI_` alias) value.
fn skip_plugin_mcp_servers_except() -> Option<String> {
    env_value_nonempty("LINGXI_SKIP_PLUGIN_MCP_SERVERS_EXCEPT")
        .or_else(|| env_value_nonempty("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT"))
}

/// Oracle `SCn(e)`: does the `_EXCEPT` list re-admit this plugin past an
/// active `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` suppression?
fn except_exempts(name: &str, install_source_id: Option<&str>) -> bool {
    let Some(except) = skip_plugin_mcp_servers_except() else {
        return false;
    };
    // Oracle `r=!pM(e)`: bare-name matching is only meaningful for a plugin
    // resolved through a marketplace-qualified install (this crate's stand-in
    // for "not directory-loaded" — see the module note above).
    except.split(',').any(|raw| {
        let entry = raw.trim();
        if entry.is_empty() {
            return false;
        }
        if entry.contains('@') {
            install_source_id.is_some_and(|id| qy_eq(entry, id))
        } else {
            install_source_id.is_some() && qy_eq(entry, name)
        }
    })
}

/// Resolve whether this plugin's MCP server discovery (`.mcp.json` + manifest
/// `mcpServers`) should be suppressed for this load. See the module note
/// above for the full oracle-derived order; `name` is the manifest's declared
/// `name` (`RawManifest::name`), not a display label.
fn resolve_skip_mcp_discovery(
    name: &str,
    sdk_skip_mcp_discovery: bool,
    install_source_id: Option<&str>,
) -> bool {
    if sdk_skip_mcp_discovery {
        return true;
    }
    if !skip_plugin_mcp_servers_env_set() {
        return false;
    }
    !except_exempts(name, install_source_id)
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
    let raw = raw.strip_prefix(UTF8_BOM).unwrap_or(raw.as_str());
    // Plugin MCP servers are dynamic-scoped (`addPluginScopeToServers` uses
    // `scope: 'dynamic'`, `mcpPluginIntegration.ts:353`).
    match mcp::parse_mcp_json_string(raw, mcp::ConfigScope::Dynamic) {
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
async fn load_lsp_servers(plugin_dir: &Path) -> HashMap<String, traits::LspServerConfig> {
    let path = plugin_dir.join(".lsp.json");
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return HashMap::new();
    };
    let raw = raw.strip_prefix(UTF8_BOM).unwrap_or(raw.as_str());
    let parsed: HashMap<String, traits::LspServerConfig> = match serde_json::from_str(raw) {
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
    let raw = raw.strip_prefix(UTF8_BOM).unwrap_or(raw.as_str());
    let wrapper: serde_json::Value = match serde_json::from_str(raw) {
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
        // Oracle `zs`: `hooks` is `union([path, inlineObject, array(union([path,
        // inlineObject]))])` — each ARRAY ITEM independently is a path or an
        // inline hooks object; a mixed array (some string paths, some inline
        // objects) is valid. Handle every item on its own merits instead of
        // requiring the whole array to be homogeneous, which previously threw
        // away ALL entries (both the paths and the inline objects) the moment
        // one item wasn't a string.
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(path) => {
                        out.extend(load_declared_hooks_from_path(plugin_dir, &path).await);
                    }
                    other => out.extend(parse_hooks_value(&other, plugin_dir)),
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
    let raw_json = raw_json.strip_prefix(UTF8_BOM).unwrap_or(raw_json.as_str());
    let Ok(value) = serde_json::from_str::<Value>(raw_json) else {
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
) -> HashMap<String, traits::LspServerConfig> {
    load_declared_json_records(plugin_dir, value, |raw| {
        serde_json::from_str::<HashMap<String, traits::LspServerConfig>>(raw).map(|parsed| {
            parsed
                .into_iter()
                .map(|(key, mut cfg)| {
                    if cfg.name.is_empty() {
                        cfg.name.clone_from(&key);
                    }
                    (cfg.name.clone(), cfg)
                })
                .collect::<HashMap<_, _>>()
        })
    })
    .await
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
        // Oracle `js`/`Ws`: `mcpServers`/`lspServers` accept
        // `array(union([path, inlineRecord]))` — each item is independently
        // a path OR an inline `{name: config}` record, so a mixed array (some
        // paths, some inline servers) is valid. Resolve every item on its own
        // terms instead of requiring the whole array to be all-string, which
        // previously dropped EVERY entry (paths included) as soon as one item
        // was an inline object.
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(path) => {
                        merge_declared_json_records(plugin_dir, &path, &parse, &mut out).await;
                    }
                    other => {
                        if let Ok(parsed) = parse(&other.to_string()) {
                            out.extend(parsed);
                        }
                    }
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
    let raw = raw.strip_prefix(UTF8_BOM).unwrap_or(raw.as_str());
    if let Ok(parsed) = parse(raw) {
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
                "lspServers":{"rust":{"name":"rust","command":"rust-analyzer","args":[],"env":{},"trigger_languages":["rust"],"root_dir_markers":["Cargo.toml"],"initialization_options":null,"extension_to_language":{}}}
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

    // ---------- UTF-8 BOM tolerance (§5, 2.1.246 upstream fix) ----------
    //
    // serde_json does not treat U+FEFF as whitespace, so a BOM-prefixed
    // manifest/config file previously failed to deserialize and the whole
    // plugin (or the individual component file) was silently dropped. Each
    // test below prefixes the exact fixture used by an existing non-BOM test
    // above with `\u{feff}` and asserts the same successful outcome.

    #[tokio::test]
    async fn plugin_json_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            "\u{feff}{\"name\":\"demo\"}",
        )
        .unwrap();

        let loaded = load_plugin_from_path(plugin).await;
        assert!(
            loaded.is_some(),
            "BOM-prefixed plugin.json must still deserialize instead of being skipped as malformed"
        );
        assert_eq!(loaded.unwrap().1.name, "demo");
    }

    #[tokio::test]
    async fn root_mcp_json_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join(".mcp.json"),
            "\u{feff}{\"echo\":{\"type\":\"stdio\",\"command\":\"echo\"}}",
        )
        .unwrap();

        let servers = load_mcp_servers(plugin).await;
        assert!(
            servers.contains_key("echo"),
            "BOM-prefixed root .mcp.json must still parse its server map, got {servers:?}"
        );
    }

    #[tokio::test]
    async fn root_lsp_json_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join(".lsp.json"),
            "\u{feff}{\"rust\":{\"name\":\"rust\",\"command\":\"rust-analyzer\",\"args\":[],\"env\":{},\"trigger_languages\":[\"rust\"],\"root_dir_markers\":[\"Cargo.toml\"],\"initialization_options\":null,\"extension_to_language\":{}}}",
        )
        .unwrap();

        let servers = load_lsp_servers(plugin).await;
        assert!(
            servers.contains_key("rust"),
            "BOM-prefixed root .lsp.json must still parse its server map, got {servers:?}"
        );
    }

    #[tokio::test]
    async fn standard_hooks_json_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join("hooks")).unwrap();
        fs::write(
            plugin.join("hooks").join("hooks.json"),
            "\u{feff}{\"hooks\":{\"PreToolUse\":[{\"matcher\":\"Write\",\"hooks\":[{\"type\":\"command\",\"command\":\"./fmt.sh\"}]}]}}",
        )
        .unwrap();

        let hooks = load_standard_hooks(plugin).await;
        assert_eq!(
            hooks.len(),
            1,
            "BOM-prefixed hooks/hooks.json must still parse instead of yielding zero hooks"
        );
    }

    #[tokio::test]
    async fn declared_hooks_path_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join("custom-hooks.json"),
            "\u{feff}{\"hooks\":{\"PreToolUse\":[{\"matcher\":\"Write\",\"hooks\":[{\"type\":\"command\",\"command\":\"./fmt.sh\"}]}]}}",
        )
        .unwrap();

        let hooks = load_declared_hooks_from_path(plugin, "./custom-hooks.json").await;
        assert_eq!(
            hooks.len(),
            1,
            "BOM-prefixed manifest-declared hooks file must still parse instead of yielding zero hooks"
        );
    }

    #[tokio::test]
    async fn declared_mcp_servers_path_with_utf8_bom_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join("custom-mcp.json"),
            "\u{feff}{\"echo\":{\"type\":\"stdio\",\"command\":\"echo\"}}",
        )
        .unwrap();

        let servers = load_declared_mcp_servers(
            plugin,
            Some(Value::String("./custom-mcp.json".to_string())),
        )
        .await;
        assert!(
            servers.contains_key("echo"),
            "BOM-prefixed manifest-declared mcpServers file must still parse, got {servers:?}"
        );
    }

    // ---------- §6: object-form `commands` map + mixed path/inline arrays ----------
    //
    // Oracle `Rs`: `commands: union([path, path[], record(string, Ds)])`. The
    // port's `PathDecl` was `untagged {One(String), Many(Vec<String>)}` with no
    // object-map arm, so ANY object-form `commands` value failed to
    // deserialize `RawManifest` as a whole and `load_plugin_from_path` silently
    // dropped the entire plugin, not just its `commands`. The fix forks a
    // `commands`-only `CommandsDecl` carrying the object-map arm (`Ds`), while
    // `skills`/`agents`/`outputStyles` keep the narrower `PathDecl` (the oracle
    // does not offer a record form for those).

    #[tokio::test]
    async fn commands_object_map_resolves_source_entries_without_dropping_the_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin.join("cmds")).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"demo",
                "commands":{
                    "about":{"source":"./cmds/about.md","description":"About this plugin"},
                    "inline-only":{"content":"Inline body, no file"}
                }
            }"#,
        )
        .unwrap();
        fs::write(plugin.join("cmds/about.md"), "About body").unwrap();

        let loaded = load_plugin_from_path(plugin).await;
        assert!(
            loaded.is_some(),
            "an object-form `commands` map with valid Ds entries must not delete the whole plugin (the §6 defect)"
        );
        let (_id, manifest) = loaded.unwrap();
        let commands: Vec<_> = manifest
            .components
            .commands
            .iter()
            .map(|c| c.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            commands,
            vec!["about.md"],
            "the `source`-backed entry must resolve to its markdown file; the \
             `content`-only entry has no on-disk file to materialize and is \
             skipped rather than fabricated, got {commands:?}"
        );
    }

    #[tokio::test]
    async fn commands_object_map_entry_with_both_source_and_content_drops_whole_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin.join("cmds")).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"demo",
                "commands":{"bad":{"source":"./cmds/about.md","content":"Also inline"}}
            }"#,
        )
        .unwrap();
        fs::write(plugin.join("cmds/about.md"), "About body").unwrap();

        let loaded = load_plugin_from_path(plugin).await;
        assert!(
            loaded.is_none(),
            "a command entry with BOTH source and content violates the oracle's \
             refine (\"Command must have either source or content, but not both\") \
             and must fail manifest parsing like any other malformed plugin.json"
        );
    }

    #[tokio::test]
    async fn commands_object_map_entry_with_neither_source_nor_content_drops_whole_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{
                "name":"demo",
                "commands":{"bad":{"description":"missing both source and content"}}
            }"#,
        )
        .unwrap();

        let loaded = load_plugin_from_path(plugin).await;
        assert!(
            loaded.is_none(),
            "a command entry with NEITHER source nor content violates the oracle's \
             refine and must fail manifest parsing, not silently load with an empty command"
        );
    }

    #[tokio::test]
    async fn declared_mcp_servers_mixed_array_merges_path_and_inline_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join("extra-mcp.json"),
            r#"{"file-server":{"type":"stdio","command":"echo"}}"#,
        )
        .unwrap();

        let value = serde_json::json!([
            "./extra-mcp.json",
            {"inline-server": {"type": "stdio", "command": "echo"}}
        ]);

        let servers = load_declared_mcp_servers(plugin, Some(value)).await;
        assert!(
            servers.contains_key("file-server") && servers.contains_key("inline-server"),
            "a mixed [path, inlineObject] array (oracle `js`) must merge BOTH the \
             file-loaded and inline server entries, got {servers:?}"
        );
    }

    #[tokio::test]
    async fn declared_lsp_servers_mixed_array_merges_path_and_inline_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join("extra-lsp.json"),
            r#"{"rust":{"name":"rust","command":"rust-analyzer","args":[],"env":{},"trigger_languages":["rust"],"root_dir_markers":["Cargo.toml"],"initialization_options":null,"extension_to_language":{}}}"#,
        )
        .unwrap();

        let value = serde_json::json!([
            "./extra-lsp.json",
            {"python": {"name": "python", "command": "pyright", "args": [], "env": {}, "trigger_languages": ["python"], "root_dir_markers": ["pyproject.toml"], "initialization_options": null, "extension_to_language": {}}}
        ]);

        let servers = load_declared_lsp_servers(plugin, Some(value)).await;
        assert!(
            servers.contains_key("rust") && servers.contains_key("python"),
            "a mixed [path, inlineObject] array (oracle `Ws`) must merge BOTH the \
             file-loaded and inline lsp entries, got {servers:?}"
        );
    }

    #[tokio::test]
    async fn declared_hooks_mixed_array_merges_path_and_inline_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        fs::write(
            plugin.join("extra-hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"./fmt.sh"}]}]}}"#,
        )
        .unwrap();

        let value = serde_json::json!([
            "./extra-hooks.json",
            {"PostToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "./log.sh"}]}]}
        ]);

        let hooks = load_declared_hooks(plugin, Some(value)).await;
        assert_eq!(
            hooks.len(),
            2,
            "a mixed [path, inlineObject] array (oracle `zs`) must merge BOTH the \
             file-loaded and inline hook entries, got {hooks:?}"
        );
    }

    // ---------- §3 + §4: plugin MCP-discovery suppression ----------

    /// A plugin whose `.mcp.json`, manifest `mcpServers`, and a default
    /// `commands/` dir are all present, so a test can assert the MCP slot was
    /// suppressed while every other component slot still loaded.
    fn write_plugin_with_mcp_and_command(plugin: &Path, name: &str) {
        fs::create_dir_all(plugin.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin.join("commands")).unwrap();
        fs::write(
            plugin
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            format!(
                r#"{{
                    "name":"{name}",
                    "mcpServers":{{"declared":{{"type":"stdio","command":"echo"}}}}
                }}"#
            ),
        )
        .unwrap();
        fs::write(plugin.join("commands/hello.md"), "hello").unwrap();
        fs::write(
            plugin.join(".mcp.json"),
            r#"{"root":{"type":"stdio","command":"echo"}}"#,
        )
        .unwrap();
    }

    /// Serializes the `_env`-mutating tests in this block and resets the four
    /// env vars ([`resolve_skip_mcp_discovery`]'s two names, each with its
    /// `LINGXI_`/`CLAUDE_CODE_` alias) to "unset" at entry. Mirrors
    /// `tools/ui/src/brief.rs`'s `brief_guard` / `push_notification.rs`'s
    /// `guard`.
    fn skip_mcp_env_guard() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("LINGXI_SKIP_PLUGIN_MCP_SERVERS");
        std::env::remove_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS");
        std::env::remove_var("LINGXI_SKIP_PLUGIN_MCP_SERVERS_EXCEPT");
        std::env::remove_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT");
        g
    }

    #[tokio::test]
    async fn sdk_skip_mcp_discovery_suppresses_mcp_but_not_other_components() {
        let _g = skip_mcp_env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        write_plugin_with_mcp_and_command(plugin, "demo");

        // No env suppression active — the SDK-host `skipMcpDiscovery:true`
        // request alone must suppress BOTH `.mcp.json` and manifest
        // `mcpServers`, while `commands/` still auto-detects (oracle: "leaves
        // other components enabled").
        let (_id, manifest) = load_plugin_from_path_with_mcp_gate(plugin, true, None)
            .await
            .unwrap();
        assert!(
            manifest.components.mcp_servers.is_empty(),
            "skipMcpDiscovery must drop both .mcp.json and declared mcpServers, got {:?}",
            manifest.components.mcp_servers
        );
        assert!(manifest.components.skip_mcp_discovery);
        assert_eq!(
            manifest.components.commands.len(),
            1,
            "non-MCP components must still load when only MCP discovery is skipped"
        );

        // REVERT proof: the same fixture with `sdk_skip_mcp_discovery: false`
        // (the plain wrapper every pre-existing call site uses) must load
        // BOTH servers — showing the assertions above are actually exercising
        // the gate, not tautologically true for this fixture.
        let (_id2, unskipped) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(unskipped.components.mcp_servers.len(), 2);
        assert!(!unskipped.components.skip_mcp_discovery);
    }

    #[tokio::test]
    async fn skip_plugin_mcp_servers_env_suppresses_discovery_for_every_plugin() {
        let _g = skip_mcp_env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        write_plugin_with_mcp_and_command(plugin, "demo");

        // Baseline: no env set, no install-source id — normal discovery.
        let (_id, baseline) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(baseline.components.mcp_servers.len(), 2);

        // `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` set to ANY non-empty value
        // (raw JS truthiness, not `isEnvTruthy`'s enumerated set) suppresses
        // MCP discovery even though nothing requested `skipMcpDiscovery` and
        // no `_EXCEPT` was given.
        std::env::set_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS", "0");
        let (_id, suppressed) = load_plugin_from_path(plugin).await.unwrap();
        assert!(
            suppressed.components.mcp_servers.is_empty(),
            "CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS=\"0\" is a non-empty string and must \
             still suppress discovery (plain process.env truthiness)"
        );
        assert!(suppressed.components.skip_mcp_discovery);

        // Unsetting restores discovery — the env var, not something else
        // about the fixture, is what suppressed it above.
        std::env::remove_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS");
        let (_id, restored) = load_plugin_from_path(plugin).await.unwrap();
        assert_eq!(restored.components.mcp_servers.len(), 2);

        // The `LINGXI_` alias is honored too.
        std::env::set_var("LINGXI_SKIP_PLUGIN_MCP_SERVERS", "1");
        let (_id, suppressed_alias) = load_plugin_from_path(plugin).await.unwrap();
        assert!(suppressed_alias.components.mcp_servers.is_empty());
    }

    #[tokio::test]
    async fn except_with_at_sign_matches_install_source_id_directory_loaded_or_not() {
        let _g = skip_mcp_env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        write_plugin_with_mcp_and_command(plugin, "demo");
        std::env::set_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS", "1");
        std::env::set_var(
            "CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT",
            "other@mkt, demo@marketplace-x",
        );

        // An "@"-containing entry matches the plugin's `name@marketplace`
        // install-source identity and re-admits it past the suppression.
        let (_id, exempted) =
            load_plugin_from_path_with_mcp_gate(plugin, false, Some("demo@marketplace-x"))
                .await
                .unwrap();
        assert_eq!(
            exempted.components.mcp_servers.len(),
            2,
            "an _EXCEPT entry naming this plugin's install-source id must re-admit it"
        );
        assert!(!exempted.components.skip_mcp_discovery);

        // A non-matching install-source id stays suppressed.
        let (_id, still_suppressed) =
            load_plugin_from_path_with_mcp_gate(plugin, false, Some("demo@some-other-marketplace"))
                .await
                .unwrap();
        assert!(still_suppressed.components.mcp_servers.is_empty());
    }

    #[tokio::test]
    async fn except_bare_name_only_exempts_non_directory_loaded_plugins() {
        let _g = skip_mcp_env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let plugin = tmp.path();
        write_plugin_with_mcp_and_command(plugin, "demo");
        std::env::set_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS", "1");
        std::env::set_var("CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS_EXCEPT", "demo");

        // Known install-source id (this crate's "not directory-loaded") — a
        // bare-name `_EXCEPT` entry matches `manifest.name` and re-admits it.
        let (_id, exempted) =
            load_plugin_from_path_with_mcp_gate(plugin, false, Some("demo@marketplace-x"))
                .await
                .unwrap();
        assert_eq!(exempted.components.mcp_servers.len(), 2);

        // No install-source id (an ad-hoc directory load, e.g. `--plugin-dir`)
        // — the oracle restricts bare-name matching to non-directory-loaded
        // plugins, so the same name must NOT re-admit it here even though
        // `manifest.name == "demo"` matches the `_EXCEPT` entry exactly.
        let (_id, still_suppressed) = load_plugin_from_path_with_mcp_gate(plugin, false, None)
            .await
            .unwrap();
        assert!(
            still_suppressed.components.mcp_servers.is_empty(),
            "a bare-name _EXCEPT entry must not exempt a directory-loaded plugin \
             (oracle `r=!pM(e)`), got {:?}",
            still_suppressed.components.mcp_servers
        );
    }
}
