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
            assert_eq!(display, "Compacted (ctrl+o to see full summary)");
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
            // (M4 cc2.1.198) /agents carries the removed-wizard description.
            assert!(display.contains(
                "(removed) Ask Claude to create/manage subagents, or edit .lingxi/agents/"
            ));
            // 79 newlines (header + 78 visible lines) after the 105-name
            // re-lock dropped `x402`; the hidden/disabled commands are
            // filtered out to match claude-code's /help.
            assert_eq!(display.matches('\n').count(), 79);
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
    mock.set_memory_path(std::path::PathBuf::from("/tmp/LINGXI.md"));
    mock.set_editor_exit_code(0);
    let r = d.dispatch("/memory").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "Edited /tmp/LINGXI.md (exit 0).");
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
    // `x402` left the 105-name surface, so it is no longer a registered stub;
    // `ant-trace` is the sample `register.rs` itself uses for one.
    let r = d.dispatch("/ant-trace").await;
    match r {
        SlashDispatchResult::Handled { display } => {
            assert_eq!(display, "ant-trace: not implemented in v0.6.0 (M5)");
        }
        other => panic!("{other:?}"),
    }
}
