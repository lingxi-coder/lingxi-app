//! Deferred-tool loading state — the shared spine of the Tool Search pipeline.
//!
//! claude-code 2.1.216 runs a "tool search" mode in which undiscovered tools
//! whose `shouldDefer === true` are omitted from the request tool list and the
//! model pulls matching definitions into context via the `ToolSearch` tool.
//! Once discovered, a definition returns to the wire with
//! `defer_loading: true` so the provider can retain dynamic-tool semantics.
//! Two facts drive this port:
//!
//! - the DEFERRAL PREDICATE (binary): `if (e.name === "EnterWorktree" &&
//!   process.env.CLAUDE_CODE_SESSION_KIND === "bg") return false; return
//!   e.shouldDefer === true` — every tool that reports `should_defer()` is
//!   deferred, EXCEPT `EnterWorktree` inside a `bg` session.
//! - the LIFECYCLE: once the model discovers a deferred tool (via
//!   `ToolSearch`), its full schema stays present on subsequent turns and the
//!   discovered name survives resume/compaction.
//!
//! [`DeferralState`] is the single object shared between the wire serializer
//! ([`crate::wire::apply_defer_loading`]) and the `ToolSearch` consumer so both
//! ends stay consistent: the wire omits exactly the undiscovered set
//! `ToolSearch` searches, and marking a tool loaded makes it visible on the
//! next turn's wire. It is OWNED by the
//! [`ToolRegistry`](crate::registry::ToolRegistry),
//! which already flows to the orchestrator's wire assembly and to the tool.
//!
//! [`mode_from_env`] reads `LINGXI_ENABLE_TOOL_SEARCH` /
//! `ENABLE_TOOL_SEARCH` (claude-code's env name) and the
//! `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` kill switch. Like claude-code, an
//! unset enable variable selects `tst`; request assembly subsequently disables
//! it for unsupported models/providers or when `ToolSearch` was denied.

use crate::tool_trait::Tool;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Byte-locked name of the worktree-entry tool (claude-code `k9e`), the sole
/// `shouldDefer` tool that is NOT deferred inside a `bg` session.
pub const ENTER_WORKTREE_TOOL_NAME: &str = "EnterWorktree";

/// Tool-search mode (claude-code `e$r()` result: `standard` / `tst` /
/// `tst-auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSearchMode {
    /// Tool search off — nothing is deferred (claude-code `"standard"`).
    Standard,
    /// Tool search on (claude-code `"tst"` / `"tst-auto"`).
    Enabled,
    /// Enable only when deferred schemas are estimated to save at least this
    /// percentage of the active model context window.
    Auto {
        /// Minimum estimated context-window percentage saved by deferral.
        percentage: u8,
    },
}

impl ToolSearchMode {
    /// Whether deferral is active in this mode.
    #[must_use]
    pub fn is_enabled(self) -> bool {
        !matches!(self, ToolSearchMode::Standard)
    }
}

/// Parse the tool-search mode from an enable value and the experimental-betas
/// kill switch. Mirrors the shape of claude-code `e$r()`.
///
/// Byte-faithful port of claude-code `qzr()` (with `Yos`/`B5g`):
/// - kill switch (`Q_e()`) set → `Standard`.
/// - `auto:N` (case-sensitive) → `Yos` parses N and clamps to `0..=100`:
///   `N==0` → `Enabled`, `N==100` (incl. any `N>100`) → `Standard`, otherwise
///   `Auto { percentage: N }`.
/// - `auto` (case-sensitive) → `Auto { percentage: 10 }`.
/// - a defined-falsy value (`0`/`false`/`no`/`off`) → `Standard`.
/// - EVERYTHING else — unset, empty, truthy, `tst`, `tst-auto`, or any
///   unrecognized string — → `Enabled` (the oracle's `Jt`-then-`return "tst"`
///   arms both resolve to enabled; only a defined-falsy value diverges).
///
/// Notably `tst-auto` as an INPUT is enabled (not auto — `B5g("tst-auto")` is
/// false and it is truthy), `auto:99999` is `Standard` (clamped to 100, not a
/// wrapped-integer `Auto`), and an unrecognized value is enabled (not standard).
#[must_use]
pub fn mode_from_values(enable: Option<&str>, disable_experimental_betas: bool) -> ToolSearchMode {
    // Q_e() kill switch.
    if disable_experimental_betas {
        return ToolSearchMode::Standard;
    }
    // Yos(e): `auto:N` clamped to 0..=100; anything else → None. Case-SENSITIVE.
    let auto_pct = enable.and_then(parse_auto_percentage);
    match auto_pct {
        Some(0) => return ToolSearchMode::Enabled, // t === 0 → "tst"
        Some(100) => return ToolSearchMode::Standard, // t === 100 → "standard"
        _ => {}
    }
    // B5g(e): e && (e === "auto" || e.startsWith("auto:")) → "tst-auto".
    if let Some(e) = enable {
        if e == "auto" || e.starts_with("auto:") {
            return ToolSearchMode::Auto {
                percentage: auto_pct.unwrap_or(10),
            };
        }
    }
    // `if(Jt(e))return"tst"` and the final `return"tst"` are the same outcome;
    // only a defined-falsy value (`nu`) diverges to "standard".
    if traits::env::is_env_defined_falsy(enable) {
        return ToolSearchMode::Standard;
    }
    ToolSearchMode::Enabled
}

