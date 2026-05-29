//! (M7-05) Tests for memory_input, plan, prompt, resource_update, image.
#![allow(clippy::doc_markdown)]

use iocraft::prelude::*;
use lingxi_tui::components::messages::image::{render_image_label, UserImageMessage};
use lingxi_tui::components::messages::memory_input::{
    render_memory_to_string, UserMemoryInputMessage,
};
use lingxi_tui::components::messages::plan::{render_plan_to_string, UserPlanMessage};
use lingxi_tui::components::messages::prompt::{render_prompt_to_string, MAX_DISPLAY_CHARS};
use lingxi_tui::components::messages::resource_update::{
    format_uri, render_resource_update_to_string, ResourceUpdate, UserResourceUpdateMessage,
};

#[test]
fn memory_input_form() {
    assert_eq!(
        render_memory_to_string("prefer tabs"),
        "# prefer tabs\nGot it."
    );
}

#[test]
fn memory_snapshot() {
    let mut e = element! { UserMemoryInputMessage(input: "use rg".to_string()) };
    insta::assert_snapshot!("memory_use_rg", e.to_string());
}

#[test]
fn plan_header_literal() {
    let s = render_plan_to_string("- step one");
    assert!(s.starts_with("Plan to implement"), "got: {s}");
    assert!(s.contains("step one"));
}

#[test]
fn plan_snapshot() {
    let mut e = element! { UserPlanMessage(plan_content: "1. do it".to_string()) };
    insta::assert_snapshot!("plan_basic", e.to_string());
}

#[test]
fn short_prompt_unchanged() {
    assert_eq!(render_prompt_to_string("hello"), "hello");
}

#[test]
fn long_prompt_truncates_head_tail() {
    let body = "x".repeat(MAX_DISPLAY_CHARS + 100);
    let s = render_prompt_to_string(&body);
    assert!(
        s.contains("\u{2026} +"),
        "expected ellipsis marker, got len {}",
        s.len()
    );
    assert!(s.contains(" lines \u{2026}"));
    assert!(s.len() < body.len());
}

#[test]
fn image_label_with_and_without_id() {
    assert_eq!(render_image_label(Some(3), None), "[Image #3]");
    assert_eq!(render_image_label(None, None), "[Image]");
    assert_eq!(
        render_image_label(Some(1), Some("800x600")),
        "[Image #1] (800x600)"
    );
}

#[test]
fn image_snapshot() {
    let mut e =
        element! { UserImageMessage(image_id: Some(2u64), metadata: Option::<String>::None) };
    insta::assert_snapshot!("image_id_2", e.to_string());
}

#[test]
fn format_uri_file_shows_basename() {
    assert_eq!(format_uri("file:///a/b/c.rs"), "c.rs");
}

#[test]
fn format_uri_long_truncates() {
    let long = format!("https://{}", "x".repeat(60));
    let out = format_uri(&long);
    assert!(out.ends_with('\u{2026}'));
    assert_eq!(out.chars().count(), 40);
}

#[test]
fn resource_update_line() {
    let u = ResourceUpdate {
        server: "fs".into(),
        target: "x.rs".into(),
        reason: Some("changed".into()),
    };
    assert_eq!(
        render_resource_update_to_string(&[u]),
        "\u{21BB} fs: x.rs \u{00B7} changed"
    );
}

#[test]
fn resource_update_snapshot() {
    let mut e = element! {
        UserResourceUpdateMessage(updates: vec![
            ("fs".to_string(), "x.rs".to_string(), Some("changed".to_string())),
        ])
    };
    insta::assert_snapshot!("resource_update_line", e.to_string());
}
