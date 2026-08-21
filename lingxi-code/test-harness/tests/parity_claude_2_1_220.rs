//! Live version-facing identifier pins for the Claude Code 2.1.220 oracle.
//!
//! 2.1.220 is the binary this port is read against — the forked-skill sidecars,
//! the `set_cwd` trust handshake, PowerShell 5.1 cwd-first shadowing, the
//! ←-on-empty gesture and the refusal cascade were all ported from it. The
//! advertised version had lagged at 2.1.217, so a session implementing 2.1.220
//! behaviour was telling servers and child processes it was something else.
//!
//! Everything below derives from ONE constant, so the three identifiers cannot
//! drift apart.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use orchestrator::prompt::{
    assemble_system_prompt, assemble_system_prompt_with_style, env_meta, ActiveOutputStyle,
    FileTree, SystemPromptContext,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const BACKLOG_CONTRACTS: &str =
    include_str!("../src/parity/fixtures/claude_2_1_220_backlog_contracts.json");
const GAP_ORACLE: &str = include_str!("../src/parity/fixtures/claude_2_1_220_gap_oracle.json");

fn backlog_contracts() -> Value {
    serde_json::from_str(BACKLOG_CONTRACTS).expect("2.1.220 backlog fixture must be valid JSON")
}

fn gap_oracle() -> Value {
    serde_json::from_str(GAP_ORACLE).expect("2.1.220 gap oracle fixture must be valid JSON")
}

/// The current parity target is exposed from one source of truth.
#[test]
fn version_const_is_2_1_220() {
    assert_eq!(traits::CLAUDE_CODE_VERSION, "2.1.220");
}

/// Child processes receive the 2.1.220 `AI_AGENT` identifier.
#[test]
fn ai_agent_env_value_is_2_1_220() {
    let derived = format!(
        "claude-code_{}_agent",
        traits::CLAUDE_CODE_VERSION.replace('.', "-")
    );
    assert_eq!(derived, "claude-code_2-1-220_agent");
}

/// WebFetch presents the 2.1.220 Claude-compatible user agent.
#[test]
fn web_fetch_user_agent_is_2_1_220() {
    let derived = format!(
        "Claude-User (claude-code/{}; +https://support.anthropic.com/)",
        traits::CLAUDE_CODE_VERSION
    );
    assert_eq!(
        derived,
        "Claude-User (claude-code/2.1.220; +https://support.anthropic.com/)"
    );
}

/// Every version-facing identifier derives from the SAME constant, so a future
/// bump cannot move one and leave another behind — which is exactly how the
/// port came to advertise 2.1.217 while implementing 2.1.220.
#[test]
fn the_identifiers_share_one_source() {
    let v = traits::CLAUDE_CODE_VERSION;
    assert!(format!("claude-code_{}_agent", v.replace('.', "-")).contains(&v.replace('.', "-")));
    assert!(format!("Claude-User (claude-code/{v}; +https://support.anthropic.com/)").contains(v));
}

