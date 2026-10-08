//! The conversation transcript: committed history cells, the active
//! (in-flight) cell, and the native-scrollback commit cursor (plan Phase 3).
//!
//! Ports the codex `ChatWidget` transcript-state split (committed cells vs a
//! mutable active cell) onto `LingXi`'s data model. Committed cells are
//! inserted into the terminal's native scrollback exactly once, tracked by
//! [`Transcript::committed_to_terminal`]; the active cell is never committed
//! while streaming — it renders as the live tail
//! ([`Transcript::visible_live_tail`]) and only enters `committed` when
//! [`Transcript::flush_active`] finalizes it, so active text is never
//! double-rendered.

use std::cell::RefCell;
use std::io;
use std::io::Write;
use std::path::Path;

use ratatui::backend::Backend;
use ratatui::text::Line;
use tui_core::message::RenderedMessage;
use tui_core::theme::Theme;

use crate::history_cell::{cell_for_message, HistoryCell, RenderMode};

/// Committed history + active in-flight cell + native-scrollback commit
/// cursor + render mode (rich/raw and verbose/expanded state).
#[derive(Default)]
pub struct Transcript {
    turn_start: usize,
    response_start: usize,
    assistant_message_ids: std::collections::HashMap<usize, lingxi_core::types::MessageId>,
    request_ids: std::collections::HashMap<usize, String>,
    row_uuids: std::collections::HashMap<usize, Vec<String>>,
    row_tokens: std::collections::HashMap<usize, String>,
    terminal_replay_required: bool,
    /// Finalized cells, in commit order.
    committed: Vec<Box<dyn HistoryCell>>,
    /// The in-flight (actively streaming) cell, if any. Held out of
    /// `committed` so native-scrollback flushes never commit it early.
    active: Option<Box<dyn HistoryCell>>,
    active_request_id: Option<String>,
    active_row_uuids: Vec<String>,
    /// How many of `committed` are already inserted into the terminal's
    /// native scrollback (the commit cursor — cells before it are immutable
    /// terminal history).
    committed_to_terminal: usize,
    /// Rich-vs-raw + verbose/expanded state applied when rendering cells.
    render_mode: RenderMode,
    /// Cached wrap of committed cells for the last (width, mode, len). Full-screen
    /// redraws wrap every committed cell at 20 Hz; this skips that work when the
    /// committed history has not changed.
    wrap_cache: RefCell<Option<CommittedWrapCache>>,
}

struct CommittedWrapCache {
    /// Viewport width the cached lines were wrapped at.
    width: u16,
    /// Render mode (raw/verbose) used when wrapping.
    render_mode: RenderMode,
    /// Number of committed cells represented by `lines`.
    committed_len: usize,
    /// Palette used to produce the styled lines.
    theme: Theme,
    /// Wrapped committed lines.
    lines: Vec<Line<'static>>,
}

impl Transcript {
    /// Start collecting cells for the next assistant response.
    pub fn start_assistant_response(&mut self) {
        self.response_start = self.committed.len();
        self.turn_start = self.response_start;
    }

    /// Associate only narrative/reasoning cells with a completed response.
    pub fn identify_assistant_response(&mut self, id: lingxi_core::types::MessageId) {
        use crate::history_cell::message::{AssistantTextCell, RedactedThinkingCell, ThinkingCell};
        for index in self.response_start..self.committed.len() {
            let cell = self.committed[index].as_any();
            if cell.is::<AssistantTextCell>()
                || cell.is::<ThinkingCell>()
                || cell.is::<RedactedThinkingCell>()
            {
                self.assistant_message_ids.insert(index, id);
            }
        }
        self.response_start = self.committed.len();
    }

    /// Remove one identified response, preserving unrelated timeline cells.
    pub fn retract_assistant_response(&mut self, id: lingxi_core::types::MessageId) {
        let old_ids = std::mem::take(&mut self.assistant_message_ids);
        let old_request_ids = std::mem::take(&mut self.request_ids);
        let old_row_uuids = std::mem::take(&mut self.row_uuids);
        let old_row_tokens = std::mem::take(&mut self.row_tokens);
        let mut old_index = 0;
        let mut new_index = 0;
        let mut removed_before_start = 0;
        let mut removed_before_turn = 0;
        let mut removed = false;
        self.committed.retain(|_| {
            let message_id = old_ids.get(&old_index);
            let keep = message_id != Some(&id);
            if keep {
                if let Some(message_id) = message_id {
                    self.assistant_message_ids.insert(new_index, *message_id);
                }
                if let Some(request_id) = old_request_ids.get(&old_index) {
                    self.request_ids.insert(new_index, request_id.clone());
                }
                if let Some(row_uuids) = old_row_uuids.get(&old_index) {
                    self.row_uuids.insert(new_index, row_uuids.clone());
                }
                if let Some(row_token) = old_row_tokens.get(&old_index) {
                    self.row_tokens.insert(new_index, row_token.clone());
                }
                new_index += 1;
            } else {
                removed = true;
                removed_before_start += usize::from(old_index < self.response_start);
                removed_before_turn += usize::from(old_index < self.turn_start);
                self.terminal_replay_required |= old_index < self.committed_to_terminal;
            }
            old_index += 1;
            keep
        });
        self.response_start -= removed_before_start;
        self.turn_start -= removed_before_turn;
        if removed {
            self.committed_to_terminal = 0;
            self.terminal_replay_required = true;
            self.invalidate_wrap_cache();
        }
    }

    /// Whether native scrollback must be rebuilt after a retraction.
    pub fn take_terminal_replay_required(&mut self) -> bool {
        std::mem::take(&mut self.terminal_replay_required)
    }

    /// Current turn prose for background handoff after identity-based removal.
    pub fn current_turn_assistant_text(&self) -> String {
        use crate::history_cell::message::AssistantTextCell;
        self.committed[self.turn_start..]
            .iter()
            .chain(self.active.iter())
            .filter_map(|cell| cell.as_any().downcast_ref::<AssistantTextCell>())
            .map(AssistantTextCell::body)
            .collect()
    }

    /// An empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a transcript from an initial conversation: every message is
    /// committed (nothing is streaming yet).
    #[must_use]
    pub fn from_messages(messages: Vec<RenderedMessage>) -> Self {
        let mut transcript = Self::default();
        for message in messages {
            transcript.push_message(message);
        }
        transcript
    }

    /// Append a finalized cell to the committed history.
    pub fn push_committed(&mut self, cell: Box<dyn HistoryCell>) {
        self.committed.push(cell);
        self.invalidate_wrap_cache();
    }

