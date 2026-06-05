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
pub mod identity;
pub mod inbound;
pub mod initialize_params;
pub mod json_config;
pub mod normalization;
pub mod oauth;
pub mod raw_conn;
pub mod registry;

pub use client::{truncate_description, McpClient, McpClientError, MAX_MCP_DESCRIPTION_LENGTH};
pub use connection::{ConfigScope, McpConnectionState, McpServerConfig};
pub use env_expansion::{expand_env_vars_in_string, EnvExpansion};
pub use identity::{
    ClientInfo, CLIENT_INFO, CLIENT_NAME, CLIENT_TITLE, CLIENT_VERSION, MCP_WEBSITE_URL,
};
pub use inbound::{ElicitationCreateHandler, RootsListHandler};
pub use initialize_params::{ClientCapabilities, InitializeParams};
pub use json_config::{load_mcp_json_with_precedence, parse_mcp_json_string, McpJsonError};
pub use raw_conn::RawConnectionProvider;
pub use registry::McpRegistry;
