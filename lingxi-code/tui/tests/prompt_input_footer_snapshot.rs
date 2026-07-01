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
fn footer_shows_esc_to_interrupt_while_loading() {
    // (PIC-07) Same hint slot as "? for shortcuts", swapped while a turn is
    // in flight.
    let mut el = element! {
        PromptInputFooter(
            mode: FooterMode::Prompt,
            is_empty: true,
            is_loading: true,
        )
    };
    let frame = el.to_string();
    assert!(frame.contains("esc to interrupt"), "{frame}");
    assert!(!frame.contains("for shortcuts"), "{frame}");
}

#[test]
fn footer_hint_suppressed_when_buffer_nonempty_even_while_loading() {
    // (PIC-07) is_loading doesn't bypass the showHint gate — a non-empty
    // buffer still suppresses the hint row entirely.
    let mut el = element! {
        PromptInputFooter(
            mode: FooterMode::Prompt,
            is_empty: false,
            is_loading: true,
        )
    };
    let frame = el.to_string();
    assert!(!frame.contains("interrupt"), "{frame}");
    assert!(!frame.contains("for shortcuts"), "{frame}");
}

#[test]
fn footer_exit_hint_replaces_everything_else() {
    // (RRS-08) exit_hint overrides the mode indicator/hint/placeholder
    // entirely, matching claude-code's exitMessage.show early return.
    let mut el = element! {
        PromptInputFooter(
            mode: FooterMode::Prompt,
            placeholder: Some("Try \"edit <filepath> to...\"".to_string()),
            is_empty: true,
            is_loading: true,
            exit_hint: Some("Ctrl-C"),
        )
    };
    let frame = el.to_string();
    assert!(frame.contains("Press Ctrl-C again to exit"), "{frame}");
    assert!(!frame.contains("interrupt"), "{frame}");
    assert!(!frame.contains("for shortcuts"), "{frame}");
    assert!(!frame.contains("Try"), "{frame}");
}

#[test]
fn footer_model_renders_on_reserved_hint_row() {
    let mut el = element! {
        View(width: 60) {
            PromptInputFooter(
                mode: FooterMode::Prompt,
                is_empty: true,
                active_model: Some("claude-sonnet-4.5".to_string()),
            )
        }
    };
    let frame = el.to_string();
    assert!(frame.contains("? for shortcuts"), "{frame}");
    assert!(frame.contains("claude-sonnet-4.5"), "{frame}");
}

#[test]
fn footer_exit_hint_keeps_right_side_model() {
    let mut el = element! {
        View(width: 72) {
            PromptInputFooter(
                mode: FooterMode::Prompt,
                is_empty: true,
                exit_hint: Some("Ctrl-C"),
                active_model: Some("claude-sonnet-4.5".to_string()),
            )
        }
    };
    let frame = el.to_string();
    assert!(frame.contains("Press Ctrl-C again to exit"), "{frame}");
    assert!(frame.contains("claude-sonnet-4.5"), "{frame}");
}

#[test]
fn footer_model_truncates_instead_of_wrapping_when_narrow() {
    let mut el = element! {
        View(width: 32) {
            PromptInputFooter(
                mode: FooterMode::Prompt,
                is_empty: true,
                width: Some(32_usize),
                active_model: Some("claude-sonnet-4.5".to_string()),
            )
        }
    };
    let frame = el.to_string();
    assert_eq!(
        frame.lines().count(),
        1,
        "model must stay on the reserved footer row, never wrap:\n{frame}"
    );
    assert!(frame.contains("? for shortcuts"), "{frame}");
    assert!(frame.contains('\u{2026}'), "{frame}");
}

#[test]
fn input_three_lines() {
    let mut el = element! {
        PromptInput(text: "first\nsecond\nthird".to_string(), cursor: 0usize, width: 40usize)
    };
    insta::assert_snapshot!("input_three_lines", el.to_string());
}
