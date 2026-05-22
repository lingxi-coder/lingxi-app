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
use lingxi_hooks::HookDefinition;
use lingxi_mcp::McpServerConfig;
use lingxi_protocol::PluginId;
use lingxi_traits::LspServerConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Optional user-config schema. Sensitive fields are resolved through
    /// the [`lingxi_secret::CredentialManager`] at load time.
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
    /// MCP servers contributed by this plugin, keyed by logical name.
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// LSP servers contributed by this plugin, keyed by logical name.
    pub lsp_servers: HashMap<String, LspServerConfig>,
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
/// Sensitive fields are routed through [`lingxi_secret::CredentialManager`];
/// non-sensitive required fields are pulled from
/// [`PluginManifest::settings`] at load time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfigSchema {
    /// Map of field name to declared field.
    pub fields: HashMap<String, UserConfigField>,
}

/// One declared user-config field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfigField {
    /// Help text for the host UI.
    pub description: String,
    /// If `true` the value is fetched through the secret-storage backend.
    pub sensitive: bool,
    /// If `true` the loader fails when the value is missing.
    pub required: bool,
}

/// A channel the plugin binds to an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginChannel {
    /// Channel name.
    pub name: String,
    /// MCP server name (must match a key in [`PluginComponents::mcp_servers`]).
    pub mcp_server: String,
    /// Optional channel-scoped user-config schema.
    pub user_config: Option<UserConfigSchema>,
}
