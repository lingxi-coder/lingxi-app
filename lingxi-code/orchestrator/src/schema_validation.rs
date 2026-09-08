//! JSON-Schema input-validation for tool dispatch — the Rust analogue of
//! claude-code's `inputSchema.safeParse` (`toolExecution.ts:615`).
//!
//! Boon validates exported JSON Schema; tools additionally retain native Zod
//! refinement issues at the tool boundary. Diagnostics use 2.1.263 `zue`.

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

/// Zod v4's issue objects survive at the tool boundary for constraints that
/// JSON Schema cannot express. `zue` groups structural errors, falling back to
/// the two-space JSON representation of *all* issues only when none group.
pub(crate) fn validate_tool_schema(
    tool: &dyn tool_api::Tool,
    input: &serde_json::Value,
) -> Result<(), String> {
    let mut issues = Vec::new();
    let fallback = validate_tool_input_schema(tool.input_validation_schema(), input).err();
    if !collect_issues(
        tool.input_validation_schema(),
        Some(input),
        &[],
        &mut issues,
    ) {
        return fallback.map_or(Ok(()), Err);
    }
    let structural_count = issues.len();
    for native in tool.input_validation_issues(input) {
        if let Some(existing) = issues[..structural_count]
            .iter_mut()
            .find(|issue| issue["code"] == native["code"] && issue["path"] == native["path"])
        {
            *existing = native;
        } else {
            issues.push(native);
        }
    }
    if issues.is_empty() {
        return Ok(());
    }
    issues.sort_by_key(|issue| {
        declaration_order(
            tool.input_validation_schema(),
            issue["path"].as_array().map_or(&[][..], Vec::as_slice),
        )
    });
    Err(format_issues(tool.name(), &issues))
}

#[cfg(test)]
pub(crate) fn validate_named_tool_input_schema(
    tool_name: &str,
    schema: &serde_json::Value,
    input: &serde_json::Value,
) -> Result<(), String> {
    let mut issues = Vec::new();
    if collect_issues(schema, Some(input), &[], &mut issues) {
        if issues.is_empty() {
            Ok(())
        } else {
            Err(format_issues(tool_name, &issues))
        }
    } else {
        validate_tool_input_schema(schema, input)
    }
}

fn js_keys(object: &serde_json::Map<String, serde_json::Value>) -> Vec<&String> {
    fn index(key: &str) -> Option<u32> {
        let n = key.parse::<u32>().ok()?;
        (n != u32::MAX && n.to_string() == key).then_some(n)
    }
    let mut keys: Vec<_> = object.keys().collect();
    keys.sort_by_key(|key| index(key).map_or((1, 0), |n| (0, n)));
    keys
}

fn js_number(value: &serde_json::Number) -> String {
    let n = value
        .as_f64()
        .expect("JSON number has an f64 representation");
    if n == 0.0 {
        return "0".into();
    }
    if !n.is_finite() {
        return "null".into();
    }
    if n.abs() >= 1e21 || n.abs() < 1e-6 {
        let scientific = format!("{n:e}");
        let (mantissa, exponent) = scientific.split_once('e').expect("scientific exponent");
        let exponent: i32 = exponent.parse().expect("numeric exponent");
        format!(
            "{mantissa}e{}{exponent}",
            if exponent >= 0 { "+" } else { "" }
        )
    } else {
        n.to_string()
    }
}

