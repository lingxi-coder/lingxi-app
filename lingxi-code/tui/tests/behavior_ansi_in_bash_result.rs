//! M6-04 T12: Bash result bodies pass through the ANSI parser; non-Bash
//! tools keep the bytes literal.

use protocol::ToolUseId;
use tui::components::messages::user_tool_result::{
    render_user_tool_result_body_spans, UserToolResultProps,
};
use tui::render::{NamedColor, StyleColor};

#[test]
fn bash_result_with_red_err_yields_red_span() {
    let body = "ok\n\x1b[31mERR\x1b[0m\nrest";
    let spans = render_user_tool_result_body_spans(&UserToolResultProps {
        id: ToolUseId::nil(),
        tool: "Bash".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    let has_red_err = spans
        .iter()
        .any(|s| s.text.contains("ERR") && s.style.fg == StyleColor::Named(NamedColor::Red));
    assert!(has_red_err, "expected a red ERR span, got {spans:?}");
}

#[test]
fn read_result_is_not_ansi_parsed() {
    // For non-Bash tools, ANSI sequences are kept as literal characters
    // (no parser applied). The TUI text element renders them verbatim;
    // the user sees the raw bytes. Matches claude-code which only runs
    // ansi-parser over Bash output.
    let body = "\x1b[31mERR\x1b[0m";
    let spans = render_user_tool_result_body_spans(&UserToolResultProps {
        id: ToolUseId::nil(),
        tool: "Read".into(),
        result: serde_json::json!({"content": body}),
        expanded: true,
        focused: false,
        ..Default::default()
    });
    // One span, raw literal text.
    assert_eq!(spans.len(), 1);
    assert!(spans[0].text.contains("\x1b[31m"));
    assert_eq!(spans[0].style.fg, StyleColor::Default);
}
