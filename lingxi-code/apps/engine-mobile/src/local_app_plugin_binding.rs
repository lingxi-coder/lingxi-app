//! Composition binding from a Local App build target to the one workflow
//! authorized to build it (plan v3 Phase -1, task P-1.1).
//!
//! Today a build target maps to a workflow NAME string, and that string is
//! then used as authority: `workflow_support::apply_materialized_local_app_collections_with_identity`
//! resolves a `required_workflow_id` for the app's pinned runtime profile and
//! REFUSES a caller-selected workflow that does not match it. That refusal is
//! security-relevant — without it a caller could point any app at any
//! workflow's collection/persistence contract — and nothing in the suite
//! pinned it before this task (see the characterization tests added to
//! `workflow_support::tests` in this same change).
//!
//! P-1.1 does not delete the name map (`workflow_support::required_workflow_id_for`
//! keeps it, unchanged, in the one place it lived before). It wraps that map
//! in a typed [`LocalAppWorkflowHandle`] so the Host (the `apply_materialized_*`
//! seam) stops doing its own string comparison and error formatting: it asks
//! this binding to `resolve` a build target and `enforce` the result against
//! the caller's launched workflow id. P-1.3 is expected to go further and
//! remove the underlying string map entirely once the launcher itself can key
//! on a handle instead of a `BUILTIN_WORKFLOWS` name lookup; that is out of
//! scope here.

use crate::local_apps_build::LocalAppBuildTarget;

/// A resolved workflow identity for a Local App build target.
///
/// Opaque by construction, not merely by convention: the inner id has no
/// `pub(crate)` accessor, so nothing outside this module can compare it
/// against a bare string literal even if it wanted to. The point of routing
/// through [`LocalAppPluginBinding`] is that the Host no longer picks or
/// compares workflow names itself — it asks the binding to do it and consumes
/// the typed `Result`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalAppWorkflowHandle(&'static str);

impl LocalAppWorkflowHandle {
    /// The wire workflow id this handle currently resolves to.
    ///
    /// Reachable only from inside this module: it exists so `enforce` can
    /// compare a caller-selected id against it and name it in the refusal.
    /// There is deliberately NO accessor handing the handle (or its id) out
    /// to the Host — an earlier revision had one, used by nothing but this
    /// module's own tests, and it was both `dead_code` on the non-test
    /// `--lib` target and an invitation to go back to comparing raw strings
    /// at the seam. Callers get `enforce`'s `Result`, not the string.
    fn id(self) -> &'static str {
        self.0
    }
}

/// A composition binding from a Local App's pinned runtime profile (via its
/// [`LocalAppBuildTarget`]) to the one workflow authorized to build it.
pub(crate) struct LocalAppPluginBinding {
    handle: LocalAppWorkflowHandle,
}

impl LocalAppPluginBinding {
    /// Resolve the workflow handle a `build_target` is pinned to.
    ///
    /// Delegates to `workflow_support::required_workflow_id_for`, which still
    /// owns the build-target → workflow-id map (P-1.3's job to remove); this
    /// binding only wraps that map's output in a typed handle so callers stop
    /// touching the string themselves.
    pub(crate) fn resolve(build_target: LocalAppBuildTarget) -> Self {
        Self {
            handle: LocalAppWorkflowHandle(crate::workflow_support::required_workflow_id_for(
                build_target,
            )),
        }
    }

