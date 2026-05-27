//! Hook registry — indexes hook definitions by source and dispatches matches
//! to the executor in priority order.

use crate::definition::{HookDefinition, HookSource};
use crate::events::HookEvent;
use lingxi_protocol::{AgentId, PluginId, SessionId};
use lingxi_traits::SubagentInheritance;
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// per claude-code BaseHookInputSchema). M5-06.
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

    /// Return every hook subscribed to `event`'s type, sorted in execution
    /// order (highest priority first).
    #[must_use]
    pub fn match_event(&self, event: &HookEvent, _ctx: &HookContext) -> Vec<&HookDefinition> {
        let et = event.event_type();
        let mut matched: Vec<&HookDefinition> = self
            .sources
            .values()
            .flatten()
            .filter(|h| h.events.contains(&et))
            .collect();
        for hooks in self.plugin.values() {
            matched.extend(hooks.iter().filter(|h| h.events.contains(&et)));
        }
        matched.sort_by(|a, b| b.priority.cmp(&a.priority));
        matched
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}
