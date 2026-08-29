//! Plugin subsystem — manifest model, 7-state lifecycle, blocklist, strict
//! policy, agent-frontmatter privilege validation, and the 8-registry
//! materialiser.
//!
//! Plugins are the most cross-cutting subsystem in the engine: every
//! component slot they declare (commands / agents / skills / hooks /
//! output styles / MCP servers / LSP servers) eventually lands in a
//! dedicated registry built by Plans 03 / 04 / 09 / 12. This crate owns
//! the lifecycle and the materialiser; Plan 16 wires the actual fetches.
//!
//! See spec §15 (Plugin System).

#![forbid(unsafe_code)]

pub mod agent_validation;
pub mod blocklist;
pub mod dependency;
pub mod discovery;
mod git;
pub mod installed;
pub mod lifecycle;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod marketplace;
mod mcpb;
pub mod source;
pub mod strict_policy;
pub mod theme_registry;
pub mod trust;
/// `${user_config.KEY}` substitution + plugin-option env helpers.
///
/// Re-exported from the `hooks` crate, which owns the single source of truth
/// (the plugin-hook executor needs the same substitution/gate logic at spawn
/// time, and `hooks` sits below `plugin` in the dependency graph). `plugin`'s
/// MCP/LSP loader continues to consume it as `plugin::user_config::*`.
pub use hooks::user_config;

pub use agent_validation::{validate_plugin_agent_frontmatter, AgentValidationError};
pub use blocklist::PluginBlocklist;
pub use dependency::{
    merge_dependency_requirements, parse_dependencies, version_satisfies_all, PluginDependency,
};
pub use discovery::{
    discover_cli_plugin_dirs, discover_effective_plugins, discover_enabled_plugins,
    discover_installed_plugins, discover_recorded_plugins,
};
pub use git::clone_plugin_git;
pub use lifecycle::PluginState;
pub use loader::{resolve_user_config, LoaderError};
pub use manager::{PluginManager, PluginManagerError};
pub use manifest::{
    ComponentPath, PluginChannel, PluginComponents, PluginManifest, PluginUserConfig,
    UserConfigField, UserConfigSchema,
};
pub use marketplace::MarketplaceManager;
/// Normalize an extracted MCP bundle into the shared plugin manifest layout.
pub use mcpb::ensure_plugin_manifest;
/// Stable content hash used for deterministic external-source cache keys.
pub use mcpb::sha256_hex as plugin_source_sha256;
/// Guarded zip extraction used by both installed MCP bundles and session-only
/// `--plugin-url` archives.
pub use mcpb::unpack_mcpb as unpack_plugin_archive;
pub use source::PluginSource;
pub use strict_policy::{PluginComponent, StrictPluginOnlyPolicy};
pub use theme_registry::{PluginThemeEntry, PluginThemeRegistry};
pub use trust::{default_trust_for_source, PluginTrustLevel};
