//! 校验仓库里签入的 plugin workflow 脚本能通过**运行时真正用的**那两个校验器。
//!
//! ## 为什么这个测试住在 `workflow` crate 里
//!
//! `validate_meta` 和 `check_determinism` 是这里的 `pub fn`，而被校验的脚本是
//! `plugins/lingxi-local-app/workflows/*.js`。放在别处就得把校验器再导出一层，
//! 或者复制一份判据——而判据复制出第二份的那一刻，它就开始漂移了。
//!
//! ## 为什么它必须存在（这不是假设，是已经发生过一次的事）
//!
//! 这三个脚本第一次交付时，**全部三个都通不过 `validate_meta`**：
//!
//! ```text
//! meta must be a pure literal: non-literal node type in meta: BinaryExpression
//! ```
//!
//! 原因是 `description` 写成了 `'…' + '…'` 的多行字符串拼接。那是**语法完全合法
//! 的 JavaScript**，所以 `node --check` 三个都是绿的——交付报告里也确实写了
//! 「all three scripts pass `node --check`」。
//!
//! 更值得记的是：`local-app-build.js` 的注释里**引用了这个校验器**，写着
//! 「`export const meta` parses as the engine's required first-statement literal
//! (`workflow/src/lib.rs`, `validate_meta`)」。它**引用了判据，却没有运行判据**。
//!
//! ⛔ 所以 `node --check` 对 workflow 脚本是**假绿**。判据是这两个函数，不是
//! 语法解析器。
#![allow(clippy::needless_raw_string_hashes)]

use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

/// 签入的 plugin workflow 脚本目录。
fn workflow_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lingxi-local-app/workflows")
}

fn authoring_spec() -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../local-apps/tests/fixtures/authoring-spec.valid-null-canvas.json"),
        )
        .expect("read the Host-valid checked-in AuthoringSpec fixture"),
    )
    .expect("parse the Host-valid checked-in AuthoringSpec fixture")
}

fn design_subtree() -> serde_json::Value {
    serde_json::json!({"design": authoring_spec()["design"].clone()})
}

fn design_schema() -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            workflow_dir()
                .parent()
                .unwrap()
                .join("schemas/design-spec.schema.json"),
        )
        .expect("read canonical design schema"),
    )
    .expect("parse canonical design schema")
}

fn workflow_schemas() -> serde_json::Value {
    let path = workflow_dir()
        .parent()
        .expect("plugin root")
        .join("schemas/workflow-agent-results.schema.json");
    let document: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("read checked-in workflow-agent-results schema"),
    )
    .expect("parse checked-in workflow-agent-results schema");
    let defs = document["$defs"]
        .as_object()
        .expect("workflow-agent-results schema must expose role definitions")
        .clone();
    let mut schemas = serde_json::Map::new();
    for name in [
        "template_selection",
        "design_subtree",
        "create_preparer",
        "build_result",
        "operator_result",
        "qa_review",
        "qa_finalize",
        "mcp_promoter",
    ] {
        let mut role = defs
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("missing role schema {name}"));
        role["$defs"] = serde_json::Value::Object(defs.clone());
        schemas.insert(name.to_string(), role);
    }
    schemas.insert(
        "mcp_proposal".into(),
        serde_json::from_str(
            &std::fs::read_to_string(
                workflow_dir()
                    .parent()
                    .expect("plugin root")
                    .join("schemas/mcp-proposal.schema.json"),
            )
            .expect("read checked-in MCP proposal schema"),
        )
        .expect("parse checked-in MCP proposal schema"),
    );
    schemas.insert("design_spec".into(), design_schema());
    serde_json::Value::Object(schemas)
}

/// The workflow runtime delegates structured-output validation to the Host's
/// agent runner. These hermetic callbacks bypass that runner, so successful
/// fixtures must still prove that they contain the required fields of the
/// exact role schema supplied to `agent({ schema })`.
///
/// Presence only — not types, patterns or `minItems`. That is the whole check:
/// naming it here rather than letting the call sites read as full schema
/// validation, which is the Host runner's job and is exercised against the real
/// validator in `agent`'s `local_app_operator_schema_requires_complete_host_qa_projection`.
///
/// A failure names the missing field per branch. An earlier revision found the
/// first branch whose required fields were all present and then asserted
/// exactly that predicate again, so only the `find` could ever fail and it
/// failed with the fixture dumped and nothing said about what was missing.
fn assert_schema_required_fields(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    label: &str,
) {
    assert!(
        schema.is_object(),
        "{label} agent call supplied no role schema, so its fixture is ungated: {schema}"
    );
    let branches = schema
        .get("anyOf")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![schema.clone()]);
    let mut missing_by_branch: Vec<(usize, Vec<&str>)> = Vec::new();
    for (index, branch) in branches.iter().enumerate() {
        // Every `$defs` branch in the checked-in role schemas carries
        // `required` today. A branch without one imposes nothing and would
        // make this gate vacuous for that role, so it fails here instead.
        let required = branch["required"].as_array().unwrap_or_else(|| {
            panic!("{label} role-schema branch {index} has no `required` array: {branch}")
        });
        let missing = required
            .iter()
            .map(|field| {
                field.as_str().unwrap_or_else(|| {
                    panic!("{label} role-schema branch {index} has a non-string required field: {field}")
                })
            })
            .filter(|field| value.get(field).is_none())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return;
        }
        missing_by_branch.push((index, missing));
    }
    panic!(
        "{label} callback fixture satisfies no role-schema branch; \
         missing required fields per branch: {missing_by_branch:?}; fixture: {value}"
    );
}

#[test]
fn verification_scope_schema_rejects_empty_required_target_lists() {
    let workflow_root = workflow_dir();
    let plugin_root = workflow_root.parent().expect("plugin root");
    let invalid_fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../local-apps/tests/fixtures/qa-result.invalid-empty-scope.json"),
        )
        .expect("read the negative empty verification scope fixture"),
    )
    .expect("parse the negative empty verification scope fixture");
    let fixture_scope = invalid_fixture["verification_scope"]
        .as_object()
        .expect("negative fixture verification scope");

    for schema_name in [
        "schemas/workflow-agent-results.schema.json",
        "schemas/qa-report.schema.json",
        "schemas/use-test-report.schema.json",
    ] {
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(plugin_root.join(schema_name))
                .unwrap_or_else(|error| panic!("read {schema_name}: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse {schema_name}: {error}"));
        let properties = schema["$defs"]["verification_scope"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{schema_name} verification scope properties"));

        for field in ["declared_target_ids", "in_scope_target_ids"] {
            assert_eq!(
                properties[field]["minItems"].as_u64(),
                Some(1),
                "{schema_name} must reject an empty {field}"
            );
            assert!(
                fixture_scope[field]
                    .as_array()
                    .expect("negative fixture target list")
                    .is_empty(),
                "negative fixture must exercise the empty {field} case"
            );
        }
        for field in ["unverified_target_ids", "unverified_scenario_ids"] {
            assert_eq!(
                properties[field].get("minItems"),
                None,
                "{schema_name} must allow an empty {field}"
            );
            assert!(
                fixture_scope[field]
                    .as_array()
                    .expect("negative fixture unverified list")
                    .is_empty(),
                "negative fixture should keep {field} empty"
            );
        }
    }
}

#[test]
fn every_checked_in_plugin_workflow_passes_the_runtime_validators() {
    let dir = workflow_dir();
    assert!(
        dir.is_dir(),
        "plugin workflow directory missing at {} — refusing to report a clean result from a \
         directory that does not exist",
        dir.display()
    );

    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read workflow dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "js"))
        .collect();
    scripts.sort();

    // Fail closed. 一个扫到零个文件的门和没有门是同一件事,但它看起来像通过了。
    assert!(
        !scripts.is_empty(),
        "enumerated ZERO .js workflow scripts under {} — refusing to report clean from an empty \
         enumeration",
        dir.display()
    );

    let mut failures = Vec::new();
    for script in &scripts {
        let name = script
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let src = match std::fs::read_to_string(script) {
            Ok(src) => src,
            Err(error) => {
                failures.push(format!("{name}: unreadable ({error})"));
                continue;
            }
        };
        if let Err(error) = workflow::validate_meta(&src) {
            failures.push(format!(
                "{name}: validate_meta rejected it — {error:?}. `meta` must be a PURE LITERAL; \
                 a concatenated description (`'a' + 'b'`) is valid JavaScript and passes \
                 `node --check`, but parses as a BinaryExpression and is refused here."
            ));
        }
        if let Err(error) = workflow::check_determinism(&src) {
            failures.push(format!(
                "{name}: check_determinism rejected it — {error:?}. Date.now(), Math.random() \
                 and zero-arg new Date() break resume, because the journal replays prior agent() \
                 results but re-runs the JS body."
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} checked-in plugin workflow script(s) fail the runtime validators:\n  - {}",
        failures.len(),
        scripts.len(),
        failures.join("\n  - ")
    );
}

/// r2-tests-honesty-011: blank out every `//`-comment line before running the
/// substring checks below, so a comment merely MENTIONING `agent(`,
/// `lingxi-local-app:` or `HOST_PRECONDITION_UNAVAILABLE` can no longer
/// satisfy (or defeat) them the way live code would.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn phase4_and_phase6_workflows_use_real_orchestration() {
    let build = std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read build workflow");
    let use_test = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    for (name, source) in [
        ("local-app-build.js", build),
        ("local-app-use-test.js", use_test),
    ] {
        let code = strip_line_comments(&source);
        assert!(
            code.contains("agent("),
            "{name} must orchestrate Plugin agents"
        );
        // Not a bare substring: the namespaced identity must be the script's
        // OWN declared `WORKFLOW_ID`, not merely a string that happens to
        // appear somewhere (a comment mentioning the namespace used to be
        // enough to satisfy this).
        assert!(
            code.lines().any(|line| line
                .trim_start()
                .starts_with("const WORKFLOW_ID = 'lingxi-local-app:")),
            "{name} must declare `const WORKFLOW_ID = 'lingxi-local-app:...'`, not merely \
             mention the namespace"
        );
        assert!(
            !code.contains("HOST_PRECONDITION_UNAVAILABLE"),
            "{name} must no longer be a Phase2 placeholder"
        );
    }
    let mcp = std::fs::read_to_string(workflow_dir().join("local-app-mcp-authoring.js"))
        .expect("read mcp workflow");
    let mcp_code = strip_line_comments(&mcp);
    assert!(
        mcp_code.contains("agent("),
        "Phase6 MCP authoring must orchestrate agents"
    );
    assert!(
        mcp_code.contains("mcp_authoring_required"),
        "zero-tool authoring must be explicit"
    );
    assert!(
        !mcp_code.contains("HOST_PRECONDITION_UNAVAILABLE"),
        "Phase6 MCP authoring must no longer be a placeholder"
    );
}

