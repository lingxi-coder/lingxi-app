//! Locked constant tables of the 100 builtin command names + the 18 core names.
//!
//! See plan `docs/superpowers/plans/2026-05-25-m5-09-commands-surface.md`
//! Task 0 step 2 (name list) + Task 0 step 3 (18 core list).
//!
//! NOTE: The 2026-05-28 addendum locked the total at **99** (originally `102`
//! in the plan prose). The 2026-06-20 slash-parity pass (findings #66/#67 vs
//! claude-code v2.1.183) re-locked the total to **94**: it removed the three
//! commands claude-code deleted upstream (`vim`, `pr-comments`, `output-style`
//! — 0 command objects in the v2.1.183 binary) and the two that became
//! `aliases:["cost","stats"]` of `/usage` rather than standalone commands
//! (`cost`, `stats` — 0 `name:"cost"`/`name:"stats"` command objects). The 18
//! core count is unchanged: `cost`'s core slot was reassigned to `usage` (now
//! the implemented command that absorbs cost/stats).
//!
//! The 2026-07-04 batch-8 slash-command pass added six newly-ported,
//! implemented commands (`fork`, `goal`, `recap`, `reload-skills`,
//! `skill-doctor`, `stop`), re-locking the total from 94 to **100**. The 18
//! core count is unchanged (the new commands are wired via
//! `command_core::register_core_batch_8`, not the core placeholder path).
//!
//! The follow-on batch-8 tail pass added the net-new headless `autocompact`
//! command (the real `type:"local",supportsNonInteractive:!0` auto-compact
//! window reporter — see `command_core::autocompact`), re-locking the total
//! from 100 to **101**. Its stub-bucket siblings from the same triage
//! (`powerup`, `scroll-speed` = interactive-only net-new; `install`,
//! `sandbox-toggle` = interactive/host-bound already-in-surface stubs) were
//! kept as stubs / not added, so only the total moved. `/btw` is now wired
//! through batch 8 and the TUI retains its most recent side-question panel.
//!
//! The 2026-07-14 slash-parity pass (H-BIN-11 vs claude-code v2.1.207) added
//! FIVE `local-jsx` builtin command objects that had been missed by the
//! name-lock lineage — they exist byte-identically as far back as the local
//! 2.1.205 binary, so they are pre-existing misses, not post-lock drift:
//! `cd` (`Move this session to a new working directory`, ungated),
//! `background`/alias `bg` (`Send this session to the background and free the
//! terminal`, `isEnabled:()=>!0`), `focus` (`Toggle focus view: just your
//! prompt, summary, and response`, `requires:{ink}`), `tui` (`Set the terminal
//! UI renderer (default | fullscreen)`, ungated), and `usage-credits`
//! (`Configure usage credits or request them from your admin when you hit a
//! limit`, two objects
//! gated by `bnr()` = `!DISABLE_EXTRA_USAGE_COMMAND && (rateLimitStatus!==null
//! || isOverageProvisioningAllowed())`, split interactive/non-interactive on
//! `isNonInteractiveSession()`). That oracle total was **106**: 105 after
//! removing the stale `x402` entry, plus `auto-mode-setup` (2.1.220 WIZARD-06,
//! `{type:"local",name:"auto-mode-setup",supportsNonInteractive:!0}`).
//! `usage-credits` is hidden from the default palette / `/help` because `bnr()`
//! resolves `false` in a fresh session with no subscription or rate-limit
//! status (see [`is_palette_hidden`] + [`USAGE_CREDITS_BNR_GATED`]); the other
//! four are visible. The 2026-08-13 follow-up adds the pre-existing 2.1.205
//! `local-jsx` `/workflows` command already implemented by the TUI, re-locking
//! the shared registry surface at **107**.
//!
//! The 2026-08-20 byte-alignment pass vs claude-code **2.1.238** keeps the
//! total at **107** through a one-in / one-out swap: `review` was DELETED
//! upstream (`name:"review"` 2.1.220 = 1 hit, 2.1.238 = 0; the PR-review
//! surface moved into the bundled `code-review` skill), and `subtask`
//! (`{type:"local-jsx",name:"subtask",description:"Send a subagent off with
//! your full context; its result comes back here",argumentHint:"<task>"}`
//! @ 2.1.238 296247354) — long implemented by `command_core::subtask` but
//! never listed here, so never advertised in `/help` or the palette — takes
//! its slot.

