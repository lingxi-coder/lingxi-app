//! (M7-05) Snapshot + behavior tests for bash_input + bash_output renderers.
#![allow(clippy::doc_markdown)]

use iocraft::prelude::*;
use lingxi_tui::components::messages::bash_input::UserBashInputMessage;
use lingxi_tui::components::messages::bash_output::{
    render_bash_output_spans, UserBashOutputMessage,
};

#[test]
fn bash_input_renders_bang_prefix() {
    let mut element = element! {
        UserBashInputMessage(command: "ls -la".to_string())
    };
    insta::assert_snapshot!("bash_input_basic", element.to_string());
}

#[test]
fn bash_output_parses_ansi_into_spans() {
    // SGR 31 (red) "err" reset, then plain "ok".
    let spans = render_bash_output_spans("\x1b[31merr\x1b[0mok", "");
    assert!(
        spans.len() >= 2,
        "expected ANSI split into >=2 spans, got {}",
        spans.len()
    );
    assert_eq!(
        spans.iter().map(|s| s.text.as_str()).collect::<String>(),
        "errok"
    );
}

#[test]
fn bash_output_snapshot() {
    let mut element = element! {
        UserBashOutputMessage(stdout: "hello\nworld".to_string(), stderr: "".to_string())
    };
    insta::assert_snapshot!("bash_output_plain", element.to_string());
}

/// Regression: multi-line bash output must render one VISUAL line per parsed
/// line. The pre-fix single-`Row`-of-spans layout collapsed everything onto a
/// single row (`helloworld`); the Column-of-rows fix restores line breaks.
#[test]
fn bash_output_renders_two_visual_lines() {
    let mut element = element! {
        UserBashOutputMessage(stdout: "hello\nworld".to_string(), stderr: "".to_string())
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected 2 visual lines, got {}: {out:?}",
        lines.len()
    );
    assert_eq!(lines[0], "hello");
    assert_eq!(lines[1], "world");
}

#[test]
fn bash_output_renders_three_visual_lines() {
    let mut element = element! {
        UserBashOutputMessage(stdout: "a\nb\nc".to_string(), stderr: "".to_string())
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "expected 3 visual lines, got {}: {out:?}",
        lines.len()
    );
    assert_eq!(lines, vec!["a", "b", "c"]);
}

/// A single-line body still renders as exactly one row (no behavior change for
/// the common case, no spurious trailing blank row).
#[test]
fn bash_output_single_line_stays_one_row() {
    let mut element = element! {
        UserBashOutputMessage(stdout: "only".to_string(), stderr: "".to_string())
    };
    let out = element.to_string();
    assert_eq!(out.lines().count(), 1, "got: {out:?}");
    assert_eq!(out.lines().next(), Some("only"));
}

/// Measurement (the M7-03 scroll-cache oracle) must equal the number of rows
/// the component actually renders. Pins measurement == render for the
/// component, not just measurement vs the joined string oracle.
#[test]
fn bash_output_measured_height_matches_rendered_rows() {
    use lingxi_tui::components::virtual_message_list::measured_height;
    use lingxi_tui::state::RenderedMessage;

    let msg = RenderedMessage::UserBashOutput {
        stdout: "a\nb\nc".to_string(),
        stderr: String::new(),
    };
    let mut element = element! {
        UserBashOutputMessage(stdout: "a\nb\nc".to_string(), stderr: "".to_string())
    };
    let rendered_rows = element.to_string().lines().count();
    assert_eq!(rendered_rows, 3, "sanity: 3-line body renders 3 rows");
    assert_eq!(
        measured_height(&msg, 80),
        rendered_rows,
        "measured_height must equal the component's rendered row count"
    );
}

/// ANSI colors survive the per-line row split: a 2-line body where line 1 has
/// a red SGR run still parses to a red span on line 1, and the lines stay
/// separate.
#[test]
fn bash_output_ansi_color_survives_line_split() {
    use lingxi_tui::render::{NamedColor, StyleColor};

    // Line 1: red "err"; line 2: plain "ok".
    let spans = render_bash_output_spans("\x1b[31merr\x1b[0m\nok", "");
    // The red "err" span and the plain "ok" span are separated by a `\n` span.
    let red_err = spans
        .iter()
        .any(|s| s.text.contains("err") && s.style.fg == StyleColor::Named(NamedColor::Red));
    assert!(red_err, "expected red err span, got {spans:?}");

    let mut element = element! {
        UserBashOutputMessage(stdout: "\x1b[31merr\x1b[0m\nok".to_string(), stderr: "".to_string())
    };
    let out = element.to_string();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "lines must stay separate, got: {out:?}");
    assert_eq!(lines[0], "err");
    assert_eq!(lines[1], "ok");
}