#[test]
fn unified_build_workflow_executes_create_identity_chain_with_hermetic_agents() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read build workflow");
    let args = serde_json::json!({
        "operation": "create",
        "app_id": "aaaa1111",
        "name": "Form App",
        "brief": "A small form app",
        "authoring_spec": authoring_spec(),
        "quality_level": "balanced",
        // r3-workflow-runtime-02: the Host launch boundary always injects
        // `workflow_run_id` at the TOP level of args (workflow_support.rs
        // :1775-1779), not only inside `host_context`. A hermetic fixture
        // that omits it can never exercise the script's `unknown` check
        // against that key, so a regression that dropped `workflow_run_id`
        // from `INTERNAL_KEYS` would pass this test while breaking every
        // real create launch.
        "workflow_run_id": "wf_hermetic1",
        "host_context": {
            "source": "verified_host",
            "operation": "create",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_hermetic1",
            "selector_capability": "sel_00000000000000000000000000000000",
            "schemas": workflow_schemas(),
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "template_catalog": {"catalog_digest": "digest", "available_template_ids": ["react-dom-r2"]},
            "staging": {"isolated": true, "final_publish": false},
            "authoring_spec": authoring_spec()
        }
    });
    let build = serde_json::json!({"ok": true, "preview_url": "http://127.0.0.1:20000", "summary": "built"});
    // r2-tests-honesty-002: capture every (label, prompt) pair the workflow
    // actually sends an agent, keyed by the SAME `options` substring the stub
    // dispatches on below, so the test can assert on real script OUTPUT (the
    // literal command text an agent would have received) instead of only on
    // the workflow's return value -- where two of the four prior assertions
    // were literals hardcoded in the script itself (`WORKFLOW_ID`, the
    // unconditional `const promotion = null`) and a third was simply the
    // stub's own canned response echoed straight through the `approval`
    // passthrough field.
    let seen_prompts: Arc<std::sync::Mutex<Vec<(String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_prompts_for_stub = seen_prompts.clone();
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    // r3-workflow-runtime-07: dispatch on the exact `label`
                    // field of the PARSED opts, not a substring of the whole
                    // Dispatch on the exact label, not a substring of the
                    // serialized options.
                    let opts: serde_json::Value =
                        serde_json::from_str(options).unwrap_or(serde_json::Value::Null);
                    let opts_label = opts
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let capture_label = if opts_label.starts_with("repair-") {
                        "repair-"
                    } else {
                        opts_label
                    }
                    .to_string();
                    seen_prompts_for_stub
                        .lock()
                        .expect("prompt log")
                        .push((capture_label, prompt.clone()));
                    match opts_label {
                        "template-selector" => serde_json::json!({"catalog_digest":"digest","template_id":"react-dom-r2","reason":"ordinary form","rejected":[],"validated_selection_handle":"vsel_0123456789abcdef0123456789abcdef"}).to_string(),
                        "create-preparer" => serde_json::json!({
                            "approved": true,
                            "ok": true,
                            "contract_handle": "contract_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                            "contract_sha256": "c".repeat(64),
                            "receipt_id": "mcp-create-receipt",
                            "status": "create_approved_no_mcp"
                        })
                        .to_string(),
                        "designer" => design_subtree().to_string(),
                        "builder-build" => build.to_string(),
                        _ if opts_label.starts_with("repair-") => build.to_string(),
                        _ => passing_qa_reply(opts_label, "wf_hermetic1", "balanced"),
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("hermetic workflow should execute");
    let result = outcome.result.expect("workflow result");
    // The only assertion on the return value that is neither a script-side
    // literal (WORKFLOW_ID, `promotion: null`) nor an echoed stub response:
    // `repairRounds` is a real counter, and a hermetic run where every stub
    // reports success must complete the create identity chain without
    // entering the repair loop.
    assert!(result.contains("\"repair_rounds\":0"));

    let prompts = seen_prompts.lock().expect("prompt log");
    let agent_calls = prompts.len();
    assert_eq!(
        agent_calls,
        6,
        "a hermetic balanced create run must make exactly selector, designer, preparer, builder, operator, tester — 6 agent calls (saw: {:?})",
        prompts.iter().map(|(label, _)| label).collect::<Vec<_>>()
    );
    let prompt_for = |label: &str| -> &str {
        prompts
            .iter()
            .find(|(seen_label, _)| seen_label == label)
            .map(|(_, prompt)| prompt.as_str())
            .unwrap_or_else(|| panic!("no agent call was dispatched under label {label:?}"))
    };
    for label in ["designer", "create-preparer"] {
        let prompt = prompt_for(label);
        assert!(
            prompt.contains("LocalAppResolveTemplateSelection with app_id=aaaa1111, workflow_run_id=wf_hermetic1, validated_selection_handle=vsel_0123456789abcdef0123456789abcdef"),
            "{label} must receive the complete Host selection identity instead of guessing the run ID: {prompt}"
        );
    }
    let stage_prompt = prompt_for("create-preparer");
    assert!(
        stage_prompt
            .contains("call LocalAppStageCreate with contract_handle from LocalAppContract"),
        "create-preparer prompt must instruct the agent to stage the create candidate for the \
         real app_id: {stage_prompt}"
    );
    assert!(
        stage_prompt.contains("workflow_run_id=wf_hermetic1"),
        "builder-stage prompt must carry the Host-minted workflow_run_id from host_context, \
         not a value the script invented: {stage_prompt}"
    );
    let approval_prompt = prompt_for("create-preparer");
    assert!(
        approval_prompt.contains("create_without_mcp=true"),
        "native-create-approval prompt must request the native (no-MCP) create path: \
         {approval_prompt}"
    );
    let build_prompt = prompt_for("builder-build");
    assert!(
        build_prompt.contains("receipt_id=mcp-create-receipt"),
        "builder-build prompt must carry the receipt_id returned by the create approval step, \
         not a value the script invented: {build_prompt}"
    );
    assert!(
        build_prompt.contains("LocalAppManifest"),
        "builder-build prompt must instruct the agent to declare collections through \
         LocalAppManifest before writing against them: {build_prompt}"
    );
    assert!(
        build_prompt.contains("LocalAppBuild with app_id=aaaa1111, workflow_run_id=wf_hermetic1"),
        "every build call must carry the Host workflow run identity: {build_prompt}"
    );
    let stage_prompt = prompt_for("create-preparer");
    assert!(
        !stage_prompt.contains("mcp_intent="),
        "an unasked MCP question must omit the object-only mcp_intent field: {stage_prompt}"
    );
}

#[test]
fn first_pass_agent_call_counts_are_exercised_for_each_quality_level() {
    let source = build_workflow_script();
    for (quality, expected) in [("fast", 5usize), ("balanced", 6), ("thorough", 7)] {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls_for_run = Arc::clone(&calls);
        let args = build_create_args(serde_json::json!({"quality_level": quality}));
        let outcome = workflow::run_with_progress(
            &source,
            move |prompts, options| {
                prompts
                    .iter()
                    .zip(options.iter())
                    .map(|(_, options)| {
                        let label = stage_label(options);
                        let options_value: serde_json::Value =
                            serde_json::from_str(options).expect("workflow agent options JSON");
                        let role_schema = options_value
                            .get("schema")
                            .and_then(serde_json::Value::as_object)
                            .expect("each Local App agent call must carry a role schema");
                        let defs = role_schema
                            .get("$defs")
                            .and_then(serde_json::Value::as_object)
                            .expect("role schema must retain the complete workflow schema defs");
                        if label == "designer" {
                            assert_eq!(options_value["schema"], design_schema(), "designer must receive the closed design contract");
                            assert_eq!(options_value["structuredOutputParseRetries"], 2);
                        } else {
                            assert!(options_value.get("structuredOutputParseRetries").is_none(), "only designers opt into parse recovery");
                            assert!(
                                defs.contains_key("qa_candidate") && defs.contains_key("mcp_promoter"),
                                "agent call {label} must use the checked-in full role schema, not a permissive object witness"
                            );
                        }
                        calls_for_run
                            .lock()
                            .expect("call capture")
                            .push(label.clone());
                        let reply = canned_stage_reply(&label)
                            .unwrap_or_else(|| passing_qa_reply(&label, "wf_regression", quality));
                        let structured: serde_json::Value =
                            serde_json::from_str(&reply).expect("structured callback fixture JSON");
                        assert_schema_required_fields(
                            &options_value["schema"],
                            &structured,
                            &label,
                        );
                        reply
                    })
                    .collect()
            },
            |_progress| {},
            None,
            true,
            Some(args.to_string()),
            None,
        )
        .unwrap_or_else(|error| panic!("{quality} quality workflow should execute: {error}"));
        assert!(
            outcome.result.is_some(),
            "{quality} quality must return a result"
        );
        let labels = calls.lock().expect("call capture");
        assert_eq!(
            labels.len(),
            expected,
            "{quality} quality must dispatch exactly {expected} successful first-pass agents: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label.starts_with("repair-")),
            "{quality} quality fixture must not enter repair: {labels:?}"
        );
    }
}

#[test]
fn apple_design_uses_only_confirmed_ios_and_ipados_targets() {
    let cases = [
        (
            "normalized iOS",
            serde_json::json!([{"id":"iphone-main","os":"  IoS ","form_factor":"phone"}]),
            "balanced",
            vec!["iphone-main"],
        ),
        (
            "iPadOS",
            serde_json::json!([{"id":"ipad-main","os":" iPaDoS ","form_factor":"tablet"}]),
            "balanced",
            vec!["ipad-main"],
        ),
        (
            "Android",
            serde_json::json!([{"id":"android-main","os":" Android ","form_factor":"phone"}]),
            "balanced",
            Vec::<&str>::new(),
        ),
        (
            "mixed targets",
            serde_json::json!([
                {"id":"iphone-main","os":"IOS","form_factor":"phone"},
                {"id":"android-main","os":"android","form_factor":"phone"},
                {"id":"ipad-main","os":"IPADOS","form_factor":"tablet"}
            ]),
            "fast",
            vec!["iphone-main", "ipad-main"],
        ),
    ];

    for (name, targets, quality, target_ids) in cases {
        let expects_apple_design = !target_ids.is_empty();
        let prompts = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let prompts_for_run = Arc::clone(&prompts);
        let args = build_create_args_with_targets(targets, quality);
        let design = serde_json::json!({"design": args["authoring_spec"]["design"]});
        let outcome = workflow::run_with_progress(
            &build_workflow_script(),
            move |prompt_batch, options| {
                prompt_batch
                    .iter()
                    .zip(options.iter())
                    .map(|(prompt, options)| {
                        let label = stage_label(options);
                        prompts_for_run
                            .lock()
                            .expect("prompt log")
                            .push((label.clone(), prompt.clone()));
                        if label == "designer" {
                            design.to_string()
                        } else {
                            canned_stage_reply(&label).unwrap_or_else(|| {
                                passing_qa_reply(&label, "wf_regression", quality)
                            })
                        }
                    })
                    .collect()
            },
            |_progress| {},
            None,
            true,
            Some(args.to_string()),
            None,
        )
        .unwrap_or_else(|error| panic!("{name} Apple routing should execute: {error}"));
        assert!(outcome.result.is_some(), "{name} should return a result");

        let prompts = prompts.lock().expect("prompt log");
        let designer = prompts.iter().find(|(label, _)| label == "designer");
        assert_eq!(designer.is_some(), quality != "fast", "{name}: {prompts:?}");
        let builder = prompts
            .iter()
            .find(|(label, _)| label == "builder-build")
            .expect("create builder prompt");
        let expected_ids = serde_json::to_string(&target_ids).expect("target IDs JSON");
        if !expects_apple_design {
            assert!(
                !builder.1.contains("Skill('lingxi-local-app:apple-design')"),
                "{name}: {builder:?}"
            );
        } else {
            assert!(
                builder.1.contains("Skill('lingxi-local-app:apple-design')"),
                "{name}: {builder:?}"
            );
            assert!(
                builder.1.contains(&expected_ids),
                "{name} lost scoped target IDs: {builder:?}"
            );
            assert!(
                builder.1.contains("confirmed brand"),
                "{name} lost brand preservation: {builder:?}"
            );
        }
        if quality != "fast" {
            let designer = designer.expect("designer prompt");
            if expects_apple_design {
                assert!(
                    designer
                        .1
                        .contains("Skill('lingxi-local-app:apple-design')"),
                    "{name}: {designer:?}"
                );
                assert!(
                    designer.1.contains(&expected_ids),
                    "{name} lost designer target IDs: {designer:?}"
                );
            } else {
                assert!(
                    !designer
                        .1
                        .contains("Skill('lingxi-local-app:apple-design')"),
                    "{name}: {designer:?}"
                );
            }
        }
    }
}

#[test]
fn fast_ui_update_loads_apple_design_without_running_designer() {
    let mut args = build_create_args(serde_json::json!({"quality_level": "fast"}));
    args["operation"] = serde_json::Value::String("update".into());
    args["revision_prompt"] = serde_json::Value::String("Refresh the list presentation".into());
    args["workflow_run_id"] = serde_json::Value::String("wf_update_fast".into());
    args["host_context"]["operation"] = serde_json::Value::String("update".into());
    args["host_context"]["workflow_run_id"] = serde_json::Value::String("wf_update_fast".into());
    args["host_context"]["update_ui_impact"] = serde_json::Value::Bool(true);
    args["host_context"]["authoring_contract_sha256"] = serde_json::Value::String("f".repeat(64));
    args["host_context"]["runtime_profile"] = serde_json::json!({
        "family": "react_dom",
        "revision": 1,
        "contract_sha256": "a".repeat(64),
        "surface": "dom"
    });
    args["host_context"]["dependency_snapshot"] = serde_json::json!({"verified": true});
    args["host_context"]["authoring_contract"] = serde_json::json!({
        "contract_handle": "contract_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "spec": authoring_spec()
    });

    let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &build_workflow_script(),
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    calls_for_run
                        .lock()
                        .expect("call log")
                        .push((label.clone(), prompt.clone()));
                    if label == "builder" {
                        serde_json::json!({
                            "ok": true,
                            "preview_url": "http://127.0.0.1:20000",
                            "contract_handle": "contract_cccccccccccccccccccccccccccccccc",
                            "summary": "updated"
                        })
                        .to_string()
                    } else {
                        passing_qa_reply(&label, "wf_update_fast", "fast")
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("fast UI update should execute");
    assert!(outcome
        .result
        .expect("workflow result")
        .contains("\"ok\":true"));
    let calls = calls.lock().expect("call log");
    assert!(
        !calls.iter().any(|(label, _)| label == "designer"),
        "{calls:?}"
    );
    let builder_prompt = calls
        .iter()
        .find(|(label, _)| label == "builder")
        .map(|(_, prompt)| prompt)
        .expect("fast UI update builder prompt");
    assert!(
        builder_prompt.contains("Skill('lingxi-local-app:apple-design')"),
        "{builder_prompt}"
    );
    assert!(builder_prompt.contains("[\"primary\"]"), "{builder_prompt}");
}

#[test]
fn refused_or_incomplete_design_stops_before_create_preparation() {
    for design in [
        serde_json::json!({"status": "refused", "refusal": "validated_selection_missing: journal row not found", "design_produced": false}),
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!({"presentations": [], "tokens": {}, "states": {}, "inputs": {}}),
    ] {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&calls);
        let outcome = workflow::run_with_progress(
            &build_workflow_script(),
            move |_, options| {
                options
                    .iter()
                    .map(|options| {
                        let label = stage_label(options);
                        seen.lock().expect("calls").push(label.clone());
                        if label == "designer" {
                            serde_json::json!({"design": design}).to_string()
                        } else {
                            canned_stage_reply(&label).unwrap_or_else(|| {
                                passing_qa_reply(&label, "wf_regression", "balanced")
                            })
                        }
                    })
                    .collect()
            },
            |_| {},
            None,
            true,
            Some(build_create_args(serde_json::json!({})).to_string()),
            None,
        );
        assert!(
            outcome.is_err(),
            "a refusal or incomplete design must stop the workflow"
        );
        assert_eq!(
            calls.lock().expect("calls").as_slice(),
            ["template-selector", "designer"]
        );
    }
}

#[test]
fn actual_role_schemas_drive_a_host_shaped_successful_create_chain() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(_, options)| {
                    let label = stage_label(options);
                    let options_value: serde_json::Value =
                        serde_json::from_str(options).expect("workflow agent options JSON");
                    let schema = options_value.get("schema").expect("agent schema");
                    if label == "designer" {
                        assert_eq!(
                            schema,
                            &design_schema(),
                            "designer must receive the closed design contract"
                        );
                    } else {
                        assert!(
                            schema["$defs"]["qa_candidate"].is_object()
                                && schema["$defs"]["mcp_promoter"].is_object(),
                            "{label} must receive the checked-in complete role schema"
                        );
                    }
                    calls_for_run.lock().expect("call log").push(label.clone());
                    canned_stage_reply(&label).unwrap_or_else(|| {
                        host_shaped_passing_qa_reply(&label, "wf_regression", "balanced")
                    })
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("actual-schema create chain should execute");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert!(
        result.contains("qa_0123456789abcdef0123456789abcdef"),
        "{result}"
    );
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        [
            "template-selector",
            "designer",
            "create-preparer",
            "builder-build",
            "operator-0",
            "tester-0"
        ]
    );
}

