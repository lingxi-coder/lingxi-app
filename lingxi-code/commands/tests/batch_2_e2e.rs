//! End-to-end: build a registry with `register_all_builtin_commands` +
//! `register_core_batch_1` + `register_core_batch_2`, dispatch each of the 12
//! batch-2 commands through the `RegistrySlashDispatcher`, and verify
//! behaviour.
//!
//! M5-11 Task 15.

use async_trait::async_trait;
use commands::dispatcher::RegistrySlashDispatcher;
use commands::registry::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2, CommandRegistry,
};
use orchestrator::test_support::MockOrchestratorHandle;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::RwLock;
use traits::{
    AgentInfo, AuthError, AuthHandle, CostSnapshot, HookInfo, LoginInfo, McpServerInfo, McpStatus,
    SlashCommandDispatcher, SlashDispatchResult, StatusSnapshot,
};

struct MockAuth {
    result: StdMutex<Result<LoginInfo, AuthError>>,
}

impl MockAuth {
    fn ok(info: LoginInfo) -> Self {
        Self {
            result: StdMutex::new(Ok(info)),
        }
    }
}

#[async_trait]
impl AuthHandle for MockAuth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        self.result.lock().unwrap().clone()
    }
    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }
    async fn current_user(&self) -> Option<LoginInfo> {
        self.result.lock().unwrap().as_ref().ok().cloned()
    }
}

fn fresh() -> (RegistrySlashDispatcher, Arc<MockOrchestratorHandle>) {
    let mut reg = CommandRegistry::new();
    register_all_builtin_commands(&mut reg);
    let handle: Arc<MockOrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
    register_core_batch_1(&mut reg, handle.clone());
    let auth: Arc<dyn AuthHandle> = Arc::new(MockAuth::ok(LoginInfo {
        email: "u@x.com".into(),
        org_id: "org".into(),
    }));
    register_core_batch_2(&mut reg, handle.clone(), auth);
    let d = RegistrySlashDispatcher::new(Arc::new(RwLock::new(reg)));
    (d, handle)
}

#[tokio::test]
async fn cost_dispatch() {
    let (d, mock) = fresh();
    mock.set_cost_snapshot(CostSnapshot {
        total_usd: 0.0042,
        input_tokens: 100,
        output_tokens: 50,
        api_calls: 1,
        session_duration: std::time::Duration::from_secs(10),
        ..CostSnapshot::default()
    });
    let r = d.dispatch("/cost").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(
            display,
            "Cost: $0.0042 (1 calls, 100+50 tokens, 10s session time)"
        );
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn config_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/config").await;
    assert!(matches!(
        r,
        SlashDispatchResult::Handled { ref display } if display.starts_with("Edited ")
    ));
}

#[tokio::test]
async fn model_list_dispatch() {
    let (d, mock) = fresh();
    mock.set_available_models(vec!["claude-opus-4-7".into()]);
    let snap = StatusSnapshot {
        model: "claude-opus-4-7".into(),
        ..StatusSnapshot::default()
    };
    mock.set_status_snapshot(snap);
    let r = d.dispatch("/model").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(
            display,
            "Current model: claude-opus-4-7\nAvailable: claude-opus-4-7"
        );
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn model_switch_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/model claude-sonnet-4-6").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Switched to model: claude-sonnet-4-6");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn permissions_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/permissions").await;
    assert!(matches!(
        r,
        SlashDispatchResult::Handled { ref display } if display.starts_with("Edited ")
    ));
}

#[tokio::test]
async fn mcp_dispatch() {
    let (d, mock) = fresh();
    mock.set_mcp_servers(vec![McpServerInfo {
        name: "m".into(),
        status: McpStatus::Connected,
        transport: "stdio".into(),
    }]);
    let r = d.dispatch("/mcp").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "MCP servers (1):\n  m  connected  stdio\n");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn hooks_dispatch() {
    let (d, mock) = fresh();
    mock.set_hooks(vec![HookInfo {
        name: "fmt".into(),
        event: "PostToolUse".into(),
        matcher: None,
        timeout_ms: 60_000,
    }]);
    let r = d.dispatch("/hooks").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Hooks (1):\n  fmt  PostToolUse  60000ms\n");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn agents_dispatch() {
    let (d, mock) = fresh();
    mock.set_agents(vec![AgentInfo {
        name: "r".into(),
        description: "x".into(),
        tools_allowed: vec![],
    }]);
    let r = d.dispatch("/agents").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Agents (1):\n  r  x\n");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn login_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/login").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Logged in as u@x.com (org: org).");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn logout_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/logout").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert_eq!(display, "Logged out.");
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn version_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/version").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("lingxi-cli "));
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn status_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/status").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("Status:\n"));
        assert_eq!(display.matches('\n').count(), 11);
    } else {
        panic!("{r:?}");
    }
}

#[tokio::test]
async fn doctor_dispatch() {
    let (d, _) = fresh();
    let r = d.dispatch("/doctor").await;
    if let SlashDispatchResult::Handled { display } = r {
        assert!(display.starts_with("Doctor:\n"));
        assert!(display.contains("Summary:"));
    } else {
        panic!("{r:?}");
    }
}
