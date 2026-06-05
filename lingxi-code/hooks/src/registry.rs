//! Hook registry — indexes hook definitions by source and dispatches matches
//! to the executor in priority order.

use crate::definition::{HookDefinition, HookSource};
use crate::events::HookEvent;
use crate::matcher::matches_pattern;
use protocol::{AgentId, PluginId, SessionId};
use std::collections::HashMap;
use std::path::PathBuf;
use traits::SubagentInheritance;

/// Per-call context handed to hooks alongside the event payload.
///
/// M1.4 shape carried only `session_id`, `agent_id`, `cwd`. M5-06 extends
/// it with `transcript_path`, `permission_mode`, `agent_type`, and
/// `inherit` (the subagent capability bundle used by the Agent-arm
/// executor). All new fields are `Option` (or default-empty `PathBuf`) so
/// existing call sites adopt them with `..Default::default()` rather than
/// having to populate everything.
#[derive(Clone, Default)]
pub struct HookContext {
    /// Session this event belongs to.
    pub session_id: SessionId,
    /// Active agent at the moment of dispatch (`None` for engine-global
    /// events like `Setup`).
    pub agent_id: Option<AgentId>,
    /// Engine cwd at the moment of dispatch.
    pub cwd: PathBuf,
    /// Path to the on-disk transcript file backing this session. Used by
    /// HTTP / command hooks to splice into their payload (`transcript_path`
    /// per claude-code `BaseHookInputSchema`). M5-06.
    pub transcript_path: PathBuf,
    /// Current permission mode (`"default" | "plan" | "acceptEdits" | …`).
    /// `None` when not applicable (e.g. engine-global hooks). M5-06.
    pub permission_mode: Option<String>,
    /// Subagent role identifier when the dispatch happened inside a
    /// subagent context (`general-purpose`, `code-reviewer`, …). M5-06.
    pub agent_type: Option<String>,
    /// Subagent capability bundle the Agent-arm executor inherits when it
    /// spawns. `None` for hooks that never reach the Agent arm. M5-06.
    pub inherit: Option<SubagentInheritance>,
    /// Stable project root injected into Command hooks as `CLAUDE_PROJECT_DIR`
    /// (B2). claude-code derives this from `getProjectRoot()` — the real repo
    /// root, deliberately *not* updated when entering a git worktree — so a
    /// hook script's `$CLAUDE_PROJECT_DIR` always resolves to the repo root
    /// (`utils/hooks.ts:813-816`). When `None`, the Command arm falls back to
    /// `cwd`, the faithful approximation until the orchestrator populates a
    /// distinct project-root concept (that wiring lives in `turn_loop.rs`,
    /// out of this crate's scope). Additive / `..Default::default()`-compatible.
    pub project_dir: Option<PathBuf>,
    /// `true` when this dispatch is a *re-entry* of the Stop hook after a
    /// previous Stop hook returned a blocking error and the agent loop
    /// continued one extra turn (B4). claude-code threads `stop_hook_active`
    /// into the Stop hook payload (`query.ts:1300`) and uses it as the
    /// infinite-loop guard: when already `true`, a fresh Stop block must NOT
    /// loop again (`query.ts:1297` rationale). Additive default `false`.
    pub stop_hook_active: bool,
    /// The text of the final assistant message of the turn, spliced into the
    /// `Stop` / `UserPromptSubmit` payload (claude-code `BaseHookInputSchema`
    /// surfaces the transcript; B1 payloads consume this). `None` when no
    /// assistant message is available (e.g. a `UserPromptSubmit` fired at
    /// prompt ingress before any model reply). Additive default.
    pub last_assistant_message: Option<String>,
    /// Path to the agent-scoped transcript file when the dispatch happened
    /// inside a subagent context. Distinct from [`Self::transcript_path`]
    /// (the session transcript); `None` outside a subagent. Additive default.
    pub agent_transcript_path: Option<PathBuf>,
}

