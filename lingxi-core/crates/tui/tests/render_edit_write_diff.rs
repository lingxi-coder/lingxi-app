//! Edit/Write tool results render as a `StructuredDiff` (M7-02).
use lingxi_tui::components::messages::user_tool_result::{
    is_diff_tool, render_edit_write_diff_lines,
};
use lingxi_tui::theme::TuiTheme;

#[test]
fn edit_result_renders_structured_diff() {
    let lines = render_edit_write_diff_lines(
        "Edit",
        Some("foo()"),
        Some("bar()"),
        Some("src/a.rs"),
        &TuiTheme,
    );
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
        .collect();
    assert!(
        joined.contains('-') && joined.contains('+'),
        "Edit shows a -/+ diff: {joined:?}"
    );
    assert!(joined.contains("foo") && joined.contains("bar"));
}

#[test]
fn write_result_renders_as_pure_add() {
    let lines = render_edit_write_diff_lines(
        "Write",
        None, // no prior content
        Some("new line 1\nnew line 2"),
        Some("src/b.rs"),
        &TuiTheme,
    );
    // The diff body (excluding the @@ header) must carry only add sigils.
    let body: String = lines
        .iter()
        .filter(|l| !l.plain_text().contains("@@"))
        .flat_map(|l| l.spans.iter().map(|s| s.text.clone()))
        .collect();
    assert!(body.contains('+'), "Write is a pure-add diff: {body:?}");
    assert!(!body.contains('-'), "Write has no remove lines: {body:?}");
}

#[test]
fn non_edit_tool_returns_none_for_diff() {
    // A Bash/Read result must NOT route through the diff path.
    assert!(!is_diff_tool("Bash"));
    assert!(!is_diff_tool("Read"));
    assert!(is_diff_tool("Edit"));
    assert!(is_diff_tool("Write"));
}
