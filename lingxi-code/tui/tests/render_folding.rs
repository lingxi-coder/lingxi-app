//! (M7-05) Folding tests: grouped_tool_use + collapsed_read_search.
#![allow(clippy::doc_markdown)]

use protocol::ToolUseId;
use tui::components::messages::collapsed_read_search::{
    render_collapsed_to_string, CollapsedCounts,
};
use tui::components::messages::grouped_tool_use::render_grouped_to_string;

fn pair(p: &str) -> (serde_json::Value, serde_json::Value) {
    (
        serde_json::json!({ "file_path": p }),
        serde_json::json!({ "content": "ok" }),
    )
}

#[test]
fn grouped_collapsed_shows_count() {
    let entries = vec![pair("a.rs"), pair("b.rs"), pair("c.rs")];
    let s = render_grouped_to_string("Read", &entries, /*expanded*/ false);
    assert_eq!(s, "\u{25CF} Read (\u{00D7}3)");
}

#[test]
fn grouped_single_drops_count() {
    let entries = vec![pair("a.rs")];
    let s = render_grouped_to_string("Read", &entries, false);
    assert_eq!(s, "\u{25CF} Read");
}

#[test]
fn grouped_expanded_lists_children() {
    let entries = vec![pair("a.rs"), pair("b.rs")];
    let s = render_grouped_to_string("Read", &entries, true);
    assert!(
        s.starts_with("\u{25CF} Read (\u{00D7}2)"),
        "header missing: {s}"
    );
    assert!(s.matches('\n').count() >= 2, "expected child lines: {s}");
    let _ = ToolUseId::new(); // id type is in scope for the variant
}

#[test]
fn collapsed_finalized_summary() {
    let c = CollapsedCounts {
        search: 2,
        read: 1,
        list: 0,
        is_active: false,
    };
    let s = render_collapsed_to_string(&c, &[], /*expanded*/ false);
    assert_eq!(s, "  \u{23BF}  Searched for 2 patterns, read 1 file");
}

#[test]
fn collapsed_active_present_tense() {
    let c = CollapsedCounts {
        search: 0,
        read: 3,
        list: 0,
        is_active: true,
    };
    let s = render_collapsed_to_string(&c, &[], false);
    assert_eq!(s, "  \u{23BF}  Reading 3 files");
}

#[test]
fn collapsed_list_directories_plural() {
    let c = CollapsedCounts {
        search: 0,
        read: 0,
        list: 2,
        is_active: false,
    };
    assert_eq!(
        render_collapsed_to_string(&c, &[], false),
        "  \u{23BF}  Listed 2 directories"
    );
}

#[test]
fn collapsed_zero_counts_empty() {
    let c = CollapsedCounts::default();
    assert_eq!(render_collapsed_to_string(&c, &[], false), "");
}

#[test]
fn collapsed_expanded_lists_entries() {
    let c = CollapsedCounts {
        search: 0,
        read: 2,
        list: 0,
        is_active: false,
    };
    let entries = vec!["a.rs".to_string(), "b.rs".to_string()];
    let s = render_collapsed_to_string(&c, &entries, true);
    assert!(s.starts_with("  \u{23BF}  Read 2 files"), "header: {s}");
    assert!(s.contains("a.rs") && s.contains("b.rs"), "entries: {s}");
}
