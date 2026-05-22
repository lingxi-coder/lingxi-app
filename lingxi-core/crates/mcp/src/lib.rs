//! MCP (Model Context Protocol) connection management for `LingXi` Core.
//!
//! Owns the per-connection state machine (`connection.rs`), the
//! [`registry::McpRegistry`] that drives transitions through a
//! platform-supplied [`lingxi_traits::McpTransport`], the OAuth 2.1
//! handshake skeleton (`oauth.rs`), the approval policy (`approval.rs`),
//! and the per-agent connection bookkeeping (`agent_scope.rs`).
//!
//! See spec §7 (MCP). The reconnect loop, health checker, and full
//! OAuth implementation land in Plan 13.

#![forbid(unsafe_code)]

pub mod agent_scope;
pub mod approval;
pub mod capabilities;
pub mod connection;
pub mod oauth;
pub mod registry;

pub use connection::{ConfigScope, McpConnectionState, McpServerConfig};
pub use registry::McpRegistry;
