//! JSON-Schema input-validation for tool dispatch — the Rust analogue of
//! claude-code's `inputSchema.safeParse` (`toolExecution.ts:615`).
//!
//! BEHAVIORAL parity only: this gate decides PASS/FAIL the same way claude-code
//! does (validate the raw model-supplied input against the tool's declared
//! JSON Schema before the tool runs), but the error MESSAGE bytes intentionally
//! differ from claude-code's Zod `formatZodValidationError` output, which is not
//! portable to a JSON-Schema validator.

use boon::{Compiler, Schemas, ValidationError};

/// Recursively flatten a boon [`ValidationError`] into concise leaf messages.
///
/// boon's `Display` is multi-line and prefixed (`jsonschema validation failed
/// with …`); we want a single compact line suitable for a `<tool_use_error>`
/// body, so we walk to the leaf causes and render `at '<loc>': <kind>` for each.
fn flatten(err: &ValidationError, out: &mut Vec<String>) {
    if err.causes.is_empty() {
        let loc = err.instance_location.to_string();
        let loc = if loc.is_empty() {
            "(root)".to_string()
        } else {
            loc
        };
        out.push(format!("at '{loc}': {}", err.kind));
    } else {
        for cause in &err.causes {
            flatten(cause, out);
        }
    }
}

/// Validate a tool input against its JSON Schema (claude-code's
/// `inputSchema.safeParse`). Returns a concise human-readable error on failure.
/// BEHAVIORAL parity only — the message bytes intentionally differ from
/// claude-code's Zod `formatZodValidationError` output (unportable).
///
/// If the schema itself fails to COMPILE (malformed schema), this returns `Ok`
/// (PASS) and logs a `tracing::warn!`: claude-code's tools always carry valid
/// Zod schemas, so a compile failure is a `LingXi` schema bug, not a model-input
/// error — we must not block a tool call because its own schema is broken.
///
/// PERF: this compiles the schema on every call. Tools are few and schemas are
/// small, so the cost is negligible; if a hot path emerges, memoize the
/// compiled `boon::SchemaIndex` per tool (keyed by the schema's identity).
pub(crate) fn validate_tool_input_schema(
    schema: &serde_json::Value,
    input: &serde_json::Value,
) -> Result<(), String> {
    // boon addresses schemas by URL; an in-memory pseudo-URL is sufficient for a
    // self-contained, dependency-free tool schema (no `$ref` to external docs).
    const URL: &str = "mem://tool-input-schema";
    let mut schemas = Schemas::new();
    let mut compiler = Compiler::new();

    if let Err(e) = compiler.add_resource(URL, schema.clone()) {
        tracing::warn!(
            error = %e,
            "tool input_schema failed to load (treating as PASS — `LingXi` schema bug, not model input error)"
        );
        return Ok(());
    }
    let sch = match compiler.compile(URL, &mut schemas) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "tool input_schema failed to compile (treating as PASS — `LingXi` schema bug, not model input error)"
            );
            return Ok(());
        }
    };

    if let Err(err) = schemas.validate(input, sch) {
        let mut out = Vec::new();
        flatten(&err, &mut out);
        // Defensive: a root error with no causes and an empty-rendering `kind`
        // would otherwise yield an empty message; fall back to the kind string.
        if out.is_empty() {
            out.push(err.kind.to_string());
        }
        return Err(out.join("; "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_tool_input_schema;
    use serde_json::json;

    #[test]
    fn missing_required_field_fails() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        let err = validate_tool_input_schema(&schema, &json!({})).unwrap_err();
        assert!(err.contains("path"), "message should name the field: {err}");
    }

    #[test]
    fn type_mismatch_fails() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        let err = validate_tool_input_schema(&schema, &json!({ "path": 123 })).unwrap_err();
        assert!(err.contains("/path"), "message should locate the field: {err}");
    }

    #[test]
    fn valid_input_passes() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        });
        assert!(validate_tool_input_schema(&schema, &json!({ "path": "/x" })).is_ok());
    }

    #[test]
    fn malformed_schema_passes_through() {
        // A JSON scalar is not a valid Draft-07 schema (a schema must be an
        // object or boolean), so boon fails to compile it and we take the
        // warn-and-PASS branch. A schema bug must never block the model's call.
        let schema = json!("not a schema object at all");
        assert!(validate_tool_input_schema(&schema, &json!({ "anything": true })).is_ok());
    }

    #[test]
    fn invalid_type_keyword_schema_passes_through() {
        // `type` must be a string or array of strings; `123` makes the schema
        // itself invalid, forcing boon's compile-error branch → warn + PASS.
        let schema = json!({ "type": 123 });
        assert!(validate_tool_input_schema(&schema, &json!({ "x": 1 })).is_ok());
    }

    #[test]
    fn empty_object_schema_accepts_anything() {
        let schema = json!({ "type": "object" });
        assert!(validate_tool_input_schema(&schema, &json!({ "a": 1, "b": [2] })).is_ok());
    }
}
