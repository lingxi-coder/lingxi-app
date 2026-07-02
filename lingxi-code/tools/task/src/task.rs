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

/// Civil date `(year, month, day)` from days since the Unix epoch — port of
/// Howard Hinnant's `civil_from_days`. Valid for the proleptic Gregorian
/// calendar; we only ever feed it non-negative (post-1970) day counts.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ISO-8601 UTC timestamp with millisecond precision
/// (e.g. `2026-06-07T12:34:56.789Z`) — port of TS `new Date().toISOString()`.
/// Std-only; deterministic for a fixed `SystemTime`. Mirrors the proven
/// `tools/ui/src/brief.rs` impl (kept local to avoid a cross-crate dep).
fn iso8601_utc(t: std::time::SystemTime) -> String {
    let dur = t
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO);
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

// ==== Product-A V2 gating (sub-batch [2]) ===================================

/// Port of `isEnvTruthy(process.env[key])` (`envUtils.ts:32-37`): lower-cased,
/// trimmed value ∈ {`1`,`true`,`yes`,`on`}.
fn env_truthy(key: &str) -> bool {
    traits::env::is_env_truthy(std::env::var(key).ok().as_deref())
}

/// Pure core of [`is_todo_v2_enabled`] — 1:1 with the v2.1.183 binary `TE()`:
/// `function TE(){if(_l(process.env.LINGXI_ENABLE_TASKS))return!1;return!0}`.
/// i.e. V2-Task-tools-enabled = NOT (the env normalizes to `0`/`false`/`no`/`off`).
/// `_l` = [`traits::env::is_env_defined_falsy`] (byte-exact: `e===void 0`⇒false,
/// boolean⇒`!e`, else lowercased+trimmed ∈ {`0`,`false`,`no`,`off`}).
///
/// There is NO non-interactive term in the binary — the prior
/// `enable_tasks_env || !non_interactive` formula was stale (wrong/opposite
/// default for unset+non-interactive and for interactive+`false`/`no`/`off`).
fn todo_v2_enabled_inner(enable_tasks_env_defined_falsy: bool) -> bool {
    !enable_tasks_env_defined_falsy
}

/// Whether the Product-A V2 Task tools are advertised (and `TodoWrite` hidden).
///
/// Port of `isTodoV2Enabled()` (binary `TE()`): enabled UNLESS
/// `LINGXI_ENABLE_TASKS` is a *defined falsy* value (`0`/`false`/`no`/`off`,
/// case-insensitive, trimmed). Unset, empty, or any other value ⇒ enabled.
///
/// The `ctx` parameter is retained for the `Tool::is_enabled` signature but is
/// unused — the binary's `TE()` reads only `process.env`, no session/interactive
/// signal.
#[must_use]
pub fn is_todo_v2_enabled(_ctx: &ToolStaticContext) -> bool {
    todo_v2_enabled_inner(traits::env::is_env_defined_falsy(
        std::env::var("LINGXI_ENABLE_TASKS").ok().as_deref(),
    ))
}

/// Pure core of [`is_agent_swarms_enabled`] (`isAgentSwarmsEnabled`,
/// `utils/agentSwarmsEnabled.ts:24-44`): ant builds are always on; external
/// builds require opt-in via the experimental env var (or the `--agent-teams`
/// CLI flag). The GrowthBook `tengu_amber_flint` killswitch and the
/// `process.argv` flag are host-runtime signals not threaded into the tool
/// crate, so the env-driven core is ported here (the killswitch is `true` by
/// default upstream, and the flag is an alternate opt-in to the same env bit).
fn agent_swarms_enabled_inner(user_type_ant: bool, experimental_env: bool) -> bool {
    user_type_ant || experimental_env
}

/// Whether the agent-swarms/teammate surface is live at call time
/// (`isAgentSwarmsEnabled()`). Gates the `TaskUpdate` auto-owner + owner-change
/// mailbox notification side-effects (`TaskUpdateTool.ts:188-199,277-298`).
///
/// `isEnabled()` for the swarm *tools* reads the `agent_swarms_enabled`
/// [`ToolStaticContext`] feature flag (see `tool_team_create.rs`); the
/// side-effect path runs inside `call()` where only env signals are available,
/// so it mirrors the env-driven core of `agentSwarmsEnabled.ts` directly
/// (`USER_TYPE === 'ant'` OR a truthy `LINGXI_EXPERIMENTAL_AGENT_TEAMS`).
#[must_use]
pub fn is_agent_swarms_enabled() -> bool {
    let user_type_ant = std::env::var("USER_TYPE").is_ok_and(|v| v == "ant");
    agent_swarms_enabled_inner(user_type_ant, env_truthy("LINGXI_EXPERIMENTAL_AGENT_TEAMS"))
}

// ==== Verification nudge (sub-batch [5]) ====================================
//
// Structural verification nudge shared by the V2 `TaskUpdate` (todo store) and
// the V1 `TodoWrite` (in-memory session todos) tools. When the main-thread
// agent closes out a 3+ item list with every item completed and none of those
// items a verification step, the tool appends a reminder to its model-facing
// result text suggesting the model spawn the verification subagent.
//
// claude-code: `TaskUpdateTool.ts:326-349` + `:396-398` (regex over task
// subjects) and `TodoWriteTool.ts:72-86` + `:104-113` (regex over todo
// contents). The nudge suffix is byte-identical in both (em-dash U+2014).

