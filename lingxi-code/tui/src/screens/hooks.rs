//! `/hooks` viewer (claude-code `hooks/` `HooksConfigMenu`): an EVENTS-FIRST
//! read-only browser. The top level lists the hook EVENTS that have configured
//! hooks — each `{Event} ({count}) — {summary}` (claude-code `SelectEventMode`)
//! — and drilling into an event shows that event's hooks (name + matcher).
//! Pure reducer over a selected index + a mode, mirroring `agents.rs`/`mcp.rs`.
//! The live path fills the rows from `OrchestratorHandle::list_hooks`.
//!
//! SCOPE: read-only VIEWER subset. claude-code's full `/hooks` is an interactive
//! CONFIGURATOR (add / edit / remove per tool+event, writing `settings.json`)
//! with a deeper event→matcher→hook→detail drill — that needs a settings-write
//! seam and stays deferred. This viewer collapses the matcher/hook/detail levels
//! into the per-event hook list.

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
    /// (hooks-detail-fields-divergent) Executor kind: `"command"` / `"http"`
    /// / `"agent"` / `"prompt"` / the LingXi-only `"builtin"`.
    pub hook_type: String,
    /// (hooks-detail-fields-divergent) Human-readable origin, e.g. `"User
    /// settings (~/.claude/settings.json)"`.
    pub source: String,
    /// (hooks-detail-fields-divergent) The executor's primary content
    /// (command line / URL / prompt / handler id).
    pub content: String,
    /// (hooks-detail-fields-divergent) Custom status message, if set.
    pub status_message: Option<String>,
}

/// `(event, summary)` catalog (claude-code `HookEventMetadata`), in the canonical
/// `SelectEventMode` order. Drives both the event ordering and the per-event
/// summary line. Events absent here still list (with an empty summary).
pub const HOOK_EVENT_SUMMARY: &[(&str, &str)] = &[
    ("PreToolUse", "Before tool execution"),
    ("PostToolUse", "After tool execution"),
    ("PostToolUseFailure", "After tool execution fails"),
    ("PermissionDenied", "After auto mode classifier denies a tool call"),
    ("PermissionRequest", "When a permission dialog is displayed"),
    ("Notification", "When notifications are sent"),
    ("UserPromptSubmit", "When the user submits a prompt"),
    ("SessionStart", "When a new session is started"),
    ("SessionEnd", "When a session is ending"),
    ("Stop", "Right before Claude concludes its response"),
    ("StopFailure", "When the turn ends due to an API error"),
    ("SubagentStart", "When a subagent (Agent tool call) is started"),
    ("SubagentStop", "When a subagent (Agent tool call) stops"),
    ("PreCompact", "Before conversation compaction"),
    ("PostCompact", "After conversation compaction"),
    ("Setup", "Repo setup hooks for init and maintenance"),
];

/// The summary line for `event`, or `""` when the event has no catalog entry.
#[must_use]
pub fn event_summary(event: &str) -> &'static str {
    HOOK_EVENT_SUMMARY
        .iter()
        .find(|(name, _)| *name == event)
        .map_or("", |(_, summary)| *summary)
}

/// Sort key placing known events in catalog order, unknown events after (stable
/// by first appearance via the count map's insertion).
fn event_order(event: &str) -> usize {
    HOOK_EVENT_SUMMARY
        .iter()
        .position(|(name, _)| *name == event)
        .unwrap_or(usize::MAX)
}

/// Event-list browsing vs. one event's hook list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HooksDialogMode {
    /// Browsing the events (top level).
    #[default]
    EventList,
    /// Viewing the hooks configured for one event.
    EventHooks,
    /// (hooks-detail-fields-divergent) Viewing one hook's full detail
    /// (claude-code `ViewHookMode`).
    HookDetail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HooksScreenState {
    /// The configured-hook rows.
    pub rows: Vec<HookRow>,
    /// Selected index (an event in `EventList`, a hook in `EventHooks`).
    pub selected: usize,
    /// Event-list or per-event hook list.
    pub mode: HooksDialogMode,
    /// The event drilled into (set in `EventHooks`).
    pub selected_event: Option<String>,
}

impl HooksScreenState {
    /// Distinct events that have ≥1 configured hook, in catalog order, paired
    /// with their hook count.
    #[must_use]
    pub fn events(&self) -> Vec<(String, usize)> {
        let mut order: Vec<String> = Vec::new();
        let mut counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &self.rows {
            if !counts.contains_key(&row.event) {
                order.push(row.event.clone());
            }
            *counts.entry(row.event.clone()).or_insert(0) += 1;
        }
        order.sort_by_key(|e| (event_order(e), e.clone()));
        order
            .into_iter()
            .map(|e| {
                let n = counts[&e];
                (e, n)
            })
            .collect()
    }

    /// The hook rows belonging to `selected_event` (in wire order).
    #[must_use]
    pub fn hooks_for_selected_event(&self) -> Vec<&HookRow> {
        match &self.selected_event {
            Some(ev) => self.rows.iter().filter(|r| &r.event == ev).collect(),
            None => Vec::new(),
        }
    }

