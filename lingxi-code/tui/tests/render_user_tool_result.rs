//! Snapshot tests for `UserToolResultMessage` (M6-04 Tasks 5 + 6).

use insta::assert_snapshot;
use iocraft::prelude::*;
use protocol::ToolUseId;
use tui::components::messages::user_tool_result::{
    render_user_tool_result_to_string, UserToolResultMessage, UserToolResultProps,
};

fn id() -> ToolUseId {
    ToolUseId::nil()
}

#[test]
fn short_5_line_result_renders_full() {
    let body = "line1\nline2\nline3\nline4\nline5";
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Read".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    assert_snapshot!(s, @r"
    └ line1
      line2
      line3
      line4
      line5
    ");
}

#[test]
fn long_200_line_result_shows_truncation_footer() {
    let body = (1..=200)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Bash".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    // First 100 lines render; lines 101..=200 are dropped.
    assert!(s.starts_with("└ line1\n"), "got: {s}");
    assert!(s.contains("\n  line100\n"), "got tail: {s}");
    assert!(!s.contains("line101"), "should not include line101");
    assert!(
        s.ends_with("[output truncated, 100 more lines]"),
        "got: {s}"
    );
}

/// Regression: a multi-line Bash tool result (expanded branch) must render one
/// VISUAL row per body line. The pre-fix single-`Row`-of-spans layout collapsed
/// the body onto one row; the Column-of-rows fix restores line breaks.
#[test]
fn bash_expanded_multi_line_renders_one_row_per_line() {
    let mut element = element! {
        UserToolResultMessage(
            id: id(),
            tool: "Bash".to_string(),
            result: serde_json::json!({"content": "alpha\nbeta\ngamma"}),
            expanded: true,
            focused: false,
        )
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    // Header `└ ` row + 3 body rows = 4 visual rows.
    assert_eq!(
        lines.len(),
        4,
        "expected header + 3 body rows = 4 visual lines, got {}: {out:?}",
        lines.len()
    );
    assert!(out.contains("alpha"));
    assert!(out.contains("beta"));
    assert!(out.contains("gamma"));
    // The three body lines must be on separate rows (not collapsed).
    assert!(
        !out.contains("alphabetagamma"),
        "body must not be collapsed onto one row: {out:?}"
    );
}

/// A single-line Bash body still renders header + exactly one body row.
#[test]
fn bash_expanded_single_line_stays_one_body_row() {
    let mut element = element! {
        UserToolResultMessage(
            id: id(),
            tool: "Bash".to_string(),
            result: serde_json::json!({"content": "only"}),
            expanded: true,
            focused: false,
        )
    };
    let out = element.to_string();
    // Header row + 1 body row = 2 rows.
    assert_eq!(out.lines().count(), 2, "got: {out:?}");
    assert!(out.contains("only"));
}

/// Measurement (the M7-03 scroll-cache oracle) when the `result` arrives as a
/// bare JSON string — the shape the proxy's `as_str()` path handles — counts
/// the same number of body lines the expanded component draws.
///
/// `UserToolResultMessage`'s `body_text` only unwraps the `{"content": …}`
/// object shape; a bare string falls through to a pretty-printed (quoted,
/// escaped) single JSON line, so the component draws header + 1 body row. The
/// measure proxy, in contrast, takes the bare string verbatim via `as_str()`
/// and counts its 3 raw lines. These two extraction paths diverge for the
/// bare-string shape — a known, documented proxy/renderer drift (the M7-04/05
/// unification folds them together; it requires threading the per-id
/// `expanded` flag into `measured_height`, which is out of scope for this
/// render-layout fix). This test pins the CURRENT measure-proxy behavior so
/// that future unification work consciously revisits it.
#[test]
fn bash_expanded_measured_height_pins_bare_string_proxy() {
    use tui::components::virtual_message_list::measured_height;
    use tui::state::RenderedMessage;

    let msg = RenderedMessage::UserToolResult {
        id: id(),
        tool: "Bash".to_string(),
        result: serde_json::json!("alpha\nbeta\ngamma"),
        old_string: None,
        new_string: None,
        file_path: None,
    };
    // Proxy counts the 3 raw body lines from the bare string.
    assert_eq!(measured_height(&msg, 80), 3);
}

/// ANSI colors survive the per-line row split in the Bash-expanded branch.
#[test]
fn bash_expanded_ansi_color_survives_line_split() {
    let mut element = element! {
        UserToolResultMessage(
            id: id(),
            tool: "Bash".to_string(),
            result: serde_json::json!({"content": "\x1b[31merr\x1b[0m\nok"}),
            expanded: true,
            focused: false,
        )
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    // Header + 2 body rows.
    assert_eq!(lines.len(), 3, "lines must stay separate, got: {out:?}");
    assert!(out.contains("err"));
    assert!(out.contains("ok"));
    assert!(!out.contains("errok"), "body must not collapse: {out:?}");
}

#[test]
fn collapsed_long_result_shows_first_line_plus_lines_suffix() {
    let body = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let s = render_user_tool_result_to_string(UserToolResultProps {
        id: id(),
        tool: "Bash".into(),
        result: serde_json::json!({"content": body}),
        expanded: false,
        focused: false,
        ..Default::default()
    });
    assert_eq!(s, "└ line1 (+9 lines)");
}
