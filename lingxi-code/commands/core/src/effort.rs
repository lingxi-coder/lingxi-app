//! `/effort` — set or show the model effort level.
//!
//! Ported from the claude-code TS local-jsx command
//! `src/commands/effort/effort.tsx` (+ `src/utils/effort.ts`). The TS `call()`
//! trims the args, then branches:
//!   * `help` / `-h` / `--help` → a static Usage block;
//!   * `''` / `current` / `status` → `showCurrentEffort`;
//!   * `auto` / `unset` → clear the persisted level;
//!   * a valid level → set it;
//!   * anything else → an invalid-argument message.
//!
//! ## Frozen-batch deferrals (see the batch deferral note)
//!
//! Two pieces of the TS behaviour are NOT reachable frozen-safely and are
//! deferred:
//!   1. **Persistence.** The TS set/clear paths call
//!      `updateSettingsForSource('userSettings', { effortLevel })`. The Rust
//!      settings crate is a read-only loader (no save/write/persist fn) and
//!      `SettingsJson` has no `effortLevel` field; `OrchestratorHandle` (frozen
//!      `traits/`) exposes no settings-write nor effort accessor. So every set
//!      here is **session-only** — which maps exactly onto the real TS
//!      `persistable === undefined` branches (`" (this session only)"` /
//!      `"Not applied … nothing saved"`), preserving 1:1 fidelity on those
//!      strings.
//!   2. **The `auto (currently {level})` computed level.** TS derives it via
//!      `getDisplayedEffortLevel(model, …)` → `resolveAppliedEffort` →
//!      `getDefaultEffortForModel` + `modelSupportsMaxEffort` (subscriber /
//!      model-default / ultrathink logic) which has no Rust equivalent. The
//!      handle supplies the model string (`get_status_snapshot().model`) but
//!      not the default-effort resolver. For the pure-auto case we emit the
//!      faithful subset `"Effort level: auto"`; the `{currently X}` suffix
//!      needs the model-default resolver port.
//!
//! The env override (`CLAUDE_CODE_EFFORT_LEVEL`) is honoured in every branch.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use std::sync::Arc;
use traits::OrchestratorHandle;

/// Usage block, verbatim from `effort.tsx` L174.
const USAGE: &str = "Usage: /effort [low|medium|high|max|auto]\n\nEffort levels:\n- low: Quick, straightforward implementation\n- medium: Balanced approach with standard testing\n- high: Comprehensive implementation with extensive testing\n- max: Maximum capability with deepest reasoning (Opus 4.6 only)\n- auto: Use the default effort level for your model";

/// Environment variable that pins / clears the effort level for the session.
const EFFORT_ENV_VAR: &str = "CLAUDE_CODE_EFFORT_LEVEL";

/// The four discrete effort levels (`effort.ts` `EFFORT_LEVELS`).
///
/// Numeric efforts are ANT-only and intentionally omitted from this port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

impl EffortLevel {
    /// The canonical lowercase string for this level.
    fn as_str(self) -> &'static str {
        match self {
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
            EffortLevel::Max => "max",
        }
    }

    /// User-facing description, verbatim from `effort.ts` L224-235
    /// (`getEffortLevelDescription`).
    fn description(self) -> &'static str {
        match self {
            EffortLevel::Low => "Quick, straightforward implementation with minimal overhead",
            EffortLevel::Medium => "Balanced approach with standard implementation and testing",
            EffortLevel::High => {
                "Comprehensive implementation with extensive testing and documentation"
            }
            EffortLevel::Max => "Maximum capability with deepest reasoning (Opus 4.6 only)",
        }
    }
}

/// Parse a single lowercase token into an [`EffortLevel`] (`isEffortLevel`).
fn parse_effort_level(s: &str) -> Option<EffortLevel> {
    match s {
        "low" => Some(EffortLevel::Low),
        "medium" => Some(EffortLevel::Medium),
        "high" => Some(EffortLevel::High),
        "max" => Some(EffortLevel::Max),
        _ => None,
    }
}

/// Resolved state of the `CLAUDE_CODE_EFFORT_LEVEL` env override
/// (`getEffortEnvOverride`).
enum EnvOverride {
    /// Env unset or unparseable — TS `undefined`.
    Unset,
    /// Env set to `unset` / `auto` — TS `null` (clears effort).
    Cleared,
    /// Env pins a concrete level — TS the parsed `EffortValue`. Carries the
    /// raw (un-normalized) string for the user-facing `={raw}` messages.
    Pinned { level: EffortLevel, raw: String },
}

