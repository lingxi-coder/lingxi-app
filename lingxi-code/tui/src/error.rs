//! Moved to `tui-core` (`tui_core::error`) during the iocraft → ratatui
//! migration. Re-exported so `crate::error::TuiError` and the `lib.rs`
//! `pub use error::TuiError` keep resolving until cutover.
pub use tui_core::error::*;
