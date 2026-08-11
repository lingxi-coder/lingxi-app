//! Workflows shipped with the binary.
//!
//! Built-ins are resolved before project/user files. That ordering is a
//! security property: a project checkout must not be able to replace a
//! well-known bundled workflow with arbitrary code.

/// A workflow whose source is compiled into the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinWorkflowDescriptor {
    /// Stable invocation name.
    pub name: &'static str,
    /// Human-readable summary for workflow listings.
    pub description: &'static str,
    /// JavaScript source consumed by the ordinary workflow launcher.
    pub script: &'static str,
    /// Whether the workflow requires an explicit user invocation.
    pub manual_only: bool,
}

/// Immutable registry of built-in workflow content.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinWorkflowRegistry;

const DEEP_RESEARCH: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "deep-research",
    description: "Research a question across independent sources, verify each claim by vote, and synthesize a cited answer.",
    script: include_str!("deep_research_workflow.js"),
    manual_only: true,
};

/// The v3 local-app build segment: the `create-local-app` skill has the agent
/// gather requirements interactively in the MAIN session (AskUserQuestion),
/// then hand the confirmed spec to this workflow, which writes the source,
/// builds until green (≤3 attempts) and starts the preview. Deliberately
/// model-invocable (`manual_only: false`): the skill instructs the model to
/// call it by name.
const LOCAL_APP_BUILD: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "local-app-build",
    description: "Implement a confirmed local-app spec: write the source, build until green, and start the preview.",
    script: include_str!("local_app_build_workflow.js"),
    manual_only: false,
};

const BUILTINS: &[BuiltinWorkflowDescriptor] = &[DEEP_RESEARCH, LOCAL_APP_BUILD];

impl BuiltinWorkflowRegistry {
    /// Return an immutable built-in by exact name.
    #[must_use]
    pub fn get(self, name: &str) -> Option<&'static BuiltinWorkflowDescriptor> {
        BUILTINS.iter().find(|descriptor| descriptor.name == name)
    }

    /// Iterate the built-ins in stable display order.
    pub fn iter(self) -> impl ExactSizeIterator<Item = &'static BuiltinWorkflowDescriptor> {
        BUILTINS.iter()
    }

    /// Stable list of built-in names.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        self.iter().map(|descriptor| descriptor.name).collect()
    }
}

/// Process-wide immutable built-in workflow registry.
pub const BUILTIN_WORKFLOWS: BuiltinWorkflowRegistry = BuiltinWorkflowRegistry;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_research_is_manual_only_and_has_locked_limits() {
        let descriptor = BUILTIN_WORKFLOWS.get("deep-research").expect("built-in");
        assert!(descriptor.manual_only);
        workflow::validate_meta(descriptor.script).expect("valid built-in metadata");
        workflow::check_determinism(descriptor.script).expect("deterministic built-in");
        assert!(descriptor.script.contains("const VOTES_PER_CLAIM = 3"));
        assert!(descriptor.script.contains("const MAX_FETCH = 15"));
        for phase in ["Scope", "Search", "Fetch", "Verify", "Synthesize"] {
            assert!(
                descriptor.script.contains(&format!("title: '{phase}'"))
                    || descriptor.script.contains(&format!("phase('{phase}')")),
                "missing {phase} phase"
            );
        }
    }

    #[test]
    fn unknown_names_do_not_fall_back() {
        assert!(BUILTIN_WORKFLOWS.get("not-a-workflow").is_none());
    }

    #[test]
    fn local_app_build_is_model_invocable_and_pins_its_contract() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        assert!(
            !descriptor.manual_only,
            "the create-local-app skill instructs the model to invoke it by name"
        );
        workflow::validate_meta(descriptor.script).expect("valid built-in metadata");
        workflow::check_determinism(descriptor.script).expect("deterministic built-in");
        for phase in ["Generate", "Build"] {
            assert!(
                descriptor.script.contains(&format!("title: '{phase}'")),
                "missing {phase} phase"
            );
        }
        // The workspace contract must ride into EVERY agent prompt: writable
        // roots, locked files, the bridge-only rule, and the no-new-deps rule.
        for anchor in [
            "app/, components/, lib/, styles/, public/",
            "lib/lingxi-bridge.js",
            "window.lingxi.v1",
            "No new npm dependencies",
            "mcp__local_apps__build",
            "mcp__local_apps__manage_runtime",
            "up to 3 attempts",
        ] {
            assert!(
                descriptor.script.contains(anchor),
                "missing contract anchor: {anchor}"
            );
        }
        // Fails fast without an app id rather than spawning agents blind.
        assert!(descriptor.script.contains("requires args.app_id"));
    }
}
