//! M7-06 snapshots: the `PromptInput` footer surface and a 3-line input.
use iocraft::prelude::*;
use tui::components::prompt_input::{FooterMode, PromptInput, PromptInputFooter};

#[test]
fn footer_prompt_mode_with_placeholder() {
    let mut el = element! {
        PromptInputFooter(
            mode: FooterMode::Prompt,
            placeholder: Some("Try \"edit <filepath> to...\"".to_string()),
            is_empty: true,
        )
    };
    insta::assert_snapshot!("footer_prompt_mode_with_placeholder", el.to_string());
}

#[test]
fn input_three_lines() {
    let mut el = element! {
        PromptInput(text: "first\nsecond\nthird".to_string(), cursor: 0usize, width: 40usize)
    };
    insta::assert_snapshot!("input_three_lines", el.to_string());
}
