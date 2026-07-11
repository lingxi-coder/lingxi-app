//! Multi-agent presentation model + feed layer (backend-neutral).
//!
//! Moved from the `tui` crate into `tui-core` during the iocraft → ratatui
//! migration. The pure-data `state`, the `event` type, the `adapter` feed
//! trait, and both feed impls (`fixture`, `poller`) live here. Only the
//! `AppState` mutation seam (`apply`) and the iocraft color styling (`style`)
//! remain in `tui` until their own dependencies are extracted.
pub mod adapter;
pub mod event;
pub mod fixture;
pub mod poller;
pub mod state;
pub mod workflow_spool;

pub use adapter::{pump_once, MultiAgentFeed};
pub use event::MultiAgentEvent;
pub use fixture::FixtureFeed;
pub use poller::{
    sort_workflows_newest_first, task_row_from_record, workflow_row_from_record, PollerFeed,
};
pub use state::{
    MultiAgentState, TaskRow, WorkerRow, WorkflowAgentRow, WorkflowPhase, WorkflowRow,
};
pub use workflow_spool::parse_workflow_spool;