/// Every built-in slash command's runtime name (without leading `/`),
/// ASCII-sorted. Locked at length **107** for the current oracle.
///
/// Changing the count or membership requires bumping the parity fixture
/// `crates/test-harness/src/parity/fixtures/parity_slash_commands_102.json`
/// (fixture filename retained for git-history continuity; the counts inside
/// reflect the current membership lock).
pub const BUILTIN_COMMAND_NAMES: &[&str; 107] = &[
    "add-dir",
    "advisor",
    "agents",
    "ant-trace",
    "auto-mode-setup",
    "autocompact",
    "autofix-pr",
    "backfill-sessions",
    "background",
    "branch",
    "break-cache",
    "bridge",
    "brief",
    "btw",
    "bughunter",
    "cd",
    "chrome",
    "clear",
    "color",
    "commit",
    "commit-push-pr",
    "compact",
    "config",
    "context",
    "copy",
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
    "focus",
    "fork",
    "goal",
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
    "passes",
    "perf-issue",
    "permissions",
    "plan",
    "plugin",
    "privacy-settings",
    "rate-limit-options",
    "recap",
    "release-notes",
    "reload-plugins",
    "reload-skills",
    "remote-env",
    "remote-setup",
    "rename",
    "reset-limits",
    "resume",
    "rewind",
    "sandbox-toggle",
    "security-review",
    "session",
    "share",
    "skill-doctor",
    "skills",
    "status",
    "statusline",
    "stickers",
    "stop",
    "subtask",
    "summary",
    "tag",
    "tasks",
    "teleport",
    "terminal-setup",
    "theme",
    "thinkback",
    "thinkback-play",
    "tui",
    "ultraplan",
    "upgrade",
    "usage",
    "usage-credits",
    "version",
    "voice",
    "workflows",
];

/// The 18 core commands that ship with real implementations in M5-10 / M5-11.
/// Subset of [`BUILTIN_COMMAND_NAMES`], ASCII-sorted.
///
/// In M5-09 each of these gets a per-name placeholder struct in
/// [`super::core_placeholders`]; the placeholders return the same locked stub
/// literal as the shared `UnimplementedCommandHandler` until the real bodies
/// land in M5-10 (batch 1: clear/compact/help/exit/memory/init) and M5-11
/// (batch 2: the remaining 12).
pub const BUILTIN_CORE_NAMES: &[&str; 19] = &[
    "agents",
    "auto-mode-setup",
    "clear",
    "compact",
    "config",
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
    "usage",
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
/// behavior and does **not** alter the locked command surface.
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
        "break-cache",
        "compiled stub in claude-code (internal cache control)",
    ),
    ("share", "compiled stub in claude-code"),
];

// ============================================================================
// STUB.6 — faithful-stub audit: refine the bucket-(d) table into TWO disjoint
// partitions, verified against the claude-code TypeScript/compiled reference.
//
// The legacy `INTENTIONALLY_DISABLED_COMMANDS` table once conflated faithful
// stubs with host-bound gaps. The last host-bound entry (`btw`) now has a live
// handler and TUI panel, leaving only the correct-by-design partition.
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
/// This is now the complete [`INTENTIONALLY_DISABLED_COMMANDS`] set.
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

/// Host/UI-bound command gaps. Empty after `/btw` gained both a handle-bound
/// command implementation and a reopenable TUI panel.
pub const HOST_BOUND_DEFERRED_GAPS: &[(&str, &str)] = &[];

