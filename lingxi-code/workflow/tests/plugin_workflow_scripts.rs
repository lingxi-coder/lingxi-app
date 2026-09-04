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
    Arc,
};

/// 签入的 plugin workflow 脚本目录。
fn workflow_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lingxi-local-app/workflows")
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
            code.lines()
                .any(|line| line.trim_start().starts_with("const WORKFLOW_ID = 'lingxi-local-app:")),
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
        "spec": "A small form app",
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
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "template_catalog": {"catalog_digest": "digest", "available_template_ids": ["react-dom-r2"]},
            "staging": {"isolated": true, "final_publish": false}
        }
    });
    let report = serde_json::json!({
        "ok": true,
        "findings": [],
        "checked_matrix": ["smoke"],
        "browser_available": true,
        "webview_checked": true,
        "degraded_verification": false,
        "data_roundtrip": {"status": "passed"},
        "render_check": {"status": "passed"},
        "motion_check": {"status": "passed"},
        "summary": "hermetic pass"
    });
    let stage = serde_json::json!({"ok": true, "dependency_input_sha256": "a".repeat(64), "summary": "staged"});
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
                    // serialized `options` string. `native-create-approval`'s
                    // agentType is `mcp-designer`, which itself contains the
                    // substring "designer" -- a `options.contains("designer")`
                    // check only stayed correct because it was ORDERED after
                    // the `native-create-approval` check, and would have
                    // silently misdispatched that call to the designer
                    // response had the branches been reordered.
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
                        "native-create-approval" => serde_json::json!({
                            "approved": true,
                            "receipt_id": "mcp-create-receipt",
                            "status": "create_approved_no_mcp"
                        })
                        .to_string(),
                        "designer" => serde_json::json!({"runtime_family":"react_dom","acceptance_checks":[],"summary":"design"}).to_string(),
                        "builder-stage" => stage.to_string(),
                        "builder-build" => build.to_string(),
                        _ if opts_label.starts_with("repair-") => build.to_string(),
                        _ => report.to_string(),
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
        agent_calls, 8,
        "a hermetic create run with quality_level 'balanced' and every stub reporting success \
         must make exactly template-selector, designer, builder-stage, native-create-approval, \
         builder-build, operator, tester, verifier — 8 agent calls (saw: {:?})",
        prompts.iter().map(|(label, _)| label).collect::<Vec<_>>()
    );
    let prompt_for = |label: &str| -> &str {
        prompts
            .iter()
            .find(|(seen_label, _)| seen_label == label)
            .map(|(_, prompt)| prompt.as_str())
            .unwrap_or_else(|| panic!("no agent call was dispatched under label {label:?}"))
    };
    let stage_prompt = prompt_for("builder-stage");
    assert!(
        stage_prompt.contains("Call LocalAppStageCreate with app_id=aaaa1111"),
        "builder-stage prompt must instruct the agent to stage the create candidate for the \
         real app_id: {stage_prompt}"
    );
    assert!(
        stage_prompt.contains("workflow_run_id=wf_hermetic1"),
        "builder-stage prompt must carry the Host-minted workflow_run_id from host_context, \
         not a value the script invented: {stage_prompt}"
    );
    let approval_prompt = prompt_for("native-create-approval");
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
}

