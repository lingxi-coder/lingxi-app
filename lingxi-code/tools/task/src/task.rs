//! `Task*` tools — two distinct products sharing one tool-name surface.
//!
//! claude-code source:
//! `claude-code/src/tools/Task{Create,Get,List,Update}Tool/*` (the Product-A
//! todo-list tools, backed by `utils/tasks.ts`) plus `TaskStopTool.ts` /
//! `TaskOutputTool.tsx` (the Product-B background-task tools).
//!
//! ## Two products, one name surface (divergence note — sub-batch [6])
//!
//! - **Product-A V2 (todo store):** `TaskCreate` / `TaskGet` / `TaskList` /
//!   `TaskUpdate` are the claude-code todo-list tools. They persist through the
//!   file-backed [`crate::todo_store::TodoStore`] (decimal ids `"1".."N"`,
//!   `engine::TodoState` = `pending`/`in_progress`/`completed`). They do NOT use
//!   `validate_task_id`, `TASK_TYPES`, or `TASK_STATUSES`.
//! - **Product-B (background-registry):** `TaskStop` / `TaskOutput` dispatch the
//!   M1 background `TaskRegistry` (9-char `[bartwmd][0-9a-z]{8}` ids). The
//!   `validate_task_id` / `TASK_TYPES` / `TASK_STATUSES` symbols below back ONLY
//!   these two tools now — they are retained Product-B-only (also for the locked
//!   `parity_agent_task_tools` / `parity_registry` fixtures).
//!
//! All six tool-name constants stay reachable at `tool_task::task::*`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use engine::TodoState;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Map, Value};
use telemetry::pii::Verified;
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    TASK_CREATE_COMPLETED, TASK_CREATE_FAILED, TASK_CREATE_STARTED, TASK_GET_COMPLETED,
    TASK_GET_FAILED, TASK_GET_STARTED, TASK_LIST_COMPLETED, TASK_LIST_STARTED,
    TASK_OUTPUT_COMPLETED, TASK_OUTPUT_FAILED, TASK_OUTPUT_STARTED, TASK_STOP_COMPLETED,
    TASK_STOP_FAILED, TASK_STOP_STARTED, TASK_UPDATE_COMPLETED, TASK_UPDATE_FAILED,
    TASK_UPDATE_STARTED,
};
use telemetry::AnalyticsBus;
use traits::task_registry::TaskRegistryError;

use crate::todo_store::{TodoStore, TodoTask};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};
use tool_api::BuiltinToolContext;

/// Tool name `'TaskCreate'` (claude-code `TASK_CREATE_TOOL_NAME`).
pub const TASK_CREATE_TOOL_NAME: &str = "TaskCreate";
/// Tool name `'TaskGet'` (claude-code `TASK_GET_TOOL_NAME`).
pub const TASK_GET_TOOL_NAME: &str = "TaskGet";
/// Tool name `'TaskList'` (claude-code `TASK_LIST_TOOL_NAME`).
pub const TASK_LIST_TOOL_NAME: &str = "TaskList";
/// Tool name `'TaskUpdate'` (claude-code `TASK_UPDATE_TOOL_NAME`).
pub const TASK_UPDATE_TOOL_NAME: &str = "TaskUpdate";
/// Tool name `'TaskStop'` (claude-code inline declaration).
pub const TASK_STOP_TOOL_NAME: &str = "TaskStop";
/// Tool name `'TaskOutput'` (claude-code `TASK_OUTPUT_TOOL_NAME`).
pub const TASK_OUTPUT_TOOL_NAME: &str = "TaskOutput";

/// The 7 background task-type wire strings. Byte-aligned with `tasks::TaskType`
/// variants (snake_case).
///
// Product-B (background-registry) — retained for TaskStop/TaskOutput + fixture lock.
pub const TASK_TYPES: &[&str] = &[
    "local_bash",
    "local_agent",
    "remote_agent",
    "in_process_teammate",
    "local_workflow",
    "monitor_mcp",
    "dream",
];

/// The 5 background task-status wire strings.
///
// Product-B (background-registry) — retained for TaskStop/TaskOutput + fixture lock.
pub const TASK_STATUSES: &[&str] = &["pending", "running", "completed", "failed", "killed"];

/// Validate the M1 task-id format `[bartwmd][0-9a-z]{8}` (9 chars total).
///
// Product-B (background-registry) — retained for TaskStop/TaskOutput + fixture lock.
/// V2 (Product-A) task ids are decimal strings and MUST NOT route through this.
///
/// # Errors
/// Returns a locked human-readable error string on mismatch.
pub fn validate_task_id(s: &str) -> Result<(), String> {
    if s.chars().count() != 9 {
        return Err(format!(
            "Task: malformed task_id '{s}' (expected 9-char [bartwmd][0-9a-z]{{8}})"
        ));
    }
    let mut chars = s.chars();
    let prefix = chars.next().expect("len==9");
    if !"bartwmd".contains(prefix) {
        return Err(format!(
            "Task: malformed task_id '{s}' (expected 9-char [bartwmd][0-9a-z]{{8}})"
        ));
    }
    for c in chars {
        if !(c.is_ascii_digit() || (c.is_ascii_lowercase() && c.is_ascii_alphabetic())) {
            return Err(format!(
                "Task: malformed task_id '{s}' (expected 9-char [bartwmd][0-9a-z]{{8}})"
            ));
        }
    }
    Ok(())
}

/// Generate a fresh task-id matching the M1 format (`[bartwmd][0-9a-z]{8}`).
///
/// Mirrors `tasks::id::generate_task_id` without taking the
/// cyclic dep on `lingxi-tasks`. Retained for test fixtures + parity
/// (the regex test in this file generates ids locally to verify they
/// match the locked regex without depending on the registry being wired).
#[cfg(test)]
fn fresh_task_id(prefix: char) -> String {
    use rand::Rng;
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let suffix: String = (0..8)
        .map(|_| ALPHA[rng.random_range(0..ALPHA.len())] as char)
        .collect();
    format!("{prefix}{suffix}")
}

fn fresh_invocation_id() -> String {
    tool_api::util::ids::ulid_or_uuid()
}

// ==== Product-A V2 gating (sub-batch [2]) ===================================

/// Feature-flag key carrying the host's non-interactive-session bit into
/// [`ToolStaticContext`]. claude-code reads `getIsNonInteractiveSession()`
/// directly; the Rust `ToolStaticContext` exposes host toggles only via
/// `feature_flags`, so the host threads the bit through under this key. Absent
/// ⇒ interactive (the common case).
const NON_INTERACTIVE_SESSION_FLAG: &str = "is_non_interactive_session";

/// Port of `isEnvTruthy(process.env[key])` (`envUtils.ts:32-37`): lower-cased,
/// trimmed value ∈ {`1`,`true`,`yes`,`on`}.
fn env_truthy(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| {
        matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}

/// Pure core of [`is_todo_v2_enabled`] (`isTodoV2Enabled`,
/// `utils/tasks.ts:130-136`): `enable_tasks_env || !non_interactive`.
fn todo_v2_enabled_inner(enable_tasks_env: bool, non_interactive: bool) -> bool {
    enable_tasks_env || !non_interactive
}

/// Whether the Product-A V2 Task tools are advertised (and `TodoWrite` hidden).
///
/// Port of `isTodoV2Enabled()`: force-enabled by a truthy
/// `CLAUDE_CODE_ENABLE_TASKS`, otherwise enabled whenever the session is
/// interactive.
#[must_use]
pub fn is_todo_v2_enabled(ctx: &ToolStaticContext) -> bool {
    let non_interactive = ctx
        .feature_flags
        .get(NON_INTERACTIVE_SESSION_FLAG)
        .copied()
        .unwrap_or(false);
    todo_v2_enabled_inner(env_truthy("CLAUDE_CODE_ENABLE_TASKS"), non_interactive)
}

// ==== Product-A V2 shared helpers ==========================================

/// Wire string for an `engine::TodoState` (`pending`/`in_progress`/`completed`).
fn status_wire(state: TodoState) -> &'static str {
    match state {
        TodoState::Pending => "pending",
        TodoState::InProgress => "in_progress",
        TodoState::Completed => "completed",
    }
}

