//! Snapshot tests for `UserTextMessage` and `AssistantTextMessage`.
//!
//! Locks byte-strings L4 (`"● "`) and L5 (`"> "`) from M6-02 T0.

use iocraft::prelude::*;
use tui::components::messages::assistant_text::AssistantTextMessage;
use tui::components::messages::user_text::UserTextMessage;

#[test]
fn user_text_single_line() {
    let mut element = element! {
        UserTextMessage(body: "hi".to_string())
    };
    insta::assert_snapshot!("user_text_single_line", element.to_string());
}

#[test]
fn assistant_text_three_lines() {
    let mut element = element! {
        AssistantTextMessage(body: "line one\nline two\nline three".to_string())
    };
    insta::assert_snapshot!("assistant_text_three_lines", element.to_string());
}

#[test]
fn task_assignment_no_description() {
    let mut element = element! {
        tui::components::messages::task_assignment::TaskAssignmentMessage(
            task_id: "123".to_string(),
            assigned_by: "alice".to_string(),
            subject: "Set up DB".to_string(),
            description: None,
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("task_assignment_no_description", element.to_string());
}

#[test]
fn task_assignment_with_description() {
    let mut element = element! {
        tui::components::messages::task_assignment::TaskAssignmentMessage(
            task_id: "7".to_string(),
            assigned_by: "lead".to_string(),
            subject: "Migrate".to_string(),
            description: Some("Move tables".to_string()),
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("task_assignment_with_description", element.to_string());
}

#[test]
fn user_agent_notification_completed() {
    let mut element = element! {
        tui::components::messages::user_agent_notification::UserAgentNotificationMessage(
            summary: "Background task finished".to_string(),
            status: Some("completed".to_string()),
            theme: tui::theme::Theme::dark(),
        )
    };
    insta::assert_snapshot!("user_agent_notification_completed", element.to_string());
}
