//! `MessageSelector` (M7-14) — search the scrollback, jump back to a message,
//! and export the transcript.
//!
//! Searches [`crate::state::AppState::messages`] by substring, sets
//! `scroll_offset` (M7-03 line model) to jump to a selected message, and
//! exports a plain-text transcript to a default path with overwrite confirm.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use iocraft::prelude::*;

use crate::components::virtual_message_list::HeightCache;
use crate::state::RenderedMessage;

/// Project a message to the plain text the search query matches against.
/// Covers the text-bearing variants; structural/marker-only variants (e.g.
/// compaction boundaries, redacted thinking) project to an empty string and
/// so never match a non-empty query.
// Several single-field text variants share an identical `clone()` body but bind
// distinct field names — keeping the arms separate documents the per-variant
// projection, so the `match_same_arms` collapse hint is intentionally allowed.
#[allow(clippy::match_same_arms)]
fn searchable_text(msg: &RenderedMessage) -> String {
    match msg {
        RenderedMessage::UserText { body, .. }
        | RenderedMessage::AssistantText { body, .. }
        | RenderedMessage::SystemText { body, .. }
        | RenderedMessage::SystemTextRich { body, .. } => body.clone(),
        RenderedMessage::AssistantToolUse { tool, input, .. } => format!("{tool} {input}"),
        RenderedMessage::UserToolResult { tool, result, .. } => format!("{tool} {result}"),
        RenderedMessage::AssistantThinking { thinking, .. } => thinking.clone(),
        RenderedMessage::UserBashInput { command } => command.clone(),
        RenderedMessage::UserBashOutput { stdout, stderr } => format!("{stdout} {stderr}"),
        RenderedMessage::UserCommand { command, args, .. } => format!("{command} {args}"),
        RenderedMessage::UserLocalCommandOutput { stdout, stderr } => {
            format!("{stdout} {stderr}")
        }
        RenderedMessage::UserMemoryInput { input } => input.clone(),
        RenderedMessage::UserPlan { plan_content } => plan_content.clone(),
        RenderedMessage::UserPrompt { text } => text.clone(),
        RenderedMessage::SystemApiError { error, .. } => error.clone(),
        RenderedMessage::RateLimit { text, .. } => text.clone(),
        _ => String::new(),
    }
}

/// Filter `messages` by a case-insensitive substring `query`, returning the
/// matching message **indices** (into `messages`) in order. An empty query
/// matches everything (the selector shows the full list).
#[must_use]
pub fn search_messages(messages: &[RenderedMessage], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..messages.len()).collect();
    }
    let q = query.to_lowercase();
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| searchable_text(m).to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// Compute the line-based `scroll_offset` (M7-03 model: lines from the
/// bottom) that pins message `target_index` to the **top** of the viewport.
///
/// `line_at_start = sum(height_at(0..target_index))`. Offset so the target's
/// first line is the viewport top:
/// `offset = total_lines - line_at_start - viewport_height`, clamped to
/// `[0, total_lines - viewport_height]`. Out-of-range `target_index`
/// clamps to the last message.
#[must_use]
pub fn message_line_offset(
    messages: &[RenderedMessage],
    cache: &HeightCache,
    target_index: usize,
    viewport_height: usize,
) -> usize {
    let total = cache.total_lines();
    let max_offset = total.saturating_sub(viewport_height);
    if messages.is_empty() {
        return 0;
    }
    let idx = target_index.min(messages.len() - 1);
    let line_at_start: usize = (0..idx).map(|i| cache.height_at(i)).sum();
    // Desired offset from the bottom that puts line_at_start at the top.
    let from_bottom = total.saturating_sub(line_at_start);
    let offset = from_bottom.saturating_sub(viewport_height);
    offset.min(max_offset)
}

/// Export failure modes.
#[derive(Debug)]
pub enum ExportError {
    /// Target file exists and `overwrite` was `false` (confirm required).
    Exists(PathBuf),
    /// Filesystem error (with the OS message).
    Io(String),
}

/// Default export directory: `~/.lingxi/exports/`, falling back to the
/// current working directory when the home dir is unavailable (§4 R10).
#[must_use]
pub fn default_export_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".lingxi").join("exports"),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Default export filename, `lingxi-transcript-<unix_secs>.txt`. The
/// timestamp keeps successive exports from colliding (so the overwrite
/// confirm is the exception, not the rule).
#[must_use]
pub fn default_export_filename() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("lingxi-transcript-{secs}.txt")
}

