//! M7-10 snapshot: the Ctrl-R search overlay row in its three states
//! (idle/search, active query, no-match) — locks the byte-for-byte labels
//! from claude-code HistorySearchInput.tsx.

use iocraft::prelude::*;
use lingxi_tui::components::prompt_input::HistorySearchOverlay;

fn render_row(query: &str, failed: bool) -> String {
    let mut canvas = element! {
        View(width: 60u16) {
            HistorySearchOverlay(query: query.to_string(), failed_match: failed)
        }
    };
    canvas.to_string()
}

#[test]
fn overlay_idle_label() {
    insta::assert_snapshot!("history_search_idle", render_row("", false));
}

#[test]
fn overlay_active_query() {
    insta::assert_snapshot!("history_search_query", render_row("cargo", false));
}

#[test]
fn overlay_no_match() {
    insta::assert_snapshot!("history_search_no_match", render_row("zzz", true));
}
