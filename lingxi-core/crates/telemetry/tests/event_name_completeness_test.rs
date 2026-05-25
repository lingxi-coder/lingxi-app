//! `ALL_EVENT_NAMES` matches the parity fixture byte-for-byte (positive case
//! is in test-harness's `parity_tengu_events.rs`; this test guards the count
//! and uniqueness without crossing the workspace boundary).

use lingxi_telemetry::tengu::ALL_EVENT_NAMES;

#[test]
fn registry_is_exactly_237_entries() {
    // M4-05 added 24 events (8 agent/task tools × 3 lifecycle stages),
    // M4-06 added 6 (2 team tools × 3 lifecycle stages),
    // M4-07 added 13 (1 MCP_STARTED + 4 new tools × 3 lifecycle stages),
    // M4-08 added 24 (8 system tools × 3 lifecycle stages):
    // 213 (post-M4-07) + 24 (M4-08) = 237.
    assert_eq!(ALL_EVENT_NAMES.len(), 237);
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
    // M4-07 grew it by +13 (97 → 110); M4-08 grew it by +24 (110 → 134),
    // shifting downstream offsets by +24.
    for n in &ALL_EVENT_NAMES[70..204] {
        assert!(n.starts_with("tengu_tool_"), "tool block: {n}");
    }
    for n in &ALL_EVENT_NAMES[204..214] {
        assert!(n.starts_with("tengu_cost_"), "cost block: {n}");
    }
    for n in &ALL_EVENT_NAMES[214..222] {
        assert!(n.starts_with("tengu_oauth_"), "oauth block: {n}");
    }
    for n in &ALL_EVENT_NAMES[222..234] {
        assert!(n.starts_with("tengu_memory_"), "memory block: {n}");
    }
    for n in &ALL_EVENT_NAMES[234..237] {
        assert!(n.starts_with("tengu_settings_"), "settings block: {n}");
    }
}
