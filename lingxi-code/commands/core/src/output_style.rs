//! `/output-style` — deprecated; points the user at `/config`.
//!
//! Ported 1:1 from the claude-code TS local-jsx command
//! `src/commands/output-style/output-style.tsx`, whose `call()` performs a
//! single synchronous `onDone(<deprecation string>, { display: 'system' })`
//! and returns `undefined` — no UI, no state, no side effects. This handle-free
//! port emits the exact deprecation string as `Done`.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;

/// Verbatim deprecation notice from `output-style.tsx`.
const DEPRECATION_MESSAGE: &str = "/output-style has been deprecated. Use /config to change your output style, or set it in your settings file. Changes take effect on the next session.";

/// Command description, verbatim from the TS metadata
/// (`output-style/index.ts`). Used in place of `core_description("output-style")`
/// — which would yield the misleading `"(unimplemented in v0.6.0)"` fallback —
/// without touching the locked names table.
const DESCRIPTION: &str = "Deprecated: use /config to change output style";

/// `/output-style` handler — returns the deprecation notice as `Done`.
///
/// No orchestrator dependency: the TS non-interactive path is a pure
/// static-text emission with zero engine/state dependency.
#[derive(Debug, Default)]
pub struct OutputStyleHandler;

impl OutputStyleHandler {
    /// Construct a new `OutputStyleHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BuiltinCommandHandler for OutputStyleHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done {
            display: Some(DEPRECATION_MESSAGE.to_string()),
        }
    }

    fn name(&self) -> &str {
        "output-style"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "output-style".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn returns_deprecation_notice() {
        let h = OutputStyleHandler::new();
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(s, DEPRECATION_MESSAGE);
                assert!(
                    s.contains("has been deprecated"),
                    "unexpected display text: {s}"
                );
            }
            other => panic!("expected Done with display, got {other:?}"),
        }
    }

    #[test]
    fn name_and_description() {
        let h = OutputStyleHandler::new();
        assert_eq!(h.name(), "output-style");
        assert_eq!(h.description(), "Deprecated: use /config to change output style");
    }
}
