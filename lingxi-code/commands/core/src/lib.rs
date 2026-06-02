//! `command-core` (M8-P9) — the cross-platform builtin slash-command handlers
//! (the 18 "core" commands) plus the registration entry points the composition
//! roots call. Built on the `command-api` runtime.
//!
//! After [`register_all_builtin_commands`] runs, the registry holds the locked
//! 99-name surface (81 unimplemented stubs + 18 core placeholders);
//! [`register_core_batch_1`] + [`register_core_batch_2`] then overwrite the 18
//! core entries with their real handle/auth-bound handlers. See spec §19.

#![forbid(unsafe_code)]

pub mod agents;
pub mod clear;
pub mod commit;
pub mod commit_push_pr;
pub mod compact;
pub mod config;
pub mod context;
pub mod cost;
pub mod doctor;
pub mod exit;
pub mod export;
pub mod files;
pub mod help;
pub mod hooks;
pub mod init;
pub mod init_verifiers;
pub mod insights;
pub mod login;
pub mod logout;
pub mod mcp;
pub mod memory;
pub mod model;
pub mod permissions;
pub mod pr_comments;
pub mod register;
pub mod resume;
pub mod review;
pub mod security_review;
pub mod status;
pub mod statusline;
pub mod stickers;
pub mod templates;
pub mod version;

mod core_placeholders;

pub use agents::AgentsHandler;
pub use clear::ClearHandler;
pub use commit::CommitHandler;
pub use commit_push_pr::CommitPushPrHandler;
pub use compact::CompactHandler;
pub use config::ConfigHandler;
pub use context::ContextHandler;
pub use cost::CostHandler;
pub use doctor::DoctorHandler;
pub use exit::ExitHandler;
pub use export::ExportHandler;
pub use files::FilesHandler;
pub use help::HelpHandler;
pub use hooks::HooksHandler;
pub use init::InitHandler;
pub use init_verifiers::InitVerifiersHandler;
pub use insights::InsightsHandler;
pub use login::LoginHandler;
pub use logout::LogoutHandler;
pub use mcp::McpHandler;
pub use memory::MemoryHandler;
pub use model::ModelHandler;
pub use permissions::PermissionsHandler;
pub use pr_comments::PrCommentsHandler;
pub use resume::ResumeHandler;
pub use review::ReviewHandler;
pub use security_review::SecurityReviewHandler;
pub use status::StatusHandler;
pub use statusline::StatuslineHandler;
pub use stickers::StickersHandler;
pub use templates::OLD_INIT_PROMPT;
pub use version::VersionHandler;

pub use register::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2,
    register_core_batch_3, register_core_batch_4,
};
