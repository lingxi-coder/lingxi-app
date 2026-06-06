//! Shared builtin command scaffolding used by every impl crate
//! (`command-core` / `command-desktop` / `command-mobile`):
//!
//! - [`names`] — the locked 99-name table (`BUILTIN_COMMAND_NAMES`),
//!   the 18-name core list (`BUILTIN_CORE_NAMES`), `core_description`, the
//!   bucket-(d) `INTENTIONALLY_DISABLED_COMMANDS` audit table, and its STUB.6
//!   refinement into `CORRECT_BY_DESIGN_STUBS` (faithful) vs
//!   `HOST_BOUND_DEFERRED_GAPS` (genuine deferred gaps).
//! - [`help_render`] — the byte-locked `/help` screen renderer.
//! - [`list_render`] — the generic `/agents` `/hooks` `/mcp` list renderer.
//! - [`unimplemented`] — the per-name stub handler returning the locked
//!   "not implemented" literal.

pub mod help_render;
pub mod list_render;
pub mod names;
pub mod unimplemented;

pub use names::{
    core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES, CORRECT_BY_DESIGN_STUBS,
    HOST_BOUND_DEFERRED_GAPS, INTENTIONALLY_DISABLED_COMMANDS,
};
pub use unimplemented::UnimplementedCommandHandler;
