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
/// design/dependency/generate/build/verify sequence. Deliberately
/// model-invocable (`manual_only: false`): the skill instructs the model to
/// call it by name.
const LOCAL_APP_BUILD: BuiltinWorkflowDescriptor = BuiltinWorkflowDescriptor {
    name: "local-app-build",
    description:
        "Design, dependency-check, generate, offline-build, and verify a confirmed local app.",
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
        for phase in ["Design", "Dependencies", "Generate", "Build", "Verify"] {
            assert!(
                descriptor.script.contains(&format!("title: '{phase}'")),
                "missing {phase} phase"
            );
        }
        let phase_positions =
            ["Design", "Dependencies", "Generate", "Build", "Verify"].map(|phase| {
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
            descriptor
                .script
                .contains("for (let round = 0; round <= 2; round += 1)"),
            "verification may run initially plus at most two repair rounds"
        );
        // The workspace contract must ride into EVERY agent prompt: writable
        // roots, locked files, the bridge-only rule, and the no-new-deps rule.
        for anchor in [
            "app/, src/, components/, lib/, styles/, public/",
            "lib/lingxi-bridge.js",
            "window.lingxi.v1",
            "existing Shell tool",
            "existing Git/Bash capability",
            "npm install",
            "npm uninstall",
            "npm ci",
            "package-lock digest",
            "network/command approval",
            "mcp__local_apps__build",
            "mcp__local_apps__manage_runtime",
            "two allowed repair rounds",
            "conditionally detect ImageGen",
            "Browser is also required for mobile-sized viewports",
            "inspect_ui/act_on_ui/read_logs",
            "degraded_verification",
            "npm create vite@latest . -- --template react --no-interactive",
            "npm create vite@latest . -- --template react-ts --no-interactive",
            ".lingxi/vite-fallback/",
            "offline-fallback",
            "scaffold_mode",
        ] {
            assert!(
                descriptor.script.contains(anchor),
                "missing contract anchor: {anchor}"
            );
        }
        // Fails fast without an app id rather than spawning agents blind.
        assert!(descriptor.script.contains("requires args.app_id"));
        assert_eq!(
            descriptor.script.matches("await agent(").count(),
            descriptor.script.matches("throwOnError: true").count(),
            "every local-app agent stage must surface its terminal failure reason",
        );
        let forbidden_model_name = ["gpt-5.6", "luna"].concat();
        assert!(!descriptor.script.contains(&forbidden_model_name));
    }

    #[test]
    fn local_app_build_stops_after_a_successful_first_verification() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        let prompts_seen = Rc::new(RefCell::new(Vec::<String>::new()));
        let captured_prompts = prompts_seen.clone();

        let outcome = workflow::run_with_progress(
            descriptor.script,
            move |prompts: &[String], _options: &[String]| {
                prompts
                    .iter()
                    .map(|prompt| {
                        captured_prompts.borrow_mut().push(prompt.clone());
                        if prompt.contains("Act as the local app design lead") {
                            r#"{"targets":[{"os":"android","form_factor":"phone"}],"scaffold_mode":"existing","template":"react-ts","summary":"designed"}"#.to_string()
                        } else if prompt.contains("Manage dependencies for local app") {
                            "dependencies-ready".to_string()
                        } else if prompt.contains("Generate the complete React implementation") {
                            "generated".to_string()
                        } else if prompt.contains("Build local app") {
                            r#"{"ok":true,"preview_url":"http://preview/first","summary":"built"}"#.to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            r#"{"ok":true,"findings":[],"checked_matrix":["android-phone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"summary":"verified"}"#.to_string()
                        } else {
                            panic!("successful workflow must not repair or rebuild: {prompt}");
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
        .expect("local-app workflow executes");

        let result: serde_json::Value =
            serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                .expect("workflow returns JSON");
        assert_eq!(result["ok"], true);
        assert_eq!(result["preview_url"], "http://preview/first");
        assert_eq!(result["scaffold_mode"], "existing");
        assert_eq!(result["template"], "react-ts");
        assert_eq!(result["repair_rounds"], 0);
        assert_eq!(
            prompts_seen
                .borrow()
                .iter()
                .filter(|prompt| prompt.contains("Invoke $frontend-qa"))
                .count(),
            1
        );
    }

    #[test]
    fn local_app_build_repairs_rebuilds_restarts_and_reverifies_structured_results() {
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        let calls = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
        let captured_calls = calls.clone();
        let verification_round = Rc::new(Cell::new(0_u8));
        let next_verification_round = verification_round.clone();

        let outcome = workflow::run_with_progress(
            descriptor.script,
            move |prompts: &[String], options: &[String]| {
                prompts
                    .iter()
                    .zip(options)
                    .map(|(prompt, option)| {
                        captured_calls
                            .borrow_mut()
                            .push((prompt.clone(), option.clone()));
                        if prompt.contains("Act as the local app design lead") {
                            r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"scaffold_mode":"official-cli","template":"react","summary":"designed"}"#.to_string()
                        } else if prompt.contains("Manage dependencies for local app") {
                            "dependencies-ready".to_string()
                        } else if prompt.contains("Generate the complete React implementation") {
                            "generated".to_string()
                        } else if prompt.contains("Build local app") {
                            r#"{"ok":true,"preview_url":"http://preview/initial","summary":"built"}"#.to_string()
                        } else if prompt.contains("Invoke $frontend-qa") {
                            let round = next_verification_round.get();
                            next_verification_round.set(round + 1);
                            if round == 0 {
                                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"summary":"needs repair"}"#.to_string()
                            } else {
                                r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"summary":"verified"}"#.to_string()
                            }
                        } else if prompt.contains("Repair the findings") {
                            "repaired".to_string()
                        } else if prompt.contains("Rebuild local app") {
                            r#"{"ok":true,"preview_url":"http://preview/repaired","summary":"rebuilt and restarted"}"#.to_string()
                        } else {
                            panic!("unexpected local-app workflow prompt: {prompt}");
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
        .expect("local-app workflow executes");

        let result: serde_json::Value =
            serde_json::from_str(outcome.result.as_deref().expect("workflow result"))
                .expect("workflow returns JSON");
        assert_eq!(result["ok"], true);
        assert_eq!(result["preview_url"], "http://preview/repaired");
        assert_eq!(result["scaffold_mode"], "official-cli");
        assert_eq!(result["template"], "react");
        assert_eq!(result["repair_rounds"], 1);
        assert_eq!(verification_round.get(), 2);

        let calls = calls.borrow();
        for prompt_anchor in [
            "Act as the local app design lead",
            "Invoke $frontend-qa",
            "Rebuild local app",
        ] {
            let (_, options) = calls
                .iter()
                .find(|(prompt, _)| prompt.contains(prompt_anchor))
                .unwrap_or_else(|| panic!("missing {prompt_anchor} agent call"));
            let options: serde_json::Value =
                serde_json::from_str(options).expect("agent options JSON");
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
            .find(|(prompt, _)| prompt.contains("Rebuild local app"))
            .expect("rebuild call");
        assert!(rebuild.0.contains(r#""action":"restart""#));
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
                            r#"{"targets":[{"os":"android","form_factor":"phone"}],"scaffold_mode":"existing","template":"react","summary":"designed"}"#.to_string()
                        } else if prompt.contains("Manage dependencies for local app") {
                            "dependencies-ready".to_string()
                        } else if prompt.contains("Generate the complete React implementation") {
                            "generated".to_string()
                        } else if prompt.contains("Build local app") {
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
        let descriptor = BUILTIN_WORKFLOWS.get("local-app-build").expect("built-in");
        workflow::run_with_progress(
            descriptor.script,
            move |prompts: &[String], _options: &[String]| {
                prompts.iter().map(|prompt| reply(prompt)).collect()
            },
            |_| {},
            None,
            false,
            Some(r#"{"app_id":"test-app","spec":"build a test app"}"#.to_string()),
            None,
        )
        .map(|outcome| outcome.result.unwrap_or_default())
        .map_err(|error| error.to_string())
    }

    /// The happy-path reply table; `dead` names the ONE stage whose agent
    /// dies (the host hands back `WF_NULL_SENTINEL`, which resolves to
    /// `null` in the script — it does NOT throw on its own).
    fn local_app_reply(prompt: &str, dead: &str) -> String {
        let anchor = if prompt.contains("Act as the local app design lead") {
            "design"
        } else if prompt.contains("Manage dependencies for local app") {
            "dependencies"
        } else if prompt.contains("Generate the complete React implementation") {
            "generate"
        } else if prompt.contains("Build local app") {
            "build"
        } else if prompt.contains("Invoke $frontend-qa") {
            "verify"
        } else if prompt.contains("Repair the findings") {
            "repair"
        } else if prompt.contains("Rebuild local app") {
            "rebuild"
        } else {
            panic!("unexpected local-app workflow prompt: {prompt}")
        };
        if anchor == dead {
            return workflow::WF_NULL_SENTINEL.to_string();
        }
        match anchor {
            "design" => r#"{"targets":[{"os":"ios","form_factor":"iphone"}],"scaffold_mode":"existing","template":"react","summary":"designed"}"#.to_string(),
            "build" | "rebuild" => r#"{"ok":true,"preview_url":"http://preview/ok","summary":"built"}"#.to_string(),
            "verify" => r#"{"ok":true,"findings":[],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"summary":"verified"}"#.to_string(),
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
            ("dependencies", "the dependency step"),
            ("generate", "the source-generation step"),
            ("build", "initial build"),
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
            if prompt.contains("Build local app") || prompt.contains("Rebuild local app") {
                r#"{"ok":false,"preview_url":"","summary":"vite build exited 1"}"#.to_string()
            } else if prompt.contains("Invoke $frontend-qa") {
                r#"{"ok":false,"findings":["no preview to check"],"checked_matrix":[],"browser_available":false,"webview_checked":false,"degraded_verification":true,"summary":"cannot verify"}"#.to_string()
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
        let error = run_local_app_build(|prompt| {
            if prompt.contains("Invoke $frontend-qa") {
                r#"{"ok":false,"findings":["button is clipped"],"checked_matrix":["iphone"],"browser_available":true,"webview_checked":true,"degraded_verification":false,"summary":"needs repair"}"#.to_string()
            } else {
                local_app_reply(prompt, "")
            }
        })
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
}
