//! `ALL_EVENT_NAMES` matches the parity fixture byte-for-byte (positive case
//! is in test-harness's `parity_tengu_events.rs`; this test guards the count
//! and uniqueness without crossing the workspace boundary).

use lingxi_telemetry::tengu::ALL_EVENT_NAMES;

#[test]
fn registry_is_exactly_241_entries() {
    // M4-05 added 24 events (8 agent/task tools × 3 lifecycle stages),
    // M4-06 added 6 (2 team tools × 3 lifecycle stages),
    // M4-07 added 13 (1 MCP_STARTED + 4 new tools × 3 lifecycle stages),
    // M4-08 added 24 (8 system tools × 3 lifecycle stages),
    // M4-09 added 1 (release marker `lingxi_core_v0_5_0_released`),
    // M5-02 added 3 (orchestrator conversation lifecycle: started/completed/failed):
    // 213 (post-M4-07) + 24 (M4-08) + 1 (M4-09) + 3 (M5-02) = 241.
    assert_eq!(ALL_EVENT_NAMES.len(), 241);
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
fn registry_entries_all_use_tengu_or_release_prefix() {
    // All events use the `tengu_` prefix EXCEPT for release-marker events,
    // which use the engine-scoped `lingxi_core_` prefix (the release marker
    // is a one-shot init event, not a subsystem event).
    for n in ALL_EVENT_NAMES {
        assert!(
            n.starts_with("tengu_") || n.starts_with("lingxi_core_"),
            "{n} must start with `tengu_` or `lingxi_core_`"
        );
    }
}

#[test]
fn lingxi_core_v0_5_0_released_is_registered() {
    assert!(
        ALL_EVENT_NAMES.contains(&"lingxi_core_v0_5_0_released"),
        "v0.5.0 release-marker event must be present in ALL_EVENT_NAMES"
    );
}

#[test]
fn release_marker_constant_matches() {
    assert_eq!(
        lingxi_telemetry::tengu::release::LINGXI_CORE_V0_5_0_RELEASED,
        "lingxi_core_v0_5_0_released"
    );
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
    // M5-02 grew the list by +3 (orchestrator conversation lifecycle),
    // inserted BEFORE the release marker per concat_all walk order.
    for n in &ALL_EVENT_NAMES[237..240] {
        assert!(
            n.starts_with("tengu_orchestrator_"),
            "orchestrator block: {n}"
        );
    }
    for n in &ALL_EVENT_NAMES[240..241] {
        assert!(n.starts_with("lingxi_core_"), "release block: {n}");
    }
}
