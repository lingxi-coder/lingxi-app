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
//! - The displayed key chords are rendered from the LIVE runtime keymap
//!   (`command_core::keybindings::get_binding_display_text`, the analogue of
//!   claude-code's `useShortcutDisplay`): each rebindable row carries an
//!   `(action, context)` plus a fallback chord, and the rendered chord is the
//!   resolved binding text (a user's `~/.claude/keybindings.json` override is
//!   reflected here) or the fallback when the action has no binding. The
//!   non-rebindable rows (`!`, `/`, `@`, `&`, `/btw`, `/keybindings`,
//!   `double tap esc`) stay literal. Under the DEFAULT keymap the rendered
//!   chords are byte-identical to the historical static table EXCEPT that the
//!   model-picker / fast-mode rows now show their true default chord (`meta + p`
//!   / `meta + o`) rather than the historical `alt + p` / `alt + o` mislabel —
//!   a deliberate parity correction.
//! - The slash-command descriptions are concise in-tree summaries, not the
//!   byte-locked `core_description` metadata (kept self-contained, mirroring
//!   `skills.rs`'s locked-constant style).
#![forbid(unsafe_code)]

use command_core::keybindings::{get_binding_display_text, ParsedBinding};
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
/// Locked footer hint (claude-code `HelpV2` dismiss hint: the resolved
/// `help:dismiss` chord + " to cancel", lowercase). Default keymap → `esc`.
pub const FOOTER: &str = "esc to cancel";
/// "For more help" docs pointer (claude-code `HelpV2/General.tsx`).
pub const MORE_HELP: &str = "For more help: https://code.claude.com/docs/en/overview";

/// One prompt-shortcut row.
///
/// `action`/`context` are `Some` for the REBINDABLE rows — the rendered chord
/// is `get_binding_display_text(action, context, bindings)` (the live keymap,
/// honoring `keybindings.json` overrides) and `fallback` is the chord shown when
/// the action has no binding (1:1 with `useShortcutDisplay`'s `fallback` arg).
/// Rows that are NOT keymap actions (`!`, `/`, `@`, `&`, `/btw`,
/// `double tap esc`, `/keybindings`) carry `action = None` and render `fallback`
/// verbatim. `fallback` is the historical spaced chord, so the DEFAULT keymap
/// renders byte-identically (resolved chords are post-formatted to the same
/// spaced `a + b` style).
#[derive(Debug, Clone, Copy)]
pub struct ShortcutRow {
    /// Keybinding action id (e.g. `"app:toggleTranscript"`), or `None` for a
    /// non-rebindable literal row.
    pub action: Option<&'static str>,
    /// Keybinding context the action resolves in (e.g. `"Global"`/`"Chat"`).
    pub context: &'static str,
    /// Chord shown when `action` is `None` or has no live binding.
    pub fallback: &'static str,
    /// Right-column label.
    pub label: &'static str,
}

/// The prompt shortcuts, in claude-code `PromptInputHelpMenu` render order
/// (left column then right column). REBINDABLE rows carry their
/// `(action, context)` so the chord renders from the live keymap; literal rows
/// (`action = None`) render their fallback verbatim. The action ids match
/// `default_bindings`: e.g. `app:toggleTranscript`/`Global` (`ctrl+o`),
/// `chat:cycleMode`/`Chat` (`shift+tab`), `chat:modelPicker`/`Chat` (`meta+p`).
pub const SHORTCUTS: &[ShortcutRow] = &[
    ShortcutRow { action: None, context: "Global", fallback: "!", label: "for bash mode" },
    ShortcutRow { action: None, context: "Global", fallback: "/", label: "for commands" },
    ShortcutRow { action: None, context: "Global", fallback: "@", label: "for file paths" },
    ShortcutRow { action: None, context: "Global", fallback: "&", label: "for background" },
    ShortcutRow { action: None, context: "Global", fallback: "/btw", label: "for side question" },
    ShortcutRow { action: Some("app:toggleTranscript"), context: "Global", fallback: "ctrl + o", label: "for verbose output" },
    ShortcutRow { action: Some("app:toggleTodos"), context: "Global", fallback: "ctrl + t", label: "to toggle tasks" },
    ShortcutRow { action: None, context: "Chat", fallback: "shift + \u{23CE}", label: "for newline" },
    ShortcutRow { action: Some("chat:cycleMode"), context: "Chat", fallback: "shift + tab", label: "to auto-accept edits" },
    ShortcutRow { action: Some("chat:undo"), context: "Chat", fallback: "ctrl + _", label: "to undo" },
    ShortcutRow { action: Some("chat:stash"), context: "Chat", fallback: "ctrl + s", label: "to stash prompt" },
    ShortcutRow { action: Some("chat:imagePaste"), context: "Chat", fallback: "ctrl + v", label: "to paste images" },
    ShortcutRow { action: Some("chat:externalEditor"), context: "Chat", fallback: "ctrl + g", label: "to edit in $EDITOR" },
    ShortcutRow { action: None, context: "Global", fallback: "ctrl + z", label: "to suspend" },
    ShortcutRow { action: Some("chat:modelPicker"), context: "Chat", fallback: "meta + p", label: "to switch model" },
    ShortcutRow { action: Some("chat:fastMode"), context: "Chat", fallback: "meta + o", label: "to toggle fast mode" },
    ShortcutRow { action: None, context: "Global", fallback: "double tap esc", label: "to clear input" },
    ShortcutRow { action: None, context: "Global", fallback: "/keybindings", label: "to customize" },
];