/// JSON.stringify(value, null, 2), including JS Number and property ordering.
fn js_json(value: &serde_json::Value, pretty: bool) -> String {
    fn render(value: &serde_json::Value, level: usize, pretty: bool) -> String {
        use serde_json::Value;
        match value {
            Value::Number(n) => js_number(n),
            Value::Array(array) if !array.is_empty() => {
                let values: Vec<_> = array.iter().map(|v| render(v, level + 1, pretty)).collect();
                if pretty {
                    format!(
                        "[\n{}{}\n{}]",
                        "  ".repeat(level + 1),
                        values.join(&format!(",\n{}", "  ".repeat(level + 1))),
                        "  ".repeat(level)
                    )
                } else {
                    format!("[{}]", values.join(","))
                }
            }
            Value::Object(object) if !object.is_empty() => {
                let values: Vec<_> = js_keys(object)
                    .into_iter()
                    .map(|key| {
                        format!(
                            "{}:{}{}",
                            serde_json::to_string(key).unwrap(),
                            if pretty { " " } else { "" },
                            render(&object[key], level + 1, pretty)
                        )
                    })
                    .collect();
                if pretty {
                    format!(
                        "{{\n{}{}\n{}}}",
                        "  ".repeat(level + 1),
                        values.join(&format!(",\n{}", "  ".repeat(level + 1))),
                        "  ".repeat(level)
                    )
                } else {
                    format!("{{{}}}", values.join(","))
                }
            }
            _ => serde_json::to_string(value).expect("JSON value"),
        }
    }
    render(value, 0, pretty)
}

fn js_value_eq(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    if left.is_number() && right.is_number() {
        left.as_f64() == right.as_f64()
    } else {
        left == right
    }
}

fn declaration_order(schema: &serde_json::Value, path: &[serde_json::Value]) -> Vec<usize> {
    let mut schema = schema;
    let mut order = Vec::new();
    for part in path {
        if let Some(key) = part.as_str() {
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            order.push(
                properties
                    .and_then(|props| js_keys(props).into_iter().position(|name| name == key))
                    .unwrap_or(usize::MAX),
            );
            schema = properties
                .and_then(|props| props.get(key))
                .unwrap_or(&serde_json::Value::Null);
        } else {
            order.push(
                part.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(usize::MAX),
            );
            schema = schema.get("items").unwrap_or(&serde_json::Value::Null);
        }
    }
    // Object refinements run after child schemas (a parent path is last).
    order.push(usize::MAX);
    order
}

fn issue_path(path: &[serde_json::Value]) -> String {
    let mut result = String::new();
    for (index, part) in path.iter().enumerate() {
        if let Some(key) = part.as_str() {
            if index != 0 {
                result.push('.');
            }
            result.push_str(key);
        } else {
            result.push_str(&format!("[{part}]"));
        }
    }
    result
}