/// `VERIFICATION_AGENT_TYPE` (`AgentTool/constants.ts:4`) — the `subagent_type`
/// the nudge tells the model to spawn.
pub(crate) const VERIFICATION_AGENT_TYPE: &str = "verification";

/// The exact claude-code verification-nudge suffix (`TaskUpdateTool.ts:397` /
/// `TodoWriteTool.ts:107`), with `${VERIFICATION_AGENT_TYPE}` interpolated and
/// the leading `\n\n`. The dash before "only the verifier" is an em-dash
/// (U+2014), matching the TS `—` / literal `—`.
pub(crate) fn verification_nudge_suffix() -> String {
    format!("\n\nNOTE: You just closed out 3+ tasks and none of them was a verification step. Before writing your final summary, spawn the verification agent (subagent_type=\"{VERIFICATION_AGENT_TYPE}\"). You cannot self-assign PARTIAL by listing caveats in your summary — only the verifier issues a verdict.")
}

/// Case-insensitive `/verif/i` test (`TaskUpdateTool.ts:345` over `t.subject` /
/// `TodoWriteTool.ts:83` over `t.content`). "verif" is ASCII, so
/// ASCII-lowercasing the haystack and substring-searching is equivalent to the
/// JS regex (no non-ASCII codepoint case-folds into `v`/`e`/`r`/`i`/`f`).
pub(crate) fn matches_verif(s: &str) -> bool {
    s.to_ascii_lowercase().contains("verif")
}

/// Whether the verification-nudge FEATURE is live at call time. claude gates the
/// nudge on `feature('VERIFICATION_AGENT') && getFeatureValue_CACHED_MAY_BE_STALE(
/// 'tengu_hive_evidence', false)` (`TaskUpdateTool.ts:334-335`). BOTH the bundle
/// feature and the GrowthBook flag default OFF in production, so the nudge never
/// reaches the model on the common interactive path. Neither real flag is
/// threaded into the tool crate yet, so this proxies them with an env opt-in that
/// is OFF by default — matching prod claude (no suffix). Swap this for the real
/// `feature(...) && getFeatureValue(...)` terms once the host threads them in.
pub(crate) fn verification_feature_enabled() -> bool {
    env_truthy("LINGXI_VERIFICATION_AGENT")
}

