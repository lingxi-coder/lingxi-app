//! `/mcp` server viewer (claude-code `mcp/` `MCPSettings`): a read-only
//! list↔detail view of the configured MCP servers (name · status · transport).
//! Pure reducer over a selected index + a mode, mirroring `agents.rs`. The live
//! path fills the rows from `OrchestratorHandle::list_mcp_servers`.
//!
//! SCOPE: this is the read-only VIEWER subset. claude-code's full `/mcp` is an
//! interactive MANAGER (connect / reconnect / authenticate / enable-disable a
//! server, drive live server processes / OAuth) — those actions need
//! connection-manager + auth seams that are out of reach for a local port and
//! stay deferred. The viewer surfaces the same server/status/transport data the
//! manager opens onto.

/// One MCP-server row. All fields are pre-rendered strings (the pump maps
/// `McpServerInfo`/`McpStatus` into them), so the screen stays free of the
/// `traits` types and the carrying `Screen` variant keeps `PartialEq + Eq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpRow {
    /// Server name as registered in settings.
    pub name: String,
    /// Rendered connection status: `"connected"` / `"disconnected"` /
    /// `"error: <reason>"`.
    pub status: String,
    /// Transport kind: `"stdio"` / `"sse"` / `"http"`.
    pub transport: String,
}

/// List vs. detail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum McpDialogMode {
    /// Browsing the server list.
    #[default]
    List,
    /// Viewing one server's detail.
    Detail,
}

/// Screen state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpScreenState {
    /// The configured-server rows.
    pub rows: Vec<McpRow>,
    /// Selected row index.
    pub selected: usize,
    /// List or detail.
    pub mode: McpDialogMode,
}

/// Controller outcome after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOutcome {
    /// Stay open.
    Stay,
    /// Close the screen.
    Close,
}

/// Reduce a key (mirrors `handle_agents_key`).
#[must_use]
pub fn handle_mcp_key(state: &mut McpScreenState, key: crossterm::event::KeyCode) -> McpOutcome {
    use crossterm::event::KeyCode;
    match state.mode {
        McpDialogMode::List => match key {
            KeyCode::Up | KeyCode::Char('k') => {
                state.selected = state.selected.saturating_sub(1);
                McpOutcome::Stay
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !state.rows.is_empty() {
                    state.selected = (state.selected + 1).min(state.rows.len() - 1);
                }
                McpOutcome::Stay
            }
            KeyCode::Enter => {
                if !state.rows.is_empty() {
                    state.mode = McpDialogMode::Detail;
                }
                McpOutcome::Stay
            }
            KeyCode::Esc | KeyCode::Char('q') => McpOutcome::Close,
            _ => McpOutcome::Stay,
        },
        McpDialogMode::Detail => match key {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => {
                state.mode = McpDialogMode::List;
                McpOutcome::Stay
            }
            _ => McpOutcome::Stay,
        },
    }
}

/// Render the screen body (claude-code MCP server list / detail).
#[must_use]
pub fn render_mcp_to_string(state: &McpScreenState) -> String {
    match state.mode {
        McpDialogMode::List => {
            let mut out = String::from("MCP Servers\n");
            if state.rows.is_empty() {
                out.push_str("No MCP servers configured.");
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
                out.push_str(&format!(" \u{00B7} {}", row.status));
                out.push('\n');
            }
            out.push_str("Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back");
            out
        }
        McpDialogMode::Detail => match state.rows.get(state.selected) {
            Some(row) => render_mcp_detail(row),
            None => "MCP Servers\n(server no longer available)".to_string(),
        },
    }
}

/// The detail body for one server.
fn render_mcp_detail(row: &McpRow) -> String {
    let mut out = String::new();
    out.push_str(&row.name);
    out.push('\n');
    out.push_str(&format!("Status: {}\n", row.status));
    out.push_str(&format!("Transport: {}", row.transport));
    out.push_str("\nesc to go back");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    fn row(name: &str, status: &str) -> McpRow {
        McpRow {
            name: name.into(),
            status: status.into(),
            transport: "stdio".into(),
        }
    }

    #[test]
    fn nav_enter_and_esc() {
        let mut s = McpScreenState {
            rows: vec![row("a", "connected"), row("b", "disconnected")],
            ..McpScreenState::default()
        };
        assert_eq!(handle_mcp_key(&mut s, KeyCode::Down), McpOutcome::Stay);
        assert_eq!(s.selected, 1);
        assert_eq!(handle_mcp_key(&mut s, KeyCode::Enter), McpOutcome::Stay);
        assert_eq!(s.mode, McpDialogMode::Detail);
        // Esc in detail → back to list.
        assert_eq!(handle_mcp_key(&mut s, KeyCode::Esc), McpOutcome::Stay);
        assert_eq!(s.mode, McpDialogMode::List);
        // Esc in list → close.
        assert_eq!(handle_mcp_key(&mut s, KeyCode::Esc), McpOutcome::Close);
    }

    #[test]
    fn down_clamps_and_empty_is_inert() {
        let mut empty = McpScreenState::default();
        assert_eq!(handle_mcp_key(&mut empty, KeyCode::Down), McpOutcome::Stay);
        assert_eq!(empty.selected, 0);
        // Enter on empty does NOT enter detail.
        assert_eq!(handle_mcp_key(&mut empty, KeyCode::Enter), McpOutcome::Stay);
        assert_eq!(empty.mode, McpDialogMode::List);
    }

    #[test]
    fn list_render_marks_selection_and_status() {
        let s = McpScreenState {
            rows: vec![row("alpha", "connected"), row("beta", "error: boom")],
            selected: 0,
            mode: McpDialogMode::List,
        };
        let out = render_mcp_to_string(&s);
        assert!(out.starts_with(
            "MCP Servers\n\u{276F} alpha \u{00B7} connected\n  beta \u{00B7} error: boom\n"
        ));
        assert!(out.ends_with(
            "Press \u{2191}\u{2193} to navigate \u{00B7} Enter to select \u{00B7} Esc to go back"
        ));
    }

    #[test]
    fn empty_list_shows_locked_empty_state() {
        let out = render_mcp_to_string(&McpScreenState::default());
        assert_eq!(out, "MCP Servers\nNo MCP servers configured.");
    }

    #[test]
    fn detail_render_fields() {
        let r = McpRow {
            name: "fs".into(),
            status: "connected".into(),
            transport: "stdio".into(),
        };
        assert_eq!(
            render_mcp_detail(&r),
            "fs\nStatus: connected\nTransport: stdio\nesc to go back"
        );
    }
}
