use llm_client::normalize_anthropic_usage;

#[test]
fn normalizes_anthropic_usage_into_independent_billing_buckets() {
    let value = serde_json::json!({
        "input_tokens": 100,
        "output_tokens": 20,
        "cache_creation_input_tokens": 30,
        "cache_read_input_tokens": 40,
        "server_tool_use": {
            "web_search_requests": 2
        }
    });

    let usage = normalize_anthropic_usage(&value);

    assert_eq!(usage.billable_tokens.input, 100);
    assert_eq!(usage.billable_tokens.output, 20);
    assert_eq!(usage.billable_tokens.cache_write, 30);
    assert_eq!(usage.billable_tokens.cache_read, 40);
    assert_eq!(usage.billable_tokens.reasoning_output, 0);
    assert_eq!(
        usage
            .server_tool_use
            .expect("server tool usage")
            .web_search_requests,
        2
    );
}