fn format_issues(name: &str, issues: &[serde_json::Value]) -> String {
    let mut missing = Vec::new();
    let mut unexpected = Vec::new();
    let mut types = Vec::new();
    for issue in issues {
        let path = issue_path(issue["path"].as_array().map_or(&[][..], Vec::as_slice));
        let message = issue["message"].as_str().unwrap_or("");
        match issue["code"].as_str() {
            Some("invalid_type") if message.contains("received undefined") => {
                missing.push(format!("The required parameter `{path}` is missing"))
            }
            Some("invalid_type") => {
                let received = message
                    .split_once("received ")
                    .map(|(_, suffix)| {
                        suffix
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .collect::<String>()
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "unknown".into());
                let expected = issue["expected"].as_str().unwrap_or("undefined");
                types.push(format!("The parameter `{path}` type is expected as `{expected}` but provided as `{received}`"));
            }
            Some("unrecognized_keys") => {
                if let Some(keys) = issue["keys"].as_array() {
                    unexpected.extend(
                        keys.iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(|key| format!("An unexpected parameter `{key}` was provided")),
                    );
                }
            }
            _ => {}
        }
    }
    let messages: Vec<_> = missing.into_iter().chain(unexpected).chain(types).collect();
    if messages.is_empty() {
        js_json(&serde_json::Value::Array(issues.to_vec()), true)
    } else {
        format!(
            "{name} failed due to the following {}:\n{}",
            if messages.len() == 1 {
                "issue"
            } else {
                "issues"
            },
            messages.join("\n")
        )
    }
}

fn value_type(value: Option<&serde_json::Value>) -> &'static str {
    use serde_json::Value;
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn quoted_options(values: &[serde_json::Value], separator: &str) -> String {
    values
        .iter()
        .map(|value| js_json(value, false))
        .collect::<Vec<_>>()
        .join(separator)
}

fn collect_issues(
    schema: &serde_json::Value,
    input: Option<&serde_json::Value>,
    path: &[serde_json::Value],
    issues: &mut Vec<serde_json::Value>,
) -> bool {
    use serde_json::{json, Value};
    let Some(object) = schema.as_object() else {
        return schema == &json!(true);
    };
    // Unexported Zod metadata comes from Tool::input_validation_issues. Never
    // manufacture errors for unknown JSON Schema vocabulary.
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "title"
                | "description"
                | "$schema"
                | "default"
                | "enum"
                | "const"
                | "anyOf"
                | "oneOf"
                | "pattern"
                | "format"
                | "minLength"
                | "maxLength"
                | "minItems"
                | "maxItems"
                | "minimum"
                | "maximum"
                | "exclusiveMinimum"
                | "exclusiveMaximum"
        )
    }) {
        return false;
    }
    if let Some(branches) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut errors = Vec::new();
        let mut dirty = None;
        for branch in branches {
            let mut branch_issues = Vec::new();
            if !collect_issues(branch, input, &[], &mut branch_issues) {
                return false;
            }
            if branch_issues.is_empty() {
                return true;
            }
            if dirty.is_none()
                && branch_issues.iter().all(|issue| {
                    !matches!(
                        issue["code"].as_str(),
                        Some("invalid_type" | "invalid_value" | "invalid_union")
                    )
                })
            {
                dirty = Some(branch_issues.clone());
            }
            errors.push(json!(branch_issues));
        }
        if let Some(mut dirty) = dirty {
            for issue in &mut dirty {
                let mut full = path.to_vec();
                full.extend(issue["path"].as_array().into_iter().flatten().cloned());
                issue["path"] = json!(full);
            }
            issues.extend(dirty);
        } else {
            issues.push(json!({"code":"invalid_union","errors":errors,"path":path,"message":"Invalid input"}));
        }
        return true;
    }
    let values = schema
        .get("enum")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| schema.get("const").map(|value| vec![value.clone()]));
    if let Some(values) = values {
        if !values
            .iter()
            .any(|value| input.is_some_and(|input| js_value_eq(input, value)))
        {
            let message = if values.len() == 1 {
                format!("Invalid input: expected {}", js_json(&values[0], false))
            } else {
                format!(
                    "Invalid option: expected one of {}",
                    quoted_options(&values, "|")
                )
            };
            issues.push(
                json!({"code":"invalid_value","values":values,"path":path,"message":message}),
            );
        }
        return true;
    }
    let received = value_type(input);
    let expected = schema.get("type").and_then(Value::as_str);
    if schema.get("type").is_some() && expected.is_none() {
        return false;
    }
    if let Some(expected) = expected {
        if !matches!(
            expected,
            "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
        ) {
            return false;
        }
        let matches = expected == received
            || (expected == "integer"
                && input.is_some_and(|v| v.as_f64().is_some_and(|n| n.fract() == 0.0)));
        if !matches {
            if expected == "integer" && received == "number" {
                issues.push(json!({"expected":"int","format":"safeint","code":"invalid_type","path":path,"message":"Invalid input: expected int, received number"}));
            } else {
                issues.push(json!({"expected":expected,"code":"invalid_type","path":path,"message":format!("Invalid input: expected {expected}, received {received}")}));
            }
            return true;
        }
    }

    let Some(input) = input else {
        return false;
    };
    if let Some(input) = input.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        let required = schema.get("required").and_then(Value::as_array);
        if required.is_some_and(|keys| {
            keys.iter().any(|key| {
                !properties
                    .is_some_and(|props| key.as_str().is_some_and(|key| props.contains_key(key)))
            })
        }) {
            return false;
        }
        if let Some(properties) = properties {
            for key in js_keys(properties) {
                let child = &properties[key];
                let value = input.get(key);
                if value.is_none()
                    && !required.is_some_and(|keys| keys.iter().any(|v| v.as_str() == Some(key)))
                {
                    continue;
                }
                let mut child_path = path.to_vec();
                child_path.push(json!(key));
                if !collect_issues(child, value, &child_path, issues) {
                    return false;
                }
            }
        }
        if schema.get("additionalProperties") == Some(&json!(false)) {
            let keys: Vec<_> = js_keys(input)
                .into_iter()
                .filter(|key| !properties.is_some_and(|p| p.contains_key(*key)))
                .cloned()
                .collect();
            if !keys.is_empty() {
                issues.push(json!({"code":"unrecognized_keys","keys":keys,"path":path,"message":format!("Unrecognized key(s) in object: {}", keys.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", "))}));
            }
        } else if let Some(extra_schema) =
            schema.get("additionalProperties").filter(|v| v.is_object())
        {
            for (key, value) in input {
                if properties.is_some_and(|props| props.contains_key(key)) {
                    continue;
                }
                let mut child_path = path.to_vec();
                child_path.push(json!(key));
                if !collect_issues(extra_schema, Some(value), &child_path, issues) {
                    return false;
                }
            }
        }
    }
    if let (Some(value), Some(pattern)) = (
        input.as_str(),
        schema.get("pattern").and_then(Value::as_str),
    ) {
        let Ok(regex) = regex::Regex::new(pattern) else {
            return false;
        };
        if !regex.is_match(value) {
            issues.push(json!({"origin":"string","code":"invalid_format","format":"regex","pattern":format!("/{pattern}/"),"path":path,"message":format!("Invalid string: must match pattern /{pattern}/")}));
        }
    }
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        if format != "uri" {
            return false;
        }
        if input
            .as_str()
            .is_some_and(|value| url::Url::parse(value).is_err())
        {
            issues.push(
                json!({"code":"invalid_format","format":"url","path":path,"message":"Invalid URL"}),
            );
        }
    }
    if let (Some(array), Some(items)) = (input.as_array(), schema.get("items")) {
        for (index, value) in array.iter().enumerate() {
            let mut child_path = path.to_vec();
            child_path.push(json!(index));
            if !collect_issues(items, Some(value), &child_path, issues) {
                return false;
            }
        }
    }
    // Zod's string length checks count UTF-16 code units, as JavaScript does.
    let measure = input
        .as_str()
        .map(|s| ("string", s.encode_utf16().count() as f64))
        .or_else(|| input.as_array().map(|v| ("array", v.len() as f64)))
        .or_else(|| input.as_f64().map(|n| ("number", n)));
    if let Some((kind, value)) = measure {
        for (key, small, inclusive) in [
            ("minLength", true, true),
            ("maxLength", false, true),
            ("minItems", true, true),
            ("maxItems", false, true),
            ("minimum", true, true),
            ("maximum", false, true),
            ("exclusiveMinimum", true, false),
            ("exclusiveMaximum", false, false),
        ] {
            let Some(limit) = schema.get(key) else {
                continue;
            };
            let Some(bound) = limit.as_f64() else {
                return false;
            };
            let fails = if small {
                value < bound || (!inclusive && value == bound)
            } else {
                value > bound || (!inclusive && value == bound)
            };
            if !fails {
                continue;
            }
            let display_limit = limit
                .as_number()
                .map_or_else(|| limit.to_string(), js_number);
            let relation = if small {
                if inclusive {
                    ">="
                } else {
                    ">"
                }
            } else if inclusive {
                "<="
            } else {
                "<"
            };
            let size = if small { "small" } else { "big" };
            let message = match kind {
                "string" => {
                    format!(
                        "Too {size}: expected string to have {relation}{display_limit} characters"
                    )
                }
                "array" => {
                    format!("Too {size}: expected array to have {relation}{display_limit} items")
                }
                _ => format!("Too {size}: expected number to be {relation}{display_limit}"),
            };
            let mut issue = serde_json::Map::new();
            issue.insert("origin".into(), json!(kind));
            issue.insert(
                "code".into(),
                json!(if small { "too_small" } else { "too_big" }),
            );
            issue.insert(
                if small { "minimum" } else { "maximum" }.into(),
                limit.clone(),
            );
            issue.insert("inclusive".into(), json!(inclusive));
            issue.insert("path".into(), json!(path));
            issue.insert("message".into(), json!(message));
            issues.push(Value::Object(issue));
        }
    }

    true
}

