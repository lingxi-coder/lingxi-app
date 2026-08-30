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
    /// Whether this workflow builds/updates a Local App's workspace (design
    /// §18 Phase -1 step 9 / §19.3).
    ///
    /// This is the ONE typed place that answers "is this a Local App build
    /// workflow" -- replacing what used to be two separate hand-maintained
    /// name arrays (`tasks::LOCAL_APP_BUILD_WORKFLOWS` and
    /// `tool_workflow::LOCAL_APP_BUILD_WORKFLOWS`, deleted by P-1.9). Both
    /// arrays answered the same question a different way for a different
    /// caller, which is exactly why they had to be kept byte-for-byte equal
    /// by a standalone twin-agreement test instead of being one source of
    /// truth. `deep-research` is the reason this cannot just be
    /// `BUILTIN_WORKFLOWS.names()` filtered some other way: it is a
    /// general-purpose workflow with no Local App identity at all, and nothing
    /// about its name distinguishes it -- only this field does.
    pub is_local_app_build: bool,
}

/// Immutable registry of built-in workflow content.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinWorkflowRegistry;

const DEEP_RESEARCH: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "deep-research",
    description: "Research a question across independent sources, verify each claim by vote, and synthesize a cited answer.",
    script: include_str!("deep_research_workflow.js"),
    manual_only: true,
    is_local_app_build: false,
};

/// The v3 local-app build segment: the `create-local-app` skill has the agent
/// gather requirements interactively in the MAIN session (AskUserQuestion),
/// then hand the confirmed spec to this workflow, which runs the deterministic
/// adaptive design/generate/build/verify sequence. Deliberately
/// model-invocable (`manual_only: false`): the skill instructs the model to
/// call it by name.
const LOCAL_APP_BUILD: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "local-app-build",
    description: "Adaptively design, generate, build, and verify a confirmed local app.",
    // Concatenated, not imported: the runtime evaluates one classic script and
    // requires `export const meta` to be its FIRST statement, so the shape file
    // has to come first and the shared driver after it.
    script: concat!(
        include_str!("local_app_build_workflow.js"),
        include_str!("local_app_workflow_core.js"),
    ),
    manual_only: false,
    is_local_app_build: true,
};

/// The drawn-surface sibling of [`LOCAL_APP_BUILD`].
///
/// A separate workflow rather than a flag, because almost everything the model
/// is told differs: a drawn app has no screen hierarchy to design, its evidence
/// is a captured frame rather than a DOM snapshot, and `data_roundtrip` — the
/// DOM workflow's only hard gate — answers `not_applicable` for the entire class
/// and therefore passes it unobserved.
///
/// The two share `local_app_workflow_core.js` verbatim, so the repair loop and
/// the terminal throws have one derivation, not two that drift.
const LOCAL_CANVAS_BUILD: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "local-canvas-build",
    description: "Adaptively design, generate, build, and verify a confirmed local app whose interface is a drawn surface.",
    script: concat!(
        include_str!("local_app_canvas_workflow.js"),
        include_str!("local_app_workflow_core.js"),
    ),
    manual_only: false,
    is_local_app_build: true,
};

const BUILTINS: &[BuiltinWorkflowDescriptor] =
    &[DEEP_RESEARCH, LOCAL_APP_BUILD, LOCAL_CANVAS_BUILD];

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

    /// Every built-in that builds/updates a Local App's workspace
    /// (`descriptor.is_local_app_build`), in stable display order.
    ///
    /// This is the ONE place both the `workflowModel` default and the
    /// component-literal scanner's needle derivation source their Local App
    /// build identity from -- see [`BuiltinWorkflowDescriptor::is_local_app_build`].
    pub fn local_app_build_descriptors(
        self,
    ) -> impl Iterator<Item = &'static BuiltinWorkflowDescriptor> {
        self.iter()
            .filter(|descriptor| descriptor.is_local_app_build)
    }

    /// Stable list of Local App build workflow names, derived from
    /// [`Self::local_app_build_descriptors`] rather than a hand-maintained
    /// array.
    #[must_use]
    pub fn local_app_build_workflow_names(self) -> Vec<&'static str> {
        self.local_app_build_descriptors()
            .map(|descriptor| descriptor.name)
            .collect()
    }

    /// True iff `name` names a built-in that has Local App build identity.
    /// A CUSTOM workflow that merely reuses one of these names is a
    /// different question this method does not (and cannot) answer -- see
    /// [`BuiltinWorkflowRegistry::local_app_build_descriptors`]'s callers for
    /// why script identity, not name, is the trusted signal wherever a
    /// caller-supplied name is in play.
    #[must_use]
    pub fn is_local_app_build_workflow(self, name: &str) -> bool {
        self.get(name)
            .is_some_and(|descriptor| descriptor.is_local_app_build)
    }

    /// True iff `script` is byte-identical to one of the compiled Local App
    /// build descriptors' `script`. Unlike [`Self::is_local_app_build_workflow`],
    /// this cannot be spoofed by a caller-supplied `name`/`meta.name`: the
    /// only way to satisfy it is to run the actual compiled-in bytes.
    #[must_use]
    pub fn is_local_app_build_script(self, script: &str) -> bool {
        self.local_app_build_descriptors()
            .any(|descriptor| descriptor.script == script)
    }
}

