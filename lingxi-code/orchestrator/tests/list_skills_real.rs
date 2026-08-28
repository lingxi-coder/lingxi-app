//! Real (non-mocked) coverage for `OrchestratorHandle::list_skills` (Task 7:
//! desktop Skills settings listing).
//!
//! Mirrors `list_mcp_real.rs`'s shape: build a real `ConversationOrchestrator`
//! (no mock handle) and prove the discovery-to-`SkillInfo` mapping actually
//! reaches through the engine, not just that a hand-built `SkillInfo` survives
//! a no-op field copy. The empty case guards against a router arm that always
//! reports zero skills looking identical to one that is genuinely unwired.

use orchestrator::test_support::{
    noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use std::sync::Arc;
use traits::OrchestratorHandle;

fn build_orch(cwd: std::path::PathBuf) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        Arc::new(MockApiClient::new(vec![])),
        Arc::new(tool_api::registry::ToolRegistry::new()),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        cwd,
    )
}

#[tokio::test]
async fn list_skills_returns_empty_when_no_config_home_is_wired() {
    let orch = Arc::new(build_orch(std::env::temp_dir()));
    let v = orch.list_skills().await;
    assert!(v.is_empty());
}

#[tokio::test]
async fn list_skills_reports_name_and_source_for_a_real_discovered_skill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir
        .path()
        .join(branding::DOT_DIR)
        .join("skills")
        .join("greet");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: greet\ndescription: says hello\n---\n\nhello\n",
    )
    .expect("write SKILL.md");

    // `config_home` is what `list_skills` reads as `lingxi_home`; wiring it to
    // `dir.path()` means the project-tier scan (`<config_home>/.lingxi/skills`)
    // finds the skill written above. This is the SAME wiring the desktop
    // composition root uses (`with_config_home(cfg.lingxi_home.clone())`).
    let orch =
        Arc::new(build_orch(dir.path().to_path_buf()).with_config_home(dir.path().to_path_buf()));

    let v = orch.list_skills().await;
    let entry = v
        .iter()
        .find(|s| s.name == "greet")
        .expect("the discovered skill must be listed by name");
    assert!(
        entry
            .source_dir
            .to_string_lossy()
            .replace('\\', "/")
            .contains("skills/greet"),
        "the entry must name where it was found, got: {}",
        entry.source_dir.display()
    );
}

/// Fix round 1 (review "Important"): a skill living under an `/add-dir`
/// registered root must appear in the listing, not just project/user tiers.
/// `session_cwd.trusted_dirs()` is the SAME live set `register_repo_root`
/// grows via `SessionCwd::add_trusted_dir` — this test wires it directly
/// (via `SessionCwd::new`'s initial `trusted` list) rather than driving a
/// full `/add-dir` round trip, since `list_skills` only reads the resulting
/// set.
#[tokio::test]
async fn list_skills_reports_a_skill_from_an_additional_add_dir_root() {
    let boot = tempfile::tempdir().expect("tempdir");
    let extra_root = tempfile::tempdir().expect("tempdir");

    let extra_skill_dir = extra_root
        .path()
        .join(branding::DOT_DIR)
        .join("skills")
        .join("multi-root-helper");
    std::fs::create_dir_all(&extra_skill_dir).expect("create skill dir");
    std::fs::write(
        extra_skill_dir.join("SKILL.md"),
        "---\nname: multi-root-helper\ndescription: lives in a second workspace root\n---\n\nhi\n",
    )
    .expect("write SKILL.md");

    let session_cwd = tool_api::SessionCwd::new(
        boot.path().to_path_buf(),
        vec![extra_root.path().to_path_buf()],
    );
    let orch = Arc::new(
        build_orch(boot.path().to_path_buf())
            .with_config_home(boot.path().to_path_buf())
            .with_session_cwd(session_cwd),
    );

    let v = orch.list_skills().await;
    assert!(
        v.iter().any(|s| s.name == "multi-root-helper"),
        "a skill under an /add-dir root must be listed, got: {v:?}"
    );
}

/// Fix round 1 (review "Important"): a skill living under the managed
/// (org-policy) directory must appear in the listing too. Overrides
/// `LINGXI_MANAGED_DIR` (the same env var `traits::live_sessions::
/// managed_settings_dir` and the composition root's `managed_settings_dir`
/// both honor) for the duration of the test, guarded by `ENV_LOCK` since the
/// override is process-global and `cargo test` runs functions in this file
/// concurrently.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn list_skills_reports_a_skill_from_the_managed_directory() {
    let _guard = ENV_LOCK.lock().await;
    let prev = std::env::var_os(mcp::enterprise_policy::MANAGED_DIR_ENV);

    let managed_root = tempfile::tempdir().expect("tempdir");
    let managed_skill_dir = managed_root
        .path()
        .join(branding::DOT_DIR)
        .join("skills")
        .join("org-policy-skill");
    std::fs::create_dir_all(&managed_skill_dir).expect("create skill dir");
    std::fs::write(
        managed_skill_dir.join("SKILL.md"),
        "---\nname: org-policy-skill\ndescription: installed by org policy\n---\n\nhi\n",
    )
    .expect("write SKILL.md");
    std::env::set_var(mcp::enterprise_policy::MANAGED_DIR_ENV, managed_root.path());

    let boot = tempfile::tempdir().expect("tempdir");
    let orch =
        Arc::new(build_orch(boot.path().to_path_buf()).with_config_home(boot.path().to_path_buf()));
    let v = orch.list_skills().await;

    match prev {
        Some(v) => std::env::set_var(mcp::enterprise_policy::MANAGED_DIR_ENV, v),
        None => std::env::remove_var(mcp::enterprise_policy::MANAGED_DIR_ENV),
    }

    assert!(
        v.iter().any(|s| s.name == "org-policy-skill"),
        "a skill under the managed directory must be listed, got: {v:?}"
    );
}
