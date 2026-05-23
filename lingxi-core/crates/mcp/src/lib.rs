//! MCP (Model Context Protocol) connection management for `LingXi` Core.
//!
//! Owns the per-connection state machine (`connection.rs`), the
//! [`registry::McpRegistry`] that drives transitions through a
//! platform-supplied [`lingxi_traits::McpTransport`], the OAuth 2.1
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
pub mod identity;
pub mod inbound;
pub mod initialize_params;
pub mod oauth;
pub mod registry;

pub use client::{McpClient, McpClientError};
pub use connection::{ConfigScope, McpConnectionState, McpServerConfig};
pub use identity::{
    ClientInfo, CLIENT_INFO, CLIENT_NAME, CLIENT_TITLE, CLIENT_VERSION, MCP_WEBSITE_URL,
};
pub use inbound::{ElicitationCreateHandler, RootsListHandler};
pub use initialize_params::{ClientCapabilities, InitializeParams};
pub use registry::McpRegistry;
