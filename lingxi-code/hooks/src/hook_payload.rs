//! Hook event payload (over-the-wire JSON) and response parser.
//!
//! Byte-locked against `claude-code/src/entrypoints/sdk/coreSchemas.ts:414-446`
//! (`PreToolUseHookInputSchema` / `PostToolUseHookInputSchema`) and
//! `claude-code/src/utils/hooks.ts:540-680` (response-processing block).

#![forbid(unsafe_code)]

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::response::{HookDecision, HookResponse};

/// Marker unit struct that serializes/deserializes as the literal `"PreToolUse"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEventNamePre;

impl Serialize for HookEventNamePre {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("PreToolUse")
    }
}
impl<'de> Deserialize<'de> for HookEventNamePre {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "PreToolUse" {
            Ok(Self)
        } else {
            Err(D::Error::custom(format!(
                "expected 'PreToolUse', got {s:?}"
            )))
        }
    }
}

/// Marker unit struct that serializes/deserializes as the literal `"PostToolUse"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEventNamePost;

impl Serialize for HookEventNamePost {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("PostToolUse")
    }
}
impl<'de> Deserialize<'de> for HookEventNamePost {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "PostToolUse" {
            Ok(Self)
        } else {
            Err(D::Error::custom(format!(
                "expected 'PostToolUse', got {s:?}"
            )))
        }
    }
}

/// Generate a marker unit struct that serializes/deserializes as a single
/// literal hook-event-name string. Mirrors the [`HookEventNamePre`] /
/// [`HookEventNamePost`] hand-written markers for the lifecycle events.
macro_rules! hook_event_name_marker {
    ($ty:ident, $lit:literal) => {
        #[doc = concat!("Marker unit struct that serializes/deserializes as the literal `\"", $lit, "\"`.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $ty;

        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str($lit)
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                if s == $lit {
                    Ok(Self)
                } else {
                    Err(D::Error::custom(format!(
                        concat!("expected '", $lit, "', got {:?}"),
                        s
                    )))
                }
            }
        }
    };
}

hook_event_name_marker!(HookEventNameStop, "Stop");
hook_event_name_marker!(HookEventNameSubagentStop, "SubagentStop");
hook_event_name_marker!(HookEventNameTaskCompleted, "TaskCompleted");
hook_event_name_marker!(HookEventNameTaskCreated, "TaskCreated");
hook_event_name_marker!(HookEventNameUserPromptSubmit, "UserPromptSubmit");
hook_event_name_marker!(HookEventNameSessionStart, "SessionStart");
hook_event_name_marker!(HookEventNameStopFailure, "StopFailure");
// B6 — additional lifecycle / environment events whose `HookEvent` variant
// already exists. Each marker serializes to exactly its wire literal.
hook_event_name_marker!(HookEventNamePostToolUseFailure, "PostToolUseFailure");
hook_event_name_marker!(HookEventNameSessionEnd, "SessionEnd");
hook_event_name_marker!(HookEventNamePreCompact, "PreCompact");
hook_event_name_marker!(HookEventNamePostCompact, "PostCompact");
hook_event_name_marker!(HookEventNameNotification, "Notification");
hook_event_name_marker!(HookEventNamePermissionRequest, "PermissionRequest");
hook_event_name_marker!(HookEventNamePermissionDenied, "PermissionDenied");
hook_event_name_marker!(HookEventNameSetup, "Setup");
hook_event_name_marker!(HookEventNameSubagentStart, "SubagentStart");
hook_event_name_marker!(HookEventNameCwdChanged, "CwdChanged");
hook_event_name_marker!(HookEventNameDirectoryAdded, "DirectoryAdded");
hook_event_name_marker!(HookEventNameFileChanged, "FileChanged");
hook_event_name_marker!(HookEventNameWorktreeRemove, "WorktreeRemove");
// Deferred-completion batch — the final four events whose `HookEvent` variant
// previously lacked a field to source a *required* wire value. Each marker
// serializes to exactly its wire literal.
hook_event_name_marker!(HookEventNameConfigChange, "ConfigChange");
hook_event_name_marker!(HookEventNameInstructionsLoaded, "InstructionsLoaded");
hook_event_name_marker!(HookEventNameElicitation, "Elicitation");
hook_event_name_marker!(HookEventNameWorktreeCreate, "WorktreeCreate");
hook_event_name_marker!(HookEventNameTeammateIdle, "TeammateIdle");
// #39 — three more user-configurable events.
hook_event_name_marker!(HookEventNamePostToolBatch, "PostToolBatch");
hook_event_name_marker!(HookEventNameUserPromptExpansion, "UserPromptExpansion");
hook_event_name_marker!(HookEventNameMessageDisplay, "MessageDisplay");
// ElicitationResult — fires after the user responds to an MCP elicitation
// (binary-confirmed at BIN off ~201751493; key literal `"ElicitationResult"`).
hook_event_name_marker!(HookEventNameElicitationResult, "ElicitationResult");

/// Wire-format `effort` object embedded in the base hook input shape
/// (1:1 with `coreSchemas.ts` base `RT` schema:
/// `effort: E.object({ level: E.string() }).optional()`).
///
/// claude-code construction (`createBaseHookInput`, minified `vd`):
/// `effort: s && getAppState && Lw(s) ? { level: jO(s, i) } : void 0` — i.e. the
/// object is present (`{ level }`) ONLY for hooks that fire within a tool-use
/// context on a model that supports the effort parameter, and omitted entirely
/// (the optional spread collapses) for session-lifecycle hooks and models
/// without effort support. `level` is the active effort level for the turn
/// (`"low"` / `"medium"` / `"high"` / `"xhigh"` / `"max"`), after any silent
/// downgrade for the selected model — the same value exposed to hook commands
/// and Bash as the `LINGXI_EFFORT` env var. Same shape as
/// `StatusLineCommandInput.effort`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct EffortLevel {
    pub level: String,
}

