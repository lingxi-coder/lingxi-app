//! `/help` keyboard-shortcuts + slash-command viewer (claude-code `HelpV2`
//! parity): a read-only, scrollable screen listing the prompt shortcuts and the
//! available slash commands.
//!
//! Four-part split mirroring `skills.rs`/`stats.rs`: a [`HelpState`] (an
//! embedded [`crate::screens::scroll::ScrollState`] over the flattened body
//! lines), a [`HelpOutcome`] enum, a pure [`handle_help_key`] reducer (scroll
//! keys delegated to the shared `ScrollState`; Esc / bare `q` close), and a pure
//! [`render_help_to_string`] oracle.
//!
//! Content port (claude-code `components/HelpV2/`): the intro line
//! ([`INTRO`], from `HelpV2/General.tsx`), the `Shortcuts` section
//! ([`SHORTCUTS`], from `PromptInput/PromptInputHelpMenu.tsx` — `! for bash
//! mode`, `/ for commands`, `ctrl + o for verbose output`, …), and a
//! `Slash commands` section ([`SLASH_COMMANDS`], the `commands` tab of
//! `HelpV2`).
//!
//! FORCED DIVERGENCE from claude-code (documented PARITY-GAPs, not blockers):
//! - claude-code's `HelpV2` is a multi-TAB dialog (`general` / `commands` /
//!   `custom-commands` / `[ant-only]`); this port flattens the `general`
//!   (Shortcuts) and `commands` tabs into ONE scrollable screen and omits the
//!   `custom-commands` + `[ant-only]` tabs (no frozen-safe TUI seam to the
//!   per-project custom-command catalog).
//! - The displayed key chords are claude-code's DEFAULT bindings, baked into a
//!   static table. The TUI has no `useShortcutDisplay` / user-keybinding seam
//!   yet, so a user's `keybindings.json` overrides are NOT reflected, and a few
//!   chords differ from the TUI's current live bindings (e.g. `ctrl + g` opens
//!   Settings here rather than `$EDITOR`; `undo` / `stash` / `fast mode` /
//!   `model picker` are listed for parity but are not yet wired as live keys).
//! - The slash-command descriptions are concise in-tree summaries, not the
//!   byte-locked `core_description` metadata (kept self-contained, mirroring
//!   `skills.rs`'s locked-constant style).
#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent};

use crate::screens::scroll::{scroll_indicator, visible_slice, ScrollState};

/// Fixed viewport height (body rows shown before scrolling kicks in). A modest
/// constant keeps the pure oracle deterministic; the live render is line-by-
/// line, but the embedded [`ScrollState`] keeps the screen scroll-capable and
/// unit-testable, mirroring `skills.rs`/`stats.rs`.
const VIEWPORT: usize = 16;

/// Column width the shortcut/command key is padded to, so the labels line up in
/// a tidy left column (claude-code `PromptInputHelpMenu` `fixedWidth`).
const KEY_WIDTH: usize = 16;

/// Locked screen title (claude-code `HelpV2` `Tabs title`).
pub const TITLE: &str = "Help";
/// Locked intro line (claude-code `HelpV2/General.tsx`).
pub const INTRO: &str = "Claude understands your codebase, makes edits with your permission, and executes commands \u{2014} right from your terminal.";
/// Locked `Shortcuts` section header.
pub const SHORTCUTS_HEADER: &str = "Shortcuts";
/// Locked `Slash commands` section header.
pub const COMMANDS_HEADER: &str = "Slash commands";
/// Locked footer hint (mirrors the read-only `skills.rs` footer).
pub const FOOTER: &str = "Esc to close";

/// The prompt shortcuts, in claude-code `PromptInputHelpMenu` render order
/// (left column then right column). Each entry is `(keys, label)`; the rendered
/// line is the key padded to [`KEY_WIDTH`] then the label.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("!", "for bash mode"),
    ("/", "for commands"),
    ("@", "for file paths"),
    ("&", "for background"),
    ("/btw", "for side question"),
    ("ctrl + o", "for verbose output"),
    ("ctrl + t", "to toggle tasks"),
    ("shift + \u{23CE}", "for newline"),
    ("shift + tab", "to auto-accept edits"),
    ("ctrl + _", "to undo"),
    ("ctrl + s", "to stash prompt"),
    ("ctrl + v", "to paste images"),
    ("ctrl + g", "to edit in $EDITOR"),
    ("ctrl + z", "to suspend"),
    ("alt + p", "to switch model"),
    ("alt + o", "to toggle fast mode"),
    ("double tap esc", "to clear input"),
    ("/keybindings", "to customize"),
];

