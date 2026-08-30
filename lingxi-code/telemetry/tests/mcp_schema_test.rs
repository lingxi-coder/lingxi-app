use telemetry::pii::Verified;
use telemetry::tengu::mcp::{
    self, ConfigInvalidSource, DegradedPayload, DegradedReason, ServerConfigInvalidPayload,
    ToolsListedPayload,
};

#[test]
fn all_three_mcp_event_names_are_locked() {
    let names: &[&str] = &[mcp::SERVER_CONFIG_INVALID, mcp::TOOLS_LISTED, mcp::DEGRADED];
    assert_eq!(names.len(), 3);
    for n in names {
        assert!(n.starts_with("tengu_mcp_"));
    }
    assert_eq!(
        mcp::SERVER_CONFIG_INVALID,
        "tengu_mcp_server_config_invalid"
    );
    assert_eq!(mcp::TOOLS_LISTED, "tengu_mcp_tools_listed");
    assert_eq!(mcp::DEGRADED, "tengu_mcp_degraded");
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
        mcp_server_name: Some(Verified::assert_safe("my-server".to_string())),
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

#[test]
fn degraded_normalized_payload_carries_only_normalized_count() {
    let payload = DegradedPayload {
        reason: DegradedReason::ToolSchemaNormalized,
        transport_type: Some(Verified::assert_safe("stdio".to_string())),
        normalized_count: Some(3),
        skipped_count: None,
        kept_count: None,
        mcp_server_name: Some(Verified::assert_safe("my-server".to_string())),
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "reason": "tool_schema_normalized",
            "transport_type": "stdio",
            "normalized_count": 3,
            "mcp_server_name": "my-server",
        })
    );
}

#[test]
fn degraded_schema_validator_unavailable_carries_no_per_server_fields() {
    let payload = DegradedPayload {
        reason: DegradedReason::SchemaValidatorUnavailable,
        transport_type: None,
        normalized_count: None,
        skipped_count: None,
        kept_count: None,
        mcp_server_name: None,
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({"reason": "schema_validator_unavailable"})
    );
}

#[test]
fn degraded_reason_values_are_snake_case() {
    for (variant, wire) in [
        (
            DegradedReason::ToolSchemaNormalized,
            "tool_schema_normalized",
        ),
        (
            DegradedReason::ToolSchemaNormalizeGated,
            "tool_schema_normalize_gated",
        ),
        (
            DegradedReason::ToolSchemaUnsupported,
            "tool_schema_unsupported",
        ),
        (DegradedReason::ToolSchemaInvalid, "tool_schema_invalid"),
        (
            DegradedReason::ToolPropertyKeyInvalid,
            "tool_property_key_invalid",
        ),
        (
            DegradedReason::ToolSchemaInvalidGated,
            "tool_schema_invalid_gated",
        ),
        (
            DegradedReason::ToolPropertyKeyInvalidGated,
            "tool_property_key_invalid_gated",
        ),
        (
            DegradedReason::SchemaValidatorUnavailable,
            "schema_validator_unavailable",
        ),
    ] {
        assert_eq!(
            serde_json::to_value(variant).unwrap(),
            serde_json::json!(wire)
        );
        assert_eq!(
            variant.wire_str(),
            wire,
            "wire_str must match the serde rendering"
        );
    }
}

#[test]
fn degraded_payload_rejects_unknown_fields() {
    let json = serde_json::json!({
        "reason": "tool_schema_normalized",
        "transport_type": "stdio",
        "normalized_count": 1,
        "mcp_server_name": "s",
        "extra": true,
    });
    let result: Result<DegradedPayload, _> = serde_json::from_value(json);
    assert!(
        result.is_err(),
        "deny_unknown_fields must reject an unrecognized key"
    );
}

// ── Round-1 review regressions ──────────────────────────────────────────────

/// `mcpServerName` is `EA(ln(e.name),HT(e.name,e.config))` at the oracle:
/// `EA(n,e){return e?Vo(n):void 0}`, and `undefined` is DROPPED by the
/// object spread. For an ordinary user-configured server `HT` is false, so
/// the key must not be in the payload at all. Modelling it as required and
/// always-populated (the shipped shape before this fix) exfiltrated the
/// user's private server name as an analytics dimension on every connect.
#[test]
fn tools_listed_omits_the_server_name_when_the_first_party_gate_is_off() {
    let payload = ToolsListedPayload {
        transport_type: Verified::assert_safe("stdio".to_string()),
        list_duration_ms: 5,
        tool_count: 1,
        always_load_count: 0,
        discovery_source: Verified::assert_safe("live".to_string()),
        mcp_server_name: None,
    };
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(
        json,
        serde_json::json!({
            "transport_type": "stdio",
            "list_duration_ms": 5,
            "tool_count": 1,
            "always_load_count": 0,
            "discovery_source": "live",
        }),
        "the mcp_server_name key must be ABSENT, not null/empty, when the gate is off"
    );
    // ...and the absence must be representable on the way back in, too:
    // `deny_unknown_fields` plus a required field would make the oracle's own
    // payload shape un-deserializable.
    let back: ToolsListedPayload = serde_json::from_value(json).expect("deserialize");
    assert!(back.mcp_server_name.is_none());
}

/// Oracle `HT` gates on FIRST-PARTY-ness only — every arm tests a built-in
/// Anthropic server, a `claudeai-proxy` transport, or an Anthropic URL. No
/// ordinary transport kind may open the gate.
#[test]
fn server_name_gate_is_closed_for_every_user_configurable_transport() {
    for kind in [
        "stdio",
        "sse",
        "http",
        "websocket",
        "inprocess",
        "sse-ide",
        "sdk-control",
    ] {
        assert!(
            !mcp::server_name_gate(kind),
            "{kind} is a user-configurable transport; the oracle's HT gate is false for it, \
             so the raw server name must never be attached"
        );
    }
    // The one `$M` arm this port can spell (`e === "claudeai-proxy"`).
    assert!(mcp::server_name_gate("claudeai-proxy"));
}

/// `connected_zero_tools` is the FIRST statement of the oracle's `yn`
/// (@182316780) — 20 lines above the seven tool-schema counters an earlier
/// revision transcribed verbatim while calling that set complete. It is the
/// single most common silent-MCP-failure signal.
#[test]
fn connected_zero_tools_is_a_modelled_degraded_reason() {
    let parsed: DegradedReason = serde_json::from_value(serde_json::json!("connected_zero_tools"))
        .expect("`connected_zero_tools` must be a modelled DegradedReason");
    assert_eq!(parsed, DegradedReason::ConnectedZeroTools);
    assert_eq!(parsed.wire_str(), "connected_zero_tools");
    assert_eq!(
        serde_json::to_value(parsed).unwrap(),
        serde_json::json!("connected_zero_tools")
    );
}

/// Oracle payload for that reason is `{reason,transportType,mcpServerName,..._}`
/// — no count field of any kind.
#[test]
fn connected_zero_tools_payload_carries_no_count_field() {
    let payload = DegradedPayload {
        reason: DegradedReason::ConnectedZeroTools,
        transport_type: Some(Verified::assert_safe("stdio".to_string())),
        normalized_count: None,
        skipped_count: None,
        kept_count: None,
        mcp_server_name: None,
    };
    assert_eq!(
        serde_json::to_value(&payload).expect("serialize"),
        serde_json::json!({ "reason": "connected_zero_tools", "transport_type": "stdio" })
    );
}