#[test]
fn qa_prompts_project_host_identity_without_schemas_catalog_or_selector_capability() {
    let source = build_workflow_script();
    let qa_prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let qa_prompts_for_run = Arc::clone(&qa_prompts);
    let mut args = build_create_args(serde_json::json!({}));
    let context = args["host_context"]
        .as_object_mut()
        .expect("host context object");
    context.insert(
        "selector_capability".into(),
        serde_json::Value::String("selector_secret_sentinel".into()),
    );
    context["template_catalog"]["catalog_secret"] =
        serde_json::Value::String("catalog_secret_sentinel".into());
    context["schemas"]["schema_secret"] =
        serde_json::Value::String("schema_secret_sentinel".into());
    workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    if label.starts_with("operator-")
                        || label.starts_with("tester-")
                        || label.starts_with("verifier")
                    {
                        qa_prompts_for_run
                            .lock()
                            .expect("QA prompt capture")
                            .push(prompt.clone());
                    }
                    canned_stage_reply(&label)
                        .unwrap_or_else(|| passing_qa_reply(&label, "wf_regression", "balanced"))
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("workflow should retain the bounded Host QA identity");
    let prompts = qa_prompts.lock().expect("QA prompts");
    assert!(!prompts.is_empty());
    for prompt in prompts.iter() {
        for secret in [
            "selector_secret_sentinel",
            "catalog_secret_sentinel",
            "schema_secret_sentinel",
        ] {
            assert!(
                !prompt.contains(secret),
                "QA prompt leaked {secret}: {prompt}"
            );
        }
        assert!(
            prompt.contains("primary-action"),
            "QA prompt lost the persisted acceptance identity: {prompt}"
        );
    }
}

#[test]
fn unified_build_verify_is_read_only_and_persisted_canvas_rejects_fast() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read build workflow");
    // r3-workflow-runtime-02: this helper mirrors the Host's real top-level
    // runtime profile and workflow identity injection alongside host_context.
    let verify_args = build_verify_args("balanced");
    let seen_options = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_prompts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let options_for_run = Arc::clone(&seen_options);
    let prompts_for_run = Arc::clone(&seen_prompts);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts_for_run
                .lock()
                .expect("prompt capture")
                .extend(prompts.iter().cloned());
            options_for_run
                .lock()
                .expect("option capture")
                .extend(options.iter().cloned());
            options
                .iter()
                .map(|options| failing_qa_reply(&stage_label(options), "wf_verify1", "balanced"))
                .collect()
        },
        |_progress| {},
        None,
        false,
        Some(verify_args.to_string()),
        None,
    )
    .expect("verify workflow should return a structured failure");
    let result = outcome.result.expect("verify result");
    assert!(result.contains("\"status\":\"verification_failed\""));
    assert!(result.contains("\"repair_rounds\":0"));
    assert!(result.contains("\"status\":\"verification_failed\""));
    assert!(
        seen_options
            .lock()
            .expect("option capture")
            .iter()
            .all(|options| !options.contains("builder") && !options.contains("repair-")),
        "verify must never invoke a builder or repair agent"
    );
    assert!(
        seen_prompts
            .lock()
            .expect("prompt capture")
            .iter()
            .all(|prompt| !prompt.contains("Handle=persisted-profile")),
        "persisted workflows must not manufacture a template-selection handle"
    );
    assert!(
        seen_prompts
            .lock()
            .expect("prompt capture")
            .iter()
            .all(|prompt| !prompt.contains("lingxi-local-app:apple-design")),
        "verify must never load Apple Design"
    );

    let fast_canvas_args = serde_json::json!({
        "operation": "update",
        "app_id": "aaaa1111",
        "quality_level": "fast",
        "host_context": {
            "source": "verified_host",
            "operation": "update",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_update1",
            "schemas": workflow_schemas(),
            "runtime_profile": {
                "family": "canvas_2d",
                "revision": 1,
                "contract_sha256": "b".repeat(64),
                "surface": "canvas"
            },
            "template_catalog": {
                "catalog_digest": "catalog",
                "available_template_ids": ["canvas-2d-r2"]
            },
            "expected_writable_collections": [],
            "dependency_snapshot": {"verified": true}
        }
    });
    let mut bare_args = build_create_args(serde_json::json!({}));
    bare_args
        .as_object_mut()
        .expect("create args")
        .remove("name");
    bare_args
        .as_object_mut()
        .expect("create args")
        .remove("brief");
    let error = workflow::run_with_progress(
        &source,
        |_prompts, _options| panic!("fast Canvas rejection must happen before any agent call"),
        |_progress| {},
        None,
        false,
        Some(fast_canvas_args.to_string()),
        None,
    )
    .expect_err("persisted Canvas profiles must reject fast quality");
    assert!(
        error.to_string().contains("CANVAS_FAST_REJECTED"),
        "unexpected fast Canvas error: {error}"
    );
}

