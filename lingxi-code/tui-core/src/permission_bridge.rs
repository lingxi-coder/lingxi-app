//! Backend-neutral permission-request bridge types.
//!
//! Moved from `tui` during the iocraft → ratatui migration so `tui-rata` can
//! consume permission requests. `WorkerPermissionInfo` (pure data) came out of
//! the iocraft `components::permissions::worker` module; the `TuiPermissionGate`
//! that produces these exchanges stays in `tui`.

use permission::gate::{AutoModePrompt, PermissionRequest, PermissionResponse};
use tokio::sync::oneshot;

/// Worker identity carried on a pending permission (TUI-side; not on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerPermissionInfo {
    /// Worker display name (rendered `@name`).
    pub name: String,
    /// Worker color name (→ `agent_color_from_name`).
    pub color: String,
    /// Optional team name (for the "sent to team … leader" line).
    pub team: Option<String>,
}

/// One in-flight permission round-trip between the orchestrator and TUI.
///
/// Constructed by `TuiPermissionGate::check` and sent over the mpsc to the TUI
/// app. The TUI fills `resp_tx` when the user resolves the dialog. Dropping
/// `resp_tx` without sending counts as cancellation (the gate maps it to
/// `Deny { reason: "TUI permission response dropped" }`).
#[derive(Debug)]
pub struct PermissionExchange {
    /// What we're asking permission for.
    pub request: PermissionRequest,
    /// One-shot reply channel — TUI sends back when the user resolves.
    pub resp_tx: oneshot::Sender<PermissionResponse>,
    /// Worker identity when the call originates from a subagent/teammate
    /// (claude-code 2.1.186 worker permission attribution). `None` for a
    /// main-thread tool call. Populated so the dialog renders the `● @name`
    /// badge.
    pub worker: Option<WorkerPermissionInfo>,
    /// When true, the prompt must not offer or persist an "allow always"
    /// rule. This is used for tools that require a human decision on every
    /// invocation (MCP `requiresUserInteraction: true`).
    pub suppress_always_allow_rule: bool,
    /// Exact persistence choice derived from real `permission_suggestions`
    /// metadata. `None` means the transport must omit the "don't ask again"
    /// row. Keeping the update beside its label prevents the response path from
    /// recomputing a different rule.
    pub permission_persistence:
        Option<permission::allow_suggestion::PermissionPersistenceSuggestion>,
    /// Engine-computed optional Auto action. `None` means the transport must
    /// not render an Auto row; clients must not infer this from the request.
    pub auto_mode_prompt: Option<AutoModePrompt>,
}
