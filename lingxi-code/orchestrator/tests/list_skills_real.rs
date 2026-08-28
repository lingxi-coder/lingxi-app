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
    assert_eq!(
        entry.plugin, None,
        "a directory-scanned skill carries no plugin provenance today"
    );
}