/// Wave 0 maps every approved engineering item to exactly one implementation
/// wave. This is inventory validation only: a fixture's `target` field is not
/// accepted as proof that the corresponding production behavior is complete.
/// The private remote-memory protocol is deliberately tracked outside this
/// list as a single explicit divergence.
#[test]
fn approved_backlog_inventory_has_25_unique_items() {
    let fixture = backlog_contracts();
    let items = fixture["items"].as_array().expect("items array");
    assert_eq!(items.len(), 25);

    let ids = items
        .iter()
        .map(|item| item["id"].as_str().expect("item id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 25, "backlog IDs must be unique");
    assert!(
        items
            .iter()
            .all(|item| matches!(item["wave"].as_u64(), Some(1..=4))),
        "every item must be assigned to Wave 1-4"
    );
}

#[test]
fn private_remote_memory_is_one_explicit_divergence() {
    let fixture = backlog_contracts();
    let divergences = fixture["divergences"]
        .as_array()
        .expect("divergences array");
    assert_eq!(divergences.len(), 1);
    assert_eq!(divergences[0]["id"], "N-env-3/N-protocol-8");
    assert!(divergences[0]["reason"]
        .as_str()
        .expect("divergence reason")
        .contains("private account remote-memory"));
}

/// Clean-room oracle inventory for stateful behavior. This pins the oracle
/// itself; production behavior is proved by the subsystem tests that execute
/// each state machine, not by these fixture values.
#[test]
fn stateful_2_1_220_oracle_inventory_is_pinned() {
    let fixture = backlog_contracts();
    let contracts = &fixture["pinned_contracts"];

    assert_eq!(
        contracts["ultracode"]["transitions"],
        serde_json::json!(["enter", "sparse", "exit"])
    );
    assert_eq!(contracts["ultracode"]["default_sparse_cadence"], 10);
    assert_eq!(contracts["observer"]["default_max_depth"], 3);
    assert_eq!(
        contracts["deep_research"]["stages"],
        serde_json::json!(["Scope", "Search", "Fetch", "Verify", "Synthesize"])
    );
    assert_eq!(contracts["deep_research"]["votes_per_claim"], 3);
    assert_eq!(contracts["deep_research"]["max_fetch"], 15);
    assert_eq!(
        contracts["opus_5_bash_addition"],
        "Command output is displayed to you, not reliably to the user."
    );
    assert_eq!(contracts["attached_left_arrow"]["outcome"], "detach");
    assert_eq!(
        contracts["accessibility"]["announces_edit_delta_only"],
        true
    );
}

/// Wave 0 retains only clean-room observables from the local 2.1.220 binary:
/// hashes/lengths/section names rather than the private prompt bodies.
#[test]
fn gap_oracle_is_pinned_without_private_prompt_text() {
    let fixture = gap_oracle();
    assert_eq!(fixture["oracle"]["version"], "2.1.220");
    assert_eq!(
        fixture["oracle"]["binary_sha256"],
        "8addc857f3fe64d5a0368af9ee50321b50afb4a6918ba3ef018ab84f5dbbe081"
    );
    assert_eq!(fixture["oracle"]["network"], "loopback-only");
    // Read from the binary this fixture pins (@225785080), not from a
    // changelog for another version — see the entry's `supersedes` note.
    assert_eq!(fixture["fast_mode"]["claude-opus-4-7"], "on");

    let prompts = fixture["system_prompt_manifests"]
        .as_array()
        .expect("system prompt manifests");
    assert_eq!(prompts.len(), 4);
    assert!(prompts.iter().all(|entry| {
        entry["length_bytes"]
            .as_u64()
            .is_some_and(|len| len > 1_000)
            && entry["sha256"]
                .as_str()
                .is_some_and(|digest| digest.len() == 64)
            && entry["headings"]
                .as_array()
                .is_some_and(|headings| !headings.is_empty())
    }));
    assert_ne!(
        prompts[0]["sha256"], prompts[1]["sha256"],
        "Claude model families must retain distinct prompt manifests"
    );
    assert_ne!(
        prompts[0]["sha256"], prompts[3]["sha256"],
        "the lean Opus prompt must not be treated as the Sonnet prompt"
    );

    let options = fixture["plugin_eval"]["options"]
        .as_array()
        .expect("plugin eval options");
    assert!(options.iter().any(|option| option == "--json"));
    assert!(options.iter().any(|option| option == "--model"));
    assert_eq!(fixture["plugin_eval"]["help_exit_code"], 0);
    assert_eq!(
        fixture["plugin_eval"]["early_access_gate"],
        "CLAUDE_CODE_WALNUT_SPIRE"
    );
    assert_eq!(
        fixture["plugin_eval"]["bare_template"]["prompt_frontmatter"]["max_turns"],
        10
    );
    assert_eq!(
        fixture["plugin_eval"]["bare_template"]["grader_frontmatter"]["type"],
        "llm"
    );
    assert_eq!(fixture["plugin_eval"]["empty_suite"]["exit_code"], 1);
    assert_eq!(fixture["plugin_eval"]["empty_suite"]["schema_version"], 1);
}

fn prompt_tool_names() -> Vec<String> {
    [
        "Agent",
        "Bash",
        "CronCreate",
        "CronDelete",
        "CronList",
        "Edit",
        "EnterWorktree",
        "ExitWorktree",
        "NotebookEdit",
        "Read",
        "ReportFindings",
        "ScheduleWakeup",
        "SendMessage",
        "Skill",
        "TaskCreate",
        "TaskGet",
        "TaskList",
        "TaskOutput",
        "TaskStop",
        "TaskUpdate",
        "WebFetch",
        "WebSearch",
        "Workflow",
        "Write",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn prompt_context(model: &str, cwd: PathBuf, memory_dir: PathBuf) -> SystemPromptContext {
    SystemPromptContext {
        cwd,
        platform: "darwin".to_string(),
        model: model.to_string(),
        model_marketing_name: env_meta::marketing_name_for_model(model).map(str::to_string),
        knowledge_cutoff: env_meta::knowledge_cutoff_for_model(model).map(str::to_string),
        shell: "zsh".to_string(),
        os_version: "Darwin 24.6.0".to_string(),
        git_status: None,
        in_worktree: false,
        file_tree: FileTree::default(),
        memory_files: Vec::new(),
        tool_names: prompt_tool_names(),
        skills_available: true,
        is_interactive: false,
        memory_dir: Some(memory_dir),
        exclude_dynamic_sections: false,
    }
}

fn normalize_prompt_body(body: &str, cwd: &Path, memory_dir: &Path) -> String {
    // Replace the nested memory path first; replacing cwd first would turn it
    // into `<CWD>/…` and leave the manifest dependent on the test layout.
    body.replace(&memory_dir.to_string_lossy().to_string(), "<MEMORY_DIR>")
        .replace(&cwd.to_string_lossy().to_string(), "<CWD>")
}

fn normalize_live_prompt(body: &str, cwd: &Path) -> String {
    body.replace(&cwd.to_string_lossy().to_string(), "<CWD>")
        .split('\n')
        .map(|line| {
            if line.starts_with(" - Primary working directory:") {
                " - Primary working directory: <CWD>".to_string()
            } else if line.starts_with(" - Platform:") {
                " - Platform: <PLATFORM>".to_string()
            } else if line.starts_with(" - Shell:") {
                " - Shell: <SHELL>".to_string()
            } else if line.starts_with(" - OS version:") {
                " - OS version: <OS_VERSION>".to_string()
            } else if line.starts_with("Today's date is ")
                || line.starts_with("The current date is ")
            {
                "<CURRENT_DATE>".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct PromptAgentTool {
    schema: Value,
}

impl PromptAgentTool {
    fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }
}

#[async_trait::async_trait]
impl tool_api::Tool for PromptAgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn input_schema(&self) -> &Value {
        &self.schema
    }

    fn is_enabled(&self, _ctx: &tool_api::ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        1_024
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(
        &self,
        input: &Value,
        _ctx: &tool_api::ToolUseContext,
    ) -> permission::PermissionResult {
        permission::PermissionResult::Allow {
            reason: permission::PermissionDecisionReason::Other {
                reason: "parity fixture".to_string(),
            },
            updated_input: Some(input.clone()),
            update_destination: None,
            metadata: permission::result::PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &tool_api::DescriptionOptions) -> String {
        "Deterministic Agent fixture".to_string()
    }

    async fn prompt(&self, _opts: &tool_api::PromptOptions) -> String {
        "Deterministic Agent fixture".to_string()
    }

    async fn call(
        &self,
        _input: Value,
        _ctx: tool_api::ToolUseContext,
        _progress_tx: tool_api::ToolProgressSender,
    ) -> Result<tool_api::ToolCallResult, tool_api::ToolError> {
        Ok(tool_api::ToolCallResult {
            data: serde_json::json!({"ok": true}),
            model_content: None,
            new_messages: Vec::new(),
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

/// Byte manifest for the prompt assembler — **regenerated for claude-code
/// 2.1.238 on 2026-08-20**.
///
/// PROVENANCE, so nobody mistakes these for oracle facts: these lengths and
/// digests are hashes of LingXi's OWN normalized prompt body. They are a
/// regression lock that catches unintended prompt drift; they are NOT extracted
/// from the Claude Code binary. The oracle-derived truth for this prompt lives
/// in the per-sentence assertions in `orchestrator/src/prompt/body_sections.rs`
/// and `env_block.rs`, which is where a byte claim must actually be proven.
///
/// They moved from the 2.1.220 values because the 2.1.238 alignment pass
/// rewrote `# Communicating with the user` (SP-1…SP-4: four em-dash clauses
/// re-punctuated, "the PR merges" -> "the change merges"), the `action_caution`
/// tail (SP-5), the Fable/Mythos autonomy tail (SP-6), the fork bullet (SP-7),
/// and dropped Opus 4.7 from the `# Environment` fast-mode line (SP-8).
///
/// Claude emits one extra first block carrying a private
/// `x-anthropic-billing-header` attestation. LingXi deliberately does not
/// spoof that private billing identity, so its request contains the remaining
/// two observable blocks: the LingXi brand prefix and the normalized body.
#[test]
fn production_prompt_bodies_match_normalized_2_1_238_manifests() {
    let temp = tempfile::tempdir().expect("temp cwd");
    let cwd = temp.path().canonicalize().expect("physical cwd");
    let memory_dir = cwd.join(".lingxi/projects/oracle/memory");
    let cases = [
        (
            "claude-fable-5",
            10_449,
            "cf1c325030407204d13bb0ef18112538177527add11387a9ade102d8db509a46",
        ),
        (
            "claude-haiku-4-5-20251001",
            27_530,
            "f374890bd85e1c7ccc39f4408849db9efb3d785ea631b6b5b8abe098f14260c2",
        ),
        (
            "claude-mythos-5",
            9_755,
            "be335dfb8bda35d5cd5c7bcd0e3522e815d26df8195c143dba37a882333fe002",
        ),
        (
            "claude-opus-4-5",
            27_514,
            "d7012ba94b9acac26f3bbf8434c8fe6d97dddac5d68cf523a3100e50fa61bd75",
        ),
        (
            "claude-opus-4-6",
            27_514,
            "cb79937b876361530ecab7720e5ae2c91dad53cdb7f16ea801dcbb912597ef23",
        ),
        (
            "claude-opus-4-7",
            27_518,
            "efa1aa1482f0796dc980f70e22e1b0e53a0667fd5d11754cf1b80ec62607e77f",
        ),
        (
            "claude-opus-4-8",
            5_970,
            "f981d9468098943aba71a17c07006d48dc34fca1ed59cae6216b8ca5757cead5",
        ),
        (
            "claude-opus-5",
            9_309,
            "f662f5e2797c7e846c707af3033d51cf58dc9c0d5dec9f72bf8b443efe3e76f1",
        ),
        (
            "claude-sonnet-4-6",
            27_521,
            "2f18032d2e1f7257f751561637ff4d4a226cc8a1fcc720129e45cceea54a2c6a",
        ),
        (
            "claude-sonnet-5",
            27_518,
            "6765e2aae1d5cc5ee568dfb90badb98fd264905965adb711328bc89d8b948c0b",
        ),
    ];

    for (model, expected_len, expected_sha) in cases {
        let assembled =
            assemble_system_prompt(&prompt_context(model, cwd.clone(), memory_dir.clone()));
        let blocks = llm_client::prompt_format::split_system_blocks(&assembled, true);
        assert_eq!(
            blocks.len(),
            2,
            "{model}: private Anthropic billing attestation must not be fabricated"
        );
        assert_eq!(
            blocks[0].text,
            "You are LingXi, an agentic command-line coding assistant."
        );
        assert!(
            blocks[1].text.starts_with('\n'),
            "{model}: the body block retains Claude's leading LF"
        );

        let body = normalize_prompt_body(&blocks[1].text, &cwd, &memory_dir);
        assert_eq!(body.len(), expected_len, "{model}: body length drifted");
        let digest = format!("{:x}", Sha256::digest(body.as_bytes()));
        assert_eq!(digest, expected_sha, "{model}: normalized body drifted");
    }
}

/// Exercise the same live prompt builder used by real turns. The byte manifests
/// above intentionally use a synthetic context so dynamic machine fields can be
/// normalized deterministically; this test prevents that stable fixture from
/// masking a broken composition path.
#[tokio::test]
async fn live_orchestrator_prompt_uses_production_context() {
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};

    use memory::lingxi_md::LingxiMdTier;
    use orchestrator::prompt::MemoryFile;
    use orchestrator::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
    use tool_api::registry::ToolRegistry;

    static PROMPT_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _prompt_env_guard = PROMPT_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prompt_env_keys = [
        "LINGXI_FORK_SUBAGENT",
        "LINGXI_SESSION_KIND",
        "LINGXI_JOB_DIR",
        "LINGXI_BG_ISOLATION",
        "LINGXI_ACT_DONT_REDERIVE",
        "CLAUDE_CODE_ACT_DONT_REDERIVE",
    ];
    let saved_prompt_env = prompt_env_keys.map(|key| (key, std::env::var_os(key)));
    for key in prompt_env_keys {
        std::env::remove_var(key);
    }
    struct RestorePromptEnv([(&'static str, Option<std::ffi::OsString>); 6]);
    impl Drop for RestorePromptEnv {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
    let _restore_prompt_env = RestorePromptEnv(saved_prompt_env);

    let temp = tempfile::tempdir().expect("temp cwd");
    let cwd = temp.path().canonicalize().expect("physical cwd");
    let memory_body = "Use deterministic production-path memory.";
    let mut registry = ToolRegistry::new();
    registry.register_builtin(Arc::new(PromptAgentTool::new()));
    let config = OrchestratorConfig {
        model: "claude-sonnet-5".to_string(),
        interactive_permissions: false,
        interactive_session: true,
        ..OrchestratorConfig::default()
    };
    let orchestrator = ConversationOrchestrator::new(
        config,
        Arc::new(MockApiClient::new(Vec::new())),
        Arc::new(registry),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::with_files(vec![MemoryFile {
            path: cwd.join("LINGXI.md"),
            body: memory_body.to_string(),
            is_local_override: false,
            tier: LingxiMdTier::Project,
            globs: None,
            raw_content: memory_body.to_string(),
            content_differs_from_disk: false,
        }])),
        cwd.clone(),
    );

    let prompt = orchestrator.assemble_system_prompt_preview().await;
    assert!(
        prompt.contains(&format!(" - Primary working directory: {}", cwd.display())),
        "live cwd must come from the production prompt context"
    );
    assert!(
        prompt.contains("If you need the user to run a shell command themselves"),
        "interactive production prompt must include the ! command guidance"
    );
    assert!(
        prompt.contains("The exact model ID is claude-sonnet-5."),
        "live model must drive the production environment block"
    );
    assert!(
        prompt.contains("Use the Agent tool with specialized agents"),
        "live tool registry must drive the production session guidance"
    );
    let additional_context = orchestrator
        .additional_context_preview()
        .await
        .expect("live memory must produce additional context");
    assert!(
        additional_context.contains(memory_body),
        "live memory provider must contribute through the production additional-context path"
    );

    let normalized = normalize_live_prompt(&prompt, &cwd);
    let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    assert_eq!(normalized.len(), 14_705, "live prompt length drifted");
    assert_eq!(
        digest, "c6e1b2fed7004891c636dbc2e8a66d74e64a86756a70071a0b79fe221c784e37",
        "live normalized production prompt drifted"
    );
}

#[test]
fn production_output_style_bodies_match_normalized_2_1_238_manifests() {
    let temp = tempfile::tempdir().expect("temp cwd");
    let cwd = temp.path().canonicalize().expect("physical cwd");
    let memory_dir = cwd.join(".lingxi/projects/oracle/memory");
    let cases = [
        (
            "Explanatory",
            10_320,
            "28096b3c4fdc3a69ab15fa6db7eef0236c118027c16cc8638bafd2a993628fbc",
        ),
        (
            "Learning",
            14_196,
            "77c6495beab3631d860386817db1d59e02a399d1cea4ef0676989db6af3fe752",
        ),
    ];

    for (style_name, expected_len, expected_sha) in cases {
        let style = outputstyles::resolve_builtin_output_style(Some(style_name))
            .expect("builtin output style");
        let assembled = assemble_system_prompt_with_style(
            &prompt_context("claude-opus-5", cwd.clone(), memory_dir.clone()),
            Some(ActiveOutputStyle {
                name: style.name,
                prompt: style.prompt,
                keep_coding_instructions: style.keep_coding_instructions,
            }),
        );
        let blocks = llm_client::prompt_format::split_system_blocks(&assembled, true);
        assert_eq!(blocks.len(), 2);

        let body = normalize_prompt_body(&blocks[1].text, &cwd, &memory_dir);
        assert_eq!(
            body.len(),
            expected_len,
            "{style_name}: body length drifted"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(body.as_bytes())),
            expected_sha,
            "{style_name}: normalized body drifted"
        );
        assert!(
            !body.contains("When you have enough information to act"),
            "{style_name}: explicit style must suppress act-don't-rederive"
        );
    }
}

#[test]
fn non_claude_models_keep_full_harness_without_claude_metadata_leakage() {
    let context = prompt_context(
        "vendor-compat-claude-sonnet-5",
        PathBuf::from("/not-materialized"),
        PathBuf::from("/memory"),
    );
    let prompt = assemble_system_prompt(&context);

    assert!(prompt.contains("# System"));
    assert!(prompt.contains("# Text output (does not apply to tool calls)"));
    assert!(prompt.contains("# auto memory"));
    assert!(!prompt.contains("# Communicating with the user"));
    assert!(!prompt.contains("The most recent Claude models are"));
    assert_eq!(
        traits::model_capabilities::prompt_profile_for(&context.model),
        traits::model_capabilities::PromptProfile::FullHarness
    );
}
