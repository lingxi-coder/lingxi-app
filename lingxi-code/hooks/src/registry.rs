//! Hook registry — indexes hook definitions by source and dispatches matches
//! to the executor in priority order.

use crate::definition::{HookDefinition, HookSource};
use crate::events::HookEvent;
use crate::matcher::{matches_if_condition, matches_pattern};
use protocol::{AgentId, HookId, PluginId, SessionId};
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
    /// Stable project root injected into Command hooks as `LINGXI_PROJECT_DIR`
    /// (B2). claude-code derives this from `getProjectRoot()` — the real repo
    /// root, deliberately *not* updated when entering a git worktree — so a
    /// hook script's `$LINGXI_PROJECT_DIR` always resolves to the repo root
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
    /// Active reasoning-effort level for the current turn, spliced into the
    /// base hook input shape as `effort: { level }` (claude-code
    /// `createBaseHookInput`, minified `vd`). `None` — the additive default —
    /// when the firing scope has no effort signal, which is faithful for
    /// session-lifecycle hooks and models that do not support the effort
    /// parameter (claude-code's `Lw(model)` gate): in both cases the binary
    /// omits the `effort` key entirely. The orchestrator populates this only
    /// for tool-use-context hooks on effort-capable models. Additive default.
    pub effort: Option<crate::hook_payload::EffortLevel>,
    /// Controlling-terminal width/height at dispatch, injected into Command
    /// hooks as `COLUMNS`/`LINES` (#43). claude-code reads
    /// `{columns,rows}=process.stdout` and sets `if(L)P.COLUMNS=String(L)` /
    /// `if(D)P.LINES=String(D)` (BIN off 205727903) — i.e. each is set ONLY when
    /// truthy (non-zero / present). The hooks crate has no TTY of its own, so the
    /// composition root passes the engine's `process.stdout` size here (`None`
    /// for headless / non-TTY hosts, matching `process.stdout.columns` being
    /// `undefined`); the Command arm then sets the env var only when `Some(n)` and
    /// `n != 0` (the binary's falsy guard). Additive default `None`.
    pub terminal_columns: Option<u16>,
    /// Controlling-terminal height — see [`Self::terminal_columns`]. Injected as
    /// the `LINES` env var (#43). Additive default `None`.
    pub terminal_rows: Option<u16>,
    /// Current session title at the moment of dispatch, threaded into
    /// `UserPromptSubmit` and `SessionStart` payloads as `session_title`
    /// (binary-confirmed at BIN off 201745825). `None` when no title is
    /// available or not applicable (most event types). Additive default `None`.
    pub session_title: Option<String>,
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
    /// Frontmatter hooks scoped to the agent (claude `addSessionHook` keyed on
    /// the agent id, registerFrontmatterHooks.ts). Registered by
    /// [`Self::register_agent_hooks`] when a subagent starts and removed wholesale
    /// by [`Self::clear_agent_hooks`] when it ends (claude `clearSessionHooks`,
    /// runAgent.ts finally). Iterated by [`Self::match_event`] / [`Self::has_hooks_for`]
    /// so an agent's frontmatter hooks actually fire while it runs.
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

    /// Register an agent's frontmatter hooks, scoped to `agent_id` so they fire
    /// only while that subagent runs and can be cleared wholesale on exit.
    ///
    /// Port of claude-code `registerFrontmatterHooks(setAppState, agentId, hooks,
    /// …, isAgent)` (registerFrontmatterHooks.ts): when `is_agent` is `true`,
    /// every `Stop` subscription on a hook is retargeted to `SubagentStop` —
    /// because a subagent's loop end fires `SubagentStop`, not `Stop` (claude
    /// `executeStopHooks` uses `SubagentStop` when called with an `agentId`). The
    /// retarget rewrites the hook's `events` in place (a hook may subscribe to
    /// multiple events; only the `Stop` entry is rewritten, and de-duplicated if
    /// `SubagentStop` is already present). `is_agent == false` registers the
    /// hooks verbatim (the skill-frontmatter path, scoped to a session id).
    pub fn register_agent_hooks(
        &mut self,
        agent_id: AgentId,
        hooks: &[HookDefinition],
        is_agent: bool,
    ) {
        if hooks.is_empty() {
            return;
        }
        let bucket = self.frontmatter.entry(agent_id).or_default();
        for hook in hooks {
            let mut hook = hook.clone();
            if is_agent {
                for ev in &mut hook.events {
                    if *ev == crate::events::HookEventType::Stop {
                        *ev = crate::events::HookEventType::SubagentStop;
                    }
                }
                // De-dup a now-doubled `SubagentStop` (a hook that subscribed to
                // BOTH Stop and SubagentStop would otherwise list it twice).
                hook.events.dedup();
            }
            bucket.push(hook);
        }
    }

    /// Remove every frontmatter hook scoped to `agent_id` (claude
    /// `clearSessionHooks(rootSetAppState, agentId)`, runAgent.ts finally).
    /// Returns the number of hooks dropped.
    pub fn clear_agent_hooks(&mut self, agent_id: AgentId) -> usize {
        self.frontmatter
            .remove(&agent_id)
            .map_or(0, |hooks| hooks.len())
    }

    /// Remove the hook with `hook_id` from whichever bucket it lives in,
    /// returning `true` when a hook was actually dropped.
    ///
    /// This is the RUNTIME half of the claude-code `once` field
    /// (`registerSkillHooks.ts:35-36`, `utils/hooks.ts:2918-2919`): a hook
    /// declared `once: true` is removed from its source bucket after it runs
    /// with a *success* outcome, so it never fires again. The executor calls
    /// this only on success — an erroring `once` hook is left in place (it may
    /// succeed on a later dispatch), exactly mirroring TS's
    /// `result.outcome === 'success'` guard around `onHookSuccess`.
    ///
    /// Scans every source bucket plus the plugin and front-matter indexes so a
    /// `once` hook is removed wherever it was registered. `HookId`s are unique
    /// per definition, so at most one entry is dropped.
    pub fn remove_once_hook(&mut self, hook_id: HookId) -> bool {
        for hooks in self.sources.values_mut() {
            if let Some(pos) = hooks.iter().position(|h| h.id == hook_id) {
                hooks.remove(pos);
                return true;
            }
        }
        for hooks in self.plugin.values_mut() {
            if let Some(pos) = hooks.iter().position(|h| h.id == hook_id) {
                hooks.remove(pos);
                return true;
            }
        }
        for hooks in self.frontmatter.values_mut() {
            if let Some(pos) = hooks.iter().position(|h| h.id == hook_id) {
                hooks.remove(pos);
                return true;
            }
        }
        false
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
    /// The `if`-condition rule-content matcher (permission rules like
    /// `"Bash(git push:*)"`) is now ALSO enforced (B3 second half): a hook with
    /// an `if`-condition fires only when [`crate::matcher::matches_if_condition`]
    /// also passes for the event's tool name + tool input. This mirrors
    /// claude-code applying the tool-name `matcher` filter
    /// (`utils/hooks.ts:1681-1685`) and THEN the `if`-condition filter
    /// (`utils/hooks.ts:1808-1850`) — both must pass. The `if`-condition is only
    /// evaluable for the four tool events `prepareIfConditionMatcher`
    /// (`utils/hooks.ts:1390-1401`) accepts (`PreToolUse`, `PostToolUse`,
    /// `PostToolUseFailure`, `PermissionRequest`); on any other event a hook
    /// that DECLARES an `if`-condition is dropped (TS `ifMatcher` is `undefined`
    /// → the filter returns `false`). Following TS, the `if`-condition is applied
    /// only to externally-executed hook kinds (Command / Http / Agent / Prompt);
    /// in-process [`crate::HookExecutor::Builtin`] hooks (the analogue of TS
    /// `callback` / `function` hooks) ignore it.
    #[must_use]
    pub fn match_event(&self, event: &HookEvent, _ctx: &HookContext) -> Vec<&HookDefinition> {
        let et = event.event_type();
        let match_query = Self::match_query_for(event);
        let if_target = Self::if_match_target(event);
        let keep = |h: &&HookDefinition| -> bool {
            if !h.events.contains(&et) {
                return false;
            }
            // TS `getMatchingHooks`: only apply the tool-name matcher filter when
            // the event yields a `matchQuery`; otherwise every subscribed hook
            // passes. A hook with no matcher always passes.
            let tool_name_ok = match (&match_query, h.matcher()) {
                (Some(query), Some(matcher)) => matches_pattern(query, matcher),
                _ => true,
            };
            if !tool_name_ok {
                return false;
            }
            // TS `ifFilteredHooks` (utils/hooks.ts:1808-1850): the `if`-condition
            // gates only externally-executed hook kinds; Builtin (callback /
            // function) hooks bypass it. A hook without an `if`-condition passes.
            if let Some(if_cond) = h.if_pattern() {
                if Self::if_condition_applies(h) {
                    match &if_target {
                        // if-evaluable event → both tool-name + content must match.
                        Some((tool_name, tool_input)) => {
                            if !matches_if_condition(if_cond, tool_name, tool_input) {
                                return false;
                            }
                        }
                        // Non-tool event with an `if`-condition → cannot be
                        // evaluated → drop (TS `if (!ifMatcher) return false`).
                        None => return false,
                    }
                }
            }
            true
        };
        let mut matched: Vec<&HookDefinition> =
            self.sources.values().flatten().filter(keep).collect();
        for hooks in self.plugin.values() {
            matched.extend(hooks.iter().filter(keep));
        }
        // Agent-scoped frontmatter hooks (registered via `register_agent_hooks`
        // for the lifetime of a running subagent) fire alongside source/plugin
        // hooks. claude scopes these by agent id, but matching here is over the
        // event/matcher only — the registration lifetime (register on start,
        // clear on stop) is what bounds them to the right agent.
        for hooks in self.frontmatter.values() {
            matched.extend(hooks.iter().filter(keep));
        }
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }

    /// Like [`Self::match_event`] but restricted to the frontmatter hooks scoped
    /// to a SINGLE `agent_id` (source / plugin / other-agent buckets excluded).
    ///
    /// Used by the child runner to fire `SubagentStop` against ONLY this agent's
    /// own frontmatter Stop→SubagentStop hooks (claude `runAgent` fires the
    /// subagent's stop hooks inside the child, runAgent.ts), without re-firing
    /// session / plugin `SubagentStop` hooks that the orchestrator-side
    /// chokepoint already covers. Returns `[]` when the agent registered no
    /// frontmatter hooks — a strict no-op for the common (no-frontmatter-hook)
    /// path so the child's behavior is byte-identical to legacy there.
    #[must_use]
    pub fn match_event_agent_scoped(
        &self,
        event: &HookEvent,
        agent_id: AgentId,
    ) -> Vec<&HookDefinition> {
        let Some(bucket) = self.frontmatter.get(&agent_id) else {
            return Vec::new();
        };
        let et = event.event_type();
        let match_query = Self::match_query_for(event);
        let if_target = Self::if_match_target(event);
        let keep = |h: &&HookDefinition| -> bool {
            if !h.events.contains(&et) {
                return false;
            }
            let tool_name_ok = match (&match_query, h.matcher()) {
                (Some(query), Some(matcher)) => matches_pattern(query, matcher),
                _ => true,
            };
            if !tool_name_ok {
                return false;
            }
            if let Some(if_cond) = h.if_pattern() {
                if Self::if_condition_applies(h) {
                    match &if_target {
                        Some((tool_name, tool_input)) => {
                            if !matches_if_condition(if_cond, tool_name, tool_input) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            }
            true
        };
        let mut matched: Vec<&HookDefinition> = bucket.iter().filter(keep).collect();
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }

    /// Like [`Self::match_event`] but with the frontmatter bucket scoped to
    /// `exclude_agent_id` OMITTED (source / plugin / every OTHER agent's
    /// frontmatter bucket still match).
    ///
    /// This is the orchestrator-chokepoint twin of [`Self::match_event_agent_scoped`]:
    /// the child runner fires a subagent's OWN frontmatter `Stop`→`SubagentStop`
    /// hooks in-child (agent-scoped, claude `runAgent`), so the chokepoint must
    /// fire the COMPLEMENT — session / plugin `SubagentStop` hooks — WITHOUT
    /// re-firing the child's frontmatter ones (which the runner already covered).
    /// Excluding by id makes that deterministic regardless of whether
    /// `clear_agent_hooks` has run yet (the runner clears the bucket after the
    /// terminal event, which races the chokepoint fire). When the excluded agent
    /// registered no frontmatter hooks (the common case, and every
    /// `FakeAgentTool` fixture), this is byte-identical to [`Self::match_event`].
    #[must_use]
    pub fn match_event_excluding_agent(
        &self,
        event: &HookEvent,
        exclude_agent_id: AgentId,
    ) -> Vec<&HookDefinition> {
        let et = event.event_type();
        let match_query = Self::match_query_for(event);
        let if_target = Self::if_match_target(event);
        let keep = |h: &&HookDefinition| -> bool {
            if !h.events.contains(&et) {
                return false;
            }
            let tool_name_ok = match (&match_query, h.matcher()) {
                (Some(query), Some(matcher)) => matches_pattern(query, matcher),
                _ => true,
            };
            if !tool_name_ok {
                return false;
            }
            if let Some(if_cond) = h.if_pattern() {
                if Self::if_condition_applies(h) {
                    match &if_target {
                        Some((tool_name, tool_input)) => {
                            if !matches_if_condition(if_cond, tool_name, tool_input) {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
            }
            true
        };
        let mut matched: Vec<&HookDefinition> =
            self.sources.values().flatten().filter(keep).collect();
        for hooks in self.plugin.values() {
            matched.extend(hooks.iter().filter(keep));
        }
        // Every frontmatter bucket EXCEPT the excluded agent's own (the runner
        // fires that one in-child, agent-scoped).
        for (aid, hooks) in &self.frontmatter {
            if *aid == exclude_agent_id {
                continue;
            }
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

    /// The `(tool_name, tool_input)` an `if`-condition is evaluated against, or
    /// `None` when the event is not one `prepareIfConditionMatcher`
    /// (`utils/hooks.ts:1390-1401`) accepts.
    ///
    /// Faithful to that guard: ONLY `PreToolUse`, `PostToolUse`,
    /// `PostToolUseFailure`, and `PermissionRequest` yield a matcher. Notably
    /// `PermissionDenied` — which DOES produce a tool-name `matchQuery`
    /// ([`Self::match_query_for`]) — is deliberately excluded here (it also
    /// carries no `tool_input`), so an `if`-conditioned hook subscribed to
    /// `PermissionDenied` is dropped, exactly as in TS.
    fn if_match_target(event: &HookEvent) -> Option<(&str, &serde_json::Value)> {
        match event {
            HookEvent::PreToolUse {
                tool_name,
                tool_input,
                ..
            }
            | HookEvent::PostToolUse {
                tool_name,
                tool_input,
                ..
            }
            | HookEvent::PostToolUseFailure {
                tool_name,
                tool_input,
                ..
            }
            | HookEvent::PermissionRequest {
                tool_name,
                tool_input,
                ..
            } => Some((tool_name.as_str(), tool_input)),
            _ => None,
        }
    }

    /// Whether the `if`-condition filter applies to this hook's executor kind.
    ///
    /// TS gates the `if`-condition on `command` / `prompt` / `agent` / `http`
    /// hooks only (`utils/hooks.ts:1824-1832`); `callback` / `function` hooks
    /// bypass it. [`crate::HookExecutor::Builtin`] is the in-process analogue of
    /// the latter, so it is the sole exempt arm.
    fn if_condition_applies(hook: &HookDefinition) -> bool {
        !matches!(hook.executor, crate::definition::HookExecutor::Builtin { .. })
    }

    /// Whether ANY registered hook (across every bucket) subscribes to
    /// `event_type`, ignoring per-hook matchers.
    ///
    /// This is the cheap *gate* check (no `HookContext`, no matcher
    /// evaluation, no payload) used to decide whether a best-effort lifecycle
    /// fire is worth arming at all — mirroring how the `engine-desktop`
    /// settings watcher only arms the `ConfigChange` fire path when a
    /// subscriber exists. The idle-prompt timer in the CLI repl consults this
    /// (via `ConversationOrchestrator::has_notification_hook`) so it never
    /// arms a useless timer when no `Notification` hook is registered. A
    /// `true` here does NOT guarantee a hook will run for a *specific* event
    /// (a declared matcher may still filter it out at `execute` time); it only
    /// reports event-type subscription, which is exactly what the gate needs.
    #[must_use]
    pub fn has_hooks_for(&self, event_type: &crate::events::HookEventType) -> bool {
        let subscribed = |h: &&HookDefinition| h.events.contains(event_type);
        self.sources.values().flatten().any(|h| subscribed(&h))
            || self.plugin.values().flatten().any(|h| subscribed(&h))
            || self.frontmatter.values().flatten().any(|h| subscribed(&h))
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
    fn remove_once_hook_drops_from_source_bucket_and_reports() {
        let mut r = HookRegistry::new();
        let keep = hk("keep", HookEventType::PostToolUse, HookSource::User);
        let drop = hk("drop", HookEventType::PostToolUse, HookSource::User);
        let drop_id = drop.id;
        let absent = HookId::new();
        r.register(keep);
        r.register(drop);

        // Removing a registered hook reports `true` and leaves only the other.
        assert!(r.remove_once_hook(drop_id), "registered hook is removed");
        let names: Vec<&str> = r.all_hooks().iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["keep"]);

        // Removing again (or removing an unknown id) reports `false`, no panic.
        assert!(!r.remove_once_hook(drop_id), "second removal is a no-op");
        assert!(!r.remove_once_hook(absent), "unknown id is a no-op");
    }

    #[test]
    fn remove_once_hook_drops_from_plugin_bucket() {
        let mut r = HookRegistry::new();
        let h = hk("plugin-once", HookEventType::PreToolUse, HookSource::Plugin);
        let id = h.id;
        r.register_plugin_hooks(protocol::PluginId::new(), vec![h]);
        assert!(r.remove_once_hook(id));
        assert!(r.all_hooks().is_empty());
    }

    #[test]
    fn register_agent_hooks_retargets_stop_to_subagent_stop() {
        // G4: isAgent=true converts a Stop subscription to SubagentStop (claude
        // registerFrontmatterHooks isAgent=true).
        let mut r = HookRegistry::new();
        let agent = AgentId::new();
        let stop_hook = hk("agent-stop", HookEventType::Stop, HookSource::FrontMatter);
        r.register_agent_hooks(agent, &[stop_hook], true);
        // It fires on SubagentStop now, NOT Stop.
        let on_subagent_stop = r.match_event(
            &HookEvent::SubagentStop {
                agent_id: agent,
                status: "completed".into(),
            },
            &HookContext::default(),
        );
        assert_eq!(on_subagent_stop.len(), 1, "Stop retargeted to SubagentStop");
        let on_stop = r.match_event(
            &HookEvent::Stop {
                reason: "x".into(),
            },
            &HookContext::default(),
        );
        assert!(on_stop.is_empty(), "no longer fires on plain Stop");
    }

    #[test]
    fn register_agent_hooks_non_agent_keeps_stop() {
        // is_agent=false (the skill-frontmatter path) registers verbatim.
        let mut r = HookRegistry::new();
        let agent = AgentId::new();
        r.register_agent_hooks(
            agent,
            &[hk("skill-stop", HookEventType::Stop, HookSource::FrontMatter)],
            false,
        );
        let on_stop = r.match_event(
            &HookEvent::Stop {
                reason: "x".into(),
            },
            &HookContext::default(),
        );
        assert_eq!(on_stop.len(), 1, "Stop preserved when is_agent=false");
    }

    #[test]
    fn clear_agent_hooks_removes_only_that_agents_hooks() {
        let mut r = HookRegistry::new();
        let a = AgentId::new();
        let b = AgentId::new();
        r.register_agent_hooks(
            a,
            &[hk("a1", HookEventType::PreToolUse, HookSource::FrontMatter)],
            true,
        );
        r.register_agent_hooks(
            b,
            &[hk("b1", HookEventType::PreToolUse, HookSource::FrontMatter)],
            true,
        );
        assert_eq!(r.all_hooks().len(), 2);
        // Clearing `a` drops only a1.
        assert_eq!(r.clear_agent_hooks(a), 1);
        let names: Vec<&str> = r.all_hooks().iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["b1"]);
        // Clearing an unknown agent is a no-op.
        assert_eq!(r.clear_agent_hooks(AgentId::new()), 0);
    }

    #[test]
    fn agent_scoped_hooks_fire_via_match_event() {
        // The frontmatter bucket is iterated by match_event so agent hooks fire.
        let mut r = HookRegistry::new();
        let agent = AgentId::new();
        r.register_agent_hooks(
            agent,
            &[hk("fm", HookEventType::PreToolUse, HookSource::FrontMatter)],
            true,
        );
        let matched = r.match_event(
            &HookEvent::PreToolUse {
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({}),
                tool_use_id: protocol::ToolUseId::new(),
            },
            &HookContext::default(),
        );
        assert_eq!(matched.len(), 1, "frontmatter hook fires");
    }

    #[test]
    fn match_event_agent_scoped_only_returns_that_agents_frontmatter() {
        // #9: the runner fires SubagentStop scoped to ITS agent's bucket only —
        // source / plugin / other-agent SubagentStop hooks are excluded so the
        // orchestrator chokepoint isn't double-fired.
        let mut r = HookRegistry::new();
        let a = AgentId::new();
        let b = AgentId::new();
        // a's frontmatter Stop→SubagentStop (the hook we want to fire).
        r.register_agent_hooks(
            a,
            &[hk("a-stop", HookEventType::Stop, HookSource::FrontMatter)],
            true,
        );
        // b's frontmatter SubagentStop — must NOT match for agent a.
        r.register_agent_hooks(
            b,
            &[hk("b-stop", HookEventType::Stop, HookSource::FrontMatter)],
            true,
        );
        // A SESSION-level SubagentStop hook — must NOT match the agent-scoped call.
        r.register(hk(
            "session-stop",
            HookEventType::SubagentStop,
            HookSource::User,
        ));

        let ev = HookEvent::SubagentStop {
            agent_id: a,
            status: "completed".into(),
        };
        let scoped = r.match_event_agent_scoped(&ev, a);
        let names: Vec<&str> = scoped.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a-stop"],
            "only agent a's own frontmatter SubagentStop fires: {names:?}"
        );

        // The general match_event DOES see the session + agent-a hook (3 total
        // here: a-stop, b-stop also fire on SubagentStop event-type, session-stop),
        // confirming the scoped variant is strictly narrower.
        let all = r.match_event(&ev, &HookContext::default());
        assert!(
            all.len() >= scoped.len(),
            "agent-scoped match is a subset of the general match"
        );
    }

    #[test]
    fn match_event_excluding_agent_omits_only_the_excluded_frontmatter() {
        // R7: the chokepoint SubagentStop fires the COMPLEMENT of the runner's
        // agent-scoped fire — session/plugin + every OTHER agent's frontmatter,
        // but NOT the excluded child's own frontmatter SubagentStop.
        let mut r = HookRegistry::new();
        let a = AgentId::new();
        let b = AgentId::new();
        // a's frontmatter Stop→SubagentStop — MUST be excluded for agent a.
        r.register_agent_hooks(
            a,
            &[hk("a-stop", HookEventType::Stop, HookSource::FrontMatter)],
            true,
        );
        // b's frontmatter Stop→SubagentStop — a DIFFERENT agent, still fires.
        r.register_agent_hooks(
            b,
            &[hk("b-stop", HookEventType::Stop, HookSource::FrontMatter)],
            true,
        );
        // A SESSION-level SubagentStop hook — always fires (the chokepoint owns it).
        r.register(hk(
            "session-stop",
            HookEventType::SubagentStop,
            HookSource::User,
        ));

        let ev = HookEvent::SubagentStop {
            agent_id: a,
            status: "completed".into(),
        };
        let mut names: Vec<&str> = r
            .match_event_excluding_agent(&ev, a)
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["b-stop", "session-stop"],
            "excludes ONLY agent a's frontmatter; session + agent b still fire: {names:?}"
        );

        // Sanity: the general match still sees all three (a included).
        let all = r.match_event(&ev, &HookContext::default());
        assert_eq!(all.len(), 3, "general match sees a-stop too");
    }

    #[test]
    fn match_event_excluding_agent_equals_match_event_when_excluded_agent_has_no_frontmatter() {
        // FakeAgentTool fixtures: the fake child id has NO frontmatter bucket, so
        // excluding it is byte-identical to the general match (fixtures green).
        let mut r = HookRegistry::new();
        r.register(hk(
            "session-stop",
            HookEventType::SubagentStop,
            HookSource::User,
        ));
        let unknown = AgentId::new();
        let ev = HookEvent::SubagentStop {
            agent_id: unknown,
            status: "completed".into(),
        };
        let excluded: Vec<&str> = r
            .match_event_excluding_agent(&ev, unknown)
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        let general: Vec<&str> = r
            .match_event(&ev, &HookContext::default())
            .iter()
            .map(|h| h.name.as_str())
            .collect();
        assert_eq!(excluded, general, "no excluded frontmatter ⇒ identical to general");
        assert_eq!(excluded, vec!["session-stop"]);
    }

    #[test]
    fn match_event_agent_scoped_empty_for_unknown_agent() {
        let r = HookRegistry::new();
        let scoped = r.match_event_agent_scoped(
            &HookEvent::SubagentStop {
                agent_id: AgentId::new(),
                status: "completed".into(),
            },
            AgentId::new(),
        );
        assert!(scoped.is_empty(), "no frontmatter hooks ⇒ empty");
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
                if_pattern: None,
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

    /// A hook carrying an `if`-condition (and optional tool-name `matcher`),
    /// executed by a Command arm so the `if`-condition filter actually applies
    /// (Builtin hooks bypass it, mirroring TS `callback`/`function`).
    fn hook_with_if(
        name: &str,
        event: HookEventType,
        matcher: Option<&str>,
        if_cond: &str,
    ) -> HookDefinition {
        HookDefinition {
            id: HookId::new(),
            name: name.into(),
            events: vec![event],
            if_condition: Some(HookCondition {
                pattern: matcher.unwrap_or_default().into(),
                match_tool_name: matcher.is_some(),
                match_input: true,
                if_pattern: Some(if_cond.into()),
            }),
            executor: HookExecutor::Command {
                command: "./noop.sh".into(),
                args: vec![],
                env: std::collections::HashMap::new(),
                cwd: None,
            },
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        }
    }

    fn pre_tool_use_in(tool: &str, input: serde_json::Value) -> HookEvent {
        HookEvent::PreToolUse {
            tool_name: tool.into(),
            tool_input: input,
            tool_use_id: ToolUseId::new(),
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

    // ── `if`-condition wiring (B3 second half) ────────────────────────────────

    #[test]
    fn if_condition_fires_only_on_matching_tool_input() {
        // `if: "Bash(git push:*)"` fires on `git push …`, not on `git status`.
        let mut reg = HookRegistry::new();
        reg.register(hook_with_if(
            "guard",
            HookEventType::PreToolUse,
            Some("Bash"),
            "Bash(git push:*)",
        ));
        let pushing = pre_tool_use_in("Bash", serde_json::json!({ "command": "git push origin x" }));
        assert_eq!(matched_names(&reg, &pushing), vec!["guard"]);
        let status = pre_tool_use_in("Bash", serde_json::json!({ "command": "git status" }));
        assert!(matched_names(&reg, &status).is_empty());
    }

    #[test]
    fn no_if_condition_fires_on_tool_name_match_alone() {
        // Absent `if` → tool-name match alone (prior behavior preserved).
        let mut reg = HookRegistry::new();
        reg.register(hook_with("plain", HookEventType::PreToolUse, Some("Bash")));
        let any = pre_tool_use_in("Bash", serde_json::json!({ "command": "rm -rf /" }));
        assert_eq!(matched_names(&reg, &any), vec!["plain"]);
    }

    #[test]
    fn both_tool_name_matcher_and_if_must_pass() {
        // matcher "Bash" AND if "Bash(git push:*)" — both gates enforced.
        let mut reg = HookRegistry::new();
        reg.register(hook_with_if(
            "both",
            HookEventType::PreToolUse,
            Some("Bash"),
            "Bash(git push:*)",
        ));
        // tool-name matcher fails (Write != Bash) → dropped even though the
        // event isn't even a git push.
        let on_write = pre_tool_use_in("Write", serde_json::json!({ "file_path": "/a" }));
        assert!(matched_names(&reg, &on_write).is_empty());
        // tool-name matcher passes but the `if` content fails → dropped.
        let wrong_cmd = pre_tool_use_in("Bash", serde_json::json!({ "command": "git pull" }));
        assert!(matched_names(&reg, &wrong_cmd).is_empty());
        // both pass → fires.
        let ok = pre_tool_use_in("Bash", serde_json::json!({ "command": "git push" }));
        assert_eq!(matched_names(&reg, &ok), vec!["both"]);
    }

    #[test]
    fn malformed_if_condition_does_not_fire() {
        // Unbalanced paren → parses as a bare tool name that won't equal "Bash".
        let mut reg = HookRegistry::new();
        reg.register(hook_with_if(
            "bad",
            HookEventType::PreToolUse,
            None,
            "Bash(git push:*",
        ));
        let ev = pre_tool_use_in("Bash", serde_json::json!({ "command": "git push" }));
        assert!(matched_names(&reg, &ev).is_empty());
    }

    #[test]
    fn if_condition_hook_dropped_on_non_if_evaluable_event() {
        // PermissionDenied yields a tool-name matchQuery but is NOT in the
        // prepareIfConditionMatcher set → an `if`-conditioned hook is dropped.
        let mut reg = HookRegistry::new();
        reg.register(hook_with_if(
            "denied-guard",
            HookEventType::PermissionDenied,
            None,
            "Bash(git push:*)",
        ));
        let ev = HookEvent::PermissionDenied {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({ "command": "git push" }),
            tool_use_id: ToolUseId::new(),
            reason: "no".into(),
        };
        assert!(matched_names(&reg, &ev).is_empty());
    }

    #[test]
    fn builtin_hook_bypasses_if_condition() {
        // A Builtin (callback/function analogue) hook with an `if` ignores it —
        // TS applies the `if` filter only to command/prompt/agent/http hooks.
        let mut reg = HookRegistry::new();
        let mut h = hook_with("builtin", HookEventType::PreToolUse, Some("Bash"));
        // Attach an `if` that would NOT match the event, yet a Builtin bypasses it.
        if let Some(c) = h.if_condition.as_mut() {
            c.if_pattern = Some("Bash(git push:*)".into());
            c.match_input = true;
        }
        reg.register(h);
        let non_matching = pre_tool_use_in("Bash", serde_json::json!({ "command": "git status" }));
        assert_eq!(matched_names(&reg, &non_matching), vec!["builtin"]);
    }
}
