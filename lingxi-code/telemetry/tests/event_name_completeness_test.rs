//! `ALL_EVENT_NAMES` matches the parity fixture byte-for-byte (positive case
//! is in test-harness's `parity_tengu_events.rs`; this test guards the count
//! and uniqueness without crossing the workspace boundary).

use telemetry::tengu::ALL_EVENT_NAMES;

#[test]
fn registry_is_exactly_330_entries() {
    // M4-05 added 24 events (8 agent/task tools × 3 lifecycle stages),
    // M4-06 added 6 (2 team tools × 3 lifecycle stages),
    // M4-07 added 13 (1 MCP_STARTED + 4 new tools × 3 lifecycle stages),
    // M4-08 added 24 (8 system tools × 3 lifecycle stages),
    // M4-09 added 1 (release marker `lingxi_core_v0_5_0_released`),
    // M5-02 added 3 (orchestrator conversation lifecycle: started/completed/failed),
    // M5-03 added 0,
    // M5-04 added 2 streaming events (turn_streaming_started/completed),
    // M5-05 added 2 permission events (permission_prompted/answered),
    // M5-06 added 8 hook events (pre/post started/completed/failed,
    //   http_skipped_ssrf, timeout),
    // M5-07 added 3 session-jsonl events (appended/rotated/corrupted),
    // M5-08 added 2 session-resume events (resume_started/completed),
    // M5-10 added 18 command events (6 batch-1 commands × 3 phases),
    // M5-11 added 36 command events (12 batch-2 commands × 3 phases):
    // 276 (post-M5-10) + 36 (M5-11) = 312.
    // M5-13 added 2 REPL session lifecycle events (repl_session_started/ended):
    // 312 + 2 = 314.
    // M5-14 added 1 release marker `lingxi_core_v0_6_0_released`:
    // 314 + 1 = 315.
    // M6-01 added 4 TUI lifecycle events (session_started/ended,
    // first_render, resize): 315 + 4 = 319.
    // M6-03 added 2 TUI streaming render events (streaming_render_started/ended):
    // 319 + 2 = 321.
    // M6-05 added 2 TUI permission-dialog events
    // (permission_dialog_shown/resolved): 321 + 2 = 323.
    // M6-06/07/08 added 0 (engine wiring reused existing M3/M4/M5 events).
    // M6-09 added 3: lingxi_core_v0_7_0_released (release marker) +
    // tengu_tui_scroll_started/scroll_ended (real emit sites in
    // app::scroll_with_viewport). tengu_tui_key_pressed deferred to M7
    // (no aggregator infra). 323 + 3 = 326.
    // M7-01..M7-15 added 0 (every TUI event candidate was deferred to the
    // M7-16 audit; the renderer/screen/vim/palette sub-plans registered none).
    // M7-16 added 4: lingxi_core_v0_8_0_released (release marker) +
    // tengu_tui_screen_opened/screen_closed/search_opened (real emit sites in
    // AppState::open_*/close_screen + MessageSelectorState::open/open_export).
    // tengu_tui_command_palette_opened/vim_mode_entered/key_pressed stay
    // deferred to M8 (no clean/aggregated emit site). 326 + 4 = 330.
    assert_eq!(ALL_EVENT_NAMES.len(), 330);
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
fn lingxi_core_v0_6_0_released_is_registered() {
    assert!(
        ALL_EVENT_NAMES.contains(&"lingxi_core_v0_6_0_released"),
        "v0.6.0 release-marker event must be present in ALL_EVENT_NAMES"
    );
}

#[test]
fn lingxi_core_v0_7_0_released_is_registered() {
    assert!(
        ALL_EVENT_NAMES.contains(&"lingxi_core_v0_7_0_released"),
        "v0.7.0 release-marker event must be present in ALL_EVENT_NAMES"
    );
}

#[test]
fn lingxi_core_v0_8_0_released_is_registered() {
    assert!(
        ALL_EVENT_NAMES.contains(&"lingxi_core_v0_8_0_released"),
        "v0.8.0 release-marker event must be present in ALL_EVENT_NAMES"
    );
}

#[test]
fn m7_16_screen_opened_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_screen_opened"));
}

#[test]
fn m7_16_screen_closed_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_screen_closed"));
}

#[test]
fn m7_16_search_opened_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_search_opened"));
}

/// The three §2.7 candidates that stayed deferred (no clean/aggregated emit
/// site) MUST NOT be registered — registering a name with no call site mints a
/// dead name (the M6 "330-vs-326" lesson).
#[test]
fn m7_16_deferred_candidates_not_registered() {
    assert!(!ALL_EVENT_NAMES.contains(&"tengu_tui_command_palette_opened"));
    assert!(!ALL_EVENT_NAMES.contains(&"tengu_tui_vim_mode_entered"));
    assert!(!ALL_EVENT_NAMES.contains(&"tengu_tui_key_pressed"));
}

