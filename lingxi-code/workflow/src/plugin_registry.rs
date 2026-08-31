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
//! — and joins the SAME lookup table as built-in / project / user workflows.
//!
//! The oracle's precedence is **project/user > plugin > built-in**, and the
//! plugin/built-in half is a REAL shadow, not a no-op. `j()` (@169051841)
//! seeds the map from the built-ins and then overwrites:
//! `let c=wWe(), e=new Map(c.map(k=>[k.name,k])), r=O(m,e); for(let k of r)
//! e.set(k.name,k)`, and the final array is
//! `[...c.filter(k=>!d.has(k.name)), ...u, ...l]` — unshadowed built-ins,
//! then plugins not shadowed by project/user, then project/user. The only
//! thing that stops a plugin record from displacing a same-named built-in is
//! its own script failing to parse (`W(o,s){if(!s||BBn(o.script))return!0;…}`,
//! `O(o,s){return o.filter(t=>W(t,s.get(t.name)))}` @169052203).
//!
//! All model-facing resolvers apply the same saved > plugin > built-in order.
//! Namespacing means a project/user collision happens only when the saved
//! workflow intentionally uses the plugin-qualified name.
//!
//! `LingXi`'s built-in/project/user tiers are all addressed by FILENAME, not
//! by parsed script metadata (`workflow::meta_string_value` is used only for
//! display/telemetry, never for lookup) — a wider, pre-existing divergence
//! from the oracle that this module does not attempt to fix. A PLUGIN
//! workflow, though, is addressed exactly as the oracle addresses it: by its
//! own `meta.name`, with no filename fallback. `v()` (@169045500) drops any
//! file whose meta does not parse (`if("error"in r) return warn(`Plugin
//! workflow ${o} has invalid meta: ${r.error} — skipping`), null`) or which
//! is not a regular file of at most [`MAX_WORKFLOW_SCRIPT_BYTES`], so a
//! shared helper module sitting in `workflows/` never becomes a workflow
//! name. Falling back to the file stem there would put a name in the
//! `Workflow` tool's `Available:` list that then fails `validate_meta` inside
//! the launcher — an accept-then-fail the oracle cannot produce.
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

/// Largest `.js` a plugin workflow may be before the loader drops it.
///
/// Oracle `um = 524288` (@156964916), applied by `v()` through
/// `ZI(c,o,um)` — a file over the cap is never read into a workflow record,
/// so an oversized script cannot reach the registry (and the resolver
/// therefore never needs its own cap).
pub const MAX_WORKFLOW_SCRIPT_BYTES: u64 = 524_288;

/// One plugin-declared workflow script, keyed by its already-namespaced name.
#[derive(Debug, Clone)]
pub struct PluginWorkflowEntry {
    /// `{plugin}:{name}` — see the module doc for how `{name}` is derived.
    pub name: String,
    /// Absolute path to the workflow's `.js` source.
    pub script_path: PathBuf,
    /// Validated script snapshot captured when the plugin is loaded. Resolvers
    /// execute these bytes instead of re-reading a mutable path after the size
    /// and metadata checks have already passed.
    pub script: String,
}

/// One immutable workflow snapshot resolved from the live registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPluginWorkflow {
    /// Original validated path, retained for provenance and diagnostics.
    pub script_path: PathBuf,
    /// Validated bytes captured at plugin load time.
    pub script: String,
}

/// Shared table of plugin-declared workflow scripts, addressable by name
/// alongside built-in / project / user workflows.
///
/// `register`/`unregister` are keyed by an opaque plugin lifecycle owner.
/// Keeping ownership in the registry is what makes same-name collisions
/// reversible: removing the current winner reveals the still-loaded entry it
/// shadowed instead of deleting that other plugin's workflow.
#[derive(Debug, Default)]
pub struct PluginWorkflowRegistry {
    /// Contributions are kept in registration order per name. The last owner
    /// wins lookup, but unloading that owner reveals the previous contributor
    /// instead of deleting an unrelated plugin's still-live workflow.
    by_name: RwLock<HashMap<String, Vec<OwnedWorkflowEntry>>>,
}

#[derive(Debug, Clone)]
struct OwnedWorkflowEntry {
    owner: String,
    script_path: PathBuf,
    script: String,
}

impl PluginWorkflowRegistry {
    /// A new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace one owner's contribution. A collision across plugins remains
    /// last-write-wins, while retaining the shadowed entry for symmetric
    /// unload. `owner` is the plugin's opaque lifecycle identity, not its
    /// display/name namespace (two sources may legally share that name).
    pub fn register(&self, owner: &str, entries: Vec<PluginWorkflowEntry>) {
        let mut guard = self
            .by_name
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for contributors in guard.values_mut() {
            contributors.retain(|entry| entry.owner != owner);
        }
        guard.retain(|_, contributors| !contributors.is_empty());
        for entry in entries {
            guard
                .entry(entry.name)
                .or_default()
                .push(OwnedWorkflowEntry {
                    owner: owner.to_string(),
                    script_path: entry.script_path,
                    script: entry.script,
                });
        }
    }

