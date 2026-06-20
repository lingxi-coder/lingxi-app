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
/// Canonical tool name in the registry.
pub const TOOL_NAME: &str = "TodoWrite";

/// claude-code v2.1.183 `FWd` (binary offset 199291572): the SHORT
/// TodoWrite tool prompt selected by the `Dh` gate for new models
/// (claude-opus-4-8 / claude-fable-5 / claude-mythos-5) and whenever
/// `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` is env-truthy. Byte-exact.
pub const TODO_WRITE_PROMPT_SIMPLE: &str = "Create and update a task list for the current session. The list is rendered to the user as your working plan.\n\n- Each todo has `content`, `status` (\"pending\" | \"in_progress\" | \"completed\"), and `activeForm` (present-tense label shown while in progress).\n- Send the full list each call; it replaces the previous one.\n- Keep one item `in_progress` at a time and mark it `completed` when done.";

/// claude-code v2.1.183 `UWd` (binary offset 199292278): the long
/// standard TodoWrite tool prompt selected by the `Dh` gate for classic
/// models and whenever `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` is
/// env-defined-falsy. The single `${Ua}` substitution renders as the
/// `Edit` tool name (`Ua="Edit"`). Byte-exact.
pub const TODO_WRITE_PROMPT_FULL: &str = r#"Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool
Use this tool proactively in these scenarios:

1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
3. User explicitly requests todo list - When the user directly asks you to use the todo list
4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
5. After receiving new instructions - Immediately capture user requirements as todos
6. When you start working on a task - Mark it as in_progress BEFORE beginning work. Ideally you should only have one todo as in_progress at a time
7. After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
1. There is only a single, straightforward task
2. The task is trivial and tracking it provides no organizational benefit
3. The task can be completed in less than 3 trivial steps
4. The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Examples of When to Use the Todo List

<example>
User: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!
Assistant: *Creates todo list with the following items:*
1. Creating dark mode toggle component in Settings page
2. Adding dark mode state management (context/store)
3. Implementing CSS-in-JS styles for dark theme
4. Updating existing components to support theme switching
5. Running tests and build process, addressing any failures or errors that occur
*Begins working on the first task*

<reasoning>
The assistant used the todo list because:
1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes
2. The user explicitly requested tests and build be run afterward
3. The assistant inferred that tests and build need to pass by adding "Ensure tests and build succeed" as the final task
</reasoning>
</example>

<example>
User: Help me rename the function getCwd to getCurrentWorkingDirectory across my project
Assistant: *Uses grep or search tools to locate all instances of getCwd in the codebase*
I've found 15 instances of 'getCwd' across 8 different files.
*Creates todo list with specific items for each file that needs updating*

<reasoning>
The assistant used the todo list because:
1. First, the assistant searched to understand the scope of the task
2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps
3. The todo list helps ensure every instance is tracked and updated systematically
4. This approach prevents missing any occurrences and maintains code consistency
</reasoning>
</example>


<example>
User: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.
Assistant: *Creates a todo list breaking down each feature into specific tasks based on the project architecture*
Let's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.

<reasoning>
The assistant used the todo list because:
1. The user provided multiple complex features to implement in a comma separated list
2. The todo list helps organize these large features into manageable tasks
3. This approach allows for tracking progress across the entire implementation
</reasoning>
</example>

<example>
User: Can you help optimize my React application? It's rendering slowly and has performance issues.
Assistant: *Reviews component structure, render patterns, state management, and data fetching*
After analyzing your codebase, I've identified several performance issues.
*Creates todo list with items like: 1) Implementing memoization for expensive calculations in ProductList, 2) Adding virtualization for long lists in Dashboard, 3) Optimizing image loading in Gallery component, 4) Fixing state update loops in ShoppingCart, 5) Reviewing bundle size and implementing code splitting*
Let's start by implementing memoization for the expensive calculations in your ProductList component.

<reasoning>
The assistant used the todo list because:
1. First, the assistant examined the codebase to identify specific performance issues
2. Based on this analysis, it identified multiple optimization opportunities
3. Performance optimization is a non-trivial task requiring multiple steps
4. The todo list helps methodically track improvements across different components
5. This systematic approach ensures all performance bottlenecks are addressed
</reasoning>
</example>

## Examples of When NOT to Use the Todo List

