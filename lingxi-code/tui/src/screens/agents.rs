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
    /// Allowed tools. Meaningful only when `wildcard_tools` is `false`.
    pub tools: Vec<String>,
    /// (agents-03) `true` when the agent inherits every tool (claude-code:
    /// `tools` frontmatter omitted) — distinguishes "All tools" from an
    /// explicit empty allow-list ("None"), which `tools` alone can't.
    pub wildcard_tools: bool,
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
        // (agents-06) Arrow-only nav + Esc-only close — claude-code's AgentsList
        // binds no j/k/q. (agents-05) Selection WRAPS at both ends.
        AgentsDialogMode::List => match key {
            KeyCode::Up => {
                if !state.rows.is_empty() {
                    state.selected = if state.selected == 0 {
                        state.rows.len() - 1
                    } else {
                        state.selected - 1
                    };
                }
                AgentsOutcome::Stay
            }
            KeyCode::Down => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1) % state.rows.len();
                }
                AgentsOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.mode = AgentsDialogMode::Detail;
                }
                AgentsOutcome::Stay
            }
            KeyCode::Esc => AgentsOutcome::Close,
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
            if state.rows.is_empty() {
                // claude-code `AgentsList` empty state: a "No agents found"
                // subtitle plus three dim help lines (verbatim).
                return "Agents\n\
                    No agents found\n\
                    No agents found. Create specialized subagents that Claude can delegate to.\n\
                    Each subagent has its own context window, custom system prompt, and specific tools.\n\
                    Try creating: Code Reviewer, Code Simplifier, Security Reviewer, Tech Lead, or UX Reviewer."
                    .to_string();
            }
            // Title + dim `{count} agents` subtitle (claude-code `AgentsList`).
            let mut out = format!("Agents\n{} agents\n", state.rows.len());
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
    // (agents-03) Wildcard -> "All tools"; truly-empty explicit list -> "None".
    let tools = if row.wildcard_tools {
        "All tools".to_string()
    } else if row.tools.is_empty() {
        "None".to_string()
    } else {
        row.tools.join(", ")
    };
    out.push_str(&format!("Tools: {tools}"));
    // (agents-04) Always render the Model line (claude-code `getAgentModelDisplay`):
    // unset → "Inherit from parent (default)", "inherit" → "Inherit from parent",
    // else the capitalized model string.
    out.push_str(&format!("\nModel: {}", agent_model_display(row.model.as_deref())));
    if let Some(pm) = &row.permission_mode {
        out.push_str(&format!("\nPermission mode: {pm}"));
    }
    if let Some(c) = &row.color {
        out.push_str(&format!("\nColor: {c}"));
    }
    // (agents-07) No detail footer — claude-code's AgentDetail shows none.
    out
}

/// claude-code `getAgentModelDisplay`: the agent-detail Model line value.
fn agent_model_display(model: Option<&str>) -> String {
    match model {
        None => "Inherit from parent (default)".to_string(),
        Some("inherit") => "Inherit from parent".to_string(),
        Some(m) => capitalize(m),
    }
}

/// claude-code `capitalize` — uppercase the first character, rest unchanged.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
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
        assert!(out.starts_with("Agents\n2 agents\n\u{276F} explorer \u{00B7} opus\n  writer\n"));
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
            wildcard_tools: false,
            model: Some("opus".into()),
            permission_mode: Some("plan".into()),
            color: Some("cyan".into()),
            path: Some(".lingxi/agents/explorer.md".into()),
        };
        let out = render_agent_detail(&r);
        // (agents-04) Model is capitalized; (agents-07) no trailing footer.
        assert_eq!(
            out,
            ".lingxi/agents/explorer.md\nDescription (tells Claude when to use this agent):\n  find things\nTools: Read, Grep\nModel: Opus\nPermission mode: plan\nColor: cyan"
        );
    }

    #[test]
    fn detail_wildcard_tools_is_all() {
        // (agents-03) wildcard_tools=true -> "All tools".
        let mut r = row("x");
        r.wildcard_tools = true;
        let out = render_agent_detail(&r);
        assert!(out.contains("Tools: All tools"));
    }

    #[test]
    fn detail_explicit_empty_tools_is_none() {
        // (agents-03) An explicit empty allow-list (wildcard_tools=false,
        // tools=[]) means "no tools", not "all tools".
        let out = render_agent_detail(&row("x"));
        assert!(!row("x").wildcard_tools);
        assert!(out.contains("Tools: None"));
    }

    #[test]
    fn detail_model_always_shown_with_inherit_defaults() {
        // (agents-04) Unset model → "Inherit from parent (default)".
        let out = render_agent_detail(&row("x"));
        assert!(out.ends_with("\nModel: Inherit from parent (default)"), "got: {out}");
        // "inherit" → "Inherit from parent" (no "(default)" suffix).
        let mut r = row("x");
        r.model = Some("inherit".into());
        assert!(render_agent_detail(&r).ends_with("\nModel: Inherit from parent"));
    }

    #[test]
    fn list_nav_wraps_at_both_ends() {
        // (agents-05) Up at the top wraps to the last row; Down at the bottom
        // wraps to the first.
        let mut s = AgentsScreenState {
            rows: vec![row("a"), row("b"), row("c")],
            ..AgentsScreenState::default()
        };
        handle_agents_key(&mut s, KeyCode::Up); // 0 → 2 (wrap)
        assert_eq!(s.selected, 2);
        handle_agents_key(&mut s, KeyCode::Down); // 2 → 0 (wrap)
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn list_ignores_vim_keys() {
        // (agents-06) j/k/q are not bound in the list.
        let mut s = AgentsScreenState {
            rows: vec![row("a"), row("b")],
            ..AgentsScreenState::default()
        };
        assert_eq!(handle_agents_key(&mut s, KeyCode::Char('j')), AgentsOutcome::Stay);
        assert_eq!(s.selected, 0, "j must not move the selection");
        assert_eq!(handle_agents_key(&mut s, KeyCode::Char('q')), AgentsOutcome::Stay);
        // q does not close.
    }
}
