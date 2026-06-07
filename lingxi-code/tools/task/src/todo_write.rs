//! `TodoWriteTool` — persists a `Vec<TodoItem>` onto the running
//! `SessionState`. Spec §7 line 486 locks the three lifecycle states:
//! `pending` / `in_progress` / `completed`. The aliases `done` and `todo`
//! are NOT accepted.

use std::time::Instant;

use async_trait::async_trait;
use engine::{TodoItem, TodoState};
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{TODO_WRITE_COMPLETED, TODO_WRITE_FAILED, TODO_WRITE_STARTED};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Wire literal for the `pending` lifecycle state. Spec §7 line 486.
pub const TODO_STATE_PENDING: &str = "pending";
/// Wire literal for the `in_progress` lifecycle state.
pub const TODO_STATE_IN_PROGRESS: &str = "in_progress";
/// Wire literal for the `completed` lifecycle state.
pub const TODO_STATE_COMPLETED: &str = "completed";
/// All three locked states in spec-declared order.
pub const TODO_STATES: &[&str] = &[
    TODO_STATE_PENDING,
    TODO_STATE_IN_PROGRESS,
    TODO_STATE_COMPLETED,
];
/// Maximum `content` length per todo (chars, defensive cap).
pub const TODO_MAX_CONTENT_CHARS: usize = 4_096;
/// Canonical tool name in the registry.
pub const TOOL_NAME: &str = "TodoWrite";

/// Validate a `Vec<TodoItem>` for the TodoWrite contract.
///
/// Mirrors claude-code `TodoItemSchema` (`utils/todo/types.ts`), which only
/// requires `content` and `activeForm` to be non-empty. TS does NOT reject a
/// list with multiple `in_progress` items — the single-in-progress convention
/// is advisory (surfaced in the prompt), never enforced — and TS input items
/// carry no `id`, so neither an id-uniqueness nor an in-progress-count check
/// exists.
///
/// # Errors
/// Returns a human-readable error string on the first rule violation.
pub fn validate_todos(todos: &[TodoItem]) -> Result<(), String> {
    for t in todos {
        if t.content.is_empty() {
            return Err("TodoWrite: todo content is empty".into());
        }
        if t.active_form.is_empty() {
            return Err("TodoWrite: todo activeForm is empty".into());
        }
        let n = t.content.chars().count();
        if n > TODO_MAX_CONTENT_CHARS {
            return Err(format!(
                "TodoWrite: todo content exceeds {TODO_MAX_CONTENT_CHARS} chars (got {n})"
            ));
        }
    }
    Ok(())
}

/// Compute `(pending, in_progress, completed)` counts over a todo list.
#[must_use]
pub fn summary(todos: &[TodoItem]) -> (u32, u32, u32) {
    let mut pending = 0u32;
    let mut in_progress = 0u32;
    let mut completed = 0u32;
    for t in todos {
        match t.status {
            TodoState::Pending => pending += 1,
            TodoState::InProgress => in_progress += 1,
            TodoState::Completed => completed += 1,
        }
    }
    (pending, in_progress, completed)
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "minLength": 1 },
                        "status":  {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"]
                        },
                        "activeForm": { "type": "string", "minLength": 1 }
                    },
                    "required": ["content", "status", "activeForm"]
                }
            }
        },
        "required": ["todos"]
    })
});

/// `TodoWriteTool` — replaces the running session's `todos` list with the
/// caller-supplied list. Wire-locked by spec §7 line 486.
pub struct TodoWriteTool {
    ctx: BuiltinToolContext,
}

impl TodoWriteTool {
    /// Construct a new tool wired to the shared builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }

    fn fresh_invocation_id() -> String {
        // Reuse the existing M4-01 invocation-id helper (ULID/UUID hybrid).
        tool_api::util::ids::ulid_or_uuid()
    }