/// The slash commands this TUI surfaces, with concise descriptions (claude-code
/// `HelpV2` `commands` tab). `(command, description)`.
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show keyboard shortcuts and commands"),
    ("/clear", "Clear the conversation history"),
    ("/exit", "Exit Claude Code"),
    ("/agents", "Manage subagents"),
    ("/mcp", "Show configured MCP servers"),
    ("/hooks", "Show configured hooks"),
    ("/model", "Set the active model"),
    ("/skills", "List available skills"),
    ("/stats", "Show usage statistics"),
    ("/doctor", "Diagnose the installation"),
    ("/memory", "Edit CLAUDE.md memory files"),
    ("/theme", "Change the color theme"),
    ("/config", "Open settings"),
    ("/status", "Show the session status"),
    ("/tasks", "View background tasks"),
    ("/vim", "Toggle vim editing mode"),
    ("/export", "Export the transcript"),
    ("/copy", "Copy the last response"),
    ("/color", "Set the prompt accent color"),
];

/// Screen state: just the scroll window over the flattened body lines (the
/// content is static, so unlike `skills`/`stats` there is nothing to load).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpState {
    /// Scroll window over the flattened content lines.
    pub scroll: ScrollState,
}

impl Default for HelpState {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpState {
    /// Build the Help screen, sizing the embedded [`ScrollState`] to the
    /// flattened body-line count and the fixed [`VIEWPORT`].
    #[must_use]
    pub fn new() -> Self {
        let len = content_lines().len();
        Self {
            scroll: ScrollState::new(len, VIEWPORT),
        }
    }
}

/// Controller outcome after a key (mirrors `SkillsOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpOutcome {
    /// Stay open (scrolled or inert key).
    Stay,
    /// Close the screen (Esc / `q`).
    Close,
}

/// Reduce one key. Scroll keys (Up/Down/PageUp/PageDown/Home/End) are handled by
/// the embedded [`ScrollState`]; Esc and bare `q` close. Everything else is
/// inert. Pure — the caller owns closing the screen + telemetry. Mirrors
/// `skills::handle_skills_key`.
#[must_use]
pub fn handle_help_key(state: &mut HelpState, key: KeyEvent) -> HelpOutcome {
    if state.scroll.handle_scroll_key(key) {
        return HelpOutcome::Stay;
    }
    match key.code {
        KeyCode::Esc => HelpOutcome::Close,
        KeyCode::Char('q') if key.modifiers == crossterm::event::KeyModifiers::NONE => {
            HelpOutcome::Close
        }
        _ => HelpOutcome::Stay,
    }
}

/// Format one `(key, label)` row: the key left-padded to [`KEY_WIDTH`], a single
/// space, then the label (claude-code `PromptInputHelpMenu` aligned columns).
/// Padding is measured in `char`s so the multi-byte `⏎` glyph aligns visually.
fn format_row(key: &str, label: &str) -> String {
    let pad = KEY_WIDTH.saturating_sub(key.chars().count());
    format!("{key}{} {label}", " ".repeat(pad))
}

/// Flatten the two sections to the body content lines (no title/intro/footer):
/// the `Shortcuts` header + one line per shortcut, then the `Slash commands`
/// header + one line per command. This is the list the embedded [`ScrollState`]
/// scrolls over.
fn content_lines() -> Vec<String> {
    let mut out = Vec::with_capacity(SHORTCUTS.len() + SLASH_COMMANDS.len() + 2);
    out.push(SHORTCUTS_HEADER.to_string());
    for (key, label) in SHORTCUTS {
        out.push(format_row(key, label));
    }
    out.push(COMMANDS_HEADER.to_string());
    for (cmd, desc) in SLASH_COMMANDS {
        out.push(format_row(cmd, desc));
    }
    out
}

