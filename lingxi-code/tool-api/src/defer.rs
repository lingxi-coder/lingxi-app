//! Deferred-tool loading state — the shared spine of the Tool Search pipeline.
//!
//! claude-code 2.1.207 runs a "tool search" mode in which tools whose
//! `shouldDefer === true` are serialized on the wire WITH their schema plus
//! `defer_loading: true`, and the model pulls them into context on demand via
//! the `ToolSearch` tool. Two facts drive this port:
//!
//! - the DEFERRAL PREDICATE (binary): `if (e.name === "EnterWorktree" &&
//!   process.env.CLAUDE_CODE_SESSION_KIND === "bg") return false; return
//!   e.shouldDefer === true` — every tool that reports `should_defer()` is
//!   deferred, EXCEPT `EnterWorktree` inside a `bg` session.
//! - the LIFECYCLE: once the model loads a deferred tool (via `ToolSearch`), it
//!   is no longer deferred on subsequent turns — its full schema stays present.
//!
//! [`DeferralState`] is the single object shared between the wire serializer
//! ([`crate::wire::apply_defer_loading`]) and the `ToolSearch` consumer so both
//! ends stay consistent: the wire defers exactly the set `ToolSearch` searches,
//! and marking a tool loaded in `ToolSearch` un-defers it on the next turn's
//! wire. It is OWNED by the [`ToolRegistry`](crate::registry::ToolRegistry),
//! which already flows to the orchestrator's wire assembly and to the tool.
//!
//! ACTIVATION divergence (documented): [`mode_from_env`] reads
//! `LINGXI_ENABLE_TOOL_SEARCH` / `ENABLE_TOOL_SEARCH` (claude-code's env name)
//! and the `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` kill switch. Unlike
//! claude-code — which defaults tool search ON (`"tst"`) for supporting models —
//! this port keeps the pipeline DISABLED unless the enable var is explicitly
//! truthy. Reason: the in-turn `tool_reference` discovery blocks (which let the
//! model load a deferred tool's schema WITHIN a turn) are a documented
//! follow-up; the cross-turn "search → mark loaded → next turn present"
//! lifecycle IS wired here, but defaulting on without the in-turn blocks would
//! strand deferred tools for a full turn. When disabled every method is inert
//! and the wire bytes are byte-identical to the pre-pipeline build.

use crate::tool_trait::Tool;
use std::collections::HashSet;
use std::sync::RwLock;

/// Byte-locked name of the worktree-entry tool (claude-code `k9e`), the sole
/// `shouldDefer` tool that is NOT deferred inside a `bg` session.
pub const ENTER_WORKTREE_TOOL_NAME: &str = "EnterWorktree";

/// Tool-search mode (claude-code `e$r()` result: `standard` / `tst` /
/// `tst-auto`). This port collapses `tst` and `tst-auto` into a single ENABLED
/// state — the `auto:N` estimated-token-savings threshold decision is not ported
/// (treated as a plain enable); see module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSearchMode {
    /// Tool search off — nothing is deferred (claude-code `"standard"`).
    Standard,
    /// Tool search on (claude-code `"tst"` / `"tst-auto"`).
    Enabled,
}

impl ToolSearchMode {
    /// Whether deferral is active in this mode.
    #[must_use]
    pub fn is_enabled(self) -> bool {
        matches!(self, ToolSearchMode::Enabled)
    }
}

/// Parse the tool-search mode from an enable value and the experimental-betas
/// kill switch. Mirrors the shape of claude-code `e$r()` with the documented
/// default-off divergence (see module docs).
///
/// - kill switch set → `Standard`.
/// - enable unset → `Standard` (DIVERGES from claude-code default `tst`).
/// - enable defined-falsy (`0`/`false`/`no`/`off`) → `Standard`.
/// - enable truthy (`1`/`true`/`yes`/`on`), or one of `tst` / `tst-auto` /
///   `auto` / `auto:N` → `Enabled`.
/// - anything else → `Standard`.
#[must_use]
pub fn mode_from_values(enable: Option<&str>, disable_experimental_betas: bool) -> ToolSearchMode {
    if disable_experimental_betas {
        return ToolSearchMode::Standard;
    }
    let Some(raw) = enable else {
        return ToolSearchMode::Standard;
    };
    let v = raw.trim().to_lowercase();
    if v == "tst" || v == "tst-auto" || v == "auto" || v.starts_with("auto:") {
        return ToolSearchMode::Enabled;
    }
    if traits::env::is_env_defined_falsy(Some(&v)) {
        return ToolSearchMode::Standard;
    }
    if traits::env::is_env_truthy(Some(&v)) {
        return ToolSearchMode::Enabled;
    }
    ToolSearchMode::Standard
}