/// Validate a `PostToolUse`-hook `updatedToolOutput` replacement against the
/// tool's declared output JSON Schema (claude-code's `outputSchema.safeParse`,
/// BIN off 202169384: `e.outputSchema?.safeParse(D.updatedToolOutput)
/// ?.success!==!1`). Returns the concise error detail on a schema MISMATCH so
/// the caller can surface the `... does not match <tool>'s output shape ...`
/// meta message (BIN off 202465455) and keep the original output.
///
/// Identical compile/validate logic to [`validate_tool_input_schema`]: a
/// schema that fails to COMPILE is treated as PASS (a `LingXi` schema bug must
/// not discard a hook's valid replacement), matching the `?.` short-circuit in
/// the binary (`success!==!1` is also satisfied when `safeParse` is `undefined`
/// because `outputSchema` is absent — handled by the caller's `None` branch).
pub(crate) fn validate_tool_output_schema(
    schema: &serde_json::Value,
    output: &serde_json::Value,
) -> Result<(), String> {
    const URL: &str = "mem://tool-output-schema";
    let mut schemas = Schemas::new();
    let mut compiler = Compiler::new();

    if let Err(e) = compiler.add_resource(URL, schema.clone()) {
        tracing::warn!(
            error = %e,
            "tool output_schema failed to load (treating as PASS — `LingXi` schema bug, not hook output error)"
        );
        return Ok(());
    }
    let sch = match compiler.compile(URL, &mut schemas) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "tool output_schema failed to compile (treating as PASS — `LingXi` schema bug, not hook output error)"
            );
            return Ok(());
        }
    };

    if let Err(err) = schemas.validate(output, sch) {
        let mut out = Vec::new();
        flatten(&err, &mut out);
        if out.is_empty() {
            out.push(err.kind.to_string());
        }
        return Err(out.join("; "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_named_tool_input_schema, validate_tool_input_schema, validate_tool_output_schema,
    };
    use serde_json::json;

    #[test]
    fn issue_json_uses_javascript_numbers_and_integer_key_order() {
        assert_eq!(
            super::js_json(&json!([1.0, -0.0, 1e-7, 1e-6, 1e20, 1e21]), false),
            "[1,0,1e-7,0.000001,100000000000000000000,1e+21]"
        );
        assert_eq!(validate_named_tool_input_schema("Tool", &json!({"type":"object","properties":{},"additionalProperties":false}), &json!({"10":1,"2":1,"01":1})).unwrap_err(), "Tool failed due to the following issues:\nAn unexpected parameter `2` was provided\nAn unexpected parameter `10` was provided\nAn unexpected parameter `01` was provided");
        assert!(validate_named_tool_input_schema("Tool", &json!({"const":1}), &json!(1.0)).is_ok());
    }

    #[test]
    fn diagnostics_match_executed_2_1_263_zod_4_4_3_oracle() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/schema-validation-2.1.263.json"))
                .unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let actual = validate_named_tool_input_schema("Tool", &case["schema"], &case["input"]);
            let expected = case["expected"]
                .as_str()
                .map_or(Ok(()), |s| Err(s.to_owned()));
            assert_eq!(actual, expected, "oracle case {}", case["name"]);
        }
    }

    #[test]
    fn named_error_matches_oracle_grouping_and_bytes() {
        let schema = json!({"type":"object", "properties": {
            "a": {"type":"number"}, "path": {"type":"string"}
        }, "required":["path"], "additionalProperties":false});
        assert_eq!(
            validate_named_tool_input_schema("Read", &schema, &json!({"a":true,"extra":1}))
                .unwrap_err(),
            "Read failed due to the following issues:\nThe required parameter `path` is missing\nAn unexpected parameter `extra` was provided\nThe parameter `a` type is expected as `number` but provided as `boolean`"
        );
    }

    #[test]
    fn named_error_uses_zod_array_path_and_singular_prefix() {
        let schema = json!({"type":"object", "properties":{"rows":{"type":"array", "items":{"type":"object", "properties":{"name":{"type":"string"}}, "required":["name"]}}}});
        assert_eq!(
            validate_named_tool_input_schema("Tool", &schema, &json!({"rows":[{}]})).unwrap_err(),
            "Tool failed due to the following issue:\nThe required parameter `rows[0].name` is missing"
        );
    }

    #[test]
    fn enum_error_preserves_zod_issue_bytes() {
        let schema = json!({"type":"string", "enum":["a","b"]});
        let expected = json!([{"code":"invalid_value","values":["a","b"],"path":[],"message":"Invalid option: expected one of \"a\"|\"b\""}]);
        assert_eq!(
            validate_named_tool_input_schema("Tool", &schema, &json!("c")).unwrap_err(),
            serde_json::to_string_pretty(&expected).unwrap()
        );
    }

    #[test]
    fn string_constraints_count_utf16_code_units() {
        assert!(validate_named_tool_input_schema(
            "Tool",
            &json!({"type":"string","minLength":2}),
            &json!("😀")
        )
        .is_ok());
        let error = validate_named_tool_input_schema(
            "Tool",
            &json!({"type":"string","maxLength":1}),
            &json!("😀"),
        )
        .unwrap_err();
        let issues: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(issues[0]["code"], "too_big");
    }

    #[test]
    fn structural_errors_win_over_enum_issues_and_keep_declaration_order() {
        let schema = json!({"type":"object", "properties": {
            "z": {"type":"string"}, "a":{"type":"string"}, "mode":{"type":"string", "enum":["a"]}
        }, "required":["z","a","mode"]});
        assert_eq!(validate_named_tool_input_schema("Tool", &schema, &json!({"mode":"bad"})).unwrap_err(),
            "Tool failed due to the following issues:\nThe required parameter `z` is missing\nThe required parameter `a` is missing");
    }

    #[test]
    fn union_returns_nested_zod_errors_and_prefers_dirty_branch() {
        let schema = json!({"anyOf":[{"type":"string","minLength":5},{"type":"number"}]});
        let error = validate_named_tool_input_schema("Tool", &schema, &json!("a")).unwrap_err();
        let issues: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(issues[0]["code"], "too_small");
        let error = validate_named_tool_input_schema("Tool", &schema, &json!(true)).unwrap_err();
        let issues: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(issues[0]["code"], "invalid_union");
        assert_eq!(issues[0]["errors"][0][0]["code"], "invalid_type");
    }

    #[test]
    fn native_custom_issues_and_type_message_overrides_follow_zue() {
        let issue = json!({"code":"custom","path":[],"message":"Duplicate questions"});
        assert_eq!(
            super::format_issues("Tool", &[issue.clone()]),
            serde_json::to_string_pretty(&vec![issue]).unwrap()
        );
        let issue = json!({"code":"invalid_type","expected":"string","received":"number","path":["field"],"message":"custom message"});
        assert_eq!(super::format_issues("Tool", &[issue]), "Tool failed due to the following issue:\nThe parameter `field` type is expected as `string` but provided as `unknown`");
    }

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
        assert!(
            err.contains("/path"),
            "message should locate the field: {err}"
        );
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

    #[test]
    fn output_schema_mismatch_returns_detail() {
        // #38: a hook `updatedToolOutput` that does not match the tool's output
        // schema must FAIL so the caller keeps the original output and emits the
        // `... does not match <tool>'s output shape ...` meta message.
        let schema = json!({
            "type": "object",
            "properties": { "result": { "type": "string" } },
            "required": ["result"],
        });
        let err = validate_tool_output_schema(&schema, &json!({})).unwrap_err();
        assert!(
            err.contains("result"),
            "message should name the field: {err}"
        );
    }

    #[test]
    fn output_schema_match_passes() {
        let schema = json!({
            "type": "object",
            "properties": { "result": { "type": "string" } },
            "required": ["result"],
        });
        assert!(validate_tool_output_schema(&schema, &json!({ "result": "ok" })).is_ok());
    }

    #[test]
    fn output_schema_malformed_passes_through() {
        // A schema bug must not discard a hook's valid replacement.
        let schema = json!("not a schema");
        assert!(validate_tool_output_schema(&schema, &json!({ "x": 1 })).is_ok());
    }
}
