//! `command-mobile` (M8-P9) — mobile-only slash-command handlers.
//!
//! Empty in M8: the mobile commands (`/mobile` `/voice` `/share` `/camera`) are
//! still served as unimplemented stubs that
//! `command_core::register_all_builtin_commands` registers from the locked
//! 99-name table. [`register`] is the slot the `engine-mobile` composition root
//! (P11) calls; it is a no-op today.

#![forbid(unsafe_code)]

use command_api::CommandRegistry;

/// Register the mobile-only command handlers into `reg`.
///
/// No-op in M8 (the mobile command names remain unimplemented stubs from
/// `command-core`). Present so the mobile composition root can call it.
pub fn register(_reg: &mut CommandRegistry) {}