/// Process-wide immutable built-in workflow registry.
pub const BUILTIN_WORKFLOWS: BuiltinWorkflowRegistry = BuiltinWorkflowRegistry;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

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

    /// `is_local_app_build` (design §18 Phase -1 step 9 / §19.3) is the ONE
    /// typed source every Local App build workflow question now reads from --
    /// replacing the two hand-typed name arrays P-1.9 deleted
    /// (`tasks::LOCAL_APP_BUILD_WORKFLOWS` and
    /// `tool_workflow::LOCAL_APP_BUILD_WORKFLOWS`). `deep-research` pins the
    /// negative case: it is a real built-in with no Local App identity at
    /// all, so a query that fell back to `BUILTIN_WORKFLOWS.names()` instead
    /// of this field would wrongly include it.
    #[test]
    fn local_app_build_is_the_exact_two_build_workflows() {
        assert_eq!(
            BUILTIN_WORKFLOWS.local_app_build_workflow_names(),
            vec!["local-app-build", "local-canvas-build"],
        );
        assert!(BUILTIN_WORKFLOWS.is_local_app_build_workflow("local-app-build"));
        assert!(BUILTIN_WORKFLOWS.is_local_app_build_workflow("local-canvas-build"));
        assert!(
            !BUILTIN_WORKFLOWS.is_local_app_build_workflow("deep-research"),
            "deep-research is a general-purpose workflow with no Local App identity"
        );
        assert!(!BUILTIN_WORKFLOWS.is_local_app_build_workflow("not-a-workflow"));

        let deep_research = BUILTIN_WORKFLOWS.get("deep-research").expect("built-in");
        assert!(
            !BUILTIN_WORKFLOWS.is_local_app_build_script(deep_research.script),
            "the script-identity check must also reject deep-research's own bytes"
        );
        let local_app_build = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        assert!(BUILTIN_WORKFLOWS.is_local_app_build_script(local_app_build.script));
        assert!(
            !BUILTIN_WORKFLOWS
                .is_local_app_build_script("export const meta = { name: 'local-app-build' };"),
            "a custom script that merely DECLARES the real name must not pass the byte check"
        );
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
        for phase in ["Design", "Generate & Build", "Verify"] {
            assert!(
                descriptor.script.contains(&format!("title: '{phase}'")),
                "missing {phase} phase"
            );
        }
        let phase_positions = ["Design", "Generate & Build", "Verify"].map(|phase| {
            descriptor
                .script
                .find(&format!("title: '{phase}'"))
                .expect("phase position")
        });
        assert!(
            phase_positions.windows(2).all(|pair| pair[0] < pair[1]),
            "local-app phases must stay ordered"
        );
        assert!(
            descriptor.script.contains("maxRepairRounds"),
            "local-app strategies must pin their repair policies"
        );
        // The workspace contract must ride into EVERY agent prompt: writable
        // roots, locked files, the bridge-only rule, and the no-new-deps rule.
        for anchor in [
            "strategy?: 'fast'|'balanced'|'thorough'",
            "'balanced'",
            "runDesign",
            "verificationMode",
            "Selected workflow strategy",
            "Complexity score",
            "args.runtime_profile",
            "Persisted runtime profile",
            "react_dom",
            "agent_calls",
            "verification_mode",
            "app/, src/, components/, lib/, styles/, public/",
            "lib/lingxi-bridge.js",
            "window.lingxi.v2",
            "streamLlmChat",
            "getClipboardText",
            "setClipboardText",
            "shareContent",
            "synthesizeSpeech",
            "readFile",
            "writeFile",
            "getDeviceStatus",
            "triggerHaptics",
            "openDeepLink",
            "listCalendarEvents",
            "searchContacts",
            "getMedia",
            "background_schedule",
            "Do not run npm, npx, node",
            "package.json, pnpm-lock.yaml, pnpm-workspace.yaml, index.html, vite.config.*",
            "host has already scaffolded the workspace",
            "fixed by the host-owned template and lockfile set",
            "existing Git capability",
            "LocalAppBuild",
            "LocalAppRuntime",
            "host may prepare the workspace dependencies when needed",
            // A dependency the generator is never told about is the same as one
            // that was never added: the contract only says "stay within the
            // locked set" and never enumerates it, so `three` has to be named
            // here or every 3D request falls back to hand-rolled 2D.
            "three@0.185.1 is in the locked set",
            // The UI kit, and the one import style that survives the pinned iife
            // build. A generated app that reaches for `@ionic/core/components`
            // fails at BUNDLE time with a code-splitting error that names neither
            // Ionic nor this contract.
            "@ionic/react barrel",
            "NEVER from @ionic/core/components",
            // Without this the outlet has nothing to animate and the platform back
            // gesture never attaches -- a silent loss of the thing Ionic was chosen
            // for.
            "IonPage as its ROOT element",
            // The run surface carries NO host chrome: the navigation bar is
            // hidden, so the only bars the user gets are the ones this app
            // draws. Lose this line and a generated app ships with zero bars
            // and no way back — a defect neither the build nor the DOM
            // snapshot reports, because the page itself is perfectly valid.
            "The host draws NO chrome around a running app",
            "renders its own IonBackButton inside IonButtons slot=\"start\" with a defaultHref",
            // The one thing the host DOES paint over the page, and the square it
            // owns. The number is derived from the larger of the two clients —
            // Android's collapsed RunPill, 16 + 4 + 48 + 4 = 72dp across and
            // 16 + 48 = 64dp up, rounded to 80 — and the JS comment beside the
            // line carries that arithmetic. Pinned as a NUMBER on purpose: a
            // client that changes its tap target must not be able to leave the
            // published square silently too small, which is exactly how it came
            // to say 64 while both clients already measured 68 and 72.
            "The host floats ONE control over the running page in the BOTTOM-LEADING corner",
            "the leading 80pt by the bottom 80pt measured from the safe area",
            // The two Ionic defaults that land inside that square. IonFab's own
            // default corner is the clear one; the leading-most tab of a
            // full-width IonTabBar is not, and a bottom tab bar is the shape a
            // generator reaches for first.
            "never place one at horizontal=\"start\" with vertical=\"bottom\"",
            "reserve the leading edge of the bar with padding-inline-start",
            // Design stage, not generate. Which screen owns a header, where the
            // back affordance lives, and whether a bottom tab bar is viable at
            // all are hierarchy decisions: made after the source exists they
            // cost a repair round instead of a sentence.
            "keep the bottom-leading corner free of app furniture",
            // This workflow is the routed DOM scaffold. It must pin the on-disk
            // entry point and forbid importing the canvas-only screen/helper.
            "app/screens/home-screen.jsx for this routed app",
            "do not import or reference canvas-only entry points such as app/screens/game-screen.jsx",
            "Whole drawn surfaces route to the dedicated canvas workflows",
            "own exactly one requestAnimationFrame loop inside an effect",
            "cancel it in that effect’s cleanup",
            "device pixel ratio",
            "clamp the first delta after a resume/background return",
            "Do not import the canvas-only lib/frame-loop.js helper here",
            "maxRepairRounds",
            "conditionally detect ImageGen",
            "A narrow viewport alone does not prove a platform",
            "Cover the full confirmed matrix: iPhone, Android phone, iPad portrait+landscape, Android tablet portrait+landscape, and desktop",
            "LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs",
            // A canvas app is invisible to the DOM path: an empty element list
            // means nothing, so the verifier must be told to reach for the
            // frame and the pointer/key vocabulary instead. Without these the
            // capability exists and the agent never learns it can use it.
            "LocalAppCaptureUi",
            "A canvas app verified only through inspect_ui has not been verified",
            // The gate keys on the PRESENCE of a canvas, not on an empty
            // element list — a real generated game ships a score bar and a
            // restart button, so the empty-DOM version never fired for it.
            "canvas_surfaces is the canvasCount field",
            // The generate stage burned all 100 turns re-verifying its own
            // output on device, so Verify never ran once. Losing this line
            // reopens that: the stage looks productive while starving the
            // one that actually gates.
            "STOP once the build succeeds and the runtime is started",
            // WebAssembly and workers are DEFAULTS, not a capability. Losing
            // this line puts the generator back to designing around a
            // restriction that no longer exists.
            "WebAssembly and Web Workers are available to every local app",
            "you may not answer not_applicable",
            // `findings` is blocking, so an agent that fills it with what it
            // checked fails a build that passed. Observed: a verify pass came
            // back `ok:true` with every gate green and ten sentences of praise
            // in `findings`, and a repair agent was dispatched to "repair" them.
            // The fixtures below all write `[]`, so only this anchor keeps the
            // field's meaning in front of the agent that has to fill it.
            "findings is the list of UNRESOLVED DEFECTS",
            "returns findings as an empty array no matter how much it verified",
            "degraded_verification",
            "records[].document",
            "LocalAppQueryData",
            "localStorage must never be authoritative",
            "Do not swallow bridge errors",
        ] {
            assert!(
                descriptor.script.contains(anchor),
                "missing contract anchor: {anchor}"
            );
        }
        // A forbidden anchor only gates something if the string it names can
        // actually appear. Two of the three strings this block used to list
        // ("use the profile-managed createFrameLoop helper from
        // lib/frame-loop.js" and "Do not call requestAnimationFrame
        // directly or hand-write a replacement loop") matched NO workflow
        // script, at this revision or any earlier one, so two thirds of the
        // block asserted the absence of text nobody had ever written while
        // reading like a live gate. The first entry is different and stays: it
        // is text this work DELETED, so it guards against reintroducing it.
        // The second entry is the canvas contract's real wording, which is what
        // a copy-paste into the DOM contract would actually bring with it.
        let canvas_frame_loop_mandate =
            "own the frame loop through the profile-managed helper createFrameLoop";
        for forbidden in [
            "app/screens/game-screen.jsx for a drawn surface",
            canvas_frame_loop_mandate,
        ] {
            assert!(
                !descriptor.script.contains(forbidden),
                "DOM workflow must not retain incompatible contract text: {forbidden}"
            );
        }
        // Proves the needle above can still match. Without this, rewording the
        // canvas mandate would silently turn the DOM prohibition back into an
        // assertion about a string that exists nowhere — the exact failure this
        // block just had.
        assert!(
            BUILTIN_WORKFLOWS
                .get("local-canvas-build")
                .expect("built-in")
                .script
                .contains(canvas_frame_loop_mandate),
            "the canvas frame-loop mandate was reworded; the DOM prohibition above now \
             guards a string that appears nowhere and can never fire"
        );
        // Fails fast without an app id rather than spawning agents blind.
        assert!(descriptor.script.contains("requires args.app_id"));
        assert_eq!(
            descriptor.script.matches("await runAgent(").count(),
            descriptor.script.matches("throwOnError: true").count(),
            "every local-app agent stage must surface its terminal failure reason",
        );
        let forbidden_model_name = ["gpt-5.6", "luna"].concat();
        assert!(!descriptor.script.contains(&forbidden_model_name));
    }

    #[test]
    fn local_app_build_rejects_non_dom_runtime_profiles_before_any_agent() {
        let mut args: Value =
            serde_json::from_str(&local_app_args(Some("balanced"), None, None, None))
                .expect("local app args are JSON");
        args.as_object_mut()
            .expect("local app args are an object")
            .insert(
                "runtime_profile".to_string(),
                runtime_profile_binding("three_3d"),
            );
        let run = drive_local_app_build(args.to_string(), |_prompts, _options| {
            panic!("invalid DOM runtime profile must fail before any agent starts");
        });
        let error = run
            .outcome
            .expect_err("non-react_dom runtime profile must be rejected");
        assert!(
            error
                .to_string()
                .contains("requires args.runtime_profile.family to be exactly one of react_dom"),
            "unexpected runtime profile error: {error}"
        );
        assert!(run.prompts.is_empty());
    }

    #[test]
    fn local_app_build_happy_path_strategy_matrix() {
        struct Case {
            name: &'static str,
            strategy: Option<&'static str>,
            expected_phases: &'static [&'static str],
            expected_calls: usize,
            expected_verification_mode: &'static str,
            expects_design: bool,
        }

        for case in [
            Case {
                name: "missing strategy defaults to balanced",
                strategy: None,
                expected_phases: &["Design", "Generate & Build", "Verify"],
                expected_calls: 3,
                expected_verification_mode: "confirmed-targets",
                expects_design: true,
            },
            Case {
                name: "fast skips standalone design",
                strategy: Some("fast"),
                expected_phases: &["Generate & Build", "Verify"],
                expected_calls: 2,
                expected_verification_mode: "smoke",
                expects_design: false,
            },
            Case {
                name: "balanced keeps the standard three-stage flow",
                strategy: Some("balanced"),
                expected_phases: &["Design", "Generate & Build", "Verify"],
                expected_calls: 3,
                expected_verification_mode: "confirmed-targets",
                expects_design: true,
            },
            Case {
                name: "thorough preserves the full happy path",
                strategy: Some("thorough"),
                expected_phases: &["Design", "Generate & Build", "Verify"],
                expected_calls: 3,
                expected_verification_mode: "full-matrix",
                expects_design: true,
            },
        ] {
            let name = case.name;
            let expects_design = case.expects_design;
            let run = drive_local_app_build(
                local_app_args(case.strategy, None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                    if prompt.contains("Act as the local app design lead") {
                        assert!(expects_design, "{name} unexpectedly invoked design");
                        dom_design()
                    } else if prompt.contains("Generate the complete React implementation") {
                        r#"{"ok":true,"preview_url":"http://preview/first","summary":"built"}"#
                            .to_string()
                    } else if prompt.contains("Invoke $frontend-qa") {
                        r#"{"ok":true,"findings":[],"checked_matrix":["android-phone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#.to_string()
                    } else {
                        panic!("{name} must not repair on the happy path: {prompt}");
                    }
                        })
                        .collect()
                },
            );
            let outcome = run.outcome.expect(case.name);
            let result: Value = serde_json::from_str(outcome.result.as_deref().expect("result"))
                .expect("workflow returns JSON");
            assert_eq!(run.prompts.len(), case.expected_calls, "{}", case.name);
            assert_eq!(run.phases, case.expected_phases, "{}", case.name);
            assert_eq!(result["ok"], true, "{}", case.name);
            assert_eq!(
                result["strategy"],
                case.strategy.unwrap_or("balanced"),
                "{}",
                case.name
            );
            assert_eq!(
                result["verification_mode"], case.expected_verification_mode,
                "{}",
                case.name
            );
            assert_eq!(
                result["preview_url"], "http://preview/first",
                "{}",
                case.name
            );
            assert_eq!(result["repair_rounds"], 0, "{}", case.name);
            assert_eq!(result["agent_calls"], case.expected_calls, "{}", case.name);
            let generation_prompt = run
                .prompts
                .iter()
                .find(|prompt| prompt.contains("Generate the complete React implementation"))
                .expect("DOM workflow has a generation prompt");
            assert!(
                generation_prompt.contains("$ionic-react-local-app"),
                "DOM generation must invoke the Ionic specialist"
            );
            assert!(
                generation_prompt.contains("$accessibility")
                    && generation_prompt.contains("$react-best-practices"),
                "DOM generation must invoke the accessibility and React reviewers"
            );
            if expects_design {
                for design_field in [
                    "information_architecture",
                    "visual_system",
                    "interaction_model",
                    "adapter_boundary",
                    "acceptance_checks",
                    "presentation",
                    "navigation",
                    "permission",
                ] {
                    assert!(
                        generation_prompt.contains(design_field),
                        "DOM generation must receive the structured design field {design_field}"
                    );
                }
            }
            assert!(!generation_prompt.contains("$canvas-2d-local-app"));
            assert!(!generation_prompt.contains("$threejs-local-app"));
            if case.strategy == Some("thorough") {
                assert!(
                    run.prompts.iter().any(|prompt| prompt.contains(
                        "Cover the full confirmed matrix: iPhone, Android phone, iPad portrait+landscape, Android tablet portrait+landscape, and desktop"
                    )),
                    "thorough verification must retain the full explicit target matrix"
                );
            }
            let expected_summary = if case.expects_design {
                format!(
                    "{} strategy completed design, generation, build, and frontend QA.",
                    case.strategy.unwrap_or("balanced")
                )
            } else {
                "fast strategy completed generation, build, and frontend QA.".to_string()
            };
            assert_eq!(result["summary"], expected_summary, "{}", case.name);
            assert!(result.get("scaffold_mode").is_none(), "{}", case.name);
            assert!(result.get("template").is_none(), "{}", case.name);
        }
    }

    #[test]
    fn local_app_build_repairs_rebuilds_restarts_and_reverifies_structured_results() {
        let calls = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
        let captured_calls = calls.clone();
        let verification_round = Rc::new(Cell::new(0_u8));
        let next_verification_round = verification_round.clone();
        let run = drive_local_app_build(
            local_app_args(
                Some("balanced"),
                Some(json!({
                    "score": 4,
                    "confidence": 0.9,
                    "reasons": ["three screens", "one writable collection"],
                    "ignored": "value"
                })),
                Some("deepseek/deepseek-v4-flash"),
                None,
            ),
            move |prompts: &[String], options: &[String]| {
                prompts
                    .iter()
                    .zip(options)
                    .map(|(prompt, option)| {
                        captured_calls
                            .borrow_mut()
                            .push((prompt.clone(), option.clone()));
                        if prompt.contains("Act as the local app design lead") {
                            dom_design()
                        } else if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/initial","summary":"built"}"#.to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            let round = next_verification_round.get();
                            next_verification_round.set(round + 1);
                            if round == 0 {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#.to_string()
                            } else {
                                r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#.to_string()
                            }
                        } else if prompt.contains("Repair the findings") {
                            r#"{"ok":true,"preview_url":"http://preview/repaired","summary":"rebuilt and restarted"}"#.to_string()
                        } else {
                            panic!("unexpected local-app workflow prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run.outcome.expect("local-app workflow executes");

        let result: Value =
            serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                .expect("workflow returns JSON");
        assert_eq!(result["ok"], true);
        assert_eq!(result["strategy"], "balanced");
        assert_eq!(result["verification_mode"], "confirmed-targets");
        assert_eq!(result["preview_url"], "http://preview/repaired");
        assert!(result.get("scaffold_mode").is_none());
        assert!(result.get("template").is_none());
        assert_eq!(result["repair_rounds"], 1);
        assert_eq!(result["agent_calls"], 5);
        assert_eq!(verification_round.get(), 2);
        assert_eq!(
            result["complexity"],
            json!({
                "score": 4,
                "band": "medium",
                "confidence": 0.9,
                "reasons": ["three screens", "one writable collection"]
            })
        );
        assert_eq!(
            calls.borrow().len(),
            5,
            "one repair uses no standalone build agents"
        );
        assert_eq!(
            run.phases,
            vec![
                "Design".to_string(),
                "Generate & Build".to_string(),
                "Verify".to_string(),
                "Generate & Build".to_string(),
                "Verify".to_string(),
            ]
        );

        let calls = calls.borrow();
        for (_, options) in calls.iter() {
            let options: Value = serde_json::from_str(options).expect("agent options JSON");
            assert_eq!(options["model"], "deepseek-v4-flash");
            assert_eq!(options["modelProfile"], "deepseek");
        }
        for prompt_anchor in [
            "Act as the local app design lead",
            "Invoke $frontend-qa",
            "Repair the findings",
        ] {
            let (_, options) = calls
                .iter()
                .find(|(prompt, _)| prompt.contains(prompt_anchor))
                .unwrap_or_else(|| panic!("missing {prompt_anchor} agent call"));
            let options: Value = serde_json::from_str(options).expect("agent options JSON");
            assert!(
                options.get("schema").is_some(),
                "{prompt_anchor} must request a structured result"
            );
        }
        let second_verification = calls
            .iter()
            .filter(|(prompt, _)| prompt.contains("Invoke $frontend-qa"))
            .nth(1)
            .expect("verification after repair");
        assert!(
            second_verification.0.contains("http://preview/repaired"),
            "re-verification must receive the restarted preview URL"
        );
        let rebuild = calls
            .iter()
            .find(|(prompt, _)| prompt.contains("Repair the findings"))
            .expect("repair-build call");
        assert!(rebuild.0.contains(r#""action":"restart""#));
    }

    #[test]
    fn local_app_build_repair_prompt_carries_authoritative_design_brief() {
        for (strategy, expects_design) in [("fast", false), ("balanced", true)] {
            let verification_round = Rc::new(Cell::new(0_u8));
            let next_verification_round = verification_round.clone();
            let args = if expects_design {
                local_app_args_with_spec(Some(strategy), "DOM_REPAIR_SPEC_SENTINEL")
            } else {
                local_app_args_with_spec_and_revision(
                    Some(strategy),
                    "DOM_REPAIR_SPEC_SENTINEL",
                    "DOM_REPAIR_REVISION_SENTINEL",
                )
            };
            let run = drive_local_app_build(args, move |prompts, _options| {
                prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                assert!(
                                    expects_design,
                                    "{strategy} repair fixture must not run standalone design"
                                );
                                dom_design()
                            } else if prompt.contains("Generate the complete React implementation") {
                                if !expects_design {
                                    assert_eq!(
                                        prompt.matches("DOM_REPAIR_REVISION_SENTINEL").count(),
                                        1,
                                        "fast generation must receive the revision exactly once"
                                    );
                                }
                                r#"{"ok":true,"preview_url":"http://preview/initial","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                if !expects_design {
                                    assert_eq!(
                                        prompt.matches("DOM_REPAIR_REVISION_SENTINEL").count(),
                                        1,
                                        "fast verification must receive the revision exactly once"
                                    );
                                }
                                let round = next_verification_round.get();
                                next_verification_round.set(round + 1);
                                if round == 0 {
                                    r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#.to_string()
                                } else {
                                    r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#.to_string()
                                }
                            } else if prompt.contains("Repair the findings") {
                                assert!(
                                    prompt.contains("Authoritative design brief (source of truth"),
                                    "DOM repair must identify the design brief as authoritative"
                                );
                                assert!(
                                    prompt.contains("Confirmed product specification (source of truth")
                                        && prompt.contains("DOM_REPAIR_SPEC_SENTINEL"),
                                    "DOM repair must receive the confirmed product specification"
                                );
                                assert_eq!(
                                    prompt.matches("DOM_REPAIR_SPEC_SENTINEL").count(),
                                    1,
                                    "DOM repair must receive the confirmed specification exactly once"
                                );
                                if !expects_design {
                                    assert_eq!(
                                        prompt.matches("DOM_REPAIR_REVISION_SENTINEL").count(),
                                        1,
                                        "fast repair must receive the revision exactly once"
                                    );
                                    assert!(
                                        prompt.contains(
                                            "No standalone design brief was produced; preserve the confirmed request."
                                        ),
                                        "fast DOM repair must use the confirmed-request design fallback"
                                    );
                                } else {
                                    assert!(
                                        prompt.contains("Quiet, native-feeling utility UI")
                                            && prompt.contains("information_architecture")
                                            && prompt.contains("adapter_boundary"),
                                        "DOM repair must receive the complete structured design brief"
                                    );
                                }
                                assert!(
                                    !prompt.contains("renderer"),
                                    "DOM repair must not route or depend on a renderer"
                                );
                                r#"{"ok":true,"preview_url":"http://preview/repaired","summary":"rebuilt"}"#
                                    .to_string()
                            } else {
                                panic!("unexpected DOM design propagation prompt: {prompt}");
                            }
                    })
                    .collect()
            });
            run.outcome
                .unwrap_or_else(|error| panic!("{strategy} DOM repair should complete: {error}"));
            assert_eq!(
                run.prompts.len(),
                if expects_design { 5 } else { 4 },
                "{strategy} repair prompt count"
            );
            assert_eq!(
                run.prompts
                    .iter()
                    .filter(|prompt| prompt.contains("Act as the local app design lead"))
                    .count(),
                usize::from(expects_design),
                "{strategy} standalone design count"
            );
        }
    }

    #[test]
    fn local_app_build_fast_generation_receives_revision_feedback_without_design_duplication() {
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, Some("FAST_REVISION_SENTINEL")),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation") {
                            assert!(
                                prompt.contains("Revision feedback:\nFAST_REVISION_SENTINEL"),
                                "fast generation must receive revision feedback"
                            );
                            assert_eq!(
                                prompt.matches("FAST_REVISION_SENTINEL").count(),
                                1,
                                "fast generation must receive revision feedback exactly once"
                            );
                            r#"{"ok":true,"preview_url":"http://preview/fast","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            assert!(
                                prompt.contains("FAST_REVISION_SENTINEL"),
                                "fast verification must receive the revision feedback"
                            );
                            assert_eq!(
                                prompt.matches("FAST_REVISION_SENTINEL").count(),
                                1,
                                "fast verification must receive revision feedback exactly once"
                            );
                            dom_verification_with_webview(Some(true))
                        } else {
                            panic!("unexpected fast revision prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        run.outcome
            .expect("fast generation with revision feedback should complete");
        assert_eq!(run.prompts.len(), 2);
        assert!(
            !run.prompts[0].contains("Act as the local app design lead"),
            "fast strategy must not run standalone design"
        );
    }

    #[test]
    fn local_app_build_fast_repair_and_reverify_keep_revision_feedback_without_design() {
        let verification_round = Rc::new(Cell::new(0_u8));
        let next_verification_round = verification_round.clone();
        let run = drive_local_app_build(
            local_app_args(
                Some("fast"),
                None,
                None,
                Some("FAST_REPAIR_REVISION_SENTINEL"),
            ),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            assert!(
                                prompt.contains(
                                    "Revision feedback:\nFAST_REPAIR_REVISION_SENTINEL"
                                ),
                                "fast generation/repair must receive revision feedback"
                            );
                            assert_eq!(
                                prompt.matches("FAST_REPAIR_REVISION_SENTINEL").count(),
                                1,
                                "fast generation/repair must receive revision feedback exactly once"
                            );
                            r#"{"ok":true,"preview_url":"http://preview/fast-repair","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            assert!(
                                prompt.contains("FAST_REPAIR_REVISION_SENTINEL"),
                                "fast verification must receive the revision feedback"
                            );
                            assert_eq!(
                                prompt.matches("FAST_REPAIR_REVISION_SENTINEL").count(),
                                1,
                                "fast verification must receive revision feedback exactly once"
                            );
                            let round = next_verification_round.get();
                            next_verification_round.set(round + 1);
                            if round == 0 {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#
                                    .to_string()
                            } else {
                                dom_verification_with_webview(Some(true))
                            }
                        } else {
                            panic!("unexpected fast repair revision prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        run.outcome
            .expect("fast repair with revision feedback should complete");
        assert_eq!(verification_round.get(), 2);
        assert_eq!(run.prompts.len(), 4);
        assert_eq!(
            run.prompts
                .iter()
                .filter(|prompt| prompt.contains("Act as the local app design lead"))
                .count(),
            0,
            "fast strategy must not run standalone design"
        );
    }

    #[test]
    fn local_app_build_rejects_invalid_strategy_before_any_agent_runs() {
        for strategy in [Some("turbo"), Some("123")] {
            let run = drive_local_app_build(
                local_app_args(strategy, None, None, None),
                |_prompts, _options| panic!("unsupported strategies must fail before agent()"),
            );
            let error = run.outcome.expect_err("unsupported strategies must fail");
            assert!(
                run.prompts.is_empty(),
                "agent() must not run for invalid strategy"
            );
            assert!(
                run.phases.is_empty(),
                "phase() must not run for invalid strategy"
            );
            assert!(
                error.to_string().contains("unsupported strategy"),
                "unexpected workflow error: {error}"
            );
        }

        let run = drive_local_app_build(
            json!({
            "app_id": "test-app",
            "spec": "build a test app",
            "strategy": 123,
            "expected_writable_collections": [],
            "runtime_profile": runtime_profile_binding("react_dom"),
            })
            .to_string(),
            |_prompts, _options| panic!("non-string strategies must fail before agent()"),
        );
        let error = run.outcome.expect_err("non-string strategies must fail");
        assert!(run.prompts.is_empty());
        assert!(run.phases.is_empty());
        assert!(error.to_string().contains("unsupported strategy 123"));
    }

    #[test]
    fn local_app_build_rejects_failed_data_roundtrip_even_when_verifier_says_ok() {
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"failed","collections":["items"],"evidence":"query_data did not contain the written record"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verifier incorrectly reported success"}"#
                                .to_string()
                        } else {
                            panic!("unexpected prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("failed native persistence must not return success");
        assert!(
            error.to_string().contains("native data round-trip failed"),
            "unexpected workflow error: {error}"
        );
        assert_eq!(run.prompts.len(), 4, "fast gets one repair before failing");
    }

    /// The render gate must FIRE, not merely exist.
    ///
    /// `data_roundtrip` is the only other gate with teeth and it answers
    /// `not_applicable` for an app with no writable collection — so before this
    /// gate, an app that draws its interface could complete verification having
    /// observed nothing at all: an empty DOM snapshot, no frame, and a green
    /// result. Each case below is one distinct way to reach "nothing was
    /// verified", and each must be rejected on its own.
    #[test]
    fn local_app_build_rejects_a_verification_that_observed_nothing() {
        // (render_check body, the fragment the failure must name)
        let cases = [
            (
                // The vacuous-canvas shape: inspect_ui saw nothing, no frame was
                // taken, and the agent shrugged it off as not applicable.
                r#"{"status":"not_applicable","canvas_surfaces":1,"frames_captured":0,"interactions_driven":[],"evidence":"the score bar and restart button both inspected fine"}"#,
                "a canvas surface was present but never verified",
            ),
            (
                // Claimed the frame was checked without ever obtaining one. The
                // claim and the count come from different places, so they can
                // disagree — and this disagreement is the whole point.
                r#"{"status":"passed","canvas_surfaces":1,"frames_captured":0,"interactions_driven":["pointer 10,10 tap"],"evidence":"looks fine"}"#,
                "captured no frame",
            ),
            (
                // An honest failure still has to fail the build.
                r#"{"status":"failed","canvas_surfaces":1,"frames_captured":2,"interactions_driven":[],"evidence":"frame is blank"}"#,
                "render check failed",
            ),
        ];
        for (render_check, expected) in cases {
            let verification = format!(
                r#"{{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{{"status":"not_applicable","collections":[],"evidence":"no writable collections"}},"render_check":{render_check},"summary":"verified"}}"#
            );
            let run = drive_local_app_build(
                local_app_args(Some("fast"), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Generate the complete React implementation")
                                || prompt.contains("Repair the findings")
                            {
                                r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                verification.clone()
                            } else {
                                panic!("unexpected prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let error = run
                .outcome
                .expect_err("a verification that observed nothing must not return success");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in: {error}"
            );
            // The count, not just the message. A render finding is a source
            // defect a repair round can fix, and it has to BUY one exactly like
            // a non-empty `findings` does — otherwise `fast`'s single allowed
            // round is silently forfeited and the whole run is lost on a defect
            // the loop was budgeted to repair. Asserting only the error text is
            // what let that asymmetry through. `fast` skips design, so the
            // sequence is generate-build, verify, repair-build, verify = 4.
            assert_eq!(
                run.prompts.len(),
                4,
                "a render finding must reach a repair round before the build fails; \
                 prompts={:#?}",
                run.prompts
            );
            // Derived findings are absent from the verifier's own JSON, so the
            // repair agent has to be told what it is repairing.
            assert!(
                run.prompts.iter().any(|prompt| {
                    prompt.contains("Repair the findings") && prompt.contains(expected)
                }),
                "the repair round must receive the render finding: {:#?}",
                run.prompts
            );
        }
    }

    /// The gate must NOT fire for an ordinary DOM app.
    ///
    /// A form-shaped app legitimately has no frame to capture, because the
    /// element snapshot already carried the evidence. If the gate rejected that
    /// it would just be a tax on every non-game app, and would be silenced.
    #[test]
    fn local_app_build_accepts_a_dom_app_that_captured_no_frame() {
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#
                                .to_string()
                        } else {
                            panic!("unexpected prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        run.outcome
            .expect("a DOM app with a non-empty snapshot needs no frame");
    }

    /// A non-empty `findings` blocks even when everything else is green, and
    /// that is deliberate.
    ///
    /// This shape is not hypothetical: a real verify pass against a generated
    /// snake game returned `ok:true` with `data_roundtrip` and `render_check`
    /// both satisfied and ten sentences of praise in `findings`, which spent a
    /// repair round asking an agent to "repair" the fact that the app worked.
    /// The defect is that the field was never DEFINED for the agent filling it,
    /// so the fix belongs in the verify prompt — this test exists so nobody
    /// "fixes" it here instead, by letting `ok:true` override the findings. That
    /// would silently un-gate every real defect an honest verifier reports.
    #[test]
    fn local_app_build_lets_no_finding_through_on_a_self_declared_pass() {
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":["the pause button does nothing on the second press"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"passed","canvas_surfaces":1,"frames_captured":4,"interactions_driven":["pointer 207,320 down"],"evidence":"frames show the board animating"},"summary":"verified"}"#
                                .to_string()
                        } else {
                            panic!("unexpected prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("a reported defect must not be overridden by ok:true");
        assert!(
            error.to_string().contains("the pause button does nothing"),
            "the failure must quote the finding, not paraphrase it: {error}"
        );
        assert_eq!(
            run.prompts.len(),
            4,
            "the finding must reach a repair round before the build fails"
        );
        // The repair agent is handed the verification verbatim, so whatever the
        // verifier wrote in `findings` becomes its instructions.
        assert!(
            run.prompts
                .iter()
                .any(|prompt| prompt.contains("Repair the findings")
                    && prompt.contains("the pause button does nothing")),
            "the repair round must receive the finding it is repairing"
        );
    }

    #[test]
    fn local_app_build_fast_and_balanced_stop_after_one_repair_round() {
        for (strategy, expected_phases, expected_calls) in [
            (
                "fast",
                vec![
                    "Generate & Build".to_string(),
                    "Verify".to_string(),
                    "Generate & Build".to_string(),
                    "Verify".to_string(),
                ],
                4_usize,
            ),
            (
                "balanced",
                vec![
                    "Design".to_string(),
                    "Generate & Build".to_string(),
                    "Verify".to_string(),
                    "Generate & Build".to_string(),
                    "Verify".to_string(),
                ],
                5_usize,
            ),
        ] {
            let run = drive_local_app_build(
                local_app_args(Some(strategy), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                dom_design()
                            } else if prompt.contains("Generate the complete React implementation")
                                || prompt.contains("Repair the findings")
                            {
                                r#"{"ok":true,"preview_url":"http://preview/repaired","summary":"built"}"#.to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#.to_string()
                            } else {
                                panic!("unexpected prompt for {strategy}: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let error = run
                .outcome
                .expect_err("verification findings must fail the workflow");
            assert_eq!(run.prompts.len(), expected_calls, "{strategy}");
            assert_eq!(run.phases, expected_phases, "{strategy}");
            assert_eq!(
                run.prompts
                    .iter()
                    .filter(|prompt| prompt.contains("Repair the findings"))
                    .count(),
                1,
                "{strategy} must not run a second repair cycle"
            );
            assert!(
                error
                    .to_string()
                    .contains("verification still has findings")
                    && error.to_string().contains("button is clipped"),
                "unexpected workflow error for {strategy}: {error}"
            );
        }
    }

    #[test]
    fn local_app_build_thorough_allows_two_repairs_before_success() {
        let verification_round = Rc::new(Cell::new(0_u8));
        let build_round = Rc::new(Cell::new(0_u8));
        let next_verification_round = verification_round.clone();
        let next_build_round = build_round.clone();
        let run = drive_local_app_build(
            local_app_args(Some("thorough"), None, None, None),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            dom_design()
                        } else if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/initial","summary":"built"}"#.to_string()
                        } else if prompt.contains("Repair the findings") {
                            let round = next_build_round.get();
                            next_build_round.set(round + 1);
                            format!(
                                r#"{{"ok":true,"preview_url":"http://preview/repaired-{round}","summary":"rebuilt"}}"#
                            )
                        } else if prompt.contains("Invoke $frontend-qa") {
                            let round = next_verification_round.get();
                            next_verification_round.set(round + 1);
                            if round < 2 {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#.to_string()
                            } else {
                                r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#.to_string()
                            }
                        } else {
                            panic!("unexpected prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run.outcome.expect("thorough workflow executes");
        let result: Value = serde_json::from_str(outcome.result.as_deref().expect("result"))
            .expect("workflow returns JSON");
        assert_eq!(verification_round.get(), 3);
        assert_eq!(build_round.get(), 2);
        assert_eq!(run.prompts.len(), 7);
        assert_eq!(result["ok"], true);
        assert_eq!(result["strategy"], "thorough");
        assert_eq!(result["verification_mode"], "full-matrix");
        assert_eq!(result["repair_rounds"], 2);
        assert_eq!(result["agent_calls"], 7);
        assert_eq!(result["preview_url"], "http://preview/repaired-1");
    }

    #[test]
    fn local_app_build_rejects_success_without_a_preview_url() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");

        let error = workflow::run_with_progress(
            descriptor.script,
            move |prompts: &[String], _options: &[String]| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            dom_design()
                        } else if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"summary":"built without returning the runtime URL"}"#
                                .to_string()
                        } else {
                            panic!("a missing preview URL must fail before verification: {prompt}");
                        }
                    })
                    .collect()
            },
            |_| {},
            None,
            false,
            Some(local_app_args(None, None, None, None)),
            None,
        )
        .expect_err("a successful build without preview_url must fail the workflow");

        assert!(
            error
                .to_string()
                .contains("initial build succeeded without a preview_url"),
            "unexpected workflow error: {error}"
        );
    }

    /// Drive the script with a canned dispatcher and return the terminal
    /// error, so each false-success path below is one table row.
    fn run_local_app_build(
        reply: impl Fn(&str) -> String + Clone + 'static,
    ) -> Result<String, String> {
        drive_local_app_build(
            local_app_args(None, None, None, None),
            move |prompts, _options| prompts.iter().map(|prompt| reply(prompt)).collect(),
        )
        .outcome
        .map(|outcome| outcome.result.unwrap_or_default())
        .map_err(|error| error.to_string())
    }

    /// The happy-path reply table; `dead` names the ONE stage whose agent
    /// dies (the host hands back `WF_NULL_SENTINEL`, which resolves to
    /// `null` in the script — it does NOT throw on its own).
    fn local_app_reply(prompt: &str, dead: &str) -> String {
        let anchor = if prompt.contains("Act as the local app design lead") {
            "design"
        } else if prompt.contains("Generate the complete React implementation") {
            "generate-build"
        } else if prompt.contains("Invoke $frontend-qa") {
            "verify"
        } else if prompt.contains("Repair the findings") {
            "repair-build"
        } else {
            panic!("unexpected local-app workflow prompt: {prompt}")
        };
        if anchor == dead {
            return workflow::WF_NULL_SENTINEL.to_string();
        }
        match anchor {
            "design" => {
                dom_design()
            }
            "generate-build" | "repair-build" => r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#.to_string(),
            "verify" => r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#.to_string(),
            other => other.to_string(),
        }
    }

    /// `agent()` resolves a dead/killed subagent to `null` WITHOUT throwing,
    /// and the task handler maps any `Ok` return to `Completed`. Every phase
    /// whose result feeds a later prompt or the verdict must therefore throw
    /// on `null`, or the user is told a never-implemented app built fine.
    #[test]
    fn local_app_build_throws_when_any_phase_agent_dies() {
        for (dead, expected) in [
            ("design", "the design step"),
            ("generate-build", "initial build"),
            ("verify", "the verification step"),
        ] {
            let dead = dead.to_string();
            let error = run_local_app_build(move |prompt| local_app_reply(prompt, &dead))
                .expect_err("a dead agent must fail the workflow");
            assert!(
                error.contains(expected) && error.contains("produced no result"),
                "dead {expected} produced the wrong error: {error}"
            );
        }
    }

    /// A self-declared build failure that survives both repair rounds must
    /// throw too — returning `{ok:false}` would still be reported Completed.
    #[test]
    fn local_app_build_throws_when_the_build_never_succeeds() {
        let run = drive_local_app_build(
            local_app_args(None, None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":false,"preview_url":"","summary":"vite build exited 1"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":false,"findings":["no preview to check"],"checked_matrix":[],"browser_available":false,"webview_checked":false,"degraded_verification":true,"data_roundtrip":{"status":"failed","collections":[],"evidence":"preview unavailable"},"render_check":{"status":"failed","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"preview unavailable"},"summary":"cannot verify"}"#
                                .to_string()
                        } else {
                            local_app_reply(prompt, "")
                        }
                        })
                        .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("a build that never succeeds must fail the workflow");
        assert_eq!(
            run.prompts.len(),
            5,
            "a failed build still gets the configured balanced repair round"
        );
        assert_eq!(
            run.prompts
                .iter()
                .filter(|prompt| prompt.contains("Repair the findings"))
                .count(),
            1,
            "the failed build must dispatch one repair call before the terminal error"
        );
        assert!(
            error.to_string().contains("the build did not succeed")
                && error.to_string().contains("vite build exited 1"),
            "unexpected workflow error: {error}"
        );
    }

    /// A green build whose QA still has findings after the two allowed repair
    /// rounds is a FAILED build, not a Completed one with a sad summary.
    #[test]
    fn local_app_build_throws_when_verification_never_passes() {
        let error = drive_local_app_build(
            local_app_args(Some("thorough"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"needs repair"}"#.to_string()
                        } else {
                            local_app_reply(prompt, "")
                        }
                    })
                    .collect()
            },
        )
        .outcome
        .map(|outcome| outcome.result.unwrap_or_default())
        .map_err(|error| error.to_string())
        .expect_err("unresolved findings must fail the workflow");
        assert!(
            error.contains("verification still has findings")
                && error.contains("button is clipped"),
            "unexpected workflow error: {error}"
        );
    }

    /// The spec is the user's confirmation. Only a non-empty string or a
    /// non-array object with at least one own field is a usable confirmation;
    /// all other JSON types must fail before the first agent call.
    #[test]
    fn local_app_build_refuses_to_invent_a_missing_spec() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        for args in [
            r#"{"app_id":"test-app"}"#,
            r#"{"app_id":"test-app","spec":""}"#,
            r#"{"app_id":"test-app","spec":{}}"#,
            r#"{"app_id":"test-app","spec":"   "}"#,
            r#"{"app_id":"test-app","spec":"{}"}"#,
            r#"{"app_id":"test-app","spec":" { } "}"#,
            r#"{"app_id":"test-app","spec":" [] "}"#,
            r#"{"app_id":"test-app","spec":" [ ] "}"#,
            r#"{"app_id":"test-app","spec":"  \"\"  "}"#,
            r#"{"app_id":"test-app","spec":" null "}"#,
            r#"{"app_id":"test-app","spec":" false "}"#,
            r#"{"app_id":"test-app","spec":" 0 "}"#,
            r#"{"app_id":"test-app","spec":[]}"#,
            r#"{"app_id":"test-app","spec":[{"brief":"build it"}]}"#,
            r#"{"app_id":"test-app","spec":true}"#,
            r#"{"app_id":"test-app","spec":false}"#,
            r#"{"app_id":"test-app","spec":0}"#,
            r#"{"app_id":"test-app","spec":42}"#,
            r#"{"app_id":"test-app","spec":null}"#,
        ] {
            let error = workflow::run_with_progress(
                descriptor.script,
                |prompts: &[String], _options: &[String]| {
                    panic!("an unconfirmed spec must fail before any agent runs: {prompts:?}")
                },
                |_| {},
                None,
                false,
                Some(args.to_string()),
                None,
            )
            .expect_err("an empty spec must fail the workflow");
            assert!(
                error.to_string().contains("refusing to invent one"),
                "args {args} produced the wrong error: {error}"
            );
        }
    }

    #[test]
    fn local_app_build_accepts_a_nonempty_spec_object() {
        let run = drive_local_app_build(
            json!({
                "app_id": "test-app",
                "spec": {"brief": "build a useful app"},
                "strategy": "fast",
                "expected_writable_collections": [],
                "runtime_profile": runtime_profile_binding("react_dom"),
            })
            .to_string(),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/spec-object","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#
                                .to_string()
                        } else {
                            panic!("unexpected prompt for nonempty object spec: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run
            .outcome
            .expect("a nonempty object spec should be accepted");
        assert_eq!(run.prompts.len(), 2);
        let generation_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Generate the complete React implementation"))
            .expect("generation prompt");
        assert!(generation_prompt.contains(r#"{"brief":"build a useful app"}"#));
        assert!(outcome.result.is_some());
    }

    #[test]
    fn local_app_build_accepts_nonempty_serialized_json_string_specs() {
        let run = drive_local_app_build(
            json!({
                "app_id": "test-app",
                "spec": r#""build a useful app""#,
                "strategy": "fast",
                "expected_writable_collections": [],
                "runtime_profile": runtime_profile_binding("react_dom"),
            })
            .to_string(),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/spec-string","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#
                                .to_string()
                        } else {
                            panic!("unexpected prompt for serialized string spec: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run
            .outcome
            .expect("a nonempty serialized string spec should be accepted");
        assert_eq!(run.prompts.len(), 2);
        let generation_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Generate the complete React implementation"))
            .expect("generation prompt");
        assert!(generation_prompt.contains(r#""build a useful app""#));
        assert!(outcome.result.is_some());
    }

    #[test]
    fn local_app_build_requires_native_webview_evidence_without_repair() {
        for webview_checked in [Some(false), None] {
            let verification = dom_verification_with_webview(webview_checked);
            let run = drive_local_app_build(
                local_app_args(Some("fast"), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Generate the complete React implementation") {
                                r#"{"ok":true,"preview_url":"http://preview/webview","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                verification.clone()
                            } else {
                                panic!("unexpected DOM webview prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let error = run
                .outcome
                .expect_err("missing or false WebView evidence must fail");
            assert!(
                error.to_string().contains("webview_checked=true"),
                "unexpected WebView evidence error: {error}"
            );
            assert_eq!(
                run.prompts.len(),
                2,
                "a terminal WebView failure must not consume a repair round"
            );
            assert_eq!(
                run.phases,
                vec!["Generate & Build".to_string(), "Verify".to_string()]
            );
            assert!(!run
                .prompts
                .iter()
                .any(|prompt| prompt.contains("Repair the findings")));
        }
    }

    #[test]
    fn local_app_build_requires_expected_collections_before_any_agent_runs() {
        let mut missing: Value =
            serde_json::from_str(&local_app_args(Some("fast"), None, None, None))
                .expect("local app args are JSON");
        missing
            .as_object_mut()
            .expect("local app args are an object")
            .remove("expected_writable_collections");

        let mut malformed_string: Value =
            serde_json::from_str(&local_app_args(Some("fast"), None, None, None))
                .expect("local app args are JSON");
        malformed_string
            .as_object_mut()
            .expect("local app args are an object")
            .insert("expected_writable_collections".to_string(), json!("items"));

        let mut malformed_item: Value =
            serde_json::from_str(&local_app_args(Some("fast"), None, None, None))
                .expect("local app args are JSON");
        malformed_item
            .as_object_mut()
            .expect("local app args are an object")
            .insert(
                "expected_writable_collections".to_string(),
                json!(["items", 42]),
            );

        for args in [missing, malformed_string, malformed_item] {
            let run = drive_local_app_build(args.to_string(), |_prompts, _options| {
                panic!("invalid expected collection args must fail before agent()")
            });
            let error = run
                .outcome
                .expect_err("expected_writable_collections is required and typed");
            assert!(
                error
                    .to_string()
                    .contains("requires args.expected_writable_collections"),
                "unexpected validation error: {error}"
            );
            assert!(run.prompts.is_empty());
            assert!(run.phases.is_empty());
        }
    }

    #[test]
    fn local_app_build_verify_prompt_carries_confirmed_spec_and_expected_collections() {
        let args = args_with_expected_collections(
            local_app_args(Some("fast"), None, None, None),
            &["items", "settings"],
        );
        let run = drive_local_app_build(args, |prompts, _options| {
            prompts
                .iter()
                .map(|prompt| {
                    if prompt.contains("Generate the complete React implementation") {
                        r#"{"ok":true,"preview_url":"http://preview/propagation","summary":"built"}"#
                            .to_string()
                    } else if prompt.contains("Invoke $frontend-qa") {
                        r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"passed","collections":["items","settings"],"evidence":"query_data returned both records"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verified"}"#
                            .to_string()
                    } else {
                        panic!("unexpected DOM propagation prompt: {prompt}");
                    }
                })
                .collect()
        });
        let outcome = run
            .outcome
            .expect("expected collection propagation should pass");
        assert!(outcome.result.is_some());
        let verify_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Invoke $frontend-qa"))
            .expect("DOM workflow has a verification prompt");
        assert!(verify_prompt.contains("build a test app"));
        assert!(verify_prompt.contains(r#"["items","settings"]"#));
    }

    #[test]
    fn local_canvas_build_verify_prompt_carries_confirmed_spec_and_expected_collections() {
        let args = args_with_expected_collections(
            local_canvas_args(Some("balanced"), Some("canvas_2d"), None, None, None),
            &["scores"],
        );
        let run = drive_local_workflow("local-canvas-build", args, |prompts, _options| {
            prompts
                .iter()
                .map(|prompt| {
                    if prompt.contains("Act as the local app design lead") {
                        canvas_design("canvas_2d")
                    } else if prompt.contains("Generate the complete drawn-surface implementation") {
                        r#"{"ok":true,"preview_url":"http://preview/propagation","summary":"built"}"#
                            .to_string()
                    } else if prompt.contains("Invoke $frontend-qa") {
                        r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"passed","collections":["scores"],"evidence":"query_data returned the score"},"render_check":{"status":"passed","canvas_surfaces":1,"frames_captured":3,"interactions_driven":["key ArrowLeft"],"evidence":"the captured frame shows the game"},"motion_check":{"status":"passed","frames_compared":2,"difference":"the ball moved"},"summary":"verified"}"#
                            .to_string()
                    } else {
                        panic!("unexpected Canvas propagation prompt: {prompt}");
                    }
                })
                .collect()
        });
        let outcome = run
            .outcome
            .expect("expected collection propagation should pass");
        assert!(outcome.result.is_some());
        let verify_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Invoke $frontend-qa"))
            .expect("Canvas workflow has a verification prompt");
        assert!(verify_prompt.contains("build a test app"));
        assert!(verify_prompt.contains(r#"["scores"]"#));
    }

    #[test]
    fn expected_collections_make_not_applicable_and_missing_passed_ids_blocking() {
        let cases = [
            (
                r#"{"status":"not_applicable","collections":[],"evidence":"the verifier skipped the write"}"#,
                "native data round-trip was not passed",
            ),
            (
                r#"{"status":"passed","collections":["items"],"evidence":"query_data returned items"}"#,
                "native data round-trip passed without expected writable collection(s): settings",
            ),
        ];
        for (roundtrip, expected) in cases {
            let args = args_with_expected_collections(
                local_app_args(Some("fast"), None, None, None),
                &["items", "settings"],
            );
            let verification = format!(
                r#"{{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{roundtrip},"render_check":{{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"}},"summary":"verifier incorrectly passed"}}"#
            );
            let run = drive_local_app_build(args, move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/roundtrip","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            verification.clone()
                        } else {
                            panic!("unexpected roundtrip prompt: {prompt}");
                        }
                    })
                    .collect()
            });
            let error = run
                .outcome
                .expect_err("expected collection roundtrip failure must block");
            assert!(
                error.to_string().contains(expected),
                "unexpected error: {error}"
            );
            assert_eq!(
                run.prompts.len(),
                4,
                "the finding must buy fast's repair round"
            );
            let repair_prompt = run
                .prompts
                .iter()
                .find(|prompt| prompt.contains("Repair the findings"))
                .expect("expected a repair prompt");
            assert_eq!(repair_prompt.matches(expected).count(), 1);
        }
    }

    #[test]
    fn structured_verifier_findings_fail_closed_instead_of_being_dropped() {
        let verification = r#"{"ok":true,"findings":[{"id":"cta-clipped","severity":"blocker","target":"src/screens/home.jsx","evidence":"the primary call to action is clipped under the safe area"}],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verifier returned a structured finding"}"#;
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/structured-findings","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            verification.to_string()
                        } else {
                            panic!("unexpected structured finding prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("structured verifier findings must remain blocking");
        assert!(
            error.to_string().contains(
                "verification returned a structured finding (blocker at src/screens/home.jsx)"
            ),
            "unexpected structured finding error: {error}"
        );
        assert_eq!(
            run.prompts.len(),
            4,
            "a structured finding must buy fast's repair round"
        );
        let repair_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Repair the findings"))
            .expect("expected a repair prompt");
        assert!(
            repair_prompt.contains(r#""severity":"blocker""#)
                && repair_prompt.contains(r#""target":"src/screens/home.jsx""#),
            "repair prompt must retain the verifier's structured finding payload"
        );
    }

    /// The element-level normalizer fails closed on every malformed finding,
    /// but the CONTAINER was still coerced to `[]` — so a verifier that answers
    /// `findings: "<one sentence>"` had its defect deleted and the build passed.
    #[test]
    fn nonarray_verifier_findings_container_fails_closed_instead_of_being_dropped() {
        let verification = r#"{"ok":true,"findings":"the settings button is clipped on iphone se","checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verifier returned a bare string findings container"}"#;
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/nonarray-findings","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            verification.to_string()
                        } else {
                            panic!("unexpected non-array finding prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("a non-array findings container must remain blocking");
        assert!(
            error
                .to_string()
                .contains("verification returned a non-array findings container"),
            "unexpected non-array finding error: {error}"
        );
        assert_eq!(
            run.prompts.len(),
            4,
            "a malformed findings container must buy fast's repair round"
        );
    }

    /// A revision has to reach the stages that JUDGE the result, not only the
    /// one that designs it. Verify and Repair label this spec the source of
    /// truth, so a pre-revision spec under that label makes them file the
    /// revision itself as a defect. The `fast` siblings above cover the
    /// design-skipped path; this covers every strategy that runs Design.
    #[test]
    fn a_revision_reaches_generation_and_verification_when_design_runs() {
        for strategy in ["balanced", "thorough"] {
            let run = drive_local_app_build(
                local_app_args_with_spec_and_revision(
                    Some(strategy),
                    "build a test app",
                    "DESIGNED_REVISION_SENTINEL",
                ),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                r#"{"targets":[{"os":"ios","form_factor":"phone"}],"summary":"designed"}"#
                                    .to_string()
                            } else if prompt.contains("Generate the complete React implementation") {
                                assert!(
                                    prompt.contains("DESIGNED_REVISION_SENTINEL"),
                                    "generation must receive the revision feedback"
                                );
                                r#"{"ok":true,"preview_url":"http://preview/designed-revision","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                assert!(
                                    prompt.contains("DESIGNED_REVISION_SENTINEL"),
                                    "verification must receive the revision feedback"
                                );
                                dom_verification_with_webview(Some(true))
                            } else {
                                panic!("unexpected designed revision prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            run.outcome.unwrap_or_else(|error| {
                panic!("{strategy} with a revision must complete: {error}")
            });
        }
    }

    #[test]
    fn whitespace_verifier_findings_fail_closed_instead_of_being_dropped() {
        let verification = r#"{"ok":true,"findings":["   "],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"render_check":{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"},"summary":"verifier returned whitespace findings"}"#;
        let run = drive_local_app_build(
            local_app_args(Some("fast"), None, None, None),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Generate the complete React implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/whitespace-findings","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            verification.to_string()
                        } else {
                            panic!("unexpected whitespace finding prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("whitespace verifier findings must remain blocking");
        assert!(
            error
                .to_string()
                .contains("verification returned an empty string finding"),
            "unexpected whitespace finding error: {error}"
        );
        assert_eq!(
            run.prompts.len(),
            4,
            "a whitespace finding must buy fast's repair round"
        );
        let repair_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Repair the findings"))
            .expect("expected a repair prompt");
        assert!(
            repair_prompt.contains("verification returned an empty string finding"),
            "repair prompt must retain the synthesized whitespace-finding error"
        );
    }

    #[test]
    fn canvas_zero_surfaces_blocks_even_when_render_and_motion_pass() {
        let verification = canvas_verification(
            r#"{"status":"passed","canvas_surfaces":0,"frames_captured":3,"interactions_driven":["key ArrowLeft"],"evidence":"the captured frame shows the game"}"#,
            CANVAS_MOTION_OK,
        );
        let run = drive_local_workflow(
            "local-canvas-build",
            local_canvas_args(Some("balanced"), Some("canvas_2d"), None, None, None),
            move |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            canvas_design("canvas_2d")
                        } else if prompt.contains("Generate the complete drawn-surface implementation")
                            || prompt.contains("Repair the findings")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/canvas-surface","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            verification.clone()
                        } else {
                            panic!("unexpected zero-surface prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("a canvas workflow with no reported surface must fail");
        assert!(
            error
                .to_string()
                .contains("canvas render check reported no canvas surface"),
            "unexpected zero-surface error: {error}"
        );
        assert_eq!(
            run.prompts.len(),
            5,
            "the surface finding must buy one repair round"
        );
        let repair_prompt = run
            .prompts
            .iter()
            .find(|prompt| prompt.contains("Repair the findings"))
            .expect("zero-surface finding reaches repair");
        assert_eq!(
            repair_prompt
                .matches("canvas render check reported no canvas surface")
                .count(),
            1
        );
    }

    struct LocalAppHarness {
        outcome: Result<workflow::RunOutcome, workflow::WorkflowError>,
        prompts: Vec<String>,
        phases: Vec<String>,
    }

    fn runtime_profile_binding(family: &str) -> Value {
        let contract_sha256 = match family {
            "react_dom" => "1".repeat(64),
            "canvas_2d" => "2".repeat(64),
            "three_3d" => "3".repeat(64),
            "phaser_2d" => "4".repeat(64),
            "babylon_3d" => "5".repeat(64),
            _ => "a".repeat(64),
        };
        json!({
            "family": family,
            "revision": 1,
            "contract_sha256": contract_sha256,
        })
    }

    fn local_app_args(
        strategy: Option<&str>,
        complexity: Option<Value>,
        model: Option<&str>,
        revision_prompt: Option<&str>,
    ) -> String {
        let mut args = json!({
            "app_id": "test-app",
            "spec": "build a test app",
            "expected_writable_collections": [],
            "runtime_profile": runtime_profile_binding("react_dom"),
        });
        let object = args.as_object_mut().expect("object");
        if let Some(strategy) = strategy {
            object.insert("strategy".to_string(), Value::String(strategy.to_string()));
        }
        if let Some(complexity) = complexity {
            object.insert("complexity".to_string(), complexity);
        }
        if let Some(model) = model {
            object.insert("model".to_string(), Value::String(model.to_string()));
        }
        if let Some(revision_prompt) = revision_prompt {
            object.insert(
                "revision_prompt".to_string(),
                Value::String(revision_prompt.to_string()),
            );
        }
        args.to_string()
    }

    fn local_app_args_with_spec(strategy: Option<&str>, spec: &str) -> String {
        let mut args: Value = serde_json::from_str(&local_app_args(strategy, None, None, None))
            .expect("local app args are JSON");
        args.as_object_mut()
            .expect("local app args are an object")
            .insert("spec".to_string(), Value::String(spec.to_string()));
        args.to_string()
    }

    fn local_app_args_with_spec_and_revision(
        strategy: Option<&str>,
        spec: &str,
        revision_prompt: &str,
    ) -> String {
        let mut args: Value = serde_json::from_str(&local_app_args(strategy, None, None, None))
            .expect("local app args are JSON");
        let object = args.as_object_mut().expect("local app args are an object");
        object.insert("spec".to_string(), Value::String(spec.to_string()));
        object.insert(
            "revision_prompt".to_string(),
            Value::String(revision_prompt.to_string()),
        );
        args.to_string()
    }

    fn args_with_expected_collections(args: String, expected: &[&str]) -> String {
        let mut args: Value = serde_json::from_str(&args).expect("local app args are JSON");
        args.as_object_mut()
            .expect("local app args are an object")
            .insert("expected_writable_collections".to_string(), json!(expected));
        args.to_string()
    }

    fn local_canvas_args(
        strategy: Option<&str>,
        runtime_family: Option<&str>,
        complexity: Option<Value>,
        model: Option<&str>,
        revision_prompt: Option<&str>,
    ) -> String {
        let mut args: Value = serde_json::from_str(&local_app_args(
            strategy,
            complexity,
            model,
            revision_prompt,
        ))
        .expect("local app args are JSON");
        let object = args.as_object_mut().expect("local app args are an object");
        if let Some(runtime_family) = runtime_family {
            object.insert(
                "runtime_profile".to_string(),
                runtime_profile_binding(runtime_family),
            );
        } else {
            object.remove("runtime_profile");
        }
        args.to_string()
    }

    fn local_canvas_args_with_spec(
        strategy: Option<&str>,
        runtime_family: Option<&str>,
        spec: &str,
    ) -> String {
        let mut args: Value = serde_json::from_str(&local_canvas_args(
            strategy,
            runtime_family,
            None,
            None,
            None,
        ))
        .expect("local canvas args are JSON");
        args.as_object_mut()
            .expect("local canvas args are an object")
            .insert("spec".to_string(), Value::String(spec.to_string()));
        args.to_string()
    }

    fn dom_design() -> String {
        json!({
            "targets": [{
                "os": "android",
                "form_factor": "phone",
                "presentation": "Ionic Material layout with a compact list and primary action",
                "navigation": "Home is the root; pushed detail screens use header back",
            }],
            "information_architecture": {
                "screens": ["home"],
                "states": {
                    "loading": "Show an inline progress indicator",
                    "empty": "Show a first-use prompt",
                    "error": "Show a retryable error message",
                    "success": "Show the populated list",
                    "permission": "Explain and retry denied host access",
                },
            },
            "visual_system": {
                "design_direction": "Quiet, native-feeling utility UI",
                "tokens": {
                    "color": ["primary", "background", "error"],
                    "typography": ["system body", "system title"],
                    "spacing": ["8", "16", "24"],
                    "radius": ["12"],
                    "elevation": ["subtle card"],
                    "motion": ["short ease"],
                    "safe_area": ["host insets"],
                },
            },
            "interaction_model": {
                "pointer_touch": ["tap primary action"],
                "keyboard_mouse": ["Enter activates focused action"],
                "back": ["header back and system back return to home"],
                "reduced_motion": ["remove nonessential transitions"],
            },
            "adapter_boundary": {
                "shared_logic": ["data loading and validation"],
                "platform_specific_shell": ["Ionic density and platform tokens"],
            },
            "acceptance_checks": ["loading, empty, error, permission, and success states render"],
            "summary": "designed",
        })
        .to_string()
    }

    fn canvas_design(runtime_family: &str) -> String {
        json!({
            "targets": [{
                "os": "ios",
                "form_factor": "iphone",
                "presentation": "Full-bleed drawn playfield with native-feeling overlay controls",
                "navigation": "No routes; phase changes and pause menu stay on the same surface",
            }],
            "runtime_family": runtime_family,
            "drawn_surface": {
                "scene": "A ball, paddle, and brick field drawn into one canvas",
                "layers": ["background", "game entities", "effects"],
                "mechanics": ["dt-based movement", "collision", "score"],
            },
            "hud_overlay": {
                "placement_safe_area": "HUD follows top safe area and leaves the bottom-leading host square clear",
                "phase_pause_relation": "Menu and pause overlays stop simulation; playing shows live HUD",
                "live_text": ["score", "phase", "pause status"],
                "reduced_motion": "Keep input and state changes while reducing decorative animation",
                "platform_treatment": ["iOS safe-area spacing", "keyboard parity for desktop QA"],
            },
            "loop": "ball and paddle advance by clamped dt",
            "phases": ["menu", "playing", "over"],
            "inputs": ["left: pointer drag / ArrowLeft"],
            "end_conditions": "ball is lost",
            "frame_budget": "one ball, one paddle, and seven brick rows per frame",
            "summary": "brick breaker",
        })
        .to_string()
    }

    fn dom_verification_with_webview(webview_checked: Option<bool>) -> String {
        let webview_field = webview_checked
            .map(|checked| format!(r#", "webview_checked":{checked}"#))
            .unwrap_or_default();
        format!(
            r#"{{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true{webview_field},"degraded_verification":false,"data_roundtrip":{{"status":"not_applicable","collections":[],"evidence":"no writable collections"}},"render_check":{{"status":"not_applicable","canvas_surfaces":0,"frames_captured":0,"interactions_driven":[],"evidence":"DOM snapshot carried the evidence"}},"summary":"verified"}}"#
        )
    }

    /// A canvas verification that is CLEAN by the DOM workflow's rules.
    ///
    /// `data_roundtrip: not_applicable` is the normal answer for a drawn app —
    /// which is exactly why it cannot be the gate.
    fn canvas_verification_with_webview(
        render_check: &str,
        motion_check: &str,
        webview_checked: Option<bool>,
    ) -> String {
        let webview_field = webview_checked
            .map(|checked| format!(r#", "webview_checked":{checked}"#))
            .unwrap_or_default();
        format!(
            r#"{{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":false{webview_field},"degraded_verification":false,"data_roundtrip":{{"status":"not_applicable","collections":[],"evidence":"no writable collections"}},"render_check":{render_check},"motion_check":{motion_check},"summary":"verified"}}"#
        )
    }

    fn canvas_verification(render_check: &str, motion_check: &str) -> String {
        canvas_verification_with_webview(render_check, motion_check, Some(true))
    }

    const CANVAS_RENDER_OK: &str = r#"{"status":"passed","canvas_surfaces":1,"frames_captured":3,"interactions_driven":["key ArrowLeft","pointer 120,300"],"evidence":"a magenta paddle on a dark field with seven rows of bricks above it"}"#;
    const CANVAS_MOTION_OK: &str = r#"{"status":"passed","frames_compared":2,"difference":"the ball moved from the upper left toward the paddle between the two captures"}"#;

    /// The canvas workflow must be reachable by name and pin the contract that
    /// makes a drawn surface buildable at all.
    #[test]
    fn local_canvas_build_is_model_invocable_and_pins_its_contract() {
        let descriptor = BUILTIN_WORKFLOWS
            .get("local-canvas-build")
            .expect("the drawn-surface workflow is a built-in");
        assert!(
            !descriptor.manual_only,
            "the skill invokes it by name, so it cannot be manual-only"
        );
        for anchor in [
            // The scaffold's entry point and the three things the frame-loop
            // helper already solves. A generated app that re-derives them looks
            // right in a desktop preview and fails on a device.
            "app/screens/game-screen.jsx",
            "createFrameLoop in lib/frame-loop.js",
            "device pixel ratio",
            "clamps the first frame after a resume",
            // Per-frame state in the store is the difference between a game and
            // a slideshow.
            "Keep per-frame simulation state in a ref",
            // The gate that replaces `data_roundtrip` for this shape.
            "There is no not_applicable: this app draws its whole interface",
            "only a difference proves the loop is running",
            // Automation drives discrete keys; a drag-only app cannot be
            // verified through the host path at all.
            "cannot be driven by the host automation path",
            // Shared with the DOM shape, and equally load-bearing here.
            "NEVER from @ionic/core/components",
            "locked `three@0.185.1` runtime",
            "runtime_family",
            "canvas_2d",
            "three_3d",
            "phaser_2d",
            "babylon_3d",
            "$canvas-2d-local-app",
            "$threejs-local-app",
            "$phaser-2d-local-app",
            "$babylon-3d-local-app",
            "$frontend-qa",
            // Host chrome, in the drawn-surface spelling. This shape has no
            // IonPage and no IonHeader from the scaffold either, so an app that
            // does not draw its own exit has none at all — the floating host
            // control is then literally the only affordance on the screen.
            "The host draws NO chrome around a running app",
            "has to be drawn on the canvas or overlaid on top of it by this app",
            // The host control and the square it owns, derived the same way as
            // the DOM shape (larger client, Android's collapsed 72 x 64 rounded
            // up to 80); the JS comment beside the line shows the arithmetic for
            // both clients. A drawn surface has no layout engine to route around
            // that corner, so the generator has to be told the rectangle.
            "The host floats ONE control over the running page in the BOTTOM-LEADING corner",
            "the leading 80pt by the bottom 80pt measured from the safe area",
            // Design stage: for a surface with no chrome at all, where the
            // title, pause and exit affordances go IS the layout decision.
            "keep the bottom-leading corner free of them",
        ] {
            assert!(
                descriptor.script.contains(anchor),
                "missing canvas contract anchor: {anchor}"
            );
        }
        assert!(
            descriptor
                .script
                .contains("minimum: 0,\n          description: 'The truthful canvasCount"),
            "Canvas render_check must accept truthful canvas_surfaces: 0"
        );
        assert!(
            descriptor
                .script
                .contains("canvas render check reported no canvas surface"),
            "Canvas runtime gate must still reject zero surfaces"
        );
        // The shared core really is shared, not copied.
        assert!(descriptor
            .script
            .contains("Shared driver for every local-app build workflow"));
        assert_eq!(
            descriptor.script.matches("await runAgent(").count(),
            descriptor.script.matches("throwOnError: true").count(),
            "every agent stage must fail loudly rather than return null"
        );
    }

    /// `fast` is the one strategy that skips Design, and a drawn surface keeps
    /// almost all of its difficulty there. The skill advises against it; this
    /// refuses it, because the complexity rubric scores a single-surface app at
    /// zero and lands on `fast` by arithmetic.
    #[test]
    fn the_canvas_workflow_refuses_the_strategy_that_skips_design() {
        let run = drive_local_workflow(
            "local-canvas-build",
            local_canvas_args(Some("fast"), Some("canvas_2d"), None, None, None),
            |prompts, _options| {
                panic!("no agent may run before the strategy is rejected: {prompts:?}")
            },
        );
        let error = run.outcome.expect_err("fast must be refused");
        assert!(
            error
                .to_string()
                .contains("does not accept the fast strategy"),
            "the refusal must name the strategy: {error}"
        );
        assert!(run.prompts.is_empty());
    }

    #[test]
    fn canvas_runtime_profile_is_required_and_validated_before_any_agent() {
        for (runtime_family, expected) in [
            (None, "requires args.runtime_profile"),
            (Some("webgl"), "requires args.runtime_profile.family"),
            (Some("Three.js"), "requires args.runtime_profile.family"),
        ] {
            let run = drive_local_workflow(
                "local-canvas-build",
                local_canvas_args(Some("balanced"), runtime_family, None, None, None),
                |_prompts, _options| {
                    panic!("invalid runtime_profile must fail before any agent starts")
                },
            );
            let error = run
                .outcome
                .expect_err("missing or invalid runtime_profile must be rejected");
            assert!(
                error.to_string().contains(expected),
                "unexpected runtime_profile validation error: {error}"
            );
            assert!(
                run.prompts.is_empty(),
                "runtime_profile validation must happen before an agent call"
            );
        }
    }

    #[test]
    fn local_canvas_build_requires_native_webview_evidence_without_repair() {
        for webview_checked in [Some(false), None] {
            let verification = canvas_verification_with_webview(
                CANVAS_RENDER_OK,
                CANVAS_MOTION_OK,
                webview_checked,
            );
            let run = drive_local_workflow(
                "local-canvas-build",
                local_canvas_args(Some("balanced"), Some("canvas_2d"), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                canvas_design("canvas_2d")
                            } else if prompt
                                .contains("Generate the complete drawn-surface implementation")
                            {
                                r#"{"ok":true,"preview_url":"http://preview/webview","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                verification.clone()
                            } else {
                                panic!("unexpected Canvas webview prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let error = run
                .outcome
                .expect_err("missing or false WebView evidence must fail");
            assert!(
                error.to_string().contains("webview_checked=true"),
                "unexpected WebView evidence error: {error}"
            );
            assert_eq!(
                run.prompts.len(),
                3,
                "a terminal WebView failure must not consume a repair round"
            );
            assert_eq!(
                run.phases,
                vec![
                    "Design".to_string(),
                    "Generate & Build".to_string(),
                    "Verify".to_string()
                ]
            );
            assert!(!run
                .prompts
                .iter()
                .any(|prompt| prompt.contains("Repair the findings")));
        }
    }

    #[test]
    fn canvas_design_runtime_family_must_match_the_persisted_profile_before_generation() {
        let run = drive_local_workflow(
            "local-canvas-build",
            local_canvas_args(Some("balanced"), Some("three_3d"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            canvas_design("canvas_2d")
                        } else {
                            panic!("design mismatch must fail before generation: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let error = run
            .outcome
            .expect_err("a design runtime_family mismatch must fail");
        assert!(error.to_string().contains("design runtime_family mismatch"));
        assert!(error.to_string().contains("expected three_3d"));
        assert!(error.to_string().contains("canvas_2d"));
        assert_eq!(run.prompts.len(), 1);
    }

    #[test]
    fn args_renderer_cannot_override_the_persisted_canvas_runtime_profile() {
        let mut args: Value = serde_json::from_str(&local_canvas_args_with_spec(
            Some("balanced"),
            Some("canvas_2d"),
            "build a 2D game, but renderer=threejs should override the task",
        ))
        .expect("canvas args are JSON");
        args.as_object_mut()
            .expect("canvas args are an object")
            .insert("renderer".to_string(), Value::String("threejs".to_string()));
        let run = drive_local_workflow(
            "local-canvas-build",
            args.to_string(),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            canvas_design("canvas_2d")
                        } else if prompt.contains("Generate the complete drawn-surface implementation") {
                            assert!(prompt.contains("$canvas-2d-local-app"));
                            assert!(!prompt.contains("$threejs-local-app"));
                            assert!(!prompt.contains("$phaser-2d-local-app"));
                            assert!(!prompt.contains("$babylon-3d-local-app"));
                            r#"{"ok":true,"preview_url":"http://preview/malicious-spec","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            canvas_verification(CANVAS_RENDER_OK, CANVAS_MOTION_OK)
                        } else {
                            panic!("unexpected malicious-spec prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run
            .outcome
            .expect("malicious spec wording must not change runtime routing");
        assert!(outcome.result.is_some());
        assert_eq!(run.prompts.len(), 3);
    }

    /// A frozen surface renders a perfectly good first frame. Without the motion
    /// gate a game whose loop never started passes verification on a still
    /// image — the exact failure `render_check` alone cannot see.
    #[test]
    fn a_missing_or_failed_motion_check_blocks_and_buys_a_repair_round() {
        let cases = [
            (
                canvas_verification(
                    CANVAS_RENDER_OK,
                    r#"{"status":"failed","frames_compared":2,"difference":"the two captures are identical"}"#,
                ),
                "motion check failed",
            ),
            (
                canvas_verification(
                    CANVAS_RENDER_OK,
                    r#"{"status":"passed","frames_compared":1,"difference":"looked fine"}"#,
                ),
                "compared fewer than two frames",
            ),
        ];
        for (verification, expected) in cases {
            let run = drive_local_workflow(
                "local-canvas-build",
                local_canvas_args(Some("balanced"), Some("canvas_2d"), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                canvas_design("canvas_2d")
                            } else if prompt
                                .contains("Generate the complete drawn-surface implementation")
                                || prompt.contains("Repair the findings")
                            {
                                r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                verification.clone()
                            } else {
                                panic!("unexpected prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let error = run
                .outcome
                .expect_err("a surface that never moved must not pass");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in: {error}"
            );
            // The COUNT, not just the message: a motion finding is a source
            // defect a repair round can fix, so it has to buy one exactly like a
            // render finding does. `balanced` runs design, so the sequence is
            // design, generate-build, verify, repair-build, verify = 5.
            assert_eq!(
                run.prompts.len(),
                5,
                "a motion finding must reach a repair round before the build fails; prompts={:#?}",
                run.prompts
            );
        }
    }

    /// The happy path, so the gates above are shown to be refusable rather than
    /// unconditional.
    #[test]
    fn a_canvas_app_that_rendered_and_moved_completes() {
        let run = drive_local_workflow(
            "local-canvas-build",
            local_canvas_args(Some("balanced"), Some("canvas_2d"), None, None, None),
            |prompts, _options| {
                prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.contains("Act as the local app design lead") {
                            canvas_design("canvas_2d")
                        } else if prompt
                            .contains("Generate the complete drawn-surface implementation")
                        {
                            r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#
                                .to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            canvas_verification(CANVAS_RENDER_OK, CANVAS_MOTION_OK)
                        } else {
                            panic!("unexpected prompt: {prompt}");
                        }
                    })
                    .collect()
            },
        );
        let outcome = run.outcome.expect("a rendered, moving app completes");
        let value: Value =
            serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                .expect("json");
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["preview_url"], serde_json::json!("http://preview/ok"));
        assert_eq!(value["repair_rounds"], serde_json::json!(0));
        // design, generate-build, verify.
        assert_eq!(run.prompts.len(), 3, "prompts={:#?}", run.prompts);
    }

    #[test]
    fn canvas_runtime_profile_routes_to_one_matching_specialist_then_shared_reviewers() {
        for (runtime_family, expected, other_skills) in [
            (
                "canvas_2d",
                "$canvas-2d-local-app",
                [
                    "$threejs-local-app",
                    "$phaser-2d-local-app",
                    "$babylon-3d-local-app",
                ],
            ),
            (
                "three_3d",
                "$threejs-local-app",
                [
                    "$canvas-2d-local-app",
                    "$phaser-2d-local-app",
                    "$babylon-3d-local-app",
                ],
            ),
            (
                "phaser_2d",
                "$phaser-2d-local-app",
                [
                    "$canvas-2d-local-app",
                    "$threejs-local-app",
                    "$babylon-3d-local-app",
                ],
            ),
            (
                "babylon_3d",
                "$babylon-3d-local-app",
                [
                    "$canvas-2d-local-app",
                    "$threejs-local-app",
                    "$phaser-2d-local-app",
                ],
            ),
        ] {
            let run = drive_local_workflow(
                "local-canvas-build",
                local_canvas_args(Some("balanced"), Some(runtime_family), None, None, None),
                move |prompts, _options| {
                    prompts
                        .iter()
                        .map(|prompt| {
                            if prompt.contains("Act as the local app design lead") {
                                assert!(
                                    prompt.contains(&format!(
                                        "runtime_family=\"{runtime_family}\""
                                    )),
                                    "design must receive the persisted {runtime_family} runtime family"
                                );
                                canvas_design(runtime_family)
                            } else if prompt.contains("Generate the complete drawn-surface implementation") {
                                assert!(
                                    prompt.contains(expected),
                                    "{runtime_family} generation must invoke {expected}"
                                );
                                assert_eq!(
                                    prompt.matches(expected).count(),
                                    1,
                                    "{runtime_family} generation invokes one runtime specialist"
                                );
                                for other in other_skills {
                                    assert!(
                                        !prompt.contains(other),
                                        "{runtime_family} generation must not invoke {other}"
                                    );
                                }
                                assert!(prompt.contains("$accessibility"));
                                assert!(prompt.contains("$react-best-practices"));
                                for design_field in [
                                    "runtime_family",
                                    "drawn_surface",
                                    "hud_overlay",
                                    "placement_safe_area",
                                    "phase_pause_relation",
                                    "live_text",
                                    "reduced_motion",
                                    "platform_treatment",
                                    "presentation",
                                    "navigation",
                                    "frame_budget",
                                ] {
                                    assert!(
                                        prompt.contains(design_field),
                                        "canvas generation must receive structured design field {design_field}"
                                    );
                                }
                                r#"{"ok":true,"preview_url":"http://preview/runtime-family","summary":"built"}"#
                                    .to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                canvas_verification(CANVAS_RENDER_OK, CANVAS_MOTION_OK)
                            } else {
                                panic!("unexpected canvas runtime-family prompt: {prompt}");
                            }
                        })
                        .collect()
                },
            );
            let outcome = run
                .outcome
                .expect("canvas runtime-family workflow should complete");
            let value: Value =
                serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                    .expect("workflow returns JSON");
            assert_eq!(value["ok"], serde_json::json!(true));
            let verify_prompt = run
                .prompts
                .iter()
                .find(|prompt| prompt.contains("Invoke $frontend-qa"))
                .expect("canvas workflow has a QA prompt");
            assert!(
                verify_prompt.contains(&format!("\"runtime_family\":\"{runtime_family}\"")),
                "QA must receive the selected runtime family from design"
            );
            assert_eq!(
                run.prompts.len(),
                3,
                "{runtime_family} prompts={:#?}",
                run.prompts
            );
        }
    }

    #[test]
    fn canvas_repairs_preserve_the_persisted_runtime_family_for_both_strategies() {
        for strategy in ["balanced", "thorough"] {
            for (runtime_family, expected, other) in [
                ("canvas_2d", "$canvas-2d-local-app", "$threejs-local-app"),
                ("three_3d", "$threejs-local-app", "$canvas-2d-local-app"),
            ] {
                let verification_round = Rc::new(Cell::new(0_u8));
                let next_verification_round = verification_round.clone();
                let run = drive_local_workflow(
                    "local-canvas-build",
                    local_canvas_args_with_spec(
                        Some(strategy),
                        Some(runtime_family),
                        "CANVAS_REPAIR_SPEC_SENTINEL",
                    ),
                    move |prompts, _options| {
                        prompts
                            .iter()
                            .map(|prompt| {
                                if prompt.contains("Act as the local app design lead") {
                                    canvas_design(runtime_family)
                                } else if prompt.contains("Generate the complete drawn-surface implementation")
                                    || prompt.contains("Repair the findings")
                                {
                                    assert!(
                                        prompt.contains(expected),
                                        "${strategy}/${runtime_family} repair route must invoke {expected}"
                                    );
                                    assert!(
                                        !prompt.contains(other),
                                        "${strategy}/${runtime_family} repair route must not invoke {other}"
                                    );
                                    assert!(prompt.contains(&format!("\"runtime_family\":\"{runtime_family}\"")));
                                    if prompt.contains("Repair the findings") {
                                        assert!(
                                            prompt.contains(
                                                "Confirmed product specification (source of truth"
                                            ) && prompt.contains("CANVAS_REPAIR_SPEC_SENTINEL"),
                                            "${strategy}/${runtime_family} repair route must receive the confirmed specification"
                                        );
                                        assert_eq!(
                                            prompt.matches("CANVAS_REPAIR_SPEC_SENTINEL").count(),
                                            1,
                                            "${strategy}/${runtime_family} repair route must receive the confirmed specification exactly once"
                                        );
                                        assert!(
                                            prompt.contains("original design payload")
                                                && prompt.contains("brick breaker")
                                                && prompt.contains("runtime_family")
                                                && prompt.contains("drawn_surface")
                                                && prompt.contains("hud_overlay"),
                                            "${strategy}/${runtime_family} repair route must retain the complete design payload"
                                        );
                                    }
                                    r#"{"ok":true,"preview_url":"http://preview/repaired-runtime-family","summary":"rebuilt"}"#
                                        .to_string()
                                } else if prompt.contains("Invoke $frontend-qa") {
                                    let round = next_verification_round.get();
                                    next_verification_round.set(round + 1);
                                    if round == 0 {
                                        format!(
                                            r#"{{"ok":false,"findings":["runtime profile test finding"],"checked_matrix":["iphone"],"browser_available":false,"webview_checked":true,"degraded_verification":true,"data_roundtrip":{{"status":"not_applicable","collections":[],"evidence":"no writable collections"}},"render_check":{{"status":"passed","canvas_surfaces":1,"frames_captured":2,"interactions_driven":["key ArrowLeft"],"evidence":"frame rendered"}},"motion_check":{{"status":"passed","frames_compared":2,"difference":"the frame changed"}},"summary":"needs repair"}}"#
                                        )
                                    } else {
                                        canvas_verification(CANVAS_RENDER_OK, CANVAS_MOTION_OK)
                                    }
                                } else {
                                    panic!("unexpected ${strategy}/${runtime_family} prompt: {prompt}");
                                }
                            })
                            .collect()
                    },
                );
                let outcome = run
                    .outcome
                    .expect("canvas repair should succeed after one repair");
                let value: Value =
                    serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                        .expect("workflow returns JSON");
                assert_eq!(value["ok"], serde_json::json!(true));
                assert_eq!(value["repair_rounds"], serde_json::json!(1));
                assert_eq!(verification_round.get(), 2);
                assert_eq!(run.prompts.len(), 5, "${strategy}/${runtime_family}");
            }
        }
    }

    fn drive_local_app_build(
        args: String,
        reply: impl Fn(&[String], &[String]) -> Vec<String> + 'static,
    ) -> LocalAppHarness {
        drive_local_workflow("local-app-build", args, reply)
    }

    /// Same harness, parameterized by workflow name.
    ///
    /// The two local-app workflows share `local_app_workflow_core.js` verbatim,
    /// so a driver that can only reach one of them would leave the shared repair
    /// loop exercised through a single shape — and the canvas gates,
    /// which live in that same loop, untested.
    fn drive_local_workflow(
        name: &str,
        args: String,
        reply: impl Fn(&[String], &[String]) -> Vec<String> + 'static,
    ) -> LocalAppHarness {
        let descriptor = BUILTIN_WORKFLOWS.get(name).expect("built-in");
        let prompts_seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let captured_prompts = prompts_seen.clone();
        let phases_seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let captured_phases = phases_seen.clone();
        let outcome = workflow::run_with_progress(
            descriptor.script,
            move |prompts: &[String], options: &[String]| {
                captured_prompts
                    .borrow_mut()
                    .extend(prompts.iter().cloned());
                reply(prompts, options)
            },
            move |progress| {
                if let workflow::Progress::Phase { title, .. } = progress {
                    captured_phases.borrow_mut().push(title.clone());
                }
            },
            None,
            false,
            Some(args),
            None,
        );
        let prompts = prompts_seen.borrow().clone();
        let phases = phases_seen.borrow().clone();
        LocalAppHarness {
            outcome,
            prompts,
            phases,
        }
    }
}
