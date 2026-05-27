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

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::gate::{
    PermissionDecision, PermissionGate, PermissionRequest, PromptDecision, PromptDefault,
    PromptError, PromptingGate,
};

/// Maximum number of consecutive invalid inputs the gate tolerates before
/// erroring out. Locked at 3 per claude-code's `MAX_RETRIES` constant
/// (see plan §"Reverse-engineered byte-locks").
const MAX_RETRIES: u32 = 3;

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

    /// Convenience constructor over the process's real stdin / stderr.
    ///
    /// Used by M5-12 CLI binary when
    /// [`OrchestratorConfig::interactive_permissions`] is `true`.
    /// Wraps `tokio::io::stdin()` and `tokio::io::stderr()` in the
    /// `Arc<Mutex<…>>` newtype the trait surface expects.
    ///
    /// [`OrchestratorConfig::interactive_permissions`]: ../../lingxi_orchestrator/struct.OrchestratorConfig.html#structfield.interactive_permissions
    #[must_use]
    pub fn with_stdio() -> Self {
        Self::new(
            Arc::new(Mutex::new(tokio::io::stdin())),
            Arc::new(Mutex::new(tokio::io::stderr())),
        )
    }
}

/// One step of input parsing. `Valid*` means the user produced a definitive
/// answer; `Invalid` means we should re-prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseOutcome {
    /// User typed a yes-variant (`y/Y/yes/YES/…`).
    ValidYes,
    /// User typed a no-variant (`n/N/no/NO/…`).
    ValidNo,
    /// User just pressed Enter — caller resolves against the default.
    Empty,
    /// Anything else.
    Invalid,
}

/// Parse a single line of user input.
///
/// Trims trailing `\r?\n` and any surrounding whitespace. Empty (after
/// trim) ⇒ [`ParseOutcome::Empty`]. Otherwise compares the lowercased
/// token against `y` / `yes` / `n` / `no`.
pub(crate) fn parse_user_input(line: &str) -> ParseOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParseOutcome::Empty;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "y" | "yes" => ParseOutcome::ValidYes,
        "n" | "no" => ParseOutcome::ValidNo,
        _ => ParseOutcome::Invalid,
    }
}

/// Resolve a [`ParseOutcome`] into a definitive Allow/Deny against a
/// default. `Invalid` is the only outcome that returns `None` (caller
/// re-prompts).
pub(crate) fn resolve_outcome(outcome: ParseOutcome, default: PromptDefault) -> Option<bool> {
    match outcome {
        ParseOutcome::ValidYes => Some(true),
        ParseOutcome::ValidNo => Some(false),
        ParseOutcome::Empty => Some(matches!(default, PromptDefault::AllowByDefault)),
        ParseOutcome::Invalid => None,
    }
}

#[async_trait]
impl PermissionGate for InteractivePromptingGate {
    async fn check(
        &self,
        name: &str,
        input: &serde_json::Value,
    ) -> PermissionDecision {
        let request = PermissionRequest {
            tool_name: name.to_string(),
            tool_input: input.clone(),
            default_decision: crate::defaults_per_tool::tool_default(name),
        };
        match self.prompt_user(&request).await {
            Ok(d) if d.allow => PermissionDecision::Allow,
            Ok(d) => PermissionDecision::Deny { reason: d.reason },
            Err(PromptError::InvalidInput { attempts }) => PermissionDecision::Deny {
                reason: format!("invalid permission input after {attempts} attempts"),
            },
            Err(PromptError::Cancelled { reason }) => {
                PermissionDecision::Deny { reason: format!("prompt cancelled: {reason}") }
            }
            Err(PromptError::Io(reason)) => {
                PermissionDecision::Deny { reason: format!("prompt io: {reason}") }
            }
        }
    }
}

