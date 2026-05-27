//! Per-tool default Y/N decisions for the interactive permission prompt.
//!
//! Scaffolded in M5-05 Task 1 (stub `tool_default` returning [`PromptDefault::DenyByDefault`]
//! for every name). Full 41-tool lookup populated in M5-05 Task 4.
#![forbid(unsafe_code)]

use crate::gate::PromptDefault;

/// Look up the default Y/N decision for a tool name.
///
/// **Stub (Task 1):** always returns [`PromptDefault::DenyByDefault`].
/// Task 4 replaces the body with the byte-locked 41-entry table.
pub fn tool_default(_name: &str) -> PromptDefault {
    PromptDefault::DenyByDefault
}
