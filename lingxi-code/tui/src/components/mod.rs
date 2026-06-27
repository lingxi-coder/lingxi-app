//! Reusable iocraft components for the M6 TUI.
//!
//! Each submodule exports one component (or a small family of related
//! components) plus its `Props` struct.

pub mod coordinator;
pub mod message_selector;
pub mod messages;
pub mod permissions;
pub mod picker_popup;
pub mod prompt_input;
pub mod scrollback;
pub mod spinner;
pub mod status_line;
pub mod status_line_command;
pub mod tasks;
pub mod virtual_message_list;
