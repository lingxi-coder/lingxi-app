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
//! ## Persistence + resolver
//!
//! The TS set/clear paths call
//! `updateSettingsForSource('userSettings', { effortLevel })`. The Rust
//! settings crate is a read-only loader, but a write is not load-bearing on
//! it: we persist with the same direct-fs seam `export.rs` uses, mirroring
//! `updateSettingsForSource` byte-for-byte (merge into the existing
//! `~/.claude/settings.json`, treat a missing value as a delete, never
//! overwrite a JSON-syntax-broken file). [`persist_effort_level`] is the port.
//!
//! Per [`to_persistable`] (TS `toPersistableEffort`, `effort.ts` L95) only
//! `low`/`medium`/`high` are persistable for non-ant users; `max` is
//! session-scoped, so setting `max` keeps the `" (this session only)"` suffix
//! and writes nothing — 1:1 with the TS `persistable === undefined` branch.
//!
//! The `auto (currently {level})` computed level is driven by the ported
//! [`get_displayed_effort_level`] → [`resolve_applied_effort`] →
//! [`get_default_effort_for_model`] + [`model_supports_max_effort`] chain.
//! Three TS branches of `getDefaultEffortForModel` are seam-blocked and
//! documented on that fn (Pro/Max/Team → medium needs subscriber-auth +
//! `GrowthBook`; ultrathink → medium needs the ultrathink seam) — none is
//! reachable in-tree, so every reachable model resolves to the API default
//! `high`, computed rather than hard-coded.
//!
//! The env override (`CLAUDE_CODE_EFFORT_LEVEL`) is honoured in every branch.

use async_trait::async_trait;
use command_api::model::{BuiltinCommandHandler, CommandResult};
use command_api::parser::ParsedSlashCommand;
use serde_json::{json, Value};
use std::path::PathBuf;
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

/// `toPersistableEffort` (`effort.ts` L95) — the persistable subset of a
/// level. `low`/`medium`/`high` persist; `max` is session-scoped for non-ant
/// users (this port is non-ant), so it returns `None`. Numeric efforts are
/// ANT-only and already absent from [`EffortLevel`].
fn to_persistable(level: EffortLevel) -> Option<EffortLevel> {
    match level {
        EffortLevel::Low | EffortLevel::Medium | EffortLevel::High => Some(level),
        EffortLevel::Max => None,
    }
}

/// `<config-home>/settings.json` — `$CLAUDE_CONFIG_DIR` when set (claude-code
/// `tr()` `??`: an empty value is honored verbatim), else `~/.claude`.
/// Byte-identical to the engine settings loader
/// (`engine/src/settings/loader.rs` `config_home_dir` + `user_settings_path`)
/// so `/effort`'s persisted `effortLevel` lands in the SAME file the loader and
/// `/config` read. `None` if neither the env override nor `HOME` resolves (TS
/// `getSettingsFilePathForSource` → `null` → `{ error: null }`).
fn user_settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude").join("settings.json"))
}

/// Persist `effortLevel` into the user `settings.json`, mirroring
/// `updateSettingsForSource('userSettings', { effortLevel })`
/// (`settings.ts` L416). `Some(level)` writes the key; `None` deletes it
/// (TS `mergeWith` treats `undefined` as a delete, L483). All other keys are
/// preserved.
///
/// Faithful to the TS error contract: a missing/empty file merges into an
/// empty object, but a file whose JSON is syntactically broken is left
/// untouched and surfaces `Invalid JSON syntax …` (L459) rather than being
/// overwritten.
fn persist_effort_level(level: Option<EffortLevel>) -> Result<(), String> {
    // TS: filePath === null → { error: null }.
    let Some(path) = user_settings_path() else {
        return Ok(());
    };

    // TS: mkdirSync(dirname(filePath)).
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to read raw settings from {}: {e}", path.display()))?;
    }

    // Read existing settings. ENOENT / empty → empty map; broken JSON → bail
    // without overwriting (TS L459).
    let mut map: serde_json::Map<String, Value> = match std::fs::read_to_string(&path) {
        Ok(content) if content.trim().is_empty() => serde_json::Map::new(),
        Ok(content) => serde_json::from_str(&content)
            .map_err(|_| format!("Invalid JSON syntax in settings file at {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => {
            return Err(format!(
                "Failed to read raw settings from {}: {e}",
                path.display()
            ))
        }
    };

    // mergeWith: Some → set, None → delete.
    match level {
        Some(level) => {
            map.insert("effortLevel".to_string(), json!(level.as_str()));
        }
        None => {
            map.remove("effortLevel");
        }
    }

    // jsonStringify(updatedSettings, null, 2) + '\n' — 2-space indent + newline.
    let serialized = serde_json::to_string_pretty(&map)
        .map_err(|e| format!("Failed to read raw settings from {}: {e}", path.display()))?;
    std::fs::write(&path, serialized + "\n")
        .map_err(|e| format!("Failed to read raw settings from {}: {e}", path.display()))?;
    Ok(())
}