/// **Statically-hidden named commands** — real, enabled (or conditionally
/// enabled) builtin command objects that claude-code ships with the literal
/// `isHidden:!0` flag, so they NEVER appear in the slash palette or `/help`
/// even though they remain dispatchable when typed in full.
///
/// These are NOT in [`CORRECT_BY_DESIGN_STUBS`] (they are not `name:'stub'`
/// no-ops, and several are conditionally enabled), so they must be filtered
/// separately. Verified against the shipped `claude.exe` v2.1.183 command
/// objects (each carries a literal `isHidden:!0` immediately after its
/// `name`/`description`):
///
/// ```text
/// name:"extra-usage",description:"Renamed to /usage-credits",isHidden:!0,isEnabled:()=>pct()&&!kr()
/// name:"heapdump",description:"Dump the JS heap to ~/Desktop",isHidden:!0,...
/// name:"rate-limit-options",description:"Show options when rate limit is reached",isEnabled:()=>Ro()||!1,isHidden:!0
/// ```
///
/// claude-code's palette / `/help` list builders all apply the filter
/// `commands.filter(c => !c.isHidden && !$te(c))` (3 confirmed sites:
/// `!ne.isHidden&&!$te(ne)`, `!S.isHidden&&!$te(S)`, `!Ur.isHidden&&!$te(Ur)`),
/// so any `isHidden:!0` command is dropped from both surfaces.
pub const HIDDEN_PALETTE_COMMANDS: &[&str] = &[
    "auto-mode-setup",
    "extra-usage",
    "heapdump",
    "rate-limit-options",
];

/// **`bnr()`-gated, hidden-by-default named commands** — real, conditionally
/// enabled builtin command objects whose `isEnabled` resolves to `false` in a
/// fresh session with no subscription and no rate-limit status, so they are
/// dropped from the default palette / `/help` even though they remain
/// dispatchable when typed in full.
///
/// H-BIN-11 (cc2.1.207): `/usage-credits` ships two objects, both gated by
/// `isEnabled:()=>bnr()&&…` where
/// `bnr()=!DISABLE_EXTRA_USAGE_COMMAND && (rateLimitStatus!==null ||
/// isOverageProvisioningAllowed())`. Because a default port session carries no
/// subscription snapshot and no live rate-limit status, `bnr()` is `false`, so
/// the command is `$te(c)`-filtered out of the default surface — matching a
/// fresh claude-code session. The DYNAMIC un-hide (when a subscription /
/// rate-limit status arrives) is applied by the palette builder via
/// `traits::subscription::SubscriptionSnapshot::is_usage_credits_command_enabled`;
/// only the default (off) state is modeled statically here.
pub const USAGE_CREDITS_BNR_GATED: &[&str] = &["usage-credits"];

/// `(command, aliases)` for the builtins that ship an `aliases:` array in
/// claude-code (cp-03). The slash palette folds these into the fuzzy candidate
/// set (so typing `/cost` finds `/usage`) and, when a row matched via a typed
/// alias, shows ` (<alias>)` after the name (`createCommandSuggestionItem`).
/// Ported from each `commands/<name>/index.ts` `aliases` literal.
pub const COMMAND_ALIASES: &[(&str, &[&str])] = &[
    // H-BIN-11 cc2.1.207: `name:"background",aliases:["bg"]`.
    ("background", &["bg"]),
    ("clear", &["reset", "new"]),
    ("config", &["settings"]),
    ("desktop", &["app"]),
    ("exit", &["quit"]),
    ("feedback", &["bug"]),
    ("mobile", &["ios", "android"]),
    ("permissions", &["allowed-tools"]),
    ("plugin", &["plugins", "marketplace"]),
    ("resume", &["continue"]),
    ("rewind", &["checkpoint", "undo"]),
    ("session", &["remote"]),
    ("tasks", &["bashes"]),
    ("usage", &["cost", "stats"]),
];

/// The aliases for `name` (empty when it has none). See [`COMMAND_ALIASES`].
#[must_use]
pub fn command_aliases(name: &str) -> &'static [&'static str] {
    COMMAND_ALIASES
        .iter()
        .find(|(cmd, _)| *cmd == name)
        .map_or(&[], |(_, aliases)| *aliases)
}

