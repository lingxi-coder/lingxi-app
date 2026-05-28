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
//! [`lingxi_traits::SlashCommandDispatcher`]. It strips a leading `/`,
//! parses the remainder via [`parser::parse_slash_command`], looks up the
//! handler in the [`CommandRegistry`], and returns one of:
//!
//! - [`lingxi_traits::SlashDispatchResult::Handled`] for known commands
//! - [`lingxi_traits::SlashDispatchResult::Unknown`] with the locked literal
//!   `"Unknown command: /{name}"` for unregistered names
//! - [`lingxi_traits::SlashDispatchResult::NotASlashCommand`] for inputs that
//!   don't start with `/`
//!
//! # Count history
//!
//! Original plan locked 102 total commands. The 2026-05-28 addendum
//! re-locked at **99 = 18 core + 81 unimplemented** after auditing the plan's
//! enumerated list against `claude-code/src/commands/`.
//!
//! # Plan reference
//!
//! See `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`.
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
pub use registry::{register_all_builtin_commands, register_core_batch_1, CommandRegistry};
