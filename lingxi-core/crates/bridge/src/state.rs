//! Lightweight bridge session state snapshot.
//!
//! Held by the engine so UI / telemetry can observe whether an IDE peer is
//! currently connected and which file (if any) it has focused.

use serde::{Deserialize, Serialize};

/// In-memory view of the bridge for the current session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeState {
    /// True when at least one trusted IDE is connected.
    pub connected: bool,
    /// Path of the file currently focused in the IDE, if known.
    pub current_file: Option<std::path::PathBuf>,
}
