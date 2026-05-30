//! Locked constant tables of the 99 builtin command names + the 18 core names.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 2 (99 list) + Task 0 step 3 (18 core list).
//!
//! NOTE: The 2026-05-28 addendum locked the total at **99** (originally `102`
//! in the plan prose). The 18 core count is unchanged.

/// Every built-in slash command's runtime name (without leading `/`),
/// ASCII-sorted. Locked at length **99** for v0.6.0.
///
/// Changing the count or membership requires bumping the parity fixture
/// `crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json`
/// (fixture filename retained for git-history continuity; the counts inside
/// reflect the 99/81/18 lock per the 2026-05-28 addendum).
pub const BUILTIN_COMMAND_NAMES: &[&str; 99] = &[
    "add-dir",
    "advisor",
    "agents",
    "ant-trace",
    "autofix-pr",
    "backfill-sessions",
    "branch",
    "break-cache",
    "bridge",
    "brief",
    "btw",
    "bughunter",
    "chrome",
    "clear",
    "color",
    "commit",
    "commit-push-pr",
    "compact",
    "config",
    "context",
    "copy",
    "cost",
    "ctx-viz",
    "debug-tool-call",
    "desktop",
    "diff",
    "doctor",
    "effort",
    "env",
    "exit",
    "export",
    "extra-usage",
    "fast",
    "feedback",
    "files",
    "good-claude",
    "heapdump",
    "help",
    "hooks",
    "ide",
    "init",
    "init-verifiers",
    "insights",
    "install",
    "install-github-app",
    "install-slack-app",
    "issue",
    "keybindings",
    "login",
    "logout",
    "mcp",
    "memory",
    "mobile",
    "mock-limits",
    "model",
    "oauth-refresh",
    "onboarding",
    "output-style",
    "passes",
    "perf-issue",
    "permissions",
    "plan",
    "plugin",
    "pr-comments",
    "privacy-settings",
    "rate-limit-options",
    "release-notes",
    "reload-plugins",
    "remote-env",
    "remote-setup",
    "rename",
    "reset-limits",
    "resume",
    "review",
    "rewind",
    "sandbox-toggle",
    "security-review",
    "session",
    "share",
    "skills",
    "stats",
    "status",
    "statusline",
    "stickers",
    "summary",
    "tag",
    "tasks",
    "teleport",
    "terminal-setup",
    "theme",
    "thinkback",
    "thinkback-play",
    "ultraplan",
    "upgrade",
    "usage",
    "version",
    "vim",
    "voice",
    "x402",
];

/// The 18 core commands that ship with real implementations in M5-10 / M5-11.
/// Subset of [`BUILTIN_COMMAND_NAMES`], ASCII-sorted.
///
/// In M5-09 each of these gets a per-name placeholder struct in
/// [`super::core_placeholders`]; the placeholders return the same locked stub
/// literal as the shared `UnimplementedCommandHandler` until the real bodies
/// land in M5-10 (batch 1: clear/compact/help/exit/memory/init) and M5-11
/// (batch 2: the remaining 12).
pub const BUILTIN_CORE_NAMES: &[&str; 18] = &[
    "agents",
    "clear",
    "compact",
    "config",
    "cost",
    "doctor",
    "exit",
    "help",
    "hooks",
    "init",
    "login",
    "logout",
    "mcp",
    "memory",
    "model",
    "permissions",
    "status",
    "version",
];

/// Descriptions for the 18 core commands, used for `/help` rendering in M5-10.
/// Lookup by core name; falls back to `"(unimplemented in v0.6.0)"` for the 81
/// non-core entries.
#[must_use]
pub fn core_description(name: &str) -> &'static str {
    match name {
        "agents" => "Manage subagents",
        "clear" => "Clear conversation history and free up context",
        "compact" => "Compact the conversation to a summary",
        "config" => "Open config panel",
        "cost" => "Show total cost and duration of the current session",
        "doctor" => "Diagnose installation and configuration",
        "exit" => "Exit the REPL",
        "help" => "Show help and available commands",
        "hooks" => "Manage hooks",
        "init" => "Initialize a new CLAUDE.md file with codebase documentation",
        "login" => "Sign in with your Anthropic account",
        "logout" => "Sign out from your Anthropic account",
        "mcp" => "Manage MCP servers",
        "memory" => "Edit Claude memory files",
        "model" => "Set the model for Claude Code to use",
        "permissions" => "Manage permissions",
        "status" => "Show Claude Code status",
        "version" => "Print version information",
        _ => "(unimplemented in v0.6.0)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_count_locked_at_99() {
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 99);
    }

    #[test]
    fn core_count_locked_at_18() {
        assert_eq!(BUILTIN_CORE_NAMES.len(), 18);
    }

    #[test]
    fn names_are_sorted_ascii_ascending() {
        let mut sorted = BUILTIN_COMMAND_NAMES.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            BUILTIN_COMMAND_NAMES.as_slice(),
            sorted.as_slice(),
            "BUILTIN_COMMAND_NAMES must be ASCII-sorted to keep diffs reviewable"
        );
    }

    #[test]
    fn core_names_are_sorted_ascii_ascending() {
        let mut sorted = BUILTIN_CORE_NAMES.to_vec();
        sorted.sort_unstable();
        assert_eq!(BUILTIN_CORE_NAMES.as_slice(), sorted.as_slice());
    }

    #[test]
    fn no_duplicate_names() {
        let mut set = std::collections::HashSet::new();
        for n in BUILTIN_COMMAND_NAMES {
            assert!(
                set.insert(*n),
                "duplicate name {n} in BUILTIN_COMMAND_NAMES"
            );
        }
    }

    #[test]
    fn every_core_name_is_in_the_full_list() {
        let full: std::collections::HashSet<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
        for c in BUILTIN_CORE_NAMES {
            assert!(
                full.contains(c),
                "core name {c} not in BUILTIN_COMMAND_NAMES"
            );
        }
    }

    #[test]
    fn all_names_are_lowercase_ascii_or_hyphen_or_digit() {
        for n in BUILTIN_COMMAND_NAMES {
            for c in n.chars() {
                assert!(
                    c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit(),
                    "name '{n}' has invalid char '{c}' (only lowercase ascii + '-' + digits allowed)"
                );
            }
        }
    }

    #[test]
    fn no_name_starts_or_ends_with_hyphen() {
        for n in BUILTIN_COMMAND_NAMES {
            assert!(!n.starts_with('-'), "name '{n}' starts with hyphen");
            assert!(!n.ends_with('-'), "name '{n}' ends with hyphen");
        }
    }

    #[test]
    fn no_double_hyphen() {
        for n in BUILTIN_COMMAND_NAMES {
            assert!(!n.contains("--"), "name '{n}' contains '--'");
        }
    }

    #[test]
    fn includes_known_canonical_names() {
        // Spot-check a few rare ones so a future name rename doesn't drift silently.
        assert!(BUILTIN_COMMAND_NAMES.contains(&"x402"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"ctx-viz"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"pr-comments"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"thinkback-play"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"terminal-setup"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"ant-trace"));
    }
}