/// Returns `true` if `name` is hidden or disabled in claude-code's default
/// external build and therefore must NOT appear in the slash palette or the
/// `/help` screen — mirroring claude-code's
/// `commands.filter(c => !c.isHidden && !$te(c))` (where `$te` is the
/// "isEnabled() resolves to off" gate). The set is the union of:
///
/// - [`CORRECT_BY_DESIGN_STUBS`] — the 23 commands claude-code disables /
///   hides / feature-gates-OFF / ships as a compiled `name:'stub'`
///   (`isEnabled:()=>!1,isHidden:!0`) for ordinary users, so `$te(c)` is true
///   (or, for `advisor`/`brief`/`teleport`/`autofix-pr`, the entitlement /
///   statsig / remote gate is OFF by default, which also resolves `isHidden`
///   true and `isEnabled()` false); and
/// - [`HIDDEN_PALETTE_COMMANDS`] — the 4 enabled-but-hidden named commands
///   (`auto-mode-setup`, `extra-usage`, `heapdump`, `rate-limit-options`); and
/// - [`USAGE_CREDITS_BNR_GATED`] — the 1 `bnr()`-gated command
///   (`usage-credits`), off-by-default in a fresh no-subscription session.
///
/// Total = 28 filtered names. The host-bound `btw` command and the implemented
/// `/reload-plugins` command remain visible. Likewise `install-slack-app`,
/// `mobile`, and `desktop` carry no
/// default-off hidden gate (`desktop`'s `Dsl()` returns `true`) and stay
/// visible; and `background`, `cd`, `focus`, `tui` (the other four H-BIN-11
/// additions) are ungated and stay visible.
#[must_use]
pub fn is_palette_hidden(name: &str) -> bool {
    HIDDEN_PALETTE_COMMANDS.contains(&name)
        || USAGE_CREDITS_BNR_GATED.contains(&name)
        || CORRECT_BY_DESIGN_STUBS.iter().any(|(n, _)| *n == name)
}

/// `(command_name, DISABLE_*_COMMAND env var)` pairs whose command object in
/// claude-code v2.1.183 carries an `isEnabled:()=>!je.DISABLE_X_COMMAND`
/// truthiness gate (`je` is `process.env`). When the env var is set to any
/// non-empty value the command's `isEnabled()` resolves to `false`, so it is
/// dropped from the slash palette and `/help` (the same
/// `commands.filter(c => !c.isHidden && !$te(c))` path) **and** must not
/// resolve.
///
/// Verified verbatim against the shipped `claude.exe` v2.1.183 command objects:
///
/// ```text
/// name:"doctor",...,isEnabled:()=>!je.DISABLE_DOCTOR_COMMAND
/// name:"login",...,isEnabled:()=>!je.DISABLE_LOGIN_COMMAND
/// name:"logout",...,isEnabled:()=>!je.DISABLE_LOGOUT_COMMAND
/// name:"upgrade",...,isEnabled:()=>!kz()&&!je.DISABLE_UPGRADE_COMMAND&&sa()!=="enterprise"
/// name:"install-github-app",...,isEnabled:()=>!je.DISABLE_INSTALL_GITHUB_APP_COMMAND
/// ```
///
/// NOTE: claude-code also defines `DISABLE_BUG_COMMAND` and
/// `DISABLE_FEEDBACK_COMMAND`, but in v2.1.183 those gate the `/feedback`
/// **input handler** (`if(je.DISABLE_FEEDBACK_COMMAND)return{kind:"disabled"…}`),
/// not a command object's `isEnabled` — and there is no standalone `bug`
/// command — so they are intentionally NOT modeled here (LingXi's `feedback`
/// command object stays enabled, matching the binary). Likewise
/// `DISABLE_EXTRA_USAGE_COMMAND` does not appear on the `extra-usage` object
/// (`isEnabled:()=>pct()&&!kr()`), so it is excluded.
pub const ENV_DISABLE_GATED_COMMANDS: &[(&str, &str)] = &[
    ("doctor", "DISABLE_DOCTOR_COMMAND"),
    ("install-github-app", "DISABLE_INSTALL_GITHUB_APP_COMMAND"),
    ("login", "DISABLE_LOGIN_COMMAND"),
    ("logout", "DISABLE_LOGOUT_COMMAND"),
    ("upgrade", "DISABLE_UPGRADE_COMMAND"),
];

