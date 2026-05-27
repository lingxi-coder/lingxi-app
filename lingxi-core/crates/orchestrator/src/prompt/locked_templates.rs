//! Header + footer constants — byte-locked from claude-code prompts.ts /
//! system.ts. See M5-03 plan "Reverse-engineered byte-locks".
#![forbid(unsafe_code)]

/// Opening literal of every assembled system prompt.
///
/// Source: `claude-code/src/constants/system.ts:10` (`DEFAULT_PREFIX`).
/// Length: 57 bytes (no leading/trailing whitespace; no LF).
pub const HEADER: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Section separator between header / `<env>` / `<memory>` /
/// `<tools>` / footer. Two LFs (one blank line).
pub const SECTION_SEP: &str = "\n\n";

/// Final trailing newline appended once at the end of `assemble_system_prompt`.
pub const TRAILING_NL: &str = "\n";