impl EffortLevel {
    /// Construct an `effort` object from the active effort `level` string.
    #[must_use]
    pub fn new(level: impl Into<String>) -> Self {
        Self {
            level: level.into(),
        }
    }
}

/// One entry in a `Stop` / `SubagentStop` payload `background_tasks` array
/// (1:1 with claude-code `Lic`). `id` / `type` / `status` / `description`
/// are always present in that order; the per-task-type extras are appended
/// after `description` (claude switches on `n.type`):
/// `local_bash` → `command`; `local_agent` → `agent_type`;
/// `monitor_mcp` / `mcp_task` → `server`, `tool`; `local_workflow` → `name`.
/// Each task type sets a disjoint subset, so declaration order
/// (`command < agent_type < server < tool < name`) reproduces every
/// claude case byte-exactly. The wire `type` value is claude's
/// kind-label map `O1o` (e.g. `local_bash` → `"shell"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct HookBackgroundTask {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub status: String,
    pub description: String,
    /// `local_bash` only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<String>,
    /// `local_agent` only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    /// `monitor_mcp` / `mcp_task`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub server: Option<String>,
    /// `monitor_mcp` / `mcp_task`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool: Option<String>,
    /// `local_workflow` only.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
}

/// One entry in a `Stop` / `SubagentStop` payload `session_crons` array
/// (1:1 with claude-code `Mic`): `{id, schedule, recurring, prompt}` in
/// that order. `schedule` ← claude `t.cron`; `recurring` ← `t.recurring ?? false`;
/// `prompt` is truncated to 1000 chars upstream (claude `TUe(t.prompt, 1000)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct HookSessionCron {
    pub id: String,
    pub schedule: String,
    pub recurring: bool,
    pub prompt: String,
}

/// Wire-format `PreToolUse` payload (1:1 with `coreSchemas.ts:414-423`).
///
/// Field meanings track claude-code exactly; see the schema reference above.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PreToolUsePayload {
    pub hook_event_name: HookEventNamePre,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// `prompt_id` — UUID correlating a user prompt with all subsequent
    /// events until the next prompt. Oracle `createBaseHookInput`
    /// (2.1.238 minified `c_`, BIN off 296935693) emits it between `cwd`
    /// and `permission_mode`: `prompt_id:Vut()??void 0`. Absent until the
    /// first user input of the process lifetime, so `None` omits the key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
}

/// Wire-format `PostToolUse` payload (1:1 with `coreSchemas.ts:436-446`).
///
/// Field meanings track claude-code exactly; see the schema reference above.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PostToolUsePayload {
    pub hook_event_name: HookEventNamePost,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_response: Value,
    pub tool_use_id: String,
    /// Tool execution time in ms (binary appends `duration_ms` last; schema
    /// `.number().optional()` — "Tool execution time in milliseconds. Excludes
    /// permission-prompt and hook time.").
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<u64>,
}

/// Wire-format `Stop` payload (1:1 with `coreSchemas.ts:513-527`
/// `StopHookInputSchema`; constructed at `utils/hooks.ts:3680-3684`).
///
/// Field order mirrors the existing tool payloads: `hook_event_name` first,
/// then the `createBaseHookInput` base fields, then the event-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct StopPayload {
    pub hook_event_name: HookEventNameStop,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub stop_hook_active: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_assistant_message: Option<String>,
    /// Background-task snapshot, spread LAST by claude (`...m`, after
    /// `last_assistant_message`) when a tool-use context is present. `None`
    /// (claude `m = void 0`) omits both this and `session_crons`; `Some([])`
    /// emits `[]`. 1:1 with claude `Lic(taskRegistry.all())`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub background_tasks: Option<Vec<HookBackgroundTask>>,
    /// Session-cron snapshot, spread immediately after `background_tasks`
    /// (1:1 with claude `Mic()`). See [`Self::background_tasks`] for the
    /// None-vs-empty semantics.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_crons: Option<Vec<HookSessionCron>>,
}

/// Wire-format `SubagentStop` payload (1:1 with `coreSchemas.ts:550-567`
/// `SubagentStopHookInputSchema`; constructed at `utils/hooks.ts:3671-3678`).
///
/// `agent_id` / `agent_type` are **required** here (vs. the optional base
/// fields) and `agent_transcript_path` is subagent-specific.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SubagentStopPayload {
    pub hook_event_name: HookEventNameSubagentStop,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    pub stop_hook_active: bool,
    pub agent_id: String,
    pub agent_transcript_path: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_assistant_message: Option<String>,
    /// Background-task snapshot — see [`StopPayload::background_tasks`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub background_tasks: Option<Vec<HookBackgroundTask>>,
    /// Session-cron snapshot — see [`StopPayload::session_crons`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_crons: Option<Vec<HookSessionCron>>,
}

/// Wire-format `TaskCompleted` payload (1:1 with `coreSchemas.ts:614-625`
/// `TaskCompletedHookInputSchema`; constructed at `utils/hooks.ts:3799-3807`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct TaskCompletedPayload {
    pub hook_event_name: HookEventNameTaskCompleted,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub task_id: String,
    pub task_subject: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub teammate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub team_name: Option<String>,
}

