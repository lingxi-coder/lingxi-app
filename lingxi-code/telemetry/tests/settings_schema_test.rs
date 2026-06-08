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
fn registry_is_exactly_330_entries() {
    assert_eq!(
        telemetry::tengu::ALL_EVENT_NAMES.len(),
        339,
        "M3-06 baseline 143 + M4-02 9 + M4-03 3 + M4-04 15 + M4-05 24 + M4-06 6 + M4-07 13 + M4-08 24 + M4-09 1 + M5-02 3 + M5-03 0 + M5-04 2 + M5-05 2 + M5-06 8 hooks + M5-07 3 jsonl + M5-08 2 resume + M5-10 18 commands + M5-11 36 commands + M5-13 2 repl + M5-14 1 v0.6.0 release + M6-01 4 tui + M6-03 2 tui streaming + M6-05 2 tui permission dialog + M6-09 1 v0.7.0 release + M6-09 2 tui scroll = 326 (M6-09 dropped tengu_tui_key_pressed -> M7) + M7-01..M7-15 0 + M7-16 1 v0.8.0 release + M7-16 3 tui (screen_opened/screen_closed/search_opened) = 330 (M7-16 deferred command_palette_opened/vim_mode_entered/key_pressed -> M8) + CronDelete/CronList 6 = 336 + FileReadTool analytics 3 (file_read_dedup/session_file_read/file_read_limits_override) = 339",
    );
}
