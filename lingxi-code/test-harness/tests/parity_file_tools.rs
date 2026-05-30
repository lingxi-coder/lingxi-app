//! Parity fixture driver: byte-locks every M4-01 numeric/string constant
//! against the values pulled from production source.
//!
//! Spec: `docs/superpowers/specs/2026-05-24-m4-tools-implementation-design.md`
//! §6 (test target — `parity_file_tools.json` driver) + §7 (wire identifiers).

#![allow(clippy::too_many_lines, clippy::items_after_statements)]

use serde::Deserialize;
use test_harness::parity::load_fixture;

#[derive(Deserialize)]
struct SizeLimitRow {
    value: u64,
    error_template: String,
}

#[derive(Deserialize)]
struct BinaryRow {
    scan_window_bytes: usize,
    error_template: String,
}

#[derive(Deserialize)]
struct StringRow {
    template: String,
}

#[derive(Deserialize)]
struct ValueRow<T> {
    value: T,
}

#[derive(Deserialize)]
struct OutputTruncationRow {
    max_length: usize,
    suffix: String,
}

#[derive(Deserialize)]
struct PathBlockedEventRow {
    value: String,
}

#[derive(Deserialize)]
struct TelemetryEventsRow {
    read: Vec<String>,
    write: Vec<String>,
    edit: Vec<String>,
    notebook: Vec<String>,
    glob: Vec<String>,
    grep: Vec<String>,
}

#[derive(Deserialize)]
struct Fixture {
    #[serde(rename = "_source")]
    source: String,
    #[serde(rename = "_note")]
    note: String,
    tools: Vec<String>,
    read_size_limit: SizeLimitRow,
    binary_detection: BinaryRow,
    path_blocked_error: StringRow,
    path_blocked_event_name: PathBlockedEventRow,
    patch_truncation_suffix: StringRow,
    output_truncation: OutputTruncationRow,
    glob_cap: ValueRow<usize>,
    grep_per_file_cap: ValueRow<usize>,
    line_indexing_base: ValueRow<u32>,
    telemetry_events: TelemetryEventsRow,
}

#[test]
fn file_tools_fixture_matches_production_constants() {
    let fx: Fixture = load_fixture("file_tools");

    assert!(!fx.source.is_empty(), "_source missing");
    assert!(!fx.note.is_empty(), "_note missing");

    assert_eq!(
        fx.tools,
        vec!["Read", "Write", "Edit", "NotebookEdit", "Glob", "Grep"]
    );

    assert_eq!(
        fx.read_size_limit.value,
        tool_file::read::MAX_FILE_READ_SIZE
    );
    assert_eq!(
        fx.read_size_limit.error_template,
        "File {path} ({size}B) exceeds 256KB read limit"
    );

    assert_eq!(
        fx.binary_detection.scan_window_bytes,
        tool_file::shared::NUL_SCAN_WINDOW
    );
    assert_eq!(
        fx.binary_detection.error_template,
        "File {path} appears to be binary (first 8KB contains NUL bytes)"
    );

    assert_eq!(
        fx.path_blocked_error.template,
        "File path {path} is outside trusted directories"
    );
    assert_eq!(
        fx.path_blocked_event_name.value,
        tool_api::util::path_validation::PATH_BLOCKED_EVENT
    );
    assert_eq!(fx.path_blocked_event_name.value, "tengu_file_path_blocked");

    assert_eq!(
        fx.patch_truncation_suffix.template,
        tool_file::edit::PATCH_TRUNCATION_SUFFIX_TEMPLATE
    );
    assert_eq!(
        fx.patch_truncation_suffix.template,
        "\n\n... [{N} lines truncated] ..."
    );

    assert_eq!(
        fx.output_truncation.max_length,
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    );
    assert_eq!(fx.output_truncation.max_length, 30_000);
    assert_eq!(
        fx.output_truncation.suffix,
        tool_api::util::output_truncation::TRUNCATION_SUFFIX
    );
    assert_eq!(
        fx.output_truncation.suffix,
        "\n\n[Output truncated due to length]"
    );

    assert_eq!(fx.glob_cap.value, tool_file::glob::MAX_GLOB_MATCHES);
    assert_eq!(fx.glob_cap.value, 100);

    assert_eq!(
        fx.grep_per_file_cap.value,
        tool_file::grep::GREP_PER_FILE_CAP
    );
    assert_eq!(fx.grep_per_file_cap.value, 100);

    assert_eq!(fx.line_indexing_base.value, 1);

    use telemetry::tengu::tool as ev;
    assert_eq!(
        fx.telemetry_events.read,
        vec![
            ev::READ_STARTED.to_string(),
            ev::READ_COMPLETED.to_string(),
            ev::READ_FAILED.to_string()
        ]
    );
    assert_eq!(
        fx.telemetry_events.write,
        vec![
            ev::WRITE_STARTED.to_string(),
            ev::WRITE_COMPLETED.to_string(),
            ev::WRITE_FAILED.to_string()
        ]
    );
    assert_eq!(
        fx.telemetry_events.edit,
        vec![
            ev::EDIT_STARTED.to_string(),
            ev::EDIT_COMPLETED.to_string(),
            ev::EDIT_FAILED.to_string()
        ]
    );
    assert_eq!(
        fx.telemetry_events.notebook,
        vec![
            ev::NOTEBOOK_STARTED.to_string(),
            ev::NOTEBOOK_COMPLETED.to_string(),
            ev::NOTEBOOK_FAILED.to_string()
        ]
    );
    assert_eq!(
        fx.telemetry_events.glob,
        vec![
            ev::GLOB_STARTED.to_string(),
            ev::GLOB_COMPLETED.to_string(),
            ev::GLOB_FAILED.to_string()
        ]
    );
    assert_eq!(
        fx.telemetry_events.grep,
        vec![
            ev::GREP_STARTED.to_string(),
            ev::GREP_COMPLETED.to_string(),
            ev::GREP_FAILED.to_string()
        ]
    );
}
