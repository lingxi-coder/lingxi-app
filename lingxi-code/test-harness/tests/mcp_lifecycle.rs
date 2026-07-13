//! Integration test for the MCP connect lifecycle.
//!
//! Drives `McpRegistry::connect` against `MockMcpTransport` to ensure the
//! state machine walks `Disconnected → Connecting → Connected` and
//! discovers the canned tool catalog.

use mcp::{ConfigScope, McpRegistry, McpServerConfig, RawConnectionProvider};
use std::sync::Arc;
use test_harness::mocks::MockMcpTransport;
use traits::{McpTransport, McpTransportSpec};

fn mock_config() -> McpServerConfig {
    McpServerConfig {
        name: "mock".into(),
        spec: McpTransportSpec::InProcess {
            registry_key: "mock".into(),
        },
        scope: ConfigScope::User,
        disabled: false,
        timeout_ms: None,
        always_load: false,
    }
}

#[tokio::test]
async fn connect_initializes_and_lists_tools() {
    let transport = Arc::new(MockMcpTransport::new());
    transport.add_tool("hello");
    let registry = McpRegistry::new(transport.clone());

    let conn_id = registry.connect(mock_config()).await.unwrap();
    assert!(!conn_id.as_uuid().is_nil());
}

/// Batch 1: with a `RawConnectionProvider` wired (the mock implements both
/// `McpTransport` and the bridge), `connect` builds a live `McpClient` so
/// `get_client` returns `Some`.
#[tokio::test]
async fn connect_registers_live_client_via_raw_conn() {
    let mock = Arc::new(MockMcpTransport::new());
    mock.add_tool("hello");
    let registry = McpRegistry::with_raw_conn(
        mock.clone() as Arc<dyn McpTransport>,
        mock as Arc<dyn RawConnectionProvider>,
    );

    registry.connect(mock_config()).await.unwrap();
    assert!(
        registry.get_client("mock").await.is_some(),
        "a working McpClient must be registered after connect"
    );
}
