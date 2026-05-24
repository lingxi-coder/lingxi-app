//! `ALL_EVENT_NAMES` matches the parity fixture byte-for-byte (positive case
//! is in test-harness's `parity_tengu_events.rs`; this test guards the count
//! and uniqueness without crossing the workspace boundary).

use lingxi_telemetry::tengu::ALL_EVENT_NAMES;

#[test]
fn registry_is_exactly_143_entries() {
    assert_eq!(ALL_EVENT_NAMES.len(), 143);
}

#[test]
fn registry_entries_are_unique() {
    use std::collections::HashSet;
    let set: HashSet<&&str> = ALL_EVENT_NAMES.iter().collect();
    assert_eq!(set.len(), ALL_EVENT_NAMES.len(), "duplicate name in ALL_EVENT_NAMES");
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
    for n in &ALL_EVENT_NAMES[70..110] {
        assert!(n.starts_with("tengu_tool_"), "tool block: {n}");
    }
    for n in &ALL_EVENT_NAMES[110..120] {
        assert!(n.starts_with("tengu_cost_"), "cost block: {n}");
    }
    for n in &ALL_EVENT_NAMES[120..128] {
        assert!(n.starts_with("tengu_oauth_"), "oauth block: {n}");
    }
    for n in &ALL_EVENT_NAMES[128..140] {
        assert!(n.starts_with("tengu_memory_"), "memory block: {n}");
    }
    for n in &ALL_EVENT_NAMES[140..143] {
        assert!(n.starts_with("tengu_settings_"), "settings block: {n}");
    }
}