/// Read [`mode_from_values`] from the live environment. Prefers the rebranded
/// `LINGXI_ENABLE_TOOL_SEARCH`, falling back to claude-code's bare
/// `ENABLE_TOOL_SEARCH`; kill switch is `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`
/// (kept, matching `llm-client`'s beta gate).
#[must_use]
pub fn mode_from_env() -> ToolSearchMode {
    let enable = std::env::var("LINGXI_ENABLE_TOOL_SEARCH")
        .ok()
        .or_else(|| std::env::var("ENABLE_TOOL_SEARCH").ok());
    let disable_betas = traits::env::is_env_truthy(
        std::env::var("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
            .ok()
            .as_deref(),
    );
    mode_from_values(enable.as_deref(), disable_betas)
}

/// Shared deferral state for a session: the tool-search mode, the `bg`-session
/// flag (for the `EnterWorktree` exception), and the set of tool names the model
/// has loaded via `ToolSearch` this session.
pub struct DeferralState {
    mode: ToolSearchMode,
    session_kind_bg: bool,
    loaded: RwLock<HashSet<String>>,
}

impl DeferralState {
    /// Construct with an explicit mode and `bg`-session flag.
    #[must_use]
    pub fn new(mode: ToolSearchMode, session_kind_bg: bool) -> Self {
        Self {
            mode,
            session_kind_bg,
            loaded: RwLock::new(HashSet::new()),
        }
    }

    /// A disabled state (nothing deferred) — the default for tests and any
    /// non-tool-search session.
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(ToolSearchMode::Standard, false)
    }

    /// Construct from the live environment: [`mode_from_env`] plus the
    /// `LINGXI_SESSION_KIND == "bg"` flag (claude-code `CLAUDE_CODE_SESSION_KIND`).
    #[must_use]
    pub fn from_env() -> Self {
        let bg = std::env::var("LINGXI_SESSION_KIND").ok().as_deref() == Some("bg");
        Self::new(mode_from_env(), bg)
    }

    /// The tool-search mode.
    #[must_use]
    pub fn mode(&self) -> ToolSearchMode {
        self.mode
    }

    /// Whether deferral is active.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.mode.is_enabled()
    }

    /// Mark a set of tool names as loaded (pulled into context via `ToolSearch`),
    /// so they stop being deferred on subsequent turns.
    pub fn mark_loaded<I, S>(&self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut g = self.loaded.write().expect("deferral loaded-set poisoned");
        for n in names {
            g.insert(n.into());
        }
    }

    /// Whether a tool has been loaded this session.
    #[must_use]
    pub fn is_loaded(&self, name: &str) -> bool {
        self.loaded
            .read()
            .expect("deferral loaded-set poisoned")
            .contains(name)
    }

    /// The claude-code deferral predicate applied to a concrete tool for THIS
    /// turn's wire.
    #[must_use]
    pub fn should_defer_tool(&self, tool: &dyn Tool) -> bool {
        self.should_defer(tool.name(), tool.should_defer())
    }

    /// The claude-code deferral predicate by name + `should_defer` flag:
    /// `enabled && wants_defer && !loaded`, with the `EnterWorktree`/`bg`
    /// exception applied first.
    #[must_use]
    pub fn should_defer(&self, name: &str, wants_defer: bool) -> bool {
        if !self.is_enabled() {
            return false;
        }
        if name == ENTER_WORKTREE_TOOL_NAME && self.session_kind_bg {
            return false;
        }
        wants_defer && !self.is_loaded(name)
    }
}

impl Default for DeferralState {
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_off_when_unset() {
        // Divergence from claude-code's default-on: unset ⇒ Standard.
        assert_eq!(mode_from_values(None, false), ToolSearchMode::Standard);
    }

    #[test]
    fn mode_truthy_enables() {
        for v in [
            "1", "true", "on", "yes", "TST", "tst", "tst-auto", "auto", "auto:0", "auto:100",
        ] {
            assert_eq!(
                mode_from_values(Some(v), false),
                ToolSearchMode::Enabled,
                "{v:?} should enable"
            );
        }
    }

    #[test]
    fn mode_falsy_disables() {
        for v in ["0", "false", "off", "no", ""] {
            assert_eq!(
                mode_from_values(Some(v), false),
                ToolSearchMode::Standard,
                "{v:?} should disable"
            );
        }
    }

    #[test]
    fn experimental_betas_kill_switch_forces_standard() {
        // Even an explicit enable is overridden by the kill switch.
        assert_eq!(
            mode_from_values(Some("true"), true),
            ToolSearchMode::Standard
        );
        assert_eq!(
            mode_from_values(Some("tst"), true),
            ToolSearchMode::Standard
        );
    }

    #[test]
    fn disabled_state_never_defers() {
        let d = DeferralState::disabled();
        assert!(!d.is_enabled());
        assert!(!d.should_defer("Task", true));
    }

    #[test]
    fn enabled_defers_should_defer_tools_only() {
        let d = DeferralState::new(ToolSearchMode::Enabled, false);
        assert!(d.should_defer("Task", true));
        assert!(!d.should_defer("Read", false));
    }

    #[test]
    fn loaded_tool_is_no_longer_deferred() {
        let d = DeferralState::new(ToolSearchMode::Enabled, false);
        assert!(d.should_defer("Task", true));
        d.mark_loaded(["Task".to_string()]);
        assert!(d.is_loaded("Task"));
        assert!(!d.should_defer("Task", true));
    }

    #[test]
    fn enter_worktree_not_deferred_in_bg_session() {
        // bg session: EnterWorktree is exempt from deferral even with wants_defer.
        let bg = DeferralState::new(ToolSearchMode::Enabled, true);
        assert!(!bg.should_defer(ENTER_WORKTREE_TOOL_NAME, true));
        // A different tool with wants_defer is still deferred in bg.
        assert!(bg.should_defer("Task", true));
        // Non-bg session: EnterWorktree follows the normal predicate.
        let fg = DeferralState::new(ToolSearchMode::Enabled, false);
        assert!(fg.should_defer(ENTER_WORKTREE_TOOL_NAME, true));
    }
}
