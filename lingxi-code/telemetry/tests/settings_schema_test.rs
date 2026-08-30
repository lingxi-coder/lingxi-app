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
fn registry_is_exactly_364_entries() {
    // Canonical count is pinned by event_name_completeness_test (364). Grep/Glob
    // emit NO telemetry (claude-code v2.1.183 emits no tengu_tool_grep_* /
    // tengu_tool_glob_* events): 6 fabricated tool names removed (353 → 347).
    // Strict-parity (2.1.195) then removed D1 tengu_tool_todo_write_* (3), D2
    // port-only tengu_cost_recorded (1), and D3 session-resume consolidation (2):
    // 347 - 6 = 341. 2.1.198 M2 added the AWS auth-refresh trust-gate tail
    // block (tengu_awsAuthRefresh_missing_trust,
    // tengu_awsCredentialExport_missing_trust): 341 + 2 = 343. Worktree 2.1.206
    // parity added 2 byte-exact tool-block events (tengu_worktree_created,
    // tengu_worktree_entered_existing): 343 + 2 = 345. ExitWorktree 2.1.206
    // parity added 2 more byte-exact tool-block events (tengu_worktree_kept,
    // tengu_worktree_removed): 345 + 2 = 347. 2.1.251 byte-alignment B8
    // (telemetry-modules) added the mcp (+2) and plugin (+14) global-tail
    // blocks: 347 + 2 + 14 = 363. §20a/§20b wiring added tengu_mcp_degraded
    // to the mcp block (+1): 363 + 1 = 364. Re-counted by hand, not pasted
    // from a failing assertion.
    assert_eq!(telemetry::tengu::ALL_EVENT_NAMES.len(), 364);
}
