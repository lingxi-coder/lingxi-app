//! `PermissionPolicy::authorize` — rule-driven decision + mode fallback.
//!
//! Classifiers and shadow detection are wired in Plan 03 (Tools System).
//! M1.3 evaluates rules in priority order (deny first, then allow) and
//! falls back to the active mode for unmatched calls.

use crate::auto_edit_safety::{check_path_safety_for_auto_edit, AutoEditSafety};
use crate::defaults_per_tool::tool_default;
use crate::denial_tracking::DenialTrackingState;
use crate::filesystem::{
    file_tool_kind, input_path_for_tool, path_in_allowed_working_path, path_matches_rule_pattern,
    FileToolKind, FsRoots,
};
use crate::gate::PromptDefault;
use crate::mode::PermissionMode;
use crate::result::{
    PermissionDecisionReason, PermissionMetadata, PermissionPrompt, PermissionResult,
};
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleSource};
use crate::shell_command;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Base commands a Bash invocation may auto-allow in `AcceptEdits` mode — 1:1
/// with claude-code `ACCEPT_EDITS_ALLOWED_COMMANDS` (`BashTool/modeValidation.ts:7-15`).
/// `sed` is included here but is FURTHER gated by the sed auto-allow verdict
/// (`sedValidation.ts`): it auto-allows only when [`crate::sed_validation::SedVerdict::Safe`].
const ACCEPT_EDITS_ALLOWED_COMMANDS: [&str; 7] =
    ["mkdir", "touch", "rm", "rmdir", "mv", "cp", "sed"];

/// Rule sources in citation-walk order, byte-locked to claude-code's `Szn`
/// (`[...Tw,"cliArg","command","session",…]` with `Tw=["userSettings",
/// "projectSettings","localSettings","flagSettings","policySettings"]`) — #35.
/// `first_match` walks each behavior bucket in this order and cites the FIRST
/// matching rule. This is CITATION precedence only: the deny-wins outcome is
/// behavior-first (`authorize_inner` checks the whole deny bucket before ask
/// before allow), so the source order never changes the allow/deny DECISION —
/// only which source's rule is reported. (Was the exact reverse of this.)
const SOURCES_BY_PRIORITY: [PermissionRuleSource; 8] = [
    PermissionRuleSource::UserSettings,
    PermissionRuleSource::ProjectSettings,
    PermissionRuleSource::LocalSettings,
    PermissionRuleSource::FlagSettings,
    PermissionRuleSource::PolicySettings,
    PermissionRuleSource::CliArg,
    PermissionRuleSource::Command,
    PermissionRuleSource::Session,
];

/// Rule-driven authorization policy.
///
/// Holds three rule buckets (allow / deny / ask) keyed by source, the active
/// mode, denial-tracking state (populated in Plan 03), and a flag that gates
/// `BypassPermissions` to defend against rogue automation.
pub struct PermissionPolicy {
    /// Active mode (drives fallback when no rule matches).
    pub mode: PermissionMode,
    /// Allow rules grouped by source.
    pub allow_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Deny rules grouped by source.
    pub deny_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Ask rules grouped by source.
    pub ask_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    /// Consecutive-denial counters (Plan 03 populates this).
    pub denial_tracking: Mutex<DenialTrackingState>,
    /// Killswitch that overrides `BypassPermissions` back to `Ask`.
    pub bypass_killswitch_active: bool,
    /// Was the session ORIGINALLY started with `BypassPermissions` available?
    /// 1:1 with TS `ToolPermissionContext.isBypassPermissionsModeAvailable`.
    /// When `true`, `Plan` mode ALSO bypasses permissions (claude-code
    /// `permissions.ts:1268-1271` `shouldBypassPermissions`) — a plan started
    /// from a bypass session keeps the bypass grant. Defaults to `false`
    /// (preserving the plan-mode mutation backstop); production engine wiring of
    /// this flag is deferred this batch. Subject to the same
    /// [`Self::bypass_killswitch_active`] override as `BypassPermissions`.
    pub bypass_permissions_available: bool,
    /// Filesystem roots for per-tool file-path CONTENT matching (phase 3a).
    /// `None` preserves the phase-2 tool-wide behavior (content ignored,
    /// matched by exact tool name); production sets this via [`Self::with_roots`]
    /// so `Edit(src/**)` / `Read(./secrets/**)` match the input path.
    pub roots: Option<FsRoots>,
    /// Allow rules stripped out on entry to [`PermissionMode::Auto`] because
    /// they would bypass the auto-mode classifier (`Bash(python:*)`, `Agent(*)`,
    /// `PowerShell(iex:*)`, …). Stashed here by
    /// [`Self::strip_dangerous_for_auto`] and re-added verbatim by
    /// [`Self::restore_dangerous`] when leaving Auto. Empty in every other mode.
    /// 1:1 with the TS `strippedDangerousRules` field on `ToolPermissionContext`.
    pub stripped_dangerous: Vec<PermissionRule>,
    /// Per-stripped-rule `(source, original_index_within_source_bucket)`, kept
    /// parallel to [`Self::stripped_dangerous`] so [`Self::restore_dangerous`]
    /// re-inserts each rule at its original position — making `strip → restore`
    /// an EXACT identity (TS only guarantees set-equality via `applyPermissionUpdate`
    /// re-add; the position record is a Rust-side strengthening, documented).
    stripped_positions: Vec<(PermissionRuleSource, usize)>,
    /// Extra directories (beyond `roots.cwd`) inside which `AcceptEdits` mode
    /// auto-allows safe file edits. 1:1 with the keys of the TS
    /// `ToolPermissionContext.additionalWorkingDirectories` map, which
    /// `allWorkingDirectories` (`filesystem.ts:667-674`) unions with the original
    /// cwd. Empty by default; set via [`Self::with_working_dirs`]. Production
    /// engine wiring is deferred this batch (cwd comes from `roots.cwd`).
    pub additional_working_dirs: Vec<PathBuf>,
    /// Minimal sandbox-runtime config for the bash sandbox-auto-allow layer
    /// (`bashToolHasPermission`'s `isSandboxingEnabled() &&
    /// isAutoAllowBashIfSandboxedEnabled() && shouldUseSandbox(input)` branch).
    /// `None` (the DEFAULT) makes the sandbox-auto-allow layer a no-op, so
    /// `authorize` behaves exactly as before when absent — preserving the
    /// opt-in posture. Set via [`Self::with_sandbox_runtime`]; the `permission`
    /// crate cannot depend on the `sandbox` crate (cycle), so this carries only
    /// the three fields the auto-allow branch reads
    /// ([`crate::sandbox_auto_allow::SandboxAutoAllowConfig`]). Production
    /// engine wiring of this field is reported as a follow-up this batch (the
    /// boot site currently constructs a disabled-default `SandboxRuntimeConfig`).
    pub sandbox_runtime: Option<crate::sandbox_auto_allow::SandboxAutoAllowConfig>,
}

