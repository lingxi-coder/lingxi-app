//! `Task*` tools — six thin wrappers over the M1 `TaskRegistry` surface.
//!
//! Spec §4 Flow D + §7 line 491-499. claude-code source:
//! `claude-code/src/tools/Task{Create,Get,List,Update,Output}Tool/constants.ts`
//! (`TaskStop` is declared inline in `TaskStopTool.ts`).
//!
//! **Architectural note (M4-05):** `lingxi-tasks` already depends on
//! `lingxi-tools`, blocking a direct path-dep from this crate to the task
//! registry. The six tools here therefore expose the byte-locked schema
//! surface (tool names, telemetry events, task-id regex, error formats,
//! status/type enums) without consuming `lingxi_tasks::TaskRegistry`
//! directly. The shape-correct stubs accept input and emit the locked
//! lifecycle events; the actual registry mutation happens in
//! `lingxi-coordinator` post-M5 when the cycle is resolved.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::pii::Verified;
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{
    TASK_CREATE_COMPLETED, TASK_CREATE_FAILED, TASK_CREATE_STARTED, TASK_GET_COMPLETED,
    TASK_GET_FAILED, TASK_GET_STARTED, TASK_LIST_COMPLETED, TASK_LIST_FAILED, TASK_LIST_STARTED,
    TASK_OUTPUT_COMPLETED, TASK_OUTPUT_FAILED, TASK_OUTPUT_STARTED, TASK_STOP_COMPLETED,
    TASK_STOP_FAILED, TASK_STOP_STARTED, TASK_UPDATE_COMPLETED, TASK_UPDATE_FAILED,
    TASK_UPDATE_STARTED,
};
use lingxi_telemetry::AnalyticsBus;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::builtin::BuiltinToolContext;
use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

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

/// The 7 task-type wire strings accepted by `TaskCreate`. Byte-aligned with
/// `lingxi_tasks::TaskType` variants (snake_case).
pub const TASK_TYPES: &[&str] = &[
    "local_bash",
    "local_agent",
    "remote_agent",
    "in_process_teammate",
    "local_workflow",
    "monitor_mcp",
    "dream",
];

/// The 5 task-status wire strings accepted by `TaskUpdate` / `TaskList`.
pub const TASK_STATUSES: &[&str] = &["pending", "running", "completed", "failed", "killed"];

/// Validate the M1 task-id format `[bartwmd][0-9a-z]{8}` (9 chars total).
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
/// Mirrors `lingxi_tasks::id::generate_task_id` without taking the
/// cyclic dep on `lingxi-tasks`.
fn fresh_task_id(prefix: char) -> String {
    use rand::Rng;
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let suffix: String = (0..8)
        .map(|_| ALPHA[rng.random_range(0..ALPHA.len())] as char)
        .collect();
    format!("{prefix}{suffix}")
}

fn task_type_prefix(s: &str) -> Result<char, String> {
    match s {
        "local_bash" => Ok('b'),
        "local_agent" => Ok('a'),
        "remote_agent" => Ok('r'),
        "in_process_teammate" => Ok('t'),
        "local_workflow" => Ok('w'),
        "monitor_mcp" => Ok('m'),
        "dream" => Ok('d'),
        other => Err(format!(
            "TaskCreate: unknown task_type '{other}' (known: {})",
            TASK_TYPES.join(", ")
        )),
    }
}

fn validate_task_status(s: &str) -> Result<(), String> {
    if TASK_STATUSES.contains(&s) {
        Ok(())
    } else {
        Err(format!(
            "Task: unknown status '{s}' (known: {})",
            TASK_STATUSES.join(", ")
        ))
    }
}

fn fresh_invocation_id() -> String {
    crate::builtin::file_read::ulid_or_uuid()
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

// ==== TaskCreateTool ========================================================

static TASK_CREATE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "task_type":   { "type": "string", "enum": ["local_bash","local_agent","remote_agent","in_process_teammate","local_workflow","monitor_mcp","dream"] },
            "description": { "type": "string", "minLength": 1 }
        },
        "required": ["task_type", "description"]
    })
});

/// Creates a new task entry.
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
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
                reason: "TaskCreate registers a new task in the in-process registry".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Create a task in the task registry".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskCreate to register a new task. task_type is one of \
         local_bash/local_agent/remote_agent/in_process_teammate/\
         local_workflow/monitor_mcp/dream."
            .into()
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

        let task_type_str = match input.get("task_type").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "missing_task_type",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskCreate: missing 'task_type'".into(),
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
        if description.trim().is_empty() {
            emit_failed(
                &bus,
                TASK_CREATE_FAILED,
                &invocation_id,
                "empty_description",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(
                "TaskCreate: description is empty".into(),
            ));
        }
        let prefix = match task_type_prefix(&task_type_str) {
            Ok(c) => c,
            Err(msg) => {
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "unknown_task_type",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(msg));
            }
        };

        emit_started(
            &bus,
            TASK_CREATE_STARTED,
            &invocation_id,
            &[(
                "task_type",
                AnalyticsValue::String(Verified::assert_safe(task_type_str.clone()).into_inner()),
            )],
        )
        .await;

        let task_id = fresh_task_id(prefix);

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
                "task_id": task_id,
                "task_type": task_type_str,
                "status": "pending",
                "description": description,
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
        "properties": { "task_id": { "type": "string" } },
        "required": ["task_id"]
    })
});

