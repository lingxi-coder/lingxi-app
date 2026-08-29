//! Plugin manifest — the Claude-Code-compatible component surface.
//!
//! A `PluginManifest` is the canonical description of a plugin: identity,
//! source provenance, declared components (commands / agents / skills /
//! hooks / output styles / MCP servers / LSP servers), trust level, and
//! optional user-config schema. Materialization into the 8 engine
//! registries is handled by [`crate::manager::PluginManager::load_plugin`].
//!
//! See spec §15.1.

use crate::source::PluginSource;
use crate::trust::PluginTrustLevel;
use crate::PluginDependency;
use hooks::HookDefinition;
use mcp::McpServerConfig;
use protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use traits::LspServerConfig;

fn default_plugin_enabled() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// Top-level plugin manifest.
///
/// Loaded once at install/enable time and held by the [`crate::lifecycle::PluginState`]
/// machine throughout the plugin's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Engine-assigned stable identifier.
    pub id: PluginId,
    /// Human-readable plugin name.
    pub name: String,
    /// Optional display label used by plugin-facing UI. Falls back to
    /// [`Self::name`] when absent and never participates in namespacing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Fallback activation state when no `enabledPlugins` scope has an
    /// explicit value for this plugin.
    #[serde(default = "default_plugin_enabled", skip_serializing_if = "is_true")]
    pub default_enabled: bool,
    /// Semver-style version string.
    pub version: String,
    /// Free-form description.
    pub description: String,
    /// Optional author string.
    pub author: Option<String>,
    /// Optional homepage / repo URL.
    pub homepage: Option<String>,
    /// Where the plugin came from.
    pub source: PluginSource,
    /// Declared components (the 7-tuple materialised into engine registries).
    pub components: PluginComponents,
    /// Trust classification (defaults to [`crate::trust::default_trust_for_source`]).
    pub trust_level: PluginTrustLevel,
    /// Plugin ids this plugin depends on.
    pub depends_on: Vec<PluginId>,
    /// Versioned dependency declarations from `plugin.json`.
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,
    /// Optional user-config schema. Sensitive fields are resolved through
    /// the [`secret::CredentialManager`] at load time.
    pub user_config: Option<UserConfigSchema>,
    /// Plugin-declared channels (each binds an MCP server to a channel name).
    pub channels: Vec<PluginChannel>,
    /// Free-form settings the plugin author wants to ship with the manifest.
    pub settings: HashMap<String, serde_json::Value>,
}

/// The 7 component slots a plugin can populate.
///
/// Each slot is materialized into its matching engine registry by
/// [`crate::manager::PluginManager::load_plugin`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginComponents {
    /// Markdown slash-command files.
    pub commands: Vec<ComponentPath>,
    /// Markdown agent files.
    pub agents: Vec<ComponentPath>,
    /// Markdown skill files.
    pub skills: Vec<ComponentPath>,
    /// Output-style files.
    pub output_styles: Vec<ComponentPath>,
    /// Inline hook definitions.
    pub hooks: Vec<HookDefinition>,
    /// MCP servers contributed by this plugin, keyed by logical name. Always
    /// empty when [`Self::skip_mcp_discovery`] is `true`.
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// LSP servers contributed by this plugin, keyed by logical name.
    pub lsp_servers: HashMap<String, LspServerConfig>,
    /// Whether this load suppressed MCP server discovery for this plugin —
    /// neither the plugin-root `.mcp.json` nor the manifest's declared
    /// `mcpServers` was read, so [`Self::mcp_servers`] is empty regardless of
    /// what the plugin actually declares. Every other component slot still
    /// loads normally.
    ///
    /// Set when an SDK host declared this plugin instance with
    /// `skipMcpDiscovery: true` (it owns the plugin's MCP connections itself
    /// — oracle `PluginConfigSchema`'s `local` variant, @155779385: *"the
    /// engine loads skills/hooks/agents/commands from this plugin but does
    /// NOT read its .mcp.json or manifest mcpServers"*), or when
    /// `CLAUDE_CODE_SKIP_PLUGIN_MCP_SERVERS` / `LINGXI_SKIP_PLUGIN_MCP_SERVERS`
    /// suppressed discovery process-wide and this plugin was not exempted via
    /// the `_EXCEPT` sibling (see `discovery::resolve_skip_mcp_discovery`).
    ///
    /// Deliberately lives here rather than on [`PluginManifest`] itself: this
    /// struct derives `Default` and its one construction site
    /// (`discovery::detect_components`) is fully owned by the same change, so
    /// adding a field cannot break another `PluginManifest { .. }` literal
    /// elsewhere in the crate that lists every field by hand and would
    /// otherwise need updating too (`plugin/src/loader.rs`'s test fixture,
    /// notably — outside this change's file ownership). A later telemetry
    /// batch reads this to derive the oracle's `has_mcp`
    /// (`!skip_mcp_discovery && !mcp_servers.is_empty()`) and `host_owned_mcp`
    /// (`skip_mcp_discovery`) fields; this change adds no telemetry itself.
    #[serde(default)]
    pub skip_mcp_discovery: bool,
}