/// Resolve a user-typed export filename to a safe `<basename>.txt`,
/// guaranteeing the export lands INSIDE the resolved export dir (M7-14 review:
/// path-traversal). The input is first clamped to its final path component via
/// [`Path::file_name`] — which drops directory parts and parent refs
/// (`"/etc/passwd"` → `passwd`, `"../../foo"` → `foo`, `"a/b/c"` → `c`) — so a
/// later `dir.join(...)` can never escape `dir`. Inputs with no usable basename
/// (`".."`, `""`, a trailing `/`) fall back to [`default_export_filename`].
/// The resulting stem then gets the forced `.txt` extension (claude-code
/// `ExportDialog` parity).
#[must_use]
fn resolve_export_filename(input: &str) -> String {
    // Basename-clamp: keep only the final component, never directory parts.
    let basename = Path::new(input)
        .file_name()
        .and_then(|n| n.to_str())
        .map_or_else(default_export_filename, str::to_string);
    // Force the .txt extension on the (now directory-free) basename.
    let stem = basename.rsplit_once('.').map_or(&*basename, |(s, _)| s);
    format!("{stem}.txt")
}

/// Render the scrollback to a plain-text transcript.
fn render_transcript(messages: &[RenderedMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        match m {
            RenderedMessage::UserText { body, .. } => {
                out.push_str("> ");
                out.push_str(body);
            }
            RenderedMessage::AssistantText { body, .. } => out.push_str(body),
            RenderedMessage::AssistantToolUse { tool, input, .. } => {
                out.push_str(&format!("● {tool}({input})"));
            }
            RenderedMessage::UserToolResult { tool, result, .. } => {
                out.push_str(&format!("└ {tool}: {result}"));
            }
            other => out.push_str(&searchable_text(other)),
        }
        out.push('\n');
    }
    out
}

/// Export the transcript to `<dir>/<filename>`, forcing a `.txt` extension.
/// When the target exists and `overwrite` is `false`, returns
/// [`ExportError::Exists`] WITHOUT touching the file — the caller prompts
/// for confirmation and retries with `overwrite = true` (§4 R10: no silent
/// clobber). Creates `dir` if absent.
///
/// # Errors
/// [`ExportError::Exists`] on an unconfirmed overwrite; [`ExportError::Io`]
/// on any filesystem failure.
pub fn export_transcript(
    messages: &[RenderedMessage],
    dir: &Path,
    filename: &str,
    overwrite: bool,
) -> Result<PathBuf, ExportError> {
    // Basename-clamp + force the .txt extension (M7-14 review: path-traversal —
    // the result CANNOT escape `dir`; claude-code ExportDialog parity).
    let final_name = resolve_export_filename(filename);
    let target = dir.join(final_name);
    if target.exists() && !overwrite {
        return Err(ExportError::Exists(target));
    }
    std::fs::create_dir_all(dir).map_err(|e| ExportError::Io(e.to_string()))?;
    let body = render_transcript(messages);
    std::fs::write(&target, body.as_bytes()).map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(target)
}

/// Outcome the caller acts on after routing a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorAction {
    /// Nothing to do (state mutated in place).
    None,
    /// Close the overlay (Esc, no jump).
    Close,
    /// Jump the scrollback to `message_index` (caller sets `scroll_offset`
    /// via [`message_line_offset`]) and close the overlay.
    Jump {
        /// Index into `AppState.messages`.
        message_index: usize,
    },
    /// (M7-14 review) The export flow confirmed: the caller must run
    /// [`export_transcript`] for the current `filename` with the given
    /// `overwrite` flag, then report the outcome back via
    /// [`MessageSelectorState::report_export`]. The key handler only mutates
    /// the editable buffer / overwrite-confirm sub-state — the actual
    /// filesystem write stays in the live caller (which owns the messages +
    /// resolves the export dir), keeping `handle_message_selector_key` pure.
    Export {
        /// Whether the user has explicitly confirmed an overwrite (§4 R10).
        /// `false` on the first Enter (write only if the target is absent);
        /// `true` once the overwrite-confirm prompt was answered `y`.
        overwrite: bool,
    },
}

