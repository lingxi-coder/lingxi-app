use telemetry::pii::{PiiTagged, Verified};
use telemetry::tengu::plugin::{
    self, EnabledVia, LoadFailedPayload, NameCollisionPayload, PluginEnabledForSessionPayload,
};

#[test]
fn all_14_plugin_event_names_are_locked() {
    assert_eq!(plugin::NAMES.len(), 14);
    for n in plugin::NAMES {
        assert!(n.starts_with("tengu_plugin_"), "{n} must be tengu_plugin_*");
    }
    assert_eq!(
        plugin::ENABLED_FOR_SESSION,
        "tengu_plugin_enabled_for_session"
    );
    assert_eq!(plugin::NAME_COLLISION, "tengu_plugin_name_collision");
    assert_eq!(plugin::FOLDER_SHADOWED, "tengu_plugin_folder_shadowed");
    assert_eq!(plugin::RENAMED, "tengu_plugin_renamed");
    assert_eq!(plugin::LOAD_FAILED, "tengu_plugin_load_failed");
    assert_eq!(plugin::INSTALLED, "tengu_plugin_installed");
    assert_eq!(plugin::INSTALLED_CLI, "tengu_plugin_installed_cli");
    assert_eq!(plugin::UNINSTALLED_CLI, "tengu_plugin_uninstalled_cli");
    assert_eq!(plugin::ENABLED_CLI, "tengu_plugin_enabled_cli");
    assert_eq!(plugin::DISABLED_CLI, "tengu_plugin_disabled_cli");
    assert_eq!(plugin::DISABLED_ALL_CLI, "tengu_plugin_disabled_all_cli");
    assert_eq!(plugin::UPDATED_CLI, "tengu_plugin_updated_cli");
    assert_eq!(plugin::COMMAND_FAILED, "tengu_plugin_command_failed");
    assert_eq!(plugin::REMOTE_FETCH, "tengu_plugin_remote_fetch");
}

#[test]
fn enabled_via_wire_values_are_kebab_case() {
    // Traced against the oracle's `zin` function — NOT the byte-alignment
    // doc's guessed value set (see plugin.rs's module doc: `auto_install` is
    // an input the function tests, never an `enabled_via` output).
    assert_eq!(
        serde_json::to_value(EnabledVia::DefaultEnable).unwrap(),
        serde_json::json!("default-enable")
    );
    assert_eq!(
        serde_json::to_value(EnabledVia::OrgPolicy).unwrap(),
        serde_json::json!("org-policy")
    );
    assert_eq!(
        serde_json::to_value(EnabledVia::AdminInstall).unwrap(),
        serde_json::json!("admin-install")
    );
    assert_eq!(
        serde_json::to_value(EnabledVia::SeedMount).unwrap(),
        serde_json::json!("seed-mount")
    );
    assert_eq!(
        serde_json::to_value(EnabledVia::UserInstall).unwrap(),
        serde_json::json!("user-install")
    );
}

fn sample_enabled_for_session() -> PluginEnabledForSessionPayload {
    PluginEnabledForSessionPayload {
        proto_plugin_name: PiiTagged::assert_pii_tagged_column("my-plugin".to_string()),
        proto_marketplace_name: Some(PiiTagged::assert_pii_tagged_column(
            "my-marketplace".to_string(),
        )),
        plugin_id_hash: Verified::assert_safe("abc123".to_string()),
        plugin_scope: Verified::assert_safe("user".to_string()),
        plugin_name_redacted: Verified::assert_safe("my-plugin".to_string()),
        marketplace_name_redacted: Verified::assert_safe("my-marketplace".to_string()),
        is_official_plugin: false,
        server_plugin_id: None,
        enabled_via: EnabledVia::UserInstall,
        installation_preference: None,
        skill_path_count: 1,
        command_path_count: 2,
        agent_path_count: 0,
        has_mcp: true,
        host_owned_mcp: false,
        has_lsp: false,
        has_hooks: true,
        has_settings: false,
        sessions_since_last_use: Some(3),
        days_since_last_use: Some(1),
        safe_mode: false,
        settings_keys: None,
        version: Some(Verified::assert_safe("1.2.3".to_string())),
    }
}