/// Wire-format `TaskCreated` payload (1:1 with `coreSchemas.ts:601-612`
/// `TaskCreatedHookInputSchema`; constructed at `utils/hooks.ts:3756-3764`).
///
/// Field set mirrors `TaskCompletedPayload` exactly (`task_id` / `task_subject`
/// required, `task_description` / `teammate_name` / `team_name` optional) — the
/// two schemas differ only in their `hook_event_name` literal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct TaskCreatedPayload {
    pub hook_event_name: HookEventNameTaskCreated,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub task_id: String,
    pub task_subject: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub teammate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub team_name: Option<String>,
}

/// Wire-format `TeammateIdle` payload (1:1 with `coreSchemas.ts:591-598`
/// `TeammateIdleHookInputSchema`; constructed at `utils/hooks.ts:3716-3720`).
///
/// Fired when a teammate's query loop stops and it is about to park awaiting the
/// next message (claude-code `stopHooks.ts:403`, gated on `isTeammate()`).
/// `teammate_name` / `team_name` are BOTH required strings in the TS schema
/// (unlike the `optional` pair on `TaskCompleted` / `TaskCreated`), so they are
/// non-`Option` here. `team_name` may be `""` when the firing scope cannot reach
/// the team identity (same documented leaf-scope gap as the `TaskCompleted`
/// `team_name: None`) — TS itself falls back to `getTeamName() ?? ''`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct TeammateIdlePayload {
    pub hook_event_name: HookEventNameTeammateIdle,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub teammate_name: String,
    pub team_name: String,
}

/// Wire-format `UserPromptSubmit` payload (1:1 with `coreSchemas.ts:484-491`
/// `UserPromptSubmitHookInputSchema`; constructed at `utils/hooks.ts:3840-3843`).
///
/// `session_title` is optional (binary-confirmed at BIN off 201745825:
/// `{hook_event_name:"UserPromptSubmit", prompt:string, session_title?:string}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct UserPromptSubmitPayload {
    pub hook_event_name: HookEventNameUserPromptSubmit,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub prompt: String,
    /// Current session title at the time the prompt is submitted (optional,
    /// binary-confirmed `session_title` key at BIN off 201745825).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_title: Option<String>,
}

/// Wire-format `SessionStart` payload (1:1 with `coreSchemas.ts:493-502`
/// `SessionStartHookInputSchema`; constructed at `utils/hooks.ts:3876-3881`).
///
/// `source` is one of `startup` / `resume` / `clear` / `compact` in TS;
/// modelled here as a free `String` (validation happens upstream).
/// `session_title` is optional (binary-confirmed at BIN off 201745825:
/// `{…, source:enum, agent_type?:string, model?:string, session_title?:string}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SessionStartPayload {
    pub hook_event_name: HookEventNameSessionStart,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Current session title at session start (optional,
    /// binary-confirmed `session_title` key at BIN off ~201746000).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_title: Option<String>,
}

/// Wire-format `StopFailure` payload (1:1 with `coreSchemas.ts:529-538`
/// `StopFailureHookInputSchema`; constructed at `utils/hooks.ts:3613-3619`).
///
/// `error` is one of the `SDKAssistantMessageError` enum members
/// (`coreSchemas.ts:1256-1266`); modelled here as a free `String`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct StopFailurePayload {
    pub hook_event_name: HookEventNameStopFailure,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_assistant_message: Option<String>,
}

/// Wire-format `PostToolUseFailure` payload (1:1 with `coreSchemas.ts:448-459`
/// `PostToolUseFailureHookInputSchema`; constructed at `utils/hooks.ts:3509-3517`).
///
/// The `HookEvent::PostToolUseFailure` variant carries the dispatched
/// `tool_input` (threaded from the turn loop's `effective_input`, matching the
/// `PostToolUse` success arm). The `is_interrupt` flag still defaults to `None`
/// until richer context is threaded through (consistent with the B1 lifecycle
/// arms' default-fill convention).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PostToolUseFailurePayload {
    pub hook_event_name: HookEventNamePostToolUseFailure,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_interrupt: Option<bool>,
    /// Tool execution time in ms (binary appends `duration_ms` after
    /// `is_interrupt`; schema `.number().optional()`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_ms: Option<u64>,
}

/// Wire-format `SessionEnd` payload (1:1 with `coreSchemas.ts:758-765`
/// `SessionEndHookInputSchema`; constructed at `utils/hooks.ts:4113-4117`).
///
/// `reason` is one of the `ExitReason` enum members (`coreSchemas.ts:747-754`);
/// modelled here as a free `String` (validation happens upstream).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SessionEndPayload {
    pub hook_event_name: HookEventNameSessionEnd,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub reason: String,
}

/// Wire-format `PreCompact` payload (1:1 with `coreSchemas.ts:569-577`
/// `PreCompactHookInputSchema`; constructed at `utils/hooks.ts:3972-3977`).
///
/// `trigger` is `manual` / `auto` in TS; modelled here as a free `String` fed
/// from `HookEvent::PreCompact.reason`. `custom_instructions` is `.nullable()`
/// (not `.optional()`) in the schema, so it is always serialized — as JSON
/// `null` when absent — rather than skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PreCompactPayload {
    pub hook_event_name: HookEventNamePreCompact,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub trigger: String,
    /// `.nullable()` in the schema — always present, `null` when absent.
    pub custom_instructions: Option<String>,
}

