//! Process-global session-level flags resolved once at session init and read by
//! leaf consumers that have no per-call context handle.
//!
//! claude-code exposes session-mode predicates as module-level globals (e.g.
//! `getIsNonInteractiveSession()`); prompt builders call them inline. The Rust
//! port models per-tool-call interactivity via `ToolUseContext.is_non_interactive_session`,
//! but the wire `tools` array (and thus each tool's `prompt()`) is assembled
//! WITHOUT a `ToolUseContext`. This global is the faithful analog for those
//! prompt-build-time reads: set once by the composition point that knows the
//! session mode, read by builders such as the `AgentTool` fork gate.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// `getIsNonInteractiveSession()` analog. Defaults `false` (interactive); set by
/// [`ConversationOrchestrator::new`](../../orchestrator) from the session's
/// `interactive_permissions` (`-p`/print/headless ⇒ non-interactive).
static NON_INTERACTIVE_SESSION: AtomicBool = AtomicBool::new(false);

/// `settings.showThinkingSummaries` analog. Defaults `false`, matching Claude
/// Code's external setting default. This is consumed by request beta assembly,
/// whose provider adapter has no settings handle.
static SHOW_THINKING_SUMMARIES: AtomicBool = AtomicBool::new(false);

/// Effective `agentPushNotifEnabled` setting. The feature flag remains a
/// separate gate at each consumer; this cell carries only the merged user /
/// project / local / managed setting value.
static AGENT_PUSH_NOTIF_ENABLED: AtomicBool = AtomicBool::new(false);

/// `$U()` (the "optimistic" tool-search gate) analog. SESSION-scoped in Claude
/// Code: `$U()` reads the tool-search MODE (`ENABLE_TOOL_SEARCH` env /
/// experimental-betas kill switch) and the active PROVIDER — never the current
/// request's tools array. Every request assembly (main loop AND side queries)
/// branches its `tool_reference` normalization on `$U()`
/// (`if(!$U())W=j6s(W);else W=xPy(W,a)`), so a side query built with an EMPTY
/// toolset (compaction summarizer, recap) in a tool-search-enabled session still
/// takes the ENABLED branch and emits "[Tool references removed - tools no
/// longer available]" — not the disabled branch's "[…tool search not enabled]".
///
/// The request builder (`llm-client`) is provider-agnostic and has no session
/// handle, so it cannot compute this itself; the orchestrator — which knows the
/// mode and resolved provider — publishes the decision here and the builder
/// reads it. Defaults `false` (tool search off / no provider support). Set by
/// the orchestrator at session init and refreshed as the model/profile resolve.
static TOOL_SEARCH_ENABLED: AtomicBool = AtomicBool::new(false);

/// Session-scoped dynamic Workflow availability, resolved by the composition
/// root after managed policy and environment gates are known.
static DYNAMIC_WORKFLOWS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Effective `workflowSizeGuideline` for the current process/session.
/// `2` is `medium`, the Claude Code 2.1.219+ default.
static WORKFLOW_SIZE_GUIDELINE: AtomicU8 = AtomicU8::new(2);

/// Whether the effective workflow-size value is owned by managed policy.
static WORKFLOW_SIZE_GUIDELINE_MANAGED: AtomicBool = AtomicBool::new(false);

/// Record whether the current process is a non-interactive (`-p`/print/headless)
/// session. Idempotent; safe to call repeatedly (the value is fixed per process).
pub fn set_non_interactive_session(non_interactive: bool) {
    NON_INTERACTIVE_SESSION.store(non_interactive, Ordering::Relaxed);
}

/// Whether the current process is a non-interactive session (default `false`).
#[must_use]
pub fn is_non_interactive_session() -> bool {
    NON_INTERACTIVE_SESSION.load(Ordering::Relaxed)
}

tokio::task_local! {
    /// Per-turn override for embedded runtimes that coexist in one process.
    /// Desktop/CLI callers without a scope retain the process-global behavior.
    static NON_INTERACTIVE_SESSION_OVERRIDE: bool;
}

/// Run `future` with a task-local session mode, so prompt builders and provider
/// beta assembly see the owning orchestrator instead of a concurrently
/// constructed runtime's process-global compatibility value. Independently
/// spawned Tokio tasks do not inherit task locals and must establish their own
/// scope explicitly.
pub async fn scope_non_interactive_session<F: std::future::Future>(
    non_interactive: bool,
    future: F,
) -> F::Output {
    NON_INTERACTIVE_SESSION_OVERRIDE
        .scope(non_interactive, future)
        .await
}

/// Read the task-local session mode when present, falling back to the legacy
/// process-global flag for callers outside an orchestrator turn.
#[must_use]
pub fn effective_non_interactive_session() -> bool {
    NON_INTERACTIVE_SESSION_OVERRIDE
        .try_with(|value| *value)
        .unwrap_or_else(|_| is_non_interactive_session())
}

