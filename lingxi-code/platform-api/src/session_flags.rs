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
use std::sync::Arc;

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

/// Session-scoped Brief-only mode. The CLI and `/brief` command publish the
/// live value here; tool registration reads it when deciding whether
/// `SendUserMessage` is available. Keeping this in the shared session flag
/// layer avoids stale process-environment snapshots when a user toggles the
/// mode during an interactive session.
static BRIEF_MODE_ENABLED: AtomicBool = AtomicBool::new(false);

/// One-shot model-facing reminder queued by the interactive `/brief` toggle.
///
/// The command's visible status line is not enough for the next model call:
/// Claude Code also attaches a transient `<system-reminder>` explaining which
/// output channel is now authoritative. Keep the pending state separate from
/// [`BRIEF_MODE_ENABLED`] so startup `--brief` enables the tool without
/// fabricating a command-toggle reminder.
static BRIEF_MODE_REMINDER: AtomicU8 = AtomicU8::new(0);

/// Model-facing reminder emitted after `/brief` enables Brief-only mode.
pub const BRIEF_MODE_ENABLED_REMINDER: &str = "<system-reminder>\nBrief mode is now enabled. Use the SendUserMessage tool for all user-facing output — plain text outside it is hidden from the user's view.\n</system-reminder>";

/// Model-facing reminder emitted after `/brief` disables Brief-only mode.
pub const BRIEF_MODE_DISABLED_REMINDER: &str = "<system-reminder>\nBrief mode is now disabled. The SendUserMessage tool is no longer available — reply with plain text.\n</system-reminder>";

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

/// Session-owned dynamic-workflow gate shared by the Workflow tool,
/// orchestrator handle, and TUI consumers. This lets one process host
/// multiple sessions without routing workflow availability through the legacy
/// process-global compatibility flag.
#[derive(Clone, Debug)]
pub struct DynamicWorkflowsGate {
    enabled: Arc<AtomicBool>,
    managed: Arc<AtomicBool>,
}

impl DynamicWorkflowsGate {
    /// Create a session-owned gate with its effective availability and policy
    /// ownership state.
    #[must_use]
    pub fn new(enabled: bool, managed: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            managed: Arc::new(AtomicBool::new(managed)),
        }
    }

    /// Whether dynamic workflows are currently available in this session.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Whether the current value is locked by managed or environment policy.
    #[must_use]
    pub fn managed(&self) -> bool {
        self.managed.load(Ordering::Relaxed)
    }

    /// Update workflow availability for every clone of this session gate.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Update whether the session gate is policy-owned.
    pub fn set_managed(&self, managed: bool) {
        self.managed.store(managed, Ordering::Relaxed);
    }

    /// Update both gate fields for every clone.
    ///
    /// The two atomic fields are stored independently; callers that expose a
    /// live transition should update them before publishing related UI state.
    pub fn set(&self, enabled: bool, managed: bool) {
        self.set_enabled(enabled);
        self.set_managed(managed);
    }
}

impl Default for DynamicWorkflowsGate {
    fn default() -> Self {
        Self::new(true, false)
    }
}

const WORKFLOW_SIZE_GUIDELINE_DEFAULT_ENCODED: u8 = 0b1010;
const WORKFLOW_SIZE_GUIDELINE_MANAGED_BIT: u8 = 1 << 2;
const WORKFLOW_SIZE_GUIDELINE_DEFAULT_BIT: u8 = 1 << 3;

/// Effective `workflowSizeGuideline` compatibility snapshot for callers that
/// still have no session-owned handle. `0b1010` encodes `medium`, unmanaged,
/// built-in default.
static WORKFLOW_SIZE_GUIDELINE_SNAPSHOT: AtomicU8 =
    AtomicU8::new(WORKFLOW_SIZE_GUIDELINE_DEFAULT_ENCODED);

fn encode_workflow_size_guideline(value: &str, managed: bool, is_default: bool) -> Option<u8> {
    let mut encoded = match value {
        "unrestricted" => 0,
        "small" => 1,
        "medium" => 2,
        "large" => 3,
        _ => return None,
    };
    if managed {
        encoded |= WORKFLOW_SIZE_GUIDELINE_MANAGED_BIT;
    }
    if is_default {
        encoded |= WORKFLOW_SIZE_GUIDELINE_DEFAULT_BIT;
    }
    Some(encoded)
}

