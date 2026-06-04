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

/// Bucket **(d)** — commands that **claude-code itself** disables, hides, or
/// gates behind an entitlement / `USER_TYPE` / statsig flag, so returning the
/// locked stub literal is **already behaviorally faithful** — these are *not*
/// a Rust parity gap.
///
/// In the leaked claude-code TypeScript reference each of these names either:
///
/// - compiles to a `name: 'stub'` command object in the shipped `index.js`
///   (the command exists in the palette but its body is a no-op stub), or
/// - is `USER_TYPE === 'ant'`-gated (Anthropic-internal only), or
/// - is statsig- / entitlement-gated (the body never runs for normal users).
///
/// Therefore the Rust port keeps every name here on the shared
/// [`super::UnimplementedCommandHandler`] **by design**. This table exists to
/// document *why* each is a non-goal and to prevent a future contributor from
/// "implementing" a command that claude-code itself ships as a stub.
///
/// Each entry is `(name, reason)`. Every `name` is a member of
/// [`BUILTIN_COMMAND_NAMES`] and is **disjoint** from the implemented core set
/// [`BUILTIN_CORE_NAMES`] (see the unit tests below). Cross-reference: SPECS.md
/// "slash-commands-stubs" bucket (d) + Batch 6.
///
/// NOTE: this is purely additive documentation — it changes **no** runtime
/// behavior and does **not** alter the locked 99-name surface.
pub const INTENTIONALLY_DISABLED_COMMANDS: &[(&str, &str)] = &[
    ("ant-trace", "USER_TYPE==='ant' (Anthropic-internal only)"),
    ("advisor", "compiled stub in claude-code (gated advisor surface)"),
    ("autofix-pr", "compiled stub in claude-code"),
    ("backfill-sessions", "compiled stub in claude-code (internal maintenance)"),
    ("brief", "entitlement-gated in claude-code"),
    ("btw", "entitlement-gated in claude-code"),
    ("bughunter", "compiled stub in claude-code"),
    ("ctx-viz", "compiled stub in claude-code (internal debug)"),
    ("debug-tool-call", "compiled stub in claude-code (internal debug)"),
    ("env", "compiled stub in claude-code"),
    ("good-claude", "compiled stub in claude-code (internal)"),
    ("issue", "compiled stub in claude-code"),
    ("mock-limits", "compiled stub in claude-code (internal rate-limit testing)"),
    ("oauth-refresh", "compiled stub in claude-code (internal auth maintenance)"),
    ("onboarding", "compiled stub in claude-code (internal)"),
    ("perf-issue", "compiled stub in claude-code"),
    ("reset-limits", "compiled stub in claude-code (internal rate-limit testing)"),
    ("summary", "compiled stub in claude-code"),
    ("tag", "USER_TYPE==='ant' (Anthropic-internal only)"),
    ("teleport", "compiled stub in claude-code (internal)"),
    ("thinkback", "statsig-gated in claude-code"),
    ("thinkback-play", "statsig-gated in claude-code"),
    ("x402", "entitlement-gated in claude-code (crypto micropayments)"),
    ("break-cache", "compiled stub in claude-code (internal cache control)"),
    ("share", "compiled stub in claude-code"),
    ("reload-plugins", "local/internal stub in claude-code"),
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

    // ========================================================================
    // Batch 6 — bucket (d) faithful-stub audit (additive; behavior-neutral).
    //
    // These tests lock the INTENTIONALLY_DISABLED_COMMANDS table as documented
    // non-goals: every name is a real builtin that claude-code itself ships as a
    // stub/gated, so it must remain on the UnimplementedCommandHandler by
    // design. The tests assert the *classification invariant* only — they make
    // no change to the locked 99-name surface (`parity_slash_commands_102`).
    // ========================================================================

    #[test]
    fn intentionally_disabled_count_is_26() {
        // Bucket (d) is 26 names per SPECS.md "slash-commands-stubs".
        assert_eq!(
            INTENTIONALLY_DISABLED_COMMANDS.len(),
            26,
            "bucket (d) is locked at 26 commands"
        );
    }

    #[test]
    fn every_intentionally_disabled_name_is_a_real_builtin() {
        let full: std::collections::HashSet<&str> =
            BUILTIN_COMMAND_NAMES.iter().copied().collect();
        for (name, _reason) in INTENTIONALLY_DISABLED_COMMANDS {
            assert!(
                full.contains(name),
                "intentionally-disabled name '{name}' is not in BUILTIN_COMMAND_NAMES"
            );
        }
    }

    #[test]
    fn no_duplicate_intentionally_disabled_names() {
        let mut set = std::collections::HashSet::new();
        for (name, _reason) in INTENTIONALLY_DISABLED_COMMANDS {
            assert!(
                set.insert(*name),
                "duplicate name '{name}' in INTENTIONALLY_DISABLED_COMMANDS"
            );
        }
    }

    #[test]
    fn every_intentionally_disabled_reason_is_documented() {
        // The whole point of the table is to record *why* each is a non-goal.
        for (name, reason) in INTENTIONALLY_DISABLED_COMMANDS {
            assert!(
                !reason.trim().is_empty(),
                "intentionally-disabled name '{name}' must document a reason"
            );
        }
    }

    #[test]
    fn intentionally_disabled_is_disjoint_from_core() {
        // Bucket (d) commands are NOT implemented; the 18 core names ARE wired
        // to real handlers. The two sets must never overlap, otherwise a name is
        // both "implemented" and "intentionally stubbed" — a contradiction.
        let core: std::collections::HashSet<&str> = BUILTIN_CORE_NAMES.iter().copied().collect();
        for (name, _reason) in INTENTIONALLY_DISABLED_COMMANDS {
            assert!(
                !core.contains(name),
                "name '{name}' is both core (implemented) and intentionally-disabled"
            );
        }
    }

    #[test]
    fn intentionally_disabled_set_is_a_documented_non_goal() {
        // Spot-check the three gating *kinds* from the spec so the rationale
        // does not silently drift: USER_TYPE==='ant', statsig-gated, and the
        // compiled `name: 'stub'` files. This asserts membership of the
        // representative names — it is a documentation lock, not a behavior test.
        let table: std::collections::HashMap<&str, &str> =
            INTENTIONALLY_DISABLED_COMMANDS.iter().copied().collect();

        // USER_TYPE==='ant' (Anthropic-internal only).
        assert!(table.contains_key("ant-trace"));
        assert!(table.contains_key("tag"));
        // statsig-gated.
        assert!(table.contains_key("thinkback"));
        assert!(table.contains_key("thinkback-play"));
        // entitlement-gated.
        assert!(table.contains_key("x402"));
        assert!(table.contains_key("brief"));
        assert!(table.contains_key("btw"));
        // compiled `name: 'stub'` files.
        for n in [
            "env",
            "share",
            "summary",
            "teleport",
            "autofix-pr",
            "backfill-sessions",
            "bughunter",
            "ctx-viz",
            "debug-tool-call",
            "good-claude",
            "issue",
            "mock-limits",
            "oauth-refresh",
            "onboarding",
            "perf-issue",
            "reset-limits",
        ] {
            assert!(table.contains_key(n), "expected compiled-stub name '{n}'");
        }
    }

    #[test]
    fn intentionally_disabled_does_not_change_the_locked_surface() {
        // Guard the no-op property: the additive bucket-(d) table is a strict
        // subset of the locked 99-name list and therefore cannot change the
        // total count, membership, or ordering that the parity fixture locks.
        assert!(INTENTIONALLY_DISABLED_COMMANDS.len() < BUILTIN_COMMAND_NAMES.len());
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 99);
    }
}
