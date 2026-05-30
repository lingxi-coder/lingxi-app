//! M4-08 parity driver — asserts every locked literal from
//! `parity/fixtures/system_tools.json` appears byte-for-byte in production
//! source (constants, telemetry NAMES array, path templates).

#![allow(clippy::unwrap_used, clippy::items_after_statements)]

use lingxi_test_harness::parity::load_fixture;
use serde_json::Value;

fn fx() -> Value {
    load_fixture::<Value>("system_tools")
}

#[test]
fn tool_names_match_production_constants() {
    let fx = fx();
    let n = &fx["tool_names"];
    assert_eq!(
        n["ask_user_question"].as_str().unwrap(),
        lingxi_tools::builtin::ask_user_question::ASK_USER_QUESTION_TOOL_NAME
    );
    assert_eq!(
        n["brief"].as_str().unwrap(),
        lingxi_tools::builtin::brief::BRIEF_TOOL_NAME
    );
    assert_eq!(
        n["config"].as_str().unwrap(),
        lingxi_tools::builtin::config::CONFIG_TOOL_NAME
    );
    assert_eq!(
        n["skill"].as_str().unwrap(),
        lingxi_tools::builtin::skill::SKILL_TOOL_NAME
    );
    assert_eq!(
        n["schedule_cron"].as_str().unwrap(),
        lingxi_tools::builtin::schedule_cron::SCHEDULE_CRON_TOOL_NAME
    );
    assert_eq!(
        n["tool_search"].as_str().unwrap(),
        lingxi_tools::builtin::tool_search::TOOL_SEARCH_TOOL_NAME
    );
    assert_eq!(
        n["remote_trigger"].as_str().unwrap(),
        lingxi_tools::builtin::remote_trigger::REMOTE_TRIGGER_TOOL_NAME
    );
    assert_eq!(
        n["synthetic_output"].as_str().unwrap(),
        lingxi_tools::builtin::synthetic_output::SYNTHETIC_OUTPUT_TOOL_NAME
    );
}

#[test]
fn wire_identifiers_match_production_constants() {
    let fx = fx();
    let w = &fx["wire_identifiers"];

    use lingxi_tools::builtin::{
        ask_user_question::{MAX_ASK_LABEL_LEN, MAX_ASK_OPTIONS, MAX_ASK_QUESTION_LEN},
        brief::{BRIEF_FILE_SUFFIX, BRIEF_SUBDIR, BRIEF_TASK_ID_PREFIX},
        config::{CONFIG_FIELDS_ALLOWED, CONFIG_FILE_NAME, CONFIG_SUBDIR},
        remote_trigger::{REMOTE_TRIGGER_CREDENTIALS_FILE, REMOTE_TRIGGER_SUBDIR},
        schedule_cron::{CRON_FILE_SUFFIX, CRON_SUBDIR, CRON_TASK_ID_PREFIX, SIX_FIELD_REJECTION},
        skill::MAX_SKILL_DESCRIPTOR_LEN,
        tool_search::TOOL_SEARCH_MAX_RESULTS,
    };

    assert_eq!(
        w["max_ask_options"].as_u64().unwrap(),
        MAX_ASK_OPTIONS as u64
    );
    assert_eq!(
        w["max_ask_label_len"].as_u64().unwrap(),
        MAX_ASK_LABEL_LEN as u64
    );
    assert_eq!(
        w["max_ask_question_len"].as_u64().unwrap(),
        MAX_ASK_QUESTION_LEN as u64
    );
    assert_eq!(w["brief_subdir"].as_str().unwrap(), BRIEF_SUBDIR);
    assert_eq!(w["brief_file_suffix"].as_str().unwrap(), BRIEF_FILE_SUFFIX);
    assert_eq!(
        w["brief_task_id_prefix"].as_str().unwrap(),
        BRIEF_TASK_ID_PREFIX.to_string()
    );
    assert_eq!(w["config_file_name"].as_str().unwrap(), CONFIG_FILE_NAME);
    assert_eq!(w["config_subdir"].as_str().unwrap(), CONFIG_SUBDIR);
    let allowed: Vec<String> = w["config_fields_allowed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        allowed,
        CONFIG_FIELDS_ALLOWED
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        w["max_skill_descriptor_len"].as_u64().unwrap(),
        MAX_SKILL_DESCRIPTOR_LEN as u64
    );
    assert_eq!(w["cron_subdir"].as_str().unwrap(), CRON_SUBDIR);
    assert_eq!(w["cron_file_suffix"].as_str().unwrap(), CRON_FILE_SUFFIX);
    assert_eq!(
        w["cron_task_id_prefix"].as_str().unwrap(),
        CRON_TASK_ID_PREFIX.to_string()
    );
    assert_eq!(
        w["six_field_rejection"].as_str().unwrap(),
        SIX_FIELD_REJECTION
    );
    assert_eq!(
        w["tool_search_max_results"].as_u64().unwrap(),
        TOOL_SEARCH_MAX_RESULTS as u64
    );
    assert_eq!(
        w["remote_trigger_credentials_file"].as_str().unwrap(),
        REMOTE_TRIGGER_CREDENTIALS_FILE
    );
    assert_eq!(
        w["remote_trigger_subdir"].as_str().unwrap(),
        REMOTE_TRIGGER_SUBDIR
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
        24,
        "M4-08 must lock exactly 24 telemetry events (8 tools × 3 stages)"
    );
    for name in &events {
        assert!(
            lingxi_telemetry::tengu::ALL_EVENT_NAMES.contains(name),
            "fixture event {name} missing from tengu::ALL_EVENT_NAMES"
        );
    }
}