#[async_trait]
impl PromptingGate for InteractivePromptingGate {
    async fn prompt_user(
        &self,
        request: &PermissionRequest,
    ) -> Result<PromptDecision, PromptError> {
        let prompt = format_prompt(request);
        let mut attempts: u32 = 0;
        // Acquire stdin lock + wrap in BufReader ONCE for the full prompt
        // round-trip. Creating a fresh BufReader per loop iteration would
        // silently discard any bytes BufReader had pre-fetched past the
        // first newline — on retry inputs (`foo\nbar\nbaz\n`) only the
        // first `foo\n` would be consumed and subsequent iterations would
        // see EOF.
        let mut in_guard = self.stdin.lock().await;
        let mut reader = BufReader::new(&mut *in_guard);
        loop {
            // 1. Write the prompt to stderr.
            {
                let mut err = self.stderr.lock().await;
                err.write_all(prompt.as_bytes())
                    .await
                    .map_err(|e| PromptError::Io(e.to_string()))?;
                err.flush()
                    .await
                    .map_err(|e| PromptError::Io(e.to_string()))?;
            }
            // 2. Read one line from stdin.
            let line = {
                let mut buf = String::new();
                let n = reader
                    .read_line(&mut buf)
                    .await
                    .map_err(|e| PromptError::Io(e.to_string()))?;
                if n == 0 {
                    return Err(PromptError::Cancelled {
                        reason: "stdin closed".to_string(),
                    });
                }
                buf
            };
            // 3. Classify and decide.
            let outcome = parse_user_input(&line);
            match resolve_outcome(outcome, request.default_decision) {
                Some(allow) => {
                    let reason = match outcome {
                        ParseOutcome::ValidYes => "user typed 'y'".to_string(),
                        ParseOutcome::ValidNo => "user typed 'n'".to_string(),
                        ParseOutcome::Empty => format!(
                            "user pressed Enter (default = {})",
                            if allow { "allow" } else { "deny" }
                        ),
                        ParseOutcome::Invalid => {
                            unreachable!("resolve_outcome returned Some for Invalid")
                        }
                    };
                    return Ok(PromptDecision {
                        allow,
                        reason,
                        persist: false,
                    });
                }
                None => {
                    attempts += 1;
                    if attempts >= MAX_RETRIES {
                        return Err(PromptError::InvalidInput { attempts });
                    }
                    continue;
                }
            }
        }
    }
}

// PermissionGate upcast lands in Task 11.

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

    // ---- parse_user_input + resolve_outcome (Task 6) ----

    #[test]
    fn parse_y_lowercase_is_valid_yes() {
        assert_eq!(parse_user_input("y\n"), ParseOutcome::ValidYes);
    }

    #[test]
    fn parse_y_uppercase_is_valid_yes() {
        assert_eq!(parse_user_input("Y\n"), ParseOutcome::ValidYes);
    }

    #[test]
    fn parse_yes_mixed_case_is_valid_yes() {
        assert_eq!(parse_user_input("YES\n"), ParseOutcome::ValidYes);
        assert_eq!(parse_user_input("Yes\n"), ParseOutcome::ValidYes);
        assert_eq!(parse_user_input("yEs\n"), ParseOutcome::ValidYes);
    }

    #[test]
    fn parse_n_lowercase_is_valid_no() {
        assert_eq!(parse_user_input("n\n"), ParseOutcome::ValidNo);
    }

    #[test]
    fn parse_no_uppercase_is_valid_no() {
        assert_eq!(parse_user_input("NO\n"), ParseOutcome::ValidNo);
    }

    #[test]
    fn parse_empty_is_empty() {
        assert_eq!(parse_user_input("\n"), ParseOutcome::Empty);
        assert_eq!(parse_user_input(""), ParseOutcome::Empty);
        assert_eq!(parse_user_input("   \n"), ParseOutcome::Empty);
    }

    #[test]
    fn parse_garbage_is_invalid() {
        assert_eq!(parse_user_input("maybe\n"), ParseOutcome::Invalid);
        assert_eq!(parse_user_input("42\n"), ParseOutcome::Invalid);
        assert_eq!(parse_user_input("yy\n"), ParseOutcome::Invalid);
    }

    #[test]
    fn parse_handles_carriage_return() {
        assert_eq!(parse_user_input("y\r\n"), ParseOutcome::ValidYes);
        assert_eq!(parse_user_input("\r\n"), ParseOutcome::Empty);
    }

    #[test]
    fn resolve_empty_with_allow_default_is_true() {
        assert_eq!(
            resolve_outcome(ParseOutcome::Empty, PromptDefault::AllowByDefault),
            Some(true)
        );
    }

    #[test]
    fn resolve_empty_with_deny_default_is_false() {
        assert_eq!(
            resolve_outcome(ParseOutcome::Empty, PromptDefault::DenyByDefault),
            Some(false)
        );
    }

    #[test]
    fn resolve_yes_overrides_deny_default() {
        assert_eq!(
            resolve_outcome(ParseOutcome::ValidYes, PromptDefault::DenyByDefault),
            Some(true)
        );
    }

    #[test]
    fn resolve_no_overrides_allow_default() {
        assert_eq!(
            resolve_outcome(ParseOutcome::ValidNo, PromptDefault::AllowByDefault),
            Some(false)
        );
    }

    #[test]
    fn resolve_invalid_is_none() {
        assert_eq!(
            resolve_outcome(ParseOutcome::Invalid, PromptDefault::AllowByDefault),
            None
        );
    }
}