/// Returns `true` when `name`'s `DISABLE_*_COMMAND` env gate (see
/// [`ENV_DISABLE_GATED_COMMANDS`]) is tripped — i.e. the env var is present and
/// non-empty. Mirrors claude-code's `isEnabled:()=>!je.DISABLE_X` JS-truthiness
/// semantics: in JS `!process.env.X` is `false` (command disabled) for any
/// non-empty string value, including `"0"`/`"false"`, and `true` (enabled) only
/// when the var is unset or the empty string.
#[must_use]
pub fn is_command_env_disabled(name: &str) -> bool {
    ENV_DISABLE_GATED_COMMANDS
        .iter()
        .find(|(cmd, _)| *cmd == name)
        .is_some_and(|(_, var)| std::env::var(var).is_ok_and(|v| !v.is_empty()))
}

/// One-line descriptions for the builtin commands, used for slash-palette +
/// `/help` rendering. The first 18 are the M5-10 core set; the remainder
/// (cp-01) are the real per-command `description:` strings ported from each
/// claude-code `commands/<name>` object so no VISIBLE palette row shows the
/// `"(unimplemented in v0.6.0)"` placeholder. Commands that are hidden/disabled
/// (internal/dev/entitlement-gated) or LingXi-specific with no claude-code
/// analogue keep the placeholder fallback. Descriptions retain the "Claude
/// Code" product noun verbatim (1:1 with the oracle — the branding judgment
/// applies to screen titles, not command help strings).
#[must_use]
pub fn core_description(name: &str) -> &'static str {
    match name {
        // (M4 cc2.1.198) The /agents wizard was removed; the command now
        // returns static guidance. Description verbatim from the 2.1.198
        // binary command object (`name:"agents"`, description `(removed) …`).
        "agents" => "(removed) Ask Claude to create/manage subagents, or edit .lingxi/agents/",
        // WIZARD-06 (2.1.220). Byte-exact from the `type:"local"` object
        // (the non-interactive half; the `local-jsx` twin shares the name).
        "auto-mode-setup" => {
            "Set up and customise auto mode \u{2014} environment context, plus optional rule tweaks"
        }
        "clear" => "Start a new session with empty context; previous session stays on disk (resumable with /resume)",
        "compact" => "Free up context by summarizing the conversation so far",
        "config" => "Open settings",
        "doctor" => "Diagnose and verify your LingXi installation and settings",
        "exit" => "Exit the CLI",
        "help" => "Show help and available commands",
        "hooks" => "View hook configurations for tool events",
        "init" => "Initialize a new LINGXI.md file with codebase documentation",
        "login" => "Sign in with your Anthropic account",
        "logout" => "Sign out from your Anthropic account",
        "mcp" => "Manage MCP servers",
        "memory" => "Edit LINGXI.md files and memory settings",
        "model" => "Set the AI model for LingXi",
        "permissions" => "Manage allow and deny tool permission rules",
        "status" => "Show LingXi status including version, model, account, API connectivity, and tool statuses",
        // claude-code v2.1.183 live `usage` command object
        // (`name:"usage",aliases:["cost","stats"],...`).
        "usage" => "Show session cost, plan usage, and what's contributing to your limits",
        "version" => "Print version information",
        // (cp-01) Remaining visible builtins — real claude-code descriptions.
        "add-dir" => "Add a new working directory",
        "advisor" => "Let Claude consult a stronger model at key moments",
        // Net-new headless auto-compact-window reporter (see
        // `command_core::autocompact`). Verbatim from the 2.1.198 binary's
        // headless `type:"local"` autocompact command object.
        "autocompact" => "Configure the auto-compact window size",
        // (H-BIN-11 cc2.1.207) `local-jsx` command objects, descriptions
        // verbatim from the 2.1.207 binary.
        "background" => "Send this session to the background and free the terminal",
        "branch" => "Create a branch of the current conversation at this point",
        "bridge" => "Connect this terminal for remote-control sessions",
        "cd" => "Move this session to a new working directory",
        "btw" => "Ask a quick side question without interrupting the main conversation",
        "chrome" => "Open Claude in Chrome settings",
        "color" => "Set the prompt bar color for this session",
        "commit" => "Create a git commit",
        "commit-push-pr" => "Commit, push, and open a PR",
        "context" => "Visualize current context usage as a colored grid",
        "copy" => "Copy Claude's last response to clipboard (or /copy N for the Nth-latest)",
        "desktop" => "Continue the current session in Claude Desktop",
        "diff" => "View uncommitted changes and per-turn diffs",
        "effort" => "Set effort level for model usage",
        "export" => "Export the current conversation to a file or clipboard",
        "fast" => "Toggle fast mode",
        "feedback" => "Submit feedback about LingXi",
        "files" => "List all files currently in context",
        "focus" => "Toggle focus view: just your prompt, summary, and response",
        "ide" => "Manage IDE integrations and show status",
        "init-verifiers" => "Create verifier skill(s) for automated verification of code changes",
        "insights" => "Generate a report analyzing your LingXi sessions",
        "install" => "Install LingXi native build",
        "install-github-app" => "Set up Claude GitHub Actions for a repository",
        "install-slack-app" => "Install the Claude Slack app",
        "keybindings" => "Open your keyboard shortcuts file",
        "mobile" => "Show QR code to download the Claude mobile app",
        "passes" => "Share a free week of LingXi with friends and earn extra usage",
        "plan" => "Enable plan mode or view the current session plan",
        "plugin" => "Manage LingXi plugins",
        "privacy-settings" => "View and update your privacy settings",
        "release-notes" => "View release notes",
        "reload-plugins" => "Activate pending plugin changes in the current session",
        "remote-env" => "Configure the default remote environment for teleport sessions",
        "remote-setup" => "Setup LingXi on the web (requires connecting your GitHub account)",
        "rename" => "Rename the current conversation",
        "resume" => "Resume a previous conversation",
        "rewind" => "Restore the code and/or conversation to a previous point",
        "sandbox-toggle" => "Toggle sandbox mode for bash commands",
        "security-review" => "Complete a security review of the pending changes on the current branch",
        "session" => "Show remote session URL and QR code",
        "skills" => "List available skills",
        "statusline" => "Set up LingXi's status line UI",
        "stickers" => "Order LingXi stickers",
        "tasks" => "View and manage everything running in the background",
        "terminal-setup" => "Install Shift+Enter key binding for newlines",
        "theme" => "Change the theme",
        "tui" => "Set the terminal UI renderer (default | fullscreen)",
        "ultraplan" => "LingXi on the web drafts an advanced plan you can edit and approve",
        "upgrade" => "Upgrade to Max for higher rate limits and more Opus",
        "usage-credits" => {
            "Configure usage credits or request them from your admin when you hit a limit"
        }
        // Deprecated hidden alias of `/usage-credits`
        // (`name:"extra-usage",description:"Renamed to /usage-credits",isHidden:!0`).
        "extra-usage" => "Renamed to /usage-credits",
        "voice" => "Toggle voice mode",
        "workflows" => "Browse running and completed workflows",
        // Batch-8 implemented commands (real handlers in `command-core`); their
        // `description()` bodies carry the verbatim oracle strings, mirrored here
        // so the palette / `/help` rows never show the placeholder fallback.
        "fork" => "Copy this conversation into a new background session and keep working here",
        "goal" => "Set a goal — keep working until the condition is met",
        "recap" => "Generate a one-line session recap now",
        "reload-skills" => "Pick up skills added or changed on disk during this session",
        "skill-doctor" => "Show which loaded skills are unused and costing context",
        "stop" => "Stop this background session; transcript and worktree are kept",
        // cc2.1.238 `w$m` (`/subtask`, registered by `register_core_batch_8`
        // whenever agent view is on — the default). Verbatim command-object
        // description; without this row `/help` showed the placeholder.
        "subtask" => "Send a subagent off with your full context; its result comes back here",
        _ => "(unimplemented in v0.6.0)",
    }
}

