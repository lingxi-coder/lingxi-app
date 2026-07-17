//! CLI permission-mode resolution — pure port of
//! `initialPermissionModeFromCLI` (claude-code `utils/permissions/
//! permissionSetup.ts:689-812`) and `permissionModeFromString`
//! (`PermissionMode.ts:117-121`).
//!
//! Pure by construction: all inputs (parsed flags + a settings view) are
//! passed in, so the priority logic is exhaustively testable without env or
//! IO. The caller (the CLI) reads the merged settings and process env.
//!
//! Documented omissions vs TS (no GrowthBook/Statsig substrate in this build):
//! - Statsig `tengu_disable_bypass_permissions_mode` gate (and its
//!   `"…disabled by your organization policy"` notice) — no Statsig substrate;
//!   only the settings-disable notice is reachable.
//! - `LINGXI_REMOTE` filtering of settings `defaultMode` (CCR) — `LingXi`
//!   has no CCR remote entrypoint; the `tengu_ccr_unsupported_default_mode_ignored`
//!   event is not reproduced. The caller passes `default_mode` straight through.

use crate::mode::PermissionMode;

/// The settings inputs the resolver reads. The caller extracts these from the
/// merged raw settings (e.g. via `default_mode_from_settings_json` +
/// `bypass_permissions_disabled_from_settings_json`), keeping this fn pure.
#[derive(Debug, Clone)]
pub struct CliModeSettings {
    /// `settings.permissions.defaultMode`, already validated to an external
    /// mode (else `None`).
    pub default_mode: Option<PermissionMode>,
    /// `settings.permissions.disableBypassPermissionsMode === "disable"` — the
    /// bypass-permissions killswitch.
    pub bypass_disabled: bool,
    /// `disableAutoMode === "disable"` at either settings position — the
    /// auto-mode killswitch (`Bpa()`). Sticky across tiers like
    /// [`Self::bypass_disabled`]. Fed to
    /// [`crate::auto_gate::apply_auto_mode_gate`] at boot so a requested `auto`
    /// (CLI flag or settings `defaultMode: auto`) is downgraded to `default`
    /// when set.
    pub auto_mode_disabled: bool,
    /// `MODE-SETTINGS-AUTO-TRUST-01`: was a settings `defaultMode: "auto"`
    /// granted by a TRUSTED tier (`policySettings`/`userSettings`/`flagSettings`)?
    ///
    /// Only consulted when [`Self::default_mode`] is [`PermissionMode::Auto`].
    /// The caller computes it by checking whether any trusted tier declared
    /// `defaultMode: "auto"` (see
    /// [`crate::loader::auto_mode_grantable_by_source`]). When `default_mode` is
    /// `Auto` but this is `false`, the auto request is IGNORED (the repo-
    /// controllable `projectSettings`/`localSettings` may not enable classifier-
    /// driven auto-accept). Irrelevant for the five external modes, which may be
    /// set from any tier.
    pub auto_default_from_trusted: bool,
    /// `MODE-BG-DISCLAIMER-02`: is this a background session
    /// (`LINGXI_SESSION_KIND == "bg"`, claude-code `CLAUDE_CODE_SESSION_KIND`)?
    /// The bg-disclaimer downgrade only applies in background sessions
    /// (`xlc()` short-circuits `!Pi()` → false otherwise).
    pub is_bg_session: bool,
    /// `MODE-BG-DISCLAIMER-02`: does ANY settings tier set
    /// `skipDangerousModePermissionPrompt` truthy (claude-code `Pq()`)? When set,
    /// the user has already accepted the Bypass Permissions disclaimer, so the
    /// bg-session downgrade does NOT apply.
    pub skip_dangerous_mode_permission_prompt: bool,
    /// `MODE-BG-DISCLAIMER-02`: the persisted global-config
    /// `bypassPermissionsModeAccepted` flag (claude-code
    /// `St().bypassPermissionsModeAccepted`). When true, bypass was accepted
    /// interactively and the bg downgrade does NOT apply.
    pub bypass_permissions_mode_accepted: bool,
}