    /// (hooks-detail-fields-divergent) The hook `selected` indexes into
    /// within [`Self::hooks_for_selected_event`] — the open `HookDetail`.
    #[must_use]
    pub fn selected_hook(&self) -> Option<&HookRow> {
        self.hooks_for_selected_event()
            .into_iter()
            .nth(self.selected)
    }
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HooksOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Reduce a key (arrow nav + Esc, mirroring `handle_agents_key`).
#[must_use]
pub fn handle_hooks_key(
    state: &mut HooksScreenState,
    key: crossterm::event::KeyCode,
) -> HooksOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        HooksDialogMode::EventList => match key {
            KeyCode::Up => {
                state.selected = state.selected.saturating_sub(1);
                HooksOutcome::Stay
            }
            KeyCode::Down => {
                let max = state.events().len();
                if max > 0 {
                    state.selected = (state.selected + 1).min(max - 1);
                }
                HooksOutcome::Stay
            }
            KeyCode::Enter => {
                let events = state.events();
                if let Some((event, _)) = events.get(state.selected) {
                    state.selected_event = Some(event.clone());
                    state.mode = HooksDialogMode::EventHooks;
                    state.selected = 0;
                }
                HooksOutcome::Stay
            }
            KeyCode::Esc => HooksOutcome::Close,
            _ => HooksOutcome::Stay,
        },
        HooksDialogMode::EventHooks => match key {
            KeyCode::Up => {
                state.selected = state.selected.saturating_sub(1);
                HooksOutcome::Stay
            }
            KeyCode::Down => {
                let max = state.hooks_for_selected_event().len();
                if max > 0 {
                    state.selected = (state.selected + 1).min(max - 1);
                }
                HooksOutcome::Stay
            }
            // (hooks-detail-fields-divergent) Enter drills into the
            // selected hook's full detail (claude-code `ViewHookMode`).
            KeyCode::Enter => {
                if !state.hooks_for_selected_event().is_empty() {
                    state.mode = HooksDialogMode::HookDetail;
                }
                HooksOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Left => {
                state.mode = HooksDialogMode::EventList;
                state.selected_event = None;
                state.selected = 0;
                HooksOutcome::Stay
            }
            _ => HooksOutcome::Stay,
        },
        HooksDialogMode::HookDetail => match key {
            KeyCode::Esc | KeyCode::Left => {
                state.mode = HooksDialogMode::EventHooks;
                HooksOutcome::Stay
            }
            _ => HooksOutcome::Stay,
        },
    }
}

