//! Live, cross-crate table of plugin-declared workflow scripts.
//!
//! `plugin.json`'s `workflows` field (a `union([path, path[]])` over
//! directories/`.js` files, or the `workflows/` auto-scan when the field is
//! absent) is parsed by `plugin::discovery` into
//! `PluginComponents::workflows`, but nothing materializes it: a plugin's
//! saved workflow is invisible to the `Workflow` tool's by-name resolver.
//!
//! Oracle (`2.1.251`, the plugin-workflow loader `v()`/`P()` in the bundled
//! JS): every plugin workflow is namespaced `${pluginName}:${meta.name}` —
//! `meta.name` parsed from the script's own `export const meta = {…}` block
//! — and joins the SAME lookup table as built-in / project / user
//! workflows, with precedence **project/user > plugin > built-in** (a
//! project/user file can shadow a plugin workflow or even a built-in; a
//! plugin cannot shadow a built-in or a project/user file). Namespacing
//! means a real collision only happens if a project/user workflow file is
//! itself literally named `<plugin>:<name>.js`.
//!
//! LingXi's built-in/project/user tiers are all addressed by FILENAME, not
//! by parsed script metadata (`workflow::meta_string_value` is used only for
//! display/telemetry, never for lookup) — a wider, pre-existing divergence
//! from the oracle that this module does not attempt to fix. To keep one
//! consistent addressing rule across every tier, a plugin workflow's `name`
//! here is its own `meta.name` when the script's meta block parses,
//! falling back to the file stem — the SAME "parse the component's own
//! declared name" rule `plugin::manager` already applies to output styles
//! and skills, applied to the one component whose metadata happens to be
//! embedded in a script comment rather than a frontmatter block.
//!
//! This type lives in `workflow` (not `plugin`) because `plugin` is not — and
//! must not become — a dependency of `tool-workflow` or `tasks` (the
//! consumers): `apps/cli`/`apps/engine-desktop` are the only crates
//! depending on `plugin` today. `workflow` is the one crate every consumer
//! already shares (`tool-workflow` and `tasks` both depend on it, and
//! `plugin` gains it as a new, acyclic dependency here), so a composition
//! root can construct ONE `Arc<PluginWorkflowRegistry>`, hand it to
//! `plugin::PluginManager::with_plugin_workflows`, and hand the SAME `Arc` to
//! `tool_workflow::WorkflowTool::with_plugin_workflows` /
//! `tasks::handlers::local_workflow::LocalWorkflowHandler::with_plugin_workflows`
//! — without either resolver crate depending on `plugin` itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// One plugin-declared workflow script, keyed by its already-namespaced name.
#[derive(Debug, Clone)]
pub struct PluginWorkflowEntry {
    /// `{plugin}:{name}` — see the module doc for how `{name}` is derived.
    pub name: String,
    /// Absolute path to the workflow's `.js` source.
    pub script_path: PathBuf,
}

/// Shared table of plugin-declared workflow scripts, addressable by name
/// alongside built-in / project / user workflows.
///
/// `register`/`unregister` are keyed by the plugin's own bookkeeping (the
/// caller — `PluginManager` — tracks which names belong to which plugin, the
/// same shape as its existing `plugin_mcp_names` / `plugin_agent_names`
/// maps), so this type itself only ever sees flat name lists.
#[derive(Debug, Default)]
pub struct PluginWorkflowRegistry {
    by_name: RwLock<HashMap<String, PathBuf>>,
}

impl PluginWorkflowRegistry {
    /// A new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or overwrite) a batch of entries — one plugin's full
    /// contribution, called once per `load_plugin`. A name collision across
    /// two plugins is last-write-wins (matching the oracle's `Map.set`
    /// during discovery); the resolution-order guarantee that matters is
    /// project/user over plugin, enforced by the CALLER checking this
    /// registry only after the project/user directories have already missed.
    pub fn register(&self, entries: Vec<PluginWorkflowEntry>) {
        if entries.is_empty() {
            return;
        }
        let mut guard = self.by_name.write().unwrap_or_else(|e| e.into_inner());
        for entry in entries {
            guard.insert(entry.name, entry.script_path);
        }
    }

    /// Remove exactly the named entries (a single plugin's set, tracked by
    /// the caller) — the symmetric counterpart to `register`, called from
    /// `unload_plugin`.
    pub fn unregister(&self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let mut guard = self.by_name.write().unwrap_or_else(|e| e.into_inner());
        for name in names {
            guard.remove(name);
        }
    }

    /// Resolve a namespaced workflow name to its script path.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<PathBuf> {
        let guard = self.by_name.read().unwrap_or_else(|e| e.into_inner());
        guard.get(name).cloned()
    }

    /// All currently registered names, sorted — for the "Available: …" name
    /// listing alongside built-in/project/user names.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let guard = self.by_name.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = guard.keys().cloned().collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_resolve_round_trips() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(vec![PluginWorkflowEntry {
            name: "acme:deploy".to_string(),
            script_path: PathBuf::from("/plugins/acme/workflows/deploy.js"),
        }]);
        assert_eq!(
            registry.resolve("acme:deploy"),
            Some(PathBuf::from("/plugins/acme/workflows/deploy.js"))
        );
        assert_eq!(registry.resolve("acme:missing"), None);
    }

    #[test]
    fn unregister_removes_exactly_the_named_entries() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(vec![
            PluginWorkflowEntry {
                name: "acme:deploy".to_string(),
                script_path: PathBuf::from("/a/deploy.js"),
            },
            PluginWorkflowEntry {
                name: "acme:rollback".to_string(),
                script_path: PathBuf::from("/a/rollback.js"),
            },
            PluginWorkflowEntry {
                name: "other:keep".to_string(),
                script_path: PathBuf::from("/b/keep.js"),
            },
        ]);
        registry.unregister(&["acme:deploy".to_string(), "acme:rollback".to_string()]);
        assert_eq!(registry.resolve("acme:deploy"), None);
        assert_eq!(registry.resolve("acme:rollback"), None);
        assert_eq!(registry.resolve("other:keep"), Some(PathBuf::from("/b/keep.js")));
    }

    #[test]
    fn names_are_sorted() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(vec![
            PluginWorkflowEntry {
                name: "zeta:z".to_string(),
                script_path: PathBuf::from("/z.js"),
            },
            PluginWorkflowEntry {
                name: "alpha:a".to_string(),
                script_path: PathBuf::from("/a.js"),
            },
        ]);
        assert_eq!(registry.names(), vec!["alpha:a".to_string(), "zeta:z".to_string()]);
    }
}
