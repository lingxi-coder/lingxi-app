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
/// then hand the confirmed spec to this workflow, which runs the deterministic
/// adaptive design/generate/build/verify sequence. Deliberately
/// model-invocable (`manual_only: false`): the skill instructs the model to
/// call it by name.
const LOCAL_APP_BUILD: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "local-app-build",
    description: "Adaptively design, generate, build, and verify a confirmed local app.",
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
            "maxRepairRounds",
            "conditionally detect ImageGen",
            "A narrow viewport alone does not prove a platform",
            "Cover the full confirmed matrix: iPhone, Android phone, iPad portrait+landscape, Android tablet portrait+landscape, and desktop",
            "LocalAppInspectUi/LocalAppActOnUi/LocalAppLogs",
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
                        r#"{"targets":[{"os":"android","form_factor":"phone"}],"summary":"designed"}"#
                            .to_string()
                    } else if prompt.contains("Generate the complete React implementation") {
                        r#"{"ok":true,"preview_url":"http://preview/first","summary":"built"}"#
                            .to_string()
                    } else if prompt.contains("Invoke $frontend-qa") {
                        r#"{"ok":true,"findings":[],"checked_matrix":["android-phone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"verified"}"#.to_string()
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
                            r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"summary":"designed"}"#.to_string()
                        } else if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"preview_url":"http://preview/initial","summary":"built"}"#.to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            let round = next_verification_round.get();
                            next_verification_round.set(round + 1);
                            if round == 0 {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"needs repair"}"#.to_string()
                            } else {
                                r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"verified"}"#.to_string()
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
                            r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"failed","collections":["items"],"evidence":"query_data did not contain the written record"},"summary":"verifier incorrectly reported success"}"#
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
                                r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"summary":"designed"}"#.to_string()
                            } else if prompt.contains("Generate the complete React implementation")
                                || prompt.contains("Repair the findings")
                            {
                                r#"{"ok":true,"preview_url":"http://preview/repaired","summary":"built"}"#.to_string()
                            } else if prompt.contains("Invoke $frontend-qa") {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"needs repair"}"#.to_string()
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
                            r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"summary":"designed"}"#.to_string()
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
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"needs repair"}"#.to_string()
                            } else {
                                r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"verified"}"#.to_string()
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
                            r#"{"targets":[{"os":"android","form_factor":"phone"}],"summary":"designed"}"#.to_string()
                        } else if prompt.contains("Generate the complete React implementation") {
                            r#"{"ok":true,"summary":"built without returning the runtime URL"}"#.to_string()
                        } else {
                            panic!("a missing preview URL must fail before verification: {prompt}");
                        }
                    })
                    .collect()
            },
            |_| {},
            None,
            false,
            Some(r#"{"app_id":"test-app","spec":"build a test app"}"#.to_string()),
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
                r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"summary":"designed"}"#
                    .to_string()
            }
            "generate-build" | "repair-build" => r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#.to_string(),
            "verify" => r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"verified"}"#.to_string(),
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
        let error = run_local_app_build(|prompt| {
            if prompt.contains("Generate the complete React implementation")
                || prompt.contains("Repair the findings")
            {
                r#"{"ok":false,"preview_url":"","summary":"vite build exited 1"}"#.to_string()
            } else if prompt.contains("Invoke $frontend-qa") {
                r#"{"ok":false,"findings":["no preview to check"],"checked_matrix":[],"browser_available":false,"webview_checked":false,"degraded_verification":true,"data_roundtrip":{"status":"failed","collections":[],"evidence":"preview unavailable"},"summary":"cannot verify"}"#.to_string()
            } else {
                local_app_reply(prompt, "")
            }
        })
        .expect_err("a build that never succeeds must fail the workflow");
        assert!(
            error.contains("the build did not succeed") && error.contains("vite build exited 1"),
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
                            r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"data_roundtrip":{"status":"not_applicable","collections":[],"evidence":"no writable collections"},"summary":"needs repair"}"#.to_string()
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

    /// The spec is the user's confirmation. `JSON.stringify(undefined ?? {})`
    /// is the truthy string `"{}"`, so the refusal has to test the coerced
    /// value, not just `input.spec`.
    #[test]
    fn local_app_build_refuses_to_invent_a_missing_spec() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        for args in [
            r#"{"app_id":"test-app"}"#,
            r#"{"app_id":"test-app","spec":""}"#,
            r#"{"app_id":"test-app","spec":{}}"#,
            r#"{"app_id":"test-app","spec":"   "}"#,
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

    struct LocalAppHarness {
        outcome: Result<workflow::RunOutcome, workflow::WorkflowError>,
        prompts: Vec<String>,
        phases: Vec<String>,
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

    fn drive_local_app_build(
        args: String,
        reply: impl Fn(&[String], &[String]) -> Vec<String> + 'static,
    ) -> LocalAppHarness {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
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