/// Returns a task's current state as JSON (surface stub).
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
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        "Read a task's current state".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskGet to look up a task by its 9-char id.".into()
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

        let task_id = match input.get("task_id").and_then(Value::as_str) {
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
                return Err(ToolError::InvalidInput("TaskGet: missing 'task_id'".into()));
            }
        };
        if let Err(msg) = validate_task_id(&task_id) {
            emit_failed(
                &bus,
                TASK_GET_FAILED,
                &invocation_id,
                "malformed_task_id",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }

        emit_started(&bus, TASK_GET_STARTED, &invocation_id, &[]).await;
        emit_completed(
            &bus,
            TASK_GET_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        // Shape-correct surface — real lookup arrives via the post-M5 wiring.
        Ok(ToolCallResult {
            data: json!({
                "task_id": task_id,
                "status": "pending",
                "task_type": "local_bash",
            }),
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
        "properties": {
            "status": { "type": "string", "enum": ["pending","running","completed","failed","killed"] }
        }
    })
});

/// Lists tasks, optionally filtered by status.
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
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        "List all tasks, optionally filtered by status".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskList to enumerate tasks. Optional 'status' filter is one of \
         pending/running/completed/failed/killed."
            .into()
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

        if let Some(s) = input.get("status").and_then(Value::as_str) {
            if let Err(msg) = validate_task_status(s) {
                emit_failed(
                    &bus,
                    TASK_LIST_FAILED,
                    &invocation_id,
                    "invalid_status",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(msg));
            }
        }

        emit_started(&bus, TASK_LIST_STARTED, &invocation_id, &[]).await;
        emit_completed(
            &bus,
            TASK_LIST_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[("count", AnalyticsValue::Int(0))],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({ "tasks": [] }),
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
        "properties": {
            "task_id": { "type": "string" },
            "status":  { "type": "string", "enum": ["pending","running","completed","failed","killed"] }
        },
        "required": ["task_id", "status"]
    })
});

/// Mutates a task's status (surface stub).
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
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
                reason: "TaskUpdate mutates registry state only".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Update a task's status".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskUpdate to transition a task to a new status.".into()
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

        let task_id = match input.get("task_id").and_then(Value::as_str) {
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
                    "TaskUpdate: missing 'task_id'".into(),
                ));
            }
        };
        if let Err(msg) = validate_task_id(&task_id) {
            emit_failed(
                &bus,
                TASK_UPDATE_FAILED,
                &invocation_id,
                "malformed_task_id",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }
        let status = match input.get("status").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_UPDATE_FAILED,
                    &invocation_id,
                    "missing_status",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskUpdate: missing 'status'".into(),
                ));
            }
        };
        if let Err(msg) = validate_task_status(&status) {
            emit_failed(
                &bus,
                TASK_UPDATE_FAILED,
                &invocation_id,
                "invalid_status",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }

        emit_started(&bus, TASK_UPDATE_STARTED, &invocation_id, &[]).await;
        emit_completed(
            &bus,
            TASK_UPDATE_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({ "task_id": task_id, "status": status }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskStopTool ===========================================================

static TASK_STOP_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": { "task_id": { "type": "string" } },
        "required": ["task_id"]
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
    fn input_schema(&self) -> &Value {
        &TASK_STOP_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        "Kill a running task".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskStop to cancel a running task by id.".into()
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

        let task_id = match input.get("task_id").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_STOP_FAILED,
                    &invocation_id,
                    "missing_task_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskStop: missing 'task_id'".into(),
                ));
            }
        };
        if let Err(msg) = validate_task_id(&task_id) {
            emit_failed(
                &bus,
                TASK_STOP_FAILED,
                &invocation_id,
                "malformed_task_id",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }

        emit_started(&bus, TASK_STOP_STARTED, &invocation_id, &[]).await;
        emit_completed(
            &bus,
            TASK_STOP_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({ "task_id": task_id, "status": "killed" }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// ==== TaskOutputTool =========================================================

static TASK_OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "task_id": { "type": "string" },
            "offset":  { "type": "integer", "minimum": 0 },
            "limit":   { "type": "integer", "minimum": 1, "maximum": 1_048_576 }
        },
        "required": ["task_id"]
    })
});

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
    fn input_schema(&self) -> &Value {
        &TASK_OUTPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        crate::shared::MAX_TOOL_OUTPUT_LENGTH
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
        "Read a task's stdout/stderr spool".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use TaskOutput to read accumulated stdout/stderr for a task.".into()
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

        let task_id = match input.get("task_id").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(
                    &bus,
                    TASK_OUTPUT_FAILED,
                    &invocation_id,
                    "missing_task_id",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                return Err(ToolError::InvalidInput(
                    "TaskOutput: missing 'task_id'".into(),
                ));
            }
        };
        if let Err(msg) = validate_task_id(&task_id) {
            emit_failed(
                &bus,
                TASK_OUTPUT_FAILED,
                &invocation_id,
                "malformed_task_id",
                started.elapsed().as_millis() as u64,
            )
            .await;
            return Err(ToolError::InvalidInput(msg));
        }

        emit_started(&bus, TASK_OUTPUT_STARTED, &invocation_id, &[]).await;
        emit_completed(
            &bus,
            TASK_OUTPUT_COMPLETED,
            &invocation_id,
            started.elapsed().as_millis() as u64,
            &[],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({
                "task_id": task_id,
                "content": "",
                "total_lines": 0,
                "truncated": false,
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

    #[test]
    fn task_type_prefix_unknown_carries_locked_string() {
        let err = task_type_prefix("bogus").unwrap_err();
        assert!(err.contains("unknown task_type 'bogus'"));
        assert!(err.contains("local_bash"));
    }
}
