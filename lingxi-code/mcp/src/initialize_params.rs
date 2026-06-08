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
    /// MCP protocol version date — the value claude-code sends on initialize.
    /// claude-code creates its MCP `Client` with no `protocolVersion` override
    /// (`services/mcp/client.ts:985-1002`), so the SDK sends its
    /// `LATEST_PROTOCOL_VERSION`; at the pinned SDK (`@modelcontextprotocol/sdk`
    /// `^1.12.1` → 1.29.0, `types.js:2`) that is `2025-11-25`.
    pub protocol_version: &'static str,
    /// Capability advertisement; see [`ClientCapabilities`].
    pub capabilities: ClientCapabilities,
    /// Identity of the calling client; see [`ClientInfo`].
    pub client_info: ClientInfo,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: "2025-11-25",
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_params_wire_shape_matches_claude_code() {
        let params = InitializeParams::default();
        let json = serde_json::to_value(&params).expect("serialize");

        // protocolVersion is the literal MCP date the SDK's
        // LATEST_PROTOCOL_VERSION resolves to (SDK 1.29.0 → 2025-11-25).
        assert_eq!(json["protocolVersion"], "2025-11-25");

        // capabilities is EXACTLY {"roots": {}, "elicitation": {}}.
        let caps = &json["capabilities"];
        assert!(caps.is_object(), "capabilities must be a JSON object");
        let caps_obj = caps.as_object().unwrap();
        assert_eq!(caps_obj.len(), 2, "capabilities must have exactly 2 keys");
        assert!(caps_obj.contains_key("roots"), "roots key required");
        assert!(
            caps_obj.contains_key("elicitation"),
            "elicitation key required"
        );
        assert!(caps["roots"].is_object(), "roots must be an object");
        assert_eq!(
            caps["roots"].as_object().unwrap().len(),
            0,
            "roots must be EMPTY"
        );
        assert!(
            caps["elicitation"].is_object(),
            "elicitation must be an object"
        );
        assert_eq!(
            caps["elicitation"].as_object().unwrap().len(),
            0,
            "elicitation must be EMPTY — Java MCP SDK rejects {{form:{{}},url:{{}}}}",
        );

        // clientInfo is camelCase (NOT client_info).
        assert!(
            json.get("clientInfo").is_some(),
            "must be camelCase clientInfo"
        );
        assert!(json.get("client_info").is_none(), "no snake_case leak");
        assert_eq!(json["clientInfo"]["name"], "claude-code");
    }

    #[test]
    fn raw_wire_bytes_contain_literal_claude_code_marker() {
        // Lock the BYTES of the outgoing JSON-RPC payload.
        let params = InitializeParams::default();
        let bytes = serde_json::to_vec(&params).expect("serialize");
        let s = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            s.contains(r#""name":"claude-code""#),
            "wire bytes must contain literal \"name\":\"claude-code\", got: {s}",
        );
    }
}