    /// Refuse a caller-selected workflow id that does not match this
    /// binding's resolved handle.
    ///
    /// Preserves the pre-Phase -1 inline refusal byte-for-byte: same app id,
    /// same pinned runtime-profile family, same required and caller-selected
    /// workflow ids named in the message.
    pub(crate) fn enforce(
        &self,
        app_id: &str,
        family: local_apps::AppRuntimeProfile,
        launched_workflow_id: &str,
    ) -> Result<(), tool_workflow::WorkflowLaunchError> {
        if launched_workflow_id != self.handle.id() {
            return Err(tool_workflow::WorkflowLaunchError(format!(
                "app {app_id:?} is pinned to runtime profile {family}, which must use {}; \
                 refusing caller-selected workflow {launched_workflow_id}",
                self.handle.id()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime-profile family and the LITERAL workflow id every build
    /// target must resolve to.
    ///
    /// Deliberately a wildcard-free `match` rather than an array of pairs:
    /// an array cannot be exhaustive, so a `LocalAppBuildTarget` variant
    /// added tomorrow would silently go untested. Here the test module stops
    /// compiling until the new variant is given an arm — a decision, not a
    /// default.
    ///
    /// The ids are restated as literals; they are NOT read back out of
    /// `workflow_support::required_workflow_id_for`. Asking the map what the
    /// map says pins no value — it only proves `resolve` still delegates,
    /// and passes unchanged if the map itself is rewritten wrongly. That
    /// matters most for `Babylon3dR1`, whose `local-canvas-build` mapping is
    /// pinned NOWHERE else in the suite: the `workflow_support` seam tests
    /// cannot reach it, because `detect_build_target` rejects the
    /// not-yet-published Babylon runtime profile first.
    fn expected_binding(
        target: LocalAppBuildTarget,
    ) -> (local_apps::AppRuntimeProfile, &'static str) {
        match target {
            LocalAppBuildTarget::ReactDomR1 => {
                (local_apps::AppRuntimeProfile::ReactDom, "local-app-build")
            }
            LocalAppBuildTarget::Canvas2dR1 => (
                local_apps::AppRuntimeProfile::Canvas2d,
                "local-canvas-build",
            ),
            LocalAppBuildTarget::Three3dR1 => {
                (local_apps::AppRuntimeProfile::Three3d, "local-canvas-build")
            }
            LocalAppBuildTarget::Phaser2dR1 => (
                local_apps::AppRuntimeProfile::Phaser2d,
                "local-canvas-build",
            ),
            LocalAppBuildTarget::Babylon3dR1 => (
                local_apps::AppRuntimeProfile::Babylon3d,
                "local-canvas-build",
            ),
        }
    }

    /// Every build target, so the assertions below actually run over the
    /// whole enum. Kept adjacent to [`expected_binding`] on purpose: a new
    /// variant's compile error lands in that match, a few lines from here.
    const ALL_BUILD_TARGETS: [LocalAppBuildTarget; 5] = [
        LocalAppBuildTarget::ReactDomR1,
        LocalAppBuildTarget::Canvas2dR1,
        LocalAppBuildTarget::Three3dR1,
        LocalAppBuildTarget::Phaser2dR1,
        LocalAppBuildTarget::Babylon3dR1,
    ];

    /// The other member of the two-element workflow-id set, used to drive the
    /// refusal path for a target whose required id is `required`.
    fn the_other_workflow_id(required: &str) -> &'static str {
        match required {
            "local-app-build" => "local-canvas-build",
            "local-canvas-build" => "local-app-build",
            other => panic!("unexpected required workflow id {other:?}"),
        }
    }

    #[test]
    fn resolve_pins_the_literal_workflow_id_of_every_build_target() {
        for target in ALL_BUILD_TARGETS {
            let (family, required) = expected_binding(target);
            let binding = LocalAppPluginBinding::resolve(target);

            binding
                .enforce("demo1234", family, required)
                .unwrap_or_else(|error| {
                    panic!("{target:?} must resolve to {required}, but it refused it: {error}")
                });

            // …and the acceptance above is not "accepts everything": the
            // other workflow id is refused, and the refusal names `required`
            // as the required id in the required POSITION.
            let launched = the_other_workflow_id(required);
            let error = binding
                .enforce("demo1234", family, launched)
                .expect_err("a workflow id other than the pinned one must be refused");
            let message = error.to_string();
            let expected_fragment =
                format!("must use {required}; refusing caller-selected workflow {launched}");
            assert!(
                message.contains(&expected_fragment),
                "{target:?} refusal must read {expected_fragment:?}, got: {message}"
            );
        }
    }

    #[test]
    fn enforce_accepts_a_matching_launched_id() {
        let binding = LocalAppPluginBinding::resolve(LocalAppBuildTarget::ReactDomR1);
        binding
            .enforce(
                "demo1234",
                local_apps::AppRuntimeProfile::ReactDom,
                "local-app-build",
            )
            .expect("matching launched workflow id must be accepted");
    }

    #[test]
    fn enforce_refuses_a_mismatched_launched_id_and_names_the_specifics() {
        let binding = LocalAppPluginBinding::resolve(LocalAppBuildTarget::ReactDomR1);
        let error = binding
            .enforce(
                "demo1234",
                local_apps::AppRuntimeProfile::ReactDom,
                "local-canvas-build",
            )
            .expect_err("mismatched launched workflow id must be refused");
        let message = error.to_string();
        assert!(
            message.contains("demo1234"),
            "must name the app id: {message}"
        );
        // Ordered fragment, not two independent `contains`: a refactor that
        // swapped the required and the caller-selected id would satisfy the
        // latter while telling the operator the exact opposite of the truth.
        assert!(
            message.contains(
                "must use local-app-build; refusing caller-selected workflow local-canvas-build"
            ),
            "must name the required id first and the caller-selected id second: {message}"
        );
    }
}
