//! MCP (Model Context Protocol) connection management for `LingXi` Core.
//!
//! Owns the per-connection state machine (`connection.rs`), the
//! [`registry::McpRegistry`] that drives transitions through a
//! platform-supplied [`traits::McpTransport`], the OAuth 2.1
//! handshake skeleton (`oauth.rs`), the per-project MCP-server enable/
//! disable and project-`.mcp.json`-approval gate (`server_gate.rs`), and the
//! `client::McpClient` JSON-RPC client built on top of `lingxi-jsonrpc`
//! (M2-02b — see `docs/superpowers/plans/2026-05-23-m2-02b-mcp-client.md`).
//!
//! There used to be a separate `agent_scope.rs` holding an
//! `AgentScopedConnections` table for a subagent's own inline
//! `mcpServers` (§24b). Both the scaffolding and the connect/inject/
//! teardown chain built on top of it were removed: the shared registry
//! is keyed by server NAME and the model-facing tool FQN is
//! `mcp__<name>__<tool>`, so the per-agent name mangling that kept two
//! subagents from colliding also produced `mcp____agent_scope__<uuid>__
//! <server>__<tool>` — an FQN whose server segment parses EMPTY, which
//! `tools/mcp::parse_full_name` rejects outright, `servers_with_tools`
//! drops, and no `mcp__<server>` permission rule can match. A working
//! §24b needs the registry key and the FQN/OAuth-key name to be
//! separable, plus a per-spawn dispatch overlay; see the revert commit.
//!
//! There used to be a separate `approval.rs` with its own
//! `McpApprovalPolicy`/`ApprovalStatus`; it was a dead duplicate (zero
//! external references, and its one config flag was written but never
//! read) of the approval gate that `server_gate.rs`'s `McpPolicyContext::
//! decide` actually implements, so it was deleted (§25b). If it is ever
//! revived: its `ConfigScope::Dynamic -> PendingApproval` mapping would
//! silently regress §27a, which made `--mcp-config` entries `Dynamic`
//! specifically so they are NOT approval-gated — see
//! `server_gate.rs`'s `decide` and its `project_approval_is_scope_aware`
//! test's `ConfigScope::Dynamic` case.

#![forbid(unsafe_code)]

pub mod capabilities;
pub mod client;
pub mod config_diagnostics;
pub mod connection;
pub mod discovery_cache;
pub mod enterprise_policy;
pub mod env_expansion;
pub mod headers_helper;
pub mod hook_dispatch;
pub mod identity;
pub mod inbound;
pub mod initialize_params;
pub mod json_config;
pub mod mcp_output_storage;
pub mod negotiation;
pub mod normalization;
pub mod oauth;
pub mod protocol_negotiation;
pub mod raw_conn;
pub mod registry;
pub mod server_gate;
pub mod tool_schema;
#[cfg(test)]
mod tracing_capture;
pub mod xaa;
pub mod xaa_idp;

pub use client::{
    truncate_description, McpClient, McpClientError, McpDirectoryEntry, MAX_MCP_DESCRIPTION_LENGTH,
    MAX_MCP_DIRECTORY_PAGES,
};
pub use connection::{ConfigScope, McpConnectionState, McpServerConfig};
pub use discovery_cache::{
    cache_gate, decide, feature_enabled as discovery_cache_feature_enabled, max_stale_ms,
    miss_telemetry_value, strike_threshold, ttl_ms, CacheGateReason, Decision as DiscoveryCacheDecision,
    DiscoveryCacheEntry, DiscoveryCacheStore, EntryLookup as DiscoveryCacheEntryLookup, MissReason,
};
pub use env_expansion::{
    expand_env_vars_in_string, expand_with_env, startup_env_snapshot, EnvExpansion,
};
pub use hook_dispatch::{ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher};
pub use identity::{
    ClientInfo, CLIENT_DESCRIPTION, CLIENT_INFO, CLIENT_NAME, CLIENT_TITLE, CLIENT_VERSION,
    MCP_WEBSITE_URL,
};
pub use inbound::{new_shared_roots, ElicitationCreateHandler, RootsListHandler, SharedRoots};
pub use initialize_params::{ClientCapabilities, InitializeParams};
pub use json_config::{
    build_server_from_json_entry, discovery_cache_flag, discovery_cache_is_schema_key_for,
    load_mcp_json_with_precedence, load_mcp_servers, parse_global_config_mcp_servers,
    parse_local_config_mcp_servers, parse_mcp_json_string, parse_plugin_mcp_json_string,
    role_flag, role_is_schema_key_for, server_entry_shape_is_valid, McpJsonError,
};
pub use mcp_output_storage::{
    binary_blob_saved_message, decode_base64, extension_for_mime_type, format_file_size,
    map_resource_contents, persist_binary_content, Base64Error, PersistBinaryResult,
    RawResourceContent as RawResourceContentRich,
};
pub use raw_conn::RawConnectionProvider;
pub use registry::{McpCatalogChanged, McpCatalogKind, McpRegistry};
pub use server_gate::{
    apply_project_server_gate, is_builtin_computer_use, mcp_server_is_disabled, McpPolicyContext,
    McpServerBlockReason, McpServerDecision, BUILTIN_COMPUTER_USE_SERVER,
};
pub use xaa_idp::{MapServerOAuthLookup, ServerOAuthLookup, XaaIdpConfigProvider, XaaIdpSettings};