/// In-memory registry of hook definitions, sharded by their declared source.
///
/// Plugin-supplied hooks are tracked separately so they can be wholesale
/// removed by `unregister_plugin` when a plugin unloads. Front-matter hooks
/// are indexed by their owning agent so the registry can scope them to that
/// agent's lifetime — that index is plumbed in Plan 09 alongside the agent
/// loader.
pub struct HookRegistry {
    sources: HashMap<HookSource, Vec<HookDefinition>>,
    plugin: HashMap<PluginId, Vec<HookDefinition>>,
    #[allow(dead_code)] // Wired up in Plan 09 with the agent front-matter loader.
    frontmatter: HashMap<AgentId, Vec<HookDefinition>>,
}

impl HookRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            plugin: HashMap::new(),
            frontmatter: HashMap::new(),
        }
    }

    /// Register a hook under its declared source.
    pub fn register(&mut self, hook: HookDefinition) {
        self.sources.entry(hook.source).or_default().push(hook);
    }

    /// Register a batch of hooks owned by `plugin_id`. Replaces any previous
    /// registration under that plugin.
    pub fn register_plugin_hooks(&mut self, plugin_id: PluginId, hooks: Vec<HookDefinition>) {
        self.plugin.insert(plugin_id, hooks);
    }

    /// Drop every hook owned by `plugin_id` (used when a plugin unloads).
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        self.plugin.remove(plugin_id);
    }

    /// Return every hook subscribed to `event`'s type that ALSO satisfies its
    /// declared matcher, sorted in execution order (highest priority first).
    ///
    /// Mirrors claude-code `getMatchingHooks` (`utils/hooks.ts:1603-1703`):
    /// first the event-type subscription filter, then the per-hook matcher
    /// filter (`hooks.ts:1681-1685`):
    ///
    /// ```ts
    /// const filteredMatchers = matchQuery
    ///   ? hookMatchers.filter(m => !m.matcher || matchesPattern(matchQuery, m.matcher))
    ///   : hookMatchers
    /// ```
    ///
    /// So when this event has a `match_query` (B3, tool-name events), a hook is
    /// dropped only if it DECLARES a matcher that does not match. A hook with
    /// no matcher (`matcher() == None`) — or an empty / `"*"` matcher — always
    /// fires, exactly as before this change. Events with no `match_query`
    /// (e.g. `TaskCompleted`) skip the matcher filter entirely.
    ///
    /// NOTE (B3 `if`-condition gap): the `if`-condition rule-content matcher
    /// (`match_input`-style permission rules like `"Bash(rm:*)"`) is NOT
    /// evaluated here — that half of B3 is blocked on the ported permission-rule
    /// parser. Only the tool-name `matcher` is enforced. Such conditions do not
    /// (yet) gate firing.
    #[must_use]
    pub fn match_event(&self, event: &HookEvent, _ctx: &HookContext) -> Vec<&HookDefinition> {
        let et = event.event_type();
        let match_query = Self::match_query_for(event);
        let keep = |h: &&HookDefinition| -> bool {
            if !h.events.contains(&et) {
                return false;
            }
            // TS `getMatchingHooks`: only apply the matcher filter when the
            // event yields a `matchQuery`; otherwise every subscribed hook
            // passes. A hook with no matcher always passes.
            match (&match_query, h.matcher()) {
                (Some(query), Some(matcher)) => matches_pattern(query, matcher),
                _ => true,
            }
        };
        let mut matched: Vec<&HookDefinition> =
            self.sources.values().flatten().filter(keep).collect();
        for hooks in self.plugin.values() {
            matched.extend(hooks.iter().filter(keep));
        }
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }

    /// Compute the `matchQuery` string for `event`, mirroring the tool-name
    /// arms of the switch in claude-code `getMatchingHooks`
    /// (`utils/hooks.ts:1616-1623`).
    ///
    /// This is the B3 MATCHER half: the query is the tool name for tool-name
    /// events (`PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
    /// `PermissionRequest`, `PermissionDenied`). For every other event TS
    /// derives the query from event-specific fields, but those non-tool match
    /// queries (and the events' payload shapes) are scoped to later batches —
    /// here they return `None`, so the matcher filter is skipped (TS:
    /// `matchQuery ? filter : hookMatchers`) and the subscribed hooks fire
    /// exactly as before. This keeps the change purely additive for non-tool
    /// events while enforcing tool-name matchers faithfully.
    fn match_query_for(event: &HookEvent) -> Option<String> {
        match event {
            HookEvent::PreToolUse { tool_name, .. }
            | HookEvent::PostToolUse { tool_name, .. }
            | HookEvent::PostToolUseFailure { tool_name, .. }
            | HookEvent::PermissionRequest { tool_name, .. }
            | HookEvent::PermissionDenied { tool_name, .. } => Some(tool_name.clone()),
            _ => None,
        }
    }

    /// Snapshot every registered hook across all sources (user / project /
    /// local / managed / plugin / frontmatter / session / skill).
    ///
    /// Used by `OrchestratorHandle::list_hooks` (M6-07) so `/hooks` can
    /// list the registry without exposing the source-sharded internals.
    /// Returned in unspecified order — callers that need stable order
    /// should sort by `name`.
    #[must_use]
    pub fn all_hooks(&self) -> Vec<&HookDefinition> {
        let mut out: Vec<&HookDefinition> = self.sources.values().flatten().collect();
        out.extend(self.plugin.values().flatten());
        out.extend(self.frontmatter.values().flatten());
        out
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod all_hooks_tests {
    use super::*;
    use crate::definition::{HookExecutor, HookSource};
    use crate::events::HookEventType;
    use protocol::HookId;

    fn hk(name: &str, event: HookEventType, source: HookSource) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: name.into(),
            events: vec![event],
            if_condition: None,
            executor: HookExecutor::Builtin {
                handler_id: "noop".into(),
            },
            source,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    #[test]
    fn all_hooks_returns_empty_for_fresh_registry() {
        let r = HookRegistry::new();
        assert!(r.all_hooks().is_empty());
    }

    #[test]
    fn all_hooks_unions_source_and_plugin_buckets() {
        let mut r = HookRegistry::new();
        r.register(hk("user-fmt", HookEventType::PostToolUse, HookSource::User));
        r.register(hk("project-lint", HookEventType::Stop, HookSource::Project));
        r.register_plugin_hooks(
            protocol::PluginId::new(),
            vec![hk(
                "plugin-x",
                HookEventType::PreToolUse,
                HookSource::Plugin,
            )],
        );

        let names: Vec<&str> = r.all_hooks().iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"user-fmt"));
        assert!(names.contains(&"project-lint"));
        assert!(names.contains(&"plugin-x"));
        assert_eq!(names.len(), 3);
    }
}

