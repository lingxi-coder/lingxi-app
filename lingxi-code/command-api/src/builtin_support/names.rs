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
    (
        "advisor",
        "compiled stub in claude-code (gated advisor surface)",
    ),
    ("autofix-pr", "compiled stub in claude-code"),
    (
        "backfill-sessions",
        "compiled stub in claude-code (internal maintenance)",
    ),
    ("brief", "entitlement-gated in claude-code"),
    ("btw", "entitlement-gated in claude-code"),
    ("bughunter", "compiled stub in claude-code"),
    ("ctx-viz", "compiled stub in claude-code (internal debug)"),
    (
        "debug-tool-call",
        "compiled stub in claude-code (internal debug)",
    ),
    ("env", "compiled stub in claude-code"),
    ("good-claude", "compiled stub in claude-code (internal)"),
    ("issue", "compiled stub in claude-code"),
    (
        "mock-limits",
        "compiled stub in claude-code (internal rate-limit testing)",
    ),
    (
        "oauth-refresh",
        "compiled stub in claude-code (internal auth maintenance)",
    ),
    ("onboarding", "compiled stub in claude-code (internal)"),
    ("perf-issue", "compiled stub in claude-code"),
    (
        "reset-limits",
        "compiled stub in claude-code (internal rate-limit testing)",
    ),
    ("summary", "compiled stub in claude-code"),
    ("tag", "USER_TYPE==='ant' (Anthropic-internal only)"),
    ("teleport", "compiled stub in claude-code (internal)"),
    ("thinkback", "statsig-gated in claude-code"),
    ("thinkback-play", "statsig-gated in claude-code"),
    (
        "x402",
        "entitlement-gated in claude-code (crypto micropayments)",
    ),
    (
        "break-cache",
        "compiled stub in claude-code (internal cache control)",
    ),
    ("share", "compiled stub in claude-code"),
    ("reload-plugins", "local/internal stub in claude-code"),
];

// ============================================================================
// STUB.6 — faithful-stub audit: refine the bucket-(d) table into TWO disjoint
// partitions, verified against the claude-code TypeScript/compiled reference.
//
// The legacy `INTENTIONALLY_DISABLED_COMMANDS` table (above, kept verbatim for
// git-history + existing-test continuity) conflated two *different* parity
// situations under one "disabled" label. STUB.6 splits them so a future
// contributor can tell, per command, whether the Rust stub is **faithful**
// (claude-code itself ships nothing to run) or a **real (deferred) gap**
// (claude-code ships a working command; the Rust port only lacks the host
// infra to run it). The two are not interchangeable: only the first set is
// "correct-by-design", and only the second is worth implementation effort.
//
// Evidence was taken directly from `claude-code/src/commands/<name>/`:
//   - compiled `index.js` literally `{ isEnabled: () => false, isHidden: true,
//     name: 'stub' }`  → the command never runs for ANYONE.
//   - `isEnabled: () => process.env.USER_TYPE === 'ant'`  → Anthropic-internal.
//   - `isEnabled: () => checkStatsigFeatureGate(...)` / `feature('KAIROS')`  →
//     feature-gated OFF in the external build.
//   - `isEnabled: () => canUserConfigureAdvisor()` + `isHidden` when not
//     configurable  → entitlement-gated OFF + hidden by default.
// ============================================================================

