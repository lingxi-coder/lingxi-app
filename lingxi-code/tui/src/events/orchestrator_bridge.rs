//! Moved to `tui-core` (`tui_core::orchestrator_bridge`) during the iocraft →
//! ratatui migration. Re-exported so every `crate::events::orchestrator_bridge::*`
//! path (and the `lib.rs` `pub use` of `TurnEvent`/`BridgeOutputStream`) keeps
//! resolving until cutover.
pub use tui_core::orchestrator_bridge::*;
