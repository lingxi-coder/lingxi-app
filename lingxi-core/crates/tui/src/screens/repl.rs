//! REPL screen — the M6-02 layout, extended with M6-03 streaming spinner.
//!
//! Composes vertical zones (top → bottom):
//!   1. `StatusLine`       — height 1
//!   2. `Scrollback`       — flex-grow 1
//!   3. `SpinnerWithVerb`  — height 1, conditional on `streaming`
//!   4. `PromptInput`      — height 1 (multi-line wrap arrives in M7)

use std::collections::HashMap;

use iocraft::prelude::*;
use lingxi_protocol::ToolUseId;

use crate::components::prompt_input::{PromptInput, PromptInputFooter};
use crate::components::spinner::SpinnerWithVerb;
use crate::components::status_line::StatusLine;
use crate::components::virtual_message_list::{HeightCache, VirtualMessageList};
use crate::state::{AppState, RenderedMessage, StatusSnapshot};

/// Predicate exposed for tests + the renderer's conditional mount.
/// Returns `true` iff the spinner should be visible (a turn is streaming).
#[inline]
#[must_use]
pub fn should_render_spinner(state: &AppState) -> bool {
    state.streaming.is_some()
}

/// Props for `ReplScreen`. All fields are clones of the current
/// `AppState`; iocraft's reactive model handles re-rendering on
/// successive frames.
#[derive(Default, Props)]
pub struct ReplScreenProps {
    /// Status-line snapshot.
    pub status: StatusSnapshot,
    /// Scrollback messages (most recent at end).
    pub messages: Vec<RenderedMessage>,
    /// Pre-built, width-synced height cache (clone of
    /// `AppState::height_cache`). Threaded straight through to
    /// `VirtualMessageList` so the windowed renderer never rebuilds it.
    pub cache: HeightCache,
    /// Current prompt text.
    pub prompt_text: String,
    /// Cursor byte index in `prompt_text`.
    pub prompt_cursor: usize,
    /// Total terminal width — drives the prompt's wrap + content height.
    pub prompt_width: usize,
    /// Scroll offset (0 = latest at bottom).
    pub scroll_offset: usize,
    /// Live viewport height (rows available for the scrollback).
    pub viewport_height: usize,
    /// Whether to render the streaming spinner between scrollback and
    /// prompt input. Wired from `AppState.streaming.is_some()`.
    pub show_spinner: bool,
    /// (M6-04) Per-tool expanded flags (clone of `AppState.expanded`).
    pub expanded: HashMap<ToolUseId, bool>,
    /// (M6-04) Focused tool id (clone of `AppState.focused_tool_id`).
    pub focused_tool_id: Option<ToolUseId>,
}

/// Compose the vertical zones of the M6-03 REPL screen.
#[component]
pub fn ReplScreen(props: &ReplScreenProps) -> impl Into<AnyElement<'static>> {
    let model = props.status.model.clone();
    let cwd = props.status.cwd.clone();
    let cost = props.status.cost.clone();
    let context_pct = props.status.context_pct;
    let permission_mode = props.status.permission_mode;
    let messages = props.messages.clone();
    let cache = props.cache.clone();
    let prompt_text = props.prompt_text.clone();
    let prompt_cursor = props.prompt_cursor;
    let prompt_width = props.prompt_width;
    let prompt_is_empty = props.prompt_text.is_empty();
    let scroll_offset = props.scroll_offset;
    let viewport_height = props.viewport_height;
    let show_spinner = props.show_spinner;
    let expanded = props.expanded.clone();
    let focused_tool_id = props.focused_tool_id;
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            StatusLine(
                model: model,
                cwd: cwd,
                cost: cost,
                context_pct: context_pct,
                permission_mode: permission_mode,
            )
            VirtualMessageList(
                messages: messages,
                cache: cache,
                scroll_offset: scroll_offset,
                viewport_height: viewport_height,
                expanded: expanded,
                focused_tool_id: focused_tool_id,
            )
            #(if show_spinner {
                element!(SpinnerWithVerb).into_any()
            } else {
                element!(View).into_any()
            })
            PromptInput(
                text: prompt_text,
                cursor: prompt_cursor,
                width: prompt_width,
            )
            PromptInputFooter(
                mode: crate::components::prompt_input::FooterMode::Prompt,
                placeholder: None,
                is_empty: prompt_is_empty,
            )
        }
    }
}