/// Render an id list as `#a, #b` (claude-code `ids.map(id => \`#${id}\`)...`).
fn join_hash(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// JS-truthiness of `task.metadata?._internal` (`TaskListTool` filter).
fn metadata_internal_truthy(metadata: &Map<String, Value>) -> bool {
    match metadata.get("_internal") {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::Bool(true) | Value::Array(_) | Value::Object(_)) => true,
        Some(Value::Number(n)) => n.as_f64().is_none_or(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
    }
}

/// Parsed `status` input for `TaskUpdate`: absent / the special `deleted`
/// action / a real `TodoState`.
enum StatusInput {
    None,
    Deleted,
    State(TodoState),
}

/// Resolve the task-list id (`getTaskListId()`), collapsed to the session id in
/// single-process. `CLAUDE_CODE_TASK_LIST_ID` overrides (TS priority 1); the
/// teammate / team-name branches collapse to the session.
async fn resolve_task_list_id(ctx: &ToolUseContext) -> String {
    if let Some(explicit) = std::env::var_os("CLAUDE_CODE_TASK_LIST_ID") {
        if !explicit.is_empty() {
            return explicit.to_string_lossy().into_owned();
        }
    }
    match &ctx.session {
        Some(session) => session.lock().await.session_id.to_string(),
        None => "default".to_string(),
    }
}

/// Borrowed view of a [`TodoTask`] used to render `TaskList` lines.
struct TaskListRow {
    id: String,
    subject: String,
    status: TodoState,
    owner: Option<String>,
    blocked_by: Vec<String>,
}

/// Render the `TaskList` result string (`mapToolResultToToolResultBlockParam`).
fn render_task_list(rows: &[TaskListRow]) -> String {
    if rows.is_empty() {
        return "No tasks found".to_string();
    }
    rows.iter()
        .map(|row| {
            let owner = row
                .owner
                .as_deref()
                .filter(|o| !o.is_empty())
                .map(|o| format!(" ({o})"))
                .unwrap_or_default();
            let blocked = if row.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" [blocked by {}]", join_hash(&row.blocked_by))
            };
            format!(
                "#{} [{}] {}{owner}{blocked}",
                row.id,
                status_wire(row.status),
                row.subject
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the `TaskGet` result string.
fn render_task_get(task: Option<&TodoTask>) -> String {
    let Some(task) = task else {
        return "Task not found".to_string();
    };
    let mut lines = vec![
        format!("Task #{}: {}", task.id, task.subject),
        format!("Status: {}", status_wire(task.status)),
        format!("Description: {}", task.description),
    ];
    if !task.blocked_by.is_empty() {
        lines.push(format!("Blocked by: {}", join_hash(&task.blocked_by)));
    }
    if !task.blocks.is_empty() {
        lines.push(format!("Blocks: {}", join_hash(&task.blocks)));
    }
    lines.join("\n")
}

async fn emit_started(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    extras: &[(&'static str, AnalyticsValue)],
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert(
        "invocation_id".into(),
        AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
    );
    for (k, v) in extras {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

async fn emit_completed(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    duration_ms: u64,
    extras: &[(&'static str, AnalyticsValue)],
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert(
        "invocation_id".into(),
        AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
    );
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    for (k, v) in extras {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

/// Convert a `TaskRegistryError` to a `ToolError` so the 6 task tools can
/// share one mapping.
fn registry_err_to_tool_err(prefix: &str, e: TaskRegistryError) -> ToolError {
    match e {
        TaskRegistryError::NotFound(id) => {
            ToolError::InvalidInput(format!("{prefix}: task not found: {id}"))
        }
        TaskRegistryError::InvalidInput(s) => ToolError::InvalidInput(format!("{prefix}: {s}")),
        TaskRegistryError::Internal(s) => ToolError::Internal(format!("{prefix}: {s}")),
    }
}

async fn emit_failed(
    bus: &Arc<AnalyticsBus>,
    event: &'static str,
    invocation_id: &str,
    error_kind: &str,
    duration_ms: u64,
) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert(
        "invocation_id".into(),
        AnalyticsValue::String(Verified::assert_safe(invocation_id.to_string()).into_inner()),
    );
    md.insert(
        "error_kind".into(),
        AnalyticsValue::String(Verified::assert_safe(error_kind.to_string()).into_inner()),
    );
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(event, md).await;
}

// ==== Product-A V2 prompts (verbatim claude-code, base/non-swarm variant) ==
//
// PARITY-GAP: each tool's getPrompt() also has an isAgentSwarmsEnabled() branch
// (teammate context / teammate workflow); those additions are omitted here.

const TASK_CREATE_PROMPT: &str = r#"Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations
- Plan mode - When using plan mode, create a task list to track the work
- User explicitly requests todo list - When the user directly asks you to use the todo list
- User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)
- After receiving new instructions - Immediately capture user requirements as tasks
- When you start working on a task - Mark it as in_progress BEFORE beginning work
- After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation

## When NOT to Use This Tool

Skip using this tool when:
- There is only a single, straightforward task
- The task is trivial and tracking it provides no organizational benefit
- The task can be completed in less than 3 trivial steps
- The task is purely conversational or informational

NOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.

## Task Fields

- **subject**: A brief, actionable title in imperative form (e.g., "Fix authentication bug in login flow")
- **description**: What needs to be done
- **activeForm** (optional): Present continuous form shown in the spinner when the task is in_progress (e.g., "Fixing authentication bug"). If omitted, the spinner shows the subject instead.

All tasks are created with status `pending`.

## Tips

- Create tasks with clear, specific subjects that describe the outcome
- After creating tasks, use TaskUpdate to set up dependencies (blocks/blockedBy) if needed
- Check TaskList first to avoid creating duplicate tasks
"#;

const TASK_GET_PROMPT: &str = r"Use this tool to retrieve a task by its ID from the task list.

## When to Use This Tool

- When you need the full description and context before starting work on a task
- To understand task dependencies (what it blocks, what blocks it)
- After being assigned a task, to get complete requirements

## Output

Returns full task details:
- **subject**: Task title
- **description**: Detailed requirements and context
- **status**: 'pending', 'in_progress', or 'completed'
- **blocks**: Tasks waiting on this one to complete
- **blockedBy**: Tasks that must complete before this one can start

## Tips

- After fetching a task, verify its blockedBy list is empty before beginning work.
- Use TaskList to see all tasks in summary form.
";

const TASK_LIST_PROMPT: &str = r"Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
- After completing a task, to check for newly unblocked work or claim the next available task
- **Prefer working on tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones

## Output

Returns a summary of each task:
- **id**: Task identifier (use with TaskGet, TaskUpdate)
- **subject**: Brief description of the task
- **status**: 'pending', 'in_progress', or 'completed'
- **owner**: Agent ID if assigned, empty if available
- **blockedBy**: List of open task IDs that must be resolved first (tasks with blockedBy cannot be claimed until dependencies resolve)

Use TaskGet with a specific task ID to view full details including description and comments.
";

const TASK_UPDATE_PROMPT: &str = r#"Use this tool to update a task in the task list.

## When to Use This Tool

**Mark tasks as resolved:**
- When you have completed the work described in a task
- When a task is no longer needed or has been superseded
- IMPORTANT: Always mark your assigned tasks as resolved when you finish them
- After resolving, call TaskList to find your next task

- ONLY mark a task as completed when you have FULLY accomplished it
- If you encounter errors, blockers, or cannot finish, keep the task as in_progress
- When blocked, create a new task describing what needs to be resolved
- Never mark a task as completed if:
  - Tests are failing
  - Implementation is partial
  - You encountered unresolved errors
  - You couldn't find necessary files or dependencies

**Delete tasks:**
- When a task is no longer relevant or was created in error
- Setting status to `deleted` permanently removes the task

**Update task details:**
- When requirements change or become clearer
- When establishing dependencies between tasks

## Fields You Can Update

- **status**: The task status (see Status Workflow below)
- **subject**: Change the task title (imperative form, e.g., "Run tests")
- **description**: Change the task description
- **activeForm**: Present continuous form shown in spinner when in_progress (e.g., "Running tests")
- **owner**: Change the task owner (agent name)
- **metadata**: Merge metadata keys into the task (set a key to null to delete it)
- **addBlocks**: Mark tasks that cannot start until this one completes
- **addBlockedBy**: Mark tasks that must complete before this one can start

## Status Workflow

Status progresses: `pending` → `in_progress` → `completed`

Use `deleted` to permanently remove a task.

## Staleness

Make sure to read a task's latest state using `TaskGet` before updating it.

## Examples

Mark task as in progress when starting work:
```json
{"taskId": "1", "status": "in_progress"}
```

Mark task as completed after finishing work:
```json
{"taskId": "1", "status": "completed"}
```

Delete a task:
```json
{"taskId": "1", "status": "deleted"}
```

Claim a task by setting owner:
```json
{"taskId": "1", "owner": "my-name"}
```

Set up task dependencies:
```json
{"taskId": "2", "addBlockedBy": ["1"]}
```
"#;

// ==== TaskCreateTool ========================================================

static TASK_CREATE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["subject", "description"],
        "properties": {
            "subject":     { "type": "string", "description": "A brief title for the task" },
            "description": { "type": "string", "description": "What needs to be done" },
            "activeForm":  { "type": "string", "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")" },
            "metadata":    { "type": "object", "description": "Arbitrary metadata to attach to the task" }
        }
    })
});

/// Product-A V2 `TaskCreate` — appends a task to the todo store.
pub struct TaskCreateTool {
    ctx: BuiltinToolContext,
}

impl TaskCreateTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        TASK_CREATE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TASK_CREATE_SCHEMA
    }
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        is_todo_v2_enabled(ctx)
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
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
                reason: "TaskCreate appends a task to the session task list".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Create a new task in the task list".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        // PARITY-GAP: the isAgentSwarmsEnabled() additions to getPrompt()
        // (teammate context / owner tips) are omitted; this is the base variant.
        TASK_CREATE_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        let subject = match input.get("subject").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "missing_subject",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskCreate: missing 'subject'".into(),
                ));
            }
        };
        let description = match input.get("description").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "missing_description",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskCreate: missing 'description'".into(),
                ));
            }
        };
        let active_form = input
            .get("activeForm")
            .and_then(Value::as_str)
            .map(str::to_string);
        let metadata = input
            .get("metadata")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        emit_started(&bus, TASK_CREATE_STARTED, &invocation_id, &[]).await;

        let list_id = resolve_task_list_id(&ctx).await;
        let store = TodoStore::for_list(&list_id);
        let task = TodoTask::new(subject.clone(), description, active_form, metadata);
        let task_id = match store.create(task).await {
            Ok(id) => id,
            Err(e) => {
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "store_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(format!("TaskCreate: {e}")));
            }
        };
        // PARITY-GAP: TaskCreated hooks + context.setAppState(expandedView)
        // (TaskCreateTool.ts:93-120) are omitted — no hook/app-state seam here.

        emit_completed(
            &bus,
            TASK_CREATE_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[(
                "task_id",
                AnalyticsValue::String(Verified::assert_safe(task_id.clone()).into_inner()),
            )],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({
                "content": format!("Task #{task_id} created successfully: {subject}"),
                "task": { "id": task_id, "subject": subject },
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskGetTool ============================================================

static TASK_GET_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["taskId"],
        "properties": {
            "taskId": { "type": "string", "description": "The ID of the task to retrieve" }
        }
    })
});

