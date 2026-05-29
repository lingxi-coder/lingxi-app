//! Insta snapshots of the palette + completion dropdowns. The render harness
//! mirrors the other `render_*` tests in this crate (see render_status_line.rs).

use iocraft::prelude::*;
use lingxi_tui::components::prompt_input::palette::{PaletteOverlay, PaletteState};

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