/// Which sub-flow the overlay is showing. `/export` opens [`SelectorMode::Export`]
/// directly (claude-code's `ExportDialog` is a filename prompt, NOT a search
/// box); Ctrl-T opens [`SelectorMode::Search`] (the search/jump list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectorMode {
    /// Search the scrollback + jump to a message (Ctrl-T).
    #[default]
    Search,
    /// Export the transcript: a filename prompt + overwrite confirm (`/export`).
    Export,
}

/// Export sub-state: the editable filename buffer, the overwrite-confirm
/// latch, and the last status line (success/failure/cancel message). Lives
/// inside [`MessageSelectorState`] while [`SelectorMode::Export`] is active.
#[derive(Debug, Clone, Default)]
pub struct ExportFlowState {
    /// Editable filename buffer, pre-filled with [`default_export_filename`].
    pub filename: String,
    /// `true` once the resolved path already existed and we are awaiting an
    /// explicit `y`/`n` overwrite decision (§4 R10: no silent clobber).
    pub awaiting_overwrite: bool,
    /// Status line surfaced after a write attempt or cancel (one of the
    /// literal-locked strings). `None` while still editing the filename.
    pub status: Option<String>,
    /// `true` once a write succeeded (or the user cancelled): the flow is
    /// done and the next key (any) closes the overlay.
    pub done: bool,
}

/// Overlay state for the message search / jump selector. Lives on
/// `AppState`. `filtered` holds indices into `AppState.messages`;
/// `selected_filtered` indexes into `filtered`.
#[derive(Debug, Clone, Default)]
pub struct MessageSelectorState {
    /// `true` while the search overlay is shown (priority-3 focus).
    pub open: bool,
    /// Which sub-flow is active (search/jump vs. export).
    pub mode: SelectorMode,
    /// Live search query.
    pub query: String,
    /// Matching message indices (into `AppState.messages`).
    pub filtered: Vec<usize>,
    /// Cursor into `filtered`.
    pub selected_filtered: usize,
    /// Export sub-state, meaningful only while `mode == Export`.
    pub export: ExportFlowState,
    /// Override for the export directory. `None` → [`default_export_dir`]
    /// (`~/.lingxi/exports/`). Tests inject a temp dir here so the export
    /// flow never touches the real home directory.
    pub export_dir_override: Option<PathBuf>,
}

impl MessageSelectorState {
    /// Open the SEARCH overlay with an empty query (matches all). Ctrl-T.
    pub fn open(&mut self) {
        self.open = true;
        self.mode = SelectorMode::Search;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
    }

    /// (M7-14 review) Open the EXPORT overlay directly (the `/export`
    /// `ExportDialog`: a filename prompt, not a search box). Pre-fills the
    /// editable buffer with [`default_export_filename`].
    pub fn open_export(&mut self) {
        self.open = true;
        self.mode = SelectorMode::Export;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
        self.export = ExportFlowState {
            filename: default_export_filename(),
            awaiting_overwrite: false,
            status: None,
            done: false,
        };
    }

    /// Close the overlay and reset.
    pub fn close(&mut self) {
        self.open = false;
        self.mode = SelectorMode::Search;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
        self.export = ExportFlowState::default();
    }

    /// Resolve the export directory: the test override if set, else the
    /// default `~/.lingxi/exports/` (§4 R10).
    #[must_use]
    pub fn resolved_export_dir(&self) -> PathBuf {
        self.export_dir_override
            .clone()
            .unwrap_or_else(default_export_dir)
    }

    /// (M7-14 review) Fold an export attempt's outcome back into the sub-state
    /// after the live caller ran [`export_transcript`]. On success surface
    /// `"Conversation exported to: {path}"` and mark the flow `done`. On
    /// [`ExportError::Exists`] arm the overwrite-confirm prompt (do NOT mark
    /// done — §4 R10: wait for explicit `y`). On [`ExportError::Io`] surface
    /// `"Failed to export conversation: {err}"` and mark done.
    pub fn report_export(&mut self, outcome: &Result<PathBuf, ExportError>) {
        match outcome {
            Ok(path) => {
                self.export.status = Some(format!("Conversation exported to: {}", path.display()));
                self.export.awaiting_overwrite = false;
                self.export.done = true;
            }
            Err(ExportError::Exists(_)) => {
                // The target exists and we have not confirmed — prompt y/n.
                self.export.awaiting_overwrite = true;
                self.export.status = None;
                self.export.done = false;
            }
            Err(ExportError::Io(err)) => {
                self.export.status = Some(format!("Failed to export conversation: {err}"));
                self.export.awaiting_overwrite = false;
                self.export.done = true;
            }
        }
    }

