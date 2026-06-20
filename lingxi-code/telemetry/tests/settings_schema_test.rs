use telemetry::tengu::settings;

#[test]
fn all_3_settings_event_names_are_locked() {
    let names: &[&str] = &[
        settings::LOADED,
        settings::INVALID_ENV,
        settings::PARSE_ERROR,
    ];
    assert_eq!(names.len(), 3);
    for n in names {
        assert!(n.starts_with("tengu_settings_"));
    }
    // M3-01 plan locks these byte-for-byte.
    assert_eq!(settings::LOADED, "tengu_settings_loaded");
    assert_eq!(settings::INVALID_ENV, "tengu_settings_invalid_env");
    assert_eq!(settings::PARSE_ERROR, "tengu_settings_parse_error");
}

#[test]
fn registry_is_exactly_347_entries() {
    // Canonical count is pinned by event_name_completeness_test (347) and the
    // byte-for-byte fixture parity_tengu_events (347). Grep/Glob emit NO
    // telemetry (claude-code v2.1.183 emits no tengu_tool_grep_* /
    // tengu_tool_glob_* events), so 6 fabricated tool names were removed,
    // shifting the registry from 353 → 347. (FileReadTool analytics is 4 names,
    // not the 3 this comment's older arithmetic assumed.)
    assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 347);
}