/// On-disk component reference plus arbitrary metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentPath {
    /// Path to the component file (relative to the plugin install dir).
    pub path: PathBuf,
    /// Optional manifest-side metadata for the component.
    pub metadata: Option<serde_json::Value>,
}

/// User-config schema declared by a plugin.
///
/// Sensitive fields are routed through [`secret::CredentialManager`]'s
/// plugin-secret storage; non-sensitive values are pulled from the settings-file
/// `pluginConfigs[plugin].options` map (falling back to a field's declared
/// `default`) at load time. See [`crate::loader::resolve_user_config`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfigSchema {
    /// Map of field name to declared field.
    pub fields: HashMap<String, UserConfigField>,
}

/// One declared user-config field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfigField {
    /// Declared input type (`string`, `number`, `boolean`, `directory`, or
    /// `file`). Kept as a string so a newer Claude Code field type remains
    /// forward-compatible instead of making an installed plugin unloadable.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    /// Label shown in the configuration dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Help text for the host UI.
    #[serde(default)]
    pub description: String,
    /// If `true` the value is fetched through the secret-storage backend.
    #[serde(default)]
    pub sensitive: bool,
    /// If `true` the loader fails when the value is missing.
    #[serde(default)]
    pub required: bool,
    /// Optional default value applied when no configured value is present
    /// (non-sensitive only — secrets are never defaulted). Mirrors claude-code's
    /// `if(p.default!==void 0)u[d]=p.default` substitution-context seeding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Whether a string field accepts multiple values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple: Option<bool>,
    /// Optional numeric lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Optional numeric upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// A plugin's persisted `userConfig` state at one settings scope: the
/// on-disk `pluginConfigs[plugin]` object.
///
/// Byte-parity with claude-code's zod
/// `pluginConfigs:record(string,object({mcpServers,options}))`: `options` holds
/// top-level non-sensitive userConfig values; `mcp_servers` holds per-server
/// overrides keyed by logical server name. Sensitive values are NEVER stored
/// here — they live in secure storage (see [`secret::CredentialManager`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginUserConfig {
    /// Top-level non-sensitive userConfig values (`options`).
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
    /// Per-server non-sensitive overrides (`mcpServers[server]`).
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, serde_json::Map<String, serde_json::Value>>,
}

impl PluginUserConfig {
    /// Extract the `pluginConfigs` map from a settings-file JSON object (the
    /// shape [`migrations::settings_update::read_settings_map`] returns),
    /// yielding `plugin-id -> PluginUserConfig`. Missing / malformed entries are
    /// skipped. This is the read side of the settings `pluginConfigs` scope.
    #[must_use]
    pub fn from_settings_map(
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> HashMap<String, PluginUserConfig> {
        let mut out = HashMap::new();
        let Some(configs) = settings.get("pluginConfigs").and_then(|v| v.as_object()) else {
            return out;
        };
        for (plugin, entry) in configs {
            if let Ok(parsed) = serde_json::from_value::<PluginUserConfig>(entry.clone()) {
                out.insert(plugin.clone(), parsed);
            }
        }
        out
    }
}

/// A channel the plugin binds to an MCP server.
///
/// This is the public `plugin.json` shape used by Claude Code 2.1.220. There
/// is no separate channel name: the referenced MCP server identifies the
/// channel, and optional per-channel configuration uses the same schema as
/// top-level plugin `userConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginChannel {
    /// MCP server name (must match a key in [`PluginComponents::mcp_servers`]).
    pub server: String,
    /// Optional channel-scoped user-config schema.
    #[serde(
        rename = "userConfig",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub user_config: Option<UserConfigSchema>,
}
