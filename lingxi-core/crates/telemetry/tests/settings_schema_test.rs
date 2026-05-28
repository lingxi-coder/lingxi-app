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
fn registry_is_exactly_314_entries() {
    assert_eq!(
        lingxi_telemetry::tengu::ALL_EVENT_NAMES.len(),
        314,
        "M3-06 baseline 143 + M4-02 9 + M4-03 3 + M4-04 15 + M4-05 24 + M4-06 6 + M4-07 13 + M4-08 24 + M4-09 1 + M5-02 3 + M5-03 0 + M5-04 2 + M5-05 2 + M5-06 8 hooks + M5-07 3 jsonl + M5-08 2 resume + M5-10 18 commands + M5-11 36 commands + M5-13 2 repl = 314",
    );
}
