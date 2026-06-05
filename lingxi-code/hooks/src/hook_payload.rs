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
hook_event_name_marker!(HookEventNameSetup, "Setup");
hook_event_name_marker!(HookEventNameSubagentStart, "SubagentStart");
hook_event_name_marker!(HookEventNameCwdChanged, "CwdChanged");
hook_event_name_marker!(HookEventNameFileChanged, "FileChanged");
hook_event_name_marker!(HookEventNameWorktreeRemove, "WorktreeRemove");
// Deferred-completion batch — the final four events whose `HookEvent` variant
// previously lacked a field to source a *required* wire value. Each marker
// serializes to exactly its wire literal.
hook_event_name_marker!(HookEventNameConfigChange, "ConfigChange");
hook_event_name_marker!(HookEventNameInstructionsLoaded, "InstructionsLoaded");
hook_event_name_marker!(HookEventNameElicitation, "Elicitation");
hook_event_name_marker!(HookEventNameWorktreeCreate, "WorktreeCreate");

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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_response: Value,
    pub tool_use_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub stop_hook_active: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_assistant_message: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    pub stop_hook_active: bool,
    pub agent_id: String,
    pub agent_transcript_path: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_assistant_message: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub task_id: String,
    pub task_subject: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub teammate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub team_name: Option<String>,
}

/// Wire-format `UserPromptSubmit` payload (1:1 with `coreSchemas.ts:484-491`
/// `UserPromptSubmitHookInputSchema`; constructed at `utils/hooks.ts:3840-3843`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct UserPromptSubmitPayload {
    pub hook_event_name: HookEventNameUserPromptSubmit,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub prompt: String,
}

/// Wire-format `SessionStart` payload (1:1 with `coreSchemas.ts:493-502`
/// `SessionStartHookInputSchema`; constructed at `utils/hooks.ts:3876-3881`).
///
/// `source` is one of `startup` / `resume` / `clear` / `compact` in TS;
/// modelled here as a free `String` (validation happens upstream).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SessionStartPayload {
    pub hook_event_name: HookEventNameSessionStart,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_use_id: String,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_interrupt: Option<bool>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub trigger: String,
    /// `.nullable()` in the schema — always present, `null` when absent.
    pub custom_instructions: Option<String>,
}

/// Wire-format `PostCompact` payload (1:1 with `coreSchemas.ts:579-589`
/// `PostCompactHookInputSchema`; constructed at `utils/hooks.ts:4044-4049`).
///
/// `trigger` (`manual` / `auto`) has no field on the `HookEvent::PostCompact`
/// variant yet, so it defaults to an empty string (the B1 default-fill
/// convention). `compact_summary` is fed from the variant's `summary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct PostCompactPayload {
    pub hook_event_name: HookEventNamePostCompact,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub tool_name: String,
    pub tool_input: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_suggestions: Option<Vec<Value>>,
}

/// Wire-format `Setup` payload (1:1 with `coreSchemas.ts:504-511`
/// `SetupHookInputSchema`; constructed at `utils/hooks.ts:3908-3912`).
///
/// `trigger` (`init` / `maintenance`) has no field on the unit
/// `HookEvent::Setup` variant yet, so it defaults to an empty string (the B1
/// default-fill convention).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(missing_docs, reason = "wire-format mirror of claude-code schema")]
pub struct SetupPayload {
    pub hook_event_name: HookEventNameSetup,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    pub agent_id: String,
    pub agent_type: String,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub old_cwd: String,
    pub new_cwd: String,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    pub name: String,
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

    // legacy decision
    match obj.get("decision").and_then(Value::as_str) {
        Some("block") => resp.decision = Some(HookDecision::Block),
        Some("approve") => resp.decision = Some(HookDecision::Approve),
        _ => {}
    }

