//! REPL screen — the M6-02 layout, extended with M6-03 streaming spinner.
//!
//! Composes vertical zones (top → bottom):
//!   1. `StatusLine`       — height 1
//!   2. `Scrollback`       — flex-grow 1
//!   3. `SpinnerWithVerb`  — height 1, conditional on `streaming`
//!   4. `PromptInput`      — height 1 (multi-line wrap arrives in M7)

use std::collections::HashMap;

use iocraft::prelude::*;
use protocol::ToolUseId;

use crate::components::prompt_input::completion::{CompletionOverlay, CompletionState};
use crate::components::prompt_input::palette::{PaletteOverlay, PaletteState, OVERLAY_MAX_ITEMS};
use crate::components::prompt_input::{
    HistorySearchOverlay, HistorySearchState, PromptInput, PromptInputFooter, VimMode,
};
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
///
/// `Default` is hand-rolled (not derived) because [`VimMode`] is a
/// contract-locked enum without a `Default` impl.
#[derive(Props)]
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
    /// (M7-07) Slash-command palette overlay state (clone of `AppState.palette`).
    /// Rendered as a dropdown ABOVE `PromptInput` when `open`. `None` collapses
    /// to a no-op via `Option::default`.
    pub palette: Option<PaletteState>,
    /// (M7-07) `@` file-ref completion overlay state (clone of
    /// `AppState.completion`). Rendered above `PromptInput` when `open`.
    pub completion: Option<CompletionState>,
    /// (M7-08) Whether vim mode is enabled (clone of `AppState.vim_enabled`).
    /// Gates the footer `-- MODE --` indicator.
    pub vim_enabled: bool,
    /// (M7-08) Current vim mode (clone of `AppState.vim.mode`). Only shown
    /// when `vim_enabled`.
    pub vim_mode: VimMode,
    /// (M7-09) Whether the active Visual selection is linewise (`V`). Selects
    /// `-- VISUAL LINE --` over `-- VISUAL --` in the footer.
    pub vim_visual_linewise: bool,
    /// (M7-10) Active Ctrl-R history-search overlay (clone of
    /// `AppState.history_search`). Rendered as a row ABOVE the prompt when
    /// `Some(_)`, mirroring claude-code's `HistorySearchInput` box.
    pub history_search: Option<HistorySearchState>,
    /// (M7-15) Active render palette (clone of `AppState.theme`). Threaded into
    /// the scrollback (`VirtualMessageList`), `StatusLine`, and the
    /// palette/completion overlays so the whole REPL recolors on theme change.
    pub theme: crate::theme::Theme,
    /// (M7-15) Active theme name (clone of `AppState.theme_setting.resolve()`).
    /// Drives syntect-colored diffs in scrollback.
    pub theme_name: crate::theme::ThemeName,
}

impl Default for ReplScreenProps {
    fn default() -> Self {
        Self {
            status: StatusSnapshot::default(),
            messages: Vec::new(),
            cache: HeightCache::default(),
            prompt_text: String::new(),
            prompt_cursor: 0,
            prompt_width: 0,
            scroll_offset: 0,
            viewport_height: 0,
            show_spinner: false,
            expanded: HashMap::new(),
            focused_tool_id: None,
            palette: None,
            completion: None,
            vim_enabled: false,
            vim_mode: VimMode::Insert,
            vim_visual_linewise: false,
            history_search: None,
            theme: crate::theme::Theme::dark(),
            theme_name: crate::theme::ThemeName::Dark,
        }
    }
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
    let palette = props.palette.clone();
    let completion = props.completion.clone();
    let vim_enabled = props.vim_enabled;
    let vim_mode = props.vim_mode;
    let vim_visual_linewise = props.vim_visual_linewise;
    let history_search = props.history_search.clone();
    let theme = props.theme;
    let theme_name = props.theme_name;
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            StatusLine(
                model: model,
                cwd: cwd,
                cost: cost,
                context_pct: context_pct,
                permission_mode: permission_mode,
                theme: theme,
            )
            VirtualMessageList(
                messages: messages,
                cache: cache,
                scroll_offset: scroll_offset,
                viewport_height: viewport_height,
                expanded: expanded,
                focused_tool_id: focused_tool_id,
                theme: theme,
                theme_name: theme_name,
            )
            #(if show_spinner {
                element!(SpinnerWithVerb).into_any()
            } else {
                element!(View).into_any()
            })
            // (M7-07) Active overlay (palette OR completion) renders as a
            // dropdown ABOVE the prompt row. iocraft 0.8 has no z-index, so
            // this draws inline in the bottom zone (design D4). Only one is
            // ever open at a time (the dispatcher enforces palette-wins-on-`/`).
            #(palette.as_ref().filter(|p| p.open).map(|p| {
                let rows: Vec<_> = p.rows().into_iter().take(OVERLAY_MAX_ITEMS).collect();
                element! {
                    PaletteOverlay(rows: rows, selected: p.selected, theme: theme)
                }
            }))
            #(completion.as_ref().filter(|c| c.open).map(|c| {
                let rows = c.rows();
                let empty_query = c.filter.is_empty();
                element! {
                    CompletionOverlay(rows: rows, selected: c.selected, empty_query: empty_query, theme: theme)
                }
            }))
            // (M7-10) History-search overlay row, drawn ABOVE the prompt when
            // active (mirrors claude-code `HistorySearchInput`). Mutually
            // exclusive with palette/completion — the dispatcher only opens one
            // priority-3 overlay at a time. `failed_match` selects the
            // `no matching prompt:` label (a non-empty query that resolved to
            // no match).
            #(history_search.as_ref().map(|hs| {
                let query = hs.query.clone();
                let failed_match = !hs.query.is_empty() && hs.match_index.is_none();
                element! {
                    HistorySearchOverlay(query: query, failed_match: failed_match)
                }
            }))
            PromptInput(
                text: prompt_text,
                cursor: prompt_cursor,
                width: prompt_width,
            )
            PromptInputFooter(
                mode: crate::components::prompt_input::FooterMode::Prompt,
                placeholder: None,
                is_empty: prompt_is_empty,
                vim_enabled: vim_enabled,
                vim_mode: vim_mode,
                vim_visual_linewise: vim_visual_linewise,
            )
        }
    }
}