impl CliModeSettings {
    /// `xlc("bypassPermissions")` (claude-code): does the bg-session disclaimer
    /// gate downgrade a requested `bypassPermissions` to `default`?
    ///
    /// `xlc(e){if(!Pi())return!1;if(e==="bypassPermissions")return!Pq()&&
    /// !St().bypassPermissionsModeAccepted;return!1}` — i.e. true iff this is a
    /// background session AND no tier set `skipDangerousModePermissionPrompt`
    /// AND `bypassPermissionsModeAccepted` is unset. A `--bg` daemon cannot show
    /// the interactive disclaimer, so bypass must be earned beforehand.
    #[must_use]
    fn bg_bypass_disclaimer_gate_trips(&self) -> bool {
        self.is_bg_session
            && !self.skip_dangerous_mode_permission_prompt
            && !self.bypass_permissions_mode_accepted
    }
}

/// `permissionModeFromString` (`PermissionMode.ts:117-121`): the valid set is
/// the five external modes plus `auto`; anything else (incl. internal `bubble`)
/// → `Default`.
///
/// `"manual"` maps to [`PermissionMode::Default`] (parity 2.1.207): the CLI's
/// commander `.choices` display swaps `default`→`manual` (`bha=WB.map(e=>
/// e==="default"?"manual":e)`) and the accepted set (`$7_=[...WB,"manual"]`)
/// keeps both spellings, while the shared `ZS(e)=e==="manual"?"default":e`
/// preprocess normalizes `manual`→`default`. The catch-all below already
/// yields `Default`, but the explicit arm documents the alias and keeps this
/// in lockstep with [`crate::default_mode_from_settings_json`].
#[must_use]
pub fn permission_mode_from_cli_string(s: &str) -> PermissionMode {
    match s {
        "default" | "manual" => PermissionMode::Default,
        "plan" => PermissionMode::Plan,
        "acceptEdits" => PermissionMode::AcceptEdits,
        "bypassPermissions" => PermissionMode::BypassPermissions,
        "dontAsk" => PermissionMode::DontAsk,
        "auto" => PermissionMode::Auto,
        _ => return PermissionMode::Default,
    }
}

/// `MODE-ENV-SCRUB-03`: byte-exact `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`
/// force-to-default notice, with the accepted `LINGXI_` env-name divergence
/// (the subprocess-env scrub is `LINGXI_SUBPROCESS_ENV_SCRUB` in this port,
/// `platforms/posix/src/process/runner.rs`). Em-dash is U+2014.
pub(crate) const ENV_SCRUB_FORCED_TO_DEFAULT_MSG: &str = "Permission mode forced to default \u{2014} LINGXI_SUBPROCESS_ENV_SCRUB is set (allowed_non_write_users hardening). Declare allowedTools explicitly, or set LINGXI_SUBPROCESS_ENV_SCRUB=0 to opt out.";

/// `MODE-BG-DISCLAIMER-02`: byte-exact claude-code `Rlc` — the notice shown when
/// a background session's requested `bypassPermissions` is downgraded to
/// `default` because the disclaimer was never accepted interactively. Em-dash is
/// U+2014.
pub(crate) const BYPASS_DISCLAIMER_DOWNGRADE_MSG: &str = "Permission mode downgraded to default \u{2014} bypass requires accepting the disclaimer interactively first";

