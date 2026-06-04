//! Slash-command runtime abstraction (M8-P9).
//!
//! The platform-agnostic command machinery: the [`CommandRegistry`],
//! [`RegistrySlashDispatcher`], parser, argument substitution, data model, and
//! the shared builtin scaffolding ([`builtin_support`] — the locked 99-name
//! table, the `/help` + list renderers, and the per-name unimplemented stub
//! handler).
//!
//! The command *handlers* live in the impl crates `command-core`
//! (cross-platform core), `command-desktop`, and `command-mobile`, each
//! exposing a `register()` entry point the composition roots call. This mirrors
//! the `tool-api` / `tool-*` and `skill-api` / `skill-builtin` splits.
//!
//! See spec §19 for the broader slash-command design.

#![forbid(unsafe_code)]

pub mod argument_substitution;
pub mod builtin_support;
pub mod dispatcher;
pub mod model;
pub mod parser;
pub mod registry;
pub mod shell_expansion;

pub use argument_substitution::{
    parse_argument_names, parse_arguments, substitute_arguments, substitute_arguments_faithful,
    FrontmatterArgs,
};
pub use dispatcher::RegistrySlashDispatcher;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::CommandRegistry;
pub use shell_expansion::{
    execute_shell_commands_in_prompt, ShellExpansionCtx, ShellExpansionError, ShellOut,
    ShellPermissionDecision, ShellPermissionGate, ShellRunError, ShellRunner,
};
