//! Mock fixtures for M4-05 wiring tests.
//!
//! Recording mocks for the 4 handle traits the M4-05 tools dispatch
//! through. The Arc::ptr_eq recursion-lock + budget-inheritance tests
//! work by capturing what the production `AgentTool::call` hands to the
//! spawner and asserting the trait-object Arcs match the originals.

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use traits::budget::{BudgetEnforcerHandle, BudgetError};
use traits::mailbox::{MailboxError, MailboxMessage, MailboxRouterHandle, RouteAck};
use traits::subagent_spawn::{
    SubagentInheritance, SubagentListingEntry, SubagentResult, SubagentSpawnError,
    SubagentSpawnRequest, SubagentSpawner, SubagentUsage,
};
use traits::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};

// =========================================================================
// MockSubagentSpawner — records every spawn + exposes captured inheritance.
// =========================================================================

/// One captured invocation of `MockSubagentSpawner::spawn`.
#[derive(Clone)]
pub struct MockSpawnInvocation {
    /// The request passed to spawn.
    pub request: SubagentSpawnRequest,
    /// The inheritance bundle passed to spawn. Captured verbatim — the
    /// `Arc` fields support `Arc::ptr_eq` round-trip against whatever the
    /// parent originally constructed.
    pub inherit: SubagentInheritance,
}

/// Recording mock for `SubagentSpawner`.
pub struct MockSubagentSpawner {
    invocations: Mutex<Vec<MockSpawnInvocation>>,
    response: Mutex<MockSpawnResponse>,
}

#[derive(Clone)]
enum MockSpawnResponse {
    Completed,
    Failed(String),
    Killed,
}

impl MockSubagentSpawner {
    /// Build a mock that returns `SubagentResult::Completed`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            invocations: Mutex::new(Vec::new()),
            response: Mutex::new(MockSpawnResponse::Completed),
        }
    }

    /// Force the next (and subsequent) spawns to fail.
    pub fn script_failed(&self, reason: impl Into<String>) {
        *self.response.lock().unwrap() = MockSpawnResponse::Failed(reason.into());
    }

    /// Force spawns to return Killed.
    pub fn script_killed(&self) {
        *self.response.lock().unwrap() = MockSpawnResponse::Killed;
    }

    /// Drain and return every captured invocation.
    #[must_use]
    pub fn invocations(&self) -> Vec<MockSpawnInvocation> {
        self.invocations.lock().unwrap().clone()
    }
}

impl Default for MockSubagentSpawner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SubagentSpawner for MockSubagentSpawner {
    async fn spawn(
        &self,
        request: SubagentSpawnRequest,
        inherit: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        self.invocations.lock().unwrap().push(MockSpawnInvocation {
            request: request.clone(),
            inherit: inherit.clone(),
        });
        let resp = self.response.lock().unwrap().clone();
        Ok(match resp {
            MockSpawnResponse::Completed => SubagentResult::Completed {
                content: json!({ "mock": true }),
                usage: SubagentUsage::default(),
            },
            MockSpawnResponse::Failed(reason) => SubagentResult::Failed { reason },
            MockSpawnResponse::Killed => SubagentResult::Killed,
        })
    }

    /// Fixed catalog so the dynamic-prompt test can assert `formatAgentLine`
    /// output. Mirrors the shape the production spawner surfaces (built-ins).
    async fn agent_listing(&self) -> Vec<SubagentListingEntry> {
        vec![
            SubagentListingEntry {
                agent_type: "general-purpose".into(),
                when_to_use: "use for anything".into(),
                tools_description: "All tools".into(),
            },
            SubagentListingEntry {
                agent_type: "Explore".into(),
                when_to_use: "search".into(),
                tools_description: "All tools except Edit".into(),
            },
        ]
    }
}

// =========================================================================
// MockTaskRegistryHandle — in-memory CRUD.
// =========================================================================

/// In-memory recording mock for `TaskRegistryHandle`.
pub struct MockTaskRegistryHandle {
    records: Mutex<HashMap<String, TaskRecord>>,
    counter: AtomicU64,
}

impl MockTaskRegistryHandle {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }

    fn fresh_id(&self, task_type: &str) -> String {
        let prefix = match task_type {
            "local_agent" => 'a',
            "remote_agent" => 'r',
            "in_process_teammate" => 't',
            "local_workflow" => 'w',
            "monitor_mcp" => 'm',
            "dream" => 'd',
            // local_bash + any unknown wire string default to 'b'.
            _ => 'b',
        };
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        // 8-char base36 suffix, padded zero — deterministic for tests.
        format!("{prefix}{n:08x}")
            .chars()
            .take(9)
            .collect::<String>()
    }
}

impl Default for MockTaskRegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskRegistryHandle for MockTaskRegistryHandle {
    async fn create(&self, input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        let rec = TaskRecord {
            task_id: self.fresh_id(&input.task_type),
            task_type: input.task_type,
            status: "pending".into(),
            description: input.description,
            command: None,
        };
        self.records
            .lock()
            .unwrap()
            .insert(rec.task_id.clone(), rec.clone());
        Ok(rec)
    }

    async fn get(&self, id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(self.records.lock().unwrap().get(id).cloned())
    }

    async fn list(&self, filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        let map = self.records.lock().unwrap();
        Ok(map
            .values()
            .filter(|r| filter.status.as_deref().is_none_or(|s| r.status == s))
            .cloned()
            .collect())
    }

    async fn update(
        &self,
        id: &str,
        patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        let mut map = self.records.lock().unwrap();
        let rec = map
            .get_mut(id)
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        if let Some(s) = patch.status {
            rec.status = s;
        }
        Ok(rec.clone())
    }