    /// Commit a [`RenderedMessage`] via [`cell_for_message`] — the transcript
    /// owns the `RenderedMessage` → [`HistoryCell`] conversion (per-variant
    /// cells for the ported core variants, the adapter cell otherwise).
    pub fn push_message(&mut self, message: RenderedMessage) {
        if let RenderedMessage::IdentifiedTranscriptRow {
            message: inner,
            request_id,
        } = message
        {
            self.row_uuids
                .entry(self.committed.len())
                .or_default()
                .push(request_id.clone());
            if matches!(
                inner.as_ref(),
                RenderedMessage::UserText { .. } | RenderedMessage::AssistantText { .. }
            ) {
                self.request_ids.insert(self.committed.len(), request_id);
            }
            self.push_message(*inner);
            return;
        }
        if let RenderedMessage::AssistantToolUse { id, .. }
        | RenderedMessage::UserToolResult { id, .. } = &message
        {
            self.request_ids
                .insert(self.committed.len(), id.as_str().to_owned());
        }
        self.push_committed(cell_for_message(message));
    }

    /// Associate the most recently committed ordinary user row with a TUI
    /// correlation token. The token is not exposed as a Mods request id; only
    /// a later successful persistence event can attach the actual JSONL uuid.
    pub fn track_latest_user_row(&mut self, row_token: String) -> bool {
        use crate::history_cell::message::UserTextCell;
        let Some(index) = self.committed.len().checked_sub(1) else {
            return false;
        };
        if !self.committed[index].as_any().is::<UserTextCell>() {
            return false;
        }
        self.row_tokens.insert(index, row_token);
        true
    }

    /// Attach the UUID reported by the successful JSONL append to its source
    /// user row. A token that no longer has a visible row is ignored.
    pub fn identify_user_row(&mut self, row_token: &str, uuid: String) -> bool {
        let Some(index) = self
            .row_tokens
            .iter()
            .find_map(|(index, token)| (token == row_token).then_some(*index))
        else {
            return false;
        };
        self.request_ids.insert(index, uuid.clone());
        let row_uuids = self.row_uuids.entry(index).or_default();
        if !row_uuids.contains(&uuid) {
            row_uuids.push(uuid);
        }
        true
    }

    /// Attach the durable JSONL UUID to the active assistant row. It is kept
    /// separately from selection ids so a tombstone can remove the exact row
    /// without changing tool-use selection semantics.
    pub fn identify_active_row(&mut self, uuid: String) -> bool {
        if self.active.is_none() {
            return false;
        }
        self.active_request_id = Some(uuid.clone());
        self.identify_active_row_uuid(uuid)
    }

    /// Attach a durable UUID for display retraction without changing the
    /// selection identifier of an aggregate row such as a collapsed tool run.
    pub fn identify_active_row_uuid(&mut self, uuid: String) -> bool {
        if self.active.is_none() {
            return false;
        }
        if !self.active_row_uuids.contains(&uuid) {
            self.active_row_uuids.push(uuid);
        }
        true
    }

    /// Attach a durable UUID to the most recently committed row, for provider
    /// tool blocks whose row is created after their block-identity callback.
    pub fn identify_latest_committed_row(&mut self, uuid: String) -> bool {
        let Some(index) = self.committed.len().checked_sub(1) else {
            return false;
        };
        let row_uuids = self.row_uuids.entry(index).or_default();
        if !row_uuids.contains(&uuid) {
            row_uuids.push(uuid);
        }
        true
    }

    /// Remove visible cells only after every persisted row UUID backing each
    /// cell is superseded. This keeps aggregate collapsed-tool cells intact
    /// while the bridge reports their individual row tombstones.
    pub fn remove_transcript_rows(&mut self, uuids: &[String]) -> usize {
        if uuids.is_empty() {
            return 0;
        }
        let uuids = uuids
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut removed = 0;
        if self
            .active_row_uuids
            .iter()
            .all(|uuid| uuids.contains(uuid.as_str()))
            && !self.active_row_uuids.is_empty()
        {
            self.active = None;
            self.active_request_id = None;
            self.active_row_uuids.clear();
            removed += 1;
        }

        let old_ids = std::mem::take(&mut self.assistant_message_ids);
        let old_request_ids = std::mem::take(&mut self.request_ids);
        let old_row_uuids = std::mem::take(&mut self.row_uuids);
        let old_row_tokens = std::mem::take(&mut self.row_tokens);
        let mut retained = Vec::with_capacity(self.committed.len());
        let mut removed_before_start = 0;
        let mut removed_before_turn = 0;
        for (old_index, cell) in self.committed.drain(..).enumerate() {
            let remove = old_row_uuids.get(&old_index).is_some_and(|row_uuids| {
                !row_uuids.is_empty() && row_uuids.iter().all(|uuid| uuids.contains(uuid.as_str()))
            });
            if remove {
                removed += 1;
                removed_before_start += usize::from(old_index < self.response_start);
                removed_before_turn += usize::from(old_index < self.turn_start);
                self.terminal_replay_required |= old_index < self.committed_to_terminal;
                continue;
            }
            let new_index = retained.len();
            if let Some(message_id) = old_ids.get(&old_index) {
                self.assistant_message_ids.insert(new_index, *message_id);
            }
            if let Some(request_id) = old_request_ids.get(&old_index) {
                self.request_ids.insert(new_index, request_id.clone());
            }
            if let Some(row_uuids) = old_row_uuids.get(&old_index) {
                self.row_uuids.insert(new_index, row_uuids.clone());
            }
            if let Some(row_token) = old_row_tokens.get(&old_index) {
                self.row_tokens.insert(new_index, row_token.clone());
            }
            retained.push(cell);
        }
        self.committed = retained;
        self.response_start -= removed_before_start;
        self.turn_start -= removed_before_turn;
        if removed > 0 {
            self.committed_to_terminal = 0;
            self.terminal_replay_required = true;
            self.invalidate_wrap_cache();
        }
        removed
    }

    /// Attach successful per-block JSONL UUIDs to the ordinary assistant
    /// text rows from a response. The internal MessageId is only a grouping
    /// key; it is never exposed to selection as a request id. If the rendered
    /// row topology cannot be matched one-to-one, leave those rows unidentified.
    pub fn identify_assistant_text_rows(
        &mut self,
        message_id: lingxi_core::types::MessageId,
        uuids: &[Option<String>],
    ) -> bool {
        use crate::history_cell::message::AssistantTextCell;
        let mut indices = self
            .assistant_message_ids
            .iter()
            .filter_map(|(index, id)| {
                (*id == message_id && self.committed[*index].as_any().is::<AssistantTextCell>())
                    .then_some(*index)
            })
            .collect::<Vec<_>>();
        indices.sort_unstable();
        if indices.len() != uuids.len() {
            return false;
        }
        for (index, uuid) in indices.into_iter().zip(uuids) {
            if let Some(uuid) = uuid {
                self.request_ids.insert(index, uuid.clone());
                let row_uuids = self.row_uuids.entry(index).or_default();
                if !row_uuids.contains(uuid) {
                    row_uuids.push(uuid.clone());
                }
            }
        }
        true
    }

