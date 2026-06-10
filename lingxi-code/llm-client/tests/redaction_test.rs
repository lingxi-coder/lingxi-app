use std::collections::BTreeMap;

use llm_client::Redactor;

#[test]
fn redacts_secret_headers_case_insensitively() {
    let mut headers = BTreeMap::new();
    headers.insert("Authorization".to_string(), "Bearer secret-token".to_string());
    headers.insert("x-api-key".to_string(), "secret-key".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());

    let redacted = Redactor.redact_headers(&headers);

    assert_eq!(redacted.get("Authorization"), Some(&"[REDACTED]".to_string()));
    assert_eq!(redacted.get("x-api-key"), Some(&"[REDACTED]".to_string()));
    assert_eq!(redacted.get("content-type"), Some(&"application/json".to_string()));
}

#[test]
fn redacts_secret_query_parameters() {
    let url = "https://example.com/v1/messages?api_key=secret&model=claude&access_token=token&signature=sig";

    let redacted = Redactor.redact_url(url);

    assert!(redacted.contains("api_key=[REDACTED]"));
    assert!(redacted.contains("model=claude"));
    assert!(redacted.contains("access_token=[REDACTED]"));
    assert!(redacted.contains("signature=[REDACTED]"));
    assert!(!redacted.contains("=secret"));
    assert!(!redacted.contains("=token"));
    assert!(!redacted.contains("=sig"));
}

#[test]
fn redacts_secret_json_fields_recursively() {
    let value = serde_json::json!({
        "api_key": "secret-key",
        "nested": {
            "access_token": "secret-token",
            "safe": "visible"
        },
        "items": [
            {"refresh_token": "refresh-secret"},
            {"name": "plain"}
        ]
    });

    let redacted = Redactor.redact_json(&value);

    assert_eq!(redacted["api_key"], "[REDACTED]");
    assert_eq!(redacted["nested"]["access_token"], "[REDACTED]");
    assert_eq!(redacted["nested"]["safe"], "visible");
    assert_eq!(redacted["items"][0]["refresh_token"], "[REDACTED]");
    assert_eq!(redacted["items"][1]["name"], "plain");
}
