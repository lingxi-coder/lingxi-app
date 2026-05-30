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

pub use argument_substitution::substitute_arguments;
pub use dispatcher::RegistrySlashDispatcher;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::CommandRegistry;