/// Shared predicate for the structural verification nudge — the common core of
/// `TaskUpdateTool.ts:333-349` and `TodoWriteTool.ts:77-86`. Returns `true`
/// when the nudge should be appended: the feature is on, this is the main
/// thread, every item is completed, there are `>= 3` items, and no item's
/// subject/content matches `/verif/i`.
///
/// `items` yields the per-item text the regex runs over (TaskUpdate: task
/// subjects; TodoWrite: todo contents). `all_completed` is whether every item
/// is `completed` (JS `Array.every`, vacuously `true` for an empty list — the
/// `count >= 3` guard rejects that case); `count` is the item count.
///
/// PURE predicate for the structural-shape part of the gate (no flag read): the
/// EXACT main-thread check `agent_id.is_none()` (== `!context.agentId`), the
/// conservative `!is_non_interactive_session` guard, every item completed,
/// `>= 3` items, and no item matching `/verif/i`. The FEATURE gate
/// ([`verification_feature_enabled`], OFF by default — mirroring
/// `feature('VERIFICATION_AGENT') && getFeatureValue('tengu_hive_evidence',
/// false)`, both OFF in prod) is applied SEPARATELY at the call site so this
/// predicate stays a pure, deterministic unit. `items` yields the per-item text
/// the `/verif/i` regex runs over (TaskUpdate: task subjects; TodoWrite:
/// contents). With the feature OFF by default, no suffix reaches the model on
/// the common interactive path — matching prod claude.
pub(crate) fn verification_nudge_needed<'a>(
    agent_id_is_none: bool,
    is_non_interactive_session: bool,
    all_completed: bool,
    count: usize,
    mut items: impl Iterator<Item = &'a str>,
) -> bool {
    !is_non_interactive_session
        && agent_id_is_none
        && all_completed
        && count >= 3
        && !items.any(matches_verif)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusInput {
    None,
    Deleted,
    State(TodoState),
}

/// Resolve the task-list id — 1:1 port of `getTaskListId()`
/// (`utils/tasks.ts:199-210`). Five-level precedence:
///
/// 1. `LINGXI_TASK_LIST_ID` env (explicit override).
/// 2. In-process teammate `teamName` ([`ToolUseContext::team_name`], TS
///    `getTeammateContext()?.teamName`) — so in-process teammates share the
///    leader's task list.
/// 3. `LINGXI_TEAM_NAME` env (TS `getTeamName()`, set when running as a
///    process-based teammate).
/// 4. Leader team name ([`traits::team_registry::leader_team_name`], TS
///    `leaderTeamName` set by `TeamCreate`).
/// 5. Session id (fallback for standalone sessions).
///
/// The leader and its in-process teammates resolve to the SAME on-disk task dir.
async fn resolve_task_list_id(ctx: &ToolUseContext) -> String {
    // 1. Explicit env override.
    if let Some(explicit) = std::env::var_os("LINGXI_TASK_LIST_ID") {
        if !explicit.is_empty() {
            return explicit.to_string_lossy().into_owned();
        }
    }
    // 2. In-process teammate team name (threaded on the call context).
    if let Some(team) = ctx.team_name.as_deref().filter(|t| !t.is_empty()) {
        return team.to_string();
    }
    // 3. LINGXI_TEAM_NAME env (process-based teammate; TS getTeamName()).
    if let Some(team) = std::env::var_os("LINGXI_TEAM_NAME") {
        if !team.is_empty() {
            return team.to_string_lossy().into_owned();
        }
    }
    // 4. Leader team name (set by TeamCreate via setLeaderTeamName).
    if let Some(team) = traits::team_registry::leader_team_name().filter(|t| !t.is_empty()) {
        return team;
    }
    // 5. Session id fallback.
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

/// `TaskCreate` prompt — swarms-ENABLED variant (`TaskCreateTool/prompt.ts`
/// `getPrompt()` with `isAgentSwarmsEnabled() === true`). Splices
/// `teammateContext` ( and potentially assigned to teammates) onto the
/// "multiple operations" bullet and inserts the two `teammateTips` bullets
/// before the final "Check TaskList first" tip. Byte-identical to the TS output.
const TASK_CREATE_PROMPT_SWARM: &str = r#"Use this tool to create a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.
It also helps the user understand the progress of the task and overall progress of their requests.

## When to Use This Tool

Use this tool proactively in these scenarios:

- Complex multi-step tasks - When a task requires 3 or more distinct steps or actions
- Non-trivial and complex tasks - Tasks that require careful planning or multiple operations and potentially assigned to teammates
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
- Include enough detail in the description for another agent to understand and complete the task
- New tasks are created with status 'pending' and no owner - use TaskUpdate with the `owner` parameter to assign them
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

/// `TaskList` prompt — swarms-ENABLED variant (`TaskListTool/prompt.ts`
/// `getPrompt()` with `isAgentSwarmsEnabled() === true`). Adds the
/// `teammateUseCase` bullet after the "need dependencies resolved" line and
/// appends the `## Teammate Workflow` section. Byte-identical to the TS output.
const TASK_LIST_PROMPT_SWARM: &str = r"Use this tool to list all tasks in the task list.

## When to Use This Tool

- To see what tasks are available to work on (status: 'pending', no owner, not blocked)
- To check overall progress on the project
- To find tasks that are blocked and need dependencies resolved
- Before assigning tasks to teammates, to see what's available
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

## Teammate Workflow

When working as a teammate:
1. After completing your current task, call TaskList to find available work
2. Look for tasks with status 'pending', no owner, and empty blockedBy
3. **Prefer tasks in ID order** (lowest ID first) when multiple tasks are available, as earlier tasks often set up context for later ones
4. Claim an available task using TaskUpdate (set `owner` to your name), or wait for leader assignment
5. If blocked, focus on unblocking tasks or notify the team lead
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
    fn search_hint(&self) -> Option<&str> {
        Some("create a task in the task list")
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
    // claude TaskCreate `isConcurrencySafe(){return!1}` — TaskCreate mutates the
    // shared task list, so it must NOT be batched concurrently with other tools.
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
        // getPrompt() (TaskCreateTool/prompt.ts): the swarms-ENABLED variant
        // splices the teammateContext / teammateTips inserts; the disabled
        // variant is byte-identical to the base const.
        if is_agent_swarms_enabled() {
            TASK_CREATE_PROMPT_SWARM.into()
        } else {
            TASK_CREATE_PROMPT.into()
        }
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
        let task = TodoTask::new(subject.clone(), description.clone(), active_form, metadata);
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

        // BLOCKING TaskCreated hooks (TaskCreateTool.ts:93-113). After the task
        // is persisted, fire the `TaskCreated` hook through the tool-path firer
        // (when one is wired). On a blocking error claude-code deletes the
        // just-created task and throws — so we roll the store entry back and
        // surface the block as a tool error. NOT swarm-gated: TS fires
        // `executeTaskCreatedHooks` unconditionally (the hook runs whenever a
        // `TaskCreated` hook is registered), so we do too. With no firer wired
        // (`None`) this is a no-op and creation proceeds — matching the
        // fire-and-forget registry path.
        if let Some(firer) = self.ctx.task_lifecycle_hooks.as_ref() {
            if let Err(reason) = firer
                .fire_task_created(
                    &task_id,
                    &subject,
                    Some(description.as_str()),
                    // claude-code `getAgentName()` / `getTeamName()`
                    // (`TaskCreateTool.ts:97-98`) — the creating teammate's
                    // display name + team, threaded on the call context.
                    ctx.agent_name.as_deref(),
                    ctx.team_name.as_deref(),
                )
                .await
            {
                // TS `await deleteTask(getTaskListId(), taskId)` then `throw`.
                store.delete(&task_id).await;
                emit_failed(
                    &bus,
                    TASK_CREATE_FAILED,
                    &invocation_id,
                    "blocked_by_hook",
                    started.elapsed().as_millis() as u64,
                )
                .await;
                // T24: each blocking error is wrapped by `getTaskCreatedHookMessage`
                // (`hooks.ts:1914-1917`) as `TaskCreated hook feedback:\n<error>`,
                // then the wrapped messages are `\n`-joined and thrown
                // (`TaskCreateTool.ts:106,112`). The firer aggregates the (possibly
                // several) blocking hooks into one `\n`-joined reason, so the prefix
                // applies once to the aggregate — byte-faithful with the TS feedback
                // the model sees.
                return Err(ToolError::Internal(format!(
                    "TaskCreated hook feedback:\n{reason}"
                )));
            }
        }
        // PARITY-GAP: context.setAppState(expandedView) (TaskCreateTool.ts:115-119)
        // is omitted — no app-state seam here.

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
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    fn search_hint(&self) -> Option<&str> {
        Some("retrieve a task by ID")
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
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    fn search_hint(&self) -> Option<&str> {
        Some("list all tasks")
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
        // getPrompt() (TaskListTool/prompt.ts): the swarms-ENABLED variant adds
        // the teammate use-case bullet + `## Teammate Workflow` section; the
        // disabled variant is byte-identical to the base const.
        if is_agent_swarms_enabled() {
            TASK_LIST_PROMPT_SWARM.into()
        } else {
            TASK_LIST_PROMPT.into()
        }
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
                // T27: `owner` is `z.string().optional()` (TaskListTool.ts:23) and
                // sourced from `task.owner` (`call`, line 80), which is `undefined`
                // when unset — so the key is OMITTED from the JSON, never emitted
                // as `owner: null`. Build the object and insert `owner` only when
                // present.
                let mut obj = json!({
                    "id": row.id,
                    "subject": row.subject,
                    "status": status_wire(row.status),
                    "blockedBy": row.blocked_by,
                });
                if let Some(owner) = row.owner.as_deref() {
                    obj["owner"] = Value::String(owner.to_string());
                }
                obj
            })
            .collect();
        Ok(ToolCallResult {
            data: json!({ "content": content, "tasks": tasks }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    fn search_hint(&self) -> Option<&str> {
        Some("update a task")
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
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
                emit_failed(
                    &bus,
                    TASK_UPDATE_FAILED,
                    &invocation_id,
                    "invalid_status",
                    duration(),
                )
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
                model_content: None,
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
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
        // Auto-set owner when a teammate marks a task as in_progress without
        // explicitly providing an owner. This ensures the task list can match
        // todo items to teammates for showing activity status
        // (TaskUpdateTool.ts:188-199). claude-code uses `getAgentName()` — the
        // teammate's DISPLAY NAME ([`ToolUseContext::agent_name`]), NOT the
        // `agent:<uuid>` id. `getAgentStatuses` matches owners against
        // name / name@team, never against an id, so writing the uuid here would
        // strand the owner. Skipped — exactly like TS when `getAgentName()`
        // returns `undefined` — when no display name is bound (the main thread /
        // leader); we must NOT write a uuid owner in that case.
        if is_agent_swarms_enabled()
            && status_input == StatusInput::State(TodoState::InProgress)
            && in_owner.is_none()
            && existing.owner.as_deref().is_none_or(str::is_empty)
        {
            if let Some(name) = ctx.agent_name.as_deref().filter(|n| !n.is_empty()) {
                new_owner = Some(name.to_string());
                updated_fields.push("owner".into());
            }
        }
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
                // BLOCKING TaskCompleted hooks (TaskUpdateTool.ts:230-265).
                // TS runs `executeTaskCompletedHooks` ONLY when a real
                // transition sets `status === 'completed'` (the V2 todo store's
                // sole terminal state — `failed` lives in the Product-B
                // registry, not here). Fire BEFORE applying the status: on a
                // blocking error claude-code returns `success:false` carrying
                // the reason and does NOT apply the status, so we return early
                // with that shape and leave `new_status` unset. NOT swarm-gated
                // (TS fires whenever a `TaskCompleted` hook is registered). With
                // no firer wired (`None`) this is a no-op — matching the
                // fire-and-forget registry path. Subject/description come from
                // the EXISTING task (TS `existingTask.subject` / `.description`).
                if st == TodoState::Completed {
                    if let Some(firer) = self.ctx.task_lifecycle_hooks.as_ref() {
                        if let Err(reason) = firer
                            .fire_task_completed(
                                &task_id,
                                status_wire(st),
                                &existing.subject,
                                Some(existing.description.as_str()),
                            )
                            .await
                        {
                            emit_completed(
                                &bus,
                                TASK_UPDATE_COMPLETED,
                                &invocation_id,
                                duration(),
                                &[],
                            )
                            .await;
                            return Ok(ToolCallResult {
                                data: json!({
                                    "content": render_task_update_fail(&task_id, Some(&reason)),
                                    "success": false,
                                    "taskId": task_id,
                                    "updatedFields": Vec::<String>::new(),
                                    "error": reason,
                                }),
                                model_content: None,
                                new_messages: vec![],
                                context_modifier: None,
                                is_error: false,
                                mcp_meta: None,
                            });
                        }
                    }
                }
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

        // Notify new owner via mailbox when ownership changes
        // (TaskUpdateTool.ts:277-298). Best-effort, like the TS `writeToMailbox`
        // (which swallows its own errors): a routing failure never fails the
        // TaskUpdate itself. `assignedBy` mirrors `getAgentName() || 'team-lead'`
        // — the acting teammate's DISPLAY NAME ([`ToolUseContext::agent_name`]),
        // else the literal `"team-lead"` label (NOT the `agent:<uuid>` id; the
        // recipient mailbox / `getAgentStatuses` key on names). The `from` route
        // address is that same name/label, which the router resolves to an id
        // (or accepts as the `team-lead` label).
        if is_agent_swarms_enabled() {
            if let (Some(owner), Some(router)) =
                (new_owner.clone(), self.ctx.mailbox_router.clone())
            {
                let sender_name = ctx
                    .agent_name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .map_or_else(|| "team-lead".to_string(), str::to_string);
                let timestamp = iso8601_utc(std::time::SystemTime::now());
                let assignment_message = serde_json::to_string(&json!({
                    "type": "task_assignment",
                    "taskId": task_id,
                    "subject": existing.subject,
                    "description": existing.description,
                    "assignedBy": sender_name,
                    "timestamp": timestamp,
                }))
                .unwrap_or_default();
                let msg = traits::mailbox::MailboxMessage {
                    message_id: fresh_invocation_id(),
                    content: assignment_message,
                    timestamp: std::time::SystemTime::now(),
                    // claude-code `color: getTeammateColor()` (TaskUpdateTool.ts:294).
                    // The no-arg `getTeammateColor()` returns the SENDER session's
                    // own assigned color (or `undefined`). The Rust host does not
                    // thread a per-session teammate color into the tool context, so
                    // the faithful value here is `None` — which serializes to NO
                    // `color` key, byte-identical with claude-code's
                    // `color: undefined` for the common main-thread / `team-lead`
                    // sender that has no assigned color.
                    color: None,
                };
                // Ignore the routing result — notification is best-effort.
                let _ = router.route(&sender_name, &owner, msg).await;
            }
        }

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
        // Structural verification nudge (TaskUpdateTool.ts:326-349 + 396-398).
        // Gated on the COMPUTED transition — TS checks `updates.status ===
        // 'completed'`, and `updates.status` is set only when `status !==
        // existingTask.status` (TaskUpdateTool.ts:230,267). The Rust mirror is
        // `new_status`, populated above only when `st != existing.status`, so a
        // no-op write that re-sends an already-`completed` status does NOT fire
        // the nudge. Gated (cheaply) with the main-thread + interactive checks
        // before re-listing the store; `store.list()` reflects the just-applied
        // update (the store mutation above already persisted it).
        let mut nudge_needed = false;
        if verification_feature_enabled()
            && new_status == Some(TodoState::Completed)
            && ctx.agent_id.is_none()
            && !ctx.options.is_non_interactive_session
        {
            let all_tasks = store.list().await;
            let all_done = all_tasks.iter().all(|t| t.status == TodoState::Completed);
            nudge_needed = verification_nudge_needed(
                ctx.agent_id.is_none(),
                ctx.options.is_non_interactive_session,
                all_done,
                all_tasks.len(),
                all_tasks.iter().map(|t| t.subject.as_str()),
            );
        }

        emit_completed(&bus, TASK_UPDATE_COMPLETED, &invocation_id, duration(), &[]).await;

        let mut content = render_task_update_success(&task_id, &updated_fields);
        // Teammate completion reminder (TaskUpdateTool.ts:386-394): when a
        // teammate (`getAgentId()`) closes a task to `completed` and swarms are
        // live, append the reminder. Gated on the COMPUTED transition's `to`
        // (`statusChange?.to === 'completed'`), which is `status_change`'s `to`.
        // Ordered BEFORE the verification nudge to match the TS suffix order.
        if matches!(status_change, Some((_, TodoState::Completed)))
            && ctx.agent_id.is_some()
            && is_agent_swarms_enabled()
        {
            content.push_str("\nTask completed. Call TaskList now to find your next available task or see if your work unblocked others.");
        }
        if nudge_needed {
            content.push_str(&verification_nudge_suffix());
        }
        let mut data = json!({
            "content": content,
            "success": true,
            "taskId": task_id,
            "updatedFields": updated_fields,
            "verificationNudgeNeeded": nudge_needed,
        });
        if let Some((from, to)) = status_change {
            data["statusChange"] = json!({ "from": status_wire(from), "to": status_wire(to) });
        }
        Ok(ToolCallResult {
            data,
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

// ==== TaskStopTool ===========================================================

// no-truncation: TaskStop emits a small fixed status string and needs no
// shared `MAX_TOOL_OUTPUT_LENGTH`/`truncate`. TaskOutput deliberately bounds
// its model-facing `<output>` via `format_task_output` (port of TS
// `formatTaskOutput`) instead of the shared 30_000-char head-truncate: a
// 32_000-char default (TASK_MAX_OUTPUT_LENGTH-overridable, clamped at 160_000)
// that keeps the TAIL behind a `[Truncated …]` header. The
// `max_result_size_chars` (100_000) value
// mirrored from claude-code's `maxResultSizeChars` is advisory metadata only —
// no production consumer enforces it, so it does NOT bound this output; the
// `format_task_output` cap is what actually bounds the TaskOutput surface.

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
        const ALIASES: &[&str] = &["KillShell", "KillBash"];
        ALIASES
    }
    /// `searchHint: 'kill a running background task'` (`TaskStopTool.ts:41`).
    fn search_hint(&self) -> Option<&str> {
        Some("kill a running background task")
    }
    /// `userFacingName: () => 'Stop Task'` for the `external` build
    /// (`TaskStopTool.ts:46` — `''` only for `USER_TYPE === 'ant'`).
    fn user_facing_name(&self) -> Option<&str> {
        Some("Stop Task")
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
    /// `shouldDefer: true` (`TaskStopTool.ts:53`).
    fn should_defer(&self) -> bool {
        true
    }
    /// `isConcurrencySafe() { return true }` (`TaskStopTool.ts:54-56`).
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
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

        // Capture command + type from the pre-kill record. claude-code
        // (`stopTask.ts:97`) computes
        // `command = isLocalShellTask(task) ? task.command : task.description`:
        // a `local_bash` task surfaces its shell COMMAND, every other task type
        // its DESCRIPTION. The registry threads `command` (populated for
        // `local_bash` only); fall back to `description` when absent.
        let task_type = record.task_type.clone();
        let command = match record.command.clone() {
            Some(cmd) => cmd,
            None => record.description.clone(),
        };

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
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

// ==== TaskOutputTool =========================================================

/// `TaskOutputTool` description (`TaskOutputTool.tsx` `async description()`).
const TASK_OUTPUT_DESCRIPTION: &str = "[Deprecated] — for bash and remote_agent tasks, prefer Read on the output file path; for local_agent tasks, use the Agent tool result directly";

/// `TaskOutputTool` prompt (`TaskOutputTool.tsx` `async prompt()`, verbatim).
const TASK_OUTPUT_PROMPT: &str = "DEPRECATED: Background tasks return their output file path in the tool result, and you receive a <task-notification> with the same path when the task completes.
- For bash tasks: prefer using the Read tool on that output file path — it contains stdout/stderr.
- For local_agent tasks: use the Agent tool result directly. Do NOT Read the .output file — it is a symlink to the full subagent conversation transcript (JSONL) and will overflow your context window.
- For remote_agent tasks: prefer using the Read tool on the output file path — it contains the streamed remote session output (same as bash).
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

/// Default cap on the model-facing `<output>` of `TaskOutput`
/// (`outputFormatting.ts:5` `TASK_MAX_OUTPUT_DEFAULT = 32_000`).
const TASK_MAX_OUTPUT_DEFAULT: usize = 32_000;
/// Upper clamp for the `TASK_MAX_OUTPUT_LENGTH` env override
/// (`outputFormatting.ts:4` `TASK_MAX_OUTPUT_UPPER_LIMIT = 160_000`).
const TASK_MAX_OUTPUT_UPPER_LIMIT: usize = 160_000;

/// Boolean that also accepts the string literals `"true"`/`"false"` — a port of
/// the TS `semanticBoolean()` (`utils/semanticBoolean.ts:22-29`) preprocess
/// step: `"true"`→`true`, `"false"`→`false`; anything else passes through to the
/// inner schema (here rejected → `None`, so the caller's `default` wins).
/// Mirrors the in-repo convention in `tools/ui/src/send_message.rs`.
fn semantic_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) if s == "true" => Some(true),
        Value::String(s) if s == "false" => Some(false),
        _ => None,
    }
}

/// T30: parse the `TaskOutput` `timeout` (ms). claude-code's schema is
/// `z.number().min(0).max(600000).default(30000)` (`TaskOutputTool.tsx:33`).
/// JSON numbers are floats, so an integer-typed `30000`, a float-typed `30000.0`,
/// and a fractional `30000.5` all pass `z.number()`. Read as `f64` (NOT `as_u64`,
/// which rejects any non-integer JSON number and silently falls back to the 30s
/// default), default `30000` when absent/non-numeric, and clamp into the
/// `[0, 600000]` bounds the zod `.min(0).max(600000)` enforces. The result is the
/// millisecond budget as a `u64`.
fn parse_timeout_ms(input: &Value) -> u64 {
    let raw = input
        .get("timeout")
        .and_then(Value::as_f64)
        .unwrap_or(30_000.0);
    raw.clamp(0.0, 600_000.0) as u64
}

/// Port of TS `parseInt(value, 10)`: skip leading ASCII whitespace, take an
/// optional `+`/`-` sign followed by base-10 digits, stop at the first
/// non-digit, and ignore the rest. Returns `None` for "NaN" (no leading
/// digits). Mirrors the parse used inside `validateBoundedIntEnvVar`.
fn parse_int_radix10(s: &str) -> Option<i128> {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\u{000B}', '\u{000C}']);
    let mut chars = s.chars().peekable();
    let mut buf = String::new();
    if let Some(&c) = chars.peek() {
        if c == '+' || c == '-' {
            buf.push(c);
            chars.next();
        }
    }
    let mut saw_digit = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            buf.push(c);
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }
    if !saw_digit {
        return None;
    }
    // Saturate huge magnitudes rather than failing: a 200-digit string is a
    // finite (very large) number in JS, which `validateBoundedIntEnvVar` then
    // caps to the upper limit. `i128::MAX` preserves the "> upper limit" path.
    Some(buf.parse::<i128>().unwrap_or(i128::MAX))
}

/// Port of `getMaxTaskOutputLength()` (`outputFormatting.ts:7-15`) over
/// `validateBoundedIntEnvVar` (`envValidation.ts:9-38`): `TASK_MAX_OUTPUT_LENGTH`
/// overrides the 32_000 default; an empty / non-positive / unparseable value
/// falls back to the default; anything above 160_000 is clamped down to it.
fn max_task_output_length() -> usize {
    let raw = match std::env::var("TASK_MAX_OUTPUT_LENGTH") {
        Ok(v) if !v.is_empty() => v,
        _ => return TASK_MAX_OUTPUT_DEFAULT,
    };
    match parse_int_radix10(&raw) {
        Some(parsed) if parsed > 0 => {
            if parsed > TASK_MAX_OUTPUT_UPPER_LIMIT as i128 {
                TASK_MAX_OUTPUT_UPPER_LIMIT
            } else {
                parsed as usize
            }
        }
        _ => TASK_MAX_OUTPUT_DEFAULT,
    }
}

/// The `[Truncated …]` header path. TS uses `getTaskOutputPath(taskId)`
/// (`diskOutput.ts:72-74`) = `<projectTempDir>/<sessionId>/tasks/<taskId>.output`.
///
/// The registry now threads the resolved ABSOLUTE spool path through
/// `TaskOutputChunk.output_path` (T11/T16), so when it is present this returns
/// the real absolute path the model can read. When the registry could not
/// resolve a path (`None`), it falls back to the deterministic filename portion
/// (`<taskId>.output`) — the truncation behaviour stays byte-faithful either way.
fn task_output_path(task_id: &str, output_path: Option<&str>) -> String {
    match output_path {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => format!("{task_id}.output"),
    }
}

/// Port of `formatTaskOutput` (`outputFormatting.ts:22-38`). When the output
/// length exceeds the (env-overridable) max, prepend a
/// `[Truncated. Full output: <path>]\n\n` header and keep the LAST
/// `maxLen - header.length` characters (the tail); otherwise return the output
/// unchanged.
///
/// TS measures `String.length`/`slice` in UTF-16 code units; this port measures
/// Unicode scalar values (`chars()`), which differ only for astral-plane chars.
/// For the ASCII/BMP output that task spools carry this is identical.
fn format_task_output(output: &str, task_id: &str, output_path: Option<&str>) -> String {
    let max_len = max_task_output_length();
    let char_count = output.chars().count();
    if char_count <= max_len {
        return output.to_string();
    }
    let header = format!(
        "[Truncated. Full output: {}]\n\n",
        task_output_path(task_id, output_path)
    );
    let available = max_len.saturating_sub(header.chars().count());
    // TS `output.slice(-availableSpace)` — keep the last `available` chars.
    let tail: String = output.chars().skip(char_count - available).collect();
    format!("{header}{tail}")
}

/// 1:1 port of `TaskOutputTool.tsx`'s `mapToolResultToToolResultBlockParam`
/// (lines 283-308): the XML render of a `retrieval_status` + optional `task`,
/// joined by a single newline (`n.join("\n")`). Fed to the model verbatim via
/// the `content` key.
fn render_task_output(retrieval_status: &str, task: Option<&TaskOutputView>) -> String {
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
        // `<output>` only when the RAW trimmed output is non-blank (TS
        // `output?.trim()`), then truncate-and-format and `.trimEnd()` the
        // result (TS: `formatTaskOutput(output, task_id)` → `content.trimEnd()`).
        if !t.output.trim().is_empty() {
            let formatted = format_task_output(&t.output, &t.task_id, t.output_path.as_deref());
            parts.push(format!("<output>\n{}\n</output>", formatted.trim_end()));
        }
        // `<error>` AFTER `<output>` (TS `mapToolResultToToolResultBlockParam`
        // lines 299-301: `if (data.task.error) parts.push(\`<error>…</error>\`)`).
        // Only emitted for a truthy (present, non-empty) error string — the
        // agent task type's `error` field.
        if let Some(error) = t.error.as_deref().filter(|e| !e.is_empty()) {
            parts.push(format!("<error>{error}</error>"));
        }
    }
    parts.join("\n")
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
    /// Agent-task error string (TS `TaskOutput.error`), rendered as a trailing
    /// `<error>…</error>` element after `<output>`. `None`/empty for non-agent
    /// tasks (and successful agents).
    error: Option<String>,
    /// Absolute on-disk spool path, threaded from `TaskOutputChunk.output_path`
    /// so the `[Truncated. Full output: <path>]` header shows the real path
    /// (claude-code `getTaskOutputPath(taskId)`). `None` ⟶ bare-filename fallback.
    output_path: Option<String>,
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
        const ALIASES: &[&str] = &[
            "AgentOutputTool",
            "BashOutputTool",
            "AgentOutput",
            "BashOutput",
        ];
        ALIASES
    }
    /// `searchHint: 'read output/logs from a background task'`
    /// (`TaskOutputTool.tsx:146`).
    fn search_hint(&self) -> Option<&str> {
        Some("read output/logs from a background task")
    }
    /// `userFacingName() { return 'Task Output' }` (`TaskOutputTool.tsx:151-153`).
    fn user_facing_name(&self) -> Option<&str> {
        Some("Task Output")
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
    /// `shouldDefer: true` (`TaskOutputTool.tsx:148`).
    fn should_defer(&self) -> bool {
        true
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
        ctx: ToolUseContext,
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
        // string-coerce `"true"`/`"false"` so a quoted `block:"false"` is honoured
        // (TS `semanticBoolean` preprocess), rather than falling through to the
        // `true` default. `timeout` defaults to 30000 ms
        // (`z.number().min(0).max(600000).default(30000)`).
        let block = input.get("block").and_then(semantic_bool).unwrap_or(true);
        let timeout_ms = parse_timeout_ms(&input);

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
                // Honour user cancellation mid-wait (TS `waitForTaskCompletion`
                // checks `abortController?.signal.aborted` at the top of each
                // poll iteration). The per-call `cancel` token fires when the
                // user interrupts (or a sibling tool errors); on cancellation we
                // stop polling and return the current (still-`timeout`) state
                // rather than blocking out the full timeout.
                if ctx.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
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

        // Mark the task notified once its terminal output has been consumed
        // (claude-code `TaskOutputTool`: `updateTaskState(task_id, t => ({ ...t,
        // notified: true }))` in BOTH the non-blocking terminal branch
        // (`status !== 'running' && status !== 'pending'`) and the blocking
        // terminal branch (after `waitForTaskCompletion`)). `chunk.done` is true
        // for exactly those terminal statuses, so a single guarded call covers
        // both. Suppresses a later duplicate `<task-notification>` for a task the
        // model has already seen; the registry also eagerly evicts the now
        // terminal+notified task. Best-effort: a failure here must not fail the
        // read that already succeeded.
        if chunk.done {
            let _ = registry.mark_notified(&task_id).await;
        }

        let retrieval_status = task_output_retrieval_status(chunk.done, block);
        // For agent tasks the registry resolves a CLEAN final answer
        // (`extractTextContent(agentTask.result.content, '\n')`); prefer it over
        // the raw on-disk transcript for the model-facing `<output>` (TS
        // `getTaskOutputData` `local_agent`: `output: cleanResult || output`).
        // `result` is `None` for non-agent tasks / empty extractions, leaving
        // the raw spool content in place.
        let output = chunk
            .result
            .clone()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| chunk.content.clone());
        let view = TaskOutputView {
            task_id: chunk.task_id.clone(),
            task_type: record.task_type.clone(),
            // Status from the (latest) chunk; fall back to the record if the
            // registry could not resolve it at the chunk point.
            status: chunk
                .status
                .clone()
                .unwrap_or_else(|| record.status.clone()),
            description: record.description.clone(),
            output,
            exit_code: chunk.exit_code,
            error: chunk.error.clone(),
            // Absolute spool path threaded from the registry (T11/T16) for the
            // `[Truncated. Full output: <path>]` header.
            output_path: chunk.output_path.clone(),
        };
        let content = render_task_output(retrieval_status, Some(&view));

        // `exit_code` is optional (TS attaches it only for `local_bash`); omit
        // the key entirely when the chunk carries none. `prompt` / `result` /
        // `error` ride only for agent tasks (TS `getTaskOutputData` adds them in
        // the `local_agent` branch), so each is emitted only when present.
        let mut task_obj = Map::new();
        task_obj.insert("task_id".into(), json!(view.task_id));
        task_obj.insert("task_type".into(), json!(view.task_type));
        task_obj.insert("status".into(), json!(view.status));
        task_obj.insert("description".into(), json!(view.description));
        task_obj.insert("output".into(), json!(view.output));
        if let Some(code) = view.exit_code {
            task_obj.insert("exit_code".into(), json!(code));
        }
        if let Some(prompt) = &chunk.prompt {
            task_obj.insert("prompt".into(), json!(prompt));
        }
        if let Some(result) = chunk.result.as_deref().filter(|r| !r.is_empty()) {
            task_obj.insert("result".into(), json!(result));
        }
        if let Some(error) = view.error.as_deref().filter(|e| !e.is_empty()) {
            task_obj.insert("error".into(), json!(error));
        }

        // Nested `{ retrieval_status, task: { … } }` plus the `content` render so
        // the model sees it (`TaskOutputTool.tsx` data + `…BlockParam`).
        Ok(ToolCallResult {
            data: json!({
                "retrieval_status": retrieval_status,
                "task": Value::Object(task_obj),
                "content": content,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
#[path = "task_test.rs"]
mod task_test;