#[cfg(test)]
mod interactivity_tests {
    #[tokio::test]
    async fn task_local_interactivity_isolated_from_process_global_flag() {
        let prior = super::is_non_interactive_session();
        super::set_non_interactive_session(false);

        let headless = super::scope_non_interactive_session(true, async {
            super::effective_non_interactive_session()
        });
        let interactive = super::scope_non_interactive_session(false, async {
            super::effective_non_interactive_session()
        });
        let (headless, interactive) = tokio::join!(headless, interactive);

        assert!(headless);
        assert!(!interactive);
        assert!(!super::effective_non_interactive_session());
        super::set_non_interactive_session(prior);
    }
}

/// Publish the merged `showThinkingSummaries` setting for request assembly.
pub fn set_show_thinking_summaries(show: bool) {
    SHOW_THINKING_SUMMARIES.store(show, Ordering::Relaxed);
}

/// Whether API-side thinking summaries are explicitly enabled.
#[must_use]
pub fn show_thinking_summaries() -> bool {
    SHOW_THINKING_SUMMARIES.load(Ordering::Relaxed)
}

/// Publish the merged `agentPushNotifEnabled` setting.
pub fn set_agent_push_notif_enabled(enabled: bool) {
    AGENT_PUSH_NOTIF_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether proactive agent push notifications are opted in by settings.
#[must_use]
pub fn agent_push_notif_enabled() -> bool {
    AGENT_PUSH_NOTIF_ENABLED.load(Ordering::Relaxed)
}

/// Publish the session-scoped tool-search gate (Claude Code `$U()`) for the
/// request builder's `tool_reference` normalization branch. Set by the
/// orchestrator from the session mode + resolved provider support; the value is
/// stable within a session (it changes only if the model/provider is switched).
pub fn set_tool_search_enabled(enabled: bool) {
    TOOL_SEARCH_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether tool search is enabled for this session (Claude Code `$U()`), default
/// `false`. Read by the request builder to select the `tool_reference`
/// normalization branch for EVERY request — including side queries whose
/// per-request toolset is empty — instead of inferring it from whether the
/// request's tools array happens to carry a `ToolSearch` declaration.
#[must_use]
pub fn tool_search_enabled() -> bool {
    TOOL_SEARCH_ENABLED.load(Ordering::Relaxed)
}

/// Publish whether dynamic workflows are available for this session.
pub fn set_dynamic_workflows_enabled(enabled: bool) {
    DYNAMIC_WORKFLOWS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether `/effort ultracode` may activate the xhigh Workflow mode.
#[must_use]
pub fn dynamic_workflows_enabled() -> bool {
    DYNAMIC_WORKFLOWS_ENABLED.load(Ordering::Relaxed)
}

/// Publish the effective workflow-size setting and whether policy owns it.
///
/// Returns `false` for an unknown wire value and leaves the prior snapshot
/// untouched.
pub fn set_workflow_size_guideline(value: &str, managed: bool) -> bool {
    let encoded = match value {
        "unrestricted" => 0,
        "small" => 1,
        "medium" => 2,
        "large" => 3,
        _ => return false,
    };
    WORKFLOW_SIZE_GUIDELINE.store(encoded, Ordering::Relaxed);
    WORKFLOW_SIZE_GUIDELINE_MANAGED.store(managed, Ordering::Relaxed);
    true
}

/// Effective workflow-size wire value. Defaults to `medium`.
#[must_use]
pub fn workflow_size_guideline() -> &'static str {
    match WORKFLOW_SIZE_GUIDELINE.load(Ordering::Relaxed) {
        0 => "unrestricted",
        1 => "small",
        3 => "large",
        _ => "medium",
    }
}

/// Whether managed policy owns the effective workflow-size setting.
#[must_use]
pub fn workflow_size_guideline_is_managed() -> bool {
    WORKFLOW_SIZE_GUIDELINE_MANAGED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_search_flag_round_trips() {
        // Serialize with the interactivity test which mutates a sibling global —
        // these are process-wide, so keep tool-search mutation self-contained and
        // restore the prior value so parallel tests observe no side effect.
        let prior = tool_search_enabled();
        set_tool_search_enabled(true);
        assert!(tool_search_enabled(), "true must be observable");
        set_tool_search_enabled(false);
        assert!(!tool_search_enabled(), "false must be observable");
        set_tool_search_enabled(prior);
    }

    #[test]
    fn agent_push_notification_setting_round_trips() {
        let prior = agent_push_notif_enabled();
        set_agent_push_notif_enabled(true);
        assert!(agent_push_notif_enabled());
        set_agent_push_notif_enabled(false);
        assert!(!agent_push_notif_enabled());
        set_agent_push_notif_enabled(prior);
    }
}
