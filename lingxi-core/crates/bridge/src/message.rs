//! Placeholder for the local IDE bridge wire protocol.
//!
//! claude-code does not define a custom "bridge message" enum. The local IDE
//! bridge speaks MCP JSON-RPC over a WebSocket from `~/.claude/ide/<port>.lock`.
//! The 9-variant `BridgeMessage` enum that lived here in v0.2.0 was an M1
//! invention for a `claude.ai` remote-control flow now out of scope (spec §5).
//!
//! M2-02 §6.2 adds `crates/bridge/src/lockfile.rs` for discovery, and reuses
//! `lingxi-mcp` JSON-RPC types for the wire side. No new wire enum needed.

use serde::{Deserialize, Serialize};

/// Placeholder kept so downstream code can name a symbol without committing
/// to any wire shape. Will be removed in M2-02 once the bridge re-exports
/// `lingxi_mcp::McpNotificationDto` directly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeMessagePlaceholder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_roundtrip_is_null() {
        // A unit struct serializes to `null` — locks the observation.
        let s = serde_json::to_string(&BridgeMessagePlaceholder).unwrap();
        assert_eq!(s, "null");
        let _back: BridgeMessagePlaceholder = serde_json::from_str(&s).unwrap();
    }

    #[test]
    fn placeholder_default_equals_value() {
        // Verify `Default` is impl'd. For a unit struct this is trivially
        // true; the call site asserts the trait bound holds (the type would
        // fail to compile otherwise).
        #[allow(clippy::default_constructed_unit_structs)]
        let via_default = BridgeMessagePlaceholder::default();
        assert_eq!(BridgeMessagePlaceholder, via_default);
    }
}
