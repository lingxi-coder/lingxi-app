//! `InteractivePromptingGate` — stdin/stderr prompt loop.
//!
//! Drives the byte-locked claude-code prompt UX over injectable AsyncRead /
//! AsyncWrite endpoints. Tests pipe via `tokio::io::duplex`.
//!
//! M5-05 task progression:
//! - Task 5: `format_prompt` byte-locks the prompt literals.
//! - Task 6: `parse_user_input` + `resolve_outcome`.
//! - Task 7 (RED) → Task 8 (GREEN): `PromptingGate::prompt_user` impl.
//! - Tasks 9-10: empty-input and retry-then-error tests.
//! - Task 11: `PermissionGate` upcast impl + orchestrator wiring.
//! - Task 12: telemetry emission for `permission_prompted` /
//!   `permission_answered`.
#![forbid(unsafe_code)]

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

use crate::gate::{PermissionRequest, PromptDefault};

/// Format the byte-locked prompt for a [`PermissionRequest`].
///
/// - Generic tools: `"Claude needs your permission to use {tool_name}\n[Y/n] "`
///   or `[y/N]` depending on the tool's default.
/// - `Agent` and its legacy alias `Task`:
///   `"Agent tool requires permission to spawn sub-agents.\n[Y/n] "`.
///
/// The suffix bracket pair is always followed by a single space.
pub(crate) fn format_prompt(request: &PermissionRequest) -> String {
    let suffix = match request.default_decision {
        PromptDefault::AllowByDefault => "[Y/n] ",
        PromptDefault::DenyByDefault => "[y/N] ",
    };
    if request.tool_name == "Agent" || request.tool_name == "Task" {
        format!("Agent tool requires permission to spawn sub-agents.\n{suffix}")
    } else {
        format!(
            "Claude needs your permission to use {}\n{suffix}",
            request.tool_name
        )
    }
}

/// Interactive permission gate driven by stdin/stderr.
///
/// Production wiring (M5-12) constructs with `tokio::io::stdin()` +
/// `tokio::io::stderr()`. Tests use `tokio::io::duplex` scripts.
pub struct InteractivePromptingGate {
    // Used by `PromptingGate::prompt_user` (lands in Task 8).
    #[allow(dead_code)]
    pub(crate) stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
    #[allow(dead_code)]
    pub(crate) stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
}

impl InteractivePromptingGate {
    /// Construct a new interactive gate over the given stdin / stderr endpoints.
    #[must_use]
    pub fn new(
        stdin: Arc<Mutex<dyn AsyncRead + Send + Unpin>>,
        stderr: Arc<Mutex<dyn AsyncWrite + Send + Unpin>>,
    ) -> Self {
        Self { stdin, stderr }
    }
}

// PromptingGate impl lands in Task 8. PermissionGate upcast lands in Task 11.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_prompt_generic_allow_default_byte_locked() {
        let req = PermissionRequest {
            tool_name: "Read".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::AllowByDefault,
        };
        let s = format_prompt(&req);
        assert_eq!(
            s.as_bytes(),
            b"Claude needs your permission to use Read\n[Y/n] "
        );
    }

    #[test]
    fn format_prompt_generic_deny_default_byte_locked() {
        let req = PermissionRequest {
            tool_name: "Bash".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::DenyByDefault,
        };
        let s = format_prompt(&req);
        assert_eq!(
            s.as_bytes(),
            b"Claude needs your permission to use Bash\n[y/N] "
        );
    }

    #[test]
    fn format_prompt_agent_special_byte_locked() {
        let req = PermissionRequest {
            tool_name: "Agent".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::AllowByDefault,
        };
        let s = format_prompt(&req);
        assert_eq!(
            s.as_bytes(),
            b"Agent tool requires permission to spawn sub-agents.\n[Y/n] "
        );
    }

    #[test]
    fn format_prompt_task_alias_also_uses_agent_message() {
        // `Task` is the legacy alias for the Agent tool.
        let req = PermissionRequest {
            tool_name: "Task".to_string(),
            tool_input: json!({}),
            default_decision: PromptDefault::AllowByDefault,
        };
        let s = format_prompt(&req);
        assert_eq!(
            s.as_bytes(),
            b"Agent tool requires permission to spawn sub-agents.\n[Y/n] "
        );
    }
}