    /// Re-run the filter against `messages` and clamp the selection.
    pub(crate) fn refilter(&mut self, messages: &[RenderedMessage]) {
        self.filtered = search_messages(messages, &self.query);
        if self.filtered.is_empty() {
            self.selected_filtered = 0;
        } else {
            self.selected_filtered = self.selected_filtered.min(self.filtered.len() - 1);
        }
    }

    /// Populate `filtered` from the current query (used right after `open`
    /// so the overlay shows the full list immediately).
    pub fn refilter_all(&mut self, messages: &[RenderedMessage]) {
        self.refilter(messages);
    }
}

/// Route one key into the selector overlay. Returns a [`SelectorAction`].
/// Dispatches by [`SelectorMode`]: the export flow owns keys while
/// `mode == Export`; otherwise the original search/jump routing runs.
pub fn handle_message_selector_key(
    st: &mut MessageSelectorState,
    messages: &[RenderedMessage],
    key: KeyEvent,
) -> SelectorAction {
    if st.mode == SelectorMode::Export {
        return handle_export_key(st, key);
    }
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            st.close();
            SelectorAction::Close
        }
        (KeyCode::Enter, _) => {
            if let Some(&idx) = st.filtered.get(st.selected_filtered) {
                st.close();
                SelectorAction::Jump { message_index: idx }
            } else {
                SelectorAction::None
            }
        }
        (KeyCode::Up, _) => {
            st.selected_filtered = st.selected_filtered.saturating_sub(1);
            SelectorAction::None
        }
        (KeyCode::Down, _) => {
            if !st.filtered.is_empty() {
                st.selected_filtered = (st.selected_filtered + 1).min(st.filtered.len() - 1);
            }
            SelectorAction::None
        }
        (KeyCode::Backspace, _) => {
            st.query.pop();
            st.refilter(messages);
            SelectorAction::None
        }
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            st.query.push(c);
            st.refilter(messages);
            SelectorAction::None
        }
        _ => SelectorAction::None,
    }
}

/// Route one key into the EXPORT sub-flow (`mode == Export`). Owns every key
/// while open (focus-trap). State machine:
///   - **done** (success/IO-failure shown): any key closes the overlay.
///   - **awaiting overwrite** (target exists): `y` → [`SelectorAction::Export`]
///     with `overwrite = true`; `n`/Esc → cancel; other keys ignored.
///   - **editing** the filename: printable chars + Backspace edit the buffer;
///     Enter → [`SelectorAction::Export`] with `overwrite = false` (the live
///     caller writes only if the target is absent, else reports `Exists` which
///     arms the overwrite prompt); Esc → cancel.
///
/// Esc anywhere surfaces `"Export cancelled"` and closes after the next key
/// (the caller closes on a [`SelectorAction::Close`]).
fn handle_export_key(st: &mut MessageSelectorState, key: KeyEvent) -> SelectorAction {
    // Terminal state: the write finished (or IO-failed). Any key dismisses.
    if st.export.done {
        st.close();
        return SelectorAction::Close;
    }
    // Overwrite-confirm gate (§4 R10: no silent clobber).
    if st.export.awaiting_overwrite {
        return match key.code {
            KeyCode::Char('y' | 'Y') => {
                // Explicit confirm → ask the caller to write with overwrite.
                SelectorAction::Export { overwrite: true }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                st.export.status = Some("Export cancelled".to_string());
                st.export.awaiting_overwrite = false;
                st.export.done = true;
                SelectorAction::None
            }
            _ => SelectorAction::None,
        };
    }
    // Editing the filename.
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            st.export.status = Some("Export cancelled".to_string());
            st.export.done = true;
            SelectorAction::None
        }
        (KeyCode::Enter, _) => {
            if st.export.filename.trim().is_empty() {
                // Nothing to write to; keep editing.
                SelectorAction::None
            } else {
                // First attempt never silently clobbers (overwrite = false).
                SelectorAction::Export { overwrite: false }
            }
        }
        (KeyCode::Backspace, _) => {
            st.export.filename.pop();
            SelectorAction::None
        }
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            st.export.filename.push(c);
            SelectorAction::None
        }
        _ => SelectorAction::None,
    }
}

