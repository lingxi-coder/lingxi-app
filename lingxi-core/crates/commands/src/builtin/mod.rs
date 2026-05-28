//! Built-in slash-command handlers.
//!
//! Surface (M5-09): 102 names registered via [`crate::registry::register_all_builtin_commands`].
//! 84 point at a shared [`unimplemented::UnimplementedCommandHandler`]; 18 core
//! get per-name placeholder structs (see [`core_placeholders`]) so M5-10 / M5-11
//! can swap each one's body independently.

pub mod compact;
pub mod cost;
pub mod help;
pub mod memory;
pub mod resume;
pub mod unimplemented;

pub use unimplemented::UnimplementedCommandHandler;