/// Wire-format `PostCompact` payload (1:1 with `coreSchemas.ts:579-589`
/// `PostCompactHookInputSchema`; constructed at `utils/hooks.ts:4044-4049`).
///
/// `trigger` (`manual` / `auto`) is fed from the `HookEvent::PostCompact`
/// variant's `trigger` field (the compaction firing site passes `"manual"` /
/// `"auto"`). `compact_summary` is fed from the variant's `summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PostCompactPayload {
    pub hook_event_name: HookEventNamePostCompact,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub trigger: String,
    pub compact_summary: String,
}

/// Wire-format `Notification` payload (1:1 with `coreSchemas.ts:473-482`
/// `NotificationHookInputSchema`; constructed at `utils/hooks.ts:3579-3585`).
///
/// `notification_type` is fed from `HookEvent::Notification.kind`. `title` has
/// no field on the variant yet and defaults to `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct NotificationPayload {
    pub hook_event_name: HookEventNameNotification,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    pub notification_type: String,
}

/// Wire-format `PermissionRequest` payload (1:1 with `coreSchemas.ts:425-434`
/// `PermissionRequestHookInputSchema`; constructed at `utils/hooks.ts:4174-4180`).
///
/// The schema carries `tool_name`, `tool_input`, and optional
/// `permission_suggestions`. The `HookEvent::PermissionRequest.reason` field has
/// no wire counterpart (the schema has no `reason`), so it is intentionally
/// dropped. `permission_suggestions` has no engine source yet → `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PermissionRequestPayload {
    pub hook_event_name: HookEventNamePermissionRequest,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_name: String,
    pub tool_input: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_suggestions: Option<Vec<Value>>,
}

/// Wire-format `PermissionDenied` payload (1:1 with `coreSchemas.ts:461-471`
/// `PermissionDeniedHookInputSchema`; constructed at `utils/hooks.ts:3545-3552`).
///
/// Carries `tool_name`, `tool_input`, `tool_use_id`, and `reason` — all required
/// by the schema and fed directly from the `HookEvent::PermissionDenied` variant.
/// Built with `createBaseHookInput(permissionMode, undefined, toolUseContext)`
/// (`utils/hooks.ts:3546`), so it threads the engine's `permission_mode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PermissionDeniedPayload {
    pub hook_event_name: HookEventNamePermissionDenied,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
    pub reason: String,
}

/// Wire-format `Setup` payload (1:1 with `coreSchemas.ts:504-511`
/// `SetupHookInputSchema`; constructed at `utils/hooks.ts:3908-3912`).
///
/// `trigger` (`init` / `maintenance`) is fed from the `HookEvent::Setup`
/// variant's `trigger` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SetupPayload {
    pub hook_event_name: HookEventNameSetup,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub trigger: String,
}

/// Wire-format `SubagentStart` payload (1:1 with `coreSchemas.ts:540-548`
/// `SubagentStartHookInputSchema`; constructed at `utils/hooks.ts:3938-3943`).
///
/// `agent_id` / `agent_type` are **required** here and fed directly from the
/// `HookEvent::SubagentStart` variant. `parent_agent_id` has no wire field and
/// is dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SubagentStartPayload {
    pub hook_event_name: HookEventNameSubagentStart,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    pub agent_id: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
}

/// Wire-format `CwdChanged` payload (1:1 with `coreSchemas.ts:727-735`
/// `CwdChangedHookInputSchema`; constructed at `utils/hooks.ts:4269-4274`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct CwdChangedPayload {
    pub hook_event_name: HookEventNameCwdChanged,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub old_cwd: String,
    pub new_cwd: String,
}

/// Wire-format `DirectoryAdded` payload (2.1.219).
///
/// claude-code `a$t` (BIN off 237753662):
/// `{...Kf(void 0),hook_event_name:"DirectoryAdded",directory:e,source:t}`,
/// dispatched with `matchQuery: t` — a hook `matcher` is therefore tested
/// against the SOURCE (`slash_command`, `register_repo_root`), not the path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryAddedPayload {
    pub hook_event_name: HookEventNameDirectoryAdded,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    /// The directory that was added.
    pub directory: String,
    /// What added it — also the matcher query.
    pub source: String,
}

/// Wire-format `FileChanged` payload (1:1 with `coreSchemas.ts:737-745`
/// `FileChangedHookInputSchema`; constructed at `utils/hooks.ts:4287-4292`).
///
/// `file_path` is fed from `HookEvent::FileChanged.path`; `event`
/// (`change` / `add` / `unlink`) from `.kind`, modelled as a free `String`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct FileChangedPayload {
    pub hook_event_name: HookEventNameFileChanged,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub file_path: String,
    pub event: String,
}

/// Wire-format `WorktreeRemove` payload (1:1 with `coreSchemas.ts:718-725`
/// `WorktreeRemoveHookInputSchema`; constructed at `utils/hooks.ts` worktree
/// removal site). `worktree_path` is fed from `HookEvent::WorktreeRemove.path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct WorktreeRemovePayload {
    pub hook_event_name: HookEventNameWorktreeRemove,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub worktree_path: String,
}

/// Wire-format `ConfigChange` payload (1:1 with `coreSchemas.ts:670-678`
/// `ConfigChangeHookInputSchema`; constructed at `utils/hooks.ts:4219-4224`).
///
/// `source` is required (one of [`ConfigChangeSource`]); `file_path` is
/// `.optional()` so it is skipped when absent. The base shape is built with
/// `createBaseHookInput(undefined)` — no `permission_mode` is threaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct ConfigChangePayload {
    pub hook_event_name: HookEventNameConfigChange,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub source: crate::events::ConfigChangeSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file_path: Option<String>,
}