/// Read and classify `CLAUDE_CODE_EFFORT_LEVEL` (`getEffortEnvOverride`).
fn effort_env_override() -> EnvOverride {
    let Ok(raw) = std::env::var(EFFORT_ENV_VAR) else {
        return EnvOverride::Unset;
    };
    let normalized = raw.to_lowercase();
    if normalized == "unset" || normalized == "auto" {
        return EnvOverride::Cleared;
    }
    match parse_effort_level(&normalized) {
        Some(level) => EnvOverride::Pinned { level, raw },
        None => EnvOverride::Unset,
    }
}

/// `/effort` handler — sets or shows the effort level (session-only port).
#[derive(Clone)]
pub struct EffortHandler {
    handle: Arc<dyn OrchestratorHandle>,
}

impl EffortHandler {
    /// Construct an `EffortHandler` bound to the given orchestrator handle.
    ///
    /// The handle is used to read the current model string for the
    /// `/effort` / `/effort current` branch (`getDisplayedEffortLevel`).
    #[must_use]
    pub fn new(handle: Arc<dyn OrchestratorHandle>) -> Self {
        Self { handle }
    }

    /// `showCurrentEffort` (`effort.tsx` L62-75) — the `''`/`current`/`status`
    /// branch. With no persisted effort (no read seam) the effective value is
    /// driven entirely by the env override.
    async fn show_current(&self) -> String {
        match effort_env_override() {
            EnvOverride::Pinned { level, .. } => {
                // env pins a level → it is the effective value.
                format!(
                    "Current effort level: {} ({})",
                    level.as_str(),
                    level.description()
                )
            }
            EnvOverride::Cleared | EnvOverride::Unset => {
                // Effective value is undefined → TS renders
                // `Effort level: auto (currently {level})` where `{level}` is
                // `getDisplayedEffortLevel = resolveAppliedEffort(...) ?? 'high'`
                // (effort.ts L178). With no env/app-state effort and no
                // model-default resolver port, that resolves to the API default
                // `'high'`, so the faithful default-case output is
                // `auto (currently high)`. The model read is retained (and
                // discarded) so the future `getDefaultEffortForModel` resolver —
                // which can yield `(currently medium)` for opus-4-6/subscribers —
                // has its seam wired.
                let _ = self.handle.get_status_snapshot().await.model;
                "Effort level: auto (currently high)".to_string()
            }
        }
    }

    /// `unsetEffortLevel` (`effort.tsx` L76-106) — the `auto`/`unset` branch.
    /// Nothing is persisted (no write seam); only the env-conflict note varies.
    fn clear_effort() -> String {
        match effort_env_override() {
            EnvOverride::Pinned { raw, .. } => format!(
                "Cleared effort from settings, but {EFFORT_ENV_VAR}={raw} still controls this session"
            ),
            EnvOverride::Cleared | EnvOverride::Unset => "Effort level set to auto".to_string(),
        }
    }

    /// `setEffortValue` (`effort.tsx` L16-61) — the valid-level branch. Because
    /// there is no write seam, `persistable` is treated as `undefined` for the
    /// suffix/conflict strings; the level itself is still session-applicable.
    fn set_effort(level: EffortLevel) -> String {
        // TS flags env conflict only when env pins a *different* level than the
        // one the user asked for (`envOverride !== effortValue`). persistable
        // === undefined for every set in this port → the session-only
        // "Not applied … nothing saved" branch when the env conflicts.
        match effort_env_override() {
            EnvOverride::Pinned {
                level: env_level,
                raw,
            } if env_level != level => format!(
                "Not applied: {EFFORT_ENV_VAR}={raw} overrides effort this session, and {} is session-only (nothing saved)",
                level.as_str()
            ),
            // No conflict → `Set effort level to {x} (this session only): {desc}`.
            // The `(this session only)` suffix fires because persistable is
            // undefined in this port (no write seam).
            _ => format!(
                "Set effort level to {} (this session only): {}",
                level.as_str(),
                level.description()
            ),
        }
    }
}

#[async_trait]
impl BuiltinCommandHandler for EffortHandler {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult {
        let trimmed = args.raw_args.trim();
        // Help args are matched on the raw (trimmed) token, mirroring
        // COMMON_HELP_ARGS.includes(args) in TS (case-sensitive there).
        if matches!(trimmed, "help" | "-h" | "--help") {
            return CommandResult::Done {
                display: Some(USAGE.to_string()),
            };
        }

        let normalized = trimmed.to_lowercase();
        let display = if normalized.is_empty()
            || normalized == "current"
            || normalized == "status"
        {
            self.show_current().await
        } else if normalized == "auto" || normalized == "unset" {
            Self::clear_effort()
        } else if let Some(level) = parse_effort_level(&normalized) {
            Self::set_effort(level)
        } else {
            // `executeEffort` invalid-arg branch (`effort.tsx` L114) — uses the
            // original (un-normalized, trimmed) argument text.
            format!("Invalid argument: {trimmed}. Valid options are: low, medium, high, max, auto")
        };

