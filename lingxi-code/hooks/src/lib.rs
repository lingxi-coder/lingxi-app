//! Hook system — 28-event observability + intervention plane.
//!
//! Hooks fire around tool dispatch, session lifecycle, permission requests,
//! and many other engine touchpoints. Each registered hook can observe an
//! event and optionally return a [`HookResponse`] that blocks, approves, or
//! mutates the in-flight action.
//!
//! M1.4 ships the data model and an executor scaffold: the 28-variant event
//! enum, hook definition / source taxonomy, response aggregation, the
//! [`HookExecutorImpl`], and the [`SsrfGuard`]. The full Command / HTTP /
//! Agent executor bodies and the in-flight tracker land in Plan 09.
//!
//! See spec §9 (Hook System) and D17 (Runtime boundary — no direct tokio).

#![forbid(unsafe_code)]

mod agent_executor;
pub mod async_registry;
pub mod builtin;
pub mod cwd_changed_firer;
pub mod definition;
pub mod events;
pub mod executor;
pub mod file_changed_firer;
pub mod hook_payload;
mod http_executor;
pub mod loader;
pub mod matcher;
pub mod prompt_executor;
pub mod registry;
pub mod response;
pub mod ssrf_guard;
pub mod task_completed_firer;
pub mod task_created_firer;
pub mod teammate_idle_firer;
pub mod terminal_seq;
pub mod user_config;
pub mod watcher_rebinder;

pub use async_registry::AsyncHookRegistry;
pub use cwd_changed_firer::{CwdChangedFire, CwdChangedFirer, OptionalCwdChangedFirer};
pub use definition::{HookCondition, HookDefinition, HookExecutor, HookSource};
pub use events::{HookEvent, HookEventType, HookProgressEvent};
pub use executor::{
    BuiltinHookHandler, HookExecutorImpl, HOOK_AGENT_TIMEOUT_MS, HOOK_COMMAND_TIMEOUT_MS,
    HOOK_HTTP_TIMEOUT_MS,
};
pub use file_changed_firer::{FileChangedFire, FileChangedFirer, OptionalFileChangedFirer};
pub use hook_payload::{
    parse_response, HookBackgroundTask, HookEventEnvelope, HookEventNamePost, HookEventNamePre,
    HookResponseParseError, HookSessionCron, PostToolUsePayload, PreToolUsePayload,
};
pub use loader::{
    parse_hooks_from_settings_json, parse_hooks_from_settings_json_gated, HookPolicyGate,
};
pub use matcher::{get_legacy_tool_names, matches_pattern, normalize_legacy_tool_name};
pub use prompt_executor::{
    HookPromptRunner, PromptHookError, PromptHookRequest, HOOK_PROMPT_TIMEOUT_MS,
};
pub use registry::{HookContext, HookRegistry};
pub use response::{
    AggregateHookResult, ElicitationHookResponse, HookDecision, HookOutcome, HookResponse,
    HookResult,
};
pub use ssrf_guard::{DnsResolver, IpRange, SsrfError, SsrfGuard};
pub use task_completed_firer::{OptionalTaskCompletedFirer, TaskCompletedFire, TaskCompletedFirer};
pub use task_created_firer::{OptionalTaskCreatedFirer, TaskCreatedFire, TaskCreatedFirer};
pub use teammate_idle_firer::{OptionalTeammateIdleFirer, TeammateIdleFire, TeammateIdleFirer};
pub use watcher_rebinder::{OptionalWatcherRebinder, WatcherRebinder};
