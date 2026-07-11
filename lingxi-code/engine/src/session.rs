//! Session state model. Persistence lives in `lingxi-session` (Plan 10).

use crate::token::Usage;
use protocol::{ConversationMessage, MessageId, SessionId, ToolUseId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// Stable identifier. claude-code `TodoItemSchema` is
    /// `{ content, status, activeForm }` — the model-facing `TodoWrite` input
    /// carries no `id`, so `default` lets id-less input deserialize, and
    /// `skip_serializing_if` keeps the empty id out of the tool OUTPUT to
    /// match TS `newTodos` (whose items omit `id`). The field type stays
    /// `String` so existing readers keep compiling.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Human-readable description of the todo.
    pub content: String,
    /// Lifecycle state of this todo. Wire-locked.
    pub status: TodoState,
    /// Present-tense form shown while the todo is in progress (claude-code
    /// `TodoItemSchema.activeForm`). Wire key is camelCase `activeForm`;
    /// `default` keeps pre-`activeForm` persisted sessions deserializable.
    #[serde(rename = "activeForm", default)]
    pub active_form: String,
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
    /// Provider profile pinned alongside `model` (disambiguates shared ids
    /// across providers). `None` = resolve unscoped. Set by `switch_model`.
    #[serde(default)]
    pub model_profile: Option<String>,
    /// Active todo list (M4-04). Mutated by `TodoWriteTool`. Defaults to empty
    /// on deserialize so pre-M4-04 persisted sessions still load.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    /// Plan-mode flag (M4-04). Flipped by `EnterPlanModeTool` /
    /// `ExitPlanModeTool`. Defaults to `false` on deserialize.
    #[serde(default)]
    pub plan_mode: bool,
    /// Full-vs-sparse plan-mode reminder tracking — the per-turn plan-mode
    /// reminder is `full` (206 `LU_`) on the FIRST plan-mode turn and `sparse`
    /// (206 `MU_`) after (206 `reminderType`). Reset to `false` on plan-mode
    /// ENTRY (`EnterPlanModeTool` / `set_plan_mode(true)`) so a re-entered plan
    /// mode replays the full reminder. The orchestrator sets it `true` after the
    /// first injection. Never persisted meaningfully (transient like the todo
    /// counters); defaults to `false` on deserialize.
    #[serde(default)]
    pub plan_reminder_shown: bool,
    /// Finding #73 — assistant turns since the last `TodoWrite` (V1) /
    /// `TaskCreate`|`TaskUpdate` (V2) tool call. The per-turn todo-reminder
    /// (`L4p`/`N4p` `turnsSinceLastTodoWrite`/`turnsSinceLastTaskManagement`)
    /// fires only once this reaches `reminder::TURNS_SINCE_WRITE` (`10`). Reset
    /// to `0` whenever a qualifying tool call is observed in a turn's assistant
    /// response; incremented once per assistant turn. The binary recomputes
    /// this by scanning the message log; this engine never persists the
    /// reminder attachment to history, so it is tracked here as explicit
    /// session state. Defaults to `0` on deserialize (pre-#73 sessions).
    #[serde(default)]
    pub turns_since_last_todo_write: u32,
    /// Finding #73 — assistant turns since the last todo/task reminder fired
    /// (`turnsSinceLastReminder`). The reminder fires only once this reaches
    /// `reminder::TURNS_BETWEEN_REMINDERS` (`10`), then resets to `0`.
    /// Incremented once per assistant turn. Defaults to `0` on deserialize.
    #[serde(default)]
    pub turns_since_last_reminder: u32,
    /// In-memory association of each tool-injected conversation message
    /// (`MessageId`) to the `tool_use_id` of the tool that injected it
    /// (the Skill tool's OWN `tool_use` block id). Faithful port of TS
    /// `tagMessagesWithToolUseID` (`tools/utils.ts:12-25`), which stamps each
    /// injected `UserMessage` with `sourceToolUseID`. TS treats this as an
    /// UNTYPED extra prop that is NEVER serialized to the JSONL transcript —
    /// the sole reader is `getToolUseID` (`utils/messages.ts:2765-2793`) for
    /// TUI grouping. Mirroring that, this is a side-table marked
    /// `#[serde(skip)]` so it stays purely in-memory and never reaches the
    /// wire (JSONL bytes stay byte-identical). No Rust consumer reads it yet
    /// (the TUI grouping is not ported).
    #[serde(skip)]
    pub injected_message_sources: HashMap<MessageId, ToolUseId>,
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
            model_profile: None,
            todos: Vec::new(),
            plan_mode: false,
            plan_reminder_shown: false,
            turns_since_last_todo_write: 0,
            turns_since_last_reminder: 0,
            injected_message_sources: HashMap::new(),
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
            active_form: "Buying milk".into(),
        });
        s.todos.push(TodoItem {
            id: "t2".into(),
            content: "write plan".into(),
            status: TodoState::InProgress,
            active_form: "Writing plan".into(),
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