#[must_use]
fn decode_workflow_size_guideline(encoded: u8) -> WorkflowSizeGuidelineSnapshot {
    let value = match encoded & 0b11 {
        0 => "unrestricted",
        1 => "small",
        3 => "large",
        _ => "medium",
    };
    WorkflowSizeGuidelineSnapshot {
        value,
        managed: encoded & WORKFLOW_SIZE_GUIDELINE_MANAGED_BIT != 0,
        is_default: encoded & WORKFLOW_SIZE_GUIDELINE_DEFAULT_BIT != 0,
    }
}

/// Coherent `workflowSizeGuideline` snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkflowSizeGuidelineSnapshot {
    /// Effective wire value (`unrestricted`, `small`, `medium`, or `large`).
    pub value: &'static str,
    /// Whether managed policy owns the effective value.
    pub managed: bool,
    /// Whether the effective value is still the built-in default.
    pub is_default: bool,
}

/// Session-owned workflow-size setting shared by the Workflow tool,
/// orchestrator handle, and TUI consumers.
#[derive(Clone, Debug)]
pub struct WorkflowSizeGuidelineState {
    snapshot: Arc<AtomicU8>,
}

impl WorkflowSizeGuidelineState {
    /// Create a session-owned workflow-size state from one validated wire value.
    #[must_use]
    pub fn new(value: &str, managed: bool, is_default: bool) -> Option<Self> {
        Some(Self {
            snapshot: Arc::new(AtomicU8::new(encode_workflow_size_guideline(
                value, managed, is_default,
            )?)),
        })
    }

    /// Read the coherent workflow-size snapshot.
    #[must_use]
    pub fn snapshot(&self) -> WorkflowSizeGuidelineSnapshot {
        decode_workflow_size_guideline(self.snapshot.load(Ordering::Relaxed))
    }

    /// Effective workflow-size wire value.
    #[must_use]
    pub fn value(&self) -> &'static str {
        self.snapshot().value
    }

    /// Whether managed policy owns the effective value.
    #[must_use]
    pub fn managed(&self) -> bool {
        self.snapshot().managed
    }

    /// Whether the effective value is still the built-in default.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.snapshot().is_default
    }

    /// Update the session-owned value as an explicit non-default choice.
    pub fn set(&self, value: &str, managed: bool) -> bool {
        self.set_with_source(value, managed, false)
    }

    /// Update the session-owned value together with managed/default provenance.
    pub fn set_with_source(&self, value: &str, managed: bool, is_default: bool) -> bool {
        let Some(encoded) = encode_workflow_size_guideline(value, managed, is_default) else {
            return false;
        };
        self.snapshot.store(encoded, Ordering::Relaxed);
        true
    }
}

impl Default for WorkflowSizeGuidelineState {
    fn default() -> Self {
        Self::new("medium", false, true).expect("default workflow guideline must be valid")
    }
}

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

/// Publish the current session's Brief-only mode.
pub fn set_brief_mode_enabled(enabled: bool) {
    BRIEF_MODE_ENABLED.store(enabled, Ordering::Relaxed);
    // Startup publication (`--brief`) and test setup establish state directly;
    // only the interactive toggle should queue a model-facing reminder.
    BRIEF_MODE_REMINDER.store(0, Ordering::Relaxed);
}

/// Whether Brief-only mode is currently enabled for this session.
#[must_use]
pub fn brief_mode_enabled() -> bool {
    BRIEF_MODE_ENABLED.load(Ordering::Relaxed)
}

/// Flip the current session's Brief-only mode and return the new value.
pub fn toggle_brief_mode_enabled() -> bool {
    let enabled = !BRIEF_MODE_ENABLED.fetch_xor(true, Ordering::Relaxed);
    BRIEF_MODE_REMINDER.store(if enabled { 1 } else { 2 }, Ordering::Relaxed);
    enabled
}

/// Consume the pending model-facing reminder from the interactive `/brief`
/// toggle, if any. The value is transient and therefore emitted at most once
/// even when a request is rebuilt for retries.
#[must_use]
pub fn take_brief_mode_reminder() -> Option<&'static str> {
    match BRIEF_MODE_REMINDER.swap(0, Ordering::Relaxed) {
        1 => Some(BRIEF_MODE_ENABLED_REMINDER),
        2 => Some(BRIEF_MODE_DISABLED_REMINDER),
        _ => None,
    }
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
    set_workflow_size_guideline_with_source(value, managed, false)
}

