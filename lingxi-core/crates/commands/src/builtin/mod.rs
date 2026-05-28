//! Built-in slash-command handlers.
//!
//! Surface (M5-09): 99 names registered via [`crate::registry::register_all_builtin_commands`].
//! 81 point at a shared [`unimplemented::UnimplementedCommandHandler`]; 18 core
//! get per-name placeholder structs (see [`core_placeholders`]) so M5-10 / M5-11
//! can swap each one's body independently.

pub mod core_placeholders;
pub mod names;
pub mod unimplemented;

pub use core_placeholders::{
    AgentsHandler, ClearHandler, CompactHandler, ConfigHandler, CostHandler, DoctorHandler,
    ExitHandler, HelpHandler, HooksHandler, InitHandler, LoginHandler, LogoutHandler, McpHandler,
    MemoryHandler, ModelHandler, PermissionsHandler, StatusHandler, VersionHandler,
};
pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use unimplemented::UnimplementedCommandHandler;
