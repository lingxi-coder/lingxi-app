//! Inbound JSON-RPC request handlers required by the
//! `{roots:{}, elicitation:{}}` capability declaration.
//!
//! Real handler bodies (with `lingxi_jsonrpc::InboundHandler` impls) land
//! in M2-02b Tasks 4-5 — this module currently exposes only the public
//! struct surface so the client module compiles.

use std::path::PathBuf;

/// Handler for inbound `roots/list` requests from the MCP server.
///
/// Returns `{"roots": [{"uri": "file://<cwd>"}]}` where `<cwd>` is the
/// absolute path supplied at [`crate::McpClient`] construction time.
pub struct RootsListHandler {
    /// Absolute current working directory advertised as the single root.
    pub cwd: PathBuf,
}

/// Handler for inbound `elicitation/create` requests from the MCP server.
///
/// Default response is `{"action": "cancel"}` (matches claude-code
/// `services/mcp/client.ts:1196`).
pub struct ElicitationCreateHandler;