/// Wire-format `InstructionsLoaded` payload (1:1 with `coreSchemas.ts:695-707`
/// `InstructionsLoadedHookInputSchema`; constructed at
/// `utils/hooks.ts:4353-4362`).
///
/// `file_path` / `memory_type` / `load_reason` are required; `globs` /
/// `trigger_file_path` / `parent_file_path` are `.optional()` and skipped when
/// absent. Built with `createBaseHookInput(undefined)` — no `permission_mode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct InstructionsLoadedPayload {
    pub hook_event_name: HookEventNameInstructionsLoaded,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub file_path: String,
    pub memory_type: crate::events::InstructionsMemoryType,
    pub load_reason: crate::events::InstructionsLoadReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub globs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trigger_file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_file_path: Option<String>,
}

/// Wire-format `Elicitation` payload (1:1 with `coreSchemas.ts:627-643`
/// `ElicitationHookInputSchema`; constructed at `utils/hooks.ts:4491-4500`).
///
/// `mcp_server_name` / `message` are required; `mode` / `url` /
/// `elicitation_id` / `requested_schema` are `.optional()` and skipped when
/// absent. Built with `createBaseHookInput(permissionMode)` — unlike the other
/// three deferred events, `permission_mode` IS threaded here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct ElicitationPayload {
    pub hook_event_name: HookEventNameElicitation,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub mcp_server_name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mode: Option<crate::events::ElicitationMode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub elicitation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub requested_schema: Option<Value>,
}

/// Wire-format `ElicitationResult` payload. 1:1 with the binary's
/// `ElicitationResultHookInputSchema` (BIN off ~201751493):
/// `gT().and({hook_event_name:"ElicitationResult", mcp_server_name:string,
/// elicitation_id?:string, mode?:enum(["form","url"]),
/// action:enum(["accept","decline","cancel"]), content?:record(string,unknown)})`.
///
/// Fired after the user responds to an MCP elicitation (or a hook intercepts it).
/// `action` is REQUIRED; `elicitation_id`, `mode`, and `content` are optional.
/// Built with `createBaseHookInput(permissionMode)` (same as `Elicitation`), so
/// `permission_mode` IS threaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct ElicitationResultPayload {
    pub hook_event_name: HookEventNameElicitationResult,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    /// Name of the MCP server that requested elicitation (wire `mcp_server_name`,
    /// required). Sourced from `HookEvent::ElicitationResult.server_name`.
    pub mcp_server_name: String,
    /// Server-assigned elicitation ID (optional).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub elicitation_id: Option<String>,
    /// Presentation mode (`form` / `url`), if specified (optional).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mode: Option<crate::events::ElicitationMode>,
    /// User's response action (required): `"accept"` / `"decline"` / `"cancel"`.
    /// Extracted from `HookEvent::ElicitationResult.result["action"]` or defaults
    /// to `"cancel"` when the field is absent from the result JSON.
    pub action: String,
    /// Structured form content returned by the user (optional). Extracted from
    /// `HookEvent::ElicitationResult.result["content"]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content: Option<Value>,
}

/// Wire-format `WorktreeCreate` payload (1:1 with `coreSchemas.ts:709-716`
/// `WorktreeCreateHookInputSchema`; constructed at `utils/hooks.ts:4931-4935`).
///
/// `name` is the only event-specific field — the hook's stdout returns the
/// resolved worktree path, so the input carries just the requested `name`.
/// Built with `createBaseHookInput(undefined)` — no `permission_mode`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct WorktreeCreatePayload {
    pub hook_event_name: HookEventNameWorktreeCreate,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub name: String,
}

/// Wire-format `PostToolBatch` payload (#39). 1:1 with claude-code's input
/// schema (BIN off 200753854): the base shape `.and({hook_event_name:
/// "PostToolBatch", tool_calls:E.array(ggp())})`, where each element is a
/// [`crate::events::PostToolBatchCall`]. Fired once after every tool call in a
/// batch resolves, before the next model request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PostToolBatchPayload {
    pub hook_event_name: HookEventNamePostToolBatch,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub tool_calls: Vec<crate::events::PostToolBatchCall>,
}

/// Wire-format `UserPromptExpansion` payload (#39). 1:1 with claude-code's input
/// schema (BIN off 200754686): the base shape `.and({hook_event_name:
/// "UserPromptExpansion", expansion_type:E.enum(["slash_command","mcp_prompt"]),
/// command_name:string, command_args:string, command_source:string().optional(),
/// prompt:string})`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct UserPromptExpansionPayload {
    pub hook_event_name: HookEventNameUserPromptExpansion,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub expansion_type: crate::events::PromptExpansionType,
    pub command_name: String,
    pub command_args: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command_source: Option<String>,
    pub prompt: String,
}

/// Wire-format `MessageDisplay` payload (#39). 1:1 with claude-code's input
/// schema (BIN off 200761745): the base shape `.and({hook_event_name:
/// "MessageDisplay", turn_id:string, message_id:string, index:number().int(),
/// final:bool, delta:string})`. The `final` wire key is renamed from the Rust
/// keyword via serde. Fired per assistant-message flush, synchronously, with
/// per-invocation telemetry suppressed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct MessageDisplayPayload {
    pub hook_event_name: HookEventNameMessageDisplay,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    /// See [`PreToolUsePayload::prompt_id`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<EffortLevel>,
    pub turn_id: String,
    pub message_id: String,
    pub index: u64,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub delta: String,
}

