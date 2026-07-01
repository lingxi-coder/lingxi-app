//! Moved to `tui-core` (`tui_core::multiagent::adapter`) during the iocraft →
//! ratatui migration. Re-exported so `crate::multiagent::adapter::*` and the
//! `multiagent::mod` re-export keep resolving until cutover.
pub use tui_core::multiagent::adapter::*;
