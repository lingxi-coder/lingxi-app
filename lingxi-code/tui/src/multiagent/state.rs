//! Moved to `tui-core` (`tui_core::multiagent::state`) during the iocraft →
//! ratatui migration. Re-exported here so every `crate::multiagent::state::*`
//! path — and the `multiagent::mod` re-export of `MultiAgentState`/`TaskRow`/
//! `WorkerRow` — keeps resolving until cutover.
pub use tui_core::multiagent::state::*;
