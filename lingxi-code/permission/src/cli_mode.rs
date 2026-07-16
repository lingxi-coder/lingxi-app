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

/// `initialPermissionModeFromCLI` (`permissionSetup.ts:689-812`): resolve the
/// session permission mode from CLI flags + settings, returning the mode plus
/// an optional user-facing notice (set when the bypass killswitch suppresses a
/// requested bypass, or a hardening/downgrade path forces the mode).
///
/// `env_scrub_active` is `isEnvTruthy(LINGXI_SUBPROCESS_ENV_SCRUB)` — the
/// `allowed_non_write_users` subprocess-env-scrub hardening flag. When set, the
/// resolver short-circuits to [`PermissionMode::Default`] BEFORE any other
/// input (`MODE-ENV-SCRUB-03` / claude-code `klc`'s leading
/// `if(ut(r.CLAUDE_CODE_SUBPROCESS_ENV_SCRUB))` guard), emitting the notice iff
/// a non-default mode was actually requested (skip flag or a CLI mode other
/// than default).
#[must_use]
pub fn initial_permission_mode_from_cli(
    permission_mode_cli: Option<&str>,
    dangerously_skip: bool,
    env_scrub_active: bool,
    settings: &CliModeSettings,
) -> (PermissionMode, Option<String>) {
    let cli_mode = permission_mode_cli.map(permission_mode_from_cli_string);

    // MODE-ENV-SCRUB-03: env-scrub hardening forces mode to default before any
    // other input is consulted. The notice fires only when a non-default mode
    // was requested (skip flag, or a CLI mode that isn't `default`), matching
    // klc's `y=s||i&&i!=="default"||a&&a!=="default"`.
    if env_scrub_active {
        let requested_non_default =
            dangerously_skip || cli_mode.is_some_and(|m| m != PermissionMode::Default);
        let notice = requested_non_default.then(|| ENV_SCRUB_FORCED_TO_DEFAULT_MSG.to_string());
        return (PermissionMode::Default, notice);
    }

    // Modes in order of priority (TS `orderedModes`).
    let mut ordered: Vec<PermissionMode> = Vec::new();
    if dangerously_skip {
        ordered.push(PermissionMode::BypassPermissions);
    }
    if let Some(cli) = cli_mode {
        ordered.push(cli);
    }
    if let Some(default_mode) = settings.default_mode {
        ordered.push(default_mode);
    }

    let mut notification: Option<String> = None;
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
        let (mode, notice) = initial_permission_mode_from_cli(None, true, false, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn cli_flag_used_when_no_skip() {
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), false, false, &no_settings());
        assert_eq!(mode, PermissionMode::Plan);
    }

    #[test]
    fn skip_outranks_cli_flag() {
        // ordered_modes pushes bypass first, then the cli mode; first valid wins.
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), true, false, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn settings_default_mode_used_as_lowest_priority() {
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::AcceptEdits),
            bypass_disabled: false,
            auto_mode_disabled: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(None, false, false, &s);
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn killswitch_skips_bypass_and_sets_notice() {
        let s = CliModeSettings {
            default_mode: None,
            bypass_disabled: true,
            auto_mode_disabled: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, false, &s);
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
        };
        let (mode, notice) = initial_permission_mode_from_cli(Some("plan"), true, false, &s);
        assert_eq!(mode, PermissionMode::Plan);
        assert_eq!(
            notice.as_deref(),
            Some("Bypass permissions mode was disabled by settings")
        );
    }

    #[test]
    fn no_inputs_is_default_no_notice() {
        let (mode, notice) = initial_permission_mode_from_cli(None, false, false, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    // ---- MODE-ENV-SCRUB-03 ----

    #[test]
    fn env_scrub_forces_default_and_suppresses_requested_bypass() {
        // A hardened/scrubbed subprocess must NOT inherit --dangerously-skip
        // bypass. Even with the bypass killswitch OFF, env-scrub wins.
        let (mode, notice) = initial_permission_mode_from_cli(None, true, true, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert_eq!(notice.as_deref(), Some(ENV_SCRUB_FORCED_TO_DEFAULT_MSG));
    }

    #[test]
    fn env_scrub_forces_default_over_settings_and_cli_plan() {
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::BypassPermissions),
            bypass_disabled: false,
            auto_mode_disabled: false,
        };
        let (mode, notice) =
            initial_permission_mode_from_cli(Some("plan"), false, true, &s);
        assert_eq!(mode, PermissionMode::Default);
        // CLI mode `plan` is non-default → notice fires.
        assert_eq!(notice.as_deref(), Some(ENV_SCRUB_FORCED_TO_DEFAULT_MSG));
    }

    #[test]
    fn env_scrub_no_notice_when_no_non_default_requested() {
        // Nothing non-default requested (no skip, no CLI mode) → silent force.
        let (mode, notice) = initial_permission_mode_from_cli(None, false, true, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }

    #[test]
    fn env_scrub_no_notice_when_cli_mode_is_default_or_manual() {
        // `default`/`manual` both normalize to Default → not a non-default
        // request → no notice (klc `i&&i!=="default"`).
        for m in ["default", "manual"] {
            let (mode, notice) =
                initial_permission_mode_from_cli(Some(m), false, true, &no_settings());
            assert_eq!(mode, PermissionMode::Default);
            assert!(notice.is_none(), "cli mode {m} should not emit a notice");
        }
    }

    #[test]
    fn env_scrub_message_is_byte_exact_with_lingxi_env_name() {
        // Lock the LINGXI_ env-name divergence + U+2014 em-dash.
        assert_eq!(
            ENV_SCRUB_FORCED_TO_DEFAULT_MSG,
            "Permission mode forced to default \u{2014} LINGXI_SUBPROCESS_ENV_SCRUB is set (allowed_non_write_users hardening). Declare allowedTools explicitly, or set LINGXI_SUBPROCESS_ENV_SCRUB=0 to opt out."
        );
    }
}
