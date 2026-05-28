//! Built-in slash-command handlers.
//!
//! Surface (M5-09): 99 names registered via [`crate::registry::register_all_builtin_commands`].
//! 81 point at a shared [`unimplemented::UnimplementedCommandHandler`]; 18 core
//! get per-name placeholder structs (see [`core_placeholders`]) so M5-10 / M5-11
//! can swap each one's body independently.
//!
//! M5-10 lit up batch 1: the 6 real handlers (`/clear`, `/compact`, `/exit`,
//! `/help`, `/init`, `/memory`) live in dedicated modules and replace the
//! M5-09 stub bodies via [`crate::registry::register_core_batch_1`].

pub mod clear;
pub mod compact;
pub mod core_placeholders;
pub mod exit;
pub mod help;
pub mod help_render;
pub mod init;
pub mod memory;
pub mod names;
pub mod templates;
pub mod unimplemented;

pub use clear::ClearHandler;
pub use compact::CompactHandler;
// M5-10 batch-1: the placeholders are still exported for back-compat tests,
// but the real handler types take precedence in `pub use` order.
pub use core_placeholders::{
    AgentsHandler, ConfigHandler, CostHandler, DoctorHandler, HooksHandler, LoginHandler,
    LogoutHandler, McpHandler, ModelHandler, PermissionsHandler, StatusHandler, VersionHandler,
};
pub use exit::ExitHandler;
pub use help::HelpHandler;
pub use init::InitHandler;
pub use memory::MemoryHandler;
pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use templates::OLD_INIT_PROMPT;
pub use unimplemented::UnimplementedCommandHandler;
