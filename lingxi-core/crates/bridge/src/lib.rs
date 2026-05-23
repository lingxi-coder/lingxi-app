//! `lingxi-bridge` — local IDE bridge (placeholder until M2-02 wires the real
//! MCP-over-WebSocket transport).
//!
//! claude-code's local IDE bridge connects to a VS Code / `JetBrains` plugin
//! announced by `~/.claude/ide/<port>.lock`. Transport is plain WebSocket
//! carrying MCP JSON-RPC, auth'd by `X-Claude-Code-Ide-Authorization` from
//! the lockfile.
//!
//! M2-01 strips out the M1-invented pairing/JWT stack. M2-02 §6.2 adds
//! `lockfile.rs` and rewrites `transport.rs` to build an
//! `McpTransportSpec::WebSocket { url, headers }` and hand off to
//! `lingxi_mcp::McpRegistry::connect_with_spec()`.
//!
//! Until then this crate exposes only stub types so downstream callers compile.

#![forbid(unsafe_code)]

pub mod message;
pub mod state;
pub mod transport;

pub use message::BridgeMessagePlaceholder;
pub use state::BridgeState;
pub use transport::IdeBridge;
