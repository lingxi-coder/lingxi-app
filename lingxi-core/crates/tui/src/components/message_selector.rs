//! MessageSelector (M7-14) — search the scrollback, jump back to a message,
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
    // Force the .txt extension (claude-code ExportDialog parity).
    let stem = filename.rsplit_once('.').map_or(filename, |(s, _)| s);
    let final_name = format!("{stem}.txt");
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
}

/// Overlay state for the message search / jump selector. Lives on
/// `AppState`. `filtered` holds indices into `AppState.messages`;
/// `selected_filtered` indexes into `filtered`.
#[derive(Debug, Clone, Default)]
pub struct MessageSelectorState {
    /// `true` while the search overlay is shown (priority-3 focus).
    pub open: bool,
    /// Live search query.
    pub query: String,
    /// Matching message indices (into `AppState.messages`).
    pub filtered: Vec<usize>,
    /// Cursor into `filtered`.
    pub selected_filtered: usize,
}

impl MessageSelectorState {
    /// Open the overlay with an empty query (matches all).
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
    }

    /// Close the overlay and reset.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.filtered.clear();
        self.selected_filtered = 0;
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
pub fn handle_message_selector_key(
    st: &mut MessageSelectorState,
    messages: &[RenderedMessage],
    key: KeyEvent,
) -> SelectorAction {
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

/// Props for [`MessageSelector`]. Cloned from `AppState` each frame.
#[derive(Default, Props)]
pub struct MessageSelectorProps {
    /// Live query string.
    pub query: String,
    /// Result labels (one per filtered match), in `filtered` order. The
    /// caller projects each matched message to a one-line preview.
    pub result_labels: Vec<String>,
    /// Cursor into `result_labels`.
    pub selected: usize,
}

/// One-line preview of a message for the result list (≤ 60 cols).
#[must_use]
pub fn preview_label(msg: &RenderedMessage) -> String {
    let text = searchable_text(msg);
    let first = text.lines().next().unwrap_or("");
    first.chars().take(60).collect()
}

/// The search/jump overlay. Renders the query line and up to the visible
/// window of results with the selected one marked.
#[component]
pub fn MessageSelector(props: &MessageSelectorProps) -> impl Into<AnyElement<'static>> {
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
        assert!(default_export_filename().ends_with(".txt"));
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