    // top-level permissionDecision (preferred over legacy)
    match obj.get("permissionDecision").and_then(Value::as_str) {
        Some("allow") => {
            if resp.decision.is_none() {
                resp.decision = Some(HookDecision::Approve);
            }
        }
        Some("deny") => resp.decision = Some(HookDecision::Block),
        // "ask" and any other value fall through — preserve the existing decision.
        _ => {}
    }
    if let Some(r) = obj.get("permissionDecisionReason").and_then(Value::as_str) {
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
        if let Some(addl) = hs.get("additionalContext").and_then(Value::as_str) {
            let combined = match resp.system_message.take() {
                Some(prev) => format!("{prev}\n{addl}"),
                None => addl.to_string(),
            };
            resp.system_message = Some(combined);
        }
        match hs.get("permissionDecision").and_then(Value::as_str) {
            Some("allow") => {
                if !matches!(resp.decision, Some(HookDecision::Block)) {
                    resp.decision = Some(HookDecision::Approve);
                }
            }
            Some("deny") => resp.decision = Some(HookDecision::Block),
            _ => {}
        }
        if let Some(r) = hs.get("permissionDecisionReason").and_then(Value::as_str) {
            resp.reason = Some(r.to_string());
        }
    }

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pre_payload_serializes_byte_lock() {
        let p = PreToolUsePayload {
            hook_event_name: HookEventNamePre,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: "tu-1".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.starts_with(r#"{"hook_event_name":"PreToolUse","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"tu-1"}"#));
    }

    #[test]
    fn post_payload_includes_tool_response() {
        let p = PostToolUsePayload {
            hook_event_name: HookEventNamePost,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            tool_name: "Read".into(),
            tool_input: json!({"path": "/x"}),
            tool_response: json!({"content": "data"}),
            tool_use_id: "tu-2".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""hook_event_name":"PostToolUse""#));
        assert!(s.contains(r#""tool_response":{"content":"data"}"#));
    }

    #[test]
    fn round_trip_pre_payload() {
        let p = PreToolUsePayload {
            hook_event_name: HookEventNamePre,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("plan".into()),
            agent_id: Some("a-1".into()),
            agent_type: Some("general-purpose".into()),
            tool_name: "Edit".into(),
            tool_input: json!({"file_path": "/f"}),
            tool_use_id: "tu".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: PreToolUsePayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tool_name, "Edit");
        assert_eq!(back.agent_type.as_deref(), Some("general-purpose"));
    }

    #[test]
    fn parse_response_allow_via_permission_decision() {
        let r = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Approve));
    }

    #[test]
    fn parse_response_block_via_legacy_decision() {
        let r = parse_response(
            r#"{"decision":"block","stopReason":"because","continue":false}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.decision, Some(HookDecision::Block));
        assert_eq!(r.reason.as_deref(), Some("because"));
    }

    #[test]
    fn parse_response_event_mismatch_errors() {
        let err = parse_response(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse"}}"#,
            "PreToolUse",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            HookResponseParseError::EventNameMismatch { .. }
        ));
    }

    #[test]
    fn parse_response_additional_context_appends_to_system_message() {
        let r = parse_response(
            r#"{"systemMessage":"hello","hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"world"}}"#,
            "PreToolUse",
        )
        .unwrap();
        assert_eq!(r.system_message.as_deref(), Some("hello\nworld"));
    }

    // ---- B1: lifecycle-event payload byte-lock tests --------------------

    #[test]
    fn stop_payload_serializes_byte_lock() {
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            stop_hook_active: true,
            last_assistant_message: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","stop_hook_active":true}"#
        );
    }

    #[test]
    fn stop_payload_serializes_with_last_message() {
        let p = StopPayload {
            hook_event_name: HookEventNameStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("default".into()),
            agent_id: None,
            agent_type: None,
            stop_hook_active: false,
            last_assistant_message: Some("done".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Stop","session_id":"s","transcript_path":"/t","cwd":"/w","permission_mode":"default","stop_hook_active":false,"last_assistant_message":"done"}"#
        );
    }

    #[test]
    fn subagent_stop_payload_serializes_byte_lock() {
        let p = SubagentStopPayload {
            hook_event_name: HookEventNameSubagentStop,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            stop_hook_active: true,
            agent_id: "agent-7".into(),
            agent_transcript_path: "/tmp/agent-7.jsonl".into(),
            agent_type: "general-purpose".into(),
            last_assistant_message: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SubagentStop","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","stop_hook_active":true,"agent_id":"agent-7","agent_transcript_path":"/tmp/agent-7.jsonl","agent_type":"general-purpose"}"#
        );
    }

    #[test]
    fn task_completed_payload_serializes_byte_lock() {
        let p = TaskCompletedPayload {
            hook_event_name: HookEventNameTaskCompleted,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            task_id: "task-42".into(),
            task_subject: "Build the thing".into(),
            task_description: None,
            teammate_name: None,
            team_name: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCompleted","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","task_id":"task-42","task_subject":"Build the thing"}"#
        );
    }

    #[test]
    fn task_completed_payload_serializes_with_optionals() {
        let p = TaskCompletedPayload {
            hook_event_name: HookEventNameTaskCompleted,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            task_id: "t1".into(),
            task_subject: "subj".into(),
            task_description: Some("desc".into()),
            teammate_name: Some("alice".into()),
            team_name: Some("core".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"TaskCompleted","session_id":"s","transcript_path":"/t","cwd":"/w","task_id":"t1","task_subject":"subj","task_description":"desc","teammate_name":"alice","team_name":"core"}"#
        );
    }

    #[test]
    fn user_prompt_submit_payload_serializes_byte_lock() {
        let p = UserPromptSubmitPayload {
            hook_event_name: HookEventNameUserPromptSubmit,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            prompt: "fix the bug".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","prompt":"fix the bug"}"#
        );
    }

    #[test]
    fn session_start_payload_serializes_byte_lock() {
        let p = SessionStartPayload {
            hook_event_name: HookEventNameSessionStart,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            source: "startup".into(),
            agent_type: None,
            model: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionStart","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","source":"startup"}"#
        );
    }

    #[test]
    fn session_start_payload_serializes_with_model() {
        let p = SessionStartPayload {
            hook_event_name: HookEventNameSessionStart,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            source: "resume".into(),
            agent_type: Some("code-reviewer".into()),
            model: Some("claude-opus".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionStart","session_id":"s","transcript_path":"/t","cwd":"/w","source":"resume","agent_type":"code-reviewer","model":"claude-opus"}"#
        );
    }

    #[test]
    fn stop_failure_payload_serializes_byte_lock() {
        let p = StopFailurePayload {
            hook_event_name: HookEventNameStopFailure,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            error: "rate_limit".into(),
            error_details: None,
            last_assistant_message: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"StopFailure","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","error":"rate_limit"}"#
        );
    }

    #[test]
    fn stop_failure_payload_serializes_with_details() {
        let p = StopFailurePayload {
            hook_event_name: HookEventNameStopFailure,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            error: "server_error".into(),
            error_details: Some("upstream 500".into()),
            last_assistant_message: Some("partial".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"StopFailure","session_id":"s","transcript_path":"/t","cwd":"/w","error":"server_error","error_details":"upstream 500","last_assistant_message":"partial"}"#
        );
    }

    // ---- B1: expected_event marker round-trips --------------------------

    #[test]
    fn lifecycle_event_name_markers_round_trip() {
        // Each marker serializes to exactly its literal and deserializes back.
        assert_eq!(
            serde_json::to_string(&HookEventNameStop).unwrap(),
            r#""Stop""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameSubagentStop).unwrap(),
            r#""SubagentStop""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameTaskCompleted).unwrap(),
            r#""TaskCompleted""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameUserPromptSubmit).unwrap(),
            r#""UserPromptSubmit""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameSessionStart).unwrap(),
            r#""SessionStart""#
        );
        assert_eq!(
            serde_json::to_string(&HookEventNameStopFailure).unwrap(),
            r#""StopFailure""#
        );

        let _: HookEventNameStop = serde_json::from_str(r#""Stop""#).unwrap();
        let _: HookEventNameSubagentStop = serde_json::from_str(r#""SubagentStop""#).unwrap();
        let _: HookEventNameTaskCompleted = serde_json::from_str(r#""TaskCompleted""#).unwrap();
        let _: HookEventNameUserPromptSubmit =
            serde_json::from_str(r#""UserPromptSubmit""#).unwrap();
        let _: HookEventNameSessionStart = serde_json::from_str(r#""SessionStart""#).unwrap();
        let _: HookEventNameStopFailure = serde_json::from_str(r#""StopFailure""#).unwrap();
    }

    #[test]
    fn lifecycle_event_name_marker_rejects_wrong_literal() {
        assert!(serde_json::from_str::<HookEventNameStop>(r#""SubagentStop""#).is_err());
        assert!(serde_json::from_str::<HookEventNameStopFailure>(r#""Stop""#).is_err());
    }

    #[test]
    fn lifecycle_payloads_round_trip() {
        let p = SubagentStopPayload {
            hook_event_name: HookEventNameSubagentStop,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("plan".into()),
            stop_hook_active: true,
            agent_id: "a-1".into(),
            agent_transcript_path: "/t/a-1.jsonl".into(),
            agent_type: "general-purpose".into(),
            last_assistant_message: Some("hi".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: SubagentStopPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.agent_id, "a-1");
        assert!(back.stop_hook_active);
        assert_eq!(back.last_assistant_message.as_deref(), Some("hi"));
    }

    // ---- B1: parse_response validates the new hookEventNames ------------

    #[test]
    fn parse_response_validates_new_event_names() {
        for name in [
            "Stop",
            "SubagentStop",
            "TaskCompleted",
            "UserPromptSubmit",
            "SessionStart",
            "StopFailure",
        ] {
            // Matching event name parses fine.
            let raw = format!(
                r#"{{"hookSpecificOutput":{{"hookEventName":"{name}","additionalContext":"x"}}}}"#
            );
            let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
            let r = parse_response(&raw, leaked).unwrap();
            assert_eq!(r.system_message.as_deref(), Some("x"));

            // A mismatched name is rejected.
            let err = parse_response(
                r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse"}}"#,
                leaked,
            )
            .unwrap_err();
            assert!(matches!(
                err,
                HookResponseParseError::EventNameMismatch { .. }
            ));
        }
    }

    // ---- B6: additional-event payload byte-lock tests -------------------

    #[test]
    fn post_tool_use_failure_payload_serializes_byte_lock() {
        let p = PostToolUseFailurePayload {
            hook_event_name: HookEventNamePostToolUseFailure,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            tool_name: "Bash".into(),
            tool_input: Value::Null,
            tool_use_id: "tu-1".into(),
            error: "boom".into(),
            is_interrupt: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PostToolUseFailure","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":null,"tool_use_id":"tu-1","error":"boom"}"#
        );
    }

    #[test]
    fn post_tool_use_failure_payload_serializes_with_interrupt() {
        let p = PostToolUseFailurePayload {
            hook_event_name: HookEventNamePostToolUseFailure,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            tool_name: "Edit".into(),
            tool_input: json!({"file_path": "/f"}),
            tool_use_id: "tu".into(),
            error: "cancelled".into(),
            is_interrupt: Some(true),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""tool_input":{"file_path":"/f"}"#));
        assert!(s.contains(r#""error":"cancelled","is_interrupt":true"#));
    }

    #[test]
    fn session_end_payload_serializes_byte_lock() {
        let p = SessionEndPayload {
            hook_event_name: HookEventNameSessionEnd,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            reason: "logout".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SessionEnd","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","reason":"logout"}"#
        );
    }

    #[test]
    fn pre_compact_payload_serializes_null_custom_instructions() {
        // `custom_instructions` is `.nullable()` (not optional) — must always
        // be present, serialized as `null` when absent.
        let p = PreCompactPayload {
            hook_event_name: HookEventNamePreCompact,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            trigger: "manual".into(),
            custom_instructions: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PreCompact","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":"manual","custom_instructions":null}"#
        );
    }

    #[test]
    fn pre_compact_payload_serializes_with_custom_instructions() {
        let p = PreCompactPayload {
            hook_event_name: HookEventNamePreCompact,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            trigger: "auto".into(),
            custom_instructions: Some("keep the API surface".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""trigger":"auto","custom_instructions":"keep the API surface""#));
    }

    #[test]
    fn post_compact_payload_serializes_byte_lock() {
        let p = PostCompactPayload {
            hook_event_name: HookEventNamePostCompact,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            trigger: String::new(),
            compact_summary: "did the thing".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PostCompact","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":"","compact_summary":"did the thing"}"#
        );
    }

    #[test]
    fn notification_payload_serializes_byte_lock() {
        let p = NotificationPayload {
            hook_event_name: HookEventNameNotification,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            message: "build done".into(),
            title: None,
            notification_type: "info".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Notification","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","message":"build done","notification_type":"info"}"#
        );
    }

    #[test]
    fn notification_payload_serializes_with_title() {
        let p = NotificationPayload {
            hook_event_name: HookEventNameNotification,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            message: "msg".into(),
            title: Some("Heads up".into()),
            notification_type: "warn".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""message":"msg","title":"Heads up","notification_type":"warn""#));
    }

    #[test]
    fn permission_request_payload_serializes_byte_lock() {
        let p = PermissionRequestPayload {
            hook_event_name: HookEventNamePermissionRequest,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            tool_name: "Bash".into(),
            tool_input: json!({"command": "rm -rf /"}),
            permission_suggestions: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"PermissionRequest","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}"#
        );
    }

    #[test]
    fn setup_payload_serializes_byte_lock() {
        let p = SetupPayload {
            hook_event_name: HookEventNameSetup,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            trigger: String::new(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Setup","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","trigger":""}"#
        );
    }

    #[test]
    fn subagent_start_payload_serializes_byte_lock() {
        let p = SubagentStartPayload {
            hook_event_name: HookEventNameSubagentStart,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: "agent-7".into(),
            agent_type: "general-purpose".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"SubagentStart","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","agent_id":"agent-7","agent_type":"general-purpose"}"#
        );
    }

    #[test]
    fn cwd_changed_payload_serializes_byte_lock() {
        let p = CwdChangedPayload {
            hook_event_name: HookEventNameCwdChanged,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            old_cwd: "/old".into(),
            new_cwd: "/new".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"CwdChanged","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","old_cwd":"/old","new_cwd":"/new"}"#
        );
    }

    #[test]
    fn file_changed_payload_serializes_byte_lock() {
        let p = FileChangedPayload {
            hook_event_name: HookEventNameFileChanged,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            file_path: "/work/src/main.rs".into(),
            event: "change".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"FileChanged","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","file_path":"/work/src/main.rs","event":"change"}"#
        );
    }

    #[test]
    fn worktree_remove_payload_serializes_byte_lock() {
        let p = WorktreeRemovePayload {
            hook_event_name: HookEventNameWorktreeRemove,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            worktree_path: "/work/.worktrees/feat".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"WorktreeRemove","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","worktree_path":"/work/.worktrees/feat"}"#
        );
    }

    #[test]
    fn b6_event_name_markers_round_trip() {
        for (got, want) in [
            (serde_json::to_string(&HookEventNamePostToolUseFailure).unwrap(), r#""PostToolUseFailure""#),
            (serde_json::to_string(&HookEventNameSessionEnd).unwrap(), r#""SessionEnd""#),
            (serde_json::to_string(&HookEventNamePreCompact).unwrap(), r#""PreCompact""#),
            (serde_json::to_string(&HookEventNamePostCompact).unwrap(), r#""PostCompact""#),
            (serde_json::to_string(&HookEventNameNotification).unwrap(), r#""Notification""#),
            (serde_json::to_string(&HookEventNamePermissionRequest).unwrap(), r#""PermissionRequest""#),
            (serde_json::to_string(&HookEventNameSetup).unwrap(), r#""Setup""#),
            (serde_json::to_string(&HookEventNameSubagentStart).unwrap(), r#""SubagentStart""#),
            (serde_json::to_string(&HookEventNameCwdChanged).unwrap(), r#""CwdChanged""#),
            (serde_json::to_string(&HookEventNameFileChanged).unwrap(), r#""FileChanged""#),
            (serde_json::to_string(&HookEventNameWorktreeRemove).unwrap(), r#""WorktreeRemove""#),
        ] {
            assert_eq!(got, want);
        }
        let _: HookEventNamePostToolUseFailure =
            serde_json::from_str(r#""PostToolUseFailure""#).unwrap();
        let _: HookEventNameSessionEnd = serde_json::from_str(r#""SessionEnd""#).unwrap();
        let _: HookEventNameWorktreeRemove = serde_json::from_str(r#""WorktreeRemove""#).unwrap();
        assert!(serde_json::from_str::<HookEventNameSetup>(r#""Notification""#).is_err());
    }

    // ---- deferred-completion batch: final-four payload byte-lock tests ------

    #[test]
    fn config_change_payload_serializes_byte_lock() {
        let p = ConfigChangePayload {
            hook_event_name: HookEventNameConfigChange,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            source: crate::events::ConfigChangeSource::LocalSettings,
            file_path: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"ConfigChange","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","source":"local_settings"}"#
        );
    }

    #[test]
    fn config_change_payload_serializes_with_file_path() {
        let p = ConfigChangePayload {
            hook_event_name: HookEventNameConfigChange,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            source: crate::events::ConfigChangeSource::PolicySettings,
            file_path: Some("/etc/claude/policy.json".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"ConfigChange","session_id":"s","transcript_path":"/t","cwd":"/w","source":"policy_settings","file_path":"/etc/claude/policy.json"}"#
        );
    }

    #[test]
    fn instructions_loaded_payload_serializes_byte_lock() {
        let p = InstructionsLoadedPayload {
            hook_event_name: HookEventNameInstructionsLoaded,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            file_path: "/work/CLAUDE.md".into(),
            memory_type: crate::events::InstructionsMemoryType::Project,
            load_reason: crate::events::InstructionsLoadReason::SessionStart,
            globs: None,
            trigger_file_path: None,
            parent_file_path: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"InstructionsLoaded","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","file_path":"/work/CLAUDE.md","memory_type":"Project","load_reason":"session_start"}"#
        );
    }

    #[test]
    fn instructions_loaded_payload_serializes_with_optionals() {
        let p = InstructionsLoadedPayload {
            hook_event_name: HookEventNameInstructionsLoaded,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            file_path: "/w/rules/api.md".into(),
            memory_type: crate::events::InstructionsMemoryType::Managed,
            load_reason: crate::events::InstructionsLoadReason::Compact,
            globs: Some(vec!["src/**/*.rs".into()]),
            trigger_file_path: Some("/w/src/main.rs".into()),
            parent_file_path: Some("/w/CLAUDE.md".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"InstructionsLoaded","session_id":"s","transcript_path":"/t","cwd":"/w","file_path":"/w/rules/api.md","memory_type":"Managed","load_reason":"compact","globs":["src/**/*.rs"],"trigger_file_path":"/w/src/main.rs","parent_file_path":"/w/CLAUDE.md"}"#
        );
    }

    #[test]
    fn elicitation_payload_serializes_byte_lock() {
        let p = ElicitationPayload {
            hook_event_name: HookEventNameElicitation,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            mcp_server_name: "github".into(),
            message: "Authorize?".into(),
            mode: None,
            url: None,
            elicitation_id: None,
            requested_schema: None,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Elicitation","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","mcp_server_name":"github","message":"Authorize?"}"#
        );
    }

    #[test]
    fn elicitation_payload_serializes_with_optionals_and_permission_mode() {
        // Elicitation alone threads permission_mode (createBaseHookInput(permissionMode)).
        let p = ElicitationPayload {
            hook_event_name: HookEventNameElicitation,
            session_id: "s".into(),
            transcript_path: "/t".into(),
            cwd: "/w".into(),
            permission_mode: Some("default".into()),
            agent_id: None,
            agent_type: None,
            mcp_server_name: "linear".into(),
            message: "Pick".into(),
            mode: Some(crate::events::ElicitationMode::Url),
            url: Some("https://example.test".into()),
            elicitation_id: Some("e-1".into()),
            requested_schema: Some(json!({"type": "object"})),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"Elicitation","session_id":"s","transcript_path":"/t","cwd":"/w","permission_mode":"default","mcp_server_name":"linear","message":"Pick","mode":"url","url":"https://example.test","elicitation_id":"e-1","requested_schema":{"type":"object"}}"#
        );
    }

    #[test]
    fn worktree_create_payload_serializes_byte_lock() {
        let p = WorktreeCreatePayload {
            hook_event_name: HookEventNameWorktreeCreate,
            session_id: "sess-1".into(),
            transcript_path: "/tmp/t.jsonl".into(),
            cwd: "/work".into(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
            name: "feature-x".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"hook_event_name":"WorktreeCreate","session_id":"sess-1","transcript_path":"/tmp/t.jsonl","cwd":"/work","name":"feature-x"}"#
        );
    }

    #[test]
    fn deferred_batch_event_name_markers_round_trip() {
        for (got, want) in [
            (serde_json::to_string(&HookEventNameConfigChange).unwrap(), r#""ConfigChange""#),
            (serde_json::to_string(&HookEventNameInstructionsLoaded).unwrap(), r#""InstructionsLoaded""#),
            (serde_json::to_string(&HookEventNameElicitation).unwrap(), r#""Elicitation""#),
            (serde_json::to_string(&HookEventNameWorktreeCreate).unwrap(), r#""WorktreeCreate""#),
        ] {
            assert_eq!(got, want);
        }
        let _: HookEventNameConfigChange = serde_json::from_str(r#""ConfigChange""#).unwrap();
        let _: HookEventNameInstructionsLoaded =
            serde_json::from_str(r#""InstructionsLoaded""#).unwrap();
        let _: HookEventNameElicitation = serde_json::from_str(r#""Elicitation""#).unwrap();
        let _: HookEventNameWorktreeCreate = serde_json::from_str(r#""WorktreeCreate""#).unwrap();
        assert!(serde_json::from_str::<HookEventNameConfigChange>(r#""Elicitation""#).is_err());
    }
}
