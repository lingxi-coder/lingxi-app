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
