//! `TeamRegistryHandle` — narrow trait giving the bridge / TUI PULL path a way
//! to read coordinator workers WITHOUT depending on the concrete
//! `coordinator` crate.
//!
//! Mirrors the [`crate::task_registry::TaskRegistryHandle`] decoupling pattern:
//! the abstract read seam lives here in `traits`; the concrete impl lives in
//! `coordinator` (lowering `WorkerAgent` → [`WorkerInfo`]). Tests inject an
//! in-memory mock.
//!
//! [`WorkerInfo`] is a plain-old-data projection of the coordinator's
//! `WorkerAgent`, field-shaped to lower 1:1 onto the TUI `WorkerRow` and the
//! `client-protocol` roster DTO (added in T18). `status` is a simplified
//! wire/label string (the same convention as [`crate::task_registry::TaskRecord`]
//! `status`) so this crate need not import the concrete `WorkerStatus` enum.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One coordinator worker as surfaced to the bridge / TUI read path.
///
/// Field-shaped to lower 1:1 onto the TUI `WorkerRow`
/// (`agent_id` / `name` / `agent_type` / `status`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerInfo {
    /// Worker agent id, stringified (the `AgentId` display / wire form).
    pub agent_id: String,
    /// Agent-type string (e.g. `"explorer"`, `"writer"`).
    pub agent_type: String,
    /// Display name.
    pub name: String,
    /// Simplified status label (e.g. `"idle"`, `"working"`, `"failed"`).
    pub status: String,
}

/// Read-only roster surface used by the bridge poll + TUI live feed.
///
/// Object-safe so consumers can hold an `Arc<dyn TeamRegistryHandle>` and
/// tests can inject a mock returning a fixed roster.
#[async_trait]
pub trait TeamRegistryHandle: Send + Sync {
    /// List all currently registered workers, lowered to [`WorkerInfo`].
    async fn list_workers(&self) -> Vec<WorkerInfo>;

    /// The name of the single team this coordinator is running, if set.
    async fn team_name(&self) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn TeamRegistryHandle>> = None;
    }

    #[test]
    fn worker_info_roundtrip_json() {
        let info = WorkerInfo {
            agent_id: "agent:00000000-0000-0000-0000-000000000001".into(),
            agent_type: "explorer".into(),
            name: "alpha".into(),
            status: "working".into(),
        };
        let s = serde_json::to_string(&info).unwrap();
        let back: WorkerInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(info, back);
    }
}