/// `Yos(e)`: parse a case-sensitive `auto:N` value, clamped to `0..=100`.
/// Returns `None` for any value that does not start with `auto:` or whose `N`
/// fails to parse (`parseInt` NaN), so the caller's `B5g` still classifies a
/// malformed `auto:xxx` as auto with the default percentage.
fn parse_auto_percentage(e: &str) -> Option<u8> {
    let rest = e.strip_prefix("auto:")?;
    let n = parse_int_prefix(rest.trim())?;
    // Math.max(0, Math.min(100, n)).
    Some(n.clamp(0, 100) as u8)
}

/// `Ld`/`parseInt(t, 10)` semantics: an optional leading sign followed by ASCII
/// digits, stopping at the first non-digit; `None` (NaN) when no digits lead.
/// A value beyond `i64` saturates by sign so the caller's `clamp(0,100)` maps a
/// huge positive to 100 and a huge negative to 0, matching JS `Math` on floats.
fn parse_int_prefix(s: &str) -> Option<i64> {
    let mut chars = s.chars().peekable();
    let mut buf = String::new();
    if chars.peek().is_some_and(|c| matches!(c, '+' | '-')) {
        buf.push(chars.next().expect("peeked sign"));
    }
    while let Some(c) = chars.peek() {
        if c.is_ascii_digit() {
            buf.push(chars.next().expect("peeked digit"));
        } else {
            break;
        }
    }
    if buf.is_empty() || buf == "+" || buf == "-" {
        return None;
    }
    Some(buf.parse::<i64>().unwrap_or_else(|_| {
        if buf.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    }))
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
/// has discovered via `ToolSearch` this session.
pub struct DeferralState {
    mode: ToolSearchMode,
    session_kind_bg: bool,
    loaded: RwLock<HashSet<String>>,
    auto_active: AtomicBool,
    request_supported: AtomicBool,
}

impl DeferralState {
    /// Construct with an explicit mode and `bg`-session flag.
    #[must_use]
    pub fn new(mode: ToolSearchMode, session_kind_bg: bool) -> Self {
        Self {
            mode,
            session_kind_bg,
            loaded: RwLock::new(HashSet::new()),
            auto_active: AtomicBool::new(false),
            request_supported: AtomicBool::new(true),
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
        if !self.request_supported.load(Ordering::Acquire) {
            return false;
        }
        match self.mode {
            ToolSearchMode::Enabled => true,
            ToolSearchMode::Auto { .. } => self.auto_active.load(Ordering::Acquire),
            ToolSearchMode::Standard => false,
        }
    }

    /// Publish whether the active request can carry `defer_loading` and
    /// `tool_reference` blocks. This is recomputed on every model/profile/tool
    /// change, so switching away from Anthropic (or denying `ToolSearch`)
    /// immediately restores the complete inline tool list.
    pub fn set_request_supported(&self, supported: bool) {
        self.request_supported.store(supported, Ordering::Release);
    }

    /// Configured automatic threshold, if this session is in auto mode.
    #[must_use]
    pub fn auto_percentage(&self) -> Option<u8> {
        match self.mode {
            ToolSearchMode::Auto { percentage } => Some(percentage),
            ToolSearchMode::Standard | ToolSearchMode::Enabled => None,
        }
    }

    /// Publish the current turn's auto-threshold result before building the
    /// searchable view and applying wire markers.
    pub fn set_auto_active(&self, active: bool) {
        if matches!(self.mode, ToolSearchMode::Auto { .. }) {
            self.auto_active.store(active, Ordering::Release);
        }
    }

    /// Mark tool names as discovered (pulled into context via `ToolSearch`), so
    /// their definitions become visible on subsequent turns.
    pub fn mark_loaded<I, S>(&self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut g = self
            .loaded
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for n in names {
            g.insert(n.into());
        }
    }

    /// Replace the session's discovered-tool snapshot atomically. Resume and
    /// `/clear` must not merge a previous conversation's ToolSearch state into
    /// the newly adopted session.
    pub fn replace_loaded<I, S>(&self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut g = self
            .loaded
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.clear();
        g.extend(names.into_iter().map(Into::into));
    }

    /// Whether a tool has been discovered this session.
    #[must_use]
    pub fn is_loaded(&self, name: &str) -> bool {
        self.loaded
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(name)
    }

    /// Snapshot deferred-tool names already discovered in this session, sorted for
    /// compact-boundary persistence (`preCompactDiscoveredTools`).
    #[must_use]
    pub fn loaded_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .loaded
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The claude-code deferral predicate applied to a concrete tool for THIS
    /// turn's wire.
    #[must_use]
    pub fn should_defer_tool(&self, tool: &dyn Tool) -> bool {
        self.should_defer(tool.name(), tool.should_defer())
    }

    /// The claude-code deferral predicate by name + `should_defer` flag:
    /// `enabled && candidate && !loaded`, with the `EnterWorktree`/`bg`
    /// exception applied first. This identifies schemas not yet discovered.
    #[must_use]
    pub fn should_defer(&self, name: &str, wants_defer: bool) -> bool {
        if !self.is_enabled() {
            return false;
        }
        self.wants_defer(name, wants_defer) && !self.is_loaded(name)
    }

    /// Candidate predicate independent of the current auto-threshold decision.
    #[must_use]
    pub fn wants_defer(&self, name: &str, wants_defer: bool) -> bool {
        if name == ENTER_WORKTREE_TOOL_NAME && self.session_kind_bg {
            return false;
        }
        wants_defer
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
    fn mode_defaults_on_when_unset() {
        assert_eq!(mode_from_values(None, false), ToolSearchMode::Enabled);
    }

    #[test]
    fn mode_truthy_enables() {
        for v in ["1", "true", "on", "yes", "TST", "tst", "auto:0"] {
            assert_eq!(
                mode_from_values(Some(v), false),
                ToolSearchMode::Enabled,
                "{v:?} should enable"
            );
        }
    }

    #[test]
    fn auto_mode_parses_threshold_edges_and_starts_inactive() {
        assert_eq!(
            mode_from_values(Some("auto"), false),
            ToolSearchMode::Auto { percentage: 10 }
        );
        assert_eq!(
            mode_from_values(Some("auto:35junk"), false),
            ToolSearchMode::Auto { percentage: 35 }
        );
        // A malformed `auto:xxx` is still auto (B5g), with the default percentage.
        assert_eq!(
            mode_from_values(Some("auto:xyz"), false),
            ToolSearchMode::Auto { percentage: 10 }
        );
        assert_eq!(
            mode_from_values(Some("auto:100"), false),
            ToolSearchMode::Standard
        );
        // N > 100 clamps to 100 → Standard (NOT a wrapped-integer Auto).
        assert_eq!(
            mode_from_values(Some("auto:99999"), false),
            ToolSearchMode::Standard
        );
        let state = DeferralState::new(ToolSearchMode::Auto { percentage: 10 }, false);
        assert!(!state.is_enabled());
        state.set_auto_active(true);
        assert!(state.is_enabled());
    }

    #[test]
    fn qzr_fallthrough_enables_non_falsy_values() {
        // The oracle's `Jt`-then-`return "tst"` arms both enable, so any value
        // that is neither auto-form nor DEFINED-falsy is enabled — including the
        // literal `tst-auto` (not auto), an empty string, and unrecognized values.
        for v in ["tst-auto", "", "enabled", "xyz", "TST"] {
            assert_eq!(
                mode_from_values(Some(v), false),
                ToolSearchMode::Enabled,
                "{v:?} should enable via the qzr fallthrough"
            );
        }
    }

    #[test]
    fn mode_falsy_disables() {
        // Only DEFINED-falsy values disable; an empty string is NOT defined-falsy.
        for v in ["0", "false", "off", "no"] {
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
        assert!(d.wants_defer("Task", true));
    }

    #[test]
    fn unsupported_request_temporarily_disables_deferral() {
        let d = DeferralState::new(ToolSearchMode::Enabled, false);
        d.set_request_supported(false);
        assert!(!d.is_enabled());
        assert!(!d.should_defer("Task", true));
        d.set_request_supported(true);
        assert!(d.is_enabled());
    }

    #[test]
    fn loaded_tool_names_are_sorted_and_deduplicated() {
        let d = DeferralState::new(ToolSearchMode::Enabled, false);
        assert!(
            d.loaded_tool_names().is_empty(),
            "empty until a tool is loaded"
        );
        d.mark_loaded(["Zed", "Read", "Zed"]);
        assert_eq!(d.loaded_tool_names(), vec!["Read", "Zed"]);
        d.replace_loaded(["Task", "Task"]);
        assert_eq!(d.loaded_tool_names(), vec!["Task"]);
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
