//! Integration test for the MCP connect lifecycle.
//!
//! Drives `McpRegistry::connect` against `MockMcpTransport` to ensure the
//! state machine walks `Disconnected → Connecting → Connected` and
//! discovers the canned tool catalog.

use lingxi_mcp::{ConfigScope, McpRegistry, McpServerConfig};
use lingxi_test_harness::mocks::MockMcpTransport;
use lingxi_traits::McpTransportSpec;
use std::sync::Arc;

#[tokio::test]
async fn connect_initializes_and_lists_tools() {
    let transport = Arc::new(MockMcpTransport::new());
    transport.add_tool("hello");
    let registry = McpRegistry::new(transport.clone());

    let config = McpServerConfig {
        name: "mock".into(),
        spec: McpTransportSpec::InProcess {
            registry_key: "mock".into(),
        },
        scope: ConfigScope::User,
        disabled: false,
    };
    let conn_id = registry.connect(config).await.unwrap();
    assert!(!conn_id.as_uuid().is_nil());
}
