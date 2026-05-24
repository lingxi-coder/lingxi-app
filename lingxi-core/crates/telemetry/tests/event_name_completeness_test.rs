//! `ALL_EVENT_NAMES` matches the parity fixture byte-for-byte (positive case
//! is in test-harness's `parity_tengu_events.rs`; this test guards the count
//! and uniqueness without crossing the workspace boundary).

use lingxi_telemetry::tengu::ALL_EVENT_NAMES;

#[test]
fn registry_is_exactly_213_entries() {
    // M4-05 added 24 events (8 agent/task tools × 3 lifecycle stages),
    // M4-06 added 6 (2 team tools × 3 lifecycle stages),
    // M4-07 added 13 (1 MCP_STARTED + 4 new tools × 3 lifecycle stages):
    // 170 (post-M4-04) + 24 (M4-05) + 6 (M4-06) + 13 (M4-07) = 213.
    assert_eq!(ALL_EVENT_NAMES.len(), 213);
}

#[test]
fn registry_entries_are_unique() {
    use std::collections::HashSet;
    let set: HashSet<&&str> = ALL_EVENT_NAMES.iter().collect();
    assert_eq!(
        set.len(),
        ALL_EVENT_NAMES.len(),
        "duplicate name in ALL_EVENT_NAMES"
    );
}

#[test]
fn registry_entries_all_use_tengu_prefix() {
    for n in ALL_EVENT_NAMES {
        assert!(n.starts_with("tengu_"), "{n} must start with `tengu_`");
    }
}

#[test]
fn category_ordering_preserved() {
    // First 25 are api_; next 30 are agent_; etc.
    for n in &ALL_EVENT_NAMES[0..25] {
        assert!(n.starts_with("tengu_api_"), "api block: {n}");
    }
    for n in &ALL_EVENT_NAMES[25..55] {
        assert!(n.starts_with("tengu_agent_"), "agent block: {n}");
    }
    for n in &ALL_EVENT_NAMES[55..70] {
        assert!(n.starts_with("tengu_session_"), "session block: {n}");
    }
    // M4-05 grew the tool block by +24 (67 → 91); M4-06 grew it by +6 (91 → 97);
    // M4-07 grew it by +13 (97 → 110), shifting downstream offsets by +13.
    for n in &ALL_EVENT_NAMES[70..180] {
        assert!(n.starts_with("tengu_tool_"), "tool block: {n}");
    }
    for n in &ALL_EVENT_NAMES[180..190] {
        assert!(n.starts_with("tengu_cost_"), "cost block: {n}");
    }
    for n in &ALL_EVENT_NAMES[190..198] {
        assert!(n.starts_with("tengu_oauth_"), "oauth block: {n}");
    }
    for n in &ALL_EVENT_NAMES[198..210] {
        assert!(n.starts_with("tengu_memory_"), "memory block: {n}");
    }
    for n in &ALL_EVENT_NAMES[210..213] {
        assert!(n.starts_with("tengu_settings_"), "settings block: {n}");
    }
}
