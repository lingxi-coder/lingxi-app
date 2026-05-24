use lingxi_telemetry::tengu::settings;

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
fn registry_has_exactly_200_entries_after_m4_06() {
    assert_eq!(
        lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(),
        200,
        "M3-06 baseline 143 + M4-02 9 powershell/repl/sleep + M4-03 3 web_search + M4-04 15 workflow + M4-05 24 agent/task + M4-06 6 team = 200",
    );
}
