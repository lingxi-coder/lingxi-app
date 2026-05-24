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
fn registry_has_exactly_170_entries_after_m4_04() {
    assert_eq!(
        lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(),
        170,
        "M3-06 baseline 143 + M4-02 9 powershell/repl/sleep events + M4-03 3 web_search events + M4-04 15 workflow events = 170",
    );
}
