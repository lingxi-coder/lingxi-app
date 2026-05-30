//! End-to-end: build a registry with `register_all_builtin_commands` +
//! `register_core_batch_1`, dispatch each of the 6 commands through the
//! `RegistrySlashDispatcher`, and verify behaviour.
//!
//! M5-10 Task 12.

use command_api::CommandRegistry;
use command_api::RegistrySlashDispatcher;
use command_core::{register_all_builtin_commands, register_core_batch_1};
use orchestrator::test_support::MockOrchestratorHandle;
use std::sync::Arc;
use tokio::sync::RwLock;
use traits::{CompactionSummary, SlashCommandDispatcher, SlashDispatchResult};

fn fresh() -> (RegistrySlashDispatcher, Arc<MockOrchestratorHandle>) {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let handle: Arc<MockOrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
    register_core_batch_1(&mut reg, handle.clone());
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
    (d, handle)
}

#[tokio::test]
async fn clear_dispatch() {
    let (d, mock) = fresh();
    let r = d.dispatch("/clear").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Conversation cleared.");
        }
        other => panic!("{other:?}"),
    }
    assert!(mock.was_clear_session_called());
}

#[tokio::test]
async fn compact_dispatch() {
    let (d, mock) = fresh();
    mock.set_compact_summary(CompactionSummary {
        messages_before: 100,
        messages_after: 5,
        bytes_saved: 4_096,
    });
    let r = d.dispatch("/compact").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Compacted: 100 → 5 messages (4096 bytes saved).");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn help_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/help").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert!(display.starts_with("Commands:\n"));
            assert!(display.contains("Manage subagents"));
            // 100 newlines (header + 99 lines).
            assert_eq!(display.matches('\n').count(), 100);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn exit_dispatch() {
    let (d, mock) = fresh();
    let r = d.dispatch("/exit").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Exiting.");
        }
        other => panic!("{other:?}"),
    }
    assert!(mock.was_exit_requested());
}

#[tokio::test]
async fn memory_dispatch() {
    let (d, mock) = fresh();
    mock.set_memory_path(std::path::PathBuf::from("/tmp/CLAUDE.md"));
    mock.set_editor_exit_code(0);
    let r = d.dispatch("/memory").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Edited /tmp/CLAUDE.md (exit 0).");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn init_dispatch_returns_handled_with_template_text() {
    // /init returns InjectMessage; the dispatcher maps InjectMessage to
    // Handled { display: content } per M5-09 Task 5 step 4.
    let (d, _) = fresh();
    let r = d.dispatch("/init").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert!(display.starts_with("Please analyze this codebase"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn non_batch_1_command_still_returns_stub() {
    let (d, _) = fresh();
    let r = d.dispatch("/x402").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "x402: not implemented in v0.6.0 (M5)");
        }
        other => panic!("{other:?}"),
    }
}
