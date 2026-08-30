use telemetry::pii::Verified;
use telemetry::tengu::mcp::{self, ConfigInvalidSource, ServerConfigInvalidPayload, ToolsListedPayload};

#[test]
fn both_mcp_event_names_are_locked() {
    let names: &[&str] = &[mcp::SERVER_CONFIG_INVALID, mcp::TOOLS_LISTED];
    assert_eq!(names.len(), 2);
    for n in names {
        assert!(n.starts_with("tengu_mcp_"));
    }
    assert_eq!(
        mcp::SERVER_CONFIG_INVALID,
        "tengu_mcp_server_config_invalid"
    );
    assert_eq!(mcp::TOOLS_LISTED, "tengu_mcp_tools_listed");
    assert_eq!(mcp::NAMES, names);
}

#[test]
fn server_config_invalid_payload_round_trips() {
    let payload = ServerConfigInvalidPayload {
        transport_type: Verified::assert_safe("stdio".to_string()),
        field: Verified::assert_safe("url".to_string()),
        source: ConfigInvalidSource::Connect,
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "transport_type": "stdio",
            "field": "url",
            "source": "connect",
        })
    );
    let round_tripped: ServerConfigInvalidPayload =
        serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_tripped.transport_type.as_str(), "stdio");
}

#[test]
fn server_config_invalid_payload_rejects_unknown_fields() {
    let json = serde_json::json!({
        "transport_type": "stdio",
        "field": "url",
        "source": "connect",
        "extra_unexpected_field": true,
    });
    let result: Result<ServerConfigInvalidPayload, _> = serde_json::from_value(json);
    assert!(
        result.is_err(),
        "deny_unknown_fields must reject an unrecognized key"
    );
}

#[test]
fn config_invalid_source_values_are_snake_case() {
    let loader = serde_json::to_value(ConfigInvalidSource::Loader).unwrap();
    let connect = serde_json::to_value(ConfigInvalidSource::Connect).unwrap();
    assert_eq!(loader, serde_json::json!("loader"));
    assert_eq!(connect, serde_json::json!("connect"));
}

#[test]
fn tools_listed_payload_round_trips() {
    let payload = ToolsListedPayload {
        transport_type: Verified::assert_safe("http".to_string()),
        list_duration_ms: 42,
        tool_count: 7,
        always_load_count: 2,
        discovery_source: Verified::assert_safe("live".to_string()),
        mcp_server_name: Verified::assert_safe("my-server".to_string()),
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "transport_type": "http",
            "list_duration_ms": 42,
            "tool_count": 7,
            "always_load_count": 2,
            "discovery_source": "live",
            "mcp_server_name": "my-server",
        })
    );
}

#[test]
fn tools_listed_payload_rejects_unknown_fields() {
    // Guards against silently reintroducing the byte-alignment doc's
    // unconfirmed `normalizedCount` / `keptCount` fields without first
    // tracing their real producer (see mcp.rs's module doc).
    let json = serde_json::json!({
        "transport_type": "stdio",
        "list_duration_ms": 1,
        "tool_count": 1,
        "always_load_count": 0,
        "discovery_source": "live",
        "mcp_server_name": "s",
        "normalizedCount": 3,
    });
    let result: Result<ToolsListedPayload, _> = serde_json::from_value(json);
    assert!(result.is_err(), "unconfirmed extra field must be rejected");
}
