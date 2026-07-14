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

/// `initialPermissionModeFromCLI` (`permissionSetup.ts:689-812`): resolve the
/// session permission mode from CLI flags + settings, returning the mode plus
/// an optional user-facing notice (set when the bypass killswitch suppresses a
/// requested bypass).
#[must_use]
pub fn initial_permission_mode_from_cli(
    permission_mode_cli: Option<&str>,
    dangerously_skip: bool,
    settings: &CliModeSettings,
) -> (PermissionMode, Option<String>) {
    // Modes in order of priority (TS `orderedModes`).
    let mut ordered: Vec<PermissionMode> = Vec::new();
    if dangerously_skip {
        ordered.push(PermissionMode::BypassPermissions);
    }
    if let Some(cli) = permission_mode_cli {
        ordered.push(permission_mode_from_cli_string(cli));
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
        let (mode, notice) = initial_permission_mode_from_cli(None, true, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(notice.is_none());
    }

    #[test]
    fn cli_flag_used_when_no_skip() {
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), false, &no_settings());
        assert_eq!(mode, PermissionMode::Plan);
    }

    #[test]
    fn skip_outranks_cli_flag() {
        // ordered_modes pushes bypass first, then the cli mode; first valid wins.
        let (mode, _) = initial_permission_mode_from_cli(Some("plan"), true, &no_settings());
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn settings_default_mode_used_as_lowest_priority() {
        let s = CliModeSettings {
            default_mode: Some(PermissionMode::AcceptEdits),
            bypass_disabled: false,
            auto_mode_disabled: false,
        };
        let (mode, _) = initial_permission_mode_from_cli(None, false, &s);
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[test]
    fn killswitch_skips_bypass_and_sets_notice() {
        let s = CliModeSettings {
            default_mode: None,
            bypass_disabled: true,
            auto_mode_disabled: false,
        };
        let (mode, notice) = initial_permission_mode_from_cli(None, true, &s);
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
        let (mode, notice) = initial_permission_mode_from_cli(Some("plan"), true, &s);
        assert_eq!(mode, PermissionMode::Plan);
        assert_eq!(
            notice.as_deref(),
            Some("Bypass permissions mode was disabled by settings")
        );
    }

    #[test]
    fn no_inputs_is_default_no_notice() {
        let (mode, notice) = initial_permission_mode_from_cli(None, false, &no_settings());
        assert_eq!(mode, PermissionMode::Default);
        assert!(notice.is_none());
    }
}
