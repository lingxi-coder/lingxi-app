//! Queue operation log entries — replayed during crash recovery so the
//! engine can rebuild the queue state after an unclean shutdown.

use crate::queue::{QueuePriority, QueueSource};
use serde::{Deserialize, Serialize};

/// One entry in the queue's append-only operation log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueOperation {
    /// Command added to the queue.
    Enqueue {
        /// Command UUID.
        uuid: String,
        /// Priority used at enqueue time.
        priority: QueuePriority,
        /// Originating subsystem.
        source: QueueSource,
    },
    /// Command popped from the queue.
    Dequeue {
        /// UUID of the dequeued command.
        uuid: String,
    },
    /// Command explicitly removed (e.g. user cancellation).
    Remove {
        /// UUID of the removed command.
        uuid: String,
        /// Reason for removal.
        reason: String,
    },
    /// Queue cleared.
    Clear {
        /// Number of items dropped.
        count: usize,
    },
}
