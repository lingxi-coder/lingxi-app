//! `command-desktop` (M8-P9) — desktop-only slash-command handlers.
//!
//! Empty in M8: the desktop commands (`/commit` `/diff` `/review` `/chrome`
//! `/ide` `/terminal-setup` …) are still served as unimplemented stubs that
//! `command_core::register_all_builtin_commands` registers from the locked
//! 99-name table. [`register`] is the slot future milestones populate with the
//! real desktop handlers; it is a no-op today, called by `engine-desktop`.

#![forbid(unsafe_code)]

use command_api::CommandRegistry;

/// Register the desktop-only command handlers into `reg`.
///
/// No-op in M8 (the desktop command names remain unimplemented stubs from
/// `command-core`). Present so the desktop composition root can call it now and
/// future milestones only need to fill the body.
pub fn register(_reg: &mut CommandRegistry) {}
