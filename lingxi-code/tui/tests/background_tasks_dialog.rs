//! M9-05 — `BackgroundTasksDialog` behavior + snapshots.

use tui::components::tasks::output_tail::OutputTailState;
use tui::multiagent::state::TaskRow;
use tui::screens::background_tasks::{
    render_background_tasks_to_string, BackgroundTasksState, TaskDialogMode,
};

fn rows() -> Vec<TaskRow> {
    vec![
        TaskRow {
            task_id: "b1".into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "cargo build".into(),
            command: None,
        },
        TaskRow {
            task_id: "b2".into(),
            task_type: "local_agent".into(),
            status: "completed".into(),
            description: "review".into(),
            command: None,
        },
    ]
}

#[test]
fn list_mode_snapshot() {
    let state = BackgroundTasksState::default();
    insta::assert_snapshot!(
        "bg_tasks_list",
        render_background_tasks_to_string(&state, &rows())
    );
}

#[test]
fn empty_list_snapshot() {
    let state = BackgroundTasksState::default();
    insta::assert_snapshot!(
        "bg_tasks_empty",
        render_background_tasks_to_string(&state, &[])
    );
}

#[test]
fn detail_mode_snapshot() {
    let state = BackgroundTasksState {
        selected: 0,
        mode: TaskDialogMode::Detail,
        detail_task_id: Some("b1".into()),
        tail: OutputTailState {
            content: "compiling...\ndone".into(),
            offset: 17,
            total_lines: 2,
            truncated: false,
        },
    };
    insta::assert_snapshot!(
        "bg_tasks_detail",
        render_background_tasks_to_string(&state, &rows())
    );
}

/// (M9-05 Task 5) Screen-routing seam: drive the SINGLE live-key dispatcher
/// `root::handle_live_key` — the exact function the live `use_terminal_events`
/// closure invokes — to prove the Shift+Down opener, in-screen nav/close, and
/// the priority ladder (a pending permission outranks the dialog). Mirrors
/// `behavior_doctor_screen.rs`.
mod routing_seam {
    use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use tui::multiagent::state::TaskRow;
    use tui::root::handle_live_key;
    use tui::screens::background_tasks::TaskDialogMode;
    use tui::screens::Screen;
    use tui::state::{AppState, StatusSnapshot};

    /// A printable / nav iocraft key with no modifiers (Press).
    fn key(code: KeyCode) -> KeyEvent {
        let mut k = KeyEvent::new(KeyEventKind::Press, code);
        k.modifiers = KeyModifiers::NONE;
        k
    }

    /// Shift+<code> iocraft key (Press).
    fn shift_key(code: KeyCode) -> KeyEvent {
        let mut k = KeyEvent::new(KeyEventKind::Press, code);
        k.modifiers = KeyModifiers::SHIFT;
        k
    }

    fn row(id: &str) -> TaskRow {
        TaskRow {
            task_id: id.into(),
            task_type: "local_bash".into(),
            status: "running".into(),
            description: "x".into(),
            command: None,
        }
    }

    /// An `AppState` with two live background tasks (so the dialog has rows).
    fn state_with_tasks() -> AppState {
        let mut st = AppState::new(StatusSnapshot::default());
        st.multiagent.tasks = vec![row("b1"), row("b2")];
        st
    }

    #[test]
    fn shift_down_opens_dialog_from_normal_editing() {
        let mut st = state_with_tasks();
        assert!(st.active_screen.is_none());
        handle_live_key(&mut st, &shift_key(KeyCode::Down), 24);
        assert!(
            matches!(st.active_screen, Some(Screen::BackgroundTasks(_))),
            "Shift+Down opens the background-tasks dialog"
        );
    }

    #[test]
    fn plain_down_does_not_open_dialog() {
        // A bare Down (no Shift) must NOT open the dialog — it stays in normal
        // editing (history-step / vertical-cursor land in the editor path).
        let mut st = state_with_tasks();
        handle_live_key(&mut st, &key(KeyCode::Down), 24);
        assert!(
            st.active_screen.is_none(),
            "plain Down must not open the dialog (Shift is required)"
        );
    }

    #[test]
    fn shift_down_does_not_leak_to_prompt() {
        let mut st = state_with_tasks();
        st.prompt_text = "draft".to_string();
        st.prompt_cursor = 5;
        handle_live_key(&mut st, &shift_key(KeyCode::Down), 24);
        assert_eq!(st.prompt_text, "draft", "opener must not touch the prompt");
    }

    #[test]
    fn nav_then_enter_then_esc_routes_through_dispatcher() {
        let mut st = state_with_tasks();
        handle_live_key(&mut st, &shift_key(KeyCode::Down), 24); // open
        handle_live_key(&mut st, &key(KeyCode::Down), 24); // select row 1
        match &st.active_screen {
            Some(Screen::BackgroundTasks(s)) => assert_eq!(s.selected, 1),
            other => panic!("expected open dialog, got {other:?}"),
        }
        handle_live_key(&mut st, &key(KeyCode::Enter), 24); // open detail
        match &st.active_screen {
            Some(Screen::BackgroundTasks(s)) => {
                assert_eq!(s.mode, TaskDialogMode::Detail);
                assert_eq!(s.detail_task_id.as_deref(), Some("b2"));
            }
            other => panic!("expected detail, got {other:?}"),
        }
        handle_live_key(&mut st, &key(KeyCode::Esc), 24); // detail Esc → back to list
        match &st.active_screen {
            Some(Screen::BackgroundTasks(s)) => assert_eq!(s.mode, TaskDialogMode::List),
            other => panic!("expected list, got {other:?}"),
        }
        handle_live_key(&mut st, &key(KeyCode::Esc), 24); // list Esc → close
        assert!(st.active_screen.is_none(), "Esc in list closes the dialog");
    }

    /// Priority ladder (§2.5): a pending permission (priority 1) outranks the
    /// open dialog (priority 2) — a key resolves the dialog's permission, never
    /// moves the screen selection.
    #[tokio::test]
    async fn permission_outranks_open_dialog() {
        use permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
        use serde_json::json;
        use std::time::Duration;
        use tokio::sync::oneshot;
        use tui::state::PendingPermission;

        let mut st = state_with_tasks();
        // Dialog is open...
        handle_live_key(&mut st, &shift_key(KeyCode::Down), 24);
        assert!(matches!(st.active_screen, Some(Screen::BackgroundTasks(_))));

        // ...and a permission arrives on top of it.
        let (tx, rx) = oneshot::channel();
        st.pending_permission = Some(PendingPermission {
            request: PermissionRequest::ToolUseConfirm {
                tool_name: "Bash".to_string(),
                tool_input: json!({"command": "ls"}),
                default_decision: PromptDefault::DenyByDefault,
            },
            worker: None,
        });
        st.pending_permission_resp_tx = Some(tx);
        st.pending_permission_started_at = Some(std::time::Instant::now());

        // Press `1` (ToolUseConfirm: AllowOnce). Priority 1 fires FIRST.
        handle_live_key(&mut st, &key(KeyCode::Char('1')), 24);

        let resp = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect("permission key must resolve the dialog (priority 1 > 2)")
            .expect("oneshot not dropped");
        assert_eq!(resp, PermissionResponse::AllowOnce);
        // The dialog screen is UNTOUCHED — selection did not move.
        match &st.active_screen {
            Some(Screen::BackgroundTasks(s)) => assert_eq!(s.selected, 0),
            other => panic!("screen must survive the permission key, got {other:?}"),
        }
    }
}