/// Product-A V2 `TaskGet` — read one task from the todo store.
pub struct TaskGetTool {
    ctx: BuiltinToolContext,
}

impl TaskGetTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &str {
        TASK_GET_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TASK_GET_SCHEMA
    }
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        is_todo_v2_enabled(ctx)
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
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
                reason: "TaskGet is read-only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Get a task by ID from the task list".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        TASK_GET_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        let task_id = match input.get("taskId").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_GET_FAILED,
                    &invocation_id,
                    "missing_task_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput("TaskGet: missing 'taskId'".into()));
            }
        };

        emit_started(&bus, TASK_GET_STARTED, &invocation_id, &[]).await;

        let list_id = resolve_task_list_id(&ctx).await;
        let store = TodoStore::for_list(&list_id);
        let task = store.get(&task_id).await;

        emit_completed(
            &bus,
            TASK_GET_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        let content = render_task_get(task.as_ref());
        let task_json = match &task {
            Some(t) => json!({
                "id": t.id,
                "subject": t.subject,
                "description": t.description,
                "status": status_wire(t.status),
                "blocks": t.blocks,
                "blockedBy": t.blocked_by,
            }),
            None => Value::Null,
        };
        Ok(ToolCallResult {
            data: json!({ "content": content, "task": task_json }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskListTool ===========================================================

static TASK_LIST_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    })
});

/// Product-A V2 `TaskList` — enumerate the todo store.
pub struct TaskListTool {
    ctx: BuiltinToolContext,
}

impl TaskListTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        TASK_LIST_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TASK_LIST_SCHEMA
    }
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        is_todo_v2_enabled(ctx)
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
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
                reason: "TaskList is read-only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "List all tasks in the task list".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        TASK_LIST_PROMPT.into()
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        emit_started(&bus, TASK_LIST_STARTED, &invocation_id, &[]).await;

        let list_id = resolve_task_list_id(&ctx).await;
        let store = TodoStore::for_list(&list_id);
        // Filter out tasks flagged metadata._internal (TaskListTool.ts:435-437).
        let all: Vec<TodoTask> = store
            .list()
            .await
            .into_iter()
            .filter(|t| !metadata_internal_truthy(&t.metadata))
            .collect();

        emit_completed(
            &bus,
            TASK_LIST_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[("count", AnalyticsValue::Int(all.len() as i64))],
        )
        .await;

        // blockedBy is recomputed to drop already-completed blockers.
        let resolved: HashSet<&str> = all
            .iter()
            .filter(|t| t.status == TodoState::Completed)
            .map(|t| t.id.as_str())
            .collect();

        let rows: Vec<TaskListRow> = all
            .iter()
            .map(|t| TaskListRow {
                id: t.id.clone(),
                subject: t.subject.clone(),
                status: t.status,
                owner: t.owner.clone(),
                blocked_by: t
                    .blocked_by
                    .iter()
                    .filter(|id| !resolved.contains(id.as_str()))
                    .cloned()
                    .collect(),
            })
            .collect();

        let content = render_task_list(&rows);
        let tasks: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "subject": row.subject,
                    "status": status_wire(row.status),
                    "owner": row.owner,
                    "blockedBy": row.blocked_by,
                })
            })
            .collect();
        Ok(ToolCallResult {
            data: json!({ "content": content, "tasks": tasks }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskUpdateTool =========================================================

static TASK_UPDATE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["taskId"],
        "properties": {
            "taskId":       { "type": "string", "description": "The ID of the task to update" },
            "subject":      { "type": "string", "description": "New subject for the task" },
            "description":  { "type": "string", "description": "New description for the task" },
            "activeForm":   { "type": "string", "description": "Present continuous form shown in spinner when in_progress (e.g., \"Running tests\")" },
            "status":       { "type": "string", "enum": ["pending","in_progress","completed","deleted"], "description": "New status for the task" },
            "addBlocks":    { "type": "array", "items": { "type": "string" }, "description": "Task IDs that this task blocks" },
            "addBlockedBy": { "type": "array", "items": { "type": "string" }, "description": "Task IDs that block this task" },
            "owner":        { "type": "string", "description": "New owner for the task" },
            "metadata":     { "type": "object", "description": "Metadata keys to merge into the task. Set a key to null to delete it." }
        }
    })
});

/// Render the `TaskUpdate` success string (`Updated task #id field, field`).
fn render_task_update_success(task_id: &str, updated_fields: &[String]) -> String {
    format!("Updated task #{task_id} {}", updated_fields.join(", "))
}

/// Render the `TaskUpdate` failure string (`error || "Task #id not found"`).
fn render_task_update_fail(task_id: &str, error: Option<&str>) -> String {
    match error.filter(|e| !e.is_empty()) {
        Some(e) => e.to_string(),
        None => format!("Task #{task_id} not found"),
    }
}

/// Product-A V2 `TaskUpdate` — mutate a task in the todo store.
pub struct TaskUpdateTool {
    ctx: BuiltinToolContext,
}

