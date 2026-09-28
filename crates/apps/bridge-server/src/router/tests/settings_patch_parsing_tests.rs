use super::parse_settings_patch;
use serde_json::json;

/// A `null` value in the wire patch means "delete this key" — it must be
/// decoded to `None`, never to a stored `Some(Value::Null)`. A regression
/// that kept the literal `Value::Null` would still satisfy "the key has
/// an entry" but would be silently wrong once applied (it would WRITE a
/// JSON `null`, not delete the key), so this asserts the exact `None`
/// shape, not just success.
#[test]
fn null_value_decodes_to_a_delete_not_a_stored_null() {
    let patch = parse_settings_patch(r#"{"outputStyle": null}"#).unwrap();
    assert_eq!(
        patch,
        vec![("outputStyle".to_string(), None)],
        "a JSON null must decode to None (delete), not Some(Value::Null)"
    );
}

/// A non-null value decodes to `Some(value)` (a set, not a delete) —
/// the companion case to the null test above, so the `is_null` branch is
/// exercised on both sides.
#[test]
fn non_null_value_decodes_to_a_set() {
    let patch = parse_settings_patch(r#"{"outputStyle": "terse"}"#).unwrap();
    assert_eq!(
        patch,
        vec![("outputStyle".to_string(), Some(json!("terse")))]
    );
}

/// A JSON array is syntactically valid JSON but not an acceptable patch
/// shape (there are no keys to patch). It must be rejected, not coerced
/// or silently accepted as an empty/no-op patch.
#[test]
fn a_json_array_patch_is_rejected() {
    let err = parse_settings_patch(r#"["outputStyle"]"#).unwrap_err();
    assert!(
        err.contains("object"),
        "error must say the patch needs to be an object, got: {err}"
    );
}

/// Syntactically broken JSON must be rejected with a message a client
/// can act on, not panic or silently produce an empty patch.
#[test]
fn invalid_json_is_rejected() {
    let err = parse_settings_patch("{ not json").unwrap_err();
    assert!(
        err.contains("JSON"),
        "error must say the patch is not valid JSON, got: {err}"
    );
}