impl PermissionPolicy {
    /// Build a fresh policy with no rules and the given mode.
    #[must_use]
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            mode,
            allow_rules: HashMap::new(),
            deny_rules: HashMap::new(),
            ask_rules: HashMap::new(),
            denial_tracking: Mutex::new(DenialTrackingState::default()),
            bypass_killswitch_active: false,
            bypass_permissions_available: false,
            roots: None,
            stripped_dangerous: Vec::new(),
            stripped_positions: Vec::new(),
            additional_working_dirs: Vec::new(),
            sandbox_runtime: None,
        }
    }

    /// Attach the minimal sandbox-runtime config that enables the bash
    /// sandbox-auto-allow layer (`bashToolHasPermission`'s sandbox branch). When
    /// absent (the default) the layer is a no-op. Populate at the engine boot
    /// site from the real `sandbox::runtime_config::SandboxRuntimeConfig`
    /// (copying its `enabled`, `auto_allow_bash_if_sandboxed`, and
    /// `excluded_commands` into
    /// [`crate::sandbox_auto_allow::SandboxAutoAllowConfig`]).
    #[must_use]
    pub fn with_sandbox_runtime(
        mut self,
        config: crate::sandbox_auto_allow::SandboxAutoAllowConfig,
    ) -> Self {
        self.sandbox_runtime = Some(config);
        self
    }

    /// Set the extra working directories inside which `AcceptEdits` mode
    /// auto-allows safe edits (beyond `roots.cwd`). Mirrors seeding the keys of
    /// TS `ToolPermissionContext.additionalWorkingDirectories`. Backward-compatible
    /// (default empty); production may leave it unset this batch.
    #[must_use]
    pub fn with_working_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.additional_working_dirs = dirs;
        self
    }

    /// Mark whether `BypassPermissions` mode was available at session start
    /// (TS `isBypassPermissionsModeAvailable`). When `true`, `Plan` mode bypasses
    /// permissions like `BypassPermissions` (see
    /// [`Self::bypass_permissions_available`]). Default `false`.
    #[must_use]
    pub fn with_bypass_available(mut self, available: bool) -> Self {
        self.bypass_permissions_available = available;
        self
    }

    /// Enable phase-3a file-path content matching by supplying the filesystem
    /// roots a rule's [`PermissionRuleSource`] resolves against. Without this,
    /// content rules for file tools fall back to phase-2 tool-wide matching.
    #[must_use]
    pub fn with_roots(mut self, roots: FsRoots) -> Self {
        // Relative-path matching is meaningless against a relative cwd: every
        // absolute tool path would resolve `../…` and silently stop matching.
        // Production always supplies an absolute cwd (`std::env::current_dir`).
        debug_assert!(
            roots.cwd.is_absolute(),
            "FsRoots.cwd must be absolute for correct file-path matching"
        );
        self.roots = Some(roots);
        self
    }

    /// Build a policy with `mode`, bucketing `rules` into the allow/deny/ask
    /// maps keyed by source. The foundation for enforcement: the loaded rules
    /// (e.g. from [`crate::loader::permission_rules_from_settings_json`]) land
    /// in the same buckets [`Self::authorize`] already evaluates. Wiring a gate
    /// over the resulting policy is a later phase.
    #[must_use]
    pub fn from_rules(
        mode: PermissionMode,
        rules: impl IntoIterator<Item = PermissionRule>,
    ) -> Self {
        let mut policy = Self::new(PermissionMode::Default);
        for rule in rules {
            let bucket = match rule.behavior {
                PermissionBehavior::Allow => &mut policy.allow_rules,
                PermissionBehavior::Deny => &mut policy.deny_rules,
                PermissionBehavior::Ask => &mut policy.ask_rules,
            };
            bucket.entry(rule.source).or_default().push(rule);
        }
        policy.set_mode(mode);
        policy
    }

    /// Tool-name targets of every TOOL-WIDE deny rule (`rule_content == None`)
    /// across all sources — the rule names that BLANKET-deny a tool. Used by the
    /// orchestrator's wire-tool filter to strip denied tools BEFORE the model
    /// sees them, 1:1 with claude-code `filterToolsByDenyRules` /
    /// `getDenyRuleForTool` (`tools.ts:262-269`, `permissions.ts:287-292`), which
    /// drops a tool when a deny rule with NO `ruleContent` matches its name via
    /// [`tool_wide_name_matches`]. CONTENT deny rules (e.g. `Bash(rm:*)`,
    /// `WebFetch(domain:x)`) are EXCLUDED — they deny specific calls, not the
    /// whole tool, so the tool stays advertised (matching TS, where
    /// `toolMatchesRule` returns `false` when `ruleContent !== undefined`).
    ///
    /// Returns the raw rule tool-name strings (which may be a bare tool name like
    /// `"WebFetch"` OR an MCP server prefix like `"mcp__github"`); the caller
    /// matches each against an advertised tool's name with
    /// [`tool_wide_name_matches`]. Order follows source-bucket iteration; the
    /// caller only tests membership, so duplicates are harmless.
    #[must_use]
    pub fn tool_wide_deny_names(&self) -> Vec<String> {
        self.deny_rules
            .values()
            .flat_map(|rules| rules.iter())
            .filter(|r| r.value.rule_content.is_none())
            .map(|r| r.value.tool_name.clone())
            .collect()
    }

    /// The source of the highest-priority DENY rule that denies `Agent(<type>)`,
    /// or `None` if no such rule exists.
    ///
    /// 1:1 with claude-code `getDenyRuleForAgent` (`o5e(ctx, "Agent", type)`): a
    /// CONTENT deny rule whose `tool_name == "Agent"` and whose `rule_content`
    /// equals `agent_type` exactly. When several sources match, the one with the
    /// highest citation [`PermissionRuleSource::priority`] is returned (claude
    /// cites the first source in its walk). The deny rule keys on the `"Agent"`
    /// tool name even when the call arrives via the legacy `Task` alias.
    #[must_use]
    pub fn agent_type_deny_source(&self, agent_type: &str) -> Option<PermissionRuleSource> {
        self.deny_rules
            .values()
            .flat_map(|rules| rules.iter())
            .filter(|r| {
                (r.value.tool_name == "Agent" || r.value.tool_name == "Task")
                    && r.value.rule_content.as_deref() == Some(agent_type)
            })
            .map(|r| r.source)
            .max_by_key(|s| s.priority())
    }

    /// The set of agent-type names denied by a CONTENT-ful `Agent(<x>)` deny rule
    /// — the listing-filter set (claude-code `Pxe`). Order follows source-bucket
    /// iteration; callers test membership, so duplicates are harmless.
    #[must_use]
    pub fn agent_deny_content_types(&self) -> Vec<String> {
        self.deny_rules
            .values()
            .flat_map(|rules| rules.iter())
            .filter(|r| {
                (r.value.tool_name == "Agent" || r.value.tool_name == "Task")
                    && r.value.rule_content.is_some()
            })
            .filter_map(|r| r.value.rule_content.clone())
            .collect()
    }

    /// Resolve a tool call to a [`PermissionResult`].
    ///
    /// Evaluation order (claude-code `checkPermissionsForToolUse` skeleton):
    /// 1. Deny rules, walked from highest to lowest source priority.
    /// 2. Ask rules, same order (NEW — a matching ask rule forces a prompt).
    /// 3. Allow rules, same order.
    /// 4. Mode fallback (`Default`/`Plan`/`AcceptEdits` ask the user,
    ///    `BypassPermissions` allows unless the killswitch is set, `DontAsk`
    ///    denies).
    ///
    /// Rule matching ([`Self::rule_matches`]): TOOL-WIDE rules match by exact
    /// tool name (claude-code `toolMatchesRule`); CONTENT rules for FILE tools
    /// match the input's path via [`crate::filesystem`] grouping (phase 3a);
    /// CONTENT rules for SHELL tools (`Bash`/`PowerShell`) match the command
    /// per [`crate::shell_command`] — deny/ask match if ANY subcommand matches
    /// (aggressive wrapper/env stripping), allow requires EVERY subcommand
    /// covered (3a-bash). Other NON-file content rules stay tool-wide
    /// (`WebFetch` domain matching is a separate deferral). Shell + file content
    /// matching is active only when [`Self::roots`] is set (production always
    /// sets it); without roots the phase-2 tool-wide behavior is preserved.
    ///
    /// The wider `checkRead/checkWritePermissionForTool` allowances
    /// (working-directory auto-allow, `.git`/`.claude` safety asks, path/sed/
    /// mode constraints) are still NOT modeled — `Read`'s mode-ask is
    /// auto-allowed by `PolicyPermissionGate` (read-only default).
    #[must_use]
    pub fn authorize(&self, tool_name: &str, input: &serde_json::Value) -> PermissionResult {
        self.authorize_with_mode(tool_name, input, self.mode)
    }

    /// Like [`Self::authorize`], but evaluates the mode-driven layers (the
    /// `DontAsk` ask→deny transform, the `AcceptEdits` auto-allows, the `Plan`
    /// mutation backstop / bypass, and the generic mode fallback) against an
    /// EXPLICIT `mode` instead of the policy's boot [`Self::mode`].
    ///
    /// This is the seam the gate uses to apply a DYNAMIC mode — e.g. authorize
    /// under [`PermissionMode::Plan`] once the session has run `EnterPlanMode`.
    /// claude-code reads `toolPermissionContext.mode` live on every check; LingXi
    /// builds [`PermissionPolicy`] once at boot with a fixed mode and the gate
    /// holds it behind a shared `Arc` (so it can neither rebuild it nor call the
    /// `&mut` [`Self::set_mode`]). Threading the mode here lets the gate honor the
    /// live session mode without touching the rule buckets, roots, sandbox config,
    /// or working dirs — only the mode-driven layers see `mode`. `authorize` is
    /// exactly `authorize_with_mode(.., self.mode)`, so the rule-only behavior is
    /// unchanged.
    #[must_use]
    pub fn authorize_with_mode(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        mode: PermissionMode,
    ) -> PermissionResult {
        let result = self.authorize_inner(tool_name, input, mode);
        // PERM.1 — DontAsk transform (claude-code `permissions.ts:503-517`):
        // applied LAST so no early-return ask escapes it. A remaining `ask`
        // becomes `deny`, EXCEPT for read-only / `AllowByDefault` tools — in TS
        // their own `checkPermissions` returns `allow` BEFORE this transform, so
        // they are never over-denied. Here the surviving `ask` is left for the
        // gate's read-only default ([`crate::policy_gate`]) to auto-allow.
        if mode == PermissionMode::DontAsk
            && matches!(result, PermissionResult::Ask { .. })
            && !matches!(tool_default(tool_name), PromptDefault::AllowByDefault)
        {
            return deny_with_mode(PermissionMode::DontAsk);
        }
        result
    }

    /// Rule + mode evaluation producing the pre-`DontAsk`-transform result.
    /// See [`Self::authorize_with_mode`] for the public contract and the
    /// evaluation order; that wrapper applies the `DontAsk` ask→deny transform.
    ///
    /// `mode` is the EFFECTIVE mode to evaluate against — the policy's boot
    /// [`Self::mode`] for [`Self::authorize`], or a caller-supplied mode (e.g.
    /// [`PermissionMode::Plan`] for the gate's live plan-mode path). Every
    /// mode-driven layer below reads this parameter, NOT `self.mode`, so the
    /// whole mode-fallback chain honors the effective mode.
    // This is the central precedence dispatcher; it grows by one short branch
    // per faithfully-ported claude-code gate layer (the 2c bash-safety branch
    // tipped it one line past the pedantic 100-line cap). Each layer is already
    // a thin call into a dedicated helper; further splitting the ordered walk
    // would obscure the 1:1 TS precedence it documents.
    #[allow(clippy::too_many_lines)]
    fn authorize_inner(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        mode: PermissionMode,
    ) -> PermissionResult {
        let sources = SOURCES_BY_PRIORITY;

        // Precedence mirrors claude-code's real decision fn `mSm`
        // (offset ~205931956). The KEY invariant (R-D1): the ENTIRE deny phase
        // (tool-wide + content) runs before ANY ask, so a `deny` rule can never be
        // downgraded to an `ask`. The mSm order is:
        //   1a tool-wide deny  (`B3t` → `SIo`, ruleContent===void 0)
        //   1b content deny    (`K5t(...,"deny")`)
        //   1c tool-wide ask   (`EIo` → `SIo`, ruleContent===void 0)
        //   → e.checkPermissions(...) runs; ONLY a `deny` verdict short-circuits
        //     (`if(l?.behavior==="deny")return l`)
        //   1d content ask     (`K5t(...,"ask")`)  — mSm step 5
        //   → the checkPermissions ASK verdicts (dangerous rm/path/safety) are
        //     returned (mSm step 6), then its ALLOW verdicts (sandbox auto-allow,
        //     read-only, exact-allow) via the allow walk / mode.
        // Because LingXi's per-tool guards below (sandbox-auto-allow,
        // dangerous-removal, path-constraint, exact-allow, bash-safety) emit only
        // ALLOW or ASK verdicts (never `deny` — deny rules are the explicit walks
        // here), and content ask BEATS a checkPermissions allow/ask (mSm step 5
        // precedes step 6/7), content ask must sit BEFORE that guard block — i.e.
        // right after the deny phase + tool-wide ask. (Locked by
        // `read_only_ask_rule_still_asks` + `sandbox_auto_allow_ask_rule_still_asks`,
        // where a content ask wins over the read-only / sandbox auto-allow.)
        // 1a. Tool-wide deny.
        if let Some(rule) = self.first_match(&self.deny_rules, &sources, tool_name, input, false) {
            return deny_with_rule(rule);
        }
        // 1b. Content deny — runs as part of the deny phase, BEFORE any ask, so
        //     a `deny:[Bash(rm:*)]` is honored even when `ask:[Bash]` is also set
        //     (mSm: `K5t(...,"deny")` precedes the tool-wide ask `EIo`). This is
        //     the R-D1 fix: previously tool-wide ask walked before content deny,
        //     downgrading a deny to an ask.
        if let Some(rule) = self.first_match(&self.deny_rules, &sources, tool_name, input, true) {
            return deny_with_rule_content(rule, tool_name, input);
        }
        // 1c. Tool-wide ask (`EIo`).
        if let Some(rule) = self.first_match(&self.ask_rules, &sources, tool_name, input, false) {
            return ask_with_rule(rule, tool_name);
        }
        // 1d. Content ask (`K5t(...,"ask")`, mSm step 5) — a matching content ask
        //     rule prompts. Placed AFTER the deny phase but BEFORE the per-tool
        //     guards below, because content ask BEATS the guards' allow/ask
        //     verdicts (sandbox auto-allow, read-only allow, exact-allow). The
        //     gate's read-only default may still auto-allow, but the rule is
        //     honored.
        if let Some(rule) = self.first_match(&self.ask_rules, &sources, tool_name, input, true) {
            return ask_with_rule(rule, tool_name);
        }
        // 1d. SANDBOX AUTO-ALLOW (claude-code `bashToolHasPermission`'s
        //     sandbox branch, `bashPermissions.ts:1829-1843` + `checkSandboxAutoAllow`).
        //     When sandboxing is enabled AND `autoAllowBashIfSandboxed` (default
        //     true) AND the command WOULD be sandboxed (`shouldUseSandbox`), a
        //     command that matched NO explicit deny/ask rule is auto-allowed —
        //     the sandbox is the safety boundary, not the prompt. ORDER: this
        //     runs AFTER the deny/ask walks (so explicit deny/ask rules still
        //     win — TS `checkSandboxAutoAllow` itself re-checks deny/ask on the
        //     full command + every subcommand before allowing; here those rules
        //     already short-circuited above) and BEFORE the path-constraint /
        //     dangerous-removal guards (1:1 with TS, where the sandbox branch
        //     precedes `bashToolCheckPermission`'s path-constraint step). Gated
        //     on [`Self::sandbox_runtime`]: `None` (the default) ⇒ no-op, so
        //     behavior is unchanged when absent.
        if self.shell_sandbox_auto_allows(tool_name, input) {
            return allow_sandbox_auto();
        }
        // 2. Dangerous-removal-path guard (claude-code `checkDangerousRemovalPaths`
        //    via `createPathChecker`, `BashTool/pathValidation.ts:728-737`). An
        //    `rm`/`rmdir` whose target resolves to a critical system path (`/`,
        //    `/etc`, the home dir, a trailing `/*` glob, …) ALWAYS asks — and this
        //    must OVERRIDE a matching allow rule (`Bash(rm:*)`), matching the TS
        //    note that the operation "cannot be auto-allowed by permission rules".
        //    Placed AFTER the deny/ask walks (explicit deny/ask rules still win,
        //    mirroring TS where an explicit deny short-circuits the check) and
        //    BEFORE the allow walk so it pre-empts any allow grant. Requires
        //    [`Self::roots`] (cwd + home); without roots the guard is skipped
        //    (preserves pre-guard behavior), consistent with shell content
        //    matching being roots-gated.
        if let Some(roots) = self.roots.as_ref() {
            if shell_command::is_shell_tool(tool_name) {
                if let Some(command) = shell_command::command_from_input(input) {
                    let home = roots
                        .home
                        .as_deref()
                        .map(|p| p.to_string_lossy().into_owned());
                    if let Some(danger) = crate::dangerous_removal::check_dangerous_removal(
                        command,
                        &roots.cwd,
                        home.as_deref(),
                    ) {
                        return ask_dangerous_removal(tool_name, danger);
                    }
                    // 2b. Bash path-constraint guard (claude-code `checkPathConstraints`,
                    //     `BashTool/pathValidation.ts:1013`). A bash command that writes
                    //     (output redirection), `cd`s, or uses process substitution to
                    //     touch a path OUTSIDE the allowed working dirs (cwd +
                    //     `additional_working_dirs`) ALWAYS asks — even past a matching
                    //     allow rule, matching the TS `behavior: 'ask'` return. Shares the
                    //     dangerous-removal slot (after deny/ask walks, before the allow
                    //     walk, roots- + shell-gated). The `astCommands` branch is dropped
                    //     in favor of the `split_command` path (documented in
                    //     `path_constraints`).
                    if let Some(ask) = crate::path_constraints::check_path_constraints(
                        command,
                        roots,
                        &self.additional_working_dirs,
                    ) {
                        return ask_path_constraint(tool_name, ask);
                    }
                    // 2b'. Per-command PATH CONTAINMENT (claude-code
                    //      `validateCommandPaths` + `PATH_EXTRACTORS`, run per
                    //      subcommand by `checkPathConstraints`,
                    //      `BashTool/pathValidation.ts:603/190-552`, wired at
                    //      `bashPermissions.ts:1106-1122`). Sibling of the
                    //      redirection/`cd` guard above: it covers the POSITIONAL
                    //      FILE ARGUMENTS of ~31 path-taking commands
                    //      (cat/head/grep/find/mv/cp/touch/sed/`git diff
                    //      --no-index`/…), so `cat /etc/passwd` (cwd `/proj/work`)
                    //      ASKS even past a matching `Bash(cat:*)` allow rule —
                    //      and past an EXACT `Bash(cat /etc/passwd)` rule, since
                    //      this runs BEFORE the `shell_exact_allow` short-circuit
                    //      (2c-exact) and the allow walk (step 3), exactly as TS
                    //      runs `validateCommandPaths` (step 3) ahead of the
                    //      exact-match-allow (step 4) and prefix-allow (step 5).
                    //      Shares this slot (after deny/ask, before exact/allow,
                    //      roots- + shell-gated) and the byte-locked ask path.
                    if let Some(ask) =
                        crate::command_path_containment::check_command_path_containment(
                            command,
                            roots,
                            &self.additional_working_dirs,
                        )
                    {
                        return ask_path_constraint(tool_name, ask);
                    }
                }
            }
        }
        // 2c. BASH COMMAND-INJECTION SAFETY (claude-code `bashCommandIsSafe`,
        //     `bashSecurity.ts`'s legacy `bashCommandIsSafe_DEPRECATED` battery,
        //     wired at `bashPermissions.ts:1217-1239` inside
        //     `checkCommandAndSuggestRules`). In the external build tree-sitter is
        //     OFF, so `astParseSucceeded` is false and this validator chain ALWAYS
        //     runs. It scans the full command with ~23 validators (substitution,
        //     IFS injection, ANSI-C/locale quoting, backtick/`$()`, CR/newline,
        //     unicode whitespace, brace expansion, zsh `zmodload`,
        //     comment-quote-desync, …) and ASKS on the FIRST detection.
        //
        //     PRECEDENCE (1:1 with TS `checkCommandAndSuggestRules`): explicit
        //     DENY and ASK rules already short-circuited above (TS step 2a). The
        //     safety check runs BEFORE the allow walk below (TS step 4) — so an
        //     explicit ALLOW rule does NOT override a safety-ask (TS runs
        //     `bashCommandIsSafe` at step 3, ahead of the allow return at step 4).
        //     Placed AFTER the sandbox-auto-allow (1d) and the dangerous-removal /
        //     path-constraint guards (2/2b) — a sandbox-auto-allowed command never
        //     reaches the per-subcommand safety inner in TS.
        //
        //     The check is shell-tool only. Tagged
        //     [`PermissionDecisionReason::SafetyCheck`] (TS `type: 'other'`
        //     carrying the validator message; `classifier_approvable` true since
        //     the TS flow attaches a pending classifier check). SAFETY: this can
        //     only make the gate STRICTER (more asks); it never downgrades a deny
        //     and never touches non-shell tools. PER-SUBCOMMAND (1:1 with TS):
        //     the split + redirect-strip + battery lives in
        //     [`Self::shell_bash_safety_ask`].
        //
        // 2c-exact. EXACT-MATCH ALLOW SHORT-CIRCUIT (claude-code
        //     `bashToolCheckExactMatchPermission`, wired as step 1 of
        //     `checkCommandAndSuggestRules`, `bashPermissions.ts:1190-1197`). An
        //     EXACT allow rule — the WHOLE trimmed command equals the rule content
        //     (an `Bash(cmd)` rule, or a `Bash(prefix:*)` rule whose bare prefix
        //     equals the full command) — short-circuits to ALLOW at the very top
        //     of the TS flow, BEFORE the command-injection safety battery (TS step
        //     3). So a command the user EXPLICITLY allowed 1:1 is allowed without a
        //     safety re-ask. This is the EXACT case ONLY: a PREFIX allow rule does
        //     NOT bypass safety (TS returns the prefix-allow at step 4, AFTER the
        //     step-3 safety check), so the 2c safety layer below stays AHEAD of the
        //     `shell_allow` prefix/aggregation walk. Deny/ask (exact or prefix) and
        //     the dangerous-removal / path-constraint guards already short-circuited
        //     above, matching TS where exact-deny/ask (step 1 of
        //     `bashToolCheckExactMatchPermission`) precede the exact-allow return.
        //     Shell-tool + roots-gated, consistent with the rest of the shell
        //     content matching. Reuses [`shell_command::command_exact_allowed`]
        //     (the `matchMode: 'exact'` arm of `filterRulesByContentsMatchingInput`).
        if self.roots.is_some() && shell_command::is_shell_tool(tool_name) {
            if let Some(rule) = self.shell_exact_allow(tool_name, input, &sources, mode) {
                return allow_with_rule(rule);
            }
        }
        if let Some(ask) = Self::shell_bash_safety_ask(tool_name, input) {
            return ask;
        }
        // 3. Allow. Shell tools need compound aggregation (a single allow rule
        //    matching ONE subcommand must not allow a whole compound command),
        //    so they take a dedicated path rather than the per-rule walk.
        if self.roots.is_some() && shell_command::is_shell_tool(tool_name) {
            if let Some(rule) = self.shell_allow(tool_name, input, &sources, mode) {
                return allow_with_rule(rule);
            }
        } else {
            for src in &sources {
                if let Some(rules) = self.allow_rules.get(src) {
                    if let Some(rule) = rules.iter().find(|r| {
                        self.rule_is_available_in_mode(r, mode)
                            && self.rule_matches(r, tool_name, input)
                    }) {
                        return allow_with_rule(rule);
                    }
                }
            }
        }
        // 3-sed. SED CONSTRAINTS (claude-code `checkSedConstraints`, TS step 5b,
        //     `bashPermissions.ts:1142-1146`). Runs in EVERY mode (not just
        //     `AcceptEdits`): each `sed` subcommand is checked against the sed
        //     allowlist — with file-writes permitted only in `AcceptEdits` — and
        //     a sed the allowlist rejects (dangerous op, or an in-place write
        //     outside the working dirs, or any in-place edit when NOT in
        //     `AcceptEdits`) ASKS with the byte-locked sed message. ORDER (1:1
        //     with TS): AFTER the allow walk (step 5 — an explicit `Bash(sed:*)`
        //     allow rule already returned above) and BEFORE the mode auto-allow
        //     (step 6) + read-only allow (step 7). A `Safe` verdict contributes
        //     nothing here (falls through to the mode / read-only layers, exactly
        //     like a TS `passthrough`). Requires [`Self::roots`] for the in-place
        //     containment check; without roots the sed layer is skipped
        //     (consistent with the other shell guards).
        if let Some(ask) = self.shell_sed_constraint_ask(tool_name, input, mode) {
            return ask;
        }
        // 3a. AcceptEdits working-dir auto-allow (claude-code `checkWritePermissionForTool`
        //     step 3, `filesystem.ts:1360-1375`). In `AcceptEdits` mode an EDITOR
        //     tool whose target path (a) passes the auto-edit safety guard
        //     (`checkPathSafetyForAutoEdit`, Batch 2 — runs FIRST at `:1242`+, so
        //     `.git`/`.claude`/dangerous/suspicious-Windows paths fall through to
        //     ask) AND (b) lives inside an allowed working dir (cwd +
        //     `additional_working_dirs`, `allWorkingDirectories` `:667-674`) is
        //     auto-allowed with a `mode` reason. This runs AFTER the deny/ask/allow
        //     walks (so explicit deny/ask rules still win — preserved by ordering)
        //     and BEFORE the generic mode fallback. On any failure the branch is
        //     simply not taken and control falls through to the `AcceptEdits`-mode
        //     ask below. Requires [`Self::roots`] (the working-dir set is derived
        //     from `roots.cwd`).
        if mode == PermissionMode::AcceptEdits && file_tool_kind(tool_name) == FileToolKind::Editor
        {
            if let Some(roots) = self.roots.as_ref() {
                if let Some(raw_path) = input_path_for_tool(tool_name, input, roots) {
                    // Safety guard runs first (Batch 2). Only a `Safe` verdict may
                    // be auto-allowed; an `Unsafe` path falls through to ask.
                    if check_path_safety_for_auto_edit(&raw_path, roots) == AutoEditSafety::Safe {
                        // Working-dir set = cwd + additional dirs (`allWorkingDirectories`).
                        let mut working_dirs =
                            Vec::with_capacity(1 + self.additional_working_dirs.len());
                        working_dirs.push(roots.cwd.clone());
                        working_dirs.extend(self.additional_working_dirs.iter().cloned());
                        if path_in_allowed_working_path(
                            Path::new(raw_path.as_ref()),
                            &working_dirs,
                            roots,
                        ) {
                            return allow_with_mode(PermissionMode::AcceptEdits);
                        }
                    }
                }
            }
        }
        // 3a-bash. AcceptEdits bash auto-allow (claude-code `checkPermissionMode`
        //     + `ACCEPT_EDITS_ALLOWED_COMMANDS`, `BashTool/modeValidation.ts`,
        //     wired at `bashPermissions.ts:1142-1151`: sed-constraints THEN mode
        //     auto-allow). Sibling of the editor-tool branch above: in `AcceptEdits`
        //     mode a SHELL command auto-allows with a `mode` reason when EVERY
        //     subcommand's base command is in [`ACCEPT_EDITS_ALLOWED_COMMANDS`] —
        //     EXCEPT `sed`, which auto-allows only when its
        //     [`crate::sed_validation::sed_auto_allow_verdict`] is `Safe` (a
        //     dangerous or out-of-workdir in-place sed falls through to ask, 1:1
        //     with the TS `checkSedConstraints` step running BEFORE the mode
        //     auto-allow). ORDER: this runs AFTER the dangerous-removal (step 2)
        //     and path-constraint (step 2b) guards — which already returned an ask
        //     for `rm -rf /` / out-of-workdir redirects+cd — so it NEVER bypasses
        //     them. Requires [`Self::roots`] for the sed containment check (the
        //     working-dir set is derived from `roots.cwd` + additional dirs).
        if mode == PermissionMode::AcceptEdits && shell_command::is_shell_tool(tool_name) {
            if let Some(roots) = self.roots.as_ref() {
                if let Some(command) = shell_command::command_from_input(input) {
                    if let Some(result) = self.accept_edits_bash_auto_allow(command, roots) {
                        return result;
                    }
                }
            }
        }
        // 3c. READ-ONLY ALLOW (claude-code `bashToolHasPermission` step 7,
        //     `bashPermissions.ts:1154-1166`: `BashTool.isReadOnly(input)` →
        //     `checkReadOnlyConstraints` → allow with `decisionReason.type:
        //     'other', reason: 'Read-only command is allowed'`). A shell command
        //     whose EVERY subcommand is read-only ([`crate::read_only_command::command_is_read_only`])
        //     and that matched no deny/ask rule, no path-constraint / dangerous
        //     guard, and no allow rule is auto-allowed — the gate need not prompt
        //     for a `cat`/`ls`/`grep`. ORDER (1:1 with TS): AFTER the sed
        //     constraints (step 5b) and the mode auto-allow (step 6) and BEFORE
        //     the generic passthrough→ask (step 8). Placed ahead of the Plan
        //     backstop so a read-only command is allowed even in `Plan` mode (TS
        //     `checkReadOnlyConstraints` returns allow regardless of mode). The
        //     read-only inference is roots-independent (a pure command-shape
        //     check), so it runs whether or not [`Self::roots`] is set — but the
        //     path-constraint guard above (roots-gated) already pre-empted any
        //     out-of-workdir write, so this never auto-allows an escape.
        if Self::shell_is_read_only(tool_name, input) {
            return allow_read_only();
        }
        // 3b. Plan-mode mutation backstop (claude-code `prepareContextForPlanMode`,
        //     `permissionSetup.ts:1462-1500`). In `Plan` mode, the primary
        //     enforcement is that mutating tools are NOT advertised on the wire
        //     (a conversation-layer concern, out of scope here); this is the
        //     permission-layer backstop. A tool that is NOT on the read-only /
        //     planning-safe allowlist ([`crate::mode_policy::is_plan_safe_tool`],
        //     the external `SAFE_YOLO_ALLOWLISTED_TOOLS` subset) and that no
        //     allow rule matched is treated as a state mutation and ASKED about
        //     (NOT denied — matching TS, the user may approve and thereby exit
        //     plan-mode constraints). Deny/ask rules and explicit allow rules
        //     already won above, so they are preserved. Plan-safe tools fall
        //     through to the generic mode fallback below (and the gate's
        //     read-only auto-allow), keeping their existing path.
        // PERM.4 — Plan-mode bypass (claude-code `permissions.ts:1268-1281`,
        //     `shouldBypassPermissions`). A `Plan` session that ORIGINALLY had
        //     `BypassPermissions` available ([`Self::bypass_permissions_available`],
        //     TS `isBypassPermissionsModeAvailable`) bypasses permissions just like
        //     `BypassPermissions` mode — the tool is ALLOWED, tagged `Plan` (1:1
        //     with TS `decisionReason: { type: 'mode', mode: 'plan' }`). It runs
        //     AFTER the deny/ask/safety walks above (which already returned), so
        //     deny rules, ask rules, and the dangerous-removal / path-constraint /
        //     sed asks stay bypass-immune — matching the TS step order (1a deny,
        //     1d ask, 1g safety all precede the 2a bypass). Subject to the same
        //     killswitch override as `BypassPermissions`.
        if mode == PermissionMode::Plan
            && self.bypass_permissions_available
            && !self.bypass_killswitch_active
        {
            return allow_with_mode(PermissionMode::Plan);
        }
        if mode == PermissionMode::Plan && !crate::mode_policy::is_plan_safe_tool(tool_name) {
            return ask_plan_mutation(tool_name);
        }
        // 4. Mode fallback. `DontAsk` falls through to the generic mode ask here;
        //    the `ask`→`deny` conversion (PERM.1) is applied last in
        //    [`Self::authorize`], so read-only tools are not over-denied.
        match mode {
            PermissionMode::BypassPermissions if !self.bypass_killswitch_active => {
                allow_with_mode(PermissionMode::BypassPermissions)
            }
            _ => ask_with_mode(mode, tool_name),
        }
    }

    /// Strip every ALLOW rule that would bypass the auto-mode classifier (e.g.
    /// `Bash(python:*)`, `Agent(*)`, `PowerShell(iex:*)`), stashing the removed
    /// rules in [`Self::stripped_dangerous`] so [`Self::restore_dangerous`] can
    /// re-add them verbatim. 1:1 with `stripDangerousPermissionsForAutoMode`
    /// (`permissionSetup.ts:510-553`): the predicate is
    /// [`crate::dangerous_perms::is_dangerous_classifier_permission`].
    ///
    /// Deny/ask rules are never touched (only allow rules can auto-allow an
    /// arbitrary-code command ahead of the classifier). Idempotent in spirit but
    /// NOT a no-op on a fresh strip — call [`Self::restore_dangerous`] before
    /// re-stripping to avoid stacking the stash. (The mode-transition driver
    /// [`Self::set_mode`] guarantees a strip is always paired with a restore.)
    ///
    /// This does NOT change [`Self::authorize`] behavior on its own: Auto mode's
    /// classifier is unwired externally (Batch 6 stub), so stripping is
    /// behavior-neutral until a future wiring batch consumes Auto mode.
    pub fn strip_dangerous_for_auto(&mut self) {
        for (&source, rules) in &mut self.allow_rules {
            // Record each dangerous rule's ORIGINAL index within this bucket, then
            // restore re-inserts in ascending original-index order — which exactly
            // reconstructs the pre-strip vec (a removed slot's gap is re-filled
            // before any later removed slot is, so indices stay valid).
            let mut kept = Vec::with_capacity(rules.len());
            for (orig_idx, rule) in std::mem::take(rules).into_iter().enumerate() {
                if crate::dangerous_perms::is_dangerous_classifier_permission(
                    &rule.value.tool_name,
                    &rule.value.rule_content,
                ) {
                    self.stripped_dangerous.push(rule);
                    self.stripped_positions.push((source, orig_idx));
                } else {
                    kept.push(rule);
                }
            }
            *rules = kept;
        }
    }

    /// Re-add every allow rule previously stashed by
    /// [`Self::strip_dangerous_for_auto`] at its original bucket position, then
    /// clear the stash so a second call is a no-op. 1:1 with
    /// `restoreDangerousPermissions` (`permissionSetup.ts:561-579`). Exact
    /// inverse of a strip: `strip → restore` returns [`Self::allow_rules`] to its
    /// pre-strip contents (positions included — see [`Self::stripped_positions`]).
    pub fn restore_dangerous(&mut self) {
        let rules = std::mem::take(&mut self.stripped_dangerous);
        let positions = std::mem::take(&mut self.stripped_positions);
        // Re-insert in ascending recorded-index order so each rule lands back in
        // the same slot it was removed from (strip recorded indices in this order).
        for (rule, (source, idx)) in rules.into_iter().zip(positions) {
            let bucket = self.allow_rules.entry(source).or_default();
            let at = idx.min(bucket.len());
            bucket.insert(at, rule);
        }
    }

    /// Transition the active mode, running the auto-mode strip/restore
    /// side-effects. 1:1 with the strip/restore arms of `transitionPermissionMode`
    /// (`permissionSetup.ts:597-646`, the `:627-637` block):
    ///
    /// - entering `Auto` (from a non-Auto mode) → [`Self::strip_dangerous_for_auto`];
    /// - leaving `Auto` (to a non-Auto mode) → [`Self::restore_dangerous`].
    ///
    /// `to == from` is a no-op (matches the TS `fromMode === toMode` guard).
    /// The Plan-mode attachment and classifier-gate side-effects from TS are out
    /// of scope here (Plan is Batch 3; the LLM classifier is the stubbed Batch 6
    /// non-goal). Strip/restore is behavior-neutral on [`Self::authorize`] until
    /// Auto's classifier is wired, so this is safe to call now.
    pub fn set_mode(&mut self, to: PermissionMode) {
        let from = self.mode;
        if from == to {
            return;
        }
        if to == PermissionMode::Auto && from != PermissionMode::Auto {
            self.strip_dangerous_for_auto();
        } else if from == PermissionMode::Auto && to != PermissionMode::Auto {
            self.restore_dangerous();
        }
        self.mode = to;
    }

    /// Does `rule` apply to a call of `tool_name` with `input`?
    ///
    /// - No [`Self::roots`] → phase-2 verbatim: exact tool-name match (content
    ///   ignored). Preserves all pre-3a behavior and tests.
    /// - TOOL-WIDE rule (`rule_content == None`) → exact tool-name match
    ///   (claude-code `toolMatchesRule`; MCP server-level wildcard is not
    ///   modeled — phase-2 parity).
    /// - CONTENT rule on a NON-file tool → exact tool-name match (3a-bash
    ///   deferral: `Bash`/`WebFetch` content still matches tool-wide).
    /// - CONTENT rule on a file tool → group + path match:
    ///   - editor tool consults `Edit`-named rules;
    ///   - reader tool consults `Read`-named rules, plus `Edit`-named rules
    ///     with `Allow` behavior (edit-allow ⇒ read-allow). An `Edit`-named
    ///     DENY rule never blocks a read (claude-code `checkRead` only consults
    ///     `read` deny rules) — the `behavior == Allow` clause enforces this
    ///     because deny rules are only ever evaluated from the deny bucket.
    fn rule_matches(
        &self,
        rule: &PermissionRule,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> bool {
        let Some(roots) = self.roots.as_ref() else {
            // No roots → phase-2: file/shell content is ignored (matched
            // tool-wide). Tool-wide rules (`rule_content == None`) honor the
            // MCP server-level prefix match (PERM.2, claude-code
            // `toolMatchesRule`); content rules keep the phase-2 exact
            // tool-name match.
            return if rule.value.rule_content.is_none() {
                tool_wide_name_matches(&rule.value.tool_name, tool_name)
            } else {
                rule.value.tool_name == tool_name
            };
        };
        let Some(pattern) = rule.value.rule_content.as_deref() else {
            // PERM.2 — tool-wide rule → tool-name match, INCLUDING the MCP
            // server-level prefix match (claude-code `toolMatchesRule`: rule
            // `mcp__server` matches tool `mcp__server__tool`; `mcp__server__*`
            // matches all of that server's tools).
            return tool_wide_name_matches(&rule.value.tool_name, tool_name);
        };
        let group_ok = match file_tool_kind(tool_name) {
            FileToolKind::NonFile => {
                // Shell tools: CONTENT rule matches the command (any-subcommand,
                // aggressive stripping). Correct for deny/ask; allow uses the
                // dedicated `shell_allow` aggregation instead of this per-rule
                // path. Other non-file tools keep tool-wide matching.
                if shell_command::is_shell_tool(tool_name) && rule.value.tool_name == tool_name {
                    let Some(command) = shell_command::command_from_input(input) else {
                        return false;
                    };
                    return shell_command::rule_matches_any_subcommand(pattern, command);
                }
                // PERM.3 — other NON-file tools: a CONTENT rule applies ONLY when
                // the rule's content equals the tool-specific content key derived
                // from the input (claude-code per-tool
                // `getRuleByContentsForTool(...).get(ruleContent)` — e.g. WebFetch
                // `domain:{host}`, Agent `{agentType}`). A content rule must NOT
                // match tool-wide; tools without a known content scheme never
                // match on content (fail-safe, so an over-broad rule cannot deny
                // unrelated calls).
                if rule.value.tool_name != tool_name {
                    return false;
                }
                let Some(key) = tool_content_key(tool_name, input) else {
                    return false;
                };
                // WebFetch `domain:` rules support normalization + wildcards
                // (claude-code `y$n`/`v$a`/`bRp`, #31); other content tools
                // (Agent) stay raw-equality.
                return if tool_name == "WebFetch" {
                    domain_rule_matches(pattern, &key)
                } else {
                    key == pattern
                };
            }
            FileToolKind::Editor => rule.value.tool_name == "Edit",
            FileToolKind::Reader => {
                rule.value.tool_name == "Read"
                    || (rule.value.tool_name == "Edit"
                        && matches!(rule.behavior, PermissionBehavior::Allow))
            }
        };
        if !group_ok {
            return false;
        }
        let Some(path) = input_path_for_tool(tool_name, input, roots) else {
            return false;
        };
        path_matches_rule_pattern(&path, pattern, rule.source, roots)
    }

    fn rule_is_available_in_mode(&self, rule: &PermissionRule, mode: PermissionMode) -> bool {
        mode != PermissionMode::Auto
            || !crate::dangerous_perms::is_dangerous_classifier_permission(
                &rule.value.tool_name,
                &rule.value.rule_content,
            )
    }

    /// First rule in `bucket` (walked highest→lowest source priority) that
    /// applies to this call AND is in the requested tier: `content == false`
    /// selects TOOL-WIDE rules (`rule_content == None`, claude-code
    /// `toolMatchesRule`), `content == true` selects CONTENT rules. Splitting
    /// the tiers lets `authorize` order tool-wide-ask ahead of content-deny as
    /// the TS general checker does.
    fn first_match<'a>(
        &self,
        bucket: &'a HashMap<PermissionRuleSource, Vec<PermissionRule>>,
        sources: &[PermissionRuleSource],
        tool_name: &str,
        input: &serde_json::Value,
        content: bool,
    ) -> Option<&'a PermissionRule> {
        for src in sources {
            if let Some(rules) = bucket.get(src) {
                if let Some(rule) = rules.iter().find(|r| {
                    r.value.rule_content.is_some() == content
                        && self.rule_matches(r, tool_name, input)
                }) {
                    return Some(rule);
                }
            }
        }
        None
    }

    /// EXACT-match allow decision for a shell tool (claude-code
    /// `bashToolCheckExactMatchPermission`, the ALLOW arm). Returns the
    /// highest-priority CONTENT allow rule whose content EXACTLY matches the full
    /// trimmed command — an `Bash(cmd)` exact rule, or a `Bash(prefix:*)` rule
    /// whose bare prefix equals the whole command. Tool-wide allow rules
    /// (`rule_content == None`) are NOT exact matches (TS exact mode matches rule
    /// CONTENT against the command string), so they are skipped here and handled
    /// by the later [`Self::shell_allow`] walk. The match itself lives in
    /// [`shell_command::command_exact_allowed`] (the `matchMode: 'exact'` arm).
    /// Used only for the safety-bypass short-circuit; the reason is the matched
    /// content rule (TS `decisionReason: { type: 'rule', rule }`).
    fn shell_exact_allow(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        sources: &[PermissionRuleSource],
        mode: PermissionMode,
    ) -> Option<&PermissionRule> {
        let command = shell_command::command_from_input(input)?;
        for src in sources {
            if let Some(rules) = self.allow_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| {
                    self.rule_is_available_in_mode(r, mode)
                        && r.value.tool_name == tool_name
                        && r.value
                            .rule_content
                            .as_deref()
                            .is_some_and(|c| shell_command::command_exact_allowed(&[c], command))
                }) {
                    return Some(rule);
                }
            }
        }
        None
    }

    /// Allow decision for a shell tool, with compound-command aggregation.
    ///
    /// 1. A TOOL-WIDE allow rule (`Bash` with no content) allows every command.
    /// 2. Otherwise the command is allowed only if EVERY subcommand is covered
    ///    by some CONTENT allow rule (`Bash(npm install:*)` etc.). Gathering
    ///    rules across all sources matches claude-code, where deny/ask/allow
    ///    precedence is by behavior, not source. The reported rule is the
    ///    highest-priority content allow rule.
    fn shell_allow(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        sources: &[PermissionRuleSource],
        mode: PermissionMode,
    ) -> Option<&PermissionRule> {
        // 1. Tool-wide allow → allow everything.
        for src in sources {
            if let Some(rules) = self.allow_rules.get(src) {
                if let Some(rule) = rules.iter().find(|r| {
                    self.rule_is_available_in_mode(r, mode)
                        && r.value.tool_name == tool_name
                        && r.value.rule_content.is_none()
                }) {
                    return Some(rule);
                }
            }
        }
        // 2. Content allow aggregation over the command's subcommands.
        let command = shell_command::command_from_input(input)?;
        let mut content_rules: Vec<&PermissionRule> = Vec::new();
        for src in sources {
            if let Some(rules) = self.allow_rules.get(src) {
                for r in rules {
                    if self.rule_is_available_in_mode(r, mode)
                        && r.value.tool_name == tool_name
                        && r.value.rule_content.is_some()
                    {
                        content_rules.push(r);
                    }
                }
            }
        }
        if content_rules.is_empty() {
            return None;
        }
        let contents: Vec<&str> = content_rules
            .iter()
            .filter_map(|r| r.value.rule_content.as_deref())
            .collect();
        if shell_command::command_fully_allowed(&contents, command) {
            content_rules.first().copied()
        } else {
            None
        }
    }

    /// `AcceptEdits` bash auto-allow decision (claude-code `checkPermissionMode`
    /// + `ACCEPT_EDITS_ALLOWED_COMMANDS`). Returns:
    /// - `Some(Allow(mode=AcceptEdits))` when EVERY subcommand's base command is
    ///   in [`ACCEPT_EDITS_ALLOWED_COMMANDS`] AND every `sed` subcommand is
    ///   [`crate::sed_validation::SedVerdict::Safe`];
    /// - `Some(Ask(Other))` when a `sed` subcommand is otherwise on the
    ///   allowlist BUT its verdict is `Unsafe` (the byte-locked sed ask, 1:1 with
    ///   TS `checkSedConstraints` returning `behavior: 'ask'` ahead of the mode
    ///   auto-allow — a dangerous or out-of-workdir in-place sed prompts even in
    ///   `acceptEdits`);
    /// - `None` when some subcommand's base command is NOT on the allowlist (the
    ///   command falls through to the generic mode fallback / ask).
    ///
    /// `roots` supplies the working-dir set for the sed containment check
    /// (cwd + [`Self::additional_working_dirs`]).
    fn accept_edits_bash_auto_allow(
        &self,
        command: &str,
        roots: &FsRoots,
    ) -> Option<PermissionResult> {
        let subs = shell_command::split_command(command);
        if subs.is_empty() {
            return None;
        }
        // FIRST pass mirrors the TS step ordering (sed-constraints BEFORE the
        // mode auto-allow): an UNSAFE sed subcommand asks immediately, even when
        // another subcommand would otherwise disqualify the whole command from
        // auto-allow. (TS `checkSedConstraints` runs over the whole command and
        // returns its ask before `checkPermissionMode` is consulted at all.)
        for sub in &subs {
            if base_command(sub) == Some("sed") {
                if let crate::sed_validation::SedVerdict::Unsafe { message, reason } =
                    crate::sed_validation::sed_auto_allow_verdict(
                        sub,
                        roots,
                        &self.additional_working_dirs,
                    )
                {
                    return Some(ask_sed_constraint(message, reason));
                }
            }
        }
        // SECOND: every subcommand's base command must be on the allowlist (sed
        // already verified Safe above). Any non-allowlisted base → fall through.
        let all_allowed = subs.iter().all(|sub| {
            base_command(sub).is_some_and(|base| ACCEPT_EDITS_ALLOWED_COMMANDS.contains(&base))
        });
        if all_allowed {
            Some(allow_with_mode(PermissionMode::AcceptEdits))
        } else {
            None
        }
    }

    /// General sed-constraints ASK (claude-code `checkSedConstraints`, TS step
    /// 5b — runs in EVERY mode). Walks the command's subcommands; for each `sed`
    /// subcommand whose mode-aware verdict
    /// ([`crate::sed_validation::sed_constraint_verdict`], `allow_file_writes`
    /// true iff `AcceptEdits`) is `Unsafe`, returns the byte-locked sed ask. A
    /// `Safe` verdict contributes nothing (returns `None`, falling through to the
    /// mode / read-only layers — 1:1 with the TS `passthrough`). Returns the
    /// FIRST unsafe sed in subcommand order, matching TS.
    fn sed_constraint_ask(
        &self,
        command: &str,
        roots: &FsRoots,
        mode: PermissionMode,
    ) -> Option<PermissionResult> {
        let allow_file_writes = mode == PermissionMode::AcceptEdits;
        for sub in shell_command::split_command(command) {
            if base_command(&sub) != Some("sed") {
                continue;
            }
            if let crate::sed_validation::SedVerdict::Unsafe { message, reason } =
                crate::sed_validation::sed_constraint_verdict(
                    &sub,
                    allow_file_writes,
                    roots,
                    &self.additional_working_dirs,
                )
            {
                return Some(ask_sed_constraint(message, reason));
            }
        }
        None
    }

    /// Shell-only sandbox-auto-allow guard (the 1d layer). `true` iff this is a
    /// shell tool, a [`Self::sandbox_runtime`] config is attached, and the
    /// command would be sandbox-auto-allowed. Non-shell tools / absent config ⇒
    /// `false` (no-op).
    fn shell_sandbox_auto_allows(&self, tool_name: &str, input: &serde_json::Value) -> bool {
        let Some(sandbox) = self.sandbox_runtime.as_ref() else {
            return false;
        };
        if !shell_command::is_shell_tool(tool_name) {
            return false;
        }
        shell_command::command_from_input(input).is_some_and(|cmd| sandbox.auto_allows(cmd))
    }

    /// Shell-only general sed-constraint ASK (the 3-sed layer). Returns the
    /// byte-locked sed ask for the first `Unsafe` sed subcommand, or `None` when
    /// not a shell tool / no roots / no command / every sed is `Safe`.
    fn shell_sed_constraint_ask(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        mode: PermissionMode,
    ) -> Option<PermissionResult> {
        if !shell_command::is_shell_tool(tool_name) {
            return None;
        }
        let roots = self.roots.as_ref()?;
        let command = shell_command::command_from_input(input)?;
        self.sed_constraint_ask(command, roots, mode)
    }

    /// Shell-only bash command-injection safety ASK (the 2c layer). Splits the
    /// command into subcommands ([`crate::shell_command::split_command`], the
    /// claude-code `splitCommand` analogue), strips each subcommand's output
    /// redirection (matching TS, where `splitCommand` yields redirect-stripped
    /// subcommands and `checkPathConstraints` validates the redirect target
    /// separately — our 2b guard), and runs the
    /// [`crate::bash_security::bash_command_is_safe`] battery on each. Returns the
    /// FIRST subcommand's ask (1:1 with the per-subcommand
    /// `checkCommandAndSuggestRules` short-circuit), or `None` for a non-shell
    /// tool / no command / every subcommand safe.
    fn shell_bash_safety_ask(
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Option<PermissionResult> {
        if !shell_command::is_shell_tool(tool_name) {
            return None;
        }
        let command = shell_command::command_from_input(input)?;

        // ── AST-authoritative safety gate (claude-code 2.1.195). When the
        // tree-sitter grammar is available, the AST verdict is authoritative:
        //   • TooComplex      → ask (cannot be statically analyzed)
        //   • Simple + Deny   → ask (a dangerous construct was found)
        //   • Simple + Ok     → clean parse, no dangerous semantics → the legacy
        //                       regex battery is intentionally SKIPPED (matching
        //                       the binary's `astParseSucceeded` gate)
        //   • ParseUnavailable→ fall through to the legacy battery below
        // SAFETY: `parse_for_security` is conservative (it OVER-marks TooComplex,
        // verified zero under-ask), and `check_semantics` is reconciled to the
        // 2.1.195 binary superset, so `Simple+Ok ⊆ the binary's allow-set` — the
        // flip can only ask MORE than the binary, never less.
        #[cfg(feature = "bash-ast")]
        {
            use crate::bash_ast_security::{
                check_semantics, parse_for_security, ParseForSecurityResult, SemanticCheckResult,
            };
            match parse_for_security(command) {
                ParseForSecurityResult::TooComplex { reason } => {
                    return Some(ask_bash_safety(tool_name, reason));
                }
                ParseForSecurityResult::Simple { commands } => {
                    if let SemanticCheckResult::Deny { reason } = check_semantics(&commands) {
                        return Some(ask_bash_safety(tool_name, reason));
                    }
                    return None;
                }
                ParseForSecurityResult::ParseUnavailable => {
                    // Grammar could not parse this command — fall through to the
                    // legacy regex battery (which never early-allows).
                }
            }
        }

        for sub in shell_command::split_command(command) {
            let stripped = shell_command::strip_output_redirections(&sub);
            if let crate::bash_security::BashSafetyVerdict::Ask { message } =
                crate::bash_security::bash_command_is_safe(&stripped)
            {
                return Some(ask_bash_safety(tool_name, message));
            }
        }
        None
    }

    /// Shell-only read-only inference (the 3c layer). `true` iff this is a shell
    /// tool whose command is wholly read-only
    /// ([`crate::read_only_command::command_is_read_only`]).
    fn shell_is_read_only(tool_name: &str, input: &serde_json::Value) -> bool {
        if !shell_command::is_shell_tool(tool_name) {
            return false;
        }
        shell_command::command_from_input(input)
            .is_some_and(crate::read_only_command::command_is_read_only)
    }
}