/// Pure render oracle: the full screen body as text.
///
/// `Help` title, the `INTRO` line, then the visible window of the flattened
/// section/row lines, then (when scrolled) a scroll indicator, then the
/// `Esc to close` footer. Mirrors `render_skills_to_string`.
#[must_use]
pub fn render_help_to_string(state: &HelpState) -> String {
    let mut out = String::from(TITLE);
    out.push('\n');
    out.push_str(INTRO);
    out.push('\n');

    let lines = content_lines();
    for line in visible_slice(&lines, &state.scroll) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(ind) = scroll_indicator(&state.scroll) {
        out.push_str(&ind);
        out.push('\n');
    }
    out.push_str(FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn render_shows_title_intro_and_section_headers() {
        let s = HelpState::new();
        let out = render_help_to_string(&s);
        assert!(out.starts_with("Help\n"), "got: {out}");
        assert!(out.contains(INTRO), "intro line present");
        assert!(out.contains(SHORTCUTS_HEADER), "Shortcuts header present");
        assert!(out.ends_with(FOOTER), "footer present, got: {out}");
    }

    #[test]
    fn render_contains_shortcut_lines() {
        // The window only shows VIEWPORT rows at a time; scroll a fresh state to
        // the bottom so both sections are exercised across the two windows.
        let top = render_help_to_string(&HelpState::new());
        // First-window shortcuts (claude-code `PromptInputHelpMenu`).
        assert!(top.contains("for bash mode"), "bash-mode shortcut, got: {top}");
        assert!(top.contains("for commands"), "slash shortcut");
        assert!(top.contains("for file paths"), "@ shortcut");
        assert!(top.contains("ctrl + o"), "verbose-output chord");
        assert!(top.contains("for verbose output"), "verbose-output label");
    }

    #[test]
    fn slash_commands_section_renders_when_scrolled() {
        // Anchor the window on the `Slash commands` header so the section + its
        // rows are visible (a plain `End` jump would scroll the header off the
        // top of the window).
        let mut s = HelpState::new();
        let header_idx = content_lines()
            .iter()
            .position(|l| l == COMMANDS_HEADER)
            .expect("Slash commands header is a body line");
        s.scroll.set_offset(header_idx);
        let out = render_help_to_string(&s);
        assert!(out.contains(COMMANDS_HEADER), "Slash commands header, got: {out}");
        assert!(out.contains("/skills"), "a known command is listed, got: {out}");
        // The very last command is reachable by jumping to the bottom.
        assert_eq!(handle_help_key(&mut s, k(KeyCode::End)), HelpOutcome::Stay);
        let bottom = render_help_to_string(&s);
        assert!(bottom.contains("/color"), "last command at the bottom, got: {bottom}");
    }

    #[test]
    fn body_is_scrollable_and_indicator_shows() {
        // The combined sections exceed VIEWPORT, so the screen scrolls and the
        // top window shows a `↓ N more` indicator.
        let s = HelpState::new();
        assert!(s.scroll.is_scrollable(), "help body taller than the window");
        let out = render_help_to_string(&s);
        assert!(out.contains("more"), "scroll indicator present, got: {out}");
    }

    #[test]
    fn esc_and_q_close_other_keys_stay() {
        let mut s = HelpState::new();
        assert_eq!(handle_help_key(&mut s, k(KeyCode::Esc)), HelpOutcome::Close);
        assert_eq!(
            handle_help_key(&mut s, k(KeyCode::Char('q'))),
            HelpOutcome::Close
        );
        assert_eq!(
            handle_help_key(&mut s, k(KeyCode::Enter)),
            HelpOutcome::Stay
        );
        // Ctrl-q is NOT the bare-`q` close (modifier guard, mirrors skills).
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(handle_help_key(&mut s, ctrl_q), HelpOutcome::Stay);
    }

    #[test]
    fn scroll_keys_move_window_and_stay() {
        let mut s = HelpState::new();
        assert_eq!(s.scroll.offset(), 0);
        assert_eq!(handle_help_key(&mut s, k(KeyCode::Down)), HelpOutcome::Stay);
        assert_eq!(s.scroll.offset(), 1);
        assert_eq!(handle_help_key(&mut s, k(KeyCode::End)), HelpOutcome::Stay);
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
        // Further Down clamps (still Stay).
        assert_eq!(handle_help_key(&mut s, k(KeyCode::Down)), HelpOutcome::Stay);
        assert_eq!(s.scroll.offset(), s.scroll.max_offset());
    }

    #[test]
    fn format_row_pads_key_and_aligns_label() {
        let line = format_row("/", "for commands");
        assert!(line.starts_with('/'), "key first");
        assert!(line.ends_with("for commands"), "label trails: {line}");
        // The key is padded to KEY_WIDTH chars + 1 space before the label.
        assert_eq!(line.chars().count(), KEY_WIDTH + 1 + "for commands".chars().count());
    }

    #[test]
    fn every_shortcut_and_command_is_a_body_line() {
        let lines = content_lines();
        // Header + each shortcut + header + each command.
        assert_eq!(lines.len(), SHORTCUTS.len() + SLASH_COMMANDS.len() + 2);
        for (key, label) in SHORTCUTS {
            assert!(
                lines.iter().any(|l| l.contains(key) && l.contains(label)),
                "shortcut {key} / {label} present"
            );
        }
        for (cmd, desc) in SLASH_COMMANDS {
            assert!(
                lines.iter().any(|l| l.contains(cmd) && l.contains(desc)),
                "command {cmd} / {desc} present"
            );
        }
    }
}