/// Envelope used to send one of either payload kind across the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(missing_docs, reason = "variants delegate to documented payload types")]
pub enum HookEventEnvelope {
    Pre(PreToolUsePayload),
    Post(PostToolUsePayload),
}

/// Failure modes from [`parse_response`].
#[derive(Debug, Clone, Error)]
pub enum HookResponseParseError {
    /// JSON deserialisation failed.
    #[error("hook response is not valid JSON: {0}")]
    Json(String),
    /// JSON parsed but the top level is not an object.
    #[error("hook response is not a JSON object")]
    NotObject,
    /// `hookSpecificOutput.hookEventName` didn't match the expected event.
    #[error("hook response hookEventName mismatch: expected '{expected}', got '{got}'")]
    EventNameMismatch {
        /// Expected event name (`"PreToolUse"` or `"PostToolUse"`).
        expected: &'static str,
        /// Actual value the hook returned.
        got: String,
    },
    /// Legacy top-level `decision` had an unrecognised value. Mirrors `azn`'s
    /// first switch `default: throw Error(`Unknown hook decision type:
    /// ${e.decision}. Valid types are: approve, block`)` (claude-code BIN off
    /// ~205721920) — the binary rejects the WHOLE hook output, so we surface a
    /// parse error rather than silently ignoring the unknown value.
    #[error("Unknown hook decision type: {value}. Valid types are: approve, block")]
    UnknownDecision {
        /// The unrecognised `decision` value the hook returned.
        value: String,
    },
    /// `hookSpecificOutput.permissionDecision` had an unrecognised value.
    /// Mirrors `azn`'s second switch `default: throw Error(`Unknown hook
    /// permissionDecision type: ${...}. Valid types are: allow, deny, ask,
    /// defer`)` (claude-code BIN off ~205721920).
    #[error(
        "Unknown hook permissionDecision type: {value}. Valid types are: allow, deny, ask, defer"
    )]
    UnknownPermissionDecision {
        /// The unrecognised `permissionDecision` value the hook returned.
        value: String,
    },
}