/// Base (first) command word of a subcommand — TS `trimmedCmd.split(/\s+/)[0]`.
/// Returns `None` for an empty subcommand.
fn base_command(sub: &str) -> Option<&str> {
    sub.split_whitespace().next()
}

/// Parsed MCP tool/rule name — 1:1 with claude-code
/// `mcpInfoFromString` (`services/mcp/mcpStringUtils.ts:19-31`).
struct McpInfo<'a> {
    server_name: &'a str,
    /// `None` for a server-level name (`mcp__server`); `Some("tool")` for a
    /// fully-qualified name; `Some("*")` for the explicit wildcard.
    tool_name: Option<&'a str>,
}

/// Split `mcp__<server>[__<tool…>]` into its parts, or `None` for a non-MCP
/// string. Mirrors TS `mcpInfoFromString`: requires the `mcp` prefix and a
/// non-empty server; everything after the server (joined back with `__`) is the
/// tool name, or `None` when absent.
fn mcp_info_from_string(s: &str) -> Option<McpInfo<'_>> {
    let mut parts = s.splitn(3, "__");
    let mcp_part = parts.next()?;
    if mcp_part != "mcp" {
        return None;
    }
    let server_name = parts.next().filter(|p| !p.is_empty())?;
    // `splitn(3, ..)` keeps everything after the second `__` (incl. further
    // `__`) intact as the tool name — matching TS `toolNameParts.join('__')`.
    let tool_name = parts.next();
    Some(McpInfo {
        server_name,
        tool_name,
    })
}

