//! MCP `initialize` request parameters per spec §6.2 + claude-code TS
//! reference `services/mcp/client.ts` lines 985-1002.

use crate::identity::ClientInfo;
use serde::Serialize;
use serde_json::{Map, Value};

/// Wire-shape `{"roots": {}, "elicitation": {}}` — both fields required,
/// both empty objects (Java MCP SDK rejects unknown elicitation props).
#[derive(Debug, Clone, Serialize)]
pub struct ClientCapabilities {
    /// `roots` capability marker — serialized as an empty JSON object.
    pub roots: Map<String, Value>,
    /// `elicitation` capability marker — serialized as an empty JSON object.
    pub elicitation: Map<String, Value>,
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            roots: Map::new(),
            elicitation: Map::new(),
        }
    }
}

/// Body of the MCP `initialize` request.
///
/// Field names are serialized as camelCase (`protocolVersion`,
/// `clientInfo`) to match the claude-code reference wire format.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// MCP protocol version date — locked to the value claude-code uses.
    pub protocol_version: &'static str,
    /// Capability advertisement; see [`ClientCapabilities`].
    pub capabilities: ClientCapabilities,
    /// Identity of the calling client; see [`ClientInfo`].
    pub client_info: ClientInfo,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: "2024-11-05",
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo::default(),
        }
    }
}