<example>
User: How do I print 'Hello World' in Python?
Assistant: In Python, you can print "Hello World" with this simple code:

python
print("Hello World")

This will output the text "Hello World" to the console when executed.

<reasoning>
The assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.
</reasoning>
</example>

<example>
User: What does the git status command do?
Assistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.

<reasoning>
The assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.
</reasoning>
</example>

<example>
User: Can you add a comment to the calculateTotal function to explain what it does?
Assistant: Sure, let me add a comment to the calculateTotal function to explain what it does.
* Uses the Edit tool to add a comment to the calculateTotal function *

<reasoning>
The assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.
</reasoning>
</example>

<example>
User: Run npm install for me and tell me what happens.
Assistant: I'll run the npm install command for you.

*Executes: npm install*

The command completed successfully. Here's the output:
[Output of npm install command]

All dependencies have been installed according to your package.json file.

<reasoning>
The assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.
</reasoning>
</example>

## Task States and Management

1. **Task States**: Use these states to track progress:
   - pending: Task not yet started
   - in_progress: Currently working on (limit to ONE task at a time)
   - completed: Task finished successfully

   **IMPORTANT**: Task descriptions must have two forms:
   - content: The imperative form describing what needs to be done (e.g., "Run tests", "Build the project")
   - activeForm: The present continuous form shown during execution (e.g., "Running tests", "Building the project")

2. **Task Management**:
   - Update task status in real-time as you work
   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)
   - Exactly ONE task must be in_progress at any time (not less, not more)
   - Complete current tasks before starting new ones
   - Remove tasks that are no longer relevant from the list entirely

3. **Task Completion Requirements**:
   - ONLY mark a task as completed when you have FULLY accomplished it
   - If you encounter errors, blockers, or cannot finish, keep the task as in_progress
   - When blocked, create a new task describing what needs to be resolved
   - Never mark a task as completed if:
     - Tests are failing
     - Implementation is partial
     - You encountered unresolved errors
     - You couldn't find necessary files or dependencies

4. **Task Breakdown**:
   - Create specific, actionable items
   - Break complex tasks into smaller, manageable steps
   - Use clear, descriptive task names
   - Always provide both forms:
     - content: "Fix authentication bug"
     - activeForm: "Fixing authentication bug"

When in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.
"#;

/// Port of claude-code `Xla(model)` = `Dh(model) ? FWd : UWd`
/// (`prompt({model:e}){return Xla(e)}`). The `Dh` gate (binary offset
/// 195159798) is: env `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` truthy → FWd;
/// env-defined-falsy → UWd; otherwise the model-class branch
/// `!UWu(model) || FWu(model)` → FWd else UWd. The session model id is
/// not threaded into `PromptOptions` in this build (would require a
/// cross-crate `tool_trait::PromptOptions` field + caller changes), so
/// the model-class branch resolves as it does in the binary when the
/// model is absent: `Dh(undefined)` returns `false` → `UWd`. The
/// env override is honored exactly. This is byte-exact for the env
/// path and for every classic model; the only residual divergence is
/// new-model (opus-4-8-class) sessions, which should pick `FWd`.
#[must_use]
pub fn select_todo_write_prompt() -> &'static str {
    let simple = std::env::var("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT")
        .ok();
    if traits::env::is_env_truthy(simple.as_deref()) {
        return TODO_WRITE_PROMPT_SIMPLE;
    }
    if traits::env::is_env_defined_falsy(simple.as_deref()) {
        return TODO_WRITE_PROMPT_FULL;
    }
    // Model-class branch with model unavailable: `Dh(undefined)` → UWd.
    TODO_WRITE_PROMPT_FULL
}

