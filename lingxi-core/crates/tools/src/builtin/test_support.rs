//! Shared test helpers for builtin tool unit tests.
//!
//! Cleanly avoids re-declaring a 100-line `PanickingFs` impl in every tool's
//! test module. The 6 M4-01 tools all use `tokio::fs` directly; the
//! `Arc<dyn FileSystem>` field in `BuiltinToolContext` is required for future
//! M5 sandbox wiring but never touched by the tools themselves.

use crate::context::{ToolUseContext, ToolUseOptions};
use crate::progress::{progress_channel, ToolProgressSender};
use async_trait::async_trait;
use std::sync::Arc;

/// A stub `FileSystem` that panics on every method.
///
/// All 6 M4-01 builtin tools go through `tokio::fs` directly and never call
/// the FS trait, so it's safe to hand them a panicking stub. M5 sandbox
/// wiring will swap this for a real `FileSystem` impl.
pub struct PanickingFs;

#[async_trait]
impl lingxi_traits::filesystem::FileSystem for PanickingFs {
    async fn read_file(
        &self,
        _: &str,
        _: Option<u64>,
        _: Option<u64>,
    ) -> Result<lingxi_traits::filesystem::FileContent, lingxi_traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::read_file");
    }
    async fn write_file(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("M4-01 builtin tools do not call FileSystem::write_file")
    }
    fn is_within_workspace(&self, _: &str) -> bool {
        true
    }
    async fn watch(
        &self,
        _: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = lingxi_traits::filesystem::FileEvent> + Send>>,
        lingxi_traits::filesystem::FsError,
    > {
        panic!("not called")
    }
    async fn append_file(
        &self,
        _: &str,
        _: &str,
    ) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn truncate(&self, _: &str, _: u64) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_mtime(
        &self,
        _: &str,
    ) -> Result<std::time::SystemTime, lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn file_size(&self, _: &str) -> Result<u64, lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn delete_file(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
    async fn flock_exclusive(
        &self,
        _: &str,
    ) -> Result<Box<dyn lingxi_traits::filesystem::FlockGuard>, lingxi_traits::filesystem::FsError>
    {
        panic!("not called")
    }
    async fn fsync(&self, _: &str) -> Result<(), lingxi_traits::filesystem::FsError> {
        panic!("not called")
    }
}

/// Convenience: return a `PanickingFs` wrapped as `Arc<dyn FileSystem>`.
pub fn make_dummy_fs() -> Arc<dyn lingxi_traits::filesystem::FileSystem> {
    Arc::new(PanickingFs) as _
}

/// Build a fresh, minimal [`ToolUseContext`] for unit tests.
pub fn fresh_ctx() -> ToolUseContext {
    ToolUseContext {
        options: ToolUseOptions {
            debug: false,
            verbose: false,
            main_loop_model: "test".into(),
            max_budget_nano_usd: None,
            mcp_clients: vec![],
            is_non_interactive_session: false,
            custom_system_prompt: None,
            append_system_prompt: None,
        },
        messages: vec![],
        tool_use_id: None,
        agent_id: None,
        content_replacement_state: None,
    }
}

/// Build a fresh progress sender wired to a dropped receiver.
pub fn fresh_tx() -> ToolProgressSender {
    let (tx, _rx) = progress_channel();
    tx
}
