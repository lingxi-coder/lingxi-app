//! M7-15 Task 10 — snapshots: StatusLine + an assistant message + a fenced
//! code block under `dark` and `light`.
//!
//! The pure string oracles (`format_status_line`, `render_entry_to_string`,
//! `render::markdown::render(...).plain_text()`) drop color, so the textual
//! snapshot proves LAYOUT/label stability across themes (it is identical for
//! dark and light — that is the point: switching theme must not shift layout).
//! COLOR divergence — the actual M7-15 deliverable — is asserted separately
//! via the styled-line debug of the code block under the two themes.
#![allow(clippy::doc_markdown)]

use lingxi_permission::PermissionMode;
use lingxi_tui::components::messages::render_entry_to_string;
use lingxi_tui::components::status_line::format_status_line;
use lingxi_tui::render::markdown::{render as render_markdown, MarkdownTheme};
use lingxi_tui::render::{StyleColor, StyledLine};
use lingxi_tui::state::RenderedMessage;
use lingxi_tui::theme::ThemeName;

/// Fixed scrollback fixture: a user line + an assistant message carrying a
/// fenced rust code block.
fn fixture_entries() -> Vec<RenderedMessage> {
    vec![
        RenderedMessage::UserText {
            body: "hi".into(),
            timestamp: 0,
        },
        RenderedMessage::AssistantText {
            body: "Here is code:\n```rust\nfn main() { let x = 1; }\n```".into(),
            timestamp: 0,
        },
    ]
}

/// Compose the textual oracle: status line + each message's plain render.
fn render_textual() -> String {
    let mut out = format_status_line(
        "claude-sonnet-4.5",
        std::path::Path::new("/repo"),
        "$0.0000",
        0.42,
        PermissionMode::Default,
    );
    out.push('\n');
    for e in fixture_entries() {
        out.push_str(&render_entry_to_string(&e, false, false));
        out.push('\n');
    }
    out
}

/// Render the fixture's fenced code block via markdown under a theme; return
/// the styled lines (which DO carry the syntect fg color).
fn code_block_styled(theme: ThemeName) -> Vec<StyledLine> {
    let md = MarkdownTheme {
        inline_code: StyleColor::Default,
        code_theme: theme,
    };
    render_markdown("```rust\nfn main() { let x = 1; }\n```", &md)
}

#[test]
fn statusline_message_codeblock_textual_layout() {
    // Layout/label snapshot — theme-independent (oracles drop color).
    insta::assert_snapshot!("theme_textual_layout", render_textual());
}

#[test]
fn code_block_recolors_between_dark_and_light() {
    // The actual M7-15 proof: the SAME code block has different styled colors
    // under dark vs light (the syntect tmTheme follows the theme).
    let dark = code_block_styled(ThemeName::Dark);
    let light = code_block_styled(ThemeName::Light);
    assert_ne!(
        format!("{dark:?}"),
        format!("{light:?}"),
        "code block must recolor between dark and light themes"
    );
}