    async fn emit_started(&self, invocation_id: &str, todo_count: usize) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "tool_name".into(),
            AnalyticsValue::String(Verified::assert_safe(TOOL_NAME.into()).into_inner()),
        );
        md.insert("todo_count".into(), AnalyticsValue::Int(todo_count as i64));
        self.ctx.bus.log_event(TODO_WRITE_STARTED, md).await;
    }

    async fn emit_completed(&self, invocation_id: &str, todos: &[TodoItem], duration_ms: u64) {
        let (pending, in_progress, completed) = summary(todos);
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "tool_name".into(),
            AnalyticsValue::String(Verified::assert_safe(TOOL_NAME.into()).into_inner()),
        );
        md.insert("todo_count".into(), AnalyticsValue::Int(todos.len() as i64));
        md.insert("pending".into(), AnalyticsValue::Int(i64::from(pending)));
        md.insert(
            "in_progress".into(),
            AnalyticsValue::Int(i64::from(in_progress)),
        );
        md.insert(
            "completed".into(),
            AnalyticsValue::Int(i64::from(completed)),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(TODO_WRITE_COMPLETED, md).await;
    }

    async fn emit_failed(&self, invocation_id: &str, error_kind: &str, duration_ms: u64) {
        let mut md: LogEventMetadata = HashMap::new();
        md.insert(
            "invocation_id".into(),
            AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
        );
        md.insert(
            "tool_name".into(),
            AnalyticsValue::String(Verified::assert_safe(TOOL_NAME.into()).into_inner()),
        );
        md.insert(
            "error_kind".into(),
            AnalyticsValue::String(Verified::assert_safe(error_kind.into()).into_inner()),
        );
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(duration_ms as i64),
        );
        self.ctx.bus.log_event(TODO_WRITE_FAILED, md).await;
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        // V1/V2 mutex: TodoWrite (V1) is advertised only when the V2 Task tools
        // are NOT (claude-code `isEnabled: () => !isTodoV2Enabled()`).
        !crate::task::is_todo_v2_enabled(ctx)
    }
    fn max_result_size_chars(&self) -> usize {
        tool_api::util::output_truncation::MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "TodoWrite mutates session state only; no external side-effect".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Replace the session todo list".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TodoWrite to track work-in-progress. Each todo has id, content, \
         and status (pending / in_progress / completed). At most one todo \
         may be in_progress at a time."
            .into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let invocation_id = Self::fresh_invocation_id();
        let started_at = Instant::now();

        // Parse `todos` from `input`.
        let raw_todos = match input.get("todos") {
            Some(v) => v.clone(),
            None => {
                self.emit_failed(
                    &invocation_id,
                    "missing_todos",
                    started_at.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TodoWrite: missing required 'todos' field".into(),
                ));
            }
        };
        let todos: Vec<TodoItem> = match serde_json::from_value(raw_todos) {
            Ok(v) => v,
            Err(e) => {
                self.emit_failed(
                    &invocation_id,
                    "invalid_input",
                    started_at.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "TodoWrite: invalid todos shape: {e}"
                )));
            }
        };

        self.emit_started(&invocation_id, todos.len()).await;

        if let Err(msg) = validate_todos(&todos) {
            self.emit_failed(
                &invocation_id,
                "validation",
                started_at.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }

        // Mutate session. `TodoWriteTool.ts:69-70` — `allDone = todos.every(completed)`
        // and `newTodos = allDone ? [] : todos`: an all-completed write CLEARS the
        // stored list (the loop has exited; nothing is left to track), while the
        // result still reports the full `todos` the model sent (`newTodos: todos`,
        // `:99`). An empty write (`todos = []`) is vacuously all-completed, so it
        // also clears — matching TS `[].every(...) === true`.
        let all_done = todos.iter().all(|t| t.status == TodoState::Completed);
        let session = ctx.session.as_ref().ok_or_else(|| {
            ToolError::Internal(
                "TodoWrite: session not wired into ToolUseContext (M4-04 contract)".into(),
            )
        })?;
        {
            let mut guard = session.lock().await;
            if all_done {
                guard.todos.clear();
            } else {
                guard.todos.clone_from(&todos);
            }
        }

        let duration_ms = started_at.elapsed().as_millis() as u64;
        self.emit_completed(&invocation_id, &todos, duration_ms)
            .await;

        let (pending, in_progress, completed) = summary(&todos);

        // Model-facing result text (`TodoWriteTool.ts:104-113`
        // `mapToolResultToToolResultBlockParam`): the fixed base string, plus
        // the structural verification nudge when the main-thread agent closes
        // out a 3+ item all-completed list with no verification step
        // (`TodoWriteTool.ts:72-86`; regex runs over each todo's `content`).
        // `all_done` was computed above for the clear-on-completion store.
        let nudge_needed = crate::task::verification_nudge_needed(
            ctx.agent_id.is_none(),
            ctx.options.is_non_interactive_session,
            all_done,
            todos.len(),
            todos.iter().map(|t| t.content.as_str()),
        );
        let mut content = String::from(
            "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable",
        );
        if nudge_needed {
            content.push_str(&crate::task::verification_nudge_suffix());
        }

        Ok(ToolCallResult {
            data: json!({
                "content": content,
                "todos": todos,
                "summary": {
                    "pending": pending,
                    "in_progress": in_progress,
                    "completed": completed,
                },
                "verificationNudgeNeeded": nudge_needed,
            }),
            new_messages: Vec::new(),
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::SessionState;
    use protocol::SessionId;
    use std::sync::Arc;
    use telemetry::{AnalyticsBus, InMemorySink};
    use tokio::sync::Mutex;
    use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};

    fn make_tool_and_session() -> (
        TodoWriteTool,
        Arc<InMemorySink>,
        Arc<Mutex<SessionState>>,
        tool_api::context::ToolUseContext,
    ) {
        let bus = Arc::new(AnalyticsBus::new());
        let sink = Arc::new(InMemorySink::default());
        let bctx = ctx_for_file_tools(make_dummy_fs(), bus.clone(), vec![std::env::temp_dir()]);
        // Attach sink synchronously by spawning a quick task is overkill —
        // tests use a tokio runtime so we can attach via .await in each test.
        let session = Arc::new(Mutex::new(SessionState::empty(
            SessionId::nil(),
            "claude-opus-4-7".into(),
        )));
        let mut use_ctx = fresh_ctx();
        use_ctx.session = Some(session.clone());
        (TodoWriteTool::new(bctx), sink, session, use_ctx)
    }

    #[test]
    fn state_literals_match_spec() {
        assert_eq!(TODO_STATE_PENDING, "pending");
        assert_eq!(TODO_STATE_IN_PROGRESS, "in_progress");
        assert_eq!(TODO_STATE_COMPLETED, "completed");
    }

    #[test]
    fn todo_states_array_has_locked_order() {
        assert_eq!(TODO_STATES, &["pending", "in_progress", "completed"]);
    }

    #[test]
    fn todo_state_serialize_matches_lock() {
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
    fn rejects_done_alias() {
        let err =
            serde_json::from_str::<TodoState>(r#""done""#).expect_err("'done' must be rejected");
        let msg = format!("{err}");
        assert!(msg.contains("unknown variant"), "msg: {msg}");
        assert!(msg.contains("done"), "msg: {msg}");
    }

    #[test]
    fn rejects_todo_alias() {
        let err =
            serde_json::from_str::<TodoState>(r#""todo""#).expect_err("'todo' must be rejected");
        assert!(format!("{err}").contains("unknown variant"));
    }

    #[test]
    fn max_content_cap_is_4096() {
        assert_eq!(TODO_MAX_CONTENT_CHARS, 4_096);
    }

    #[test]
    fn validate_accepts_well_formed_list() {
        let todos = vec![
            TodoItem {
                id: "a".into(),
                content: "x".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
            TodoItem {
                id: "b".into(),
                content: "y".into(),
                status: TodoState::InProgress,
                active_form: "active".into(),
            },
            TodoItem {
                id: "c".into(),
                content: "z".into(),
                status: TodoState::Completed,
                active_form: "active".into(),
            },
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn validate_rejects_empty_content() {
        let todos = vec![TodoItem {
            id: "a".into(),
            content: String::new(),
            status: TodoState::Pending,
            active_form: "active".into(),
        }];
        let err = validate_todos(&todos).expect_err("empty content must reject");
        assert_eq!(err, "TodoWrite: todo content is empty");
    }

    #[test]
    fn validate_rejects_over_long_content() {
        let huge = "x".repeat(TODO_MAX_CONTENT_CHARS + 1);
        let n = huge.chars().count();
        let todos = vec![TodoItem {
            id: "a".into(),
            content: huge,
            status: TodoState::Pending,
            active_form: "active".into(),
        }];
        let err = validate_todos(&todos).expect_err("over-long must reject");
        assert_eq!(
            err,
            format!("TodoWrite: todo content exceeds {TODO_MAX_CONTENT_CHARS} chars (got {n})")
        );
    }

    #[test]
    fn validate_accepts_duplicate_id() {
        // TS `TodoItemSchema` has no `id` concept, so there is no uniqueness
        // rule; id-less input deserializes to empty ids, which must not reject.
        let todos = vec![
            TodoItem {
                id: "dup".into(),
                content: "x".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
            TodoItem {
                id: "dup".into(),
                content: "y".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn validate_accepts_two_in_progress() {
        // TS treats single-in-progress as advisory (prompt-only), never an
        // error — multiple `in_progress` items must be accepted.
        let todos = vec![
            TodoItem {
                id: "a".into(),
                content: "x".into(),
                status: TodoState::InProgress,
                active_form: "active".into(),
            },
            TodoItem {
                id: "b".into(),
                content: "y".into(),
                status: TodoState::InProgress,
                active_form: "active".into(),
            },
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn validate_accepts_zero_in_progress() {
        let todos = vec![
            TodoItem {
                id: "a".into(),
                content: "x".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
            TodoItem {
                id: "b".into(),
                content: "y".into(),
                status: TodoState::Completed,
                active_form: "active".into(),
            },
        ];
        assert!(validate_todos(&todos).is_ok());
    }

    #[test]
    fn summary_counts_each_state() {
        let todos = vec![
            TodoItem {
                id: "a".into(),
                content: "x".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
            TodoItem {
                id: "b".into(),
                content: "y".into(),
                status: TodoState::Pending,
                active_form: "active".into(),
            },
            TodoItem {
                id: "c".into(),
                content: "z".into(),
                status: TodoState::InProgress,
                active_form: "active".into(),
            },
            TodoItem {
                id: "d".into(),
                content: "w".into(),
                status: TodoState::Completed,
                active_form: "active".into(),
            },
        ];
        assert_eq!(summary(&todos), (2_u32, 1_u32, 1_u32));
    }

    #[tokio::test]
    async fn execute_persists_todos_to_session() {
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "id": "t1", "content": "first",  "status": "pending",     "activeForm": "Doing first"  },
                { "id": "t2", "content": "second", "status": "in_progress", "activeForm": "Doing second" },
                { "id": "t3", "content": "third",  "status": "completed",   "activeForm": "Doing third"  }
            ]
        });
        let res = tool
            .call(input, use_ctx, fresh_tx())
            .await
            .expect("happy path");
        let s = session.lock().await;
        assert_eq!(s.todos.len(), 3);
        assert_eq!(s.todos[0].id, "t1");
        assert_eq!(s.todos[1].status, TodoState::InProgress);
        assert_eq!(res.data["summary"]["pending"], 1);
        assert_eq!(res.data["summary"]["in_progress"], 1);
        assert_eq!(res.data["summary"]["completed"], 1);
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&TODO_WRITE_STARTED.to_string()));
        assert!(names.contains(&TODO_WRITE_COMPLETED.to_string()));
        assert!(!names.contains(&TODO_WRITE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn execute_rejects_unknown_status_with_serde_message() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "id": "t1", "content": "x", "status": "done" }
            ]
        });
        let err = tool
            .call(input, use_ctx, fresh_tx())
            .await
            .expect_err("unknown variant must fail");
        let msg = format!("{err}");
        assert!(msg.contains("TodoWrite: invalid todos shape"), "msg: {msg}");
        assert!(msg.contains("unknown variant"), "msg: {msg}");
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(
            names.contains(&TODO_WRITE_FAILED.to_string()),
            "failed event must fire: {names:?}"
        );
    }

    #[tokio::test]
    async fn execute_accepts_two_in_progress() {
        // TS does not reject multiple `in_progress`; the write succeeds and
        // both items persist (id-less input — TS items have no `id`).
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "content": "x", "status": "in_progress", "activeForm": "Doing x" },
                { "content": "y", "status": "in_progress", "activeForm": "Doing y" }
            ]
        });
        let res = tool
            .call(input, use_ctx, fresh_tx())
            .await
            .expect("two in_progress is accepted");
        assert_eq!(res.data["summary"]["in_progress"], 2);
        {
            let s = session.lock().await;
            assert_eq!(s.todos.len(), 2);
        }
        let names: Vec<String> = sink.events().await.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&TODO_WRITE_COMPLETED.to_string()));
        assert!(!names.contains(&TODO_WRITE_FAILED.to_string()));
    }

    #[tokio::test]
    async fn execute_accepts_id_less_input_and_omits_id_in_output() {
        // claude-code TodoItem input is `{ content, status, activeForm }` with
        // no `id`. Such input must deserialize and persist, and the OUTPUT
        // `todos` must not carry an `id` (TS `newTodos` items omit it).
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "content": "first",  "status": "pending",     "activeForm": "Doing first"  },
                { "content": "second", "status": "in_progress",  "activeForm": "Doing second" }
            ]
        });
        let res = tool
            .call(input, use_ctx, fresh_tx())
            .await
            .expect("id-less input is accepted");
        {
            let s = session.lock().await;
            assert_eq!(s.todos.len(), 2);
            assert_eq!(s.todos[0].id, "", "id defaults to empty when absent");
        }
        let out = &res.data["todos"];
        for item in out.as_array().expect("todos array") {
            assert!(
                item.get("id").is_none(),
                "OUTPUT must not leak an id: {item}"
            );
        }
    }

    #[test]
    fn validate_rejects_empty_active_form() {
        // claude-code TodoItemSchema requires `activeForm` non-empty (min(1)).
        let todos = vec![TodoItem {
            id: "a".into(),
            content: "x".into(),
            status: TodoState::Pending,
            active_form: String::new(),
        }];
        let err = validate_todos(&todos).expect_err("empty activeForm must reject");
        assert_eq!(err, "TodoWrite: todo activeForm is empty");
    }

    #[tokio::test]
    async fn execute_round_trips_active_form() {
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "id": "t1", "content": "ship it", "status": "in_progress", "activeForm": "Shipping it" }
            ]
        });
        tool.call(input, use_ctx, fresh_tx())
            .await
            .expect("activeForm happy path");
        let s = session.lock().await;
        assert_eq!(s.todos.len(), 1);
        assert_eq!(s.todos[0].active_form, "Shipping it");
    }

    // ── all-completed clears the stored list (TodoWriteTool.ts:70) ───────

    #[tokio::test]
    async fn all_completed_write_clears_session_but_reports_full_list() {
        // `newTodos = allDone ? [] : todos`: every-completed write clears the
        // stored list, while the result still reports the full list the model
        // sent (`newTodos: todos`).
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "id": "a", "content": "build", "status": "completed", "activeForm": "Building" },
                { "id": "b", "content": "ship",  "status": "completed", "activeForm": "Shipping" }
            ]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        {
            let s = session.lock().await;
            assert!(s.todos.is_empty(), "all-completed write clears the stored todos");
        }
        // The result still reports the full 2-item list + all-completed summary.
        assert_eq!(res.data["todos"].as_array().unwrap().len(), 2);
        assert_eq!(res.data["summary"]["completed"], 2);
        assert_eq!(res.data["summary"]["pending"], 0);
    }

    #[tokio::test]
    async fn partial_write_retains_session_todos() {
        // Not all completed ⇒ the stored list is the full write (no clear).
        let (tool, sink, session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                { "id": "a", "content": "build", "status": "completed",   "activeForm": "Building" },
                { "id": "b", "content": "ship",  "status": "in_progress", "activeForm": "Shipping" }
            ]
        });
        tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        let s = session.lock().await;
        assert_eq!(s.todos.len(), 2, "a not-all-completed write is stored verbatim");
    }

    // ── verification nudge (sub-batch [5]) ───────────────────────────────

    const TODO_BASE: &str = "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable";

    const NUDGE_MARKER: &str = "spawn the verification agent (subagent_type=\"verification\")";

    fn completed(id: &str, content: &str) -> Value {
        json!({ "id": id, "content": content, "status": "completed", "activeForm": content })
    }

    #[tokio::test]
    async fn nudge_fires_when_all_completed_3plus_no_verif() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                completed("1", "Implement parser"),
                completed("2", "Wire it up"),
                completed("3", "Write docs"),
            ]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        let content = res.data["content"].as_str().unwrap();
        assert!(content.starts_with(TODO_BASE), "base prefix: {content}");
        assert!(content.contains(NUDGE_MARKER), "nudge present: {content}");
        assert_eq!(res.data["verificationNudgeNeeded"], json!(true));
    }

    #[tokio::test]
    async fn no_nudge_when_fewer_than_three() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [completed("1", "Implement"), completed("2", "Document")]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        assert_eq!(res.data["content"], json!(TODO_BASE));
        assert_eq!(res.data["verificationNudgeNeeded"], json!(false));
    }

    #[tokio::test]
    async fn no_nudge_when_content_matches_verif() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                completed("1", "Implement parser"),
                completed("2", "Verify the fix"),
                completed("3", "Write docs"),
            ]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        assert_eq!(res.data["content"], json!(TODO_BASE));
        assert_eq!(res.data["verificationNudgeNeeded"], json!(false));
    }

    #[tokio::test]
    async fn no_nudge_for_subagent() {
        let (tool, sink, _session, mut use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        use_ctx.agent_id = Some(protocol::AgentId::new());
        let input = json!({
            "todos": [
                completed("1", "Implement parser"),
                completed("2", "Wire it up"),
                completed("3", "Write docs"),
            ]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        assert_eq!(res.data["content"], json!(TODO_BASE));
        assert_eq!(res.data["verificationNudgeNeeded"], json!(false));
    }

    #[tokio::test]
    async fn no_nudge_when_not_all_completed() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [
                completed("1", "Implement parser"),
                completed("2", "Wire it up"),
                { "id": "3", "content": "Write docs", "status": "pending", "activeForm": "Writing docs" },
            ]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        assert_eq!(res.data["content"], json!(TODO_BASE));
        assert_eq!(res.data["verificationNudgeNeeded"], json!(false));
    }
}
