//! M7-04 batch-1 renderer snapshots.
use lingxi_tui::components::messages::compact_boundary::render_compact_boundary_to_string;
use lingxi_tui::components::messages::redacted_thinking::render_redacted_thinking_to_string;
use lingxi_tui::components::messages::thinking::{render_thinking_to_string, ThinkingProps};

#[test]
fn thinking_collapsed() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Considering the tradeoffs between A and B.".into(),
        expanded: false,
    });
    insta::assert_snapshot!("thinking_collapsed", s);
}

#[test]
fn thinking_expanded() {
    let s = render_thinking_to_string(ThinkingProps {
        thinking: "Step one.\nStep two.".into(),
        expanded: true,
    });
    insta::assert_snapshot!("thinking_expanded", s);
}

#[test]
fn redacted_thinking_line() {
    insta::assert_snapshot!(
        "redacted_thinking_line",
        render_redacted_thinking_to_string()
    );
}

#[test]
fn compact_boundary_line() {
    insta::assert_snapshot!("compact_boundary_line", render_compact_boundary_to_string());
}
