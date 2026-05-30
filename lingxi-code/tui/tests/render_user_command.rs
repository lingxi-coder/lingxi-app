//! (M7-05) Tests for command + local_command_output renderers.
#![allow(clippy::doc_markdown)]

use iocraft::prelude::*;
use tui::components::messages::command::{render_command_to_string, UserCommandMessage};
use tui::components::messages::local_command_output::{
    render_local_output_to_string, UserLocalCommandOutputMessage,
};

#[test]
fn command_slash_form() {
    assert_eq!(
        render_command_to_string("clear", "", false),
        "\u{276F} /clear"
    );
    assert_eq!(
        render_command_to_string("model", "sonnet", false),
        "\u{276F} /model sonnet"
    );
}

#[test]
fn command_skill_form() {
    assert_eq!(
        render_command_to_string("brainstorm", "", true),
        "\u{276F} Skill(brainstorm)"
    );
}

#[test]
fn command_snapshot() {
    let mut e = element! {
        UserCommandMessage(command: "help".to_string(), args: "".to_string(), is_skill: false)
    };
    insta::assert_snapshot!("command_slash", e.to_string());
}

#[test]
fn local_output_no_content() {
    assert_eq!(render_local_output_to_string("", ""), "(no content)");
}

#[test]
fn local_output_snapshot() {
    let mut e = element! {
        UserLocalCommandOutputMessage(stdout: "done".to_string(), stderr: "".to_string())
    };
    insta::assert_snapshot!("local_output_done", e.to_string());
}
