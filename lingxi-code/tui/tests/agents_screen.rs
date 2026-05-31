//! M9-08 — agents screen snapshots.

use tui::screens::agents::{
    render_agents_to_string, AgentRow, AgentsDialogMode, AgentsScreenState,
};

fn rows() -> Vec<AgentRow> {
    vec![
        AgentRow {
            name: "explorer".into(),
            description: "find things".into(),
            tools: vec!["Read".into(), "Grep".into()],
            model: Some("opus".into()),
            permission_mode: Some("plan".into()),
            color: Some("cyan".into()),
            path: Some(".lingxi/agents/explorer.md".into()),
        },
        AgentRow {
            name: "writer".into(),
            description: "writes code".into(),
            tools: vec![],
            ..AgentRow::default()
        },
    ]
}

#[test]
fn agents_list_snapshot() {
    let s = AgentsScreenState {
        rows: rows(),
        selected: 0,
        mode: AgentsDialogMode::List,
    };
    insta::assert_snapshot!("agents_list", render_agents_to_string(&s));
}

#[test]
fn agents_detail_snapshot() {
    let s = AgentsScreenState {
        rows: rows(),
        selected: 0,
        mode: AgentsDialogMode::Detail,
    };
    insta::assert_snapshot!("agents_detail", render_agents_to_string(&s));
}

/// Behavior: `open_agents` sets `active_screen` to `Screen::Agents`.
#[test]
fn open_agents_sets_active_screen() {
    use tui::screens::Screen;
    use tui::state::AppState;
    let mut st = AppState::default_for_tests();
    assert_eq!(st.active_screen, None);
    st.open_agents(rows());
    assert!(
        matches!(&st.active_screen, Some(Screen::Agents(s)) if s.rows.len() == 2 && s.selected == 0),
        "open_agents must set active_screen to Screen::Agents with the given rows"
    );
}