/// Parse a hook's JSON reply into a [`HookResponse`].
///
/// `expected_event` is `"PreToolUse"` or `"PostToolUse"` — validates the
/// nested `hookSpecificOutput.hookEventName` per `hooks.ts:585`.
pub fn parse_response(
    raw: &str,
    expected_event: &'static str,
) -> Result<HookResponse, HookResponseParseError> {
    let v: Value =
        serde_json::from_str(raw).map_err(|e| HookResponseParseError::Json(e.to_string()))?;
    let obj = v.as_object().ok_or(HookResponseParseError::NotObject)?;
    let mut resp = HookResponse::default();

    // continue / stopReason. `continue: false` is the *preventContinuation*
    // signal for lifecycle (Stop / SubagentStop / TaskCompleted) hooks
    // (claude-code `hooks.ts:404`, `query.ts:1278`): it terminates the agent
    // loop regardless of any `decision: block`. `stopReason` becomes the
    // surfaced `reason`. For non-lifecycle events the orchestrator ignores
    // `prevent_continuation`, so this stays behavior-neutral there (B4).
    let cont = obj.get("continue").and_then(Value::as_bool).unwrap_or(true);
    if !cont {
        resp.prevent_continuation = true;
        if let Some(reason) = obj.get("stopReason").and_then(Value::as_str) {
            resp.reason = Some(reason.to_string());
        }
    }

    // suppressOutput
    if let Some(b) = obj.get("suppressOutput").and_then(Value::as_bool) {
        resp.suppress_output = b;
    }

    // systemMessage
    if let Some(s) = obj.get("systemMessage").and_then(Value::as_str) {
        resp.system_message = Some(s.to_string());
    }

    // #40 top-level `terminalSequence` (claude-code schema BIN off 200873127).
    // A TOP-LEVEL field (NOT under hookSpecificOutput) read for ALL hook result
    // types. The allowlist validation (`NEo`) runs at APPLY time (the consumer),
    // so parse just captures the raw string here.
    if let Some(ts) = obj.get("terminalSequence").and_then(Value::as_str) {
        resp.terminal_sequence = Some(ts.to_string());
    }

    // Legacy top-level `decision` (claude-code `azn`'s FIRST switch, BIN off
    // ~205721920: `if(e.decision)switch(e.decision){case"approve":…;case"block":
    // …;default:throw Error("Unknown hook decision type: …. Valid types are:
    // approve, block")}`). The `if(e.decision)` guard means a falsy value
    // (absent / empty string / `null`) is skipped; a PRESENT but unrecognised
    // string THROWS (rejects the whole hook output). We mirror both: only a
    // non-empty string reaches the switch, and any value other than
    // `approve`/`block` returns `UnknownDecision`.
    match obj.get("decision").and_then(Value::as_str) {
        // `if(e.decision)` is falsy for the empty string — skip it like the binary.
        None | Some("") => {}
        Some("block") => resp.decision = Some(HookDecision::Block),
        Some("approve") => resp.decision = Some(HookDecision::Approve),
        Some(other) => {
            return Err(HookResponseParseError::UnknownDecision {
                value: other.to_string(),
            })
        }
    }

    // NOTE: the binary NEVER reads a BARE top-level `e.permissionDecision`
    // (`azn` only ever consults `e.hookSpecificOutput.permissionDecision`, and
    // only when `hookSpecificOutput.hookEventName==="PreToolUse"`). The previous
    // top-level `permissionDecision` parse was a non-parity divergence and has
    // been removed (R-O2a) — `permissionDecision` is honoured ONLY under
    // `hookSpecificOutput` below.
    // Top-level `reason` (claude-code: the block-message resolution
    // `blockingError = e.hookSpecificOutput.permissionDecisionReason || e.reason
    // || "Blocked by hook"`, plus the general `{decision:"block", reason}` output
    // shape). `permissionDecisionReason` is a hookSpecificOutput-ONLY field in
    // CC's schema (`permissionDecisionReason:S.string().optional()` inside the
    // hookSpecificOutput object) — there is NO top-level `permissionDecisionReason`,
    // so the previous top-level read of that key was a phantom that both invented a
    // non-existent field AND dropped the real top-level `reason` (a
    // `{decision:"block", reason:"…"}` hook surfaced an empty block message). Read
    // top-level `reason` here; the lower `hookSpecificOutput.permissionDecisionReason`
    // parse (below) overrides it, preserving CC's `hsOut.permissionDecisionReason ||
    // e.reason` precedence.
    if let Some(r) = obj.get("reason").and_then(Value::as_str) {
        resp.reason = Some(r.to_string());
    }

    // hookSpecificOutput
    if let Some(hs) = obj.get("hookSpecificOutput").and_then(Value::as_object) {
        if let Some(name) = hs.get("hookEventName").and_then(Value::as_str) {
            if name != expected_event {
                return Err(HookResponseParseError::EventNameMismatch {
                    expected: expected_event,
                    got: name.to_string(),
                });
            }
        }
        if let Some(upd) = hs.get("updatedInput") {
            resp.updated_input = Some(upd.clone());
        }
        // `hookSpecificOutput.updatedMCPToolOutput` (claude-code
        // `parseHookJSONOutput`, `utils/hooks.ts:646-649`). The TS switch only
        // reads this for the `PostToolUse` case, so we scope it to that event —
        // a hook returning it on any other event has it ignored, exactly as in
        // TS. TS guards on truthiness (`if (json.hookSpecificOutput
        // .updatedMCPToolOutput)`), so a JSON `null` / `false` / `0` / `""` is
        // NOT treated as a replacement; `Value::is_null()` covers the `null`
        // case (the only one expressible once the key is present), keeping a
        // hook that explicitly returns `null` a no-op like TS.
        if expected_event == "PostToolUse" {
            // `hookSpecificOutput.updatedToolOutput` (claude-code BIN off
            // 205724076: `if(e.hookSpecificOutput.updatedToolOutput!==void 0)
            // u.updatedToolOutput=e.hookSpecificOutput.updatedToolOutput`).
            // `!== void 0` semantics — the key being PRESENT (even with a JSON
            // `null` value) IS a replacement, distinct from the legacy MCP field
            // below which is truthiness-gated. `Map::get` returns `Some` only
            // when the key is present, so this faithfully mirrors `!== void 0`:
            // an explicit `null` becomes `Some(Some(Value::Null))` (a
            // replacement with null), an omitted key leaves the outer `None`.
            if let Some(out) = hs.get("updatedToolOutput") {
                resp.updated_tool_output = Some(Some(out.clone()));
            }
            if let Some(out) = hs.get("updatedMCPToolOutput") {
                if !out.is_null() {
                    resp.updated_mcp_tool_output = Some(out.clone());
                }
            }
        }
        // `hookSpecificOutput.additionalContext` (claude-code
        // `result.additionalContext`, `utils/hooks.ts:622`). Kept DISTINCT from
        // the top-level `systemMessage` — claude-code routes them to SEPARATE
        // attachments (`hook_additional_context` vs `hook_system_message`) where
        // only `additionalContext` reaches the model (`utils/messages.ts:4117`
        // vs `:4258`). We must NOT merge them into one field.
        if let Some(addl) = hs.get("additionalContext").and_then(Value::as_str) {
            resp.additional_context = Some(addl.to_string());
        }
        // `hookSpecificOutput.retry` (claude-code `parseHookJSONOutput`,
        // `case 'PermissionDenied': result.retry = json.hookSpecificOutput.retry`,
        // `utils/hooks.ts:654-655`). The TS switch reads this ONLY for the
        // `PermissionDenied` case, so we scope it to that event — a hook
        // returning `retry` on any other event has it ignored, exactly as in TS.
        if expected_event == "PermissionDenied" {
            if let Some(retry) = hs.get("retry").and_then(Value::as_bool) {
                resp.retry = Some(retry);
            }
        }
        // `hookSpecificOutput.permissionDecision` switch (claude-code `azn`'s
        // SECOND switch, BIN off ~205721920:
        // `if(e.hookSpecificOutput?.hookEventName==="PreToolUse"
        //   && e.hookSpecificOutput.permissionDecision)switch(...){
        //     case"allow":u.permissionBehavior="allow";break;
        //     case"deny":u.permissionBehavior="deny",…;break;
        //     case"ask":u.permissionBehavior="ask";break;
        //     case"defer":u.permissionBehavior="defer";break;
        //     default:throw Error("Unknown hook permissionDecision type: …")}`).
        // THREE parity rules captured here:
        //   (R-O2b) GATED to `hookEventName === "PreToolUse"` — for any other
        //           event the binary never enters this switch.
        //   (R-D2)  UNCONDITIONAL reassignment — `permissionBehavior` is set
        //           outright (no `if (decision != Block)` guard), so a hsOut
        //           permissionDecision ALWAYS overrides a prior legacy
        //           `decision:"block"`. Target: `{"decision":"block",
        //           "hookSpecificOutput":{"permissionDecision":"allow"}}` →
        //           Approve (the binary's `permissionBehavior="allow"`).
        //   (R-O2c) `default: throw` — an unrecognised value rejects the whole
        //           hook output (returns `UnknownPermissionDecision`).
        // `allow`→`Approve` (skips the prompt), `deny`→`Block`, `ask`→`Ask`
        // (R-D3, forces the interactive prompt), `defer`→`Defer`.
        if expected_event == "PreToolUse" {
            match hs.get("permissionDecision").and_then(Value::as_str) {
                // The binary guards the switch on `&& e.hookSpecificOutput
                // .permissionDecision` (truthy) — a missing key OR an empty
                // string is falsy, so the switch is SKIPPED (no throw).
                None | Some("") => {}
                Some("allow") => resp.decision = Some(HookDecision::Approve),
                Some("deny") => resp.decision = Some(HookDecision::Block),
                Some("ask") => resp.decision = Some(HookDecision::Ask),
                Some("defer") => resp.decision = Some(HookDecision::Defer),
                Some(other) => {
                    return Err(HookResponseParseError::UnknownPermissionDecision {
                        value: other.to_string(),
                    })
                }
            }
        }
        if let Some(r) = hs.get("permissionDecisionReason").and_then(Value::as_str) {
            resp.reason = Some(r.to_string());
        }

        // Elicitation answer (claude-code `parseElicitationHookOutput`,
        // `utils/hooks.ts:4434-4446`): `hookSpecificOutput.action` is the
        // elicitation response action; `.content` the optional form content.
        // A `decline` action additionally drives a block (the JS path sets a
        // `blockingError` => the handler returns `{action:'decline'}`), so we
        // map it onto `HookDecision::Block` here, preserving any earlier
        // reason. Only set when an `action` is present, exactly like the JS
        // `if (!specific.action) return {}`. Applies to both `Elicitation` and
        // `ElicitationResult` events (binary-confirmed: both share the same
        // hookSpecificOutput shape: `{hookEventName, action?, content?}`).
        if let Some(action) = hs.get("action").and_then(Value::as_str) {
            resp.elicitation_response = Some(crate::response::ElicitationHookResponse {
                action: action.to_string(),
                content: hs.get("content").cloned(),
            });
            if action == "decline" {
                resp.decision = Some(HookDecision::Block);
            }
        }

        // `hookSpecificOutput.sessionTitle` (binary-confirmed at BIN off
        // 201754804: gated to `hookEventName === "UserPromptSubmit"`).
        // Allows a hook to rename the session. The orchestrator applies it
        // via the session-title update path.
        if expected_event == "UserPromptSubmit" {
            if let Some(title) = hs.get("sessionTitle").and_then(Value::as_str) {
                resp.session_title = Some(title.to_string());
            }
            // `hookSpecificOutput.suppressOriginalPrompt` (binary-confirmed at
            // BIN off 201754804: boolean, description "When decision is 'block',
            // omit the original prompt from the block message"). Scoped to
            // `UserPromptSubmit` only (the binary's schema gate).
            if let Some(b) = hs.get("suppressOriginalPrompt").and_then(Value::as_bool) {
                resp.suppress_original_prompt = b;
            }
        }

        // `hookSpecificOutput.displayContent` (binary-confirmed at BIN off
        // 201757586: gated to `hookEventName === "MessageDisplay"`). Replaces
        // the assistant delta on-screen; does NOT affect the stored message.
        // Scoped to `MessageDisplay` (the binary's schema gate).
        if expected_event == "MessageDisplay" {
            if let Some(dc) = hs.get("displayContent").and_then(Value::as_str) {
                resp.display_content = Some(dc.to_string());
            }
        }

        // `hookSpecificOutput.watchPaths` (claude-code `parseHookJSONOutput`:
        // `"watchPaths" in e.hookSpecificOutput && e.hookSpecificOutput.watchPaths`
        // — captured on both the exec and MCP hook-result paths). A `FileChanged`
        // / `CwdChanged` hook may add paths to the file-changed watch set. The
        // binary's parse is NOT gated on `hookEventName` (unlike `sessionTitle` /
        // `displayContent`) — the `"watchPaths" in hsOut` test stands alone — so we
        // capture it for ANY event; only the file-changed watcher's FileChanged /
        // CwdChanged flows consume it, leaving other events behavior-neutral.
        // Present-key capture: a PRESENT array (even empty) becomes
        // `Some(vec![...])` — JS arrays are always truthy, so `&& hsOut.watchPaths`
        // keeps an empty `[]` — distinguishing it from an absent key (`None`); a
        // non-array value has no `Vec<String>` representation and is ignored.
        if let Some(arr) = hs.get("watchPaths").and_then(Value::as_array) {
            resp.watch_paths = Some(
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            );
        }

        // `hookSpecificOutput.{initialUserMessage,reloadSkills}` (claude-code
        // `SessionStart` consumer: `if(p.initialUserMessage)$os=p.initialUserMessage`
        // and `if(p.reloadSkills)u=!0`). Scoped to `SessionStart` (the binary reads
        // these only in the SessionStart case of its result switch).
        if expected_event == "SessionStart" {
            if let Some(m) = hs.get("initialUserMessage").and_then(Value::as_str) {
                resp.initial_user_message = Some(m.to_string());
            }
            if let Some(b) = hs.get("reloadSkills").and_then(Value::as_bool) {
                resp.reload_skills = Some(b);
            }
        }
    }

    Ok(resp)
}

#[cfg(test)]
#[path = "hook_payload_test.rs"]
mod hook_payload_test;
