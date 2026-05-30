//! Slash-command subsystem.
//!
//! # Surface (M5-09)
//!
//! After [`register_all_builtin_commands`] runs, the [`CommandRegistry`] holds
//! exactly **99** entries. 81 of them point at a per-name
//! [`builtin::UnimplementedCommandHandler`] and return the locked literal
//! `"{name}: not implemented in v0.6.0 (M5)"`. The remaining 18 (the "core"
//! list defined by [`builtin::BUILTIN_CORE_NAMES`]) point at per-name
//! placeholder structs in [`builtin::core_placeholders`] that **also** return
//! the same locked literal in M5-09 but exist as stable type-ids so M5-10 and
//! M5-11 can swap their bodies independently without touching registry wiring.
//!
//! # Dispatch
//!
//! [`dispatcher::RegistrySlashDispatcher`] implements
//! [`traits::SlashCommandDispatcher`]. It strips a leading `/`,
//! parses the remainder via [`parser::parse_slash_command`], looks up the
//! handler in the [`CommandRegistry`], and returns one of:
//!
//! - [`traits::SlashDispatchResult::Handled`] for known commands
//! - [`traits::SlashDispatchResult::Unknown`] with the locked literal
//!   `"Unknown command: /{name}"` for unregistered names
//! - [`traits::SlashDispatchResult::NotASlashCommand`] for inputs that
//!   don't start with `/`
//!
//! # Count history
//!
//! Original plan locked 102 total commands. The 2026-05-28 addendum
//! re-locked at **99 = 18 core + 81 unimplemented** after auditing the plan's
//! enumerated list against `claude-code/src/commands/`.
//!
//! # Batch 1 (M5-10)
//!
//! After [`register_core_batch_1`] runs against a fully-initialised registry,
//! the 6 batch-1 commands are wired to real implementations:
//!
//! - `/clear` — calls [`traits::OrchestratorHandle::clear_session`]
//! - `/compact` — calls [`traits::OrchestratorHandle::force_compact`]
//! - `/help` — renders the locked 100-line table via
//!   [`builtin::help_render::render_help_screen`]
//! - `/exit` — calls [`traits::OrchestratorHandle::request_exit`]
//! - `/memory` — calls
//!   [`traits::OrchestratorHandle::open_memory_editor`] which spawns
//!   `$EDITOR` (with `VISUAL` fallback, then `vi`/`notepad.exe`)
//! - `/init` — emits [`crate::CommandResult::InjectMessage`] carrying the
//!   byte-locked [`builtin::OLD_INIT_PROMPT`] template (1592 bytes, 21
//!   lines, sha256 `cfdedaa2…b55a39`)
//!
//! Each command emits 3 telemetry events
//! (`tengu_command_<name>_{started,completed,failed}`) defined in
//! [`telemetry::tengu::command`]. The total tengu registry grew
//! from 258 → 276 events.
//!
//! # Batch 2 (M5-11)
//!
//! After [`register_core_batch_2`] runs, the remaining 12 core commands are
//! wired:
//!
//! - `/cost`        — `OrchestratorHandle::snapshot_cost`
//! - `/config`      — `edit_config_file` ($EDITOR on config.json)
//! - `/model`       — list (no arg) / switch (arg) via `list_available_models` + `switch_model`
//! - `/permissions` — `edit_permissions_file`
//! - `/mcp`         — `list_mcp_servers`
//! - `/hooks`       — `list_hooks`
//! - `/agents`      — `list_agents`
//! - `/login`       — `AuthHandle::login` (interactive OAuth)
//! - `/logout`      — `AuthHandle::logout`
//! - `/version`     — `CARGO_PKG_VERSION` + git SHA
//! - `/status`      — `get_status_snapshot` → 11-line panel
//! - `/doctor`      — `run_doctor_checks` → 6-check report
//!
//! Each command emits 3 telemetry events
//! (`tengu_command_<name>_{started,completed,failed}`) defined in
//! [`telemetry::tengu::command`]. The total tengu registry grew
//! from 276 → **312** events (+36 for the 12 batch-2 commands × 3 phases).
//!
//! # Plan reference
//!
//! See `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`,
//! `docs/superpowers/plans/2026-05-25-m5-10-commands-batch-1.md`, and
//! `docs/superpowers/plans/2026-05-25-m5-11-commands-batch-2.md`.
//!
//! See spec §19 for the broader slash-command design.

#![forbid(unsafe_code)]

pub mod argument_substitution;
pub mod builtin;
pub mod dispatcher;
pub mod model;
pub mod parser;
pub mod registry;

pub use argument_substitution::substitute_arguments;
pub use dispatcher::RegistrySlashDispatcher;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::{
    register_all_builtin_commands, register_core_batch_1, register_core_batch_2, CommandRegistry,
};