impl TaskUpdateTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        TASK_UPDATE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &TASK_UPDATE_SCHEMA
    }
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool {
        is_todo_v2_enabled(ctx)
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
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
                reason: "TaskUpdate mutates the session task list only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Update a task in the task list".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        TASK_UPDATE_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        let task_id = match input.get("taskId").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_UPDATE_FAILED,
                    &invocation_id,
                    "missing_task_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskUpdate: missing 'taskId'".into(),
                ));
            }
        };

        emit_started(&bus, TASK_UPDATE_STARTED, &invocation_id, &[]).await;
        let duration = || started.elapsed().as_millis() as u64;

        let list_id = resolve_task_list_id(&ctx).await;
        let store = TodoStore::for_list(&list_id);

        let existing = match store.get(&task_id).await {
            Some(t) => t,
            None => {
                emit_completed(&bus, TASK_UPDATE_COMPLETED, &invocation_id, duration(), &[]).await;
                return Ok(ToolCallResult {
                    data: json!({
                        "content": render_task_update_fail(&task_id, Some("Task not found")),
                        "success": false,
                        "taskId": task_id,
                        "updatedFields": Vec::<String>::new(),
                        "error": "Task not found",
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                });
            }
        };

        // Status: None / "deleted" (special action) / a real TodoState.
        let status_input = match input.get("status").and_then(Value::as_str) {
            None => StatusInput::None,
            Some("deleted") => StatusInput::Deleted,
            Some("pending") => StatusInput::State(TodoState::Pending),
            Some("in_progress") => StatusInput::State(TodoState::InProgress),
            Some("completed") => StatusInput::State(TodoState::Completed),
            Some(other) => {
                emit_failed(&bus, TASK_UPDATE_FAILED, &invocation_id, "invalid_status", duration())
                    .await;
                return Err(ToolError::InvalidInput(format!(
                    "TaskUpdate: invalid status '{other}'"
                )));
            }
        };

        // status == "deleted" → delete + early return (TaskUpdateTool.ts:755-768).
        if let StatusInput::Deleted = status_input {
            let deleted = store.delete(&task_id).await;
            emit_completed(&bus, TASK_UPDATE_COMPLETED, &invocation_id, duration(), &[]).await;
            let data = if deleted {
                json!({
                    "content": render_task_update_success(&task_id, &["deleted".to_string()]),
                    "success": true,
                    "taskId": task_id,
                    "updatedFields": ["deleted"],
                    "statusChange": { "from": status_wire(existing.status), "to": "deleted" },
                })
            } else {
                json!({
                    "content": render_task_update_fail(&task_id, Some("Failed to delete task")),
                    "success": false,
                    "taskId": task_id,
                    "updatedFields": Vec::<String>::new(),
                    "error": "Failed to delete task",
                })
            };
            return Ok(ToolCallResult {
                data,
                new_messages: vec![],
                context_modifier: None,
                mcp_meta: None,
            });
        }

        // Diff each provided field against the existing task.
        let in_subject = input.get("subject").and_then(Value::as_str);
        let in_description = input.get("description").and_then(Value::as_str);
        let in_active_form = input.get("activeForm").and_then(Value::as_str);
        let in_owner = input.get("owner").and_then(Value::as_str);
        let in_metadata = input.get("metadata").and_then(Value::as_object);

        let mut updated_fields: Vec<String> = Vec::new();
        let mut new_subject: Option<String> = None;
        let mut new_description: Option<String> = None;
        let mut new_active_form: Option<String> = None;
        let mut new_owner: Option<String> = None;
        let mut new_status: Option<TodoState> = None;
        let mut new_metadata: Option<Map<String, Value>> = None;

        if let Some(s) = in_subject {
            if s != existing.subject {
                new_subject = Some(s.to_string());
                updated_fields.push("subject".into());
            }
        }
        if let Some(d) = in_description {
            if d != existing.description {
                new_description = Some(d.to_string());
                updated_fields.push("description".into());
            }
        }
        if let Some(af) = in_active_form {
            if Some(af) != existing.active_form.as_deref() {
                new_active_form = Some(af.to_string());
                updated_fields.push("activeForm".into());
            }
        }
        if let Some(o) = in_owner {
            if Some(o) != existing.owner.as_deref() {
                new_owner = Some(o.to_string());
                updated_fields.push("owner".into());
            }
        }
        // PARITY-GAP: the isAgentSwarmsEnabled() auto-owner branch
        // (TaskUpdateTool.ts:730-741) is omitted.
        if let Some(meta) = in_metadata {
            let mut merged = existing.metadata.clone();
            for (key, value) in meta {
                if value.is_null() {
                    merged.remove(key);
                } else {
                    merged.insert(key.clone(), value.clone());
                }
            }
            new_metadata = Some(merged);
            updated_fields.push("metadata".into());
        }

        let mut status_change: Option<(TodoState, TodoState)> = None;
        if let StatusInput::State(st) = status_input {
            if st != existing.status {
                // PARITY-GAP: TaskCompleted hooks (TaskUpdateTool.ts:773-807) omitted.
                new_status = Some(st);
                updated_fields.push("status".into());
                status_change = Some((existing.status, st));
            }
        }

        let has_field_updates = new_subject.is_some()
            || new_description.is_some()
            || new_active_form.is_some()
            || new_owner.is_some()
            || new_status.is_some()
            || new_metadata.is_some();
        if has_field_updates {
            store
                .update(&task_id, |t| {
                    if let Some(v) = new_subject.clone() {
                        t.subject = v;
                    }
                    if let Some(v) = new_description.clone() {
                        t.description = v;
                    }
                    if let Some(v) = new_active_form.clone() {
                        t.active_form = Some(v);
                    }
                    if let Some(v) = new_owner.clone() {
                        t.owner = Some(v);
                    }
                    if let Some(v) = new_status {
                        t.status = v;
                    }
                    if let Some(v) = new_metadata.clone() {
                        t.metadata = v;
                    }
                })
                .await;
        }
        // PARITY-GAP: teammate-mailbox owner notification (TaskUpdateTool.ts:818-840) omitted.

        // addBlocks: this task blocks each listed id.
        if let Some(add_blocks) = input.get("addBlocks").and_then(Value::as_array) {
            let new_blocks: Vec<String> = add_blocks
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !existing.blocks.iter().any(|b| b == id))
                .map(str::to_string)
                .collect();
            for block_id in &new_blocks {
                store.block_task(&task_id, block_id).await;
            }
            if !new_blocks.is_empty() {
                updated_fields.push("blocks".into());
            }
        }
        // addBlockedBy: each listed id blocks this task.
        if let Some(add_blocked_by) = input.get("addBlockedBy").and_then(Value::as_array) {
            let new_blocked_by: Vec<String> = add_blocked_by
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !existing.blocked_by.iter().any(|b| b == id))
                .map(str::to_string)
                .collect();
            for blocker_id in &new_blocked_by {
                store.block_task(blocker_id, &task_id).await;
            }
            if !new_blocked_by.is_empty() {
                updated_fields.push("blockedBy".into());
            }
        }
        // PARITY-GAP: structural verification nudge (TaskUpdateTool.ts:875-891) omitted.

        emit_completed(&bus, TASK_UPDATE_COMPLETED, &invocation_id, duration(), &[]).await;

        let mut data = json!({
            "content": render_task_update_success(&task_id, &updated_fields),
            "success": true,
            "taskId": task_id,
            "updatedFields": updated_fields,
        });
        if let Some((from, to)) = status_change {
            data["statusChange"] = json!({ "from": status_wire(from), "to": status_wire(to) });
        }
        Ok(ToolCallResult {
            data,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskStopTool ===========================================================

// no-truncation: TaskStop + TaskOutput bound their model-facing output via their
// own `max_result_size_chars` (100_000) cap — the same value claude-code sets on
// `maxResultSizeChars` — rather than the shared `MAX_TOOL_OUTPUT_LENGTH`/`truncate`
// path used by the variable-length file/shell tools.

/// `TaskStopTool` description (`TaskStopTool.ts` `async description()`).
const TASK_STOP_DESCRIPTION: &str = "Stop a running background task by ID";

/// `TaskStopTool` prompt — `DESCRIPTION` from `TaskStopTool/prompt.ts` (verbatim,
/// including the leading + trailing newlines of the TS template literal).
const TASK_STOP_PROMPT: &str = "
- Stops a running background task by its ID
- Takes a task_id parameter identifying the task to stop
- Returns a success or failure status
- Use this tool when you need to terminate a long-running task
";

static TASK_STOP_SCHEMA: Lazy<Value> = Lazy::new(|| {
    // `z.strictObject` with two OPTIONAL strings and no `required` array
    // (`TaskStopTool.ts:10-19`). `shell_id` is the deprecated KillShell field.
    json!({
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "The ID of the background task to stop"
            },
            "shell_id": {
                "type": "string",
                "description": "Deprecated: use task_id instead"
            }
        },
        "additionalProperties": false
    })
});