#[test]
fn unified_build_update_keeps_mcp_authoring_explicit() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read build workflow");
    let args = serde_json::json!({
        "operation": "update",
        "app_id": "aaaa1111",
        "revision_prompt": "Add a bounded search filter",
        "quality_level": "balanced",
        // r3-workflow-runtime-02: mirror the Host's real top-level injection
        // (workflow_support.rs :1488-1493, :1775-1779) alongside host_context,
        // not only inside it.
        "workflow_run_id": "wf_update2",
        // Three fields only -- see the verify fixture's note; the build path
        // never populates `surface` at the top level.
        "runtime_profile": {
            "family": "react_dom",
            "revision": 1,
            "contract_sha256": "a".repeat(64)
        },
        "expected_writable_collections": ["items"],
        "host_context": {
            "source": "verified_host",
            "operation": "update",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_update2",
            "schemas": workflow_schemas(),
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "runtime_profile": {
                "family": "react_dom",
                "revision": 1,
                "contract_sha256": "a".repeat(64),
                "surface": "dom"
            },
            "template_catalog": {
                "catalog_digest": "catalog",
                "available_template_ids": ["react-dom-r2"]
            },
            "expected_writable_collections": ["items"],
            "dependency_snapshot": {"verified": true},
            "active_catalog": {"catalog_sha256": "b".repeat(64)},
            "authoring_contract_sha256": "f".repeat(64),
            "authoring_contract": {
                "contract_handle": "contract_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "spec": authoring_spec()
            },
            "authoring_spec": authoring_spec()
        }
    });
    let nested_calls = Arc::new(AtomicUsize::new(0));
    let nested_calls_for_run = Arc::clone(&nested_calls);
    let update_calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let update_calls_for_run = Arc::clone(&update_calls);
    let build = serde_json::json!({
        "ok": true,
        "preview_url": "http://127.0.0.1:20000",
        "contract_handle": "contract_cccccccccccccccccccccccccccccccc",
        "summary": "updated"
    });
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            options
                .iter()
                .zip(prompts.iter())
                .map(|(options, prompt)| {
                    update_calls_for_run
                        .lock()
                        .expect("update call log")
                        .push((stage_label(options), prompt.clone()));
                    if options.contains("__wf_resolve") {
                        nested_calls_for_run.fetch_add(1, Ordering::SeqCst);
                        "return { status: 'promoted', promotion: { promoted: true } };".to_string()
                    } else if options.contains("builder") {
                        build.to_string()
                    } else {
                        passing_qa_reply(&stage_label(options), "wf_update2", "balanced")
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("update workflow should execute");
    let result = outcome.result.expect("update result");
    assert!(result.contains("\"ok\":true"));
    assert_eq!(
        nested_calls.load(Ordering::SeqCst),
        0,
        "ordinary app updates must not run MCP authoring without an explicit user request"
    );
    let update_calls = update_calls.lock().expect("update call log");
    assert!(
        !update_calls.iter().any(|(label, _)| label == "designer"),
        "a code-only update must skip designer: {update_calls:?}"
    );
    let builder_prompt = update_calls
        .iter()
        .find(|(label, _)| label == "builder")
        .map(|(_, prompt)| prompt)
        .expect("update builder prompt");
    assert!(
        !builder_prompt.contains("Skill('lingxi-local-app:apple-design')"),
        "a code-only update must preserve existing Apple styles: {builder_prompt}"
    );
    for required in [
        "operation=stage, app_id=aaaa1111, workflow_run_id=wf_update2",
        "base_contract_sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "the full confirmed update",
        "LocalAppBuild with exactly app_id=aaaa1111, workflow_run_id=wf_update2",
    ] {
        assert!(
            builder_prompt.contains(required),
            "update builder prompt is missing {required:?}: {builder_prompt}"
        );
    }
}

#[test]
fn host_ui_impact_flag_is_the_only_update_design_trigger() {
    let source = build_workflow_script();
    let args = serde_json::json!({
        "operation": "update",
        "app_id": "aaaa1111",
        "revision_prompt": "Refresh the search presentation",
        "quality_level": "balanced",
        "workflow_run_id": "wf_update_ui1",
        "host_context": {
            "source": "verified_host",
            "operation": "update",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_update_ui1",
            "update_ui_impact": true,
            "authoring_contract_sha256": "f".repeat(64),
            "schemas": workflow_schemas(),
            "runtime_profile": {"family": "react_dom", "revision": 1, "contract_sha256": "a".repeat(64)},
            "template_catalog": {"catalog_digest": "catalog", "available_template_ids": ["react-dom-r2"]},
            "expected_writable_collections": [],
            "dependency_snapshot": {"verified": true},
            "authoring_contract": {"contract_handle": "contract_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "spec": authoring_spec()},
            "authoring_spec": authoring_spec()
        }
    });
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if label == "designer" {
                        let options_value: serde_json::Value = serde_json::from_str(options).unwrap();
                        assert_eq!(options_value["schema"], design_schema());
                        assert_eq!(options_value["structuredOutputParseRetries"], 2);
                        assert!(prompt.contains("Resolve impact"), "UI-impact update lost design instruction: {prompt}");
                        assert!(prompt.contains("Skill('lingxi-local-app:apple-design')"), "UI-impact update lost Apple design instruction: {prompt}");
                        design_subtree().to_string()
                    } else if label == "builder" {
                        assert!(prompt.contains("Skill('lingxi-local-app:apple-design')"), "UI-impact update builder lost Apple design instruction: {prompt}");
                        serde_json::json!({"ok": true, "preview_url": "http://127.0.0.1:20000", "contract_handle": "contract_cccccccccccccccccccccccccccccccc", "summary": "updated"}).to_string()
                    } else {
                        passing_qa_reply(&label, "wf_update_ui1", "balanced")
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("UI-impact update should execute");
    assert!(outcome
        .result
        .expect("workflow result")
        .contains("\"ok\":true"));
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        ["designer", "builder", "operator-0", "tester-0"]
    );
}

#[test]
fn mcp_authoring_workflow_validates_zero_tool_candidates_and_executes_approval_path() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-mcp-authoring.js"))
        .expect("read mcp workflow");
    let initial_args = serde_json::json!({
        "app_id": "aaaa1111",
        "user_goal": "Expose the saved recipes search as MCP",
        "workflow_run_id": "wf_mcp_authoring1",
        "runtime_profile": {"family":"react_dom","revision":1,"contract_sha256":"a".repeat(64),"surface":"dom"},
        "expected_writable_collections": ["recipes"],
        "host_context": {
            "source": "verified_host",
            "operation": "initial",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_mcp_authoring1",
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "schemas": workflow_schemas(),
            "template_catalog": {"catalog_digest": "digest", "available_template_ids": ["react-dom-r2"]},
            "expected_writable_collections": ["recipes"],
            "dependency_snapshot": {"verified": true},
            "active_catalog": null
        }
    });
    let revise_args = serde_json::json!({
        "app_id": "aaaa1111",
        "user_goal": "Expose the saved recipes search as MCP",
        "workflow_run_id": "wf_mcp_authoring2",
        "runtime_profile": {"family":"react_dom","revision":1,"contract_sha256":"a".repeat(64),"surface":"dom"},
        "expected_writable_collections": ["recipes"],
        "host_context": {
            "source": "verified_host",
            "operation": "revise",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_mcp_authoring2",
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "schemas": workflow_schemas(),
            "template_catalog": {"catalog_digest": "digest", "available_template_ids": ["react-dom-r2"]},
            "expected_writable_collections": ["recipes"],
            "dependency_snapshot": {"verified": true},
            "active_catalog": {"build_id":"build-a","manifest_revision":3,"authoring_revision":1,"catalog_sha256":"9".repeat(64),"approval_contract_sha256":"a".repeat(64),"tool_surface_sha256":"b".repeat(64)}
        }
    });
    let evidence = serde_json::json!({
        "summary": "search UI with persisted recipes collection",
        "runtime_profile": {"family": "react_dom"},
        "collections": ["recipes"],
        "active_catalog": null
    });
    let proposal = serde_json::json!({
        "appId": "aaaa1111",
        "manifestRevision": 3,
        "userGoalSha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "summary": "Expose bounded recipe search",
        "tools": [{
            "name": "search_recipes",
            "title": "Search recipes",
            "description": "Search saved recipes",
            "inputSchema": {"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},
            "outputSchema": {"type":"object","properties":{"items":{"type":"array"}},"required":["items"],"additionalProperties":false},
            "semanticFlowId": "search_recipes",
            "inputs": {"query":{"tool_input":{"json_pointer":"/query"}}},
            "result": {"step_output":{"step_id":"search","json_pointer":"/items"}}
        }],
        "requiredFlowChanges": [],
        "excludedCapabilities": []
    });
    let validated = serde_json::json!({
        "ok": true,
        "status": "approval_required",
        "proposal_sha256": "1".repeat(64),
        "approval_contract_sha256": "2".repeat(64),
        "tool_surface_sha256": "3".repeat(64),
        "findings": []
    });
    let approved = serde_json::json!({
        "approved": true,
        "receipt_id": "mrcpt_00000000000000000000000000000000",
        "status": "approved"
    });
    let qa = serde_json::json!({
        "ok": true,
        "findings": [],
        "mcp_schema": "passed",
        "flow_binding": "passed",
        "calls": "passed",
        "isolation": "passed",
        "verification_sha256": "4".repeat(64),
        "summary": "passed"
    });
    let promoted = serde_json::json!({
        "promoted": true,
        "catalog_sha256": "5".repeat(64),
        "status": "promoted",
        "publication_state": "published_unverified"
    });
    let approval_calls = Arc::new(AtomicUsize::new(0));
    let approval_calls_for_run = Arc::clone(&approval_calls);
    let evidence_for_promote = evidence.clone();
    let proposal_for_initial = proposal.clone();
    let validated_for_initial = validated.clone();
    let approved_for_initial = approved.clone();
    let qa_for_initial = qa.clone();
    let promoted_for_initial = promoted.clone();
    let promotion_prompt = Arc::new(Mutex::new(String::new()));
    let promotion_prompt_for_run = Arc::clone(&promotion_prompt);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            options
                .iter()
                .zip(prompts.iter())
                .map(|(options, prompt)| {
                    if options.contains("app-evidence") {
                        evidence.to_string()
                    } else if options.contains("host-proposal-validation") {
                        validated_for_initial.to_string()
                    } else if options.contains("native-approval") {
                        approval_calls_for_run.fetch_add(1, Ordering::SeqCst);
                        approved_for_initial.to_string()
                    } else if options.contains("mcp-qa") {
                        qa_for_initial.to_string()
                    } else if options.contains("mcp-promote") {
                        *promotion_prompt_for_run.lock().expect("promotion prompt") =
                            prompt.clone();
                        promoted_for_initial.to_string()
                    } else {
                        proposal_for_initial.to_string()
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        false,
        Some(initial_args.to_string()),
        None,
    )
    .expect("mcp authoring workflow should execute");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"status\":\"promoted\""));
    assert_eq!(approval_calls.load(Ordering::SeqCst), 1);
    let promotion_prompt = promotion_prompt.lock().expect("promotion prompt");
    assert!(
        promotion_prompt.contains(
            r#"{"app_id":"aaaa1111","workflow_run_id":"wf_mcp_authoring1","receipt_id":"mrcpt_00000000000000000000000000000000"}"#
        ),
        "promotion must use only the real tool input fields: {promotion_prompt}"
    );
    for forbidden_payload in [
        "1111111111111111",
        "4444444444444444",
        "tool_surface_sha256",
    ] {
        assert!(
            !promotion_prompt.contains(forbidden_payload),
            "promotion prompt leaked non-input candidate/QA data {forbidden_payload}: {promotion_prompt}"
        );
    }

    let evidence_for_zero_tools = evidence_for_promote.clone();
    let proposal_for_revise = proposal.clone();
    let mut validated_for_revise = validated.clone();
    validated_for_revise["status"] = serde_json::Value::String("approved_reusable".into());
    let qa_for_revise = qa.clone();
    let promoted_for_revise = promoted.clone();
    let reuse_promotion_prompt = Arc::new(Mutex::new(String::new()));
    let reuse_promotion_prompt_for_run = Arc::clone(&reuse_promotion_prompt);
    let revise_outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            options
                .iter()
                .zip(prompts.iter())
                .map(|(options, prompt)| {
                    if options.contains("app-evidence") {
                        evidence_for_promote.to_string()
                    } else if options.contains("host-proposal-validation") {
                        validated_for_revise.to_string()
                    } else if options.contains("native-approval") {
                        panic!("approved_reusable must not request a second native approval")
                    } else if options.contains("mcp-qa") {
                        qa_for_revise.to_string()
                    } else if options.contains("mcp-promote") {
                        *reuse_promotion_prompt_for_run
                            .lock()
                            .expect("reuse promotion prompt") = prompt.clone();
                        promoted_for_revise.to_string()
                    } else {
                        proposal_for_revise.to_string()
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        false,
        Some(revise_args.to_string()),
        None,
    )
    .expect("revise mcp authoring workflow should execute");
    let revise_result = revise_outcome.result.expect("revise workflow result");
    assert!(revise_result.contains("\"status\":\"promoted\""));
    let reuse_promotion_prompt = reuse_promotion_prompt
        .lock()
        .expect("reuse promotion prompt");
    assert!(
        reuse_promotion_prompt.contains(
            r#"{"app_id":"aaaa1111","workflow_run_id":"wf_mcp_authoring2"}"#
        ) && !reuse_promotion_prompt.contains("receipt_id"),
        "an approval-reuse promotion must omit the absent optional receipt: {reuse_promotion_prompt}"
    );

    let zero_tool_outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    if options.contains("app-evidence") {
                        evidence_for_zero_tools.to_string()
                    } else if options.contains("host-proposal-validation") {
                        serde_json::json!({
                            "ok": true,
                            "status": "approval_required",
                            "proposal_sha256": "6".repeat(64),
                            "approval_contract_sha256": "7".repeat(64),
                            "tool_surface_sha256": "8".repeat(64),
                            "findings": []
                        })
                        .to_string()
                    } else {
                        serde_json::json!({
                            "appId": "aaaa1111",
                            "manifestRevision": 3,
                            "userGoalSha256": "0000000000000000000000000000000000000000000000000000000000000000",
                            "summary": "No bounded MCP surface available",
                            "tools": [],
                            "requiredFlowChanges": [],
                            "excludedCapabilities": []
                        })
                        .to_string()
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        false,
        Some(initial_args.to_string()),
        None,
    )
    .expect("zero-tool mcp authoring workflow should execute");
    let zero_tool_result = zero_tool_outcome.result.expect("zero-tool workflow result");
    assert!(zero_tool_result.contains("\"status\":\"mcp_authoring_required\""));
    assert!(zero_tool_result.contains("\"candidate_preserved\":true"));
    assert!(zero_tool_result.contains("\"validation\""));
}

// ---------------------------------------------------------------------------
// Durable regression locks for the three `local-app-build.js` behaviour changes
// (retry-on-transient-agent-failure, structured `verification_failed` return,
// and the 4-way half-confirmed naming ternaries).
//
// These were authored and proved red-then-green by the lane that made those
// changes, which could not land them: `plugins/**` was its only owned path and
// this file is the ONLY place in the repo that drives these scripts through the
// real QuickJS runtime. Folded in here so the locks outlive the scratchpad.
//
// ⚠️ HONESTY NOTE: this lane re-ran them GREEN but did NOT re-establish their
// redness, because the mutation site is `local-app-build.js`, a file another
// lane owns and is concurrently editing — planting there is a write across a
// disjoint-owner boundary. Their discriminating power rests on the authoring
// lane's red runs, not on a plant performed here.
// ---------------------------------------------------------------------------

/// The checked-in build workflow, read from disk (not `include_str!`) so a
/// stale build artifact cannot serve an old copy.
fn build_workflow_script() -> String {
    std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read local-app-build.js")
}

/// A minimal well-formed verified-host `create` launch, plus whatever the
/// caller wants layered on top.
fn build_create_args(extra: serde_json::Value) -> serde_json::Value {
    let mut base = serde_json::json!({
        "operation": "create",
        "app_id": "aaaa1111",
        "name": "Form App",
        "brief": "A small form app",
        "authoring_spec": authoring_spec(),
        "quality_level": "balanced",
        "workflow_run_id": "wf_regression",
        "host_context": {
            "source": "verified_host",
            "operation": "create",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_regression",
            "selector_capability": "sel_00000000000000000000000000000000",
            "schemas": workflow_schemas(),
            "template_catalog": {
                "catalog_digest": "digest",
                "available_template_ids": ["react-dom-r2"]
            },
            "staging": {"isolated": true, "final_publish": false},
            "authoring_spec": authoring_spec()
        }
    });
    for (key, value) in extra.as_object().expect("extra must be an object") {
        base.as_object_mut()
            .expect("base is an object")
            .insert(key.clone(), value.clone());
    }
    base
}

fn build_create_args_with_targets(targets: serde_json::Value, quality: &str) -> serde_json::Value {
    let mut spec = authoring_spec();
    let ids: Vec<_> = targets
        .as_array()
        .expect("target fixtures")
        .iter()
        .map(|target| target["id"].clone())
        .collect();
    let presentation = spec["design"]["presentations"][0].clone();
    spec["design"]["presentations"] = serde_json::Value::Array(
        ids.iter()
            .map(|id| {
                let mut presentation = presentation.clone();
                presentation["target_id"] = id.clone();
                presentation
            })
            .collect(),
    );
    spec["acceptance_checks"][0]["target_ids"] = serde_json::json!(ids);
    spec["targets"] = targets;
    let mut args = build_create_args(serde_json::json!({
        "quality_level": quality,
        "authoring_spec": spec.clone(),
    }));
    args["host_context"]["authoring_spec"] = spec;
    args
}

fn build_verify_args(quality: &str) -> serde_json::Value {
    serde_json::json!({
        "operation": "verify",
        "app_id": "aaaa1111",
        "quality_level": quality,
        "workflow_run_id": "wf_verify1",
        "runtime_profile": {
            "family": "react_dom",
            "revision": 1,
            "contract_sha256": "a".repeat(64)
        },
        "expected_writable_collections": [],
        "host_context": {
            "source": "verified_host",
            "operation": "verify",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_verify1",
            "schemas": workflow_schemas(),
            "runtime_profile": {
                "family": "react_dom",
                "revision": 1,
                "contract_sha256": "a".repeat(64),
                "surface": "dom"
            },
            "template_catalog": {
                "catalog_digest": "catalog",
                "available_template_ids": ["react-dom-r2"]
            },
            "expected_writable_collections": [],
            "dependency_snapshot": {"verified": true},
            "authoring_spec": authoring_spec()
        }
    })
}

fn use_test_args(quality: &str) -> serde_json::Value {
    serde_json::json!({
        "app_id": "aaaa1111",
        "scope": "acceptance",
        "scenarios": ["primary-action"],
        "quality_level": quality,
        "workflow_run_id": "wf_use_test",
        "runtime_profile": {
            "family": "react_dom",
            "revision": 1,
            "contract_sha256": "a".repeat(64)
        },
        "host_context": {
            "source": "verified_host",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_use_test",
            "schemas": workflow_schemas(),
            "runtime_profile": {
                "family": "react_dom",
                "revision": 1,
                "contract_sha256": "a".repeat(64),
                "surface": "dom"
            },
            "authoring_spec": authoring_spec()
        }
    })
}

/// Raw evidence identity returned by the non-judging operator.
fn operator_report(qa_handle: &str) -> serde_json::Value {
    serde_json::json!({
        "qa_handle": qa_handle,
        "evidence_ids": ["native-1"],
        "status": "evidence_collected",
        "issues": [],
        "verification_scope": {
            "declared_target_ids": ["primary"],
            "in_scope_target_ids": ["primary"],
            "unverified_target_ids": [],
            "unverified_scenario_ids": []
        },
        "upstream_failures": [],
        "upstream_findings": [],
        "summary": "Host evidence collected"
    })
}

fn partial_operator_report(qa_handle: &str) -> serde_json::Value {
    let mut report = operator_report(qa_handle);
    report["verification_scope"] = serde_json::json!({
        "declared_target_ids": ["primary", "ipad"],
        "in_scope_target_ids": ["primary"],
        "unverified_target_ids": ["ipad"],
        "unverified_scenario_ids": ["ipad-layout"]
    });
    report["upstream_failures"] = serde_json::json!([{
        "id": "environment:ipad-unavailable",
        "message": "the iPad device is unavailable",
        "introduced_at_ms": 10
    }]);
    report["upstream_findings"] = serde_json::json!([{
        "id": "environment:ipad-unavailable",
        "message": "the iPad device is unavailable",
        "blocking": true,
        "resolved_by_evidence_ids": []
    }]);
    report
}

fn qa_candidate_report(
    workflow_run_id: &str,
    quality: &str,
    passed: bool,
    verifier: bool,
    qa_handle: &str,
) -> serde_json::Value {
    let result_id = if verifier {
        "qa-result-verifier"
    } else {
        "qa-result-tester"
    };
    let findings = if passed {
        serde_json::json!([])
    } else {
        serde_json::json!([{
            "id": "source:acceptance-save",
            "message": "the save button does nothing",
            "blocking": true,
            "resolved_by_evidence_ids": []
        }])
    };
    let mut result: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../local-apps/tests/fixtures/qa-result.valid.json"),
        )
        .expect("read the Host-valid checked-in QA result fixture"),
    )
    .expect("parse the Host-valid checked-in QA result fixture");
    result["identity"]["app_id"] = serde_json::Value::String("aaaa1111".into());
    result["identity"]["workflow_run_id"] = serde_json::Value::String(workflow_run_id.into());
    result["identity"]["qa_handle"] = serde_json::Value::String(qa_handle.into());
    result["identity"]["verification_strategy"] = serde_json::Value::String(quality.into());
    result["scenario_judgements"][0]["status"] =
        serde_json::Value::String(if passed { "passed" } else { "failed" }.into());
    result["scenario_judgements"][0]["summary"] =
        serde_json::Value::String(if passed { "Passed" } else { "Failed" }.into());
    result["findings"] = findings;
    result["previous_result_id"] = if verifier {
        serde_json::Value::String("qa-result-tester".into())
    } else {
        serde_json::Value::Null
    };
    result["finalized_at_ms"] = serde_json::Value::from(if verifier { 3 } else { 2 });
    let result_sha256 = result["result_sha256"]
        .as_str()
        .expect("fixture result digest")
        .to_string();
    serde_json::json!({
        // Deliberately opposite on passing results: workflow success must come
        // from the Host candidate, never this model-facing convenience flag.
        "ok": !passed,
        "status": "candidate",
        "receipt": {
            "receipt_id": format!("receipt-{result_id}"),
            "app_id": "aaaa1111",
            "workflow_run_id": workflow_run_id,
            "qa_handle": qa_handle,
            "result_id": result_id,
            "identity_sha256": "a".repeat(64),
            "result_sha256": result_sha256,
            "issued_at_ms": 2
        },
        "result": result
    })
}

fn passing_qa_reply(label: &str, workflow_run_id: &str, quality: &str) -> String {
    let qa_handle = qa_handle_for_label(label);
    if label.starts_with("operator-") {
        operator_report(&qa_handle).to_string()
    } else {
        qa_candidate_report(
            workflow_run_id,
            quality,
            true,
            label.starts_with("verifier"),
            &qa_handle,
        )
        .to_string()
    }
}

fn host_shaped_passing_qa_reply(label: &str, workflow_run_id: &str, quality: &str) -> String {
    let qa_handle = "qa_0123456789abcdef0123456789abcdef";
    if label.starts_with("operator-") {
        operator_report(qa_handle).to_string()
    } else {
        qa_candidate_report(
            workflow_run_id,
            quality,
            true,
            label.starts_with("verifier"),
            qa_handle,
        )
        .to_string()
    }
}

fn failing_qa_reply(label: &str, workflow_run_id: &str, quality: &str) -> String {
    let qa_handle = qa_handle_for_label(label);
    if label.starts_with("operator-") {
        operator_report(&qa_handle).to_string()
    } else {
        qa_candidate_report(
            workflow_run_id,
            quality,
            false,
            label.starts_with("verifier"),
            &qa_handle,
        )
        .to_string()
    }
}

fn qa_handle_for_label(label: &str) -> String {
    if label.contains("resample") {
        return "qa_11111111111111111111111111111111".into();
    }
    let round = label
        .rsplit_once('-')
        .and_then(|(_, suffix)| suffix.parse::<usize>().ok())
        .unwrap_or(0);
    if round == 0 {
        "qa_00000000000000000000000000000000".into()
    } else {
        format!("qa_{round:032}")
    }
}

/// Canned answers for the non-verification stages. `None` means "this stage is
/// operator/tester/verifier — the test decides".
fn canned_stage_reply(label: &str) -> Option<String> {
    Some(match label {
        "template-selector" => serde_json::json!({
            "catalog_digest": "digest",
            "template_id": "react-dom-r2",
            "reason": "ordinary form",
            "rejected": [],
            "validated_selection_handle": "vsel_0123456789abcdef0123456789abcdef"
        })
        .to_string(),
        "create-preparer" => serde_json::json!({
            "approved": true,
            "ok": true,
            "contract_handle": "contract_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contract_sha256": "c".repeat(64),
            "receipt_id": "mcp-create-receipt",
            "status": "create_approved_no_mcp"
        })
        .to_string(),
        "designer" => design_subtree().to_string(),
        _ if label == "builder-build" || label.starts_with("repair-") => serde_json::json!({
            "ok": true,
            "preview_url": "http://127.0.0.1:20000",
            "summary": "built"
        })
        .to_string(),
        _ => return None,
    })
}

fn stage_label(options: &str) -> String {
    serde_json::from_str::<serde_json::Value>(options)
        .unwrap_or(serde_json::Value::Null)
        .get("label")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// r4-failure-paths-05: ONE transient verification-stage failure (the host
/// hands back the null sentinel) is retried once and the create still
/// completes. The Host candidate, rather than any model pass flag, decides the
/// result.
#[test]
fn a_single_transient_verification_agent_failure_is_retried_once() {
    let src = build_workflow_script();
    let operator_calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&operator_calls);
    let outcome = workflow::run_with_progress(
        &src,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") && seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        return workflow::WF_NULL_SENTINEL.to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    );
    let outcome = outcome.unwrap_or_else(|error| {
        panic!("a single transient operator failure must not abort the create: {error}")
    });
    let result = outcome.result.expect("workflow result");
    assert_eq!(
        operator_calls.load(Ordering::SeqCst),
        2,
        "the operator must be dispatched exactly twice (fail, then retry), got {result}"
    );
    assert!(result.contains("\"ok\":true"), "{result}");
    // 6 nominal stages + the one retried operator call.
    assert!(
        result.contains("\"agent_calls\":7"),
        "agent_calls must count the retry: {result}"
    );
}

/// The other half of the retry: a SECOND consecutive failure is still
/// terminal, and the abort names the stage. Without this, `runVerification`
/// could swallow failures forever and the test above would still pass.
#[test]
fn a_persistent_verification_agent_failure_is_still_terminal() {
    let src = build_workflow_script();
    let outcome = workflow::run_with_progress(
        &src,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        return workflow::WF_NULL_SENTINEL.to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    );
    let error = outcome.expect_err("a persistently failing operator must abort the run");
    assert!(
        error.to_string().contains("operator-0"),
        "the abort must name the stage: {error}"
    );
}

/// r4-failure-paths-04: a create that exhausts its repair budget returns a
/// STRUCTURED result rather than throwing an anonymous Error, so the findings,
/// the already-serving preview URL and the create receipt survive to the
/// caller instead of being flattened into a message string.
#[test]
fn create_verification_exhaustion_returns_a_structured_result() {
    let src = build_workflow_script();
    let outcome = workflow::run_with_progress(
        &src,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    canned_stage_reply(&label)
                        .unwrap_or_else(|| failing_qa_reply(&label, "wf_regression", "balanced"))
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    );
    let outcome = outcome.unwrap_or_else(|error| {
        panic!("create verification exhaustion must return a structured result, not throw: {error}")
    });
    let result = outcome.result.expect("workflow result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(result.contains("\"repair_rounds\":1"), "{result}");
    assert!(
        result.contains("the save button does nothing"),
        "the findings must survive: {result}"
    );
    assert!(
        result.contains("http://127.0.0.1:20000"),
        "the already-serving preview_url must survive: {result}"
    );
    assert!(
        result.contains("mcp-create-receipt"),
        "the create receipt must survive: {result}"
    );
}

/// r4-workflow-runtime-03, half-confirmed launch: a launch carrying ONLY the
/// confirmed display name (no brief) must send that name verbatim to both
/// naming stages, and must NOT be reported to the model as carrying nothing.
#[test]
fn a_name_only_confirmed_launch_reaches_both_naming_stages_verbatim() {
    let src = build_workflow_script();
    let prompts_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&prompts_seen);
    workflow::run_with_progress(
        &src,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    sink.lock().expect("prompt sink").push(prompt.clone());
                    let label = stage_label(options);
                    canned_stage_reply(&label)
                        .unwrap_or_else(|| passing_qa_reply(&label, "wf_regression", "balanced"))
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({"name": "Recipe Box"})).to_string()),
        None,
    )
    .expect("a name-only create must run");
    let seen = prompts_seen.lock().expect("prompt sink");
    let find = |needle: &str| {
        seen.iter()
            .find(|prompt| prompt.contains(needle))
            .cloned()
            .unwrap_or_else(|| panic!("no prompt contained {needle:?}"))
    };
    let stage = find("call LocalAppStageCreate with contract_handle from LocalAppContract");
    assert!(
        stage.contains("name=\"Recipe Box\""),
        "the confirmed display name must reach LocalAppStageCreate verbatim: {stage}"
    );
    assert!(
        !stage.contains("carried no user-confirmed values"),
        "a name-only launch must not be reported as carrying nothing: {stage}"
    );
}

