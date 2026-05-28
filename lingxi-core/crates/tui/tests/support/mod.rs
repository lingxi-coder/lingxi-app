//! Shared helpers for the M6-02 behavior tests.
//!
//! Provides a `fake_status` snapshot and a `fake_dispatcher` backed by
//! the real M5-09 registry seeded with built-in stubs.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use lingxi_commands::dispatcher::RegistrySlashDispatcher;
use lingxi_commands::registry::{register_all_builtin_commands, CommandRegistry};
use lingxi_permission::PermissionMode;
use lingxi_tui::state::StatusSnapshot;
use tokio::sync::RwLock;

/// Fixed status snapshot reused across tests.
pub fn fake_status() -> StatusSnapshot {
    StatusSnapshot {
        model: "claude-sonnet-4.5".into(),
        cwd: PathBuf::from("/a/b"),
        cost: "$0.000".to_string(),
        context_pct: 0.0,
        permission_mode: PermissionMode::Default,
    }
}

/// A `RegistrySlashDispatcher` pre-seeded with built-in commands.
pub fn fake_dispatcher() -> RegistrySlashDispatcher {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)))
}

/// Test orchestrator that returns canned assistant text.
pub struct FakeOrch(pub String);

/// Mirror of `app::ConversationOrchestratorTrait` used by `run_one_submit`.
#[async_trait]
pub trait FakeOrchTrait: Send + Sync {
    async fn run_turn(&self, prompt: &str) -> String;
}

#[async_trait]
impl FakeOrchTrait for FakeOrch {
    async fn run_turn(&self, _prompt: &str) -> String {
        self.0.clone()
    }
}

/// Construct a `FakeOrch` returning `s` on every `run_turn`.
pub fn fake_orchestrator_returning(s: &str) -> FakeOrch {
    FakeOrch(s.to_string())
}