/// Kills a task (surface stub).
pub struct TaskStopTool {
    ctx: BuiltinToolContext,
}

impl TaskStopTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &str {
        TASK_STOP_TOOL_NAME
    }
    /// `KillShell` is the deprecated name kept as an alias for backward
    /// compatibility (`TaskStopTool.ts:44`). `find_by_name` honours aliases.
    fn aliases(&self) -> &[&str] {
        const ALIASES: &[&str] = &["KillShell"];
        ALIASES
    }
    fn input_schema(&self) -> &Value {
        &TASK_STOP_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        true
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
                reason: "TaskStop kills the named task".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        TASK_STOP_DESCRIPTION.into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        TASK_STOP_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // Resolve id: `task_id ?? shell_id`, then `if (!id)` (`TaskStopTool.ts:111-115`).
        // `??` (`Option::or`) only falls back for an absent `task_id`; a present
        // empty string survives the coalesce and is then rejected by `!id`.
        let resolved = input
            .get("task_id")
            .and_then(Value::as_str)
            .or_else(|| input.get("shell_id").and_then(Value::as_str))
            .unwrap_or("");
        if resolved.is_empty() {
            emit_failed(
                &bus,
                TASK_STOP_FAILED,
                &invocation_id,
                "missing_task_id",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(
                "Missing required parameter: task_id".into(),
            ));
        }
        let task_id = resolved.to_string();

        emit_started(&bus, TASK_STOP_STARTED, &invocation_id, &[]).await;

        let registry = match self.ctx.task_registry.clone() {
            Some(r) => r,
            None => {
                emit_failed(
                    &bus,
                    TASK_STOP_FAILED,
                    &invocation_id,
                    "registry_not_wired",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(
                    "TaskStopTool: TaskRegistryHandle not wired into BuiltinToolContext".into(),
                ));
            }
        };

        // Pre-validation against the pre-kill record (`stopTask.ts:44-55`):
        // missing → "No task found with ID: {id}"; non-running →
        // "Task {id} is not running (status: {status})".
        let record = match registry.get(&task_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                emit_failed(
                    &bus,
                    TASK_STOP_FAILED,
                    &invocation_id,
                    "not_found",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "No task found with ID: {task_id}"
                )));
            }
            Err(e) => {
                emit_failed(
                    &bus,
                    TASK_STOP_FAILED,
                    &invocation_id,
                    "registry_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(registry_err_to_tool_err("TaskStop", e));
            }
        };
        if record.status != "running" {
            emit_failed(
                &bus,
                TASK_STOP_FAILED,
                &invocation_id,
                "not_running",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(format!(
                "Task {task_id} is not running (status: {})",
                record.status
            )));
        }

        // Capture command + type from the pre-kill record. The narrow registry
        // surface carries `description` (no separate `command` field), so it
        // backs both the bash `command` and the agent `description` TS sources.
        let task_type = record.task_type.clone();
        let command = record.description.clone();

        if let Err(e) = registry.kill(&task_id).await {
            emit_failed(
                &bus,
                TASK_STOP_FAILED,
                &invocation_id,
                "registry_error",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(registry_err_to_tool_err("TaskStop", e));
        }
        emit_completed(
            &bus,
            TASK_STOP_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        // No `content` key → the orchestrator JSON-stringifies the whole data
        // (matches TS `mapToolResultToToolResultBlockParam` → `jsonStringify`).
        Ok(ToolCallResult {
            data: json!({
                "message": format!("Successfully stopped task: {task_id} ({command})"),
                "task_id": task_id,
                "task_type": task_type,
                "command": command,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskOutputTool =========================================================

/// `TaskOutputTool` description (`TaskOutputTool.tsx` `async description()`).
const TASK_OUTPUT_DESCRIPTION: &str = "[Deprecated] — prefer Read on the task output file path";

/// `TaskOutputTool` prompt (`TaskOutputTool.tsx` `async prompt()`, verbatim).
const TASK_OUTPUT_PROMPT: &str = "DEPRECATED: Prefer using the Read tool on the task's output file path instead. Background tasks return their output file path in the tool result, and you receive a <task-notification> with the same path when the task completes — Read that file directly.

- Retrieves output from a running or completed task (background shell, agent, or remote session)
- Takes a task_id parameter identifying the task
- Returns the task output along with status information
- Use block=true (default) to wait for task completion
- Use block=false for non-blocking check of current status
- Task IDs can be found using the /tasks command
- Works with all task types: background shells, async agents, and remote sessions";

static TASK_OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    // `z.strictObject` (`TaskOutputTool.tsx:30-34`): required `task_id`, a
    // `block` bool (default true), and a `timeout` integer 0..600000 (default
    // 30000 ms). Replaces the old `offset`/`limit` paging params.
    json!({
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "The task ID to get output from"
            },
            "block": {
                "type": "boolean",
                "default": true,
                "description": "Whether to wait for completion"
            },
            "timeout": {
                "type": "integer",
                "minimum": 0,
                "maximum": 600_000,
                "default": 30_000,
                "description": "Max wait time in ms"
            }
        },
        "required": ["task_id"],
        "additionalProperties": false
    })
});

/// 1:1 port of `TaskOutputTool.tsx`'s `mapToolResultToToolResultBlockParam`
/// (lines 283-308): the XML render of a `retrieval_status` + optional `task`,
/// joined by a blank line. Fed to the model verbatim via the `content` key.
fn render_task_output(
    retrieval_status: &str,
    task: Option<&TaskOutputView>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "<retrieval_status>{retrieval_status}</retrieval_status>"
    ));
    if let Some(t) = task {
        parts.push(format!("<task_id>{}</task_id>", t.task_id));
        parts.push(format!("<task_type>{}</task_type>", t.task_type));
        parts.push(format!("<status>{}</status>", t.status));
        // `<exit_code>` only when the task carries one (TS: defined && non-null).
        if let Some(code) = t.exit_code {
            parts.push(format!("<exit_code>{code}</exit_code>"));
        }
        // `<output>` only when the trimmed output is non-blank (TS `output?.trim()`).
        if !t.output.trim().is_empty() {
            parts.push(format!("<output>\n{}\n</output>", t.output.trim_end()));
        }
    }
    parts.join("\n\n")
}

/// The `task` payload surfaced by `TaskOutputTool` — the subset of the TS
/// `TaskOutput` shape the narrow registry surface can resolve.
struct TaskOutputView {
    task_id: String,
    task_type: String,
    status: String,
    description: String,
    output: String,
    exit_code: Option<i32>,
}

/// Byte-faithful port of `TaskOutputTool.tsx`'s `retrieval_status` decision.
///
/// TS computes (`call`, lines 219-281):
/// - non-blocking (`block=false`): terminal task → `success`, otherwise
///   `not_ready`;
/// - blocking (`block=true`): a task that is terminal (after the poll) →
///   `success`, otherwise (still running/pending) → `timeout`.
///
/// `done` is the chunk's terminal flag (`status` ∈ {completed, failed, killed});
/// the Rust registry `output()` is a single read (the `block`/`timeout` poll
/// loop is Batch 3), so the blocking branch resolves against the chunk's
/// current `done` rather than re-polling.
fn task_output_retrieval_status(done: bool, block: bool) -> &'static str {
    if done {
        "success"
    } else if block {
        "timeout"
    } else {
        "not_ready"
    }
}

/// Reads a task's spool file (surface stub).
pub struct TaskOutputTool {
    ctx: BuiltinToolContext,
}

