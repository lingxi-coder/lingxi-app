use super::parse_mcp_config_json;
use serde_json::json;

#[test]
fn a_json_object_decodes_to_itself() {
    let value = parse_mcp_config_json(r#"{"command": "npx"}"#).unwrap();
    assert_eq!(value, json!({ "command": "npx" }));
}

#[test]
fn a_non_object_is_rejected() {
    let err = parse_mcp_config_json(r#"["npx"]"#).unwrap_err();
    assert!(
        err.contains("object"),
        "error must say the config needs to be an object, got: {err}"
    );
}

#[test]
fn invalid_json_is_rejected() {
    let err = parse_mcp_config_json("{ not json").unwrap_err();
    assert!(
        err.contains("JSON"),
        "error must say the config is not valid JSON, got: {err}"
    );
}
