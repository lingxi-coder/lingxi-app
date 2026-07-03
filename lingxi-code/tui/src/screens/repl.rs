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
use crate::components::prompt_input::palette::{PaletteOverlay, PaletteState};
use crate::components::prompt_input::{
    HistorySearchOverlay, HistorySearchState, PromptInput, PromptInputFooter, SessionColorBanner,
    VimMode,
};
use crate::components::spinner::SpinnerWithVerb;
use crate::components::status_line::StatusLine;
use crate::components::virtual_message_list::{HeightCache, VirtualMessageList};
use crate::render_iocraft::StyleColorIocraftExt;
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
    /// (ARGS.3) Inline progressive argument-hint (e.g. `"[arg2] [arg3]"`)
    /// rendered as dimmed ghost text after the typed command in the
    /// `commandWithoutArgs` state, or `None` to render nothing. Computed from
    /// `AppState::prompt_argument_hint`; `None` for every built-in / non-command
    /// buffer, keeping the prompt byte-identical to today.
    pub prompt_argument_hint: Option<String>,
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
    /// (M9-05) Background-task footer pill (claude-code `BackgroundTaskStatus`):
    /// `{n} background task[s] · ↓ to view`, or `None` when hidden (no tasks /
    /// all teammates). Computed by `render_task_footer(&AppState.multiagent.tasks)`
    /// and rendered as the bottom-most row in `theme.dim`.
    pub task_footer: Option<String>,
    /// (M9-06) Team-status footer pill (claude-code `TeamStatus`):
    /// `{n} teammate[s]`, or `None` when there are no teammates (excluding
    /// `team-lead`). Computed by `render_team_footer(&AppState.multiagent.workers, false)`
    /// and rendered adjacent to the task footer in `theme.dim`.
    pub team_footer: Option<String>,
    /// (M9-06) Name of the teammate currently being viewed (`Some`) or `None`
    /// in normal mode. When `Some`, the `TeammateViewHeader` renders above the
    /// transcript in `theme.claude`.
    pub viewing_teammate: Option<String>,
    /// (`/color`) Session agent-color name (clone of `AppState.session_agent_color`,
    /// claude-code `useSwarmBanner` standalone branch — `standaloneAgentContext.color`).
    /// When `Some(name)`, a full-width colored rule line (`SessionColorBanner`)
    /// renders directly ABOVE the prompt, tinted by the agent color. `None`
    /// hides it (no row) — matching claude-code's `color: undefined` →
    /// standalone branch falls through to `return null`.
    pub session_agent_color: Option<String>,
    /// (A6) Resolved custom status-line text (clone of
    /// `AppState.status_line_text`). `Some(text)` makes the top `StatusLine`
    /// render the custom command's transformed stdout instead of the built-in
    /// `model cwd cost ctx% mode` row; `None` keeps the built-in row unchanged.
    pub status_line_text: Option<String>,
    /// (A6) Horizontal padding (cells) for the custom status row, read from
    /// `statusLine.padding` (default `0`). Forwarded to `StatusLine.padding_x`.
    pub status_line_padding: usize,
    /// (TokenWarning) The live context-pressure banner (clone of
    /// `AppState.context_pressure`), or `None` when the context is below the
    /// warning threshold. Rendered directly above the prompt as claude-code's
    /// `<TokenWarning>` line (`PromptInput/Notifications.tsx:321`).
    pub context_pressure: Option<traits::ContextPressureBanner>,
    /// (RRS-08) `Some(key)` while the idle Ctrl-C/Ctrl-D double-press exit
    /// window is armed (clone of `AppState.sigint_armed_at`/`_key`, already
    /// resolved against the window by the caller) — forwarded to
    /// `PromptInputFooter.exit_hint`.
    pub exit_hint: Option<&'static str>,
    /// (SS-06) `prefersReducedMotion` — forwarded to the streaming spinner so
    /// it pins its glyph and stops animating.
    pub reduced_motion: bool,
    /// (SS-08) The session's currently-active todo (clone of
    /// `AppState.current_todo`), forwarded to the streaming spinner so its
    /// verb reflects the in-progress task (`leaderVerb` order). `None` keeps
    /// the random pool verb.
    pub current_todo: Option<crate::state::CurrentTodo>,
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
            prompt_argument_hint: None,
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
            task_footer: None,
            team_footer: None,
            viewing_teammate: None,
            session_agent_color: None,
            status_line_text: None,
            status_line_padding: 0,
            context_pressure: None,
            exit_hint: None,
            reduced_motion: false,
            current_todo: None,
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
    let exit_hint = props.exit_hint;
    let reduced_motion = props.reduced_motion;
    let current_todo = props.current_todo.clone();
    let footer_model = (!model.trim().is_empty()).then_some(model.clone());
    // (A6) Custom status-line text + padding + width-for-truncation.
    let status_line_text = props.status_line_text.clone();
    // (TokenWarning) The live context-pressure banner, rendered above the prompt.
    let context_pressure = props.context_pressure.clone();
    let status_line_padding = props.status_line_padding;
    let status_line_width = props.prompt_width;
    let messages = props.messages.clone();
    let cache = props.cache.clone();
    let prompt_text = props.prompt_text.clone();
    let prompt_cursor = props.prompt_cursor;
    let prompt_width = props.prompt_width;
    let prompt_argument_hint = props.prompt_argument_hint.clone();
    let prompt_is_empty = props.prompt_text.is_empty();
    let scroll_offset = props.scroll_offset;
    let viewport_height = props.viewport_height;
    let show_spinner = props.show_spinner;
    let expanded = props.expanded.clone();
    let focused_tool_id = props.focused_tool_id.clone();
    let palette = props.palette.clone();
    let completion = props.completion.clone();
    let vim_enabled = props.vim_enabled;
    let vim_mode = props.vim_mode;
    let vim_visual_linewise = props.vim_visual_linewise;
    let history_search = props.history_search.clone();
    let theme = props.theme;
    let theme_name = props.theme_name;
    let task_footer = props.task_footer.clone();
    let team_footer = props.team_footer.clone();
    let viewing_teammate = props.viewing_teammate.clone();
    let session_agent_color = props.session_agent_color.clone();
    let dim = theme.dim;
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct, height: 100pct) {
            StatusLine(
                model: model,
                cwd: cwd,
                cost: cost,
                context_pct: context_pct,
                permission_mode: permission_mode,
                theme: theme,
                // (A6) When the user configured a `statusLine` command and it
                // produced output, render that custom text (padded + truncated)
                // in place of the built-in row; `None` keeps the built-in row.
                custom: status_line_text,
                padding_x: status_line_padding,
                width: status_line_width,
            )
            // (M9-06) Teammate-view header: renders above the transcript when a
            // teammate is being viewed. Hidden (no row) in normal mode.
            #(viewing_teammate.as_ref().map(|name| {
                let header = crate::components::coordinator::teammate_view_header::render_teammate_view_header(name, "");
                element! { View(flex_direction: FlexDirection::Column) { Text(content: header, color: theme.claude.to_iocraft()) } }
            }))
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
            #(if scroll_offset > 0 {
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: format!("Scrolled {scroll_offset} lines"), color: dim.to_iocraft())
                    }
                }.into_any()
            } else {
                element!(View).into_any()
            })
            #(if show_spinner {
                // Glyph + verb render in the active theme's Claude accent
                // (claude-code `Spinner` `defaultColor='claude'`).
                element!(SpinnerWithVerb(color: theme.claude.to_iocraft(), reduced_motion: reduced_motion, current_todo: current_todo.clone())).into_any()
            } else {
                element!(View).into_any()
            })
            // (M7-07) Active overlay (palette OR completion) renders as a
            // dropdown ABOVE the prompt row. iocraft 0.8 has no z-index, so
            // this draws inline in the bottom zone (design D4). Only one is
            // ever open at a time (the dispatcher enforces palette-wins-on-`/`).
            #(palette.as_ref().filter(|p| p.open).map(|p| {
                // (cp-08) The FULL filtered list (not just the visible window)
                // goes in — PaletteOverlay needs every row's name width to
                // compute the shared column width, then slices internally.
                let rows = p.rows();
                element! {
                    PaletteOverlay(rows: rows, selected: p.selected, theme: theme, width: prompt_width)
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
            // (TokenWarning) Context-pressure banner, drawn directly above the
            // prompt — the 1:1 of claude-code's `<TokenWarning>` mounted in
            // `PromptInput/Notifications.tsx:321`. Hidden (no row) when the
            // context is below the warning threshold (the component's `return
            // null`). `dimColor` for the auto-compact countdown, `warning`/`error`
            // for the "Context low" line (`TokenWarning.tsx:169`).
            #(context_pressure.as_ref().map(|b| {
                let text = b.text.clone();
                let color = match b.level {
                    traits::ContextPressureLevel::Dim => theme.dim,
                    traits::ContextPressureLevel::Warning => theme.warning,
                    traits::ContextPressureLevel::Error => theme.error,
                };
                element! {
                    View(flex_direction: FlexDirection::Row) {
                        Text(content: text, color: color.to_iocraft())
                    }
                }
            }))
            // (`/color`) Standalone-agent banner rule, drawn directly ABOVE the
            // prompt when a session color is set (claude-code `useSwarmBanner`
            // standalone branch + `PromptInput.tsx:2250-2267`). Hidden (no row)
            // when `None`, matching `color: undefined` → `return null`.
            #(session_agent_color.as_ref().map(|name| {
                let name = name.clone();
                element! {
                    SessionColorBanner(color_name: name, width: prompt_width)
                }
            }))
            // The input view is a SINGLE bordered box — the 1:1 of claude-code's
            // `PromptInput` `<Box borderStyle="round" borderLeft={false}
            // borderRight={false} borderBottom>`: top + bottom rules only (no
            // side edges → iocraft draws bare `─` lines, no corners), colored
            // `promptBorder` (= `theme.dim` == rgb(153,153,153), claude-code's
            // exact value). Drawn as ONE View so the box renders AND clears
            // atomically; hand-drawing two independent `Text("─")` rule rows let
            // the borders ghost / jump / vanish whenever the bottom zone's height
            // changed (palette open/close, wrap, hint toggle).
            View(
                border_style: BorderStyle::Round,
                border_color: theme.dim.to_iocraft(),
                border_edges: Edges::Top | Edges::Bottom,
                width: 100pct,
            ) {
                PromptInput(
                    text: prompt_text,
                    cursor: prompt_cursor,
                    width: prompt_width,
                    show_cursor: true,
                    argument_hint: prompt_argument_hint,
                )
            }
            PromptInputFooter(
                mode: crate::components::prompt_input::FooterMode::Prompt,
                placeholder: None,
                is_empty: prompt_is_empty,
                vim_enabled: vim_enabled,
                vim_mode: vim_mode,
                vim_visual_linewise: vim_visual_linewise,
                // (PIC-10) Surface the active permission mode in the footer
                // (the only place it shows after SS-01 removed the status line).
                permission_mode: permission_mode,
                // (PIC-07) A turn in flight swaps the hint to "esc to
                // interrupt" — same signal that gates the spinner row.
                is_loading: show_spinner,
                // (RRS-08) Replaces the whole footer-left with "Press {key}
                // again to exit" while the double-press window is armed.
                exit_hint: exit_hint,
                active_model: footer_model,
                width: Some(prompt_width),
            )
            // (M9-05) Background-task footer pill, drawn bottom-most when present
            // (claude-code `BackgroundTaskStatus`). Hidden (no row) when `None`.
            #(task_footer.map(|line| element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: line, color: dim.to_iocraft())
                }
            }))
            // (M9-06) Team-status footer pill (claude-code `TeamStatus`).
            // Hidden (no row) when `None` (no teammates / only `team-lead`).
            #(team_footer.map(|line| element! {
                View(flex_direction: FlexDirection::Row) {
                    Text(content: line, color: dim.to_iocraft())
                }
            }))
        }
    }
}
