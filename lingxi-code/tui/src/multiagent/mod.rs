//! Multi-agent TUI surface — presentation model, mutation seam, and the
//! swappable feed adapter. (M9)

pub mod event;
pub mod state;

pub use event::MultiAgentEvent;
pub use state::{MultiAgentState, TaskRow, WorkerRow};
