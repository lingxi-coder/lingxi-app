//! M7-14 snapshot: message search results.

use iocraft::prelude::*;
use tui::components::message_selector::MessageSelector;

#[test]
fn snapshot_message_selector_results() {
    let mut el = element! {
        MessageSelector(
            query: "world".to_string(),
            result_labels: vec![
                "hello world".to_string(),
                "worldly affairs".to_string(),
            ],
            selected: 0usize,
        )
    };
    let out = el.to_string();
    insta::assert_snapshot!(out);
}
