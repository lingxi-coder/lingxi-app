//! M9-07 — worker-permission chrome snapshots + cross-state-seam test.

use tui::components::permissions::worker::{render_worker_badge, render_worker_pending_to_string};

#[test]
fn worker_badge_snapshot() {
    insta::assert_snapshot!("worker_badge", render_worker_badge("alice"));
}

#[test]
fn worker_pending_snapshot() {
    insta::assert_snapshot!(
        "worker_pending_full",
        render_worker_pending_to_string("Bash", "run mkdir /tmp/x", Some("alice"), Some("my-team"))
    );
}

mod seam {
    use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind};
    use permission::gate::{PermissionRequest, PromptDefault};
    use serde_json::json;
    use tui::components::permissions::worker::WorkerPermissionInfo;
    use tui::root::handle_live_key;
    use tui::screens::Screen;
    use tui::state::{AppState, PendingPermission, StatusSnapshot};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(KeyEventKind::Press, code)
    }

    fn fresh() -> AppState {
        AppState::new(StatusSnapshot::default())
    }

    fn arm_worker_permission(st: &mut AppState) {
        st.pending_permission = Some(PendingPermission {
            request: PermissionRequest::ToolUseConfirm {
                tool_name: "Bash".to_string(),
                tool_input: json!({"command": "ls"}),
                default_decision: PromptDefault::DenyByDefault,
            },
            worker: Some(WorkerPermissionInfo {
                name: "alice".into(),
                color: "magenta".into(),
                team: Some("my-team".into()),
            }),
        });
        st.pending_permission_started_at = Some(std::time::Instant::now());
    }

    #[test]
    fn worker_permission_wins_over_open_screen() {
        let mut st = fresh();
        st.open_doctor(tui::screens::doctor::DoctorDiagnostics::capture(
            std::path::Path::new("/work"),
            0,
            0,
            (80, 24),
        ));
        assert!(matches!(st.active_screen, Some(Screen::Doctor(_))));
        arm_worker_permission(&mut st);

        // `q` would close a Doctor screen (priority 2); but priority 1 owns it.
        handle_live_key(&mut st, &key(KeyCode::Char('q')), 24);

        assert!(
            st.pending_permission.is_some(),
            "permission (priority 1) still owns the keys — `q` did not reach the screen"
        );
        assert!(
            matches!(st.active_screen, Some(Screen::Doctor(_))),
            "the Doctor screen is intact — the permission key did not close it"
        );
        assert!(
            st.pending_permission.as_ref().unwrap().worker.is_some(),
            "the worker tag is preserved on the pending permission"
        );
    }

    #[test]
    fn worker_permission_resolves_and_clears() {
        let mut st = fresh();
        arm_worker_permission(&mut st);
        assert!(st.pending_permission.is_some());

        // `1` = AllowOnce — resolves and clears the dialog.
        handle_live_key(&mut st, &key(KeyCode::Char('1')), 24);

        assert!(
            st.pending_permission.is_none(),
            "the worker permission was resolved and the slot cleared"
        );
    }
}