impl TaskOutputTool {
    /// Construct.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for TaskOutputTool {
    fn name(&self) -> &str {
        TASK_OUTPUT_TOOL_NAME
    }
    /// Backwards-compatible aliases for the renamed tools
    /// (`TaskOutputTool.tsx:150`).
    fn aliases(&self) -> &[&str] {
        const ALIASES: &[&str] = &["AgentOutputTool", "BashOutputTool"];
        ALIASES
    }
    fn input_schema(&self) -> &Value {
        &TASK_OUTPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
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
                reason: "TaskOutput reads spool file only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        TASK_OUTPUT_DESCRIPTION.into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        TASK_OUTPUT_PROMPT.into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let invocation_id = fresh_invocation_id();
        let bus = self.ctx.bus.clone();

        // `task_id` is required by the schema; guard mirrors `validateInput`
        // (`TaskOutputTool.tsx:188-193`): `if (!task_id)` → "Task ID is required".
        let task_id = match input.get("task_id").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "missing_task_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput("Task ID is required".into()));
            }
        };

        emit_started(&bus, TASK_OUTPUT_STARTED, &invocation_id, &[]).await;

        let registry = match self.ctx.task_registry.clone() {
            Some(r) => r,
            None => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "registry_not_wired",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::Internal(
                    "TaskOutputTool: TaskRegistryHandle not wired into BuiltinToolContext".into(),
                ));
            }
        };

        // `block` defaults to true (TS `semanticBoolean(z.boolean().default(true))`);
        // `timeout` defaults to 30000 ms (`z.number().min(0).max(600000).default(30000)`).
        let block = input.get("block").and_then(Value::as_bool).unwrap_or(true);
        let timeout_ms = input.get("timeout").and_then(Value::as_u64).unwrap_or(30_000);

        // Existence check (`TaskOutputTool.tsx:215-218`): `if (!task) throw …`.
        // The record carries `task_type` + `description`, which the output
        // chunk does not.
        let record = match registry.get(&task_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "not_found",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "No task found with ID: {task_id}"
                )));
            }
            Err(e) => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "registry_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(registry_err_to_tool_err("TaskOutput", e));
            }
        };

        // First (and, for `block==false`, only) read.
        let mut chunk = match registry.output(&task_id, None).await {
            Ok(c) => c,
            Err(e) => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "registry_error",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(registry_err_to_tool_err("TaskOutput", e));
            }
        };

        // Blocking wait (`waitForTaskCompletion`, `TaskOutputTool.tsx:118-143`):
        // re-read every 100 ms until the task is terminal (`done`) or the
        // timeout elapses. Elapsed is measured on a `std::time::Instant`.
        if block && !chunk.done {
            let wait_started = Instant::now();
            while !chunk.done {
                if (wait_started.elapsed().as_millis() as u64) >= timeout_ms {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                chunk = match registry.output(&task_id, None).await {
                    Ok(c) => c,
                    Err(e) => {
                        emit_failed(
                            &bus,
                            TASK_OUTPUT_FAILED,
                            &invocation_id,
                            "registry_error",
                            started.elapsed().as_millis() as u64,
                        )
                        .await;
                        return Err(registry_err_to_tool_err("TaskOutput", e));
                    }
                };
            }
        }

        emit_completed(
            &bus,
            TASK_OUTPUT_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        let retrieval_status = task_output_retrieval_status(chunk.done, block);
        let view = TaskOutputView {
            task_id: chunk.task_id.clone(),
            task_type: record.task_type.clone(),
            // Status from the (latest) chunk; fall back to the record if the
            // registry could not resolve it at the chunk point.
            status: chunk.status.clone().unwrap_or_else(|| record.status.clone()),
            description: record.description.clone(),
            output: chunk.content.clone(),
            exit_code: chunk.exit_code,
        };
        let content = render_task_output(retrieval_status, Some(&view));

        // `exit_code` is optional (TS attaches it only for `local_bash`); omit
        // the key entirely when the chunk carries none.
        let mut task_obj = Map::new();
        task_obj.insert("task_id".into(), json!(view.task_id));
        task_obj.insert("task_type".into(), json!(view.task_type));
        task_obj.insert("status".into(), json!(view.status));
        task_obj.insert("description".into(), json!(view.description));
        task_obj.insert("output".into(), json!(view.output));
        if let Some(code) = view.exit_code {
            task_obj.insert("exit_code".into(), json!(code));
        }

        // Nested `{ retrieval_status, task: { … } }` plus the `content` render so
        // the model sees it (`TaskOutputTool.tsx` data + `…BlockParam`).
        Ok(ToolCallResult {
            data: json!({
                "retrieval_status": retrieval_status,
                "task": Value::Object(task_obj),
                "content": content,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_tool_name_constants_locked() {
        assert_eq!(TASK_CREATE_TOOL_NAME, "TaskCreate");
        assert_eq!(TASK_GET_TOOL_NAME, "TaskGet");
        assert_eq!(TASK_LIST_TOOL_NAME, "TaskList");
        assert_eq!(TASK_UPDATE_TOOL_NAME, "TaskUpdate");
        assert_eq!(TASK_STOP_TOOL_NAME, "TaskStop");
        assert_eq!(TASK_OUTPUT_TOOL_NAME, "TaskOutput");
    }

    // ── retrieval_status (TaskOutputTool.tsx `call` lines 219-281) ───────────

    #[test]
    fn retrieval_status_done_is_success_regardless_of_block() {
        // TS: a terminal task → `success` in both the blocking and
        // non-blocking branches.
        assert_eq!(task_output_retrieval_status(true, true), "success");
        assert_eq!(task_output_retrieval_status(true, false), "success");
    }

    #[test]
    fn retrieval_status_running_non_blocking_is_not_ready() {
        // TS non-blocking branch: running/pending → `not_ready`.
        assert_eq!(task_output_retrieval_status(false, false), "not_ready");
    }

    #[test]
    fn retrieval_status_running_blocking_is_timeout() {
        // TS blocking branch: still running/pending after the wait → `timeout`.
        assert_eq!(task_output_retrieval_status(false, true), "timeout");
    }

    #[test]
    fn task_output_schema_declares_block_default_true() {
        let block = &TASK_OUTPUT_SCHEMA["properties"]["block"];
        assert_eq!(block["type"], "boolean");
        assert_eq!(block["default"], true);
    }

    #[test]
    fn validate_task_id_accepts_valid_9_char_ids() {
        assert!(validate_task_id("b3f9zk2x1").is_ok());
        assert!(validate_task_id("a2k1m9pq0").is_ok());
        assert!(validate_task_id("d000abcde").is_ok());
        assert!(validate_task_id("r12345678").is_ok());
        assert!(validate_task_id("t12345678").is_ok());
        assert!(validate_task_id("w12345678").is_ok());
        assert!(validate_task_id("m12345678").is_ok());
    }

    #[test]
    fn validate_task_id_rejects_bad() {
        assert!(validate_task_id("").is_err());
        assert!(validate_task_id("toolong0000").is_err());
        assert!(validate_task_id("X12345678").is_err());
        assert!(validate_task_id("b1234567Z").is_err());
        assert!(validate_task_id("b1234567_").is_err());
    }

    #[test]
    fn task_id_regex_matches_fresh_generated() {
        use regex::Regex;
        let re = Regex::new(r"^[bartwmd][0-9a-z]{8}$").unwrap();
        for c in ['b', 'a', 'r', 't', 'w', 'm', 'd'] {
            let id = fresh_task_id(c);
            assert!(re.is_match(&id), "generated id {id} fails regex");
            assert!(validate_task_id(&id).is_ok());
        }
    }

    #[test]
    fn task_types_byte_aligned_with_m1_surface() {
        assert_eq!(
            TASK_TYPES,
            &[
                "local_bash",
                "local_agent",
                "remote_agent",
                "in_process_teammate",
                "local_workflow",
                "monitor_mcp",
                "dream"
            ]
        );
    }

    #[test]
    fn task_statuses_locked() {
        assert_eq!(
            TASK_STATUSES,
            &["pending", "running", "completed", "failed", "killed"]
        );
    }

    // ── Product-A V2 gating (sub-batch [2]) ──────────────────────────────

    #[test]
    fn todo_v2_enabled_inner_matches_ts_predicate() {
        // enable_tasks_env || !non_interactive
        assert!(todo_v2_enabled_inner(false, false)); // interactive → on
        assert!(!todo_v2_enabled_inner(false, true)); // non-interactive → off
        assert!(todo_v2_enabled_inner(true, true)); // forced on by env
        assert!(todo_v2_enabled_inner(true, false));
    }

    #[test]
    fn is_todo_v2_enabled_reads_non_interactive_flag() {
        // Default (no flag) → interactive → V2 enabled. The CLAUDE_CODE_ENABLE_TASKS
        // env branch is covered by `todo_v2_enabled_inner` to avoid global env races.
        let mut ctx = ToolStaticContext::default();
        assert!(is_todo_v2_enabled(&ctx));
        ctx.feature_flags
            .insert(NON_INTERACTIVE_SESSION_FLAG.to_string(), true);
        // (Holds only when CLAUDE_CODE_ENABLE_TASKS is not truthy in the env.)
        if !env_truthy("CLAUDE_CODE_ENABLE_TASKS") {
            assert!(!is_todo_v2_enabled(&ctx));
        }
    }

    // ── Product-A V2 result-string rendering ─────────────────────────────

    fn mk_task(id: &str, subject: &str, status: TodoState) -> TodoTask {
        TodoTask {
            id: id.into(),
            subject: subject.into(),
            description: "desc".into(),
            active_form: None,
            owner: None,
            status,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: Map::new(),
        }
    }

    #[test]
    fn render_task_get_missing_and_present() {
        assert_eq!(render_task_get(None), "Task not found");
        let mut t = mk_task("1", "Do thing", TodoState::InProgress);
        t.blocked_by = vec!["2".into(), "3".into()];
        t.blocks = vec!["4".into()];
        assert_eq!(
            render_task_get(Some(&t)),
            "Task #1: Do thing\nStatus: in_progress\nDescription: desc\nBlocked by: #2, #3\nBlocks: #4"
        );
        let bare = mk_task("5", "Bare", TodoState::Pending);
        assert_eq!(
            render_task_get(Some(&bare)),
            "Task #5: Bare\nStatus: pending\nDescription: desc"
        );
    }

    #[test]
    fn render_task_list_empty_and_rows() {
        assert_eq!(render_task_list(&[]), "No tasks found");
        let rows = vec![
            TaskListRow {
                id: "1".into(),
                subject: "First".into(),
                status: TodoState::Pending,
                owner: Some("alice".into()),
                blocked_by: vec!["2".into()],
            },
            TaskListRow {
                id: "2".into(),
                subject: "Second".into(),
                status: TodoState::Completed,
                owner: None,
                blocked_by: Vec::new(),
            },
        ];
        assert_eq!(
            render_task_list(&rows),
            "#1 [pending] First (alice) [blocked by #2]\n#2 [completed] Second"
        );
    }

    #[test]
    fn render_task_update_strings() {
        assert_eq!(
            render_task_update_success("7", &["status".into(), "owner".into()]),
            "Updated task #7 status, owner"
        );
        assert_eq!(render_task_update_fail("9", Some("Task not found")), "Task not found");
        assert_eq!(render_task_update_fail("9", None), "Task #9 not found");
        assert_eq!(render_task_update_fail("9", Some("")), "Task #9 not found");
    }

    #[test]
    fn metadata_internal_truthiness_follows_js() {
        let mut m = Map::new();
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(true));
        assert!(metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(false));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(0));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!(""));
        assert!(!metadata_internal_truthy(&m));
        m.insert("_internal".into(), json!("yes"));
        assert!(metadata_internal_truthy(&m));
        m.insert("_internal".into(), Value::Null);
        assert!(!metadata_internal_truthy(&m));
    }

    // ── Product-A V2 schemas ─────────────────────────────────────────────

    #[test]
    fn task_create_schema_locked() {
        let s = &*TASK_CREATE_SCHEMA;
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["required"], json!(["subject", "description"]));
        assert_eq!(s["properties"]["subject"]["type"], "string");
        assert_eq!(s["properties"]["activeForm"]["type"], "string");
        assert_eq!(s["properties"]["metadata"]["type"], "object");
    }

    #[test]
    fn task_get_schema_requires_task_id() {
        let s = &*TASK_GET_SCHEMA;
        assert_eq!(s["required"], json!(["taskId"]));
        assert_eq!(s["additionalProperties"], false);
    }

    #[test]
    fn task_list_schema_takes_no_params() {
        let s = &*TASK_LIST_SCHEMA;
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"], json!({}));
    }

    #[test]
    fn task_update_schema_status_enum_includes_deleted() {
        let s = &*TASK_UPDATE_SCHEMA;
        assert_eq!(s["required"], json!(["taskId"]));
        assert_eq!(
            s["properties"]["status"]["enum"],
            json!(["pending", "in_progress", "completed", "deleted"])
        );
    }

    // ── Product-B TaskStop / TaskOutput drift (batch [3]) ────────────────
    //
    // A small in-memory `TaskRegistryHandle` mock backs the `call`-level
    // tests for the two background-registry tools.

    mod product_b {
        use super::*;
        use std::collections::VecDeque;
        use std::sync::Mutex as StdMutex;
        use telemetry::AnalyticsBus;
        use tool_api::test_support::{ctx_for_file_tools, fresh_ctx, fresh_tx, make_dummy_fs};
        use traits::task_registry::{
            TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryHandle,
            TaskUpdatePatch,
        };

        #[derive(Default)]
        struct MockRegistry {
            /// `None` ⇒ task not found.
            record: StdMutex<Option<TaskRecord>>,
            /// Successive `output()` results; the last entry repeats once drained.
            chunks: StdMutex<VecDeque<TaskOutputChunk>>,
            kill_calls: StdMutex<u32>,
            output_calls: StdMutex<u32>,
        }

        impl MockRegistry {
            fn with_record(record: Option<TaskRecord>) -> Arc<Self> {
                Arc::new(Self {
                    record: StdMutex::new(record),
                    ..Self::default()
                })
            }
            fn push_chunk(self: &Arc<Self>, c: TaskOutputChunk) {
                self.chunks.lock().unwrap().push_back(c);
            }
        }

        #[async_trait]
        impl TaskRegistryHandle for MockRegistry {
            async fn create(
                &self,
                _input: TaskCreateInput,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
                Ok(self.record.lock().unwrap().clone())
            }
            async fn list(
                &self,
                _filter: TaskListFilter,
            ) -> Result<Vec<TaskRecord>, TaskRegistryError> {
                Ok(self.record.lock().unwrap().clone().into_iter().collect())
            }
            async fn update(
                &self,
                _id: &str,
                _patch: TaskUpdatePatch,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn set_status(
                &self,
                _id: &str,
                _status: &str,
            ) -> Result<TaskRecord, TaskRegistryError> {
                Err(TaskRegistryError::Internal("unused in product_b tests".into()))
            }
            async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
                *self.kill_calls.lock().unwrap() += 1;
                let mut guard = self.record.lock().unwrap();
                match guard.as_mut() {
                    Some(r) => {
                        r.status = "killed".into();
                        Ok(r.clone())
                    }
                    None => Err(TaskRegistryError::NotFound(id.into())),
                }
            }
            async fn output(
                &self,
                id: &str,
                _offset: Option<u64>,
            ) -> Result<TaskOutputChunk, TaskRegistryError> {
                *self.output_calls.lock().unwrap() += 1;
                let mut q = self.chunks.lock().unwrap();
                if q.len() > 1 {
                    Ok(q.pop_front().unwrap())
                } else if let Some(front) = q.front() {
                    Ok(front.clone())
                } else {
                    Err(TaskRegistryError::NotFound(id.into()))
                }
            }
        }

        fn rec(status: &str) -> TaskRecord {
            TaskRecord {
                task_id: "b12345678".into(),
                task_type: "local_bash".into(),
                status: status.into(),
                description: "echo hi".into(),
            }
        }

        fn chunk(status: &str, done: bool, exit_code: Option<i32>, content: &str) -> TaskOutputChunk {
            TaskOutputChunk {
                task_id: "b12345678".into(),
                content: content.into(),
                total_lines: 1,
                truncated: false,
                status: Some(status.into()),
                exit_code,
                done,
            }
        }

        fn bctx(reg: Arc<dyn TaskRegistryHandle>) -> BuiltinToolContext {
            let bus = Arc::new(AnalyticsBus::new());
            let mut c = ctx_for_file_tools(make_dummy_fs(), bus, vec![std::env::temp_dir()]);
            c.task_registry = Some(reg);
            c
        }

        fn err_msg(e: ToolError) -> String {
            match e {
                ToolError::InvalidInput(s) | ToolError::Internal(s) => s,
                other => panic!("unexpected error variant: {other:?}"),
            }
        }

        // ── schemas ──────────────────────────────────────────────────────

        #[test]
        fn task_stop_schema_two_optional_strings_no_required() {
            let s = &*TASK_STOP_SCHEMA;
            assert_eq!(s["additionalProperties"], false);
            assert!(s.get("required").is_none(), "no required array");
            assert_eq!(s["properties"]["task_id"]["type"], "string");
            assert_eq!(
                s["properties"]["task_id"]["description"],
                "The ID of the background task to stop"
            );
            assert_eq!(s["properties"]["shell_id"]["type"], "string");
            assert_eq!(
                s["properties"]["shell_id"]["description"],
                "Deprecated: use task_id instead"
            );
        }

        #[test]
        fn task_output_schema_drops_offset_limit_adds_timeout() {
            let s = &*TASK_OUTPUT_SCHEMA;
            assert_eq!(s["additionalProperties"], false);
            assert_eq!(s["required"], json!(["task_id"]));
            assert!(s["properties"].get("offset").is_none());
            assert!(s["properties"].get("limit").is_none());
            let t = &s["properties"]["timeout"];
            assert_eq!(t["type"], "integer");
            assert_eq!(t["minimum"], 0);
            assert_eq!(t["maximum"], 600_000);
            assert_eq!(t["default"], 30_000);
            assert_eq!(s["properties"]["block"]["default"], true);
        }

        #[test]
        fn aliases_match_ts() {
            let stop = TaskStopTool::new(bctx(MockRegistry::with_record(None)));
            assert_eq!(stop.aliases(), &["KillShell"]);
            let out = TaskOutputTool::new(bctx(MockRegistry::with_record(None)));
            assert_eq!(out.aliases(), &["AgentOutputTool", "BashOutputTool"]);
        }

        // ── TaskStop ─────────────────────────────────────────────────────

        #[tokio::test]
        async fn task_stop_success_result_shape() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let res = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect("stop ok");
            assert_eq!(
                res.data["message"],
                "Successfully stopped task: b12345678 (echo hi)"
            );
            assert_eq!(res.data["task_id"], "b12345678");
            assert_eq!(res.data["task_type"], "local_bash");
            assert_eq!(res.data["command"], "echo hi");
            // No `content` key ⇒ orchestrator JSON-stringifies the whole data.
            assert!(res.data.get("content").is_none());
            assert_eq!(*reg.kill_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_stop_accepts_shell_id_alias() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let res = tool
                .call(json!({ "shell_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect("shell_id resolves");
            assert_eq!(res.data["task_id"], "b12345678");
            assert_eq!(*reg.kill_calls.lock().unwrap(), 1);
        }

        #[tokio::test]
        async fn task_stop_empty_task_id_falls_through_to_missing() {
            // `"" ?? shell_id` keeps "" (not nullish) → `!id` → missing error,
            // even though shell_id is present (TS quirk, reproduced 1:1).
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let err = tool
                .call(
                    json!({ "task_id": "", "shell_id": "b12345678" }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect_err("empty task_id is missing");
            assert_eq!(err_msg(err), "Missing required parameter: task_id");
            assert_eq!(*reg.kill_calls.lock().unwrap(), 0);
        }

        #[tokio::test]
        async fn task_stop_missing_param() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskStopTool::new(bctx(reg));
            let err = tool
                .call(json!({}), fresh_ctx(), fresh_tx())
                .await
                .expect_err("missing");
            assert_eq!(err_msg(err), "Missing required parameter: task_id");
        }

        #[tokio::test]
        async fn task_stop_not_found() {
            let reg = MockRegistry::with_record(None);
            let tool = TaskStopTool::new(bctx(reg));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not found");
            assert_eq!(err_msg(err), "No task found with ID: b12345678");
        }

        #[tokio::test]
        async fn task_stop_not_running() {
            let reg = MockRegistry::with_record(Some(rec("completed")));
            let tool = TaskStopTool::new(bctx(reg.clone()));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not running");
            assert_eq!(
                err_msg(err),
                "Task b12345678 is not running (status: completed)"
            );
            // Pre-validation rejects before any kill.
            assert_eq!(*reg.kill_calls.lock().unwrap(), 0);
        }

        // ── TaskOutput ───────────────────────────────────────────────────

        #[tokio::test]
        async fn task_output_nonblock_done_is_success() {
            let reg = MockRegistry::with_record(Some(rec("completed")));
            reg.push_chunk(chunk("completed", true, Some(0), "all done\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["task_id"], "b12345678");
            assert_eq!(res.data["task"]["task_type"], "local_bash");
            assert_eq!(res.data["task"]["status"], "completed");
            assert_eq!(res.data["task"]["description"], "echo hi");
            assert_eq!(res.data["task"]["output"], "all done\n");
            assert_eq!(res.data["task"]["exit_code"], 0);
            // Non-blocking ⇒ exactly one output read.
            assert_eq!(*reg.output_calls.lock().unwrap(), 1);
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<retrieval_status>success</retrieval_status>"));
            assert!(content.contains("<task_id>b12345678</task_id>"));
            assert!(content.contains("<task_type>local_bash</task_type>"));
            assert!(content.contains("<status>completed</status>"));
            assert!(content.contains("<exit_code>0</exit_code>"));
            assert!(content.contains("<output>\nall done\n</output>"));
            // Tags joined by a blank line.
            assert!(content.contains("</retrieval_status>\n\n<task_id>"));
        }

        #[tokio::test]
        async fn task_output_nonblock_running_is_not_ready() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, ""));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": false }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "not_ready");
            // Blank output ⇒ no <output> tag; running ⇒ no <exit_code>.
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<status>running</status>"));
            assert!(!content.contains("<output>"));
            assert!(!content.contains("<exit_code>"));
            // exit_code key omitted when the chunk carries none.
            assert!(res.data["task"].get("exit_code").is_none());
        }

        #[tokio::test]
        async fn task_output_block_polls_until_done() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            // running, running, then completed — the poll loop must reach it.
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("running", false, None, ""));
            reg.push_chunk(chunk("completed", true, Some(0), "final\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 600_000 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "success");
            assert_eq!(res.data["task"]["status"], "completed");
            assert_eq!(res.data["task"]["output"], "final\n");
            assert!(*reg.output_calls.lock().unwrap() >= 3);
        }

        #[tokio::test]
        async fn task_output_block_timeout_is_timeout() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            reg.push_chunk(chunk("running", false, None, "still going\n"));
            let tool = TaskOutputTool::new(bctx(reg.clone()));
            // timeout=0 ⇒ the loop breaks immediately without sleeping.
            let res = tool
                .call(
                    json!({ "task_id": "b12345678", "block": true, "timeout": 0 }),
                    fresh_ctx(),
                    fresh_tx(),
                )
                .await
                .expect("ok");
            assert_eq!(res.data["retrieval_status"], "timeout");
            assert_eq!(res.data["task"]["status"], "running");
            let content = res.data["content"].as_str().unwrap();
            assert!(content.contains("<retrieval_status>timeout</retrieval_status>"));
        }

        #[tokio::test]
        async fn task_output_not_found() {
            let reg = MockRegistry::with_record(None);
            let tool = TaskOutputTool::new(bctx(reg));
            let err = tool
                .call(json!({ "task_id": "b12345678" }), fresh_ctx(), fresh_tx())
                .await
                .expect_err("not found");
            assert_eq!(err_msg(err), "No task found with ID: b12345678");
        }

        #[tokio::test]
        async fn task_output_missing_task_id() {
            let reg = MockRegistry::with_record(Some(rec("running")));
            let tool = TaskOutputTool::new(bctx(reg));
            let err = tool
                .call(json!({}), fresh_ctx(), fresh_tx())
                .await
                .expect_err("missing");
            assert_eq!(err_msg(err), "Task ID is required");
        }

        #[test]
        fn render_task_output_null_task_is_status_only() {
            // The `task: null` (timeout) branch renders just the status line.
            assert_eq!(
                render_task_output("timeout", None),
                "<retrieval_status>timeout</retrieval_status>"
            );
        }
    }
}