    /// Replace the active in-flight cell. Callers that must not lose a
    /// still-streaming predecessor call [`Self::flush_active`] first.
    pub fn set_active(&mut self, cell: Box<dyn HistoryCell>) {
        if self.active.is_none() {
            self.active_request_id = None;
            self.active_row_uuids.clear();
        }
        self.active = Some(cell);
    }

    /// Mutate the active cell in place (streaming deltas). Returns the
    /// closure's result, or `None` when no cell is active.
    pub fn mutate_active<R>(&mut self, f: impl FnOnce(&mut dyn HistoryCell) -> R) -> Option<R> {
        self.active.as_deref_mut().map(f)
    }

    /// Finalize the active cell: move it to the end of the committed history
    /// (no-op when idle). It becomes eligible for the next native-scrollback
    /// flush.
    pub fn flush_active(&mut self) {
        if let Some(cell) = self.active.take() {
            if let Some(request_id) = self.active_request_id.take() {
                self.request_ids.insert(self.committed.len(), request_id);
            }
            if !self.active_row_uuids.is_empty() {
                self.row_uuids.insert(
                    self.committed.len(),
                    std::mem::take(&mut self.active_row_uuids),
                );
            }
            self.committed.push(cell);
            self.invalidate_wrap_cache();
        }
    }

    /// Drop the active cell WITHOUT committing it. Used to discard the empty
    /// streaming placeholder [`TurnEvent::TurnStarted`] opens when a tool call
    /// (or thinking block) arrives before any assistant text streams — an
    /// empty `AssistantTextCell` would otherwise render a stray bare `●`
    /// marker with no body.
    pub fn discard_active(&mut self) {
        self.active = None;
        self.active_request_id = None;
        self.active_row_uuids.clear();
    }

    /// Insert every not-yet-committed finalized cell into the terminal's
    /// native scrollback (above the bottom viewport), advancing the commit
    /// cursor. Cells that render to no lines are consumed by the cursor
    /// without inserting. A cell contributing a raw escape block (an inline
    /// image) gets it emitted below its lines — rich mode only; raw mode is
    /// copy-friendly text. The active cell is never flushed here — it stays
    /// the live tail until [`Self::flush_active`].
    ///
    /// # Errors
    /// Propagates the first terminal IO error from the history insertion.
    pub fn flush_to_native_scrollback<B: Backend<Error = std::io::Error> + Write>(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
        width: u16,
        theme: &Theme,
    ) -> io::Result<()> {
        self.flush_to_native_scrollback_with_hyperlinks_and_cwd(terminal, width, theme, false, None)
    }

    /// Insert finalized cells into native scrollback, optionally wrapping
    /// visible markdown URLs and file attachment paths in OSC 8 links. The
    /// caller owns terminal capability detection; keeping that decision out of
    /// [`Transcript`] makes this state container deterministic in tests and
    /// leaves alternate-screen rendering escape-free.
    pub fn flush_to_native_scrollback_with_hyperlinks<
        B: Backend<Error = std::io::Error> + Write,
    >(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
        width: u16,
        theme: &Theme,
        hyperlinks_enabled: bool,
    ) -> io::Result<()> {
        self.flush_to_native_scrollback_with_hyperlinks_and_cwd(
            terminal,
            width,
            theme,
            hyperlinks_enabled,
            None,
        )
    }

    /// Variant of [`Self::flush_to_native_scrollback_with_hyperlinks`] that
    /// resolves relative attachment paths against the owning session's
    /// working directory. The process current directory is deliberately not
    /// consulted here because multiple embedded sessions may have different
    /// working directories.
    pub fn flush_to_native_scrollback_with_hyperlinks_and_cwd<
        B: Backend<Error = std::io::Error> + Write,
    >(
        &mut self,
        terminal: &mut crate::terminal::Terminal<B>,
        width: u16,
        theme: &Theme,
        hyperlinks_enabled: bool,
        hyperlink_cwd: Option<&Path>,
    ) -> io::Result<()> {
        while self.committed_to_terminal < self.committed.len() {
            let cell = &self.committed[self.committed_to_terminal];
            let lines = cell.scrollback_lines(
                width.max(1),
                theme,
                self.render_mode,
                hyperlinks_enabled,
                hyperlink_cwd,
            );
            let escape = if self.render_mode.raw {
                None
            } else {
                cell.scrollback_escape()
            };
            self.committed_to_terminal += 1;
            if !lines.is_empty() {
                terminal.insert_history_lines(&lines)?;
            }
            if let Some(escape) = escape {
                terminal.insert_history_image(escape.rows, &escape.escape)?;
            }
        }
        Ok(())
    }

    /// The live tail: what the active cell currently renders (empty when
    /// idle). This — not the committed flush — is how in-flight content
    /// becomes visible, so active text is never double-rendered. Lines are
    /// hard-wrapped to `width` so long streamed text is fully visible (and
    /// counted row-exactly for viewport sizing) instead of clipping at the
    /// right edge; on finalization the terminal wraps the committed content
    /// the same way (`insert_history_lines` row accounting).
    #[must_use]
    pub fn visible_live_tail(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        self.active.as_ref().map_or_else(Vec::new, |cell| {
            wrap_to_width(
                cell.display_lines(width.max(1), theme, self.render_mode),
                width,
            )
        })
    }

    /// Render the complete structured transcript for the alternate-screen
    /// surface.  Inline mode continues to commit finalized cells into native
    /// scrollback; full-screen mode has no native scrollback and therefore
    /// redraws the committed cells plus active tail from this immutable view.
    #[must_use]
    pub fn visible_fullscreen_lines(&self, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        self.visible_fullscreen_lines_with_hyperlinks(width, theme, false, None)
    }

