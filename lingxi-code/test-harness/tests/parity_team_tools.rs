//! M4-06 parity driver — asserts every locked literal from
//! `parity/fixtures/team_tools.json` appears byte-for-byte in production
//! source (constants, telemetry NAMES array, M3-02 `TEAM_MEM_SUBDIR`).

#![allow(clippy::unwrap_used)]

use serde_json::Value;
use test_harness::parity::load_fixture;

fn fx() -> Value {
    load_fixture::<Value>("team_tools")
}

#[test]
fn team_tool_names_match_production_constants() {
    let fx = fx();
    assert_eq!(
        fx["tool_names"]["team_create"].as_str().unwrap(),
        tool_team::team::TEAM_CREATE_TOOL_NAME
    );
    assert_eq!(
        fx["tool_names"]["team_delete"].as_str().unwrap(),
        tool_team::team::TEAM_DELETE_TOOL_NAME
    );
}

#[test]
fn wire_identifiers_match_production_constants() {
    let fx = fx();
    let w = &fx["wire_identifiers"];

    // M3-02 lock: lingxi-tools mirrors TEAM_MEM_SUBDIR locally to avoid the
    // dep cycle with lingxi-memory (see fixture _note (f)). Assert both the
    // local mirror AND the upstream M3-02 symbol equal the fixture literal.
    assert_eq!(
        w["team_mem_subdir"].as_str().unwrap(),
        tool_team::team::TEAM_MEM_SUBDIR
    );
    assert_eq!(
        w["team_mem_subdir"].as_str().unwrap(),
        memory::memdir::paths::TEAM_MEM_SUBDIR
    );
    // And the two mirrors must agree with each other.
    assert_eq!(
        tool_team::team::TEAM_MEM_SUBDIR,
        memory::memdir::paths::TEAM_MEM_SUBDIR
    );

    assert_eq!(
        w["team_config_filename"].as_str().unwrap(),
        tool_team::team::TEAM_CONFIG_FILENAME
    );
    assert_eq!(
        w["default_team_name"].as_str().unwrap(),
        tool_team::team::DEFAULT_TEAM_NAME
    );
    assert_eq!(
        w["max_team_name_len"].as_u64().unwrap(),
        tool_team::team::MAX_TEAM_NAME_LEN as u64
    );
    assert_eq!(
        w["team_name_pattern_desc"].as_str().unwrap(),
        tool_team::team::TEAM_NAME_PATTERN_DESC
    );
}

#[test]
fn telemetry_events_present_in_tengu_tool_names_array() {
    let fx = fx();
    let events: Vec<&str> = fx["telemetry_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        events.len(),
        6,
        "M4-06 must lock exactly 6 telemetry events"
    );
    for name in &events {
        assert!(
            telemetry::tengu::ALL_EVENT_NAMES.contains(name),
            "fixture event {name} missing from tengu::ALL_EVENT_NAMES \
             (which sources tengu::tool::NAMES)"
        );
    }
}

#[test]
fn telemetry_constant_symbols_match_event_strings() {
    use telemetry::tengu::tool::{
        TEAM_CREATE_COMPLETED, TEAM_CREATE_FAILED, TEAM_CREATE_STARTED, TEAM_DELETE_COMPLETED,
        TEAM_DELETE_FAILED, TEAM_DELETE_STARTED,
    };
    let fx = fx();
    let want = fx["telemetry_events"].as_array().unwrap();
    assert_eq!(want[0].as_str().unwrap(), TEAM_CREATE_STARTED);
    assert_eq!(want[1].as_str().unwrap(), TEAM_CREATE_COMPLETED);
    assert_eq!(want[2].as_str().unwrap(), TEAM_CREATE_FAILED);
    assert_eq!(want[3].as_str().unwrap(), TEAM_DELETE_STARTED);
    assert_eq!(want[4].as_str().unwrap(), TEAM_DELETE_COMPLETED);
    assert_eq!(want[5].as_str().unwrap(), TEAM_DELETE_FAILED);
}

#[test]
fn team_dir_template_matches_resolve_team_dir() {
    let fx = fx();
    let tmpl = fx["team_dir_template"].as_str().unwrap();
    assert_eq!(tmpl, "~/.lingxi/team-mem/<team_name>/");
    let home = std::path::PathBuf::from("/tmp/parity-home");
    let dir = tool_team::team::resolve_team_dir(&home, "example");
    assert_eq!(
        dir,
        std::path::PathBuf::from("/tmp/parity-home/.lingxi/team-mem/example")
    );
}

#[test]
fn error_string_format_skeletons_match_production() {
    let fx = fx();
    let e = &fx["error_string_formats"];
    assert_eq!(
        e["empty_team_name"].as_str().unwrap(),
        "Team: team_name is empty"
    );
    assert_eq!(
        e["invalid_chars"].as_str().unwrap(),
        "Team: team_name '{team_name}' contains invalid characters (allowed: [a-zA-Z0-9_-]+)"
    );
    assert_eq!(
        e["delete_non_empty_no_force"].as_str().unwrap(),
        "TeamDelete: team '{team_name}' directory is non-empty ({file_count} files); pass force=true to delete anyway"
    );
    assert_eq!(
        e["create_already_exists"].as_str().unwrap(),
        "TeamCreate: team '{team_name}' already exists at {team_dir}"
    );
    assert_eq!(
        e["delete_not_found"].as_str().unwrap(),
        "TeamDelete: team '{team_name}' does not exist at {team_dir}"
    );
    assert_eq!(
        e["missing_team_name_field_create"].as_str().unwrap(),
        "TeamCreate: missing or non-string team_name"
    );
    assert_eq!(
        e["missing_team_name_field_delete"].as_str().unwrap(),
        "TeamDelete: missing or non-string team_name"
    );
}

#[test]
fn constants_lock_block_mirrors_production() {
    let fx = fx();
    let c = &fx["constants_lock"];
    assert_eq!(
        c["TEAM_CREATE_TOOL_NAME"].as_str().unwrap(),
        tool_team::team::TEAM_CREATE_TOOL_NAME
    );
    assert_eq!(
        c["TEAM_DELETE_TOOL_NAME"].as_str().unwrap(),
        tool_team::team::TEAM_DELETE_TOOL_NAME
    );
    assert_eq!(
        c["DEFAULT_TEAM_NAME"].as_str().unwrap(),
        tool_team::team::DEFAULT_TEAM_NAME
    );
    assert_eq!(
        c["TEAM_CONFIG_FILENAME"].as_str().unwrap(),
        tool_team::team::TEAM_CONFIG_FILENAME
    );
    assert_eq!(
        c["MAX_TEAM_NAME_LEN"].as_u64().unwrap(),
        tool_team::team::MAX_TEAM_NAME_LEN as u64
    );
    assert_eq!(
        c["TEAM_NAME_PATTERN_DESC"].as_str().unwrap(),
        tool_team::team::TEAM_NAME_PATTERN_DESC
    );
    assert_eq!(
        c["M3_02_TEAM_MEM_SUBDIR"].as_str().unwrap(),
        memory::memdir::paths::TEAM_MEM_SUBDIR
    );
}
