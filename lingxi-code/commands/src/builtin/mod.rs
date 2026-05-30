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

pub mod agents;
pub mod clear;
pub mod compact;
pub mod config;
pub mod core_placeholders;
pub mod cost;
pub mod doctor;
pub mod exit;
pub mod help;
pub mod help_render;
pub mod hooks;
pub mod init;
pub mod list_render;
pub mod login;
pub mod logout;
pub mod mcp;
pub mod memory;
pub mod model;
pub mod names;
pub mod permissions;
pub mod status;
pub mod templates;
pub mod unimplemented;
pub mod version;

pub use agents::AgentsHandler;
pub use clear::ClearHandler;
pub use compact::CompactHandler;
pub use config::ConfigHandler;
pub use cost::CostHandler;
pub use doctor::DoctorHandler;
pub use exit::ExitHandler;
pub use help::HelpHandler;
pub use hooks::HooksHandler;
pub use init::InitHandler;
pub use login::LoginHandler;
pub use logout::LogoutHandler;
pub use mcp::McpHandler;
pub use memory::MemoryHandler;
pub use model::ModelHandler;
pub use names::{core_description, BUILTIN_COMMAND_NAMES, BUILTIN_CORE_NAMES};
pub use permissions::PermissionsHandler;
pub use status::StatusHandler;
pub use templates::OLD_INIT_PROMPT;
pub use unimplemented::UnimplementedCommandHandler;
pub use version::VersionHandler;
