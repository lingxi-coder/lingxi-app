//! Moved to `tui-core` (`tui_core::multiagent::poller`) during the iocraft →
//! ratatui migration. Re-exported so `crate::multiagent::poller::*` and the
//! `multiagent::mod` re-export keep resolving until cutover.
pub use tui_core::multiagent::poller::*;