    /// Render the complete transcript for the alternate-screen surface,
    /// optionally carrying OSC 8 metadata for URLs and file attachments.
    ///
    /// The plain path retains the cached cell-grid lines. Linked lines are
    /// rebuilt from the cells on each call because the owning session cwd can
    /// change after `/cd`; caching those lines would retain stale file targets.
    #[must_use]
    pub fn visible_fullscreen_lines_with_hyperlinks(
        &self,
        width: u16,
        theme: &Theme,
        hyperlinks_enabled: bool,
        hyperlink_cwd: Option<&Path>,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        if hyperlinks_enabled {
            let mut lines = Vec::new();
            for cell in &self.committed {
                lines.extend(wrap_to_width(
                    cell.scrollback_lines(width, theme, self.render_mode, true, hyperlink_cwd),
                    width,
                ));
            }
            if let Some(active) = &self.active {
                lines.extend(wrap_to_width(
                    active.scrollback_lines(width, theme, self.render_mode, true, hyperlink_cwd),
                    width,
                ));
            }
            return lines;
        }

        let committed_len = self.committed.len();
        let mut cache = self.wrap_cache.borrow_mut();
        let hit = cache.as_ref().is_some_and(|c| {
            c.width == width
                && c.render_mode == self.render_mode
                && c.committed_len == committed_len
                && c.theme == *theme
        });
        let mut lines = if hit {
            cache.as_ref().expect("checked").lines.clone()
        } else {
            let mut wrapped = Vec::new();
            for cell in &self.committed {
                wrapped.extend(wrap_to_width(
                    cell.display_lines(width, theme, self.render_mode),
                    width,
                ));
            }
            *cache = Some(CommittedWrapCache {
                width,
                render_mode: self.render_mode,
                committed_len,
                theme: *theme,
                lines: wrapped.clone(),
            });
            wrapped
        };
        drop(cache);
        if let Some(active) = &self.active {
            lines.extend(wrap_to_width(
                active.display_lines(width, theme, self.render_mode),
                width,
            ));
        }
        lines
    }

    /// Identify a rendered transcript entry only when it contains the whole
    /// selection. The caller supplies rows relative to the visible history
    /// viewport, after the same tail clipping used by fullscreen rendering.
    #[must_use]
    pub fn request_id_for_fullscreen_rows(
        &self,
        start_row: usize,
        end_row: usize,
        viewport_height: usize,
        width: u16,
        theme: &Theme,
        hyperlinks_enabled: bool,
        hyperlink_cwd: Option<&Path>,
    ) -> Option<&str> {
        if start_row > end_row || end_row >= viewport_height {
            return None;
        }
        let width = width.max(1);
        let heights = self
            .committed
            .iter()
            .map(|cell| {
                let lines = if hyperlinks_enabled {
                    cell.scrollback_lines(width, theme, self.render_mode, true, hyperlink_cwd)
                } else {
                    cell.display_lines(width, theme, self.render_mode)
                };
                wrap_to_width(lines, width).len()
            })
            .collect::<Vec<_>>();
        let active_height = self.active.as_ref().map_or(0, |cell| {
            let lines = if hyperlinks_enabled {
                cell.scrollback_lines(width, theme, self.render_mode, true, hyperlink_cwd)
            } else {
                cell.display_lines(width, theme, self.render_mode)
            };
            wrap_to_width(lines, width).len()
        });
        let total = heights.iter().sum::<usize>() + active_height;
        let skip = total.saturating_sub(viewport_height);
        let absolute_start = skip + start_row;
        let absolute_end = skip + end_row;
        let mut first = 0;
        for (index, height) in heights.into_iter().enumerate() {
            let last = first + height;
            if height > 0 && first <= absolute_start && absolute_end < last {
                return self.request_ids.get(&index).map(String::as_str);
            }
            first = last;
        }
        None
    }

    /// Drop all transcript state: committed cells, the active cell, and the
    /// native-scrollback commit cursor (`/clear`).
    pub fn clear(&mut self) {
        self.turn_start = 0;
        self.response_start = 0;
        self.assistant_message_ids.clear();
        self.request_ids.clear();
        self.row_uuids.clear();
        self.row_tokens.clear();
        self.committed.clear();
        self.active = None;
        self.active_request_id = None;
        self.active_row_uuids.clear();
        self.committed_to_terminal = 0;
        self.invalidate_wrap_cache();
    }

    /// The committed cells, in commit order (the active cell is excluded).
    #[must_use]
    pub fn committed_cells(&self) -> &[Box<dyn HistoryCell>] {
        &self.committed
    }

    /// The active in-flight cell, if any.
    #[must_use]
    pub fn active_cell(&self) -> Option<&dyn HistoryCell> {
        self.active.as_deref()
    }

    /// The native-scrollback commit cursor: how many committed cells are
    /// already inserted into the terminal.
    #[must_use]
    pub fn committed_to_terminal(&self) -> usize {
        self.committed_to_terminal
    }

    /// Mark every finalized cell as needing to be emitted again.
    ///
    /// A detached PTY has no terminal scrollback to preserve for the next
    /// controller. Reattach therefore clears the new terminal and asks the
    /// live widget to rebuild native scrollback from its structured cells.
    pub fn reset_terminal_commit(&mut self) {
        self.committed_to_terminal = 0;
    }

    /// `true` when the transcript holds nothing (no committed cells and no
    /// active cell).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed.is_empty() && self.active.is_none()
    }

    /// The render mode applied to cell rendering.
    #[must_use]
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Replace the render mode (rich/raw + verbose).
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
        self.invalidate_wrap_cache();
    }

    /// Whether collapsible content renders expanded (Ctrl-O state).
    #[must_use]
    pub fn verbose(&self) -> bool {
        self.render_mode.verbose
    }

    /// Set the verbose/expanded state.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.render_mode.verbose = verbose;
        self.invalidate_wrap_cache();
    }

    /// Flip the verbose/expanded state; returns the new value.
    pub fn toggle_verbose(&mut self) -> bool {
        self.render_mode.verbose = !self.render_mode.verbose;
        self.invalidate_wrap_cache();
        self.render_mode.verbose
    }

    fn invalidate_wrap_cache(&mut self) {
        *self.wrap_cache.get_mut() = None;
    }
}

