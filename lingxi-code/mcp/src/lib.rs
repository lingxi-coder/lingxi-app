//! MCP (Model Context Protocol) connection management for `LingXi` Core.
//!
//! Owns the per-connection state machine (`connection.rs`), the
//! [`registry::McpRegistry`] that drives transitions through a
//! platform-supplied [`traits::McpTransport`], the OAuth 2.1
//! handshake skeleton (`oauth.rs`), the approval policy (`approval.rs`),
//! the per-agent connection bookkeeping (`agent_scope.rs`), and the
//! `client::McpClient` JSON-RPC client built on top of `lingxi-jsonrpc`
//! (M2-02b — see `docs/superpowers/plans/2026-05-23-m2-02b-mcp-client.md`).

#![forbid(unsafe_code)]

pub mod agent_scope;
pub mod approval;
pub mod capabilities;
pub mod client;
pub mod connection;
pub mod env_expansion;
pub mod hook_dispatch;
pub mod identity;
pub mod inbound;
pub mod initialize_params;
pub mod json_config;
pub mod mcp_output_storage;
pub mod normalization;
pub mod oauth;
pub mod raw_conn;
pub mod registry;
pub mod server_gate;
pub mod xaa;
pub mod xaa_idp;

pub use client::{truncate_description, McpClient, McpClientError, MAX_MCP_DESCRIPTION_LENGTH};
pub use connection::{ConfigScope, McpConnectionState, McpServerConfig};
pub use env_expansion::{expand_env_vars_in_string, EnvExpansion};
pub use hook_dispatch::{ElicitationHookOutcome, ElicitationHookRequest, HookDispatcher};
pub use identity::{
    ClientInfo, CLIENT_DESCRIPTION, CLIENT_INFO, CLIENT_NAME, CLIENT_TITLE, CLIENT_VERSION,
    MCP_WEBSITE_URL,
};
pub use inbound::{ElicitationCreateHandler, RootsListHandler};
pub use initialize_params::{ClientCapabilities, InitializeParams};
pub use json_config::{
    load_mcp_json_with_precedence, load_mcp_servers, parse_global_config_mcp_servers,
    parse_local_config_mcp_servers, parse_mcp_json_string, McpJsonError,
};
pub use mcp_output_storage::{
    binary_blob_saved_message, decode_base64, extension_for_mime_type, format_file_size,
    map_resource_contents, persist_binary_content, Base64Error, PersistBinaryResult,
    RawResourceContent as RawResourceContentRich,
};
pub use raw_conn::RawConnectionProvider;
pub use registry::McpRegistry;
pub use server_gate::{
    apply_project_server_gate, is_builtin_computer_use, mcp_server_is_disabled,
    BUILTIN_COMPUTER_USE_SERVER,
};
pub use xaa_idp::{MapServerOAuthLookup, ServerOAuthLookup, XaaIdpConfigProvider, XaaIdpSettings};
