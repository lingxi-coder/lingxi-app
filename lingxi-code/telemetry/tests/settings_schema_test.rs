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
    // Canonical count is pinned by event_name_completeness_test (341). Grep/Glob
    // emit NO telemetry (claude-code v2.1.183 emits no tengu_tool_grep_* /
    // tengu_tool_glob_* events): 6 fabricated tool names removed (353 → 347).
    // Strict-parity (2.1.195) then removed D1 tengu_tool_todo_write_* (3), D2
    // port-only tengu_cost_recorded (1), and D3 session-resume consolidation (2):
    // 347 - 6 = 341.
    assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 341);
}