/// Render the screen body (claude-code `SelectEventMode` / per-event hook list).
#[must_use]
pub fn render_hooks_to_string(state: &HooksScreenState) -> String {
    match state.mode {
        HooksDialogMode::EventList => {
            if state.rows.is_empty() {
                return "Hooks\nNo hooks configured.".to_string();
            }
            let total = state.rows.len();
            // Title + `{N} hook(s) configured` subtitle + dim read-only banner.
            let mut out = format!(
                "Hooks\n{total} {} configured\n\u{24d8} This menu is read-only. To add or modify hooks, edit settings.json directly or ask Claude to help.\n",
                if total == 1 { "hook" } else { "hooks" }
            );
            // (hooks-events-first) one row per event: `{Event} ({count}) — {summary}`.
            for (i, (event, count)) in state.events().iter().enumerate() {
                let marker = if i == state.selected { "\u{276F} " } else { "  " };
                out.push_str(marker);
                out.push_str(&format!("{event} ({count})"));
                let summary = event_summary(event);
                if !summary.is_empty() {
                    out.push_str(&format!(" \u{2014} {summary}"));
                }
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        HooksDialogMode::EventHooks => {
            let Some(event) = state.selected_event.as_deref() else {
                return "Hooks\n(event no longer available)".to_string();
            };
            let hooks = state.hooks_for_selected_event();
            // Header `{Event} hooks` + summary, then one `{name} · {matcher}`
            // line per configured hook (`(all)` for an unset matcher).
            let mut out = format!("{event} hooks\n");
            let summary = event_summary(event);
            if !summary.is_empty() {
                out.push_str(&format!("{summary}\n"));
            }
            for (i, row) in hooks.iter().enumerate() {
                let marker = if i == state.selected { "\u{276F} " } else { "  " };
                let matcher = row.matcher.as_deref().unwrap_or("(all)");
                out.push_str(&format!("{marker}{} \u{00B7} {matcher}\n", row.name));
            }
            out.push_str(
                "Press \u{2191}\u{2193} to navigate \u{00B7} Enter for details \u{00B7} Esc to go back",
            );
            out
        }
        // (hooks-detail-fields-divergent) claude-code `ViewHookMode`.
        HooksDialogMode::HookDetail => {
            let Some(row) = state.selected_hook() else {
                return "Hook details\n(hook no longer available)".to_string();
            };
            let mut out = String::from("Hook details\n");
            out.push_str(&format!("Event: {}\n", row.event));
            let matcher = row.matcher.as_deref().unwrap_or("(all)");
            out.push_str(&format!("Matcher: {matcher}\n"));
            out.push_str(&format!("Type: {}\n", row.hook_type));
            out.push_str(&format!("Source: {}\n", row.source));
            let content_label = match row.hook_type.as_str() {
                "command" => "Command",
                "http" => "URL",
                "agent" | "prompt" => "Prompt",
                _ => "Content",
            };
            out.push_str(&format!("{content_label}: {}\n", row.content));
            if let Some(msg) = row.status_message.as_deref() {
                out.push_str(&format!("Status message: {msg}\n"));
            }
            out.push_str(
                "To modify or remove this hook, edit settings.json directly or ask Claude to help.\n",
            );
            out.push_str("Esc to go back");
            out
        }
    }
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
            ..HookRow::default()
        }
    }

    #[test]
    fn drill_into_event_then_back_and_close() {
        let mut s = HooksScreenState {
            rows: vec![row("fmt", "PreToolUse"), row("lint", "PostToolUse")],
            ..HooksScreenState::default()
        };
        // Two distinct events at the top.
        assert_eq!(s.events().len(), 2);
        // Down selects the second event (PostToolUse).
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Down), HooksOutcome::Stay);
        assert_eq!(s.selected, 1);
        // Enter drills into it.
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Enter), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::EventHooks);
        assert_eq!(s.selected_event.as_deref(), Some("PostToolUse"));
        // Esc backs out to the event list.
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Esc), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::EventList);
        assert!(s.selected_event.is_none());
        // Esc from the event list closes.
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Esc), HooksOutcome::Close);
    }

    #[test]
    fn event_list_groups_by_event_with_count_and_summary() {
        let s = HooksScreenState {
            rows: vec![
                row("fmt", "PreToolUse"),
                row("guard", "PreToolUse"),
                row("lint", "PostToolUse"),
            ],
            selected: 0,
            ..HooksScreenState::default()
        };
        let out = render_hooks_to_string(&s);
        assert!(out.starts_with(
            "Hooks\n3 hooks configured\n\u{24d8} This menu is read-only. To add or modify hooks, edit settings.json directly or ask Claude to help.\n\u{276F} PreToolUse (2) \u{2014} Before tool execution\n  PostToolUse (1) \u{2014} After tool execution\n"
        ), "got: {out}");
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn event_hooks_lists_hooks_with_matchers() {
        let s = HooksScreenState {
            rows: vec![
                HookRow {
                    name: "fmt".into(),
                    event: "PreToolUse".into(),
                    matcher: Some("Edit|Write".into()),
                    timeout_ms: 0,
                    ..HookRow::default()
                },
                row("guard", "PreToolUse"),
            ],
            selected: 0,
            mode: HooksDialogMode::EventHooks,
            selected_event: Some("PreToolUse".into()),
        };
        let out = render_hooks_to_string(&s);
        assert_eq!(
            out,
            "PreToolUse hooks\nBefore tool execution\n\u{276F} fmt \u{00B7} Edit|Write\n  guard \u{00B7} (all)\nPress \u{2191}\u{2193} to navigate \u{00B7} Enter for details \u{00B7} Esc to go back"
        );
    }

    #[test]
    fn empty_list_shows_locked_empty_state() {
        let out = render_hooks_to_string(&HooksScreenState::default());
        assert_eq!(out, "Hooks\nNo hooks configured.");
    }

    #[test]
    fn enter_on_a_hook_opens_detail_with_all_fields() {
        // (hooks-detail-fields-divergent)
        let mut s = HooksScreenState {
            rows: vec![HookRow {
                name: "fmt".into(),
                event: "PreToolUse".into(),
                matcher: Some("Edit|Write".into()),
                timeout_ms: 5_000,
                hook_type: "command".into(),
                source: "Project settings (.claude/settings.json)".into(),
                content: "prettier --write".into(),
                status_message: Some("Formatting…".into()),
            }],
            mode: HooksDialogMode::EventHooks,
            selected_event: Some("PreToolUse".into()),
            ..HooksScreenState::default()
        };
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Enter), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::HookDetail);
        let out = render_hooks_to_string(&s);
        assert_eq!(
            out,
            "Hook details\n\
             Event: PreToolUse\n\
             Matcher: Edit|Write\n\
             Type: command\n\
             Source: Project settings (.claude/settings.json)\n\
             Command: prettier --write\n\
             Status message: Formatting\u{2026}\n\
             To modify or remove this hook, edit settings.json directly or ask Claude to help.\n\
             Esc to go back"
        );
        // Esc backs out to the per-event list, NOT the top-level event list.
        assert_eq!(handle_hooks_key(&mut s, KeyCode::Esc), HooksOutcome::Stay);
        assert_eq!(s.mode, HooksDialogMode::EventHooks);
    }

    #[test]
    fn events_sorted_by_catalog_order() {
        // PostToolUse appears before PreToolUse in the rows but PreToolUse is
        // first in the catalog → it sorts first.
        let s = HooksScreenState {
            rows: vec![row("a", "PostToolUse"), row("b", "PreToolUse")],
            ..HooksScreenState::default()
        };
        let events = s.events();
        assert_eq!(events[0].0, "PreToolUse");
        assert_eq!(events[1].0, "PostToolUse");
    }
}