#[test]
fn release_marker_constant_matches() {
    assert_eq!(
        telemetry::tengu::release::LINGXI_CORE_V0_5_0_RELEASED,
        "lingxi_core_v0_5_0_released"
    );
    assert_eq!(
        telemetry::tengu::release::LINGXI_CORE_V0_6_0_RELEASED,
        "lingxi_core_v0_6_0_released"
    );
    assert_eq!(
        telemetry::tengu::release::LINGXI_CORE_V0_7_0_RELEASED,
        "lingxi_core_v0_7_0_released"
    );
    assert_eq!(
        telemetry::tengu::release::LINGXI_CORE_V0_8_0_RELEASED,
        "lingxi_core_v0_8_0_released"
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
    // M5-07 grew the session block by +3 (appended/rotated/corrupted): 15 -> 18.
    // M5-08 grew it by +2 (resume_started/completed): 18 -> 20.
    for n in &ALL_EVENT_NAMES[55..75] {
        assert!(n.starts_with("tengu_session_"), "session block: {n}");
    }
    // M4-05 grew the tool block by +24 (67 → 91); M4-06 grew it by +6 (91 → 97);
    // M4-07 grew it by +13 (97 → 110); M4-08 grew it by +24 (110 → 134),
    // shifting downstream offsets by +24.
    // M5-07 shifts all post-session offsets by +3.
    // M5-08 shifts all post-session offsets by another +2.
    for n in &ALL_EVENT_NAMES[75..209] {
        assert!(n.starts_with("tengu_tool_"), "tool block: {n}");
    }
    for n in &ALL_EVENT_NAMES[209..219] {
        assert!(n.starts_with("tengu_cost_"), "cost block: {n}");
    }
    for n in &ALL_EVENT_NAMES[219..227] {
        assert!(n.starts_with("tengu_oauth_"), "oauth block: {n}");
    }
    for n in &ALL_EVENT_NAMES[227..239] {
        assert!(n.starts_with("tengu_memory_"), "memory block: {n}");
    }
    for n in &ALL_EVENT_NAMES[239..242] {
        assert!(n.starts_with("tengu_settings_"), "settings block: {n}");
    }
    // M5-02 grew the orchestrator block by +3 (conversation lifecycle).
    // M5-04 grew it by +2 (streaming). M5-05 grew it by +2 (permission).
    // M5-06 grew it by +8 (hook pre/post + http_skipped_ssrf + timeout).
    // M5-13 grew it by +2 (REPL session started/ended).
    // Block size is now 17; release marker still trails. Walk order is
    // fixed by tengu::mod.rs's concat_all (settings → orchestrator →
    // release).
    for n in &ALL_EVENT_NAMES[242..259] {
        assert!(
            n.starts_with("tengu_orchestrator_") || n.starts_with("tengu_repl_"),
            "orchestrator block: {n}"
        );
    }
    // M5-14 grew the release block from 1 to 2 (+lingxi_core_v0_6_0_released).
    // M6-09 grew it from 2 to 3 (+lingxi_core_v0_7_0_released).
    // M7-16 grew it from 3 to 4 (+lingxi_core_v0_8_0_released): 259..263.
    for n in &ALL_EVENT_NAMES[259..263] {
        assert!(n.starts_with("lingxi_core_"), "release block: {n}");
    }
    // M5-10/M5-11: command block (54 events: 18 batch-1 + 36 batch-2) follows
    // the release markers. Walk order (per tengu::mod.rs concat_all):
    // … → release → command. M6-09 shifted the start by +1 (release 2→3);
    // M7-16 shifts it another +1 (release 3→4): 263..317.
    for n in &ALL_EVENT_NAMES[263..317] {
        assert!(n.starts_with("tengu_command_"), "command block: {n}");
    }
    // M6-01: tui block (4 events) trails command.
    // M6-03: tui block grows to 6 events (+streaming_render_{started,ended}).
    // M6-05: tui block grows to 8 events (+permission_dialog_{shown,resolved}).
    // M6-09: tui block grows to 10 events (+scroll_started/scroll_ended).
    // M7-16: tui block grows to 13 events
    //        (+screen_opened/screen_closed/search_opened). Block shifted by +2
    //        total vs M6-09 (release 2→3→4): 317..330.
    for n in &ALL_EVENT_NAMES[317..330] {
        assert!(n.starts_with("tengu_tui_"), "tui block: {n}");
    }
}

#[test]
fn m6_03_streaming_render_events_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_streaming_render_started"));
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_streaming_render_ended"));
}

#[test]
fn m6_05_permission_dialog_events_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_permission_dialog_shown"));
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_permission_dialog_resolved"));
}

#[test]
fn m6_09_scroll_events_registered() {
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_scroll_started"));
    assert!(ALL_EVENT_NAMES.contains(&"tengu_tui_scroll_ended"));
}