/// Hard-wrap `lines` at `width` display columns, preserving span styles and
/// never splitting a wide (2-column) glyph across rows. This mirrors how the
/// terminal itself wraps committed history on flush (character wrap, and the
/// same `div_ceil` row count [`crate::terminal::Terminal::insert_history_lines`]
/// budgets), so the live tail shows — and is sized for — every streamed
/// column instead of clipping at the right edge (plan Phase 13 layout fix).
fn wrap_to_width(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    use ratatui::text::Span;
    use unicode_width::UnicodeWidthChar;
    let max = usize::from(width.max(1));
    let mut out = Vec::new();
    for line in lines {
        let (style, alignment) = (line.style, line.alignment);
        let mut row: Vec<Span<'static>> = Vec::new();
        let mut row_width = 0usize;
        let mut active_target = None;
        for span in line.spans {
            let span_style = span.style;
            let mut chunk = String::new();
            let mut cursor = 0;
            while cursor < span.content.len() {
                if let Some((next, next_target)) =
                    crate::render::osc8_control_at(&span.content, cursor)
                {
                    chunk.push_str(&span.content[cursor..next]);
                    active_target = next_target;
                    cursor = next;
                    continue;
                }
                let Some(ch) = span.content[cursor..].chars().next() else {
                    break;
                };
                let next = cursor + ch.len_utf8();
                let ch_width = ch.width().unwrap_or(0);
                if row_width + ch_width > max && row_width > 0 {
                    if !chunk.is_empty() {
                        row.push(Span::styled(std::mem::take(&mut chunk), span_style));
                    }
                    if active_target.is_some() {
                        if let Some(last) = row.last_mut() {
                            last.content.to_mut().push_str("\x1b]8;;\x07");
                        } else {
                            chunk.push_str("\x1b]8;;\x07");
                        }
                    }
                    let mut wrapped = Line::from(std::mem::take(&mut row));
                    wrapped.style = style;
                    wrapped.alignment = alignment;
                    out.push(wrapped);
                    row_width = 0;
                    if let Some(target) = active_target.as_deref() {
                        chunk.push_str(&format!("\x1b]8;;{target}\x07"));
                    }
                }
                chunk.push(ch);
                row_width += ch_width;
                cursor = next;
            }
            if !chunk.is_empty() {
                row.push(Span::styled(chunk, span_style));
            }
        }
        let mut wrapped = Line::from(row);
        wrapped.style = style;
        wrapped.alignment = alignment;
        out.push(wrapped);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::history_cell::message::AssistantTextCell;
    use crate::terminal::test_support::TestWriteBackend;
    use crate::terminal::Terminal;
    use unicode_width::UnicodeWidthChar;

    /// An 80x24 bottom-anchored test terminal with a 4-row viewport plus the
    /// raw escape-byte capture handle (history inserts are raw writes).
    fn test_terminal() -> (Terminal<TestWriteBackend>, Rc<RefCell<Vec<u8>>>) {
        let backend = TestWriteBackend::new(80, 24);
        let raw = backend.raw_handle();
        let mut terminal = Terminal::with_options(backend).expect("test terminal");
        terminal.set_bottom_viewport_height(4).expect("viewport");
        // Drop the setup escapes so tests only see flush output.
        raw.borrow_mut().clear();
        (terminal, raw)
    }

    fn raw_string(raw: &Rc<RefCell<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&raw.borrow()).into_owned()
    }

    fn system(body: &str) -> RenderedMessage {
        RenderedMessage::SystemText {
            body: body.to_string(),
            timestamp: 0,
            is_error: false,
        }
    }

    fn assistant_cell(body: &str) -> Box<dyn HistoryCell> {
        Box::new(AssistantTextCell::new(body.to_string()))
    }

    #[test]
    fn fullscreen_selection_request_id_requires_one_identified_entry() {
        let mut transcript = Transcript::new();
        let tool_id = lingxi_core::types::ToolUseId::from("toolu_selection");
        transcript.push_message(system("header"));
        transcript.push_message(RenderedMessage::AssistantToolUse {
            id: tool_id.clone(),
            tool: "Read".into(),
            input: serde_json::json!({"file_path":"/tmp/example"}),
        });
        transcript.push_message(system("footer"));
        let theme = Theme::dark();
        let heights = transcript
            .committed_cells()
            .iter()
            .map(|cell| {
                wrap_to_width(cell.display_lines(80, &theme, transcript.render_mode()), 80).len()
            })
            .collect::<Vec<_>>();
        let total = heights.iter().sum::<usize>();
        let request = |first, last, viewport| {
            transcript
                .request_id_for_fullscreen_rows(first, last, viewport, 80, &theme, false, None)
        };
        assert_eq!(
            request(heights[0], heights[0] + heights[1] - 1, total),
            Some(tool_id.as_str())
        );
        assert_eq!(request(heights[0] - 1, heights[0], total), None);
        assert_eq!(request(heights[0], heights[0] + heights[1], total), None);
        assert_eq!(request(0, 0, total), None);
        assert_eq!(request(0, 0, total - heights[0]), Some(tool_id.as_str()));
    }

    #[test]
    fn fullscreen_selection_uses_replayed_message_uuid_and_drops_it_on_clear() {
        let mut transcript = Transcript::new();
        transcript.push_message(RenderedMessage::IdentifiedTranscriptRow {
            message: Box::new(RenderedMessage::UserText {
                body: "replayed prompt".into(),
                timestamp: 0,
            }),
            request_id: "transcript-uuid".into(),
        });
        let theme = Theme::dark();
        assert_eq!(
            transcript.request_id_for_fullscreen_rows(0, 0, 20, 80, &theme, false, None),
            Some("transcript-uuid")
        );
        transcript.clear();
        assert_eq!(
            transcript.request_id_for_fullscreen_rows(0, 0, 20, 80, &theme, false, None),
            None
        );
    }

    #[test]
    fn fullscreen_selection_uses_persisted_user_uuid_after_row_token_resolution() {
        let mut transcript = Transcript::new();
        transcript.push_message(RenderedMessage::UserText {
            body: "live prompt".into(),
            timestamp: 0,
        });
        assert!(transcript.track_latest_user_row("ui-row-token".into()));
        let theme = Theme::dark();
        let selection_id = |transcript: &Transcript| {
            transcript
                .request_id_for_fullscreen_rows(0, 0, 20, 80, &theme, false, None)
                .map(str::to_owned)
        };
        assert_eq!(selection_id(&transcript), None);

        let persisted_uuid = "83f67a72-a806-49b7-9a18-e57307177a86";
        assert!(transcript.identify_user_row("ui-row-token", persisted_uuid.into()));
        assert_eq!(selection_id(&transcript).as_deref(), Some(persisted_uuid));
        assert_ne!(selection_id(&transcript).as_deref(), Some("ui-row-token"));
    }

    #[test]
    fn fullscreen_selection_uses_persisted_assistant_uuid_not_message_id() {
        let mut transcript = Transcript::new();
        transcript.start_assistant_response();
        transcript.set_active(assistant_cell("live answer"));
        transcript.flush_active();
        let message_id = lingxi_core::types::MessageId::new();
        transcript.identify_assistant_response(message_id);

        let persisted_uuid = "a3566d96-b8e6-4a19-a063-6c9539c9e26d".to_string();
        assert!(
            transcript.identify_assistant_text_rows(message_id, &[Some(persisted_uuid.clone())])
        );
        let theme = Theme::dark();
        assert_eq!(
            transcript.request_id_for_fullscreen_rows(0, 0, 20, 80, &theme, false, None),
            Some(persisted_uuid.as_str())
        );
        assert_ne!(persisted_uuid, message_id.to_string());
    }

    #[test]
    fn assistant_uuid_assignment_requires_matching_text_row_count() {
        let mut transcript = Transcript::new();
        transcript.start_assistant_response();
        transcript.set_active(assistant_cell("first"));
        transcript.flush_active();
        transcript.set_active(assistant_cell("second"));
        transcript.flush_active();
        let message_id = lingxi_core::types::MessageId::new();
        transcript.identify_assistant_response(message_id);

        assert!(!transcript.identify_assistant_text_rows(
            message_id,
            &[Some("83f67a72-a806-49b7-9a18-e57307177a86".into())]
        ));
        let theme = Theme::dark();
        assert_eq!(
            transcript.request_id_for_fullscreen_rows(0, 0, 20, 80, &theme, false, None),
            None
        );
    }

    #[test]
    fn retracts_only_identified_assistant_cells_and_preserves_later_response() {
        let mut transcript = Transcript::new();
        let failed = lingxi_core::types::MessageId::new();
        let retained = lingxi_core::types::MessageId::new();
        transcript.push_message(system("before"));
        transcript.start_assistant_response();
        transcript.push_committed(assistant_cell("failed"));
        transcript.push_message(system("unrelated notice"));
        transcript.identify_assistant_response(failed);
        transcript.push_committed(assistant_cell("retained"));
        transcript.identify_assistant_response(retained);
        transcript.committed_to_terminal = transcript.committed.len();
        transcript.retract_assistant_response(lingxi_core::types::MessageId::new());
        assert_eq!(transcript.committed.len(), 4);
        transcript.retract_assistant_response(failed);
        assert_eq!(transcript.committed.len(), 3);
        assert!(transcript.take_terminal_replay_required());
        assert_eq!(transcript.committed_to_terminal(), 0);
        assert_eq!(
            transcript.committed[2]
                .as_any()
                .downcast_ref::<AssistantTextCell>()
                .unwrap()
                .body(),
            "retained"
        );
        transcript.retract_assistant_response(failed);
        assert_eq!(transcript.committed.len(), 3);
        transcript.retract_assistant_response(retained);
        assert_eq!(transcript.committed.len(), 2);
    }

    #[test]
    fn server_fallback_row_uuid_survives_flush_and_removes_only_its_cell() {
        let mut transcript = Transcript::new();
        transcript.start_assistant_response();
        transcript.set_active(assistant_cell("discarded refusal"));
        let row_uuid = "83f67a72-a806-49b7-9a18-e57307177a86".to_string();
        assert!(transcript.identify_active_row(row_uuid.clone()));
        transcript.flush_active();
        transcript.push_message(system("unrelated notice"));
        transcript.committed_to_terminal = transcript.committed.len();

        assert_eq!(transcript.row_uuids.get(&0), Some(&vec![row_uuid.clone()]));
        assert_eq!(transcript.request_ids.get(&0), Some(&row_uuid));
        assert_eq!(
            transcript.remove_transcript_rows(std::slice::from_ref(&row_uuid)),
            1
        );
        assert_eq!(transcript.committed.len(), 1);
        assert!(transcript.committed[0]
            .as_any()
            .is::<crate::history_cell::system::SystemTextCell>());
        assert!(transcript.take_terminal_replay_required());
        assert_eq!(transcript.committed_to_terminal(), 0);
    }

    #[test]
    fn server_fallback_row_uuid_can_remove_the_current_active_cell() {
        let mut transcript = Transcript::new();
        transcript.set_active(assistant_cell("live refusal"));
        let row_uuid = "a3566d96-b8e6-4a19-a063-6c9539c9e26d".to_string();
        assert!(transcript.identify_active_row(row_uuid.clone()));

        assert_eq!(transcript.remove_transcript_rows(&[row_uuid]), 1);
        assert!(transcript.is_empty());
    }

    #[test]
    fn aggregate_row_waits_until_all_member_uuids_are_tombstoned() {
        let mut transcript = Transcript::new();
        transcript.set_active(assistant_cell("collapsed tool run"));
        // A collapsed progress cell can represent several persisted rows. The
        // Native fallback tombstone carries each actual outer message UUID,
        // not the aggregate's tool id or its current screen position.
        let first_row_uuid = "5ad9fc2b-7172-4b95-9db2-6f0e8c1d7a0a";
        let second_row_uuid = "b19f8c0e-40e2-49ab-9fd0-9c3f2d6a710e";
        assert!(transcript.identify_active_row_uuid(first_row_uuid.into()));
        assert!(transcript.identify_active_row_uuid(second_row_uuid.into()));
        transcript.flush_active();

        assert_eq!(transcript.remove_transcript_rows(&[first_row_uuid.into()]), 0);
        assert_eq!(transcript.committed.len(), 1);
        assert_eq!(
            transcript.remove_transcript_rows(&[first_row_uuid.into(), second_row_uuid.into()]),
            1
        );
        assert!(transcript.committed.is_empty());
    }

    /// Append `delta` to the active assistant cell; `false` when the active
    /// cell is missing or not assistant text (mirrors the app's delta path).
    fn append_delta(transcript: &mut Transcript, delta: &str) -> bool {
        transcript
            .mutate_active(|cell| {
                if let Some(assistant) = cell.as_any_mut().downcast_mut::<AssistantTextCell>() {
                    assistant.append(delta);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    #[test]
    fn flush_commits_cells_in_push_order() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("first-line-alpha"));
        transcript.push_message(system("second-line-beta"));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
        let out = raw_string(&raw);
        let first = out.find("first-line-alpha").expect("first committed");
        let second = out.find("second-line-beta").expect("second committed");
        assert!(first < second, "commit order preserved:\n{out}");
    }

    #[test]
    fn commit_cursor_only_flushes_new_cells_and_never_reflushes() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("early-cell"));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 1);
        transcript.push_message(system("late-cell"));
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
        let out = raw_string(&raw);
        assert_eq!(
            out.matches("early-cell").count(),
            1,
            "already-committed cells are never re-inserted:\n{out}"
        );
        assert_eq!(out.matches("late-cell").count(), 1);
    }

    #[test]
    fn invisible_cells_advance_the_cursor_without_inserting() {
        let mut transcript = Transcript::new();
        // Empty user text renders to zero lines.
        transcript.push_message(RenderedMessage::UserText {
            body: String::new(),
            timestamp: 0,
        });
        transcript.push_message(system("visible"));
        let (mut terminal, _raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 2);
    }

    #[test]
    fn active_cell_mutates_in_place_and_shows_in_live_tail() {
        let mut transcript = Transcript::new();
        transcript.set_active(assistant_cell("Hel"));
        assert!(append_delta(&mut transcript, "lo"));
        let tail = transcript.visible_live_tail(80, &Theme::dark());
        let text: String = tail.iter().map(ToString::to_string).collect();
        assert!(text.contains("Hello"), "live tail shows the delta: {text}");
        assert!(
            transcript.committed_cells().is_empty(),
            "mutation never commits"
        );
    }

    #[test]
    fn mutate_active_returns_none_when_idle() {
        let mut transcript = Transcript::new();
        assert_eq!(transcript.mutate_active(|_| 42), None);
        assert!(!append_delta(&mut transcript, "x"));
        assert!(transcript.visible_live_tail(80, &Theme::dark()).is_empty());
    }

    #[test]
    fn flush_active_moves_active_to_committed_end() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("older"));
        transcript.set_active(assistant_cell("streamed"));
        transcript.flush_active();
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_cells().len(), 2);
        let last = transcript.committed_cells()[1]
            .as_any()
            .downcast_ref::<AssistantTextCell>()
            .expect("assistant cell");
        assert_eq!(last.body(), "streamed");
        // Flushing again is a no-op.
        transcript.flush_active();
        assert_eq!(transcript.committed_cells().len(), 2);
    }

    #[test]
    fn live_tail_and_committed_flush_never_double_render_active_text() {
        let mut transcript = Transcript::new();
        transcript.push_message(system("finalized-text"));
        transcript.set_active(assistant_cell("streaming-tail-text"));
        let (mut terminal, raw) = test_terminal();
        let theme = Theme::dark();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &theme)
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("finalized-text"), "committed flushed:\n{out}");
        assert!(
            !out.contains("streaming-tail-text"),
            "active text must NOT be committed while streaming:\n{out}"
        );
        let tail: String = transcript
            .visible_live_tail(80, &theme)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(tail.contains("streaming-tail-text"), "got: {tail}");

        // Once finalized, the text commits exactly once and leaves the tail.
        transcript.flush_active();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &theme)
            .unwrap();
        let out = raw_string(&raw);
        assert_eq!(out.matches("streaming-tail-text").count(), 1);
        assert!(transcript.visible_live_tail(80, &theme).is_empty());
    }

    #[test]
    fn clear_resets_cells_active_and_commit_cursor() {
        let mut transcript = Transcript::from_messages(vec![system("a"), system("b")]);
        let (mut terminal, _raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        transcript.set_active(assistant_cell("mid-stream"));
        assert_eq!(transcript.committed_to_terminal(), 2);
        transcript.clear();
        assert!(transcript.is_empty());
        assert!(transcript.committed_cells().is_empty());
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_to_terminal(), 0);
    }

    #[test]
    fn verbose_state_expands_collapsible_cells_at_flush_time() {
        let thinking = RenderedMessage::AssistantThinking {
            thinking: "hidden reasoning body".to_string(),
            expanded: false,
        };
        // Collapsed (default): the placeholder commits, not the body.
        let mut collapsed = Transcript::new();
        collapsed.push_message(thinking.clone());
        let (mut terminal, raw) = test_terminal();
        collapsed
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("ctrl+o to expand"), "collapsed:\n{out}");
        assert!(!out.contains("hidden reasoning body"), "collapsed:\n{out}");

        // Verbose: the body commits.
        let mut expanded = Transcript::new();
        assert!(expanded.toggle_verbose(), "toggle returns the new state");
        assert!(expanded.verbose());
        expanded.push_message(thinking);
        let (mut terminal, raw) = test_terminal();
        expanded
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("hidden reasoning body"), "expanded:\n{out}");
    }

    #[test]
    fn render_mode_raw_flushes_plain_lines() {
        let mut transcript = Transcript::new();
        transcript.set_render_mode(RenderMode {
            raw: true,
            verbose: false,
        });
        assert!(transcript.render_mode().raw);
        transcript.push_message(RenderedMessage::AssistantText {
            body: "plain-raw-body".to_string(),
            timestamp: 0,
        });
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        assert_eq!(transcript.committed_to_terminal(), 1);
        assert!(raw_string(&raw).contains("plain-raw-body"));
    }

    #[test]
    fn native_scrollback_emits_osc8_for_markdown_urls_and_file_attachments() {
        let mut transcript = Transcript::new();
        transcript.push_message(RenderedMessage::AssistantText {
            body: "See [docs](https://example.com/docs) or <https://example.com/>.".to_string(),
            timestamp: 0,
        });
        transcript.push_message(RenderedMessage::Attachment {
            attachment: tui_core::message::Attachment::File {
                display_path: "src/report.txt".to_string(),
                num_lines: 12,
                truncated: false,
            },
        });
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback_with_hyperlinks_and_cwd(
                &mut terminal,
                80,
                &Theme::dark(),
                true,
                Some(std::path::Path::new("/workspace/project")),
            )
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains(&tui_core::render::osc8::hyperlink(
            "docs",
            "https://example.com/docs",
        )));
        assert!(out.contains(&tui_core::render::osc8::hyperlink(
            "https://example.com/",
            "https://example.com/",
        )));
        assert!(out.contains(&tui_core::render::osc8::file_link(
            "/workspace/project/src/report.txt",
        )));
        assert_eq!(
            out.matches("\x1b]8;;").count(),
            6,
            "open + close per link: {out:?}"
        );
    }

    #[test]
    fn fullscreen_lines_emit_osc8_for_urls_and_relative_files_when_enabled() {
        let transcript = Transcript::from_messages(vec![
            RenderedMessage::AssistantText {
                body: "See [docs](https://example.com/docs).".to_string(),
                timestamp: 0,
            },
            RenderedMessage::Attachment {
                attachment: tui_core::message::Attachment::File {
                    display_path: "src/report.txt".to_string(),
                    num_lines: 1,
                    truncated: false,
                },
            },
        ]);
        let lines = transcript.visible_fullscreen_lines_with_hyperlinks(
            80,
            &Theme::dark(),
            true,
            Some(std::path::Path::new("/workspace/project")),
        );
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains(&tui_core::render::osc8::hyperlink(
            "docs",
            "https://example.com/docs",
        )));
        assert!(rendered.contains(&tui_core::render::osc8::file_link(
            "/workspace/project/src/report.txt",
        )));

        let plain = transcript.visible_fullscreen_lines(80, &Theme::dark());
        let plain_rendered = plain
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(!plain_rendered.contains("\x1b]8;;"));
        assert!(plain_rendered.contains("docs"));
    }

    #[test]
    fn fullscreen_linked_lines_reopen_links_when_wrapping() {
        let transcript = Transcript::from_messages(vec![RenderedMessage::AssistantText {
            body: "[abcdefgh](https://example.com/long)".to_string(),
            timestamp: 0,
        }]);
        let lines =
            transcript.visible_fullscreen_lines_with_hyperlinks(4, &Theme::dark(), true, None);
        assert!(lines.len() >= 3, "marker plus wrapped label: {lines:?}");
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("\x1b]8;;https://example.com/long\x07"));
        assert!(rendered.contains("\x1b]8;;\x07"));
        fn visible_width(line: &Line<'_>) -> usize {
            line.spans
                .iter()
                .map(|span| {
                    let mut width = 0;
                    let mut cursor = 0;
                    while cursor < span.content.len() {
                        if let Some((next, _)) =
                            crate::render::osc8_control_at(&span.content, cursor)
                        {
                            cursor = next;
                            continue;
                        }
                        let Some(ch) = span.content[cursor..].chars().next() else {
                            break;
                        };
                        cursor += ch.len_utf8();
                        width += ch.width().unwrap_or(0);
                    }
                    width
                })
                .sum()
        }
        assert!(
            lines.iter().all(|line| visible_width(line) <= 4),
            "visible lines must remain wrapped to width: {lines:?}"
        );
    }

    #[test]
    fn native_scrollback_keeps_urls_and_file_attachments_plain_when_disabled() {
        let mut transcript = Transcript::new();
        transcript.push_message(RenderedMessage::AssistantText {
            body: "See [docs](https://example.com/docs) or <https://example.com/>.".to_string(),
            timestamp: 0,
        });
        transcript.push_message(RenderedMessage::Attachment {
            attachment: tui_core::message::Attachment::File {
                display_path: "src/report.txt".to_string(),
                num_lines: 12,
                truncated: false,
            },
        });
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback_with_hyperlinks(&mut terminal, 80, &Theme::dark(), false)
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("docs"));
        assert!(out.contains("https://example.com/"));
        assert!(out.contains("src/report.txt"));
        assert!(
            !out.contains("\x1b]8;;"),
            "unsupported terminals stay plain: {out:?}"
        );
    }

    #[test]
    fn from_messages_seeds_committed_only() {
        let transcript = Transcript::from_messages(vec![system("seeded")]);
        assert_eq!(transcript.committed_cells().len(), 1);
        assert!(transcript.active_cell().is_none());
        assert_eq!(transcript.committed_to_terminal(), 0, "nothing flushed yet");
        assert!(!transcript.is_empty());
    }

    /// An image cell on a graphics-capable terminal: the text fallback line
    /// commits first, the inline-image escape follows below it. Raw render
    /// mode (copy-friendly) suppresses the escape but keeps the fallback.
    #[test]
    fn image_cell_flush_emits_text_fallback_then_inline_escape() {
        use crate::history_cell::attachments::UserImageCell;
        use crate::term_image::ImageProtocol;

        let png = std::env::temp_dir().join(format!(
            "tui-rata-transcript-img-{}.png",
            std::process::id()
        ));
        image::RgbaImage::new(4, 20).save(&png).expect("test png");

        let cell = |protocol| {
            Box::new(UserImageCell::with_protocol(
                Some(7),
                None,
                Some(png.display().to_string()),
                protocol,
            ))
        };

        // Rich mode + kitty: fallback line then the escape.
        let mut transcript = Transcript::new();
        transcript.push_committed(cell(ImageProtocol::Kitty));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        let fallback = out.find("[Image #7]").expect("text fallback committed");
        let escape = out.find("\x1b_Ga=T,f=100,r=2,").expect("kitty escape");
        assert!(fallback < escape, "fallback precedes the image:\n{out:?}");
        assert_eq!(transcript.committed_to_terminal(), 1);

        // Raw mode: fallback only, no escape.
        let mut transcript = Transcript::new();
        transcript.set_render_mode(RenderMode {
            raw: true,
            verbose: false,
        });
        transcript.push_committed(cell(ImageProtocol::Kitty));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("[Image #7]"), "fallback stays in raw mode");
        assert!(!out.contains("\x1b_G"), "no escape in raw mode:\n{out:?}");

        // No graphics support: fallback only.
        let mut transcript = Transcript::new();
        transcript.push_committed(cell(ImageProtocol::None));
        let (mut terminal, raw) = test_terminal();
        transcript
            .flush_to_native_scrollback(&mut terminal, 80, &Theme::dark())
            .unwrap();
        let out = raw_string(&raw);
        assert!(out.contains("[Image #7]"));
        assert!(!out.contains("\x1b_G"));

        std::fs::remove_file(&png).ok();
    }

    // ===== Plan Phase 13: live-tail hard wrap =====

    #[test]
    fn live_tail_hard_wraps_long_streamed_lines_to_the_viewport_width() {
        let mut transcript = Transcript::new();
        let mut cell = AssistantTextCell::new(String::new());
        // One long markdown paragraph: 10 x 8 = 80 chars + the 2-column
        // marker = 82 columns.
        cell.append(&"abcdefgh".repeat(10));
        transcript.set_active(Box::new(cell));
        let tail = transcript.visible_live_tail(40, &Theme::dark());
        // 82 columns at width 40 = 3 rows — the same row count the terminal
        // budgets when this line is later flushed (div_ceil accounting).
        assert_eq!(tail.len(), 3, "82 columns / 40 = 3 rows");
        assert!(
            tail.iter().all(|line| line.width() <= 40),
            "every wrapped row fits the width: {:?}",
            tail.iter()
                .map(ratatui::text::Line::width)
                .collect::<Vec<_>>()
        );
        // Nothing is lost to clipping: the rows concatenate back to the text.
        let joined: String = tail
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            joined,
            format!(
                "{}{}",
                crate::history_cell::message::ASSISTANT_MARKER,
                "abcdefgh".repeat(10)
            )
        );
    }

    #[test]
    fn live_tail_wrap_never_splits_wide_glyphs() {
        let mut transcript = Transcript::new();
        let mut cell = AssistantTextCell::new(String::new());
        // 2-column marker + 3 wide chars (2 cols each) = 8 columns. At width
        // 5 the wide glyph straddling the boundary moves to the next row.
        cell.append("你好吗");
        transcript.set_active(Box::new(cell));
        let tail = transcript.visible_live_tail(5, &Theme::dark());
        assert_eq!(tail.len(), 2, "8 columns at width 5 = 2 rows");
        assert_eq!(tail[0].width(), 4, "● 你 (a split would make 5)");
        assert_eq!(tail[1].width(), 4, "好吗");
        // Short tails are untouched.
        let tail = transcript.visible_live_tail(80, &Theme::dark());
        assert_eq!(tail.len(), 1);
    }
}
