//! Shared builtin command scaffolding used by every impl crate
//! (`command-core` / `command-desktop` / `command-mobile`):
//!
//! - [`names`] — the locked 99-name table (`BUILTIN_COMMAND_NAMES`),
//!   the 18-name core list (`BUILTIN_CORE_NAMES`), and `core_description`.
//! - [`help_render`] — the byte-locked `/help` screen renderer.
//! - [`list_render`] — the generic `/agents` `/hooks` `/mcp` list renderer.
//! - [`unimplemented`] — the per-name stub handler returning the locked
//!   "not implemented" literal.

pub mod help_render;
pub mod list_render;
pub mod names;
pub mod unimplemented;

pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use unimplemented::UnimplementedCommandHandler;