#[test]
fn unified_build_verify_is_read_only_and_persisted_canvas_rejects_fast() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-build.js"))
        .expect("read build workflow");
    let verify_args = serde_json::json!({
        "operation": "verify",
        "app_id": "aaaa1111",
        "quality_level": "balanced",
        // r3-workflow-runtime-02: mirror the Host's real top-level injection
        // (workflow_support.rs :1488-1493, :1775-1779) alongside host_context,
        // not only inside it.
        "workflow_run_id": "wf_verify1",
        // The build path's top-level `runtime_profile` carries exactly three
        // fields (workflow_support.rs:1478-1482); `surface` is added only by
        // `enrich_persisted_plugin_workflow_context` on the use-test /
        // mcp-authoring path, so it must NOT appear here.
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
            "dependency_snapshot": {"verified": true}
        }
    });
    let seen_options = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_prompts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let options_for_run = Arc::clone(&seen_options);
    let prompts_for_run = Arc::clone(&seen_prompts);
    let failed_report = serde_json::json!({
        "ok": false,
        "findings": [],
        "checked_matrix": ["webview"],
        "browser_available": true,
        "webview_checked": true,
        "degraded_verification": false,
        "data_roundtrip": {"status": "passed"},
        "render_check": {"status": "not_applicable"},
        "motion_check": {"status": "not_applicable"},
        "summary": "acceptance check failed"
    });
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
            prompts.iter().map(|_| failed_report.to_string()).collect()
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
    assert!(result.contains("verification report returned ok=false"));
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

    let fast_canvas_args = serde_json::json!({
        "operation": "update",
        "app_id": "aaaa1111",
        "quality_level": "fast",
        "host_context": {
            "source": "verified_host",
            "operation": "update",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_update1",
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
            "active_catalog": {"catalog_sha256": "b".repeat(64)}
        }
    });
    let nested_calls = Arc::new(AtomicUsize::new(0));
    let nested_calls_for_run = Arc::clone(&nested_calls);
    let report = serde_json::json!({
        "ok": true,
        "findings": [],
        "checked_matrix": ["webview", "data"],
        "browser_available": true,
        "webview_checked": true,
        "degraded_verification": false,
        "data_roundtrip": {"status": "passed"},
        "render_check": {"status": "not_applicable"},
        "motion_check": {"status": "not_applicable"},
        "summary": "update verified"
    });
    let build = serde_json::json!({
        "ok": true,
        "preview_url": "http://127.0.0.1:20000",
        "summary": "updated"
    });
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    if options.contains("__wf_resolve") {
                        nested_calls_for_run.fetch_add(1, Ordering::SeqCst);
                        "return { status: 'promoted', promotion: { promoted: true } };".to_string()
                    } else if options.contains("builder") {
                        build.to_string()
                    } else {
                        report.to_string()
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
    assert!(result.contains("\"mcp_update\":null"));
    assert_eq!(
        nested_calls.load(Ordering::SeqCst),
        0,
        "ordinary app updates must not run MCP authoring without an explicit user request"
    );
}

#[test]
fn mcp_authoring_workflow_validates_zero_tool_candidates_and_executes_approval_path() {
    let source = std::fs::read_to_string(workflow_dir().join("local-app-mcp-authoring.js"))
        .expect("read mcp workflow");
    let initial_args = serde_json::json!({
        "app_id": "aaaa1111",
        "user_goal": "Expose the saved recipes search as MCP",
        "host_context": {
            "source": "verified_host",
            "operation": "initial",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_mcp_authoring1",
            "invocation_capability": "mcpv_00000000000000000000000000000000",
            "template_catalog": {"catalog_digest": "digest", "available_template_ids": ["react-dom-r2"]},
            "expected_writable_collections": ["recipes"],
            "dependency_snapshot": {"verified": true},
            "active_catalog": null
        }
    });
    let revise_args = serde_json::json!({
        "app_id": "aaaa1111",
        "user_goal": "Expose the saved recipes search as MCP",
        "host_context": {
            "source": "verified_host",
            "operation": "revise",
            "app_id": "aaaa1111",
            "workflow_run_id": "wf_mcp_authoring2",
            "invocation_capability": "mcpv_00000000000000000000000000000000",
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
        "app_id": "aaaa1111",
        "manifest_revision": 3,
        "user_goal_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "summary": "Expose bounded recipe search",
        "tools": [{
            "name": "search_recipes",
            "title": "Search recipes",
            "description": "Search saved recipes",
            "input_schema": {"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},
            "output_schema": {"type":"object","properties":{"items":{"type":"array"}},"required":["items"],"additionalProperties":false},
            "semantic_flow_id": "search_recipes",
            "inputs": {"query":{"toolInput":{"json_pointer":"/query"}}},
            "result": {"stepOutput":{"step_id":"search","json_pointer":"/items"}}
        }],
        "required_flow_changes": [],
        "excluded_capabilities": []
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
    let outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
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

    let evidence_for_zero_tools = evidence_for_promote.clone();
    let proposal_for_revise = proposal.clone();
    let validated_for_revise = validated.clone();
    let approved_for_revise = approved.clone();
    let qa_for_revise = qa.clone();
    let promoted_for_revise = promoted.clone();
    let revise_outcome = workflow::run_with_progress(
        &source,
        move |_prompts, options| {
            options
                .iter()
                .map(|options| {
                    if options.contains("app-evidence") {
                        evidence_for_promote.to_string()
                    } else if options.contains("host-proposal-validation") {
                        validated_for_revise.to_string()
                    } else if options.contains("native-approval") {
                        approved_for_revise.to_string()
                    } else if options.contains("mcp-qa") {
                        qa_for_revise.to_string()
                    } else if options.contains("mcp-promote") {
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
                            "app_id": "aaaa1111",
                            "manifest_revision": 3,
                            "user_goal_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                            "summary": "No bounded MCP surface available",
                            "tools": [],
                            "required_flow_changes": [],
                            "excluded_capabilities": []
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