#[test]
fn enabled_for_session_payload_round_trips_with_proto_fields() {
    let payload = sample_enabled_for_session();
    let json = serde_json::to_value(&payload).expect("serialize");
    assert_eq!(json["_PROTO_plugin_name"], serde_json::json!("my-plugin"));
    assert_eq!(
        json["_PROTO_marketplace_name"],
        serde_json::json!("my-marketplace")
    );
    assert_eq!(json["plugin_id_hash"], serde_json::json!("abc123"));
    assert_eq!(json["enabled_via"], serde_json::json!("user-install"));
    // Absent-optional fields must not appear on the wire at all.
    assert!(json.get("server_plugin_id").is_none());
    assert!(json.get("installation_preference").is_none());
    assert!(json.get("settings_keys").is_none());

    let round_tripped: PluginEnabledForSessionPayload =
        serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_tripped.plugin_id_hash.as_str(), "abc123");
}

#[test]
fn enabled_for_session_payload_rejects_unknown_fields() {
    let mut json = serde_json::to_value(sample_enabled_for_session()).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::json!(1));
    let result: Result<PluginEnabledForSessionPayload, _> = serde_json::from_value(json);
    assert!(result.is_err(), "deny_unknown_fields must reject `unexpected`");
}

#[test]
fn strip_proto_fields_removes_the_pii_tagged_columns() {
    use std::collections::HashMap;
    use telemetry::{pii::strip_proto_fields, AnalyticsValue};

    let payload = sample_enabled_for_session();
    let json = serde_json::to_value(&payload).unwrap();
    let mut metadata: telemetry::LogEventMetadata = HashMap::new();
    for (k, v) in json.as_object().unwrap() {
        let av = match v {
            serde_json::Value::String(s) => AnalyticsValue::String(s.clone()),
            serde_json::Value::Bool(b) => AnalyticsValue::Bool(*b),
            serde_json::Value::Number(n) => AnalyticsValue::Int(n.as_i64().unwrap_or_default()),
            _ => AnalyticsValue::None,
        };
        metadata.insert(k.clone(), av);
    }
    assert!(metadata.contains_key("_PROTO_plugin_name"));
    strip_proto_fields(&mut metadata);
    assert!(!metadata.keys().any(|k| k.starts_with("_PROTO_")));
    // Non-proto fields survive.
    assert!(metadata.contains_key("plugin_id_hash"));
}

#[test]
fn name_collision_payload_does_not_spread_plugin_identity() {
    let payload = NameCollisionPayload {
        item_type: Verified::assert_safe("skill".to_string()),
        proto_skill_name: PiiTagged::assert_pii_tagged_column("my-skill".to_string()),
        item_name_hash: Verified::assert_safe("hash".to_string()),
        source_count: 2,
        sources: Verified::assert_safe("a,b".to_string()),
        winner_source: Some(Verified::assert_safe("b".to_string())),
    };
    let json = serde_json::to_value(&payload).unwrap();
    // Confirms this event's shape is NOT the plugin-identity block (no
    // plugin_id_hash / _PROTO_plugin_name here — only _PROTO_skill_name).
    assert!(json.get("plugin_id_hash").is_none());
    assert_eq!(json["_PROTO_skill_name"], serde_json::json!("my-skill"));
}

#[test]
fn load_failed_payload_rejects_unknown_fields() {
    let payload = LoadFailedPayload {
        error_category: Verified::assert_safe("network".to_string()),
        cache_only: false,
        component: None,
        errno: None,
        proto_plugin_name: PiiTagged::assert_pii_tagged_column("p".to_string()),
        proto_marketplace_name: None,
        plugin_id_hash: Verified::assert_safe("h".to_string()),
        plugin_scope: Verified::assert_safe("user".to_string()),
        plugin_name_redacted: Verified::assert_safe("p".to_string()),
        marketplace_name_redacted: Verified::assert_safe("m".to_string()),
        is_official_plugin: false,
    };
    let mut json = serde_json::to_value(&payload).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::json!(1));
    let result: Result<LoadFailedPayload, _> = serde_json::from_value(json);
    assert!(result.is_err());
}