/// Does a TOOL-WIDE rule name match a tool — 1:1 with the tool-name branch of
/// claude-code `toolMatchesRule` (`permissions.ts:251-268`). Exact name match,
/// OR an MCP server-level rule: `mcp__server` (or `mcp__server__*`) matches any
/// `mcp__server__tool` of that server.
///
/// `pub` so the orchestrator's wire-tool deny filter
/// ([`PermissionPolicy::tool_wide_deny_names`] → consumer in `build_wire_tools`)
/// strips denied tools BEFORE the model sees them using the SAME matcher the
/// runtime check uses (claude-code `filterToolsByDenyRules`, `tools.ts:262-269`).
#[must_use]
pub fn tool_wide_name_matches(rule_tool_name: &str, tool_name: &str) -> bool {
    if rule_tool_name == tool_name {
        return true;
    }
    let (Some(rule_info), Some(tool_info)) = (
        mcp_info_from_string(rule_tool_name),
        mcp_info_from_string(tool_name),
    ) else {
        return false;
    };
    (rule_info.tool_name.is_none() || rule_info.tool_name == Some("*"))
        && rule_info.server_name == tool_info.server_name
}

/// The tool-specific permission-rule CONTENT key derived from a tool call's
/// input, for the NON-file/NON-shell content tools (PERM.3). A content rule
/// matches iff its content string equals this key (claude-code per-tool
/// `…ToPermissionRuleContent` + `getRuleByContentsForTool(...).get(key)`).
///
/// - `WebFetch` → `domain:{hostname}` from `input.url`
///   (`WebFetchTool.ts:50-63`).
/// - `Agent` (and its legacy alias `Task`) → the `subagent_type`, defaulting to
///   `general-purpose` when omitted (claude-code `getDenyRuleForAgent`:
///   `ruleContent === agentType`, with the general-purpose default).
/// - any other tool → `None` (no content scheme ⇒ a content rule never matches).
fn tool_content_key(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "WebFetch" => {
            let url = input.get("url")?.as_str()?;
            Some(format!("domain:{}", url_hostname(url)?))
        }
        "Agent" | "Task" => {
            // TS resolves an omitted `subagent_type` to the general-purpose
            // agent's type before matching deny rules.
            let agent_type = input
                .get("subagent_type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("general-purpose");
            Some(agent_type.to_string())
        }
        _ => None,
    }
}

