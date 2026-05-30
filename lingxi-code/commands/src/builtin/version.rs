//! `/version` — print build info (semver + short git SHA).
//!
//! Locked display template (`LingXi` UX, M5-11 T0 step 2 L11):
//!   `"lingxi-cli {version} ({git_sha:short})"`
//! The `{git_sha:short}` is baked at build time by `lingxi-cli`'s `build.rs`
//! (M5-12). When the env var is absent (e.g. when the lingxi-commands lib is
//! built without going through that binary), we fall back to `"unknown"`.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md`
//! Task 4.

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;

/// Short git SHA baked into the binary by `lingxi-cli/build.rs`. Falls back
/// to `"unknown"` when the env var is absent (e.g. cargo install / library
/// build without the binary wrapper).
const GIT_SHA_SHORT: &str = match option_env!("LINGXI_GIT_SHA_SHORT") {
    Some(s) => s,
    None => "unknown",
};

/// `/version` handler — emits the locked semver + SHA literal.
#[derive(Debug, Default, Clone)]
pub struct VersionHandler;

impl VersionHandler {
    /// Construct a fresh `VersionHandler`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl BuiltinCommandHandler for VersionHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit_command_started(cmd_evt::VERSION_STARTED);
        let s = format!(
            "lingxi-cli {} ({})",
            env!("CARGO_PKG_VERSION"),
            GIT_SHA_SHORT
        );
        lingxi_telemetry::emit_command_completed(cmd_evt::VERSION_COMPLETED, "");
        CommandResult::Done { display: Some(s) }
    }
    fn name(&self) -> &str {
        "version"
    }
    fn description(&self) -> &str {
        core_description("version")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "version".to_string(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn version_format() {
        let h = VersionHandler::new();
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert!(s.starts_with("lingxi-cli "));
            assert!(s.ends_with(')'));
            assert!(s.contains('('));
            assert!(s.contains(env!("CARGO_PKG_VERSION")));
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = VersionHandler::new();
        assert_eq!(h.name(), "version");
        assert_eq!(h.description(), "Print version information");
    }
}
