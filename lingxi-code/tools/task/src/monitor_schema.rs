//! Monitor's native Zod parse: defaults, stripping, and declaration-ordered issues.
use serde_json::{json, Map, Value};
use tool_api::native_schema::{format_issues, js_json, js_keys, value_type, NativeSchemaError};

pub(super) fn hidden_control(text: &str) -> bool {
    text.chars()
        .any(|c| c != '\t' && c != '\n' && ((c as u32) < 32 || (127..=159).contains(&(c as u32))))
}

fn type_issue(expected: &str, input: Option<&Value>, path: Vec<Value>) -> Value {
    json!({"expected":expected,"code":"invalid_type","path":path,"message":format!("Invalid input: expected {expected}, received {}",value_type(input))})
}
fn custom(path: Vec<Value>, message: &str) -> Value {
    json!({"code":"custom","path":path,"message":message})
}
fn finish(parsed: Value, issues: Vec<Value>) -> Result<Value, NativeSchemaError> {
    if issues.is_empty() {
        Ok(parsed)
    } else {
        Err(NativeSchemaError {
            display: format_issues("Monitor", &issues),
            raw: js_json(&Value::Array(issues), true),
        })
    }
}

pub(super) fn parse(input: &Value, bounded: bool) -> Result<Value, NativeSchemaError> {
    let Some(object) = input.as_object() else {
        return finish(Value::Null, vec![type_issue("object", Some(input), vec![])]);
    };
    let mut issues = Vec::new();
    let mut parsed = Map::new();
    for (field, expected, default) in [
        ("description", "string", None),
        ("timeout_ms", "number", Some(json!(300000))),
        ("persistent", "boolean", Some(json!(false))),
        ("command", "string", None),
        ("ws", "object", None),
    ] {
        if field == "persistent" && bounded {
            continue;
        }
        let value = object.get(field).cloned().or(default);
        if value.is_none() && field != "description" {
            continue;
        }
        if value_type(value.as_ref()) != expected {
            issues.push(type_issue(expected, value.as_ref(), vec![json!(field)]));
            continue;
        }
        let mut value = value.unwrap();
        match field {
            "timeout_ms" => {
                let number = value.as_f64().unwrap();
                if number < 1000.0 {
                    issues.push(json!({"origin":"number","code":"too_small","minimum":1000,"inclusive":true,"path":["timeout_ms"],"message":"Too small: expected number to be >=1000"}));
                }
                if bounded && number > 3600000.0 {
                    issues.push(json!({"origin":"number","code":"too_big","maximum":3600000,"inclusive":true,"path":["timeout_ms"],"message":"timeout_ms must be ≤ 3600000"}));
                }
            }
            "command" => {
                if hidden_control(value.as_str().unwrap()) {
                    issues.push(custom(vec![json!(field)], "command contains control characters that would be hidden in the approval dialog"));
                }
            }
            "ws" => {
                let ws = value.as_object().unwrap();
                let mut parsed_ws = Map::new();
                if let Some(url) = ws.get("url").and_then(Value::as_str) {
                    parsed_ws.insert("url".into(), json!(url));
                    if hidden_control(url) {
                        issues.push(custom(vec![json!("ws"), json!("url")], "url contains control characters that would be hidden in the approval dialog"));
                    }
                    if permission::monitor_websocket_url_host(url).is_none() {
                        issues.push(custom(vec![json!("ws"), json!("url")], "url must be a valid ASCII ws:// or wss:// URL with no userinfo or whitespace"));
                    }
                } else {
                    issues.push(type_issue(
                        "string",
                        ws.get("url"),
                        vec![json!("ws"), json!("url")],
                    ));
                }
                if let Some(protocols) = ws.get("protocols") {
                    if let Some(protocols_array) = protocols.as_array() {
                        let mut strings_only = true;
                        for (index, protocol) in protocols_array.iter().enumerate() {
                            let path = vec![json!("ws"), json!("protocols"), json!(index)];
                            if let Some(token) = protocol.as_str() {
                                if token.is_empty()
                                    || !token.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b)
                                    })
                                {
                                    issues.push(json!({"origin":"string","code":"invalid_format","format":"regex","pattern":"/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/","path":path,"message":"protocol must be an RFC 6455 token"}));
                                }
                            } else {
                                strings_only = false;
                                issues.push(type_issue("string", Some(protocol), path));
                            }
                        }
                        if strings_only {
                            let mut seen = std::collections::HashSet::new();
                            if protocols_array
                                .iter()
                                .any(|p| !seen.insert(p.as_str().unwrap()))
                            {
                                issues.push(custom(
                                    vec![json!("ws"), json!("protocols")],
                                    "protocols must be unique",
                                ));
                            }
                        }
                    } else {
                        issues.push(type_issue(
                            "array",
                            Some(protocols),
                            vec![json!("ws"), json!("protocols")],
                        ));
                    }
                    parsed_ws.insert("protocols".into(), protocols.clone());
                }
                value = Value::Object(parsed_ws);
            }
            _ => {}
        }
        parsed.insert(field.into(), value);
    }
    if !bounded {
        let keys: Vec<_> = js_keys(object)
            .into_iter()
            .filter(|k| {
                !matches!(
                    k.as_str(),
                    "description" | "timeout_ms" | "persistent" | "command" | "ws" | "__proto__"
                )
            })
            .cloned()
            .collect();
        if !keys.is_empty() {
            issues.push(json!({"code":"unrecognized_keys","keys":keys,"path":[],"message":format!("Unrecognized key{}: {}",if keys.len()==1 {""}else{"s"},keys.iter().map(|k|format!("\"{k}\"")).collect::<Vec<_>>().join(", "))}));
        }
    }
    let aborted = issues.iter().any(|issue| {
        matches!(
            issue["code"].as_str(),
            Some("invalid_type" | "unrecognized_keys")
        )
    });
    if !aborted {
        // PRESENCE, not emptiness — `monitor.rs`'s runtime check re-decides this
        // same rule with `input.get("command").is_some()`. An emptiness test
        // here accepts `{command: "", ws: {...}}`, which the runtime then
        // rejects with a bare `InvalidInput` instead of the Zod-shaped issue
        // this validator exists to produce.
        let command = parsed.contains_key("command");
        if command == parsed.contains_key("ws") {
            issues.push(custom(vec![], "exactly one of command or ws"));
        }
        if !bounded
            && parsed.get("persistent") != Some(&json!(true))
            && parsed
                .get("timeout_ms")
                .and_then(Value::as_f64)
                .is_some_and(|n| n > 3600000.0)
        {
            issues.push(custom(
                vec![json!("timeout_ms")],
                "timeout_ms must be ≤ 3600000",
            ));
        }
    }
    finish(Value::Object(parsed), issues)
}