#[cfg(test)]
mod match_event_matcher_tests {
    //! B3 matcher integration: `match_event` drops hooks whose declared
    //! tool-name matcher does not match the event's tool, while no-matcher
    //! hooks keep firing for any subscribed event.
    use super::*;
    use crate::definition::{HookCondition, HookExecutor, HookSource};
    use crate::events::HookEventType;
    use protocol::{HookId, ToolUseId};

    fn hook_with(name: &str, event: HookEventType, matcher: Option<&str>) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: name.into(),
            events: vec![event],
            if_condition: matcher.map(|m| HookCondition {
                pattern: m.into(),
                match_tool_name: true,
                match_input: false,
            }),
            executor: HookExecutor::Builtin {
                handler_id: "noop".into(),
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    fn pre_tool_use(tool: &str) -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: tool.into(),
            tool_input: serde_json::json!({}),
            tool_use_id: ToolUseId::new(),
        }
    }

    fn matched_names(reg: &HookRegistry, event: &HookEvent) -> Vec<String> {
        reg.match_event(event, &HookContext::default())
            .iter()
            .map(|h| h.name.clone())
            .collect()
    }

    #[test]
    fn write_matcher_hook_does_not_fire_on_bash_event() {
        let mut reg = HookRegistry::new();
        reg.register(hook_with(
            "write-only",
            HookEventType::PreToolUse,
            Some("Write"),
        ));
        // Fires on a Write tool event.
        assert_eq!(
            matched_names(&reg, &pre_tool_use("Write")),
            vec!["write-only"]
        );
        // Dropped on a Bash tool event — the declared matcher does not match.
        assert!(matched_names(&reg, &pre_tool_use("Bash")).is_empty());
    }

    #[test]
    fn no_matcher_hook_fires_on_any_tool_event() {
        let mut reg = HookRegistry::new();
        reg.register(hook_with("always", HookEventType::PreToolUse, None));
        // A hook with no matcher fires regardless of the tool name.
        assert_eq!(matched_names(&reg, &pre_tool_use("Bash")), vec!["always"]);
        assert_eq!(matched_names(&reg, &pre_tool_use("Write")), vec!["always"]);
        assert_eq!(matched_names(&reg, &pre_tool_use("Read")), vec!["always"]);
    }

    #[test]
    fn pipe_matcher_fires_on_any_segment() {
        let mut reg = HookRegistry::new();
        reg.register(hook_with(
            "fmt",
            HookEventType::PreToolUse,
            Some("Write|Edit"),
        ));
        assert_eq!(matched_names(&reg, &pre_tool_use("Write")), vec!["fmt"]);
        assert_eq!(matched_names(&reg, &pre_tool_use("Edit")), vec!["fmt"]);
        assert!(matched_names(&reg, &pre_tool_use("Bash")).is_empty());
    }

    #[test]
    fn star_matcher_fires_on_any_tool() {
        let mut reg = HookRegistry::new();
        reg.register(hook_with("star", HookEventType::PreToolUse, Some("*")));
        assert_eq!(matched_names(&reg, &pre_tool_use("Bash")), vec!["star"]);
        assert_eq!(matched_names(&reg, &pre_tool_use("Write")), vec!["star"]);
    }

    #[test]
    fn regex_matcher_filters_tool_name() {
        let mut reg = HookRegistry::new();
        reg.register(hook_with(
            "bashish",
            HookEventType::PreToolUse,
            Some("^Bash.*"),
        ));
        assert_eq!(matched_names(&reg, &pre_tool_use("Bash")), vec!["bashish"]);
        assert_eq!(
            matched_names(&reg, &pre_tool_use("BashOutput")),
            vec!["bashish"]
        );
        assert!(matched_names(&reg, &pre_tool_use("Write")).is_empty());
    }

    #[test]
    fn matcher_filter_skipped_for_non_tool_event_with_query() {
        // A Stop hook with a (tool-name) matcher still fires on a Stop event,
        // because Stop has no tool match-query (TS leaves matchQuery undefined),
        // so the matcher filter is skipped. This guards the no-regression rule.
        let mut reg = HookRegistry::new();
        reg.register(hook_with("stopper", HookEventType::Stop, Some("Write")));
        let event = HookEvent::Stop {
            reason: "done".into(),
        };
        assert_eq!(matched_names(&reg, &event), vec!["stopper"]);
    }

    #[test]
    fn matcher_filter_applies_to_plugin_hooks_too() {
        let mut reg = HookRegistry::new();
        reg.register_plugin_hooks(
            protocol::PluginId::new(),
            vec![hook_with(
                "plugin-write",
                HookEventType::PreToolUse,
                Some("Write"),
            )],
        );
        assert_eq!(
            matched_names(&reg, &pre_tool_use("Write")),
            vec!["plugin-write"]
        );
        assert!(matched_names(&reg, &pre_tool_use("Bash")).is_empty());
    }
}