/// A create without confirmed naming fails before any agent can write or stage.
#[test]
fn a_launch_with_no_confirmed_values_falls_back_to_prose() {
    let src = build_workflow_script();
    let prompts_seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&prompts_seen);
    let mut bare_args = build_create_args(serde_json::json!({}));
    bare_args
        .as_object_mut()
        .expect("create args")
        .remove("name");
    bare_args
        .as_object_mut()
        .expect("create args")
        .remove("brief");
    let error = workflow::run_with_progress(
        &src,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    sink.lock().expect("prompt sink").push(prompt.clone());
                    let label = stage_label(options);
                    canned_stage_reply(&label)
                        .unwrap_or_else(|| passing_qa_reply(&label, "wf_regression", "balanced"))
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(bare_args.to_string()),
        None,
    )
    .expect_err("a create without confirmed naming must fail closed");
    assert!(error.to_string().contains("CREATE_NAMING_REQUIRED"));
    assert!(prompts_seen.lock().expect("prompt sink").is_empty());
}

#[test]
fn stale_template_selection_stops_before_preparer_or_builder() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if label == "template-selector" {
                        serde_json::json!({
                            "catalog_digest": "stale",
                            "template_id": "react-dom-r2",
                            "reason": "stale",
                            "rejected": [],
                            "validated_selection_handle": "vsel_0123456789abcdef0123456789abcdef"
                        })
                        .to_string()
                    } else {
                        panic!("stale selection must stop before {label}")
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    );
    let error = outcome.expect_err("stale selection must fail closed");
    assert!(error.to_string().contains("stale or unavailable template"));
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        ["template-selector"]
    );
}

