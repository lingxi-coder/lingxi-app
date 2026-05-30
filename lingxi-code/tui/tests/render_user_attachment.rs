//! (M7-05) Tests for the AttachmentMessage solo-user summary renderer.
#![allow(clippy::doc_markdown)]

use iocraft::prelude::*;
use tui::components::messages::attachment::{
    render_attachment_to_string, Attachment, AttachmentMessage,
};

#[test]
fn directory_line() {
    let a = Attachment::Directory {
        display_path: "src".into(),
    };
    assert_eq!(render_attachment_to_string(&a), "Listed directory src/");
}

#[test]
fn file_line_truncated() {
    let a = Attachment::File {
        display_path: "a.rs".into(),
        num_lines: 120,
        truncated: true,
    };
    assert_eq!(render_attachment_to_string(&a), "Read a.rs (120+ lines)");
}

#[test]
fn file_line_untruncated() {
    let a = Attachment::File {
        display_path: "a.rs".into(),
        num_lines: 10,
        truncated: false,
    };
    assert_eq!(render_attachment_to_string(&a), "Read a.rs (10 lines)");
}

#[test]
fn selected_lines() {
    let a = Attachment::SelectedLines {
        count: 3,
        display_path: "x.rs".into(),
        ide_name: "VSCode".into(),
    };
    assert_eq!(
        render_attachment_to_string(&a),
        "\u{29C9} Selected 3 lines from x.rs in VSCode"
    );
}

#[test]
fn mcp_resource_line() {
    let a = Attachment::McpResource {
        name: "doc".into(),
        server: "ctx7".into(),
    };
    assert_eq!(
        render_attachment_to_string(&a),
        "Read MCP resource doc from ctx7"
    );
}

#[test]
fn plan_file_referenced() {
    let a = Attachment::PlanFileReference {
        plan_file_path: "/tmp/p.md".into(),
    };
    assert_eq!(
        render_attachment_to_string(&a),
        "Plan file referenced (/tmp/p.md)"
    );
}

#[test]
fn attachment_snapshot() {
    let a = Attachment::PdfReference {
        display_path: "doc.pdf".into(),
        page_count: 5,
    };
    let mut e = element! { AttachmentMessage(attachment: a) };
    insta::assert_snapshot!("attachment_pdf", e.to_string());
}