/// Resolve one [`ShortcutRow`]'s display chord against the live keymap bindings.
/// 1:1 with `useShortcutDisplay`'s `getDisplayText(action, context) ?? fallback`:
/// a rebindable row's chord is the resolved binding text (post-formatted to the
/// spaced `a + b` style help uses), falling back to `fallback` when unbound; a
/// literal row (`action = None`) always renders `fallback`.
fn row_chord(row: &ShortcutRow, bindings: &[ParsedBinding]) -> String {
    match row.action {
        Some(action) => get_binding_display_text(action, row.context, bindings)
            .map_or_else(|| row.fallback.to_string(), |c| spaced_chord(&c)),
        None => row.fallback.to_string(),
    }
}

/// Re-space a resolved chord (`"ctrl+o"` → `"ctrl + o"`) so a live-rendered
/// default chord is byte-identical to the historical static spaced table. The
/// resolver emits `+`-joined chords with no surrounding spaces; help's column
/// uses ` + ` separators.
fn spaced_chord(chord: &str) -> String {
    chord.replace('+', " + ")
}

/// The slash commands this TUI surfaces, with concise descriptions (claude-code
/// `HelpV2` `commands` tab). `(command, description)`.
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show keyboard shortcuts and commands"),
    ("/clear", "Clear the conversation history"),
    ("/exit", "Exit Claude Code"),
    ("/agents", "Manage agent configurations"),
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
    /// flattened body-line count and the fixed [`VIEWPORT`]. The line COUNT is
    /// keymap-independent (one row per shortcut/command regardless of the
    /// rendered chord), so sizing uses the default bindings.
    #[must_use]
    pub fn new() -> Self {
        let len = content_lines(&default_bindings()).len();
        Self {
            scroll: ScrollState::new(len, VIEWPORT),
        }
    }
}