/// Props for [`MessageSelector`]. Cloned from `AppState` each frame.
#[derive(Default, Props)]
pub struct MessageSelectorProps {
    /// Which sub-flow to render (search/jump vs. export).
    pub mode: SelectorMode,
    /// Live search query (search mode).
    pub query: String,
    /// Result labels (one per filtered match), in `filtered` order. The
    /// caller projects each matched message to a one-line preview.
    pub result_labels: Vec<String>,
    /// Cursor into `result_labels`.
    pub selected: usize,
    /// Export sub-state (export mode): editable filename, overwrite-confirm
    /// latch, and the status line.
    pub export: ExportFlowState,
}

/// One-line preview of a message for the result list (≤ 60 cols).
#[must_use]
pub fn preview_label(msg: &RenderedMessage) -> String {
    let text = searchable_text(msg);
    let first = text.lines().next().unwrap_or("");
    first.chars().take(60).collect()
}

/// The selector overlay. In [`SelectorMode::Search`] it renders the query
/// line + the result window; in [`SelectorMode::Export`] it renders the
/// `ExportDialog` (filename prompt → overwrite confirm → status). All the
/// export literals (`"Export Conversation"`, `"Enter filename:"`,
/// `"Conversation exported to: …"`, `"Failed to export conversation: …"`,
/// `"Export cancelled"`) are surfaced here.
#[component]
pub fn MessageSelector(props: &MessageSelectorProps) -> impl Into<AnyElement<'static>> {
    if props.mode == SelectorMode::Export {
        return render_export(&props.export);
    }
    let selected = props.selected;
    let rows: Vec<AnyElement<'static>> = props
        .result_labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let marker = if i == selected { "❯ " } else { "  " };
            element! { Text(content: format!("{marker}{label}")) }.into_any()
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct) {
            Text(content: format!("Search: {}", props.query))
            #(rows)
            Text(content: "↑/↓ select · Enter jump · Esc cancel")
        }
    }
    .into_any()
}