    async fn set_status(&self, id: &str, status: &str) -> Result<TaskRecord, TaskRegistryError> {
        let mut map = self.records.lock().unwrap();
        let rec = map
            .get_mut(id)
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        rec.status = status.into();
        Ok(rec.clone())
    }

    async fn kill(&self, id: &str) -> Result<TaskRecord, TaskRegistryError> {
        let mut map = self.records.lock().unwrap();
        let rec = map
            .get_mut(id)
            .ok_or_else(|| TaskRegistryError::NotFound(id.into()))?;
        rec.status = "killed".into();
        Ok(rec.clone())
    }

    async fn output(
        &self,
        id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        let map = self.records.lock().unwrap();
        if !map.contains_key(id) {
            return Err(TaskRegistryError::NotFound(id.into()));
        }
        Ok(TaskOutputChunk {
            task_id: id.into(),
            content: String::new(),
            total_lines: 0,
            truncated: false,
            ..Default::default()
        })
    }
}

// =========================================================================
// MockMailboxRouterHandle — records routed messages.
// =========================================================================

/// One captured `route` call.
#[derive(Clone, Debug)]
pub struct RoutedMessage {
    /// Sender id (string form passed to `route`).
    pub from_agent: String,
    /// Recipient id.
    pub to_agent: String,
    /// Routed message body.
    pub message: MailboxMessage,
}

/// Recording mock for `MailboxRouterHandle`.
pub struct MockMailboxRouterHandle {
    sent: Mutex<Vec<RoutedMessage>>,
    /// Locked claim-window seconds — defaults to 30 (spec §7 line 498).
    pub claim_window_secs: u64,
}

impl MockMailboxRouterHandle {
    /// Construct a mock that acks with the 30s claim window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            claim_window_secs: 30,
        }
    }

    /// Drain and return every captured route.
    #[must_use]
    pub fn sent(&self) -> Vec<RoutedMessage> {
        self.sent.lock().unwrap().clone()
    }
}

impl Default for MockMailboxRouterHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MailboxRouterHandle for MockMailboxRouterHandle {
    async fn route(
        &self,
        from_agent: &str,
        to_agent: &str,
        message: MailboxMessage,
    ) -> Result<RouteAck, MailboxError> {
        self.sent.lock().unwrap().push(RoutedMessage {
            from_agent: from_agent.into(),
            to_agent: to_agent.into(),
            message,
        });
        Ok(RouteAck {
            claimed_at: SystemTime::now(),
            claim_window_secs: self.claim_window_secs,
        })
    }
}

// =========================================================================
// MockBudgetEnforcerHandle — atomic-counter gate.
// =========================================================================

/// Atomic-counter mock for `BudgetEnforcerHandle`. Returns `Exceeded`
/// when the running total surpasses the configured cap.
pub struct MockBudgetEnforcerHandle {
    /// Cumulative nano-USD charged.
    pub total_nano_usd: AtomicU64,
    /// Cap above which `check_and_charge` returns Exceeded. `u64::MAX`
    /// effectively disables the gate.
    pub cap_nano_usd: u64,
}

impl MockBudgetEnforcerHandle {
    /// Build a mock with the supplied cap. Initial total is 0.
    #[must_use]
    pub fn new(cap_nano_usd: u64) -> Self {
        Self {
            total_nano_usd: AtomicU64::new(0),
            cap_nano_usd,
        }
    }

    /// Convenience: enforcer that will always allow charges through.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::new(u64::MAX)
    }

    /// Force the running total. Used by tests that want the gate to trip.
    pub fn set_total(&self, n: u64) {
        self.total_nano_usd.store(n, Ordering::SeqCst);
    }
}

#[async_trait]
impl BudgetEnforcerHandle for MockBudgetEnforcerHandle {
    async fn check_and_charge(&self, nano_usd: u64) -> Result<(), BudgetError> {
        let prev = self.total_nano_usd.fetch_add(nano_usd, Ordering::SeqCst);
        let post = prev.saturating_add(nano_usd);
        if post > self.cap_nano_usd {
            Err(BudgetError::Exceeded {
                current_nano_usd: post,
            })
        } else {
            Ok(())
        }
    }

    async fn snapshot_total_nano_usd(&self) -> u64 {
        self.total_nano_usd.load(Ordering::SeqCst)
    }
}

// =========================================================================
// Convenience: wrap mocks in Arcs.
// =========================================================================

/// Convenience: wrap [`MockSubagentSpawner`] in `Arc<dyn SubagentSpawner>`.
#[must_use]
pub fn arc_mock_spawner() -> Arc<MockSubagentSpawner> {
    Arc::new(MockSubagentSpawner::new())
}

/// Convenience: wrap [`MockTaskRegistryHandle`] in `Arc<dyn TaskRegistryHandle>`.
#[must_use]
pub fn arc_mock_task_registry() -> Arc<MockTaskRegistryHandle> {
    Arc::new(MockTaskRegistryHandle::new())
}

/// Convenience: wrap [`MockMailboxRouterHandle`] in `Arc<dyn MailboxRouterHandle>`.
#[must_use]
pub fn arc_mock_mailbox() -> Arc<MockMailboxRouterHandle> {
    Arc::new(MockMailboxRouterHandle::new())
}

/// Convenience: wrap [`MockBudgetEnforcerHandle`] in
/// `Arc<dyn BudgetEnforcerHandle>`.
#[must_use]
pub fn arc_mock_budget(cap: u64) -> Arc<MockBudgetEnforcerHandle> {
    Arc::new(MockBudgetEnforcerHandle::new(cap))
}