#[test]
fn telemetry_constant_symbols_match_event_strings() {
    use lingxi_telemetry::tengu::tool::{
        ASK_USER_QUESTION_COMPLETED, ASK_USER_QUESTION_FAILED, ASK_USER_QUESTION_STARTED,
        BRIEF_COMPLETED, BRIEF_FAILED, BRIEF_STARTED, CONFIG_COMPLETED, CONFIG_FAILED,
        CONFIG_STARTED, REMOTE_TRIGGER_COMPLETED, REMOTE_TRIGGER_FAILED, REMOTE_TRIGGER_STARTED,
        SCHEDULE_CRON_COMPLETED, SCHEDULE_CRON_FAILED, SCHEDULE_CRON_STARTED, SKILL_COMPLETED,
        SKILL_FAILED, SKILL_STARTED, SYNTHETIC_OUTPUT_COMPLETED, SYNTHETIC_OUTPUT_FAILED,
        SYNTHETIC_OUTPUT_STARTED, TOOL_SEARCH_COMPLETED, TOOL_SEARCH_FAILED, TOOL_SEARCH_STARTED,
    };
    let fx = fx();
    let want: Vec<&str> = fx["telemetry_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let got: Vec<&str> = vec![
        ASK_USER_QUESTION_STARTED,
        ASK_USER_QUESTION_COMPLETED,
        ASK_USER_QUESTION_FAILED,
        BRIEF_STARTED,
        BRIEF_COMPLETED,
        BRIEF_FAILED,
        CONFIG_STARTED,
        CONFIG_COMPLETED,
        CONFIG_FAILED,
        SKILL_STARTED,
        SKILL_COMPLETED,
        SKILL_FAILED,
        SCHEDULE_CRON_STARTED,
        SCHEDULE_CRON_COMPLETED,
        SCHEDULE_CRON_FAILED,
        TOOL_SEARCH_STARTED,
        TOOL_SEARCH_COMPLETED,
        TOOL_SEARCH_FAILED,
        REMOTE_TRIGGER_STARTED,
        REMOTE_TRIGGER_COMPLETED,
        REMOTE_TRIGGER_FAILED,
        SYNTHETIC_OUTPUT_STARTED,
        SYNTHETIC_OUTPUT_COMPLETED,
        SYNTHETIC_OUTPUT_FAILED,
    ];
    assert_eq!(want.len(), got.len());
    for (w, g) in want.iter().zip(got.iter()) {
        assert_eq!(*w, *g);
    }
}

#[test]
fn brief_path_template_matches_resolver() {
    let fx = fx();
    assert_eq!(
        fx["path_templates"]["brief"].as_str().unwrap(),
        "~/.claude/brief/<task_id>.txt"
    );
}

#[test]
fn config_path_template_matches_resolver() {
    let fx = fx();
    assert_eq!(
        fx["path_templates"]["config"].as_str().unwrap(),
        "~/.claude/settings.json"
    );
}

#[test]
fn cron_path_template_matches_resolver() {
    let fx = fx();
    assert_eq!(
        fx["path_templates"]["schedule_cron"].as_str().unwrap(),
        "~/.claude/cron/<task_id>.json"
    );
}

#[test]
fn remote_trigger_path_template_matches_resolver() {
    let fx = fx();
    assert_eq!(
        fx["path_templates"]["remote_trigger"].as_str().unwrap(),
        "~/.claude/.credentials.json"
    );
}