/// **Partition A — CORRECT-BY-DESIGN faithful stubs.** claude-code *itself*
/// disables, hides, feature-gates-OFF, or ships a literal no-op `name: 'stub'`
/// for each of these, so the Rust port returning the locked
/// `"{name}: not implemented in v0.6.0 (M5)"` literal is **behaviorally
/// faithful** — there is no command body to port. These are non-goals: a
/// future contributor must NOT "implement" them.
///
/// Each entry is `(name, why-claude-code-ships-nothing)`. Every `name` is a
/// member of [`BUILTIN_COMMAND_NAMES`], disjoint from the implemented core set
/// [`BUILTIN_CORE_NAMES`], and disjoint from [`HOST_BOUND_DEFERRED_GAPS`]
/// (asserted by the STUB.6 partition tests in the test-harness).
///
/// This is the refined subset of [`INTENTIONALLY_DISABLED_COMMANDS`]: it is
/// that table MINUS the three names that claude-code actually implements
/// (see [`HOST_BOUND_DEFERRED_GAPS`]).
pub const CORRECT_BY_DESIGN_STUBS: &[(&str, &str)] = &[
    // --- 18 compiled `{ isEnabled:()=>false, isHidden:true, name:'stub' }` ---
    (
        "ant-trace",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "autofix-pr",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "backfill-sessions",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "break-cache",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "bughunter",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "ctx-viz",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "debug-tool-call",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "env",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "good-claude",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "issue",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "mock-limits",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "oauth-refresh",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "onboarding",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "perf-issue",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "reset-limits",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "share",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "summary",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    (
        "teleport",
        "compiled `name:'stub'` (isEnabled:()=>false, isHidden) in claude-code",
    ),
    // --- USER_TYPE==='ant' (Anthropic-internal only) ---
    (
        "tag",
        "isEnabled:()=>process.env.USER_TYPE==='ant' (Anthropic-internal only)",
    ),
    // --- statsig / feature-gated OFF in the external build ---
    (
        "thinkback",
        "isEnabled gated by statsig `tengu_thinkback` (off externally)",
    ),
    (
        "thinkback-play",
        "statsig `tengu_thinkback` + isHidden:true (off externally)",
    ),
    (
        "brief",
        "isEnabled:()=>feature('KAIROS')&&config — feature gate OFF externally",
    ),
    // --- entitlement-gated OFF + hidden by default ---
    (
        "advisor",
        "isEnabled:()=>canUserConfigureAdvisor() (false by default) + isHidden",
    ),
];

/// **Partition B — HOST-BOUND-DEFERRED gaps (NOT correct-by-design).**
/// claude-code *implements* each of these (the source has a real body and is
/// enabled for ordinary users — **no** `isEnabled:()=>false`, no ant/statsig
/// gate). The Rust port returns the stub literal only because the supporting
/// host surface is not wired yet (an interactive JSX/TUI dialog, or an SDK
/// control-request path that has no plain-text registry analog). These are
/// **genuine deferred gaps**, a different partition from
/// [`CORRECT_BY_DESIGN_STUBS`]: implementing them IS in-scope future work, so
/// they must never be mislabeled as faithful-by-design.
///
/// Each entry is `(name, what-claude-code-actually-ships + why-deferred)`.
pub const HOST_BOUND_DEFERRED_GAPS: &[(&str, &str)] = &[
    ("btw", "claude-code ships a `local-jsx` side-question dialog, enabled (no gate); deferred for lack of TUI dialog infra"),
    ("x402", "claude-code ships a `local` text command (supportsNonInteractive, no isEnabled gate); deferred host wallet/x402 service infra"),
    ("reload-plugins", "claude-code ships a `local` command (no gate); driven via SDK control-request, no plain-text registry analog yet"),
];

/// Descriptions for the 18 core commands, used for `/help` rendering in M5-10.
/// Lookup by core name; falls back to `"(unimplemented in v0.6.0)"` for the 81
/// non-core entries.
#[must_use]
pub fn core_description(name: &str) -> &'static str {
    match name {
        "agents" => "Manage agent configurations",
        "clear" => "Start a new session with empty context; previous session stays on disk (resumable with /resume)",
        "compact" => "Free up context by summarizing the conversation so far",
        "config" => "Open settings",
        "cost" => "Show total cost and duration of the current session",
        "doctor" => "Diagnose and verify your Claude Code installation and settings",
        "exit" => "Exit the REPL",
        "help" => "Show help and available commands",
        "hooks" => "Manage hooks",
        "init" => "Initialize a new CLAUDE.md file with codebase documentation",
        "login" => "Sign in with your Anthropic account",
        "logout" => "Sign out from your Anthropic account",
        "mcp" => "Manage MCP servers",
        "memory" => "Open a memory file in your editor",
        "model" => "Set the AI model for Claude Code",
        "permissions" => "Manage allow and deny tool permission rules",
        "status" => "Show Claude Code status including version, model, account, API connectivity, and tool statuses",
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
        let full: std::collections::HashSet<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
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

    // ========================================================================
    // STUB.6 — the refined two-partition split (CORRECT_BY_DESIGN_STUBS vs
    // HOST_BOUND_DEFERRED_GAPS). These lock the *classification invariant* of
    // the constants; the end-to-end "still dispatches to the stub literal"
    // regression lives in test-harness (parity_slash_command_stub_partition).
    // ========================================================================

    fn name_set(table: &[(&'static str, &'static str)]) -> std::collections::HashSet<&'static str> {
        table.iter().map(|(n, _)| *n).collect()
    }

    #[test]
    fn correct_by_design_count_is_23() {
        assert_eq!(CORRECT_BY_DESIGN_STUBS.len(), 23);
    }

    #[test]
    fn host_bound_deferred_count_is_3() {
        assert_eq!(HOST_BOUND_DEFERRED_GAPS.len(), 3);
    }

    #[test]
    fn every_partitioned_name_is_a_real_builtin() {
        let full: std::collections::HashSet<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
        for (name, _) in CORRECT_BY_DESIGN_STUBS
            .iter()
            .chain(HOST_BOUND_DEFERRED_GAPS)
        {
            assert!(
                full.contains(name),
                "partitioned name '{name}' is not a real builtin"
            );
        }
    }

    #[test]
    fn every_partitioned_reason_is_documented() {
        for (name, reason) in CORRECT_BY_DESIGN_STUBS
            .iter()
            .chain(HOST_BOUND_DEFERRED_GAPS)
        {
            assert!(!reason.trim().is_empty(), "'{name}' must document a reason");
        }
    }

    #[test]
    fn no_duplicate_names_within_each_partition() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in CORRECT_BY_DESIGN_STUBS
            .iter()
            .chain(HOST_BOUND_DEFERRED_GAPS)
        {
            assert!(seen.insert(*name), "duplicate partitioned name '{name}'");
        }
    }

    #[test]
    fn the_two_partitions_are_disjoint() {
        // The whole point of STUB.6: a name cannot be BOTH a faithful
        // correct-by-design stub AND a genuine deferred gap.
        let cbd = name_set(CORRECT_BY_DESIGN_STUBS);
        let gaps = name_set(HOST_BOUND_DEFERRED_GAPS);
        assert!(
            cbd.is_disjoint(&gaps),
            "correct-by-design and host-bound-deferred overlap"
        );
    }

    #[test]
    fn both_partitions_are_disjoint_from_implemented_core() {
        // Neither a faithful stub nor a deferred gap may also be a wired core
        // command — that would be a contradiction.
        let core: std::collections::HashSet<&str> = BUILTIN_CORE_NAMES.iter().copied().collect();
        for (name, _) in CORRECT_BY_DESIGN_STUBS
            .iter()
            .chain(HOST_BOUND_DEFERRED_GAPS)
        {
            assert!(
                !core.contains(name),
                "'{name}' is both core (implemented) and stubbed"
            );
        }
    }

    #[test]
    fn partitions_union_equals_legacy_intentionally_disabled_set() {
        // The refined split is exactly the legacy bucket-(d) table re-bucketed:
        // CORRECT_BY_DESIGN ∪ HOST_BOUND_DEFERRED == INTENTIONALLY_DISABLED.
        // This proves the refinement neither dropped nor invented a name; it
        // only moved the three host-bound names into their correct partition.
        let legacy = name_set(INTENTIONALLY_DISABLED_COMMANDS);
        let mut union = name_set(CORRECT_BY_DESIGN_STUBS);
        union.extend(name_set(HOST_BOUND_DEFERRED_GAPS));
        assert_eq!(
            union, legacy,
            "partition union must equal the legacy disabled set"
        );
        assert_eq!(
            CORRECT_BY_DESIGN_STUBS.len() + HOST_BOUND_DEFERRED_GAPS.len(),
            INTENTIONALLY_DISABLED_COMMANDS.len(),
            "23 + 3 == 26"
        );
    }

    #[test]
    fn host_bound_deferred_holds_exactly_the_three_implemented_names() {
        // Lock the specific three claude-code IMPLEMENTS (verified against
        // src/commands/{btw,x402,reload-plugins}: no isEnabled gate, real body)
        // so they can never silently slide back into the correct-by-design set.
        let gaps = name_set(HOST_BOUND_DEFERRED_GAPS);
        let expected: std::collections::HashSet<&str> =
            ["btw", "x402", "reload-plugins"].into_iter().collect();
        assert_eq!(gaps, expected);
    }
}
