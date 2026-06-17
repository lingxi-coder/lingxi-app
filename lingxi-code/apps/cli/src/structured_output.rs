//! `--json-schema` structured-output support.
//!
//! Faithful port of claude-code's structured-output path (`QueryEngine.ts`,
//! gated on `hasStructuredOutputTool`): in print mode the model is FORCED to
//! call a `StructuredOutput` tool whose `input_schema` is the user-supplied
//! schema; the returned arguments are validated against that schema client-side
//! and the turn is retried up to `MAX_STRUCTURED_OUTPUT_RETRIES` times on
//! failure. This module owns the two pure pieces — schema validation and the
//! retry budget — so they are unit-testable without the live turn loop. The
//! tool itself + the print-path retry loop wire these in.
//!
//! The validator covers the common JSON-Schema subset structured-output prompts
//! use: `type` (incl. type arrays + `integer`), `required`, `properties`,
//! `items`, `enum`, and `additionalProperties: false`. Unsupported keywords
//! (`$ref`, `allOf`/`anyOf`/`oneOf`, `pattern`, numeric/length bounds) are NOT
//! enforced — they PASS rather than falsely reject (documented; a full validator
//! is a follow-up). claude-code uses a complete validator; this is the faithful
//! common-case subset.

use serde_json::Value;

/// `MAX_STRUCTURED_OUTPUT_RETRIES` default (claude-code reads the env of the
/// same name, defaulting to `5`).
pub const DEFAULT_MAX_STRUCTURED_OUTPUT_RETRIES: u32 = 5;

/// Resolve the structured-output retry budget from `MAX_STRUCTURED_OUTPUT_RETRIES`
/// (1:1 with `QueryEngine.ts` `parseInt(process.env.MAX_STRUCTURED_OUTPUT_RETRIES || '5')`).
/// A missing / unparseable / non-positive value falls back to the default.
#[must_use]
pub fn resolve_max_retries(env_value: Option<&str>) -> u32 {
    match env_value.and_then(|v| v.trim().parse::<u32>().ok()) {
        Some(n) if n > 0 => n,
        _ => DEFAULT_MAX_STRUCTURED_OUTPUT_RETRIES,
    }
}

/// One iteration's outcome in the structured-output retry loop.
#[derive(Debug, Clone, PartialEq)]
pub enum StructuredDecision {
    /// The captured result conforms — emit this JSON and finish.
    Emit(Value),
    /// Not yet valid — re-run the turn with this corrective prompt.
    Retry(String),
}

/// Decide what to do after a structured-output turn from the model's captured
/// `StructuredOutput` arguments (`captured`) and the user `schema`. Pure — the
/// retry loop in the print path drives `run_turn` around this. `None` (the model
/// failed to call the tool) and a schema-invalid value both yield a corrective
/// [`StructuredDecision::Retry`] prompt; a conforming value yields `Emit`.
#[must_use]
pub fn structured_output_decision(captured: Option<Value>, schema: &Value) -> StructuredDecision {
    match captured {
        Some(value) => {
            let errors = validate(&value, schema);
            if errors.is_empty() {
                StructuredDecision::Emit(value)
            } else {
                StructuredDecision::Retry(format!(
                    "Your previous output did not conform to the JSON schema: {}. \
                     Call the StructuredOutput tool again with corrected output.",
                    errors.join("; ")
                ))
            }
        }
        None => StructuredDecision::Retry(
            "You must call the StructuredOutput tool with the final result.".to_string(),
        ),
    }
}

/// Validate `value` against the JSON-Schema `schema`. Returns the list of
/// human-readable error paths; an empty vec means the value conforms.
#[must_use]
pub fn validate(value: &Value, schema: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    validate_at(value, schema, "$", &mut errors);
    errors
}

fn validate_at(value: &Value, schema: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        return; // non-object schema (e.g. `true`) imposes no constraint
    };

    // `type` — wrong type short-circuits the other keyword checks at this node.
    if let Some(ty) = obj.get("type") {
        if !type_matches(value, ty) {
            errors.push(format!(
                "{path}: expected type {}, got {}",
                type_desc(ty),
                value_type_name(value)
            ));
            return;
        }
    }

    // `enum`
    if let Some(Value::Array(allowed)) = obj.get("enum") {
        if !allowed.iter().any(|a| a == value) {
            errors.push(format!("{path}: value is not one of the allowed enum values"));
        }
    }

    // object keywords
    if let Value::Object(map) = value {
        if let Some(Value::Array(req)) = obj.get("required") {
            for name in req.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    errors.push(format!("{path}: missing required property \"{name}\""));
                }
            }
        }
        let props = obj.get("properties").and_then(Value::as_object);
        let no_additional = obj.get("additionalProperties") == Some(&Value::Bool(false));
        if props.is_some() || no_additional {
            for (k, v) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(subschema) => validate_at(v, subschema, &format!("{path}.{k}"), errors),
                    None if no_additional => {
                        errors.push(format!("{path}: additional property \"{k}\" is not allowed"));
                    }
                    None => {}
                }
            }
        }
    }

    // array keyword
    if let Value::Array(arr) = value {
        if let Some(item_schema) = obj.get("items") {
            for (i, item) in arr.iter().enumerate() {
                validate_at(item, item_schema, &format!("{path}[{i}]"), errors);
            }
        }
    }
}

