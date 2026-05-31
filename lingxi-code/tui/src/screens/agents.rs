//! `/agents` discovery screen (claude-code `AgentsList.tsx` + `AgentDetail.tsx`):
//! a list of agent definitions with a per-agent detail view. Pure reducer over
//! a selected index + a mode (mirrors `background_tasks.rs`). The live path
//! fills `name`/`description`/`tools` from `OrchestratorHandle::list_agents`;
//! the richer detail fields are fixture/`None` (frozen trait, spec §2.4).

/// One agent row. `name`/`description`/`tools` come from the wire
/// (`AgentInfo`); the rest are optional detail fields (fixture-supplied).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentRow {
    /// Agent type / name.
    pub name: String,
    /// `when_to_use` description.
    pub description: String,
    /// Allowed tools (empty = all tools).
    pub tools: Vec<String>,
    /// Optional model display.
    pub model: Option<String>,
    /// Optional permission mode.
    pub permission_mode: Option<String>,
    /// Optional color name.
    pub color: Option<String>,
    /// Optional source/file path (shown dim atop the detail).
    pub path: Option<String>,
}

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentsDialogMode {
    /// Browsing the agent list.
    #[default]
    List,
    /// Viewing one agent's detail.
    Detail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentsScreenState {
    /// The agent catalog rows.
    pub rows: Vec<AgentRow>,
    /// Selected row index.
    pub selected: usize,
    /// List or detail.
    pub mode: AgentsDialogMode,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Reduce a key (mirrors `handle_background_tasks_key`).
#[must_use]
pub fn handle_agents_key(
    state: &mut AgentsScreenState,
    key: crossterm::event::KeyCode,
) -> AgentsOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        AgentsDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                AgentsOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1).min(state.rows.len() - 1);
                }
                AgentsOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.mode = AgentsDialogMode::Detail;
                }
                AgentsOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => AgentsOutcome::Close,
            _ => AgentsOutcome::Stay,
        },
        AgentsDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = AgentsDialogMode::List;
                AgentsOutcome::Stay
            }
            _ => AgentsOutcome::Stay,
        },
    }
}

/// Render the screen body (claude-code `AgentsList`/`AgentDetail`).
#[must_use]
pub fn render_agents_to_string(state: &AgentsScreenState) -> String {
    match state.mode {
        AgentsDialogMode::List => {
            let mut out = String::from("Agents\n");
            if state.rows.is_empty() {
                out.push_str("No agents found.");
                return out;
            }
            for (i, row) in state.rows.iter().enumerate() {
                let marker = if i == state.selected {
                    "\u{276F} "
                } else {
                    "  "
                };
                out.push_str(marker);
                out.push_str(&row.name);
                if let Some(m) = &row.model {
                    out.push_str(&format!(" \u{00B7} {m}"));
                }
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        AgentsDialogMode::Detail => match state.rows.get(state.selected) {
            Some(row) => render_agent_detail(row),
            None => "Agents\n(agent no longer available)".to_string(),
        },
    }
}

/// The detail body for one agent.
fn render_agent_detail(row: &AgentRow) -> String {
    let mut out = String::new();
    if let Some(p) = &row.path {
        out.push_str(p);
        out.push('\n');
    }
    out.push_str("Description (tells Claude when to use this agent):\n  ");
    out.push_str(&row.description);
    out.push('\n');
    let tools = if row.tools.is_empty() {
        "All tools".to_string()
    } else {
        row.tools.join(", ")
    };
    out.push_str(&format!("Tools: {tools}"));
    if let Some(m) = &row.model {
        out.push_str(&format!("\nModel: {m}"));
    }
    if let Some(pm) = &row.permission_mode {
        out.push_str(&format!("\nPermission mode: {pm}"));
    }
    if let Some(c) = &row.color {
        out.push_str(&format!("\nColor: {c}"));
    }
    out.push_str("\nesc to go back");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn row(name: &str) -> AgentRow {
        AgentRow {
            name: name.into(),
            description: "does things".into(),
            tools: vec![],
            ..AgentRow::default()
        }
    }

    #[test]
    fn nav_and_enter_and_esc() {
        let mut s = AgentsScreenState {
            rows: vec![row("a"), row("b")],
            ..AgentsScreenState::default()
        };
        assert_eq!(
            handle_agents_key(&mut s, KeyCode::Down),
            AgentsOutcome::Stay
        );
        assert_eq!(s.selected, 1);
        assert_eq!(
            handle_agents_key(&mut s, KeyCode::Enter),
            AgentsOutcome::Stay
        );
        assert_eq!(s.mode, AgentsDialogMode::Detail);
        // Esc in detail → back to list.
        assert_eq!(handle_agents_key(&mut s, KeyCode::Esc), AgentsOutcome::Stay);
        assert_eq!(s.mode, AgentsDialogMode::List);
        // Esc in list → close.
        assert_eq!(
            handle_agents_key(&mut s, KeyCode::Esc),
            AgentsOutcome::Close
        );
    }

    #[test]
    fn list_render_marks_selection_and_model_badge() {
        let mut rows = vec![row("explorer"), row("writer")];
        rows[0].model = Some("opus".into());
        let s = AgentsScreenState {
            rows,
            selected: 0,
            mode: AgentsDialogMode::List,
        };
        let out = render_agents_to_string(&s);
        assert!(out.starts_with("Agents\n\u{276F} explorer \u{00B7} opus\n  writer\n"));
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn detail_render_fields() {
        let r = AgentRow {
            name: "explorer".into(),
            description: "find things".into(),
            tools: vec!["Read".into(), "Grep".into()],
            model: Some("opus".into()),
            permission_mode: Some("plan".into()),
            color: Some("cyan".into()),
            path: Some(".lingxi/agents/explorer.md".into()),
        };
        let out = render_agent_detail(&r);
        assert_eq!(
            out,
            ".lingxi/agents/explorer.md\nDescription (tells Claude when to use this agent):\n  find things\nTools: Read, Grep\nModel: opus\nPermission mode: plan\nColor: cyan\nesc to go back"
        );
    }

    #[test]
    fn detail_empty_tools_is_all() {
        let out = render_agent_detail(&row("x"));
        assert!(out.contains("Tools: All tools"));
    }
}
