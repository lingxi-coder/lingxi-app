use lingxi_telemetry::tengu::cost;

#[test]
fn all_10_cost_event_names_are_locked() {
    let names: &[&str] = &[
        cost::RECORDED,
        cost::BUDGET_WARNING,
        cost::BUDGET_EXCEEDED,
        cost::SUMMARY_REQUESTED,
        cost::SUMMARY_GENERATED,
        cost::UNKNOWN_MODEL,
        cost::BATCH_DISCOUNT_APPLIED,
        cost::PRICING_TABLE_REFRESHED,
        cost::SESSION_TOTAL_UPDATED,
        cost::PERSISTENCE_FAILED,
    ];
    assert_eq!(names.len(), 10);
    for n in names {
        assert!(n.starts_with("tengu_cost_"));
    }
    // M3-05 plan locks these three byte-for-byte.
    assert_eq!(cost::RECORDED, "tengu_cost_recorded");
    assert_eq!(cost::BUDGET_WARNING, "tengu_cost_budget_warning");
    assert_eq!(cost::BUDGET_EXCEEDED, "tengu_cost_budget_exceeded");
}

#[test]
fn recorded_payload_field_set_matches_m3_05() {
    use lingxi_telemetry::Verified;
    let p = cost::RecordedPayload {
        model: Verified::assert_safe("claude-sonnet-4-5".into()),
        input_tokens: 100,
        output_tokens: 200,
        cache_read_input_tokens: 50,
        cache_creation_input_tokens: 25,
        cost_usd: 1_500_000_000_u64,
        session_id: Verified::assert_safe("sess-uuid".into()),
        is_batch_request: false,
    };
    let json = serde_json::to_string(&p).unwrap();
    // is_batch_request always false in M3.
    assert!(json.contains("\"is_batch_request\":false"));
    // cost_usd field name (the value is nano-USD; key name preserved for M3-05 parity).
    assert!(json.contains("\"cost_usd\":1500000000"));
}
