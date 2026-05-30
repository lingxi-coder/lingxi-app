//! Session state model. Persistence lives in `lingxi-session` (Plan 10).

use crate::token::Usage;
use protocol::{ConversationMessage, SessionId};
use serde::{Deserialize, Serialize};

/// Running total of `Usage` across all turns in a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CumulativeUsage(pub Usage);

impl CumulativeUsage {
    /// Accumulate a per-turn `Usage` into the cumulative total.
    pub fn add(&mut self, u: &Usage) {
        self.0.add(u);
    }
}

/// One entry in a session's todo list. Wire-locked by spec §7 line 486.
///
/// `status` serialises to one of three literals: `"pending"`, `"in_progress"`,
/// or `"completed"` — see [`TodoState`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    /// Stable identifier (caller-supplied; uniqueness enforced by `TodoWriteTool`).
    pub id: String,
    /// Human-readable description of the todo.
    pub content: String,
    /// Lifecycle state of this todo. Wire-locked.
    pub status: TodoState,
}

/// Wire-locked todo lifecycle states.
///
/// Serialises to `"pending"` / `"in_progress"` / `"completed"` per
/// spec §7 line 486. The aliases `"done"` and `"todo"` are explicitly
/// rejected by `TodoWriteTool`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoState {
    /// Not yet started.
    Pending,
    /// Actively being worked on (at most one per todo list).
    InProgress,
    /// Finished.
    Completed,
}

/// In-memory model of a conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Unique identifier for this session.
    pub session_id: SessionId,
    /// Ordered conversation history.
    pub history: Vec<ConversationMessage>,
    /// Cumulative token usage across all turns.
    pub usage: CumulativeUsage,
    /// Model identifier (e.g. `"claude-opus-4-7"`).
    pub model: String,
    /// Active todo list (M4-04). Mutated by `TodoWriteTool`. Defaults to empty
    /// on deserialize so pre-M4-04 persisted sessions still load.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// Plan-mode flag (M4-04). Flipped by `EnterPlanModeTool` /
    /// `ExitPlanModeTool`. Defaults to `false` on deserialize.
    #[serde(default)]
    pub plan_mode: bool,
}

impl SessionState {
    /// Construct a fresh session with no history.
    #[must_use]
    pub fn empty(session_id: SessionId, model: String) -> Self {
        Self {
            session_id,
            history: Vec::new(),
            usage: CumulativeUsage::default(),
            model,
            todos: Vec::new(),
            plan_mode: false,
        }
    }
}

#[cfg(test)]
mod m4_04_session_extension_tests {
    use super::*;
    use protocol::SessionId;

    #[test]
    fn fresh_session_has_empty_todos_and_plan_mode_false() {
        let s = SessionState::empty(SessionId::nil(), "x".into());
        assert!(s.todos.is_empty(), "fresh session must have no todos");
        assert!(!s.plan_mode, "fresh session must not be in plan mode");
    }

    #[test]
    fn session_with_todos_round_trips_via_json() {
        let mut s = SessionState::empty(SessionId::nil(), "x".into());
        s.todos.push(TodoItem {
            id: "t1".into(),
            content: "buy milk".into(),
            status: TodoState::Pending,
        });
        s.todos.push(TodoItem {
            id: "t2".into(),
            content: "write plan".into(),
            status: TodoState::InProgress,
        });
        s.plan_mode = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(
            json.contains(r#""status":"pending""#),
            "pending wire literal: {json}"
        );
        assert!(
            json.contains(r#""status":"in_progress""#),
            "in_progress wire literal: {json}"
        );
        assert!(
            json.contains(r#""plan_mode":true"#),
            "plan_mode wire literal: {json}"
        );
        let back: SessionState = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back.todos.len(), 2);
        assert_eq!(back.todos[0].id, "t1");
        assert_eq!(back.todos[0].status, TodoState::Pending);
        assert_eq!(back.todos[1].status, TodoState::InProgress);
        assert!(back.plan_mode);
    }

    #[test]
    fn todo_state_serializes_to_locked_literals() {
        assert_eq!(
            serde_json::to_string(&TodoState::Pending).unwrap(),
            r#""pending""#
        );
        assert_eq!(
            serde_json::to_string(&TodoState::InProgress).unwrap(),
            r#""in_progress""#
        );
        assert_eq!(
            serde_json::to_string(&TodoState::Completed).unwrap(),
            r#""completed""#
        );
    }

    #[test]
    fn todo_state_rejects_done_alias() {
        let err = serde_json::from_str::<TodoState>(r#""done""#).expect_err(
            "'done' must NOT be accepted (spec §7 locks pending/in_progress/completed)",
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("unknown variant"),
            "msg should name unknown variant: {msg}"
        );
        assert!(msg.contains("done"), "msg should echo the bad input: {msg}");
    }

    #[test]
    fn todo_state_rejects_todo_alias() {
        let err = serde_json::from_str::<TodoState>(r#""todo""#)
            .expect_err("'todo' must NOT be accepted");
        let msg = format!("{err}");
        assert!(msg.contains("unknown variant"), "msg: {msg}");
    }
}