/// `modelSupportsMaxEffort` (`effort.ts` L53) — the non-ant reachable subset:
/// `max` is Opus-4.6-only for public models. The 3P-override and ANT branches
/// are seam-blocked (no `get3PModelCapabilityOverride` / `resolveAntModel` in
/// this port).
fn model_supports_max_effort(model: &str) -> bool {
    model.to_lowercase().contains("opus-4-6")
}

/// `getDefaultEffortForModel` (`effort.ts` L279) — the non-ant reachable
/// subset.
///
/// Every TS branch that can return a non-`undefined` default is seam-blocked
/// in this port and so returns `None`:
///   * ANT model overrides (`resolveAntModel` / `getAntModelOverrideConfig`);
///   * Opus-4.6 → `medium` for Pro/Max/Team subscribers — needs the
///     subscriber-auth (`isProSubscriber` …) and `GrowthBook`
///     (`getOpusDefaultEffortConfig`) seams;
///   * ultrathink → `medium` — needs the `isUltrathinkEnabled` seam.
///
/// With none of those reachable in-tree the TS fallback (`return undefined`,
/// L328 — "resolve to high in the API") is the only live path, so this
/// returns `None` for every model. The fn exists to wire the precedence
/// chain; flipping any seam on later changes the answer without touching the
/// call sites.
fn get_default_effort_for_model(_model: &str) -> Option<EffortLevel> {
    None
}

/// `convertEffortValueToLevel` (`effort.ts` L202) — for the string levels this
/// port carries it is a passthrough (the numeric-coercion + `GrowthBook`
/// `'high'` guard only apply to numeric/remote values, which are ANT-only and
/// absent from [`EffortLevel`]).
fn convert_effort_value_to_level(level: EffortLevel) -> EffortLevel {
    level
}

/// `resolveAppliedEffort` (`effort.ts` L152) — the effort that would actually
/// be sent for `model`, following `env → app-state → model default`. `None`
/// means "send no effort param" (env cleared, or no default).
///
/// `app_state` mirrors the TS `appStateEffortValue` argument; this port has no
/// app-state effort read seam, so call sites pass `None`.
fn resolve_applied_effort(model: &str, app_state: Option<EffortLevel>) -> Option<EffortLevel> {
    match effort_env_override() {
        // envOverride === null → undefined.
        EnvOverride::Cleared => None,
        // envOverride ?? appState ?? getDefaultEffortForModel(model).
        env => {
            let resolved = match env {
                EnvOverride::Pinned { level, .. } => Some(level),
                _ => app_state.or_else(|| get_default_effort_for_model(model)),
            };
            // API rejects 'max' on non-Opus-4.6 — downgrade to 'high' (L163).
            match resolved {
                Some(EffortLevel::Max) if !model_supports_max_effort(model) => {
                    Some(EffortLevel::High)
                }
                other => other,
            }
        }
    }
}

