//! Moved to `tui-core` (`tui_core::bash_runner`) during the iocraft → ratatui
//! migration. Re-exported so every `crate::bash_runner::*` path keeps
//! resolving until cutover.
pub use tui_core::bash_runner::*;