/// Serializes every test that mutates or reads the `DISABLE_*_COMMAND` process
/// env: the env-gate tests below AND the sibling `help_render` render tests,
/// whose `render_help_screen()` consults those gates. Rust runs a crate's tests
/// in-process and in parallel, so without ONE shared lock a mutation here can
/// race a concurrent render in another module and intermittently drop a command
/// (e.g. `/login`) — the latent flake behind `every_visible_command_appears_once`
/// / `output_has_exactly_69_lines`. `pub(crate)` so help_render can lock it too.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_count_locked_at_107() {
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 107);
    }

    #[test]
    fn h_bin_11_names_present_and_gated() {
        // H-BIN-11 (cc2.1.207): five newly-registered names.
        for n in ["background", "cd", "focus", "tui", "usage-credits"] {
            assert!(
                BUILTIN_COMMAND_NAMES.contains(&n),
                "H-BIN-11 name '{n}' missing"
            );
        }
        // `/background` carries the `bg` alias.
        assert_eq!(command_aliases("background"), &["bg"]);
        // Four are visible; `/usage-credits` is bnr()-gated hidden-by-default.
        for visible in ["background", "cd", "focus", "tui"] {
            assert!(
                !is_palette_hidden(visible),
                "/{visible} must be visible in the default palette"
            );
        }
        assert!(
            is_palette_hidden("usage-credits"),
            "/usage-credits is bnr()-gated and hidden by default"
        );
        // Descriptions are the verbatim 2.1.207 command-object strings.
        assert_eq!(
            core_description("cd"),
            "Move this session to a new working directory"
        );
        assert_eq!(
            core_description("background"),
            "Send this session to the background and free the terminal"
        );
        assert_eq!(
            core_description("focus"),
            "Toggle focus view: just your prompt, summary, and response"
        );
        assert_eq!(
            core_description("tui"),
            "Set the terminal UI renderer (default | fullscreen)"
        );
        assert_eq!(
            core_description("usage-credits"),
            "Configure usage credits or request them from your admin when you hit a limit"
        );
    }

    /// cc2.1.238 byte-alignment: the one-in / one-out membership swap plus the
    /// six `/help` one-liners and the two changed descriptions, each verbatim
    /// from a 2.1.238 command object.
    #[test]
    fn cc_2_1_238_command_surface() {
        // `/review` was DELETED upstream (`name:"review"`: 220 = 1, 238 = 0);
        // `/subtask` takes its slot in the 107-name lock.
        assert!(
            !BUILTIN_COMMAND_NAMES.contains(&"review"),
            "/review was removed in claude-code 2.1.238"
        );
        assert!(
            BUILTIN_COMMAND_NAMES.contains(&"subtask"),
            "/subtask is registered by batch 8 and must be advertised"
        );
        assert!(!is_palette_hidden("subtask"));
        assert_eq!(
            core_description("subtask"),
            "Send a subagent off with your full context; its result comes back here"
        );

        // `/rewind` carries THREE names upstream:
        // `aliases:["checkpoint","undo"]` (238 and 220 alike).
        assert_eq!(command_aliases("rewind"), &["checkpoint", "undo"]);

        // 220 -> 238 description changes (LingXi brands CLAUDE.md as LINGXI.md).
        assert_eq!(
            core_description("memory"),
            "Edit LINGXI.md files and memory settings"
        );
        // Agent view is on by default, so `/fork` advertises `b$m`, the twin
        // `register_core_batch_8` actually registers.
        assert_eq!(
            core_description("fork"),
            "Copy this conversation into a new background session and keep working here"
        );

        // Six one-liners that matched NO oracle command object on any surface.
        for (name, want) in [
            ("hooks", "View hook configurations for tool events"),
            ("keybindings", "Open your keyboard shortcuts file"),
            ("tasks", "View and manage everything running in the background"),
            ("chrome", "Open Claude in Chrome settings"),
            ("advisor", "Let Claude consult a stronger model at key moments"),
            (
                "usage-credits",
                "Configure usage credits or request them from your admin when you hit a limit",
            ),
        ] {
            assert_eq!(core_description(name), want, "/{name} description drift");
        }
    }

    // ── #63 DISABLE_*_COMMAND env gates ──────────────────────────────────────
    // These tests mutate process env, so they serialize through the module-level
    // [`ENV_LOCK`] (shared with help_render) to avoid racing each other or any
    // concurrent env-reading render test in this binary.

    #[test]
    fn env_disable_gates_are_the_five_command_object_gated_names() {
        // Verified verbatim against claude.exe v2.1.183 command objects.
        let pairs: Vec<(&str, &str)> = ENV_DISABLE_GATED_COMMANDS.to_vec();
        assert_eq!(
            pairs,
            vec![
                ("doctor", "DISABLE_DOCTOR_COMMAND"),
                ("install-github-app", "DISABLE_INSTALL_GITHUB_APP_COMMAND"),
                ("login", "DISABLE_LOGIN_COMMAND"),
                ("logout", "DISABLE_LOGOUT_COMMAND"),
                ("upgrade", "DISABLE_UPGRADE_COMMAND"),
            ]
        );
        // Every gated name is a real builtin.
        let full: std::collections::HashSet<&str> = BUILTIN_COMMAND_NAMES.iter().copied().collect();
        for (cmd, _) in ENV_DISABLE_GATED_COMMANDS {
            assert!(full.contains(cmd), "gated name '{cmd}' is not a builtin");
        }
    }

    #[test]
    fn ungated_command_is_never_env_disabled() {
        let _g = ENV_LOCK.lock().unwrap();
        // `clear` has no DISABLE gate, so the helper is always false for it.
        assert!(!is_command_env_disabled("clear"));
        assert!(!is_command_env_disabled("feedback"));
    }

    #[test]
    fn gated_command_disabled_only_when_env_set_nonempty() {
        let _g = ENV_LOCK.lock().unwrap();
        let var = "DISABLE_DOCTOR_COMMAND";
        // Default (unset): enabled.
        std::env::remove_var(var);
        assert!(!is_command_env_disabled("doctor"));
        // Empty string: still enabled (JS `!""` is true → command stays on).
        std::env::set_var(var, "");
        assert!(!is_command_env_disabled("doctor"));
        // Any non-empty value: disabled (JS truthiness — even "0"/"false").
        std::env::set_var(var, "1");
        assert!(is_command_env_disabled("doctor"));
        std::env::set_var(var, "0");
        assert!(is_command_env_disabled("doctor"));
        std::env::remove_var(var);
        assert!(!is_command_env_disabled("doctor"));
    }

    #[test]
    fn env_disabled_command_is_dropped_from_help_render() {
        use crate::builtin_support::help_render::render_help_screen;
        let _g = ENV_LOCK.lock().unwrap();
        let var = "DISABLE_LOGIN_COMMAND";
        std::env::remove_var(var);
        let before = render_help_screen();
        assert!(before.contains("  /login "), "login visible by default");
        std::env::set_var(var, "1");
        let after = render_help_screen();
        assert!(
            !after.contains("  /login "),
            "login must be dropped from /help when DISABLE_LOGIN_COMMAND is set"
        );
        assert_eq!(
            after.matches('\n').count(),
            before.matches('\n').count() - 1,
            "exactly one command (login) is removed"
        );
        std::env::remove_var(var);
    }

    #[test]
    fn core_count_locked_at_19() {
        assert_eq!(BUILTIN_CORE_NAMES.len(), 19);
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
        assert!(!BUILTIN_COMMAND_NAMES.contains(&"x402"));
        assert!(BUILTIN_COMMAND_NAMES.contains(&"ctx-viz"));
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
    // no change to the locked command surface (`parity_slash_commands_102`).
    // ========================================================================

    #[test]
    fn intentionally_disabled_count_is_23() {
        assert_eq!(
            INTENTIONALLY_DISABLED_COMMANDS.len(),
            23,
            "disabled command classification is locked at 23 commands"
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
        assert!(table.contains_key("brief"));
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
        // subset of the locked name list and therefore cannot change the
        // total count, membership, or ordering that the parity fixture locks.
        assert!(INTENTIONALLY_DISABLED_COMMANDS.len() < BUILTIN_COMMAND_NAMES.len());
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 107);
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
    fn host_bound_deferred_set_is_empty() {
        assert!(HOST_BOUND_DEFERRED_GAPS.is_empty());
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
            "all disabled commands are correct-by-design"
        );
        assert_eq!(BUILTIN_COMMAND_NAMES.len(), 107);
    }

    #[test]
    fn btw_is_not_classified_as_disabled_or_deferred() {
        let disabled = name_set(INTENTIONALLY_DISABLED_COMMANDS);
        let gaps = name_set(HOST_BOUND_DEFERRED_GAPS);
        assert!(!disabled.contains("btw"));
        assert!(!gaps.contains("btw"));
    }
}