#[test]
fn create_denial_returns_normal_result_without_builder_or_contract_erasure() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options.iter().map(|options| {
                let label = stage_label(options);
                calls_for_run.lock().expect("call log").push(label.clone());
                match label.as_str() {
                    "template-selector" => serde_json::json!({
                        "catalog_digest": "digest",
                        "template_id": "react-dom-r2",
                        "reason": "form",
                        "rejected": [],
                        "validated_selection_handle": "vsel_0123456789abcdef0123456789abcdef"
                    }).to_string(),
                    "designer" => design_subtree().to_string(),
                    "create-preparer" => serde_json::json!({"ok": false, "approved": false, "status": "create_declined"}).to_string(),
                    _ => panic!("denial must stop before {label}"),
                }
            }).collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("user denial is a normal workflow result");
    let result = outcome.result.expect("denial result");
    assert!(result.contains("\"status\":\"create_declined\""));
    assert!(result.contains("\"ok\":false"));
    assert!(!calls
        .lock()
        .expect("call log")
        .iter()
        .any(|label| label == "builder-build"));
}

#[test]
fn qa_success_without_host_receipt_fails_closed() {
    let source = build_workflow_script();
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    canned_stage_reply(&label).unwrap_or_else(|| {
                        if label.starts_with("operator-") {
                            return operator_report("qa_00000000000000000000000000000000")
                                .to_string();
                        }
                        let mut report = qa_candidate_report(
                            "wf_regression",
                            "balanced",
                            true,
                            false,
                            "qa_00000000000000000000000000000000",
                        );
                        if label == "tester-0" {
                            report
                                .as_object_mut()
                                .expect("report object")
                                .remove("receipt");
                        }
                        report.to_string()
                    })
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    );
    let error = outcome.expect_err("Host receipt is required");
    assert!(error
        .to_string()
        .contains("tester must return the complete Host QA candidate and receipt"));
}

#[test]
fn thorough_requires_tester_finalize_before_verifier() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        return operator_report("qa_00000000000000000000000000000000").to_string();
                    }
                    if label == "tester-0" {
                        let mut report = qa_candidate_report(
                            "wf_regression",
                            "thorough",
                            true,
                            false,
                            "qa_00000000000000000000000000000000",
                        );
                        report.as_object_mut().expect("candidate").remove("receipt");
                        return report.to_string();
                    }
                    panic!("verifier must not run without tester receipt: {label}");
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({"quality_level": "thorough"})).to_string()),
        None,
    );
    let error = outcome.expect_err("thorough tester receipt is mandatory");
    assert!(
        error
            .to_string()
            .contains("tester must return the complete Host QA candidate and receipt"),
        "{error}"
    );
    assert!(
        !calls
            .lock()
            .expect("call log")
            .iter()
            .any(|label| label == "verifier"),
        "verifier ran before tester finalized"
    );
}

#[test]
fn infrastructure_failure_never_enters_source_repair() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    canned_stage_reply(&label).unwrap_or_else(|| {
                        if label.starts_with("operator-") {
                            partial_operator_report("qa_00000000000000000000000000000000")
                                .to_string()
                        } else {
                            serde_json::json!({
                                "status": "infrastructure_failed",
                                "qa_handle": "qa_00000000000000000000000000000000",
                                "findings": [],
                                "summary": "Host QA backend unavailable"
                            })
                            .to_string()
                        }
                    })
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("infrastructure failure should remain a structured workflow result");
    let result = outcome.result.expect("workflow result");
    assert!(
        result.contains("\"status\":\"infrastructure_failed\""),
        "{result}"
    );
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost on infrastructure failure: {result}"
    );
    assert!(result.contains("\"repair_rounds\":0"), "{result}");
    assert!(
        !calls
            .lock()
            .expect("call log")
            .iter()
            .any(|label| label.starts_with("repair-")),
        "infrastructure failure was sent to source repair"
    );
}

#[test]
fn evidence_resample_is_once_and_separate_from_source_repair() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label == "tester-0" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": "qa_00000000000000000000000000000000",
                            "findings": [],
                            "summary": "capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("one evidence resample should recover");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert!(result.contains("\"evidence_resamples\":1"), "{result}");
    assert!(result.contains("\"repair_rounds\":0"), "{result}");
    let calls = calls.lock().expect("call log");
    assert!(calls.iter().any(|label| label == "operator-resample"));
    assert!(calls.iter().any(|label| label == "tester-resample"));
    assert!(!calls.iter().any(|label| label.starts_with("repair-")));
}

#[test]
fn exhausted_evidence_resample_preserves_build_partial_scope() {
    let source = build_workflow_script();
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        return partial_operator_report(&qa_handle_for_label(&label)).to_string();
                    }
                    if label == "tester-0" || label == "tester-resample" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": if label == "tester-0" {
                                "qa_00000000000000000000000000000000"
                            } else {
                                "qa_11111111111111111111111111111111"
                            },
                            "findings": [],
                            "summary": "capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("exhausted evidence resample should remain structured");
    let result = outcome.result.expect("workflow result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(result.contains("\"evidence_resamples\":1"), "{result}");
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost after exhausted resample: {result}"
    );
}

#[test]
fn verifier_evidence_resample_preserves_build_partial_scope() {
    let source = build_workflow_script();
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        return partial_operator_report(&qa_handle_for_label(&label)).to_string();
                    }
                    if label == "verifier" || label == "verifier-resample" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": if label == "verifier" {
                                "qa_00000000000000000000000000000000"
                            } else {
                                "qa_11111111111111111111111111111111"
                            },
                            "findings": [],
                            "summary": "verifier capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "thorough")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({"quality_level": "thorough"})).to_string()),
        None,
    )
    .expect("verifier evidence resample should remain structured");
    let result = outcome.result.expect("workflow result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost on verifier disposition: {result}"
    );
}

#[test]
fn verify_operation_can_use_its_one_evidence_resample_without_repair() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if label == "tester-0" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": "qa_00000000000000000000000000000000",
                            "findings": [],
                            "summary": "capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_verify1", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_verify_args("balanced").to_string()),
        None,
    )
    .expect("verify should recover through its bounded evidence resample");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert!(result.contains("\"evidence_resamples\":1"), "{result}");
    assert!(result.contains("\"repair_rounds\":0"), "{result}");
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        [
            "operator-0",
            "tester-0",
            "operator-resample",
            "tester-resample"
        ]
    );
}

#[test]
fn evidence_resample_remains_available_after_the_final_source_repair() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label == "operator-0" || label == "tester-0" {
                        return failing_qa_reply(&label, "wf_regression", "balanced");
                    }
                    if label == "tester-1" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": "qa_00000000000000000000000000000001",
                            "findings": [],
                            "summary": "post-rebuild capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("the independent evidence budget should survive the final source repair");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert!(result.contains("\"repair_rounds\":1"), "{result}");
    assert!(result.contains("\"evidence_resamples\":1"), "{result}");
    let calls = calls.lock().expect("call log");
    assert!(calls.iter().any(|label| label == "repair-1"), "{calls:?}");
    assert!(
        calls.iter().any(|label| label == "operator-resample")
            && calls.iter().any(|label| label == "tester-resample"),
        "{calls:?}"
    );
}

#[test]
fn source_repair_budgets_are_one_one_two() {
    let source = build_workflow_script();
    for (quality, expected_repairs) in [("fast", 1usize), ("balanced", 1), ("thorough", 2)] {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls_for_run = Arc::clone(&calls);
        let outcome = workflow::run_with_progress(
            &source,
            move |_prompts, options| {
                options
                    .iter()
                    .map(|options| {
                        let label = stage_label(options);
                        calls_for_run.lock().expect("call log").push(label.clone());
                        canned_stage_reply(&label)
                            .unwrap_or_else(|| failing_qa_reply(&label, "wf_regression", quality))
                    })
                    .collect()
            },
            |_progress| {},
            None,
            true,
            Some(build_create_args(serde_json::json!({"quality_level": quality})).to_string()),
            None,
        )
        .unwrap_or_else(|error| panic!("{quality} failure must return findings: {error}"));
        let result = outcome.result.expect("workflow result");
        assert!(
            result.contains("\"status\":\"verification_failed\""),
            "{result}"
        );
        assert!(
            result.contains(&format!("\"repair_rounds\":{expected_repairs}")),
            "{result}"
        );
        let repair_calls = calls
            .lock()
            .expect("call log")
            .iter()
            .filter(|label| label.starts_with("repair-"))
            .count();
        assert_eq!(repair_calls, expected_repairs, "{quality}: {result}");
    }
}