/// `initialPermissionModeFromCLI` (`permissionSetup.ts:689-812`): resolve the
/// session permission mode from CLI flags + settings, returning the mode plus
/// an optional user-facing notice (set when the bypass killswitch suppresses a
/// requested bypass, or a hardening/downgrade path forces the mode).
///
/// `agent_frontmatter_mode` is the `--agent`-resolved agent definition's
/// `permissionMode` (`MODE-FRONTMATTER-04` / klc's `a=o?.permissionMode`). It is
/// pushed into `orderedModes` AFTER the `--permission-mode` flag and BEFORE the
/// settings `defaultMode`, and it participates in the env-scrub non-default
/// request test (klc `a&&a!=="default"`).
///
/// `env_scrub_active` is `isEnvTruthy(LINGXI_SUBPROCESS_ENV_SCRUB)` — the
/// `allowed_non_write_users` subprocess-env-scrub hardening flag. When set, the
/// resolver short-circuits to [`PermissionMode::Default`] BEFORE any other
/// input (`MODE-ENV-SCRUB-03` / claude-code `klc`'s leading
/// `if(ut(r.CLAUDE_CODE_SUBPROCESS_ENV_SCRUB))` guard), emitting the notice iff
/// a non-default mode was actually requested (skip flag, a CLI mode other than
/// default, or an agent frontmatter mode other than default).
#[must_use]
pub fn initial_permission_mode_from_cli(
    permission_mode_cli: Option<&str>,
    dangerously_skip: bool,
    agent_frontmatter_mode: Option<PermissionMode>,
    env_scrub_active: bool,
    settings: &CliModeSettings,
) -> (PermissionMode, Option<String>) {
    let cli_mode = permission_mode_cli.map(permission_mode_from_cli_string);

    // MODE-ENV-SCRUB-03: env-scrub hardening forces mode to default before any
    // other input is consulted. The notice fires only when a non-default mode
    // was requested (skip flag, a CLI mode that isn't `default`, or an agent
    // frontmatter mode that isn't `default`), matching klc's
    // `y=s||i&&i!=="default"||a&&a!=="default"`.
    if env_scrub_active {
        let requested_non_default = dangerously_skip
            || cli_mode.is_some_and(|m| m != PermissionMode::Default)
            || agent_frontmatter_mode.is_some_and(|m| m != PermissionMode::Default);
        let notice = requested_non_default.then(|| ENV_SCRUB_FORCED_TO_DEFAULT_MSG.to_string());
        return (PermissionMode::Default, notice);
    }

    // Modes in order of priority (TS `orderedModes`).
    let mut ordered: Vec<PermissionMode> = Vec::new();
    let mut notification: Option<String> = None;

    // MODE-BG-DISCLAIMER-02: in a background session that never accepted the
    // Bypass Permissions disclaimer, a requested `bypassPermissions` (from the
    // skip flag OR from --permission-mode) is downgraded to `default` at push
    // time (claude-code `xlc()`), carrying the `Rlc` notice. A `--bg` daemon
    // cannot show the interactive disclaimer, so it must not silently run with
    // full bypass.
    let bg_bypass_downgrade = settings.bg_bypass_disclaimer_gate_trips();

    if dangerously_skip {
        if bg_bypass_downgrade {
            notification = Some(BYPASS_DISCLAIMER_DOWNGRADE_MSG.to_string());
            ordered.push(PermissionMode::Default);
        } else {
            ordered.push(PermissionMode::BypassPermissions);
        }
    }
    if let Some(cli) = cli_mode {
        if cli == PermissionMode::BypassPermissions && bg_bypass_downgrade {
            notification = Some(BYPASS_DISCLAIMER_DOWNGRADE_MSG.to_string());
            ordered.push(PermissionMode::Default);
        } else {
            ordered.push(cli);
        }
    }
    // MODE-FRONTMATTER-04: agent frontmatter permissionMode sits between the
    // CLI flag and the settings defaultMode. (claude-code does NOT run the bg
    // disclaimer gate `xlc` on the frontmatter mode.)
    if let Some(frontmatter) = agent_frontmatter_mode {
        ordered.push(frontmatter);
    }
    if let Some(default_mode) = settings.default_mode {
        // MODE-SETTINGS-AUTO-TRUST-01: a settings `defaultMode: "auto"` is only
        // honored when a trusted tier (policy/user/flag) granted it. From the
        // repo-controllable projectSettings/localSettings tiers it is IGNORED
        // (klc warns + emits `tengu_settings_auto_mode_untrusted_source_ignored`;
        // the warn/telemetry are omitted at this pure layer). The five external
        // modes are unaffected — they may be set from any tier.
        let untrusted_auto =
            default_mode == PermissionMode::Auto && !settings.auto_default_from_trusted;
        if !untrusted_auto {
            ordered.push(default_mode);
        }
    }

    for mode in ordered {
        if mode == PermissionMode::BypassPermissions && settings.bypass_disabled {
            // TS: skip this mode, carry the notice forward.
            notification = Some("Bypass permissions mode was disabled by settings".to_string());
            continue;
        }
        return (mode, notification);
    }
    (PermissionMode::Default, notification)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::PermissionMode;

    fn no_settings() -> CliModeSettings {
        CliModeSettings {
            default_mode: None,
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        }
    }

    #[test]
    fn from_string_accepts_the_five_external_modes() {
        assert_eq!(
            permission_mode_from_cli_string("default"),
            PermissionMode::Default
        );
        assert_eq!(
            permission_mode_from_cli_string("plan"),
            PermissionMode::Plan
        );
        assert_eq!(
            permission_mode_from_cli_string("acceptEdits"),
            PermissionMode::AcceptEdits
        );
        assert_eq!(
            permission_mode_from_cli_string("bypassPermissions"),
            PermissionMode::BypassPermissions
        );
        assert_eq!(
            permission_mode_from_cli_string("dontAsk"),
            PermissionMode::DontAsk
        );
    }

    #[test]
    fn from_string_unknown_falls_to_default() {
        assert_eq!(
            permission_mode_from_cli_string("bubble"),
            PermissionMode::Default
        );
        assert_eq!(
            permission_mode_from_cli_string("garbage"),
            PermissionMode::Default
        );
    }

    #[test]
    fn from_string_accepts_auto() {
        assert_eq!(
            permission_mode_from_cli_string("auto"),
            PermissionMode::Auto
        );
    }

    #[test]
    fn from_string_accepts_manual_as_default_alias() {
        // parity 2.1.207: `manual` is the CLI-facing alias for `default`.
        assert_eq!(
            permission_mode_from_cli_string("manual"),
            PermissionMode::Default
        );
    }

    #[test]
    fn dangerously_skip_wins_and_yields_bypass() {
        let (mode, notice) =
            initial_permission_mode_from_cli(None, true, None, false, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn cli_flag_used_when_no_skip() {
        let (mode, _) =
            initial_permission_mode_from_cli(Some("plan"), false, None, false, &no_settings());
        assert_eq!(mode, PermissionMode::Plan);
    }

    #[test]
    fn skip_outranks_cli_flag() {
        // ordered_modes pushes bypass first, then the cli mode; first valid wins.
        let (mode, _) =
            initial_permission_mode_from_cli(Some("plan"), true, None, false, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn settings_default_mode_used_as_lowest_priority() {
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::AcceptEdits),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(None, false, None, false, &s);
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn killswitch_skips_bypass_and_sets_notice() {
        let s = CliModeSettings {
            default_mode: None,
            bypass_disabled: true,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, None, false, &s);
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(
            notice.as_deref(),
            Some("Bypass permissions mode was disabled by settings")
        );
    }

    #[test]
    fn killswitch_falls_through_to_next_valid_mode() {
        // skip → bypass (disabled, skipped+notice) then cli plan is valid → plan,
        // but notice is carried (TS keeps `notification` across the loop).
        let s = CliModeSettings {
            default_mode: None,
            bypass_disabled: true,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(Some("plan"), true, None, false, &s);
        assert_eq!(mode, PermissionMode::Plan);
        assert_eq!(
            notice.as_deref(),
            Some("Bypass permissions mode was disabled by settings")
        );
    }

    #[test]
    fn no_inputs_is_default_no_notice() {
        let (mode, notice) =
            initial_permission_mode_from_cli(None, false, None, false, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    // ---- MODE-ENV-SCRUB-03 ----

    #[test]
    fn env_scrub_forces_default_and_suppresses_requested_bypass() {
        // A hardened/scrubbed subprocess must NOT inherit --dangerously-skip
        // bypass. Even with the bypass killswitch OFF, env-scrub wins.
        let (mode, notice) =
            initial_permission_mode_from_cli(None, true, None, true, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some(ENV_SCRUB_FORCED_TO_DEFAULT_MSG));
    }

    #[test]
    fn env_scrub_forces_default_over_settings_and_cli_plan() {
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::BypassPermissions),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(Some("plan"), false, None, true, &s);
        assert_eq!(mode, PermissionMode::Default);
        // CLI mode `plan` is non-default → notice fires.
        assert_eq!(notice.as_deref(), Some(ENV_SCRUB_FORCED_TO_DEFAULT_MSG));
    }

    #[test]
    fn env_scrub_no_notice_when_no_non_default_requested() {
        // Nothing non-default requested (no skip, no CLI mode) → silent force.
        let (mode, notice) =
            initial_permission_mode_from_cli(None, false, None, true, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    #[test]
    fn env_scrub_no_notice_when_cli_mode_is_default_or_manual() {
        // `default`/`manual` both normalize to Default → not a non-default
        // request → no notice (klc `i&&i!=="default"`).
        for m in ["default", "manual"] {
            let (mode, notice) =
                initial_permission_mode_from_cli(Some(m), false, None, true, &no_settings());
            assert_eq!(mode, PermissionMode::Default);
            assert!(notice.is_none(), "cli mode {m} should not emit a notice");
        }
    }

    // ---- MODE-FRONTMATTER-04 ----

    #[test]
    fn agent_frontmatter_mode_used_when_no_cli_flag() {
        let (mode, _) = initial_permission_mode_from_cli(
            None,
            false,
            Some(PermissionMode::AcceptEdits),
            false,
            &no_settings(),
        );
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn cli_flag_outranks_agent_frontmatter_mode() {
        // orderedModes: CLI flag pushed before frontmatter → CLI wins.
        let (mode, _) = initial_permission_mode_from_cli(
            Some("plan"),
            false,
            Some(PermissionMode::AcceptEdits),
            false,
            &no_settings(),
        );
        assert_eq!(mode, PermissionMode::Plan);
    }

    #[test]
    fn agent_frontmatter_mode_outranks_settings_default_mode() {
        // orderedModes: frontmatter pushed before settings defaultMode.
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::Plan),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(
            None,
            false,
            Some(PermissionMode::AcceptEdits),
            false,
            &s,
        );
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn skip_outranks_agent_frontmatter_mode() {
        let (mode, _) = initial_permission_mode_from_cli(
            None,
            true,
            Some(PermissionMode::Plan),
            false,
            &no_settings(),
        );
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn agent_frontmatter_non_default_triggers_env_scrub_notice() {
        // env-scrub `y` includes `a&&a!=="default"`.
        let (mode, notice) = initial_permission_mode_from_cli(
            None,
            false,
            Some(PermissionMode::Plan),
            true,
            &no_settings(),
        );
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some(ENV_SCRUB_FORCED_TO_DEFAULT_MSG));
    }

    #[test]
    fn agent_frontmatter_default_does_not_trigger_env_scrub_notice() {
        let (mode, notice) = initial_permission_mode_from_cli(
            None,
            false,
            Some(PermissionMode::Default),
            true,
            &no_settings(),
        );
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    #[test]
    fn env_scrub_message_is_byte_exact_with_lingxi_env_name() {
        // Lock the LINGXI_ env-name divergence + U+2014 em-dash.
        assert_eq!(
            ENV_SCRUB_FORCED_TO_DEFAULT_MSG,
            "Permission mode forced to default \u{2014} LINGXI_SUBPROCESS_ENV_SCRUB is set (allowed_non_write_users hardening). Declare allowedTools explicitly, or set LINGXI_SUBPROCESS_ENV_SCRUB=0 to opt out."
        );
    }

    // ---- MODE-SETTINGS-AUTO-TRUST-01 ----

    #[test]
    fn settings_auto_from_untrusted_tier_is_ignored() {
        // A repo-controllable projectSettings/localSettings `defaultMode: auto`
        // must NOT enter auto mode — it is dropped, leaving Default.
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::Auto),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, false, None, false, &s);
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    #[test]
    fn settings_auto_from_trusted_tier_is_honored() {
        // policy/user/flag tiers may grant auto.
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::Auto),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: true,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(None, false, None, false, &s);
        assert_eq!(mode, PermissionMode::Auto);
    }

    #[test]
    fn settings_external_modes_honored_from_untrusted_tier() {
        // The trust gate applies ONLY to auto; the five external modes may be
        // set from any tier even when auto_default_from_trusted is false.
        for m in [
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::DontAsk,
        ] {
            let s = CliModeSettings {
                default_mode: Some(m),
                bypass_disabled: false,
                auto_mode_disabled: false,
                auto_default_from_trusted: false,
                is_bg_session: false,
                skip_dangerous_mode_permission_prompt: false,
                bypass_permissions_mode_accepted: false,
            };
            let (mode, _) = initial_permission_mode_from_cli(None, false, None, false, &s);
            assert_eq!(mode, m);
        }
    }

    #[test]
    fn untrusted_auto_does_not_shadow_a_cli_flag() {
        // Even if untrusted settings auto is present, a CLI mode still wins and
        // the untrusted auto is simply dropped (never reached).
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::Auto),
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: false,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), false, None, false, &s);
        assert_eq!(mode, PermissionMode::Plan);
    }

    // ---- MODE-BG-DISCLAIMER-02 ----

    /// A background session with the disclaimer NOT yet accepted (gate trips).
    fn bg_gate_tripping() -> CliModeSettings {
        CliModeSettings {
            default_mode: None,
            bypass_disabled: false,
            auto_mode_disabled: false,
            auto_default_from_trusted: false,
            is_bg_session: true,
            skip_dangerous_mode_permission_prompt: false,
            bypass_permissions_mode_accepted: false,
        }
    }

    #[test]
    fn bg_session_downgrades_skip_bypass_to_default() {
        let (mode, notice) =
            initial_permission_mode_from_cli(None, true, None, false, &bg_gate_tripping());
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some(BYPASS_DISCLAIMER_DOWNGRADE_MSG));
    }

    #[test]
    fn bg_session_downgrades_cli_bypass_to_default() {
        let (mode, notice) = initial_permission_mode_from_cli(
            Some("bypassPermissions"),
            false,
            None,
            false,
            &bg_gate_tripping(),
        );
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some(BYPASS_DISCLAIMER_DOWNGRADE_MSG));
    }

    #[test]
    fn bg_session_does_not_downgrade_non_bypass_cli_mode() {
        // The gate only downgrades bypassPermissions; a `plan` CLI mode is
        // untouched even in a bg session.
        let (mode, notice) =
            initial_permission_mode_from_cli(Some("plan"), false, None, false, &bg_gate_tripping());
        assert_eq!(mode, PermissionMode::Plan);
        assert!(notice.is_none());
    }

    #[test]
    fn non_bg_session_keeps_bypass() {
        // Not a bg session → gate never trips → bypass is honored.
        let s = CliModeSettings {
            is_bg_session: false,
            ..bg_gate_tripping()
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, None, false, &s);
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn bg_session_with_skip_dangerous_prompt_keeps_bypass() {
        // skipDangerousModePermissionPrompt set in a tier ⇒ disclaimer already
        // accepted ⇒ no downgrade.
        let s = CliModeSettings {
            skip_dangerous_mode_permission_prompt: true,
            ..bg_gate_tripping()
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, None, false, &s);
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn bg_session_with_accepted_flag_keeps_bypass() {
        // bypassPermissionsModeAccepted persisted ⇒ no downgrade.
        let s = CliModeSettings {
            bypass_permissions_mode_accepted: true,
            ..bg_gate_tripping()
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, None, false, &s);
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn bg_downgrade_message_is_byte_exact() {
        assert_eq!(
            BYPASS_DISCLAIMER_DOWNGRADE_MSG,
            "Permission mode downgraded to default \u{2014} bypass requires accepting the disclaimer interactively first"
        );
    }
}
