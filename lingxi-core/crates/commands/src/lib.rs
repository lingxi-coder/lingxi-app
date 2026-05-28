//! Slash-command subsystem: parser, argument substitution, registry, and
//! built-in command handlers.
//!
//! See spec §19.

#![forbid(unsafe_code)]

pub mod argument_substitution;
pub mod builtin;
pub mod model;
pub mod parser;
pub mod registry;

pub use argument_substitution::substitute_arguments;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::{register_all_builtin_commands, CommandRegistry};
