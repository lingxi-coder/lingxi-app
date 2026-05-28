//! REPL screen — the only screen in M6-02.
//!
//! Composes three vertical zones (top → bottom):
//!   1. `StatusLine`   — height 1
//!   2. `Scrollback`   — flex-grow 1
//!   3. `PromptInput`  — height 1 (multi-line wrap arrives in M7)

use iocraft::prelude::*;

use crate::components::prompt_input::PromptInput;
use crate::components::scrollback::Scrollback;
use crate::components::status_line::StatusLine;
use crate::state::{RenderedMessage, StatusSnapshot};

/// Props for `ReplScreen`. All fields are clones of the current
/// `AppState`; iocraft's reactive model handles re-rendering on
/// successive frames.
#[derive(Default, Props)]
pub struct ReplScreenProps {
    /// Status-line snapshot.
    pub status: StatusSnapshot,
    /// Scrollback messages (most recent at end).
    pub messages: Vec<RenderedMessage>,
    /// Current prompt text.
    pub prompt_text: String,
    /// Cursor byte index in `prompt_text`.
    pub prompt_cursor: usize,
    /// Scroll offset (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Live viewport height (rows available for the scrollback).
    pub viewport_height: usize,
}

/// Compose the three vertical zones of the M6-02 REPL screen.
#[component]
pub fn ReplScreen(props: &ReplScreenProps) -> impl Into<AnyElement<'static>> {
    let model = props.status.model.clone();
    let cwd = props.status.cwd.clone();
    let cost = props.status.cost.clone();
    let context_pct = props.status.context_pct;
    let permission_mode = props.status.permission_mode;
    let messages = props.messages.clone();
    let prompt_text = props.prompt_text.clone();
    let prompt_cursor = props.prompt_cursor;
    let scroll_offset = props.scroll_offset;
    let viewport_height = props.viewport_height;
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            StatusLine(
                model: model,
                cwd: cwd,
                cost: cost,
                context_pct: context_pct,
                permission_mode: permission_mode,
            )
            Scrollback(
                messages: messages,
                scroll_offset: scroll_offset,
                viewport_height: viewport_height,
            )
            PromptInput(
                text: prompt_text,
                cursor: prompt_cursor,
            )
        }
    }
}