fn type_matches(value: &Value, ty: &Value) -> bool {
    match ty {
        Value::String(s) => single_type_matches(value, s),
        // JSON Schema allows `type` to be an array of acceptable types.
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|s| single_type_matches(value, s)),
        _ => true, // malformed `type` → do not enforce
    }
}

fn single_type_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        // `integer`: an integral number (i64/u64, or a float with no fraction).
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => true, // unknown type token → do not enforce
    }
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_desc(ty: &Value) -> String {
    match ty {
        Value::String(s) => s.clone(),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"),
        _ => "<any>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_budget_defaults_and_overrides() {
        assert_eq!(resolve_max_retries(None), 5);
        assert_eq!(resolve_max_retries(Some("3")), 3);
        assert_eq!(resolve_max_retries(Some("0")), 5); // non-positive → default
        assert_eq!(resolve_max_retries(Some("nope")), 5); // unparseable → default
        assert_eq!(resolve_max_retries(Some("  7 ")), 7); // trimmed
    }

    #[test]
    fn valid_object_passes() {
        let schema = json!({
            "type": "object",
            "required": ["name", "age"],
            "properties": { "name": { "type": "string" }, "age": { "type": "integer" } }
        });
        assert!(validate(&json!({"name": "Ada", "age": 36}), &schema).is_empty());
    }

    #[test]
    fn missing_required_property_fails() {
        let schema = json!({ "type": "object", "required": ["name"] });
        let errs = validate(&json!({}), &schema);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("missing required property \"name\""), "{errs:?}");
    }

    #[test]
    fn wrong_property_type_fails() {
        let schema = json!({
            "type": "object",
            "properties": { "age": { "type": "integer" } }
        });
        let errs = validate(&json!({"age": "old"}), &schema);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("$.age: expected type integer, got string"), "{errs:?}");
    }

    #[test]
    fn integer_accepts_integral_float_rejects_fraction() {
        let schema = json!({ "type": "integer" });
        assert!(validate(&json!(5.0), &schema).is_empty());
        assert!(!validate(&json!(5.5), &schema).is_empty());
    }

    #[test]
    fn enum_constraint() {
        let schema = json!({ "enum": ["a", "b"] });
        assert!(validate(&json!("a"), &schema).is_empty());
        assert!(!validate(&json!("c"), &schema).is_empty());
    }

    #[test]
    fn nested_array_items_validated() {
        let schema = json!({
            "type": "array",
            "items": { "type": "object", "required": ["id"], "properties": { "id": { "type": "integer" } } }
        });
        assert!(validate(&json!([{"id": 1}, {"id": 2}]), &schema).is_empty());
        let errs = validate(&json!([{"id": 1}, {"name": "x"}]), &schema);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("$[1]: missing required property \"id\""), "{errs:?}");
    }

    #[test]
    fn type_array_accepts_either() {
        let schema = json!({ "type": ["string", "null"] });
        assert!(validate(&json!("x"), &schema).is_empty());
        assert!(validate(&json!(null), &schema).is_empty());
        assert!(!validate(&json!(42), &schema).is_empty());
    }

    #[test]
    fn additional_properties_false_rejects_extras() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "string" } },
            "additionalProperties": false
        });
        assert!(validate(&json!({"a": "x"}), &schema).is_empty());
        let errs = validate(&json!({"a": "x", "b": 1}), &schema);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("additional property \"b\""), "{errs:?}");
    }

    #[test]
    fn unsupported_keywords_pass_rather_than_falsely_reject() {
        // `$ref` / `pattern` / numeric bounds aren't enforced — they must not
        // cause a spurious failure on otherwise-conforming data.
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "number", "minimum": 10, "pattern": "x" } }
        });
        assert!(validate(&json!({"n": 1}), &schema).is_empty());
    }

    #[test]
    fn decision_emits_on_conforming_value() {
        let schema = json!({ "type": "object", "required": ["x"] });
        assert_eq!(
            structured_output_decision(Some(json!({"x": 1})), &schema),
            StructuredDecision::Emit(json!({"x": 1}))
        );
    }

    #[test]
    fn decision_retries_with_errors_on_invalid_value() {
        let schema = json!({ "type": "object", "required": ["x"] });
        match structured_output_decision(Some(json!({})), &schema) {
            StructuredDecision::Retry(msg) => assert!(msg.contains("did not conform"), "{msg}"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn decision_retries_when_tool_was_not_called() {
        match structured_output_decision(None, &json!({"type": "object"})) {
            StructuredDecision::Retry(msg) => {
                assert!(msg.contains("must call the StructuredOutput"), "{msg}");
            }
            other => panic!("expected Retry, got {other:?}"),
        }
    }
}