/// Validate a `Vec<TodoItem>` for the TodoWrite contract.
///
/// Mirrors claude-code `TodoItemSchema` (zod `BWd`, v2.1.183), which only
/// requires `content` and `activeForm` to be non-empty (`.min(1, ...)`); there
/// is NO `.max()` on `content`, so long todos are accepted. TS does NOT reject a
/// list with multiple `in_progress` items — the single-in-progress convention
/// is advisory (surfaced in the prompt), never enforced — and TS input items
/// carry no `id`, so neither an id-uniqueness nor an in-progress-count check
/// exists. The emptiness messages are the verbatim zod strings.
///
/// # Errors
/// Returns a human-readable error string on the first rule violation.
pub fn validate_todos(todos: &[TodoItem]) -> Result<(), String> {
    for t in todos {
        if t.content.is_empty() {
            return Err("Content cannot be empty".into());
        }
        if t.active_form.is_empty() {
            return Err("Active form cannot be empty".into());
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
                "description": "The updated todo list",
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
        "required": ["todos"],
        "additionalProperties": false
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
        // claude-code v2.1.183 `description(){return Qla}`.
        "Update the todo list for the current session. To be used proactively and often to track progress and pending tasks. Make sure that at least one task is in_progress at all times. Always provide both content (imperative) and activeForm (present continuous) for each task.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        // claude-code v2.1.183 `prompt({model:e}){return Xla(e)}` where
        // `function Xla(e){return Dh(e)?FWd:UWd}`. `Dh` selects the SHORT `FWd`
        // prompt for new models (opus-4-8 / fable-5 / mythos-5) and the long
        // `UWd` for classic models, gated up front by the
        // `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` env override.
        select_todo_write_prompt().into()
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

        // Model-facing result text (`TodoWriteTool.ts`
        // `mapToolResultToToolResultBlockParam`): in claude-code v2.1.183 the
        // tool_result `content` is UNCONDITIONALLY the fixed base string
        // (verified at binary offset 199302445 — `content:"Todos have been
        // modified successfully. …"`). There is NO verification-nudge suffix:
        // grep of v2.1.183 for `spawn the verification agent`, `You just closed
        // out`, `tengu_hive_evidence`, `VERIFICATION_AGENT`, and
        // `verificationNudge` all return 0. The whole nudge feature is absent,
        // so neither the suffix nor a `verificationNudgeNeeded` field is emitted.
        let content = String::from(
            "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable",
        );

        Ok(ToolCallResult {
            data: json!({
                "content": content,
                "todos": todos,
                "summary": {
                    "pending": pending,
                    "in_progress": in_progress,
                    "completed": completed,
                },
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

    #[tokio::test]
    async fn description_matches_binary_qla() {
        // claude-code v2.1.183 `description(){return Qla}`.
        let (tool, _sink, _session, _use_ctx) = make_tool_and_session();
        let desc = tool
            .description(
                &json!({}),
                &DescriptionOptions {
                    is_non_interactive_session: false,
                },
            )
            .await;
        assert_eq!(
            desc,
            "Update the todo list for the current session. To be used proactively and often to track progress and pending tasks. Make sure that at least one task is in_progress at all times. Always provide both content (imperative) and activeForm (present continuous) for each task."
        );
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
        assert_eq!(err, "Content cannot be empty");
    }

    #[test]
    fn validate_accepts_long_content() {
        // claude-code zod `TodoItemSchema` (BWd) has no `.max()` on content —
        // long todos must be accepted (matches v2.1.183).
        let huge = "x".repeat(10_000);
        let todos = vec![TodoItem {
            id: "a".into(),
            content: huge,
            status: TodoState::Pending,
            active_form: "active".into(),
        }];
        assert!(validate_todos(&todos).is_ok());
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
        assert_eq!(err, "Active form cannot be empty");
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

    // ── result text is the BARE base string (finding #74) ────────────────
    //
    // claude-code v2.1.183 has NO verification-nudge feature: the TodoWrite
    // tool_result `content` is unconditionally the base string (binary offset
    // 199302445), and there is no `verificationNudgeNeeded` result field.
    // These tests pin the always-bare result and the absence of the field.

    const TODO_BASE: &str = "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable";

    /// Markers from the removed (fabricated) verification-nudge suffix — must
    /// NEVER appear in the result text on any path.
    const NUDGE_MARKER: &str = "spawn the verification agent (subagent_type=\"verification\")";

    fn completed(id: &str, content: &str) -> Value {
        json!({ "id": id, "content": content, "status": "completed", "activeForm": content })
    }

    #[tokio::test]
    async fn result_is_bare_base_when_all_completed_3plus() {
        // The exact scenario that previously fired the (fabricated) nudge —
        // 3+ all-completed items on the main-thread interactive path. The
        // result must now be the bare base string with no suffix and no
        // `verificationNudgeNeeded` field.
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
        assert_eq!(res.data["content"], json!(TODO_BASE), "bare base string only");
        assert!(
            !res.data["content"].as_str().unwrap().contains(NUDGE_MARKER),
            "no verification-nudge suffix"
        );
        assert!(
            res.data.get("verificationNudgeNeeded").is_none(),
            "no verificationNudgeNeeded field in result data"
        );
    }

    #[tokio::test]
    async fn result_is_bare_base_for_fewer_than_three() {
        let (tool, sink, _session, use_ctx) = make_tool_and_session();
        tool.ctx.bus.attach_sink(sink.clone()).await;
        let input = json!({
            "todos": [completed("1", "Implement"), completed("2", "Document")]
        });
        let res = tool.call(input, use_ctx, fresh_tx()).await.expect("ok");
        assert_eq!(res.data["content"], json!(TODO_BASE));
        assert!(res.data.get("verificationNudgeNeeded").is_none());
    }

    #[tokio::test]
    async fn result_is_bare_base_for_subagent() {
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
        assert!(res.data.get("verificationNudgeNeeded").is_none());
    }

    #[tokio::test]
    async fn result_is_bare_base_when_not_all_completed() {
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
        assert!(res.data.get("verificationNudgeNeeded").is_none());
    }

    // ── model-facing prompt() = Xla(model) = Dh(model) ? FWd : UWd (#71) ──
    //
    // All cases live in ONE test (no `serial_test` dep) so the parallel test
    // runner never has two bodies mutating the shared
    // `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` env var at once. The `EnvGuard`
    // restores the prior value on drop. No other test module reads this var.

    #[tokio::test]
    async fn prompt_selects_uwd_or_fwd_via_dh_env_gate() {
        let (tool, _sink, _session, _use_ctx) = make_tool_and_session();
        let opts = PromptOptions {
            include_examples: false,
        };

        // Default (var unset): no model threaded ⇒ `Dh(undefined)` is false ⇒
        // the long UWd prompt — NOT the old custom 3-sentence blurb.
        {
            let _g = EnvGuard::clear("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT");
            let p = tool.prompt(&opts).await;
            assert_eq!(p, TODO_WRITE_PROMPT_FULL);
            // Binary-locked head + tail of UWd.
            assert!(p.starts_with(
                "Use this tool to create and manage a structured task list for your current coding session."
            ));
            assert!(p.ends_with(
                "Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.\n"
            ));
            // The `${Ua}` substitution renders the Edit tool name verbatim.
            assert!(p.contains("Uses the Edit tool to add a comment"));
            // The stale blurb (and its false `id` claim) is gone.
            assert!(!p.contains("Use TodoWrite to track work-in-progress"));
        }

        // `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` truthy ⇒ the short FWd prompt.
        for truthy in ["1", "true", "yes", "on", " On "] {
            let _g = EnvGuard::set("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT", truthy);
            let p = tool.prompt(&opts).await;
            assert_eq!(p, TODO_WRITE_PROMPT_SIMPLE, "truthy {truthy:?} ⇒ FWd");
            assert!(p.starts_with(
                "Create and update a task list for the current session. The list is rendered to the user as your working plan."
            ));
            // FWd no longer (falsely) claims todos carry an `id` field.
            assert!(!p.contains("has id"));
        }

        // `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT` defined-falsy ⇒ the long UWd prompt.
        for falsy in ["0", "false", "no", "off"] {
            let _g = EnvGuard::set("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT", falsy);
            let p = tool.prompt(&opts).await;
            assert_eq!(p, TODO_WRITE_PROMPT_FULL, "defined-falsy {falsy:?} ⇒ UWd");
        }

        // Any other (non-truthy, non-defined-falsy) value falls through to the
        // model-class branch, which resolves to UWd when no model is available.
        {
            let _g = EnvGuard::set("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT", "garbage");
            assert_eq!(tool.prompt(&opts).await, TODO_WRITE_PROMPT_FULL);
        }

        // Selector helper agrees with the tool method.
        {
            let _g = EnvGuard::set("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT", "yes");
            assert_eq!(select_todo_write_prompt(), TODO_WRITE_PROMPT_SIMPLE);
        }
        {
            let _g = EnvGuard::clear("CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT");
            assert_eq!(select_todo_write_prompt(), TODO_WRITE_PROMPT_FULL);
        }
    }

    /// Minimal scoped env guard: sets/clears a var for the test body and
    /// restores the prior value on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
        fn clear(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