/// Publish the effective workflow-size value together with its provenance.
/// Composition roots use `is_default=true` only when the winning settings
/// layer is the built-in defaults layer; live `/config` changes are explicit.
pub fn set_workflow_size_guideline_with_source(
    value: &str,
    managed: bool,
    is_default: bool,
) -> bool {
    let Some(encoded) = encode_workflow_size_guideline(value, managed, is_default) else {
        return false;
    };
    WORKFLOW_SIZE_GUIDELINE_SNAPSHOT.store(encoded, Ordering::Relaxed);
    true
}

/// Effective workflow-size wire value. Defaults to `medium`.
#[must_use]
pub fn workflow_size_guideline() -> &'static str {
    workflow_size_guideline_snapshot().value
}

/// Whether managed policy owns the effective workflow-size setting.
#[must_use]
pub fn workflow_size_guideline_is_managed() -> bool {
    workflow_size_guideline_snapshot().managed
}

/// Whether `workflowSizeGuideline` is the built-in default rather than a user,
/// project, CLI, or managed setting.
#[must_use]
pub fn workflow_size_guideline_is_default() -> bool {
    workflow_size_guideline_snapshot().is_default
}

/// Coherent compatibility snapshot for callers that still rely on the legacy
/// process-global workflow-size publication.
#[must_use]
pub fn workflow_size_guideline_snapshot() -> WorkflowSizeGuidelineSnapshot {
    decode_workflow_size_guideline(WORKFLOW_SIZE_GUIDELINE_SNAPSHOT.load(Ordering::Relaxed))
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

    #[test]
    fn brief_mode_round_trips() {
        let prior = brief_mode_enabled();
        set_brief_mode_enabled(true);
        assert!(brief_mode_enabled());
        set_brief_mode_enabled(false);
        assert!(!brief_mode_enabled());
        set_brief_mode_enabled(prior);
    }

    #[test]
    fn brief_mode_toggle_returns_new_value() {
        let prior = brief_mode_enabled();
        set_brief_mode_enabled(false);
        assert!(toggle_brief_mode_enabled());
        assert!(!toggle_brief_mode_enabled());
        set_brief_mode_enabled(prior);
    }

    #[test]
    fn brief_toggle_queues_one_shot_model_reminder() {
        let prior = brief_mode_enabled();
        set_brief_mode_enabled(false);

        assert!(toggle_brief_mode_enabled());
        assert_eq!(
            take_brief_mode_reminder(),
            Some(BRIEF_MODE_ENABLED_REMINDER)
        );
        assert_eq!(take_brief_mode_reminder(), None);

        assert!(!toggle_brief_mode_enabled());
        assert_eq!(
            take_brief_mode_reminder(),
            Some(BRIEF_MODE_DISABLED_REMINDER)
        );
        assert_eq!(take_brief_mode_reminder(), None);

        set_brief_mode_enabled(prior);
    }

    #[test]
    fn workflow_size_guideline_tracks_default_provenance() {
        let prior = workflow_size_guideline();
        let prior_managed = workflow_size_guideline_is_managed();
        let prior_default = workflow_size_guideline_is_default();

        assert!(set_workflow_size_guideline_with_source(
            "medium", false, true
        ));
        assert!(workflow_size_guideline_is_default());
        assert!(set_workflow_size_guideline("medium", false));
        assert!(!workflow_size_guideline_is_default());

        let _ = set_workflow_size_guideline_with_source(prior, prior_managed, prior_default);
    }

    #[test]
    fn workflow_size_guideline_state_instances_are_isolated() {
        let first = WorkflowSizeGuidelineState::new("small", false, false).unwrap();
        let second = WorkflowSizeGuidelineState::new("large", true, true).unwrap();

        first.set("unrestricted", false);

        assert_eq!(first.value(), "unrestricted");
        assert!(!first.managed());
        assert!(!first.is_default());
        assert_eq!(second.value(), "large");
        assert!(second.managed());
        assert!(second.is_default());
    }

    #[test]
    fn dynamic_workflows_gate_instances_are_isolated() {
        let first = DynamicWorkflowsGate::new(true, false);
        let second = DynamicWorkflowsGate::new(false, true);

        first.set(false, false);

        assert!(!first.enabled());
        assert!(!first.managed());
        assert!(!second.enabled());
        assert!(second.managed());
    }
}