        CommandResult::Done {
            display: Some(display),
        }
    }

    fn name(&self) -> &str {
        "effort"
    }

    fn description(&self) -> &str {
        // Verbatim TS metadata (effort/index.ts) — these handlers are actually
        // implemented, so they carry the real description rather than the
        // `core_description` "(unimplemented)" fallback the pass-1 stubs use.
        "Set effort level for model usage"
    }
}

#[cfg(test)]
// The env-serialization guard is deliberately held across the command's
// `.await`: it keeps `CLAUDE_CODE_EFFORT_LEVEL` stable for the duration of
// `handle()` so the process-global env var can't race between parallel tests.
// `#[tokio::test]` runs on a single-thread runtime and nothing re-locks
// `ENV_LOCK` inside the awaited future, so there is no deadlock risk.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use orchestrator::test_support::MockOrchestratorHandle;

    /// Env-mutating tests must run serialized: they share the one process-wide
    /// `CLAUDE_CODE_EFFORT_LEVEL`. A module-level mutex serializes them.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn args(raw: &str) -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "effort".to_string(),
            raw_args: raw.to_string(),
            positional_args: vec![],
        }
    }

    fn handler() -> EffortHandler {
        EffortHandler::new(Arc::new(MockOrchestratorHandle::new()))
    }

    async fn run(raw: &str) -> String {
        match handler().handle(&args(raw)).await {
            CommandResult::Done { display: Some(s) } => s,
            other => panic!("expected Done with display, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn help_args_render_usage() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(EFFORT_ENV_VAR);
        for raw in ["help", "-h", "--help", "  help  "] {
            assert_eq!(run(raw).await, USAGE);
        }
    }

    #[tokio::test]
    async fn current_with_no_env_renders_auto_subset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(EFFORT_ENV_VAR);
        for raw in ["", "  ", "current", "status", "CURRENT"] {
            assert_eq!(run(raw).await, "Effort level: auto (currently high)");
        }
    }

    #[tokio::test]
    async fn current_with_env_pinned_renders_effective_level() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("current").await,
            "Current effort level: high (Comprehensive implementation with extensive testing and documentation)"
        );
        std::env::remove_var(EFFORT_ENV_VAR);
    }

    #[tokio::test]
    async fn current_with_env_cleared_renders_auto_subset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(EFFORT_ENV_VAR, "unset");
        assert_eq!(run("").await, "Effort level: auto (currently high)");
        std::env::remove_var(EFFORT_ENV_VAR);
    }

    #[tokio::test]
    async fn set_valid_level_no_env_is_session_only() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(EFFORT_ENV_VAR);
        assert_eq!(
            run("medium").await,
            "Set effort level to medium (this session only): Balanced approach with standard implementation and testing"
        );
        assert_eq!(
            run("MAX").await,
            "Set effort level to max (this session only): Maximum capability with deepest reasoning (Opus 4.6 only)"
        );
    }

    #[tokio::test]
    async fn set_level_conflicting_env_is_not_applied() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(EFFORT_ENV_VAR, "low");
        assert_eq!(
            run("high").await,
            "Not applied: CLAUDE_CODE_EFFORT_LEVEL=low overrides effort this session, and high is session-only (nothing saved)"
        );
        std::env::remove_var(EFFORT_ENV_VAR);
    }

    #[tokio::test]
    async fn set_level_matching_env_has_no_conflict_note() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("high").await,
            "Set effort level to high (this session only): Comprehensive implementation with extensive testing and documentation"
        );
        std::env::remove_var(EFFORT_ENV_VAR);
    }

    #[tokio::test]
    async fn clear_no_env_sets_auto() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(EFFORT_ENV_VAR);
        assert_eq!(run("auto").await, "Effort level set to auto");
        assert_eq!(run("unset").await, "Effort level set to auto");
    }

    #[tokio::test]
    async fn clear_with_env_pinned_warns() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("auto").await,
            "Cleared effort from settings, but CLAUDE_CODE_EFFORT_LEVEL=high still controls this session"
        );
        std::env::remove_var(EFFORT_ENV_VAR);
    }

    #[tokio::test]
    async fn invalid_arg_message() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(EFFORT_ENV_VAR);
        assert_eq!(
            run("bogus").await,
            "Invalid argument: bogus. Valid options are: low, medium, high, max, auto"
        );
    }

    #[tokio::test]
    async fn name_and_description() {
        let h = handler();
        assert_eq!(h.name(), "effort");
        assert_eq!(h.description(), "Set effort level for model usage");
    }
}