/// Render the `ExportDialog` body for the given export sub-state.
fn render_export(export: &ExportFlowState) -> AnyElement<'static> {
    // Once a status line is set (success / IO-failure / cancel), show it +
    // the dismiss hint. Otherwise show the filename prompt (or the overwrite
    // confirm when the target already exists).
    let body: AnyElement<'static> = if let Some(status) = &export.status {
        element! {
            View(flex_direction: FlexDirection::Column, width: 100pct) {
                Text(content: status.clone())
                Text(content: "Press any key to dismiss")
            }
        }
        .into_any()
    } else if export.awaiting_overwrite {
        element! {
            View(flex_direction: FlexDirection::Column, width: 100pct) {
                Text(content: format!("{} already exists. Overwrite? (y/n)", export.filename))
                Text(content: "y confirm · n/Esc cancel")
            }
        }
        .into_any()
    } else {
        element! {
            View(flex_direction: FlexDirection::Column, width: 100pct) {
                Text(content: "Enter filename:")
                Text(content: format!("> {}", export.filename))
                Text(content: "Enter export · Esc cancel")
            }
        }
        .into_any()
    };
    element! {
        View(flex_direction: FlexDirection::Column, width: 100pct) {
            Text(content: "Export Conversation")
            #(vec![body])
        }
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::RenderedMessage;
    use std::fs;
    use tempfile::TempDir;

    fn user(body: &str) -> RenderedMessage {
        RenderedMessage::UserText {
            body: body.to_string(),
            timestamp: 0,
        }
    }
    fn asst(body: &str) -> RenderedMessage {
        RenderedMessage::AssistantText {
            body: body.to_string(),
            timestamp: 0,
        }
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn empty_query_returns_all_indices() {
        let msgs = vec![user("hello"), asst("world")];
        assert_eq!(search_messages(&msgs, ""), vec![0, 1]);
    }

    #[test]
    fn substring_filter_is_case_insensitive() {
        let msgs = vec![user("Hello World"), asst("goodbye"), user("WORLDLY")];
        assert_eq!(search_messages(&msgs, "world"), vec![0, 2]);
    }

    #[test]
    fn no_match_returns_empty() {
        let msgs = vec![user("a"), asst("b")];
        assert!(search_messages(&msgs, "zzz").is_empty());
    }

    #[test]
    fn searches_tool_use_and_result_text() {
        let id = lingxi_protocol::ToolUseId::new();
        let msgs = vec![RenderedMessage::AssistantToolUse {
            id,
            tool: "Bash".into(),
            input: serde_json::json!({"command": "ls -la"}),
        }];
        // matches the tool name
        assert_eq!(search_messages(&msgs, "bash"), vec![0]);
        // matches inside the json input
        assert_eq!(search_messages(&msgs, "ls -la"), vec![0]);
    }

    #[test]
    fn jump_to_last_message_is_offset_zero() {
        // 4 one-line msgs (total 4 lines), viewport 10 → bottom anchor.
        let msgs = vec![user("m0"), user("m1"), user("m2"), user("m3")];
        let cache = HeightCache::build(&msgs, 80);
        // Jumping to the newest message keeps us at the bottom (offset 0).
        assert_eq!(message_line_offset(&msgs, &cache, 3, 10), 0);
    }

    #[test]
    fn jump_to_first_pins_it_to_viewport_top() {
        // Heights: m0=1, m1=50, m2=1 → total 52. viewport 10.
        let msgs = vec![user("m0"), user(&vec!["x"; 50].join("\n")), user("m2")];
        let cache = HeightCache::build(&msgs, 80);
        assert_eq!(cache.total_lines(), 52);
        // line_at_start_of[0] = 0. offset = total(52) - 0 - viewport(10) = 42.
        assert_eq!(message_line_offset(&msgs, &cache, 0, 10), 42);
    }

    #[test]
    fn jump_to_middle_tall_message_pins_its_top() {
        let msgs = vec![user("m0"), user(&vec!["x"; 50].join("\n")), user("m2")];
        let cache = HeightCache::build(&msgs, 80);
        // line_at_start_of[1] = 1. offset = 52 - 1 - 10 = 41.
        assert_eq!(message_line_offset(&msgs, &cache, 1, 10), 41);
    }

    #[test]
    fn jump_offset_never_exceeds_max() {
        let msgs = vec![user("only")];
        let cache = HeightCache::build(&msgs, 80);
        // total 1, viewport 10 → max_offset 0; clamps to 0.
        assert_eq!(message_line_offset(&msgs, &cache, 0, 10), 0);
    }

    #[test]
    fn export_writes_plaintext_transcript_to_dir() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("hello"), asst("hi there")];
        let path = export_transcript(&msgs, tmp.path(), "session.txt", false).unwrap();
        assert_eq!(path, tmp.path().join("session.txt"));
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("hello"));
        assert!(body.contains("hi there"));
    }

    #[test]
    fn export_forces_txt_extension() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        // No extension → .txt appended.
        let p1 = export_transcript(&msgs, tmp.path(), "notes", false).unwrap();
        assert_eq!(p1.extension().unwrap(), "txt");
        // Wrong extension → replaced with .txt (distinct stem so it doesn't
        // collide with `notes.txt` above and trip the overwrite guard).
        let p2 = export_transcript(&msgs, tmp.path(), "other.md", false).unwrap();
        assert_eq!(p2.file_name().unwrap(), "other.txt");
    }

    // (M7-14 review) Path-traversal: the user-typed filename is clamped to its
    // BASENAME so the export can never escape the resolved export dir. Drive
    // the pure resolver directly with an injected temp dir.

    #[test]
    fn export_absolute_path_is_clamped_to_basename() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        // "/etc/passwd" MUST NOT write outside the dir — it lands as
        // <dir>/passwd.txt, never /etc/passwd.txt.
        let p = export_transcript(&msgs, tmp.path(), "/etc/passwd", false).unwrap();
        assert_eq!(p.parent().unwrap(), tmp.path());
        assert_eq!(p.file_name().unwrap(), "passwd.txt");
    }

    #[test]
    fn export_parent_traversal_is_clamped_to_basename() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        // "../../escape" must not climb out of the dir.
        let p = export_transcript(&msgs, tmp.path(), "../../escape", false).unwrap();
        assert_eq!(p.parent().unwrap(), tmp.path());
        assert_eq!(p.file_name().unwrap(), "escape.txt");
    }

    #[test]
    fn export_nested_relative_path_keeps_only_final_component() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        // "sub/dir/name" → only the final component survives.
        let p = export_transcript(&msgs, tmp.path(), "sub/dir/name", false).unwrap();
        assert_eq!(p.parent().unwrap(), tmp.path());
        assert_eq!(p.file_name().unwrap(), "name.txt");
    }

    #[test]
    fn export_pure_parent_ref_falls_back_to_default_filename() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        // ".." has no usable basename → fall back to the default filename.
        let p = export_transcript(&msgs, tmp.path(), "..", false).unwrap();
        assert_eq!(p.parent().unwrap(), tmp.path());
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("lingxi-transcript-"));
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("txt"));
    }

    #[test]
    fn export_normal_filename_still_works() {
        let tmp = TempDir::new().unwrap();
        let msgs = vec![user("x")];
        let p = export_transcript(&msgs, tmp.path(), "session", false).unwrap();
        assert_eq!(p.parent().unwrap(), tmp.path());
        assert_eq!(p.file_name().unwrap(), "session.txt");
    }

    #[test]
    fn resolve_export_filename_clamps_and_forces_txt() {
        // Drive the pure resolver: absolute / parent / nested all clamp to the
        // basename, then get the `.txt` rule; a bare ".." falls back.
        assert_eq!(resolve_export_filename("/etc/passwd"), "passwd.txt");
        assert_eq!(resolve_export_filename("../../escape"), "escape.txt");
        assert_eq!(resolve_export_filename("sub/dir/name"), "name.txt");
        assert_eq!(resolve_export_filename("notes.md"), "notes.txt");
        let fallback = resolve_export_filename("..");
        assert!(fallback.starts_with("lingxi-transcript-"));
        assert_eq!(
            Path::new(&fallback).extension().and_then(|e| e.to_str()),
            Some("txt")
        );
    }

    #[test]
    fn export_refuses_silent_overwrite() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dup.txt");
        fs::write(&target, b"original").unwrap();
        let msgs = vec![user("new content")];
        // overwrite=false on an existing file → ExportError::Exists, original kept.
        match export_transcript(&msgs, tmp.path(), "dup.txt", false) {
            Err(ExportError::Exists(p)) => assert_eq!(p, target),
            other => panic!("expected Exists, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        // overwrite=true → clobbers (after the user confirmed).
        export_transcript(&msgs, tmp.path(), "dup.txt", true).unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("new content"));
    }

    #[test]
    fn default_filename_ends_in_txt() {
        let name = default_export_filename();
        assert_eq!(
            Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase),
            Some("txt".to_string())
        );
    }

    #[test]
    fn typing_builds_query_and_filters_selection() {
        let msgs = vec![user("apple"), asst("banana"), user("apricot")];
        let mut st = MessageSelectorState::default();
        st.open();
        assert!(st.open);
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('a')));
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('p')));
        assert_eq!(st.query, "ap");
        // "ap" matches apple(0) + apricot(2).
        assert_eq!(st.filtered, vec![0, 2]);
        // Selection clamps within the filtered set.
        assert!(st.selected_filtered < st.filtered.len());
    }

    #[test]
    fn up_down_move_within_filtered_results() {
        let msgs = vec![user("x1"), user("x2"), user("x3")];
        let mut st = MessageSelectorState::default();
        st.open();
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('x'))); // all match
        assert_eq!(st.filtered, vec![0, 1, 2]);
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Down));
        assert_eq!(st.selected_filtered, 1);
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Up));
        assert_eq!(st.selected_filtered, 0);
    }

    #[test]
    fn enter_returns_jump_to_underlying_message_index() {
        let msgs = vec![user("alpha"), asst("beta"), user("alpaca")];
        let mut st = MessageSelectorState::default();
        st.open();
        handle_message_selector_key(&mut st, &msgs, k(KeyCode::Char('a'))); // matches 0,1,2
                                                                            // "a" matches alpha(0), beta(1), alpaca(2) → pick the 3rd.
        st.selected_filtered = 2;
        let action = handle_message_selector_key(&mut st, &msgs, k(KeyCode::Enter));
        assert_eq!(action, SelectorAction::Jump { message_index: 2 });
        assert!(!st.open); // selecting closes the overlay
    }

    #[test]
    fn esc_closes_without_jump() {
        let msgs = vec![user("a")];
        let mut st = MessageSelectorState::default();
        st.open();
        let action = handle_message_selector_key(&mut st, &msgs, k(KeyCode::Esc));
        assert_eq!(action, SelectorAction::Close);
        assert!(!st.open);
    }
}
