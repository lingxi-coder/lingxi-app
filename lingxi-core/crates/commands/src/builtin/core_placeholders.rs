//! Per-name placeholder handler structs for the 18 core commands.
//!
//! In M5-09 each placeholder returns the same locked stub literal as
//! [`super::unimplemented::UnimplementedCommandHandler`]. M5-10 / M5-11
//! replace each placeholder's body with the real implementation, one
//! struct at a time, without touching registry wiring.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 4.

use crate::registry::CommandRegistry;

/// Register the 18 per-name core placeholders, overwriting the shared
/// unimplemented entries put down by [`crate::register_all_builtin_commands`].
///
/// M5-09 stub: no-op (Task 4 fills this in). The shared
/// `UnimplementedCommandHandler` registrations from Pass 1 stay in place,
/// which is correct behaviour for the M5-09 surface (everything returns the
/// stub literal regardless of whether we overwrite with a per-name placeholder
/// of the same literal).
pub fn register_core_placeholders(_reg: &mut CommandRegistry) {
    // Filled in by Task 4.
}
