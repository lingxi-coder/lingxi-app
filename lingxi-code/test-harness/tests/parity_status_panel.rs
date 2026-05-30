//! Byte-locked /status panel layout vs golden text fixture (M5-11 T14).

use commands::builtin::status::render_status;
use traits::StatusSnapshot;

const GOLDEN: &str = include_str!("../src/parity/fixtures/parity_status_panel.txt");

#[test]
fn status_panel_layout_matches_golden() {
    let snap = StatusSnapshot {
        session_id: "abc-123".into(),
        model: "claude-opus-4-7".into(),
        n_messages: 17,
        total_cost_usd: 0.0421,
        input_tokens: 4_500,
        output_tokens: 1_200,
        n_mcp_connected: 1,
        n_mcp_total: 2,
        n_hooks: 3,
        n_agents: 5,
        started_at: "2026-05-26T10:00:00Z".into(),
        cwd: std::path::PathBuf::from("/repo"),
    };
    assert_eq!(render_status(&snap), GOLDEN);
}
