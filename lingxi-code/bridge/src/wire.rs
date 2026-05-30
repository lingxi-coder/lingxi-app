//! Remote-drive **wire protocol** types (M8-P13).
//!
//! These are the handshake + framing DTOs for M9's remote-driving work (a
//! desktop engine driven by a mobile/remote client over a bridge transport).
//! M8 ships only the type definitions + serde derives — no transport, no auth
//! flow, no server loop (those land in M9, see `// M9:` markers).
//!
//! The module is named `wire` (not `protocol`) deliberately: `bridge` depends
//! on the `protocol` crate, so a local `mod protocol` would shadow it. The
//! public types are re-exported at the crate root, so consumers write
//! `bridge::ServerHello` regardless.

use serde::{Deserialize, Serialize};

/// Bridge wire-protocol version this build speaks.
pub const BRIDGE_PROTOCOL_VERSION: &str = "0.1.0";

/// Capability flags advertised in the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Endpoint can stream incremental turn events.
    pub supports_streaming: bool,
    /// Endpoint exposes the tool surface.
    pub supports_tools: bool,
    /// Endpoint exposes the skill surface.
    pub supports_skills: bool,
    /// Endpoint exposes the slash-command surface.
    pub supports_commands: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            supports_streaming: true,
            supports_tools: true,
            supports_skills: true,
            supports_commands: true,
        }
    }
}

/// Client → server opening handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    /// Wire-protocol version the client speaks.
    pub protocol_version: String,
    /// Human-readable client identifier (e.g. `"lingxi-ios/0.9.0"`).
    pub client_name: String,
    /// What the client can handle.
    pub capabilities: Capabilities,
}

/// Server → client handshake reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    /// Wire-protocol version the server speaks.
    pub protocol_version: String,
    /// Human-readable server identifier (e.g. `"lingxi-engine-desktop/0.9.0"`).
    pub server_name: String,
    /// What the server can provide.
    pub capabilities: Capabilities,
}

/// A framed JSON-RPC-style request from client to server.
///
/// M9 maps `method`/`params` onto `protocol::Effect` execution; M8 keeps the
/// envelope generic (a `serde_json::Value` payload) so the wire shape is locked
/// without coupling to the concrete effect enum yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeRequest {
    /// Correlation id (echoed in the matching [`BridgeResponse`]).
    pub id: u64,
    /// Method name (e.g. `"run_turn"`, `"dispatch_tool"`).
    pub method: String,
    /// Method parameters. // M9: typed against protocol::Effect.
    pub params: serde_json::Value,
}

/// A framed reply to a [`BridgeRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeResponse {
    /// Correlation id of the originating request.
    pub id: u64,
    /// Success payload. // M9: typed against protocol::EffectResult.
    pub result: Option<serde_json::Value>,
    /// Error payload (mutually exclusive with `result`).
    pub error: Option<BridgeWireError>,
}

/// Wire-level error inside a [`BridgeResponse`] (distinct from the engine-side
/// runtime `transport::BridgeError`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeWireError {
    /// Numeric error code (JSON-RPC convention).
    pub code: i32,
    /// Log-safe error message.
    pub message: String,
}

/// Server → client auth challenge. // M9: JWT/nonce flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// Opaque nonce the client signs.
    pub nonce: String,
}

/// Client → server auth reply. // M9: signed token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    /// Bearer token / signed nonce.
    pub token: String,
}