/// Extract the hostname from a URL string — the WHATWG `new URL(url).hostname`
/// claude-code uses to key `domain:${new URL(n).hostname}` rules (`_qa`,
/// offset ~201711761). Delegates to the `url` crate (the Rust WHATWG URL
/// parser): `Url::host_str()` reproduces `.hostname` exactly — it
/// Punycode/IDNA-encodes IDN hosts (`münchen.de` → `xn--mnchen-3ya.de`),
/// percent-decodes the host (`foo%2Ebar.com` → `foo.bar.com`), lowercases it,
/// strips userinfo/port/path/query/fragment, and preserves a bracketed IPv6
/// literal. Returns `None` when the URL fails to parse or carries no host
/// (1:1 with `new URL(...)` throwing / a hostless URL — `_qa` then keys no
/// `domain:` rule).
fn url_hostname(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// Whether a WebFetch `domain:` CONTENT-rule `pattern` matches the candidate
/// `domain:{host}` `key` — 1:1 with claude-code's per-rule `y$n` logic (#31):
/// raw exact hit, else normalized exact (non-wildcard) match, else `bRp`
/// wildcard match. (`y$n`'s exact-over-wildcard PRECEDENCE across a bucket is a
/// separate, finer nuance handled at the rule-walk level; this is the per-rule
/// predicate.)
fn domain_rule_matches(pattern: &str, key: &str) -> bool {
    if pattern == key {
        return true;
    }
    let np = normalize_domain_key(pattern);
    let nk = normalize_domain_key(key);
    if np.contains('*') {
        domain_wildcard_matches(&np, &nk)
    } else {
        np == nk
    }
}

/// Normalize a `domain:` rule string — claude-code `v$a`: lowercase the host and
/// strip a trailing run of dots (the `replace(/(?<=[^*.])\.+(?=(:\d+)?$)/,"")`).
/// Non-`domain:` strings are returned unchanged.
fn normalize_domain_key(s: &str) -> String {
    let Some(rest) = s.strip_prefix("domain:") else {
        return s.to_string();
    };
    let lower = rest.to_ascii_lowercase();
    // Split off an optional trailing `:port` (`:\d+$`) so dots are stripped from
    // the host body only (matching the `(?=(:\d+)?$)` lookahead).
    let (body, port) = match lower.rfind(':') {
        Some(i)
            if !lower[i + 1..].is_empty() && lower[i + 1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            (&lower[..i], &lower[i..])
        }
        _ => (lower.as_str(), ""),
    };
    let trimmed = body.trim_end_matches('.');
    // The lookbehind `(?<=[^*.])` requires the char before the dot-run to be
    // neither `*` nor `.` (and to exist); otherwise the dots are NOT stripped.
    let strip = trimmed.len() != body.len()
        && !matches!(trimmed.chars().next_back(), None | Some('*') | Some('.'));
    if strip {
        format!("domain:{trimmed}{port}")
    } else {
        format!("domain:{lower}")
    }
}

/// Whether wildcard `pattern` matches `candidate` (both normalized `domain:`
/// strings) — claude-code `bRp` + `w$a`. `domain:*` matches all; `domain:*.x`
/// matches one-or-more leading labels then `x`; a bare `*` becomes `[^.:]*`.
fn domain_wildcard_matches(pattern: &str, candidate: &str) -> bool {
    if !pattern.starts_with("domain:") || !candidate.starts_with("domain:") {
        return false;
    }
    if pattern == "domain:*" {
        return true;
    }
    let regex_str = if let Some(suffix) = pattern.strip_prefix("domain:*.") {
        format!("^domain:(?:[^.:]+\\.)+{}$", escape_domain_wildcard(suffix))
    } else {
        let rest = &pattern["domain:".len()..];
        format!("^domain:{}$", escape_domain_wildcard(rest))
    };
    // `(?i)` mirrors `bRp`'s `new RegExp(n, "i")` (inputs are already lowercased).
    regex::Regex::new(&format!("(?i){regex_str}")).is_ok_and(|re| re.is_match(candidate))
}

/// Escape regex metacharacters and turn each `*` into `[^.:]*` — claude-code
/// `w$a` (`split("*").map(escape).join("[^.:]*")`).
fn escape_domain_wildcard(s: &str) -> String {
    s.split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join("[^.:]*")
}

/// Byte-locked sed-constraint ask: an `acceptEdits` `sed` subcommand whose
/// auto-allow verdict is `Unsafe` (claude-code `checkSedConstraints` returning
/// `behavior: 'ask'`, `sedValidation.ts:665-675`). Tagged
/// [`PermissionDecisionReason::Other`] (TS `decisionReason.type: 'other'`),
/// carrying the byte-locked message + reason.
fn ask_sed_constraint(message: String, reason: String) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::Other { reason },
        prompt: PermissionPrompt {
            title: "Allow Bash?".to_string(),
            message,
            options: vec!["Allow once".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

fn allow_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

fn deny_with_rule(rule: &PermissionRule) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        explanation: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Deny from a CONTENT (command-specific) rule match. For the command tools
/// (`Bash` / `PowerShell`) this carries the command in the model-facing message,
/// 1:1 with `bashPermissions.ts:1003` / `powershellPermissions.ts:396`:
/// `Permission to use ${tool} with command ${command} has been denied.`
/// (`command = input.command.trim()`). Other tools — and the TOOL-WIDE deny path
/// (`permissions.ts:1087`) — use the generic `deny_reason_string` message via an
/// absent `explanation`.
fn deny_with_rule_content(
    rule: &PermissionRule,
    tool_name: &str,
    input: &serde_json::Value,
) -> PermissionResult {
    let explanation = if tool_name == "Bash" || tool_name == "PowerShell" {
        input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|cmd| {
                format!(
                    "Permission to use {tool_name} with command {} has been denied.",
                    cmd.trim()
                )
            })
    } else {
        // Generic CONTENT deny (binary `uMe`/`mZt`: `Permission to use ${e.name}
        // with ${s.ruleValue.ruleContent} has been denied.`, od -c @215384392).
        // Non-shell tools with a dedicated content rule (e.g. `WebFetch(domain:…)`,
        // `Agent(type)`) surface the matched `ruleContent`; a tool-wide rule
        // (`rule_content == None`) falls through to the bare generic message.
        rule.value
            .rule_content
            .as_deref()
            .map(|content| format!("Permission to use {tool_name} with {content} has been denied."))
    };
    PermissionResult::Deny {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        explanation,
        metadata: PermissionMetadata::default(),
    }
}

fn allow_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::PermissionMode { mode },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Sandbox auto-allow grant (claude-code `checkSandboxAutoAllow`'s final
/// `behavior: 'allow'`, `decisionReason: { type: 'other', reason: 'Auto-allowed
/// with sandbox (autoAllowBashIfSandboxed enabled)' }`). Tagged
/// [`PermissionDecisionReason::Other`] carrying the byte-faithful reason (TS
/// uses `type: 'other'` here, NOT a sandbox-specific reason — preserved so the
/// existing `SandboxOverrideReason` enum is untouched).
fn allow_sandbox_auto() -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::Other {
            reason: "Auto-allowed with sandbox (autoAllowBashIfSandboxed enabled)".to_string(),
        },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Read-only command auto-allow (claude-code `bashToolHasPermission` step 7,
/// `behavior: 'allow'`, `decisionReason: { type: 'other', reason: 'Read-only
/// command is allowed' }`). Tagged [`PermissionDecisionReason::Other`] carrying
/// the byte-faithful reason.
fn allow_read_only() -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::Other {
            reason: "Read-only command is allowed".to_string(),
        },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

fn deny_with_mode(mode: PermissionMode) -> PermissionResult {
    PermissionResult::Deny {
        reason: PermissionDecisionReason::PermissionMode { mode },
        explanation: None,
        metadata: PermissionMetadata::default(),
    }
}

fn ask_with_rule(rule: &PermissionRule, tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::MatchedRule { rule: rule.clone() },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            // `createPermissionRequestMessage` rule branch (permissions.ts:163):
            // `Permission rule '${rule}' from ${source} requires approval for
            // this ${tool} command`.
            message: format!(
                "Permission rule '{}' from {} requires approval for this {tool_name} command",
                rule.value.to_rule_string(),
                crate::shadow::format_source(rule.source)
            ),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

fn ask_with_mode(mode: PermissionMode, tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::PermissionMode { mode },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            // `createPermissionRequestMessage` mode branch (permissions.ts:200):
            // `Current permission mode (${modeTitle}) requires approval for this
            // ${tool} command`.
            message: format!(
                "Current permission mode ({}) requires approval for this {tool_name} command",
                mode.title()
            ),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Dangerous-removal ask: an `rm`/`rmdir` targeting a critical system path
/// (claude-code `checkDangerousRemovalPaths`). Tagged with
/// [`PermissionDecisionReason::Other`] (the TS `decisionReason.type: 'other'`),
/// carrying the byte-locked message; offers no rule-saving suggestion (TS:
/// "Don't provide suggestions — we don't want to encourage saving dangerous
/// commands").
fn ask_dangerous_removal(
    tool_name: &str,
    danger: crate::dangerous_removal::DangerousRemoval,
) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::Other {
            reason: danger.reason,
        },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: danger.message,
            options: vec!["Allow once".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Bash path-constraint ask: a command writing/`cd`-ing/process-substituting
/// outside the allowed working dirs (claude-code `checkPathConstraints`). Tagged
/// with [`PermissionDecisionReason::Other`] (the TS `decisionReason.type:
/// 'other'`), carrying the byte-locked message. Offers no rule-saving
/// suggestion (the TS suggestions are a UI concern modeled elsewhere; the
/// permission-layer decision is the ask itself).
fn ask_path_constraint(
    tool_name: &str,
    ask: crate::path_constraints::PathConstraintAsk,
) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::Other { reason: ask.reason },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: ask.message,
            options: vec!["Allow once".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Bash command-injection safety ask: a shell command whose
/// [`crate::bash_security::bash_command_is_safe`] battery returned a detection
/// (claude-code `bashCommandIsSafe` → `checkCommandAndSuggestRules` step 3
/// returning `behavior: 'ask'`, `bashPermissions.ts:1223-1237`). Tagged
/// [`PermissionDecisionReason::SafetyCheck`] carrying the byte-faithful validator
/// `message`. `classifier_approvable` is `true`: the TS flow attaches a pending
/// `BASH_CLASSIFIER` check that may auto-approve before the user responds (the
/// classifier itself is unwired here, so this is a hint for a later batch). No
/// rule-saving suggestion (TS: "Don't suggest saving a potentially dangerous
/// command", `:1236`).
fn ask_bash_safety(tool_name: &str, message: String) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::SafetyCheck {
            reason: message.clone(),
            classifier_approvable: true,
        },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message,
            options: vec!["Allow once".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

/// Plan-mode mutation backstop ask: a tool that would mutate state in `Plan`
/// mode. Tagged with [`PermissionMode::Plan`]; the message tells the user that
/// approving exits the plan-mode constraints (matching claude-code, which
/// surfaces a plan-mode mutation as an interactive ask, NOT a hard deny).
fn ask_plan_mutation(tool_name: &str) -> PermissionResult {
    PermissionResult::Ask {
        reason: PermissionDecisionReason::PermissionMode {
            mode: PermissionMode::Plan,
        },
        prompt: PermissionPrompt {
            title: format!("Allow {tool_name}?"),
            message: "Plan mode: this tool would modify state; approve to exit \
                plan-mode constraints."
                .into(),
            options: vec!["Allow once".into(), "Always allow".into(), "Deny".into()],
        },
        pending_classifier_check: None,
        metadata: PermissionMetadata::default(),
    }
}

#[cfg(test)]
#[path = "policy_test.rs"]
mod policy_test;