/// The default merged keybindings — used to size the scroll window and as the
/// fallback for the keymap-free [`render_help_to_string`] oracle (so existing
/// callers/tests render the byte-identical default chords).
fn default_bindings() -> Vec<ParsedBinding> {
    command_core::keybindings::Keymap::defaults()
        .bindings()
        .to_vec()
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
/// the `Shortcuts` header + one line per shortcut (its chord resolved from
/// `bindings` — the live keymap), then the `Slash commands` header + one line
/// per command. This is the list the embedded [`ScrollState`] scrolls over.
fn content_lines(bindings: &[ParsedBinding]) -> Vec<String> {
    let mut out = Vec::with_capacity(SHORTCUTS.len() + SLASH_COMMANDS.len() + 2);
    out.push(SHORTCUTS_HEADER.to_string());
    for row in SHORTCUTS {
        // (help-3) `ctrl + z to suspend` is non-Windows only
        // (claude-code `getPlatform() !== 'windows'`).
        if cfg!(windows) && row.label == "to suspend" {
            continue;
        }
        out.push(format_row(&row_chord(row, bindings), row.label));
    }
    out.push(COMMANDS_HEADER.to_string());
    for (cmd, desc) in SLASH_COMMANDS {
        out.push(format_row(cmd, desc));
    }
    out
}

/// Pure render oracle: the full screen body as text, rendering shortcut chords
/// from the DEFAULT keymap. Existing callers/tests keep byte-identical output;
/// the live screen uses [`render_help_to_string_with`] to reflect a user's
/// `keybindings.json` overrides.
#[must_use]
pub fn render_help_to_string(state: &HelpState) -> String {
    render_help_to_string_with(state, &default_bindings())
}

/// Pure render oracle parameterized on the live keymap `bindings`: the shortcut
/// chords are resolved via `get_binding_display_text` (the `useShortcutDisplay`
/// analogue), so a user's `~/.claude/keybindings.json` override shows here.
///
/// `Help` title, the `INTRO` line, then the visible window of the flattened
/// section/row lines, then (when scrolled) a scroll indicator, then the
/// `Esc to close` footer. Mirrors `render_skills_to_string`.
#[must_use]
pub fn render_help_to_string_with(state: &HelpState, bindings: &[ParsedBinding]) -> String {
    let mut out = String::from(TITLE);
    out.push('\n');
    out.push_str(INTRO);
    out.push('\n');

    let lines = content_lines(bindings);
    for line in visible_slice(&lines, &state.scroll) {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(ind) = scroll_indicator(&state.scroll) {
        out.push_str(&ind);
        out.push('\n');
    }
    // (help-5) "For more help" docs pointer, above the footer.
    out.push_str(MORE_HELP);
    out.push('\n');
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
        assert!(
            top.contains("for bash mode"),
            "bash-mode shortcut, got: {top}"
        );
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
        let header_idx = content_lines(&default_bindings())
            .iter()
            .position(|l| l == COMMANDS_HEADER)
            .expect("Slash commands header is a body line");
        s.scroll.set_offset(header_idx);
        let out = render_help_to_string(&s);
        assert!(
            out.contains(COMMANDS_HEADER),
            "Slash commands header, got: {out}"
        );
        assert!(
            out.contains("/skills"),
            "a known command is listed, got: {out}"
        );
        // The very last command is reachable by jumping to the bottom.
        assert_eq!(handle_help_key(&mut s, k(KeyCode::End)), HelpOutcome::Stay);
        let bottom = render_help_to_string(&s);
        assert!(
            bottom.contains("/color"),
            "last command at the bottom, got: {bottom}"
        );
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
        assert_eq!(
            line.chars().count(),
            KEY_WIDTH + 1 + "for commands".chars().count()
        );
    }

    #[test]
    fn every_shortcut_and_command_is_a_body_line() {
        let bindings = default_bindings();
        let lines = content_lines(&bindings);
        // (help-3) the `to suspend` row is dropped on Windows only.
        let dropped = usize::from(cfg!(windows));
        // Header + each shortcut + header + each command.
        assert_eq!(
            lines.len(),
            SHORTCUTS.len() - dropped + SLASH_COMMANDS.len() + 2
        );
        for row in SHORTCUTS {
            if cfg!(windows) && row.label == "to suspend" {
                continue;
            }
            let chord = row_chord(row, &bindings);
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains(&chord) && l.contains(row.label)),
                "shortcut {chord} / {} present",
                row.label
            );
        }
        for (cmd, desc) in SLASH_COMMANDS {
            assert!(
                lines.iter().any(|l| l.contains(cmd) && l.contains(desc)),
                "command {cmd} / {desc} present"
            );
        }
    }

    /// (GAP D — help display) Build a `Vec<ParsedBinding>` from an inline
    /// keybindings JSON override merged over the defaults (the live keymap the
    /// help screen renders against).
    fn bindings_with_override(json: &str) -> Vec<ParsedBinding> {
        use command_core::keybindings::load_keybindings;
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "lingxi-help-kb-{}-{}.json",
            std::process::id(),
            // nanos to keep parallel test invocations from colliding.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
        let bindings = load_keybindings(true, &path, false).bindings;
        let _ = std::fs::remove_file(&path);
        bindings
    }

    /// (GAP D — help display, TDD) A user override of a rebindable shortcut
    /// (`app:toggleTranscript` → `ctrl+y`) is reflected in the live-rendered
    /// help chord, while an un-overridden row keeps its default chord. This is
    /// the `useShortcutDisplay` semantics applied to the Help screen.
    #[test]
    fn help_renders_overridden_chord_and_keeps_unspecified_default() {
        let bindings = bindings_with_override(
            r#"{ "bindings": [ { "context": "Global", "bindings": { "ctrl+y": "app:toggleTranscript" } } ] }"#,
        );
        let s = HelpState::new();
        let out = render_help_to_string_with(&s, &bindings);

        // The verbose-output row (app:toggleTranscript / Global) now shows the
        // overridden chord `ctrl + y` instead of the default `ctrl + o`.
        assert!(
            out.contains("ctrl + y") && out.contains("for verbose output"),
            "overridden chord rendered, got: {out}"
        );
        // The un-overridden toggle-tasks row keeps its default `ctrl + t`.
        assert!(
            out.contains("ctrl + t") && out.contains("to toggle tasks"),
            "unspecified row keeps default chord, got: {out}"
        );
    }

    /// (GAP D — help display) Under the DEFAULT keymap the rebindable rows
    /// render their default chords (the model-picker / fast-mode rows show their
    /// TRUE default `meta + p` / `meta + o`, the documented parity correction).
    #[test]
    fn default_keymap_renders_default_chords() {
        let bindings = default_bindings();
        let chord = |action: &'static str, ctx: &'static str| -> String {
            row_chord(
                &ShortcutRow { action: Some(action), context: ctx, fallback: "X", label: "" },
                &bindings,
            )
        };
        assert_eq!(chord("app:toggleTranscript", "Global"), "ctrl + o");
        assert_eq!(chord("chat:cycleMode", "Chat"), "shift + tab");
        assert_eq!(chord("chat:imagePaste", "Chat"), "ctrl + v");
        // The true default for the model picker is meta+p (not the historical
        // `alt + p` mislabel) — rendered live as `meta + p`.
        assert_eq!(chord("chat:modelPicker", "Chat"), "meta + p");
        assert_eq!(chord("chat:fastMode", "Chat"), "meta + o");
    }
}
