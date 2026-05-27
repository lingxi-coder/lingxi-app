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

/// Closing literal of every assembled system prompt.
///
/// Source: `claude-code/src/constants/prompts.ts:766-770` (the
/// `notes:` block inside `enhanceSystemPromptWithEnvDetails`). The
/// em-dash character (U+2014, 3 UTF-8 bytes) appears once in bullet 2.
/// Byte length is asserted in `prompt_assemble_test::footer_byte_length_locked`
/// (re-verify via `wc -c` after each claude-code rebase).
pub const FOOTER: &str = "Notes:\n\
- Agent threads always have their cwd reset between bash calls, as a result please only use absolute file paths.\n\
- In your final response, share file paths (always absolute, never relative) that are relevant to the task. Include code snippets only when the exact text is load-bearing (e.g., a bug you found, a function signature the caller asked for) — do not recap code you merely read.\n\
- For clear communication with the user the assistant MUST avoid using emojis.\n\
- Do not use a colon before tool calls. Text like \"Let me read the file:\" followed by a read tool call should just be \"Let me read the file.\" with a period.\n";