/// `getDisplayedEffortLevel` (`effort.ts` L174) — [`resolve_applied_effort`]
/// with the `?? 'high'` API-default fallback, then `convertEffortValueToLevel`.
fn get_displayed_effort_level(model: &str, app_state: Option<EffortLevel>) -> EffortLevel {
    let resolved = resolve_applied_effort(model, app_state).unwrap_or(EffortLevel::High);
    convert_effort_value_to_level(resolved)
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
                // `getDisplayedEffortLevel(model, appStateEffort)` (effort.ts
                // L178). This port has no app-state effort read seam (TS reads
                // `appStateEffort`, not `settings.json`, so a persisted level
                // does NOT surface here either), so app-state is `None`; with
                // every reachable model default seam-blocked the resolver lands
                // on the API default `high`. The level is now computed by the
                // real `getDisplayedEffortLevel` port rather than hard-coded.
                let model = self.handle.get_status_snapshot().await.model;
                format!(
                    "Effort level: auto (currently {})",
                    get_displayed_effort_level(&model, None).as_str()
                )
            }
        }
    }

    /// `unsetEffortLevel` (`effort.tsx` L76-106) — the `auto`/`unset` branch.
    /// Deletes the persisted `effortLevel`; only the env-conflict note varies.
    ///
    /// Kept an associated fn (not `&self`): the body only touches the
    /// process env + the user `settings.json` via free helpers, so a `&self`
    /// receiver would trip `clippy::unused_self`.
    fn clear_effort() -> String {
        // updateSettingsForSource('userSettings', { effortLevel: undefined }).
        if let Err(msg) = persist_effort_level(None) {
            return format!("Failed to set effort level: {msg}");
        }
        match effort_env_override() {
            EnvOverride::Pinned { raw, .. } => format!(
                "Cleared effort from settings, but {EFFORT_ENV_VAR}={raw} still controls this session"
            ),
            EnvOverride::Cleared | EnvOverride::Unset => "Effort level set to auto".to_string(),
        }
    }

    /// `setEffortValue` (`effort.tsx` L16-61) — the valid-level branch.
    ///
    /// Kept an associated fn (not `&self`) for the same reason as
    /// [`Self::clear_effort`] — no receiver state is used, so `&self` would
    /// trip `clippy::unused_self`.
    fn set_effort(level: EffortLevel) -> String {
        // toPersistableEffort: low/medium/high persist, max is session-only.
        let persistable = to_persistable(level);
        if persistable.is_some() {
            if let Err(msg) = persist_effort_level(Some(level)) {
                return format!("Failed to set effort level: {msg}");
            }
        }

        // TS flags env conflict only when env pins a *different* level than the
        // one the user asked for (`envOverride !== effortValue`). The note
        // wording then branches on whether the level was persistable.
        match effort_env_override() {
            EnvOverride::Pinned {
                level: env_level,
                raw,
            } if env_level != level => {
                if persistable.is_none() {
                    // Session-only level can't outlast the env (L38).
                    format!(
                        "Not applied: {EFFORT_ENV_VAR}={raw} overrides effort this session, and {} is session-only (nothing saved)",
                        level.as_str()
                    )
                } else {
                    // Persisted, but env wins until cleared (L47).
                    format!(
                        "{EFFORT_ENV_VAR}={raw} overrides this session — clear it and {} takes over",
                        level.as_str()
                    )
                }
            }
            // No conflict → `Set effort level to {x}{suffix}: {desc}`. The
            // `(this session only)` suffix fires only for non-persistable
            // (`max`) levels (L54).
            _ => {
                let suffix = if persistable.is_some() {
                    ""
                } else {
                    " (this session only)"
                };
                format!(
                    "Set effort level to {}{suffix}: {}",
                    level.as_str(),
                    level.description()
                )
            }
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
        let display = if normalized.is_empty() || normalized == "current" || normalized == "status"
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
    /// `CLAUDE_CODE_EFFORT_LEVEL` *and* `HOME` (now that the set/clear paths
    /// write `~/.claude/settings.json`). A module-level mutex serializes them.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII test fixture: holds [`ENV_LOCK`], redirects `HOME` to a fresh
    /// per-test temp dir (so persistence never touches the real `~/.claude`),
    /// and clears `CLAUDE_CODE_EFFORT_LEVEL`. On drop it restores the prior
    /// `HOME` and removes the temp dir. Mirrors the `HOME_LOCK` pattern in
    /// `engine/src/settings`; uses `std::env::temp_dir()` rather than the
    /// `tempfile` crate, matching the `export.rs` test precedent (no new dep).
    struct TestEnv {
        _guard: std::sync::MutexGuard<'static, ()>,
        home: PathBuf,
        prev_home: Option<std::ffi::OsString>,
    }

    impl TestEnv {
        fn new() -> Self {
            let guard = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let prev_home = std::env::var_os("HOME");
            // Unique per process + per nanosecond so parallel binaries / repeat
            // runs never collide.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let home = std::env::temp_dir()
                .join(format!("lingxi-effort-test-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            std::env::set_var("HOME", &home);
            std::env::remove_var(EFFORT_ENV_VAR);
            Self {
                _guard: guard,
                home,
                prev_home,
            }
        }

        /// The redirected `~/.claude/settings.json` path.
        fn settings_path(&self) -> PathBuf {
            self.home.join(".claude").join("settings.json")
        }

        /// Pre-seed `settings.json` with the given raw bytes (for the
        /// merge / broken-JSON tests).
        fn write_settings(&self, raw: &str) {
            let path = self.settings_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw).unwrap();
        }

        /// Read `settings.json` back as a JSON object, or `None` if absent.
        fn read_settings(&self) -> Option<serde_json::Map<String, Value>> {
            std::fs::read_to_string(self.settings_path())
                .ok()
                .map(|c| serde_json::from_str(&c).unwrap())
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            match &self.prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            std::env::remove_var(EFFORT_ENV_VAR);
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

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
        let _env = TestEnv::new();
        for raw in ["help", "-h", "--help", "  help  "] {
            assert_eq!(run(raw).await, USAGE);
        }
    }

    #[tokio::test]
    async fn current_with_no_env_renders_auto_subset() {
        let _env = TestEnv::new();
        for raw in ["", "  ", "current", "status", "CURRENT"] {
            assert_eq!(run(raw).await, "Effort level: auto (currently high)");
        }
    }

    #[tokio::test]
    async fn current_with_env_pinned_renders_effective_level() {
        let _env = TestEnv::new();
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("current").await,
            "Current effort level: high (Comprehensive implementation with extensive testing and documentation)"
        );
    }

    #[tokio::test]
    async fn current_with_env_cleared_renders_auto_subset() {
        let _env = TestEnv::new();
        std::env::set_var(EFFORT_ENV_VAR, "unset");
        assert_eq!(run("").await, "Effort level: auto (currently high)");
    }

    #[tokio::test]
    async fn set_valid_level_no_env_is_session_only() {
        let env = TestEnv::new();
        // low/medium/high are now persistable → suffix dropped.
        assert_eq!(
            run("medium").await,
            "Set effort level to medium: Balanced approach with standard implementation and testing"
        );
        // max stays session-only (toPersistableEffort(max) === undefined for
        // non-ant) → suffix kept, nothing written.
        assert_eq!(
            run("MAX").await,
            "Set effort level to max (this session only): Maximum capability with deepest reasoning (Opus 4.6 only)"
        );
        // Persisted value is the last *persistable* set (medium); max didn't
        // overwrite it.
        assert_eq!(
            env.read_settings().unwrap().get("effortLevel"),
            Some(&json!("medium"))
        );
    }

    #[tokio::test]
    async fn set_level_conflicting_env_is_not_applied() {
        let _env = TestEnv::new();
        // env=low, ask max (non-persistable) → "Not applied … nothing saved".
        std::env::set_var(EFFORT_ENV_VAR, "low");
        assert_eq!(
            run("max").await,
            "Not applied: CLAUDE_CODE_EFFORT_LEVEL=low overrides effort this session, and max is session-only (nothing saved)"
        );
    }

    #[tokio::test]
    async fn set_level_matching_env_has_no_conflict_note() {
        let _env = TestEnv::new();
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("high").await,
            "Set effort level to high: Comprehensive implementation with extensive testing and documentation"
        );
    }

    #[tokio::test]
    async fn clear_no_env_sets_auto() {
        let _env = TestEnv::new();
        assert_eq!(run("auto").await, "Effort level set to auto");
        assert_eq!(run("unset").await, "Effort level set to auto");
    }

    #[tokio::test]
    async fn clear_with_env_pinned_warns() {
        let _env = TestEnv::new();
        std::env::set_var(EFFORT_ENV_VAR, "high");
        assert_eq!(
            run("auto").await,
            "Cleared effort from settings, but CLAUDE_CODE_EFFORT_LEVEL=high still controls this session"
        );
    }

    #[tokio::test]
    async fn invalid_arg_message() {
        let _env = TestEnv::new();
        assert_eq!(
            run("bogus").await,
            "Invalid argument: bogus. Valid options are: low, medium, high, max, auto"
        );
    }

    #[tokio::test]
    async fn name_and_description() {
        let _env = TestEnv::new();
        let h = handler();
        assert_eq!(h.name(), "effort");
        assert_eq!(h.description(), "Set effort level for model usage");
    }

    // ---- persistence (updateSettingsForSource) parity ----

    #[tokio::test]
    async fn persist_creates_settings_and_writes_effort_level() {
        let env = TestEnv::new();
        assert_eq!(
            run("high").await,
            "Set effort level to high: Comprehensive implementation with extensive testing and documentation"
        );
        let map = env.read_settings().expect("settings.json written");
        assert_eq!(map.get("effortLevel"), Some(&json!("high")));
        assert_eq!(map.len(), 1);
    }

    #[tokio::test]
    async fn persist_merges_into_existing_settings() {
        let env = TestEnv::new();
        env.write_settings("{\"model\":\"opus\"}");
        run("low").await;
        let map = env.read_settings().unwrap();
        assert_eq!(map.get("model"), Some(&json!("opus")));
        assert_eq!(map.get("effortLevel"), Some(&json!("low")));
    }

    #[tokio::test]
    async fn persist_max_is_session_only_not_written() {
        let env = TestEnv::new();
        env.write_settings("{}");
        assert_eq!(
            run("max").await,
            "Set effort level to max (this session only): Maximum capability with deepest reasoning (Opus 4.6 only)"
        );
        assert!(env.read_settings().unwrap().get("effortLevel").is_none());
    }

    #[tokio::test]
    async fn clear_removes_effort_level() {
        let env = TestEnv::new();
        env.write_settings("{\"effortLevel\":\"high\",\"model\":\"x\"}");
        assert_eq!(run("auto").await, "Effort level set to auto");
        let map = env.read_settings().unwrap();
        assert!(map.get("effortLevel").is_none());
        assert_eq!(map.get("model"), Some(&json!("x")));
    }

    #[tokio::test]
    async fn broken_settings_json_not_overwritten() {
        let env = TestEnv::new();
        let raw = "{ bad json";
        env.write_settings(raw);
        let msg = run("high").await;
        let path = env.settings_path();
        assert_eq!(
            msg,
            format!(
                "Failed to set effort level: Invalid JSON syntax in settings file at {}",
                path.display()
            )
        );
        // File bytes must be untouched (parity with TS L459).
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[tokio::test]
    async fn set_conflicting_env_with_persistable_uses_override_note() {
        let env = TestEnv::new();
        std::env::set_var(EFFORT_ENV_VAR, "low");
        assert_eq!(
            run("high").await,
            "CLAUDE_CODE_EFFORT_LEVEL=low overrides this session — clear it and high takes over"
        );
        // Persistable set still wrote to disk (env only wins at resolve time).
        assert_eq!(
            env.read_settings().unwrap().get("effortLevel"),
            Some(&json!("high"))
        );
    }

    #[tokio::test]
    async fn current_after_persist_still_renders_auto_high() {
        let _env = TestEnv::new();
        // Persist a level…
        run("high").await;
        // …then `current` still renders auto, because showCurrentEffort reads
        // appStateEffort (None here), not settings.json.
        assert_eq!(run("current").await, "Effort level: auto (currently high)");
    }

    // ---- resolver unit (effort.ts) ----

    #[test]
    fn resolver_unit() {
        // `get_displayed_effort_level` reads `CLAUDE_CODE_EFFORT_LEVEL`, so
        // serialize against the env-mutating tests (TestEnv clears it).
        let _env = TestEnv::new();
        assert_eq!(get_default_effort_for_model("claude-opus-4-6"), None);
        assert_eq!(
            get_displayed_effort_level("claude-opus-4-6", None),
            EffortLevel::High
        );
        assert!(model_supports_max_effort("claude-opus-4-6"));
        assert!(!model_supports_max_effort("claude-sonnet-4-6"));
    }
}
