//! Background async hook registry — tracks long-running hooks spawned via
//! [`crate::RuntimeSpawner`] so their completions can be collected on a
//! dedicated channel.
//!
//! The full implementation (timeout enforcement, cancellation, telemetry) is
//! delivered in Plan 09 alongside the Skills / Commands / Output Styles
//! plumbing. M1.4 ships only the type so other crates can reference it.

use crate::definition::HookDefinition;
use crate::events::HookEvent;
use crate::registry::HookContext;
use crate::response::HookResult;
use protocol::HookId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use traits::{BackgroundTaskHandle, RuntimeError, RuntimeSpawner};

/// Registry of currently-running non-blocking hooks.
///
/// Each `spawn` call hands the hook off to the runtime spawner and stores
/// the resulting handle so the engine can cancel or join it later. Completed
/// hooks publish a `(HookId, HookResult)` tuple on `completion_tx` so the
/// engine can fold the result back into the originating session.
pub struct AsyncHookRegistry {
    #[allow(dead_code)] // Wired up in Plan 09 with the full async dispatch logic.
    runtime: Arc<dyn RuntimeSpawner>,
    #[allow(dead_code)] // Wired up in Plan 09.
    in_flight: Arc<Mutex<HashMap<HookId, BackgroundTaskHandle>>>,
    #[allow(dead_code)] // Wired up in Plan 09.
    completion_tx: mpsc::Sender<(HookId, HookResult)>,
}

impl AsyncHookRegistry {
    /// Build a new registry backed by `runtime` and the supplied completion
    /// channel sender.
    #[must_use]
    pub fn new(
        runtime: Arc<dyn RuntimeSpawner>,
        completion_tx: mpsc::Sender<(HookId, HookResult)>,
    ) -> Self {
        Self {
            runtime,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            completion_tx,
        }
    }

    /// Spawn a non-blocking hook into the background. Stubbed for M1.4 —
    /// returns `Ok(())` without spawning anything. The full body lands in
    /// Plan 09.
    #[allow(clippy::unused_async)] // Full impl awaits the spawned task; stub does not.
    pub async fn spawn(
        &self,
        _hook: HookDefinition,
        _event: HookEvent,
        _ctx: HookContext,
    ) -> Result<(), RuntimeError> {
        // Full impl in Plan 09 (Skills / Cmd / Styles tie-in). M1.4 ships the
        // type so the dispatcher can reference it.
        Ok(())
    }
}