#[test]
fn non_source_host_candidate_does_not_consume_repair_budget() {
    let source = build_workflow_script();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    canned_stage_reply(&label).unwrap_or_else(|| {
                        if label.starts_with("operator-") {
                            return operator_report("qa_00000000000000000000000000000000")
                                .to_string();
                        }
                        let mut candidate = qa_candidate_report(
                            "wf_regression",
                            "balanced",
                            false,
                            false,
                            "qa_00000000000000000000000000000000",
                        );
                        candidate["result"]["findings"][0]["id"] =
                            serde_json::Value::String("contract:acceptance-mismatch".into());
                        candidate.to_string()
                    })
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("non-source failure should be a structured result");
    let result = outcome.result.expect("workflow result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(result.contains("\"repair_rounds\":0"), "{result}");
    assert!(
        !calls
            .lock()
            .expect("call log")
            .iter()
            .any(|label| label.starts_with("repair-")),
        "a non-source Host finding entered repair"
    );
}

#[test]
fn repair_uses_no_expired_contract_and_denies_contract_manifest_dependency_tools() {
    let source = build_workflow_script();
    let repair_prompt = Arc::new(Mutex::new(String::new()));
    let repair_options = Arc::new(Mutex::new(String::new()));
    let repair_prompt_for_run = Arc::clone(&repair_prompt);
    let repair_options_for_run = Arc::clone(&repair_options);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    if label == "repair-1" {
                        *repair_prompt_for_run.lock().expect("repair prompt") = prompt.clone();
                        *repair_options_for_run.lock().expect("repair options") = options.clone();
                        return canned_stage_reply(&label).expect("repair build");
                    }
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label == "operator-0" {
                        failing_qa_reply(&label, "wf_regression", "balanced")
                    } else if label == "tester-0" {
                        let mut report = qa_candidate_report(
                            "wf_regression",
                            "balanced",
                            false,
                            false,
                            "qa_00000000000000000000000000000000",
                        );
                        report["result"]["identity"]["runtime_profile"]["family"] =
                            serde_json::Value::String("canvas_2d".into());
                        report.to_string()
                    } else {
                        passing_qa_reply(&label, "wf_regression", "balanced")
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("one source repair should recover");
    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert!(result.contains("\"repair_rounds\":1"), "{result}");
    let prompt = repair_prompt.lock().expect("repair prompt");
    assert!(
        prompt.contains("LocalAppBuild with app_id=aaaa1111 and workflow_run_id=wf_regression; omit contract_handle"),
        "{prompt}"
    );
    assert!(
        prompt.contains("do not reuse it and do not restage"),
        "{prompt}"
    );
    assert!(
        prompt.contains(r#""family":"canvas_2d""#),
        "repair must carry the authoritative non-DOM Host QA profile: {prompt}"
    );
    assert!(
        prompt.contains("Skill('lingxi-local-app:apple-design')")
            && prompt.contains("[\"primary\"]"),
        "create repair must preserve the originally applicable Apple design scope: {prompt}"
    );
    let options: serde_json::Value =
        serde_json::from_str(&repair_options.lock().expect("repair options"))
            .expect("repair options JSON");
    let denied = options["disallowedTools"]
        .as_array()
        .expect("repair disallowedTools");
    for tool in [
        "LocalAppContract",
        "LocalAppManifest",
        "LocalAppInstallDeps",
        "LocalAppConfirmDependencyChange",
        "LocalAppUpdateDependencies",
        "LocalAppScaffold",
        "LocalAppApproveMcpProposal",
    ] {
        assert!(
            denied.iter().any(|entry| entry.as_str() == Some(tool)),
            "repair did not deny {tool}: {options}"
        );
    }
}

#[test]
fn valid_host_evidence_ids_are_not_truncated_from_tester_prompt() {
    let source = build_workflow_script();
    let tester_prompt = Arc::new(Mutex::new(String::new()));
    let tester_prompt_for_run = Arc::clone(&tester_prompt);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label == "operator-0" {
                        let evidence_ids = (0..64)
                            .map(|index| format!("evidence-{index:03}"))
                            .collect::<Vec<_>>();
                        return serde_json::json!({
                            "qa_handle": "qa_00000000000000000000000000000000",
                            "evidence_ids": evidence_ids,
                            "status": "evidence_collected",
                            "issues": [],
                            "verification_scope": {
                                "declared_target_ids": ["primary"],
                                "in_scope_target_ids": ["primary"],
                                "unverified_target_ids": [],
                                "unverified_scenario_ids": []
                            },
                            "upstream_failures": [],
                            "upstream_findings": [],
                            "summary": "complete"
                        })
                        .to_string();
                    }
                    if label == "tester-0" {
                        *tester_prompt_for_run.lock().expect("tester prompt") = prompt.clone();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("workflow with 64 valid evidence ids should run");
    assert!(outcome.result.expect("result").contains("\"ok\":true"));
    let prompt = tester_prompt.lock().expect("tester prompt");
    assert!(
        prompt.contains("evidence-063"),
        "the last valid Host evidence id was truncated: {prompt}"
    );
}

#[test]
fn post_repair_qa_carries_host_ledger_resolution_and_partial_scope_into_tester() {
    let source = build_workflow_script();
    let tester_prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let tester_prompts_for_run = Arc::clone(&tester_prompts);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    if label == "tester-1" {
                        tester_prompts_for_run
                            .lock()
                            .expect("tester prompts")
                            .push(prompt.clone());
                    }
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        let evidence = if label == "operator-1" {
                            vec!["repair-evidence"]
                        } else {
                            vec!["native-1"]
                        };
                        let mut report = operator_report(&qa_handle_for_label(&label));
                        report["evidence_ids"] = serde_json::json!(evidence);
                        if label == "operator-1" {
                            report["verification_scope"] = serde_json::json!({
                                "declared_target_ids": ["primary", "ipad"],
                                "in_scope_target_ids": ["primary"],
                                "unverified_target_ids": ["ipad"],
                                "unverified_scenario_ids": ["ipad-layout"]
                            });
                            report["declared_target_ids"] = serde_json::json!(["primary", "ipad"]);
                            report["target_ids"] = serde_json::json!(["primary"]);
                            report["unverified_target_ids"] = serde_json::json!(["ipad"]);
                            report["unverified_scenario_ids"] = serde_json::json!(["ipad-layout"]);
                            report["upstream_findings"] = serde_json::json!([{
                                "id": "source:acceptance-save",
                                "message": "the save action produced no bridge write",
                                "blocking": true,
                                "resolved_by_evidence_ids": []
                            }]);
                            report["upstream_failures"] = serde_json::json!([{
                                "id": "source:acceptance-save",
                                "message": "the save action produced no bridge write",
                                "introduced_at_ms": 10
                            }]);
                        }
                        return report.to_string();
                    }
                    if label == "tester-0" {
                        return failing_qa_reply(&label, "wf_regression", "balanced");
                    }
                    if label == "tester-1" {
                        let mut report = qa_candidate_report(
                            "wf_regression",
                            "balanced",
                            true,
                            false,
                            "qa_00000000000000000000000000000001",
                        );
                        report["result"]["verification_scope"] = serde_json::json!({
                            "declared_target_ids": ["primary", "ipad"],
                            "in_scope_target_ids": ["primary"],
                            "unverified_target_ids": ["ipad"],
                            "unverified_scenario_ids": ["ipad-layout"]
                        });
                        report["result"]["findings"] = serde_json::json!([{
                            "id": "source:acceptance-save",
                            "message": "the save action produced no bridge write",
                            "blocking": false,
                            "resolved_by_evidence_ids": ["repair-evidence"]
                        }]);
                        return report.to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect("post-repair Host ledger flow should execute");

    let result = outcome.result.expect("workflow result");
    assert!(result.contains("\"repair_rounds\":1"), "{result}");
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]"),
        "{result}"
    );
    let prompts = tester_prompts.lock().expect("tester prompts");
    let prompt = prompts.first().expect("post-repair tester prompt");
    assert!(
        prompt.contains("source:acceptance-save"),
        "ledger finding lost: {prompt}"
    );
    assert!(
        prompt.contains("the save action produced no bridge write"),
        "ledger message lost: {prompt}"
    );
    assert!(
        prompt.contains("repair-evidence"),
        "exact resolution evidence path lost: {prompt}"
    );
    assert!(
        prompt.contains("unverified_target_ids"),
        "partial scope lost: {prompt}"
    );
}

#[test]
fn use_test_reads_persisted_authoring_spec_and_rejects_overrides() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let args = use_test_args("balanced");
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    passing_qa_reply(&label, "wf_use_test", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("use-test should consume the Host-persisted contract");
    let result = outcome.result.expect("use-test result");
    assert!(result.contains("\"ok\":true"), "{result}");
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        ["operator-0", "tester-0"]
    );

    let mut forged = args;
    forged
        .as_object_mut()
        .expect("args")
        .insert("authoring_spec".into(), authoring_spec());
    let error = workflow::run_with_progress(
        &source,
        |_prompts, _options| panic!("override must fail before agent dispatch"),
        |_progress| {},
        None,
        true,
        Some(forged.to_string()),
        None,
    )
    .expect_err("use-test must reject caller-supplied AuthoringSpec");
    assert!(
        error
            .to_string()
            .contains("unknown field(s): authoring_spec"),
        "{error}"
    );
}

#[test]
fn use_test_qa_prompts_do_not_echo_host_schemas_or_catalog_capabilities() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let prompts_seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let prompts_for_run = Arc::clone(&prompts_seen);
    let mut args = use_test_args("balanced");
    let context = args["host_context"]
        .as_object_mut()
        .expect("host context object");
    context["schemas"]["schema_secret"] =
        serde_json::Value::String("schema_secret_sentinel".into());
    context.insert(
        "template_catalog".into(),
        serde_json::json!({"catalog_secret": "catalog_secret_sentinel"}),
    );
    context.insert(
        "selector_capability".into(),
        serde_json::Value::String("selector_secret_sentinel".into()),
    );
    workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    prompts_for_run
                        .lock()
                        .expect("prompt capture")
                        .push(prompt.clone());
                    passing_qa_reply(&stage_label(options), "wf_use_test", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("use-test should retain only its bounded QA context");
    for prompt in prompts_seen.lock().expect("prompts").iter() {
        for secret in [
            "selector_secret_sentinel",
            "catalog_secret_sentinel",
            "schema_secret_sentinel",
        ] {
            assert!(
                !prompt.contains(secret),
                "use-test prompt leaked {secret}: {prompt}"
            );
        }
        assert!(
            prompt.contains("primary-action"),
            "use-test prompt lost the persisted acceptance identity: {prompt}"
        );
    }
}