    /// Remove one plugin owner's entries without disturbing a same-name entry
    /// contributed by another loaded plugin.
    pub fn unregister(&self, owner: &str) {
        let mut guard = self
            .by_name
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for contributors in guard.values_mut() {
            contributors.retain(|entry| entry.owner != owner);
        }
        guard.retain(|_, contributors| !contributors.is_empty());
    }

    /// Resolve a namespaced workflow name to its script path.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<ResolvedPluginWorkflow> {
        let guard = self
            .by_name
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(name)
            .and_then(|contributors| contributors.last())
            .map(|entry| ResolvedPluginWorkflow {
                script_path: entry.script_path.clone(),
                script: entry.script.clone(),
            })
    }

    /// All currently registered names, sorted — for the "Available: …" name
    /// listing alongside built-in/project/user names.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let guard = self
            .by_name
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        registry.register(
            "plugin-a",
            vec![PluginWorkflowEntry {
                name: "acme:deploy".to_string(),
                script_path: PathBuf::from("/plugins/acme/workflows/deploy.js"),
                script: "deploy();".to_string(),
            }],
        );
        assert_eq!(
            registry.resolve("acme:deploy"),
            Some(ResolvedPluginWorkflow {
                script_path: PathBuf::from("/plugins/acme/workflows/deploy.js"),
                script: "deploy();".to_string(),
            })
        );
        assert_eq!(registry.resolve("acme:missing"), None);
    }

    #[test]
    fn unregister_removes_exactly_the_named_entries() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(
            "plugin-a",
            vec![
                PluginWorkflowEntry {
                    name: "acme:deploy".to_string(),
                    script_path: PathBuf::from("/a/deploy.js"),
                    script: "a-deploy".to_string(),
                },
                PluginWorkflowEntry {
                    name: "acme:rollback".to_string(),
                    script_path: PathBuf::from("/a/rollback.js"),
                    script: "a-rollback".to_string(),
                },
            ],
        );
        registry.register(
            "plugin-b",
            vec![PluginWorkflowEntry {
                name: "other:keep".to_string(),
                script_path: PathBuf::from("/b/keep.js"),
                script: "keep".to_string(),
            }],
        );
        registry.unregister("plugin-a");
        assert_eq!(registry.resolve("acme:deploy"), None);
        assert_eq!(registry.resolve("acme:rollback"), None);
        assert_eq!(
            registry.resolve("other:keep"),
            Some(ResolvedPluginWorkflow {
                script_path: PathBuf::from("/b/keep.js"),
                script: "keep".to_string(),
            })
        );
    }

    #[test]
    fn names_are_sorted() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(
            "plugin-a",
            vec![
                PluginWorkflowEntry {
                    name: "zeta:z".to_string(),
                    script_path: PathBuf::from("/z.js"),
                    script: "z".to_string(),
                },
                PluginWorkflowEntry {
                    name: "alpha:a".to_string(),
                    script_path: PathBuf::from("/a.js"),
                    script: "a".to_string(),
                },
            ],
        );
        assert_eq!(
            registry.names(),
            vec!["alpha:a".to_string(), "zeta:z".to_string()]
        );
    }

    #[test]
    fn unloading_last_writer_reveals_shadowed_owner() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(
            "market-a",
            vec![PluginWorkflowEntry {
                name: "same:deploy".to_string(),
                script_path: PathBuf::from("/a/deploy.js"),
                script: "from-a".to_string(),
            }],
        );
        registry.register(
            "market-b",
            vec![PluginWorkflowEntry {
                name: "same:deploy".to_string(),
                script_path: PathBuf::from("/b/deploy.js"),
                script: "from-b".to_string(),
            }],
        );
        assert_eq!(
            registry.resolve("same:deploy"),
            Some(ResolvedPluginWorkflow {
                script_path: PathBuf::from("/b/deploy.js"),
                script: "from-b".to_string(),
            })
        );

        registry.unregister("market-b");
        assert_eq!(
            registry.resolve("same:deploy"),
            Some(ResolvedPluginWorkflow {
                script_path: PathBuf::from("/a/deploy.js"),
                script: "from-a".to_string(),
            })
        );
        registry.unregister("market-a");
        assert_eq!(registry.resolve("same:deploy"), None);
    }

    #[test]
    fn registering_empty_replaces_an_owners_old_inventory() {
        let registry = PluginWorkflowRegistry::new();
        registry.register(
            "plugin-a",
            vec![PluginWorkflowEntry {
                name: "acme:old".to_string(),
                script_path: PathBuf::from("/old.js"),
                script: "old".to_string(),
            }],
        );
        registry.register("plugin-a", Vec::new());
        assert_eq!(registry.resolve("acme:old"), None);
    }
}
