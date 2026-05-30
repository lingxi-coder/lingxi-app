//! Multi-agent TUI surface — presentation model, mutation seam, and the
//! swappable feed adapter. (M9)

pub mod adapter;
pub mod apply;
pub mod event;
pub mod fixture;
pub mod poller;
pub mod state;

pub use adapter::{pump_once, MultiAgentFeed};
pub use apply::apply_multiagent_event;
pub use event::MultiAgentEvent;
pub use fixture::FixtureFeed;
pub use poller::{task_row_from_record, PollerFeed};
pub use state::{MultiAgentState, TaskRow, WorkerRow};
