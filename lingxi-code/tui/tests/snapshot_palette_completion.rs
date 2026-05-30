//! Insta snapshots of the palette + completion dropdowns. The render harness
//! mirrors the other `render_*` tests in this crate (see `render_status_line.rs`).

use iocraft::prelude::*;
use tui::components::prompt_input::palette::{PaletteOverlay, PaletteState};

fn render(el: impl Into<AnyElement<'static>>) -> String {
    let mut canvas = element! { View(width: 60u16) { #(el.into()) } };
    canvas.to_string()
}

#[test]
fn palette_dropdown_three_filtered_commands() {
    let mut state = PaletteState::default();
    state.sync_from_prompt("/co"); // compact / config / context / copy / cost ...
                                   // Keep snapshot stable: take the first 3 ranked rows.
    let rows: Vec<_> = state.rows().into_iter().take(3).collect();
    let out = render(element! { PaletteOverlay(rows: rows, selected: 0usize) });
    insta::assert_snapshot!(out);
}

use tui::components::prompt_input::completion::CompletionOverlay;

#[test]
fn completion_dropdown_three_paths() {
    let rows = vec![
        "src/lib.rs".to_string(),
        "src/main.rs".to_string(),
        "README.md".to_string(),
    ];
    let out = render(element! {
        CompletionOverlay(rows: rows, selected: 1usize, empty_query: false)
    });
    insta::assert_snapshot!(out);
}

#[test]
fn completion_empty_state_no_query() {
    let out = render(element! {
        CompletionOverlay(rows: Vec::<String>::new(), selected: 0usize, empty_query: true)
    });
    insta::assert_snapshot!(out);
}

use tui::app::render_screen;
use tui::state::{AppState, StatusSnapshot};

#[test]
fn repl_screen_shows_palette_above_prompt() {
    let mut st = AppState::new(StatusSnapshot::default());
    // Open the palette via the public sync the live path uses.
    st.prompt_text = "/co".into();
    st.prompt_cursor = 3;
    st.palette.sync_from_prompt(&st.prompt_text);
    // render_screen(state, viewport_height, viewport_width) — 3 args (M7-03+).
    let el = render_screen(&st, 10, 60);
    let out = render(el);
    assert!(
        out.contains('\u{2013}'),
        "palette rows use the en-dash separator"
    );
    insta::assert_snapshot!(out);
}
