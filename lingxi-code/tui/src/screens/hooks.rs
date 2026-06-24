//! `/hooks` viewer (claude-code `hooks/` `HooksConfigMenu`): a read-only
//! list↔detail view of the configured hooks (name · event, with matcher +
//! timeout in the detail). Pure reducer over a selected index + a mode,
//! mirroring `agents.rs` / `mcp.rs`. The live path fills the rows from
//! `OrchestratorHandle::list_hooks`.
//!
//! SCOPE: this is the read-only VIEWER subset. claude-code's full `/hooks` is an
//! interactive CONFIGURATOR (add / edit / remove a hook per tool+event, writing
//! `settings.json`) — that needs a settings-write seam and stays deferred. The
//! viewer surfaces the same hook data (name / event / matcher / timeout) the
//! configurator opens onto.

/// One hook row. Pre-rendered/owned fields (the pump maps `HookInfo` into them)
/// so the carrying `Screen` variant keeps `PartialEq + Eq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookRow {
    /// Hook identifier.
    pub name: String,
    /// Hook event (e.g. `"PreToolUse"`, `"PostToolUse"`, `"Stop"`).
    pub event: String,
    /// Optional matcher regex (tool-name pattern); `None` = matches any tool.
    pub matcher: Option<String>,
    /// Timeout in milliseconds.
    pub timeout_ms: u64,
}

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HooksDialogMode {
    /// Browsing the hook list.
    #[default]
    List,
    /// Viewing one hook's detail.
    Detail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HooksScreenState {
    /// The configured-hook rows.
    pub rows: Vec<HookRow>,
    /// Selected row index.
    pub selected: usize,
    /// List or detail.
    pub mode: HooksDialogMode,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Reduce a key (mirrors `handle_agents_key`).
#[must_use]
pub fn handle_hooks_key(
    state: &mut HooksScreenState,
    key: crossterm::event::KeyCode,
) -> HooksOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        HooksDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                HooksOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1).min(state.rows.len() - 1);
                }
                HooksOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.mode = HooksDialogMode::Detail;
                }
                HooksOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => HooksOutcome::Close,
            _ => HooksOutcome::Stay,
        },
        HooksDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = HooksDialogMode::List;
                HooksOutcome::Stay
            }
            _ => HooksOutcome::Stay,
        },
    }
}

/// Render the screen body (claude-code hooks list / detail).
#[must_use]
pub fn render_hooks_to_string(state: &HooksScreenState) -> String {
    match state.mode {
        HooksDialogMode::List => {
            if state.rows.is_empty() {
                return "Hooks\nNo hooks configured.".to_string();
            }
            let n = state.rows.len();
            // Title + `{N} hook(s) configured` subtitle + dim read-only banner.
            let mut out = format!(
                "Hooks\n{n} {} configured\n\u{24d8} This menu is read-only. To add or modify hooks, edit settings.json directly or ask Claude to help.\n",
                if n == 1 { "hook" } else { "hooks" }
            );
            for (i, row) in state.rows.iter().enumerate() {
                let marker = if i == state.selected {
                    "\u{276F} "
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&row.name);
                out.push_str(&format!(" \u{00B7} {}", row.event));
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        HooksDialogMode::Detail => match state.rows.get(state.selected) {
            Some(row) => render_hook_detail(row),
            None => "Hooks\n(hook no longer available)".to_string(),
        },
    }
}

/// The detail body for one hook. claude-code titles it `Hook details` (not the
/// synthetic hook name), shows Event + Matcher, and uses `(all)` for an empty
/// matcher. (Type/Source aren't carried on `HookRow`; Timeout is dropped — TS
/// shows neither in the detail view.)
fn render_hook_detail(row: &HookRow) -> String {
    let mut out = String::from("Hook details\n");
    out.push_str(&format!("Event: {}\n", row.event));
    let matcher = row.matcher.as_deref().unwrap_or("(all)");
    out.push_str(&format!("Matcher: {matcher}"));
    out.push_str("\nEsc to go back");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn row(name: &str, event: &str) -> HookRow {
        HookRow {
            name: name.into(),
            event: event.into(),
            matcher: None,
            timeout_ms: 60_000,
        }
    }

    #[test]
    fn nav_enter_and_esc() {
        let mut s = HooksScreenState {
            rows: vec![row("fmt", "PreToolUse"), row("lint", "PostToolUse")],
            ..HooksScreenState::default()
        };
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Down), HooksOutcome::Stay);
        assert_eq!(s.selected, 1);
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Enter), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::Detail);
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Esc), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::List);
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Esc), HooksOutcome::Close);
    }

    #[test]
    fn list_render_marks_selection_and_event() {
        let s = HooksScreenState {
            rows: vec![row("fmt", "PreToolUse"), row("lint", "PostToolUse")],
            selected: 1,
            mode: HooksDialogMode::List,
        };
        let out = render_hooks_to_string(&s);
        assert!(out.starts_with(
            "Hooks\n2 hooks configured\n\u{24d8} This menu is read-only. To add or modify hooks, edit settings.json directly or ask Claude to help.\n  fmt \u{00B7} PreToolUse\n\u{276F} lint \u{00B7} PostToolUse\n"
        ));
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn empty_list_shows_locked_empty_state() {
        let out = render_hooks_to_string(&HooksScreenState::default());
        assert_eq!(out, "Hooks\nNo hooks configured.");
    }

    #[test]
    fn detail_render_fields_and_any_matcher() {
        let r = HookRow {
            name: "fmt".into(),
            event: "PreToolUse".into(),
            matcher: Some("Edit|Write".into()),
            timeout_ms: 30_000,
        };
        assert_eq!(
            render_hook_detail(&r),
            "Hook details\nEvent: PreToolUse\nMatcher: Edit|Write\nEsc to go back"
        );
        // None matcher renders the "(all)" placeholder.
        let any = render_hook_detail(&row("x", "Stop"));
        assert!(any.contains("Matcher: (all)"));
    }
}