#[test]
fn use_test_carries_bounded_requested_intent_through_initial_and_resample_prompts() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let prompts_seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let prompts_for_run = Arc::clone(&prompts_seen);
    let mut args = use_test_args("balanced");
    args["scope"] = serde_json::Value::String("focus-on-distinct-user-journey".into());
    args["scenarios"] = serde_json::json!(["distinct-scenario-alpha", "distinct-scenario-beta"]);
    let outcome = workflow::run_with_progress(
        &source,
        move |prompts, options| {
            prompts
                .iter()
                .zip(options.iter())
                .map(|(prompt, options)| {
                    let label = stage_label(options);
                    prompts_for_run
                        .lock()
                        .expect("prompt capture")
                        .push((label.clone(), prompt.clone()));
                    if label.starts_with("operator-") {
                        return operator_report(&qa_handle_for_label(&label)).to_string();
                    }
                    if label == "tester-0" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": "qa_00000000000000000000000000000000",
                            "findings": [],
                            "summary": "capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_use_test", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(args.to_string()),
        None,
    )
    .expect("use-test requested intent flow should execute");
    assert!(outcome
        .result
        .expect("use-test result")
        .contains("\"ok\":true"));

    let prompts = prompts_seen.lock().expect("prompts");
    for label in [
        "operator-0",
        "tester-0",
        "operator-resample",
        "tester-resample",
    ] {
        let prompt = prompts
            .iter()
            .find(|(seen_label, _)| seen_label == label)
            .map(|(_, prompt)| prompt)
            .unwrap_or_else(|| panic!("missing {label} prompt: {prompts:?}"));
        assert!(
            prompt.contains("focus-on-distinct-user-journey"),
            "{label} prompt lost requested scope: {prompt}"
        );
        assert!(
            prompt.contains("distinct-scenario-alpha") && prompt.contains("distinct-scenario-beta"),
            "{label} prompt lost requested scenarios: {prompt}"
        );
        assert!(
            prompt.contains("untrusted") && prompt.contains("Host"),
            "{label} prompt must subordinate requested intent to Host authority: {prompt}"
        );
    }
}

#[test]
fn use_test_returns_host_candidate_failure_instead_of_throwing() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| failing_qa_reply(&stage_label(options), "wf_use_test", "balanced"))
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(use_test_args("balanced").to_string()),
        None,
    )
    .expect("a Host verification failure is a structured workflow outcome");
    let result = outcome.result.expect("use-test result");
    assert!(result.contains("\"ok\":false"), "{result}");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(result.contains("receipt-qa-result-tester"), "{result}");
    assert!(result.contains("the save button does nothing"), "{result}");
}

#[test]
fn use_test_returns_infrastructure_failure_without_throwing_or_resampling() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_run = Arc::clone(&calls);
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    calls_for_run.lock().expect("call log").push(label.clone());
                    if label.starts_with("operator-") {
                        partial_operator_report("qa_00000000000000000000000000000000").to_string()
                    } else {
                        serde_json::json!({
                            "status": "infrastructure_failed",
                            "qa_handle": "qa_00000000000000000000000000000000",
                            "findings": [],
                            "summary": "Host QA backend unavailable"
                        })
                        .to_string()
                    }
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(use_test_args("balanced").to_string()),
        None,
    )
    .expect("infrastructure failure is a structured workflow outcome");
    let result = outcome.result.expect("use-test result");
    assert!(result.contains("\"ok\":false"), "{result}");
    assert!(
        result.contains("\"status\":\"infrastructure_failed\""),
        "{result}"
    );
    assert!(result.contains("\"evidence_resamples\":0"), "{result}");
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost on use-test infrastructure failure: {result}"
    );
    assert_eq!(
        calls.lock().expect("call log").as_slice(),
        ["operator-0", "tester-0"]
    );
}

#[test]
fn use_test_exhausted_evidence_resample_preserves_partial_scope() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if label.starts_with("operator-") {
                        return partial_operator_report(&qa_handle_for_label(&label)).to_string();
                    }
                    if label == "tester-0" || label == "tester-resample" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": if label == "tester-0" {
                                "qa_00000000000000000000000000000000"
                            } else {
                                "qa_11111111111111111111111111111111"
                            },
                            "findings": [],
                            "summary": "capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_use_test", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(use_test_args("balanced").to_string()),
        None,
    )
    .expect("use-test exhausted evidence resample should remain structured");
    let result = outcome.result.expect("use-test result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(result.contains("\"evidence_resamples\":1"), "{result}");
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost after use-test resample exhaustion: {result}"
    );
}

#[test]
fn use_test_verifier_disposition_preserves_partial_scope() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if label.starts_with("operator-") {
                        return partial_operator_report(&qa_handle_for_label(&label)).to_string();
                    }
                    if label == "verifier" || label == "verifier-resample" {
                        return serde_json::json!({
                            "status": "evidence_resample_required",
                            "qa_handle": if label == "verifier" {
                                "qa_00000000000000000000000000000000"
                            } else {
                                "qa_11111111111111111111111111111111"
                            },
                            "findings": [],
                            "summary": "verifier capture was incomplete"
                        })
                        .to_string();
                    }
                    passing_qa_reply(&label, "wf_use_test", "thorough")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(use_test_args("thorough").to_string()),
        None,
    )
    .expect("use-test verifier disposition should remain structured");
    let result = outcome.result.expect("use-test result");
    assert!(
        result.contains("\"status\":\"verification_failed\""),
        "{result}"
    );
    assert!(
        result.contains("\"unverified_target_ids\":[\"ipad\"]")
            && result.contains("\"unverified_scenario_ids\":[\"ipad-layout\"]"),
        "partial Host verification scope was lost on use-test verifier disposition: {result}"
    );
}

#[test]
fn verifier_and_tester_agent_grants_remain_read_only() {
    let agents = workflow_dir().parent().expect("plugin root").join("agents");
    let verifier = std::fs::read_to_string(agents.join("verifier.md")).expect("verifier");
    let tester = std::fs::read_to_string(agents.join("tester.md")).expect("tester");
    let designer = std::fs::read_to_string(agents.join("designer.md")).expect("designer");
    for (role, source) in [("verifier", &verifier), ("tester", &tester)] {
        let grants = source.split("---").nth(1).unwrap_or_default();
        for forbidden in [
            "LocalAppInspectUi",
            "LocalAppCaptureUi",
            "LocalAppActOnUi",
            "LocalAppMutateData",
            "LocalAppBuild",
            "LocalAppPromoteMcpCandidate",
            "Write",
            "Edit",
        ] {
            assert!(
                !grants
                    .lines()
                    .any(|line| line.trim() == format!("- {forbidden}")),
                "{role} must not grant {forbidden}: {grants}"
            );
        }
    }
    let designer_grants = designer.split("---").nth(1).unwrap_or_default();
    for engine in [
        "ionic-react-local-app",
        "canvas-2d-local-app",
        "threejs-local-app",
        "phaser-2d-local-app",
        "babylon-3d-local-app",
    ] {
        assert!(
            !designer_grants.contains(engine),
            "designer must not preload engine guide {engine}"
        );
    }
}

#[test]
fn mcp_promotion_uses_a_dedicated_tools_only_agent() {
    let agents = workflow_dir().parent().expect("plugin root").join("agents");
    let promoter = std::fs::read_to_string(agents.join("mcp-promoter.md")).expect("mcp promoter");
    let grants = promoter.split("---").nth(1).unwrap_or_default();
    assert!(
        grants
            .lines()
            .any(|line| line.trim() == "- LocalAppPromoteMcpCandidate"),
        "promoter must have the Host promotion tool: {grants}"
    );
    for forbidden in [
        "Write",
        "Edit",
        "LocalAppBuild",
        "LocalAppMutateData",
        "LocalAppQaFinalize",
    ] {
        assert!(
            !grants
                .lines()
                .any(|line| line.trim() == format!("- {forbidden}")),
            "promoter must not grant {forbidden}: {grants}"
        );
    }
    let workflow = std::fs::read_to_string(workflow_dir().join("local-app-mcp-authoring.js"))
        .expect("mcp workflow");
    assert!(
        workflow.contains("agentType: 'mcp-promoter'"),
        "promotion must run as the dedicated promoter role"
    );
    assert!(
        !workflow.contains("agentType: 'verifier', label: 'mcp-promote'"),
        "verifier must not own MCP promotion"
    );
    let inventory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../apps/engine-mobile/builtin-plugin-inventory.txt");
    let inventory = std::fs::read_to_string(inventory).expect("plugin inventory");
    assert!(
        inventory
            .lines()
            .any(|line| line.trim() == "agents/mcp-promoter.md"),
        "promoter must be registered in the compiled plugin inventory"
    );
}

#[test]
fn mcp_proposal_schema_uses_the_actual_host_dto_wire_names() {
    let path = workflow_dir()
        .parent()
        .expect("plugin root")
        .join("schemas/mcp-proposal.schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read MCP proposal schema"))
            .expect("parse MCP proposal schema");
    let properties = schema["properties"]
        .as_object()
        .expect("proposal properties");
    for required in [
        "appId",
        "manifestRevision",
        "userGoalSha256",
        "tools",
        "requiredFlowChanges",
        "excludedCapabilities",
    ] {
        assert!(
            properties.contains_key(required),
            "missing Host DTO field {required}"
        );
    }
    for stale in [
        "app_id",
        "manifest_revision",
        "user_goal_sha256",
        "required_flow_changes",
        "excluded_capabilities",
    ] {
        assert!(
            !properties.contains_key(stale),
            "stale non-DTO field {stale}"
        );
    }
    assert_eq!(schema["additionalProperties"], false);
    assert!(
        schema["properties"]["tools"].get("minItems").is_none(),
        "the Host DTO and workflow both support a zero-tool candidate"
    );
    let tool = schema["$defs"]["tool"]["properties"]
        .as_object()
        .expect("tool properties");
    for field in [
        "inputSchema",
        "outputSchema",
        "semanticFlowId",
        "inputs",
        "result",
    ] {
        assert!(
            tool.contains_key(field),
            "missing AppMcpToolProposal field {field}"
        );
    }
    let variants = schema["$defs"]["flow_value_binding"]["oneOf"]
        .as_array()
        .expect("FlowValueBinding variants");
    for tag in ["literal", "tool_input", "step_output"] {
        assert!(
            variants
                .iter()
                .any(|variant| variant["properties"].get(tag).is_some()),
            "missing serde FlowValueBinding tag {tag}"
        );
    }
}

#[test]
fn callback_fixtures_are_host_shaped_and_invalid_qa_handles_fail_closed() {
    let spec = authoring_spec();
    assert!(!spec["design"]["presentations"]
        .as_array()
        .expect("fixture presentations")
        .is_empty());
    assert_eq!(spec["targets"][0]["form_factor"], "iphone");
    let candidate = qa_candidate_report(
        "wf_fixture",
        "balanced",
        true,
        false,
        "qa_0123456789abcdef0123456789abcdef",
    );
    assert!(!candidate["result"]["evidence"]
        .as_array()
        .expect("fixture evidence")
        .is_empty());
    assert_eq!(
        candidate["result"]["scenario_judgements"][0]["evidence_ids"][0],
        candidate["result"]["evidence"][0]["evidence_id"]
    );

    let source = build_workflow_script();
    let error = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    if let Some(canned) = canned_stage_reply(&label) {
                        return canned;
                    }
                    if label.starts_with("operator-") {
                        return operator_report("qa_test").to_string();
                    }
                    passing_qa_reply(&label, "wf_regression", "balanced")
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect_err("a permissive callback runner must not make a non-Host QA handle green");
    assert!(error.to_string().contains("qa_<32>"), "{error}");
}

#[test]
fn missing_qa_begin_projection_fails_closed_in_both_workflows() {
    let mut malformed = operator_report("qa_00000000000000000000000000000000");
    malformed
        .as_object_mut()
        .expect("operator fixture object")
        .remove("verification_scope");
    let malformed = malformed.to_string();

    let build = build_workflow_script();
    let malformed_for_build = malformed.clone();
    let build_error = workflow::run_with_progress(
        &build,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    let label = stage_label(options);
                    canned_stage_reply(&label).unwrap_or_else(|| malformed_for_build.clone())
                })
                .collect()
        },
        |_progress| {},
        None,
        true,
        Some(build_create_args(serde_json::json!({})).to_string()),
        None,
    )
    .expect_err("build must reject an operator response without Host scope");
    assert!(
        build_error
            .to_string()
            .contains("complete Host QA scope and ledger projection"),
        "{build_error}"
    );

    let use_test = std::fs::read_to_string(workflow_dir().join("local-app-use-test.js"))
        .expect("read use-test workflow");
    let use_test_error = workflow::run_with_progress(
        &use_test,
        move |_prompts, options| options.iter().map(|_options| malformed.clone()).collect(),
        |_progress| {},
        None,
        true,
        Some(use_test_args("balanced").to_string()),
        None,
    )
    .expect_err("use-test must reject an operator response without Host scope");
    assert!(
        use_test_error
            .to_string()
            .contains("complete Host QA scope and ledger projection"),
        "{use_test_error}"
    );
}
