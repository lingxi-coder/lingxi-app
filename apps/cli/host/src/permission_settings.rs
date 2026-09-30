use super::init;
use super::permission_mode_preference;
use crate::argv::Argv;

/// Resolve the session permission mode (and any suppression notice) from CLI
/// flags + merged `settings.json` — the shared resolver every dispatch path
/// uses so the one-shot/print, interactive TUI, and stdio-REPL paths all see
/// the SAME `initialPermissionModeFromCLI` result.
///
/// This is `read_cli_mode_settings` + `permission::initial_permission_mode_from_cli`,
/// with NO bypass-safety guard: the guard runs exactly once in [`run_cli`]
/// (before mode dispatch, for all modes), so the interactive paths that call
/// this helper to re-derive the mode must NOT re-run it. Returns
/// `(mode, notice)` where `notice` is `Some` when the bypass killswitch
/// suppressed a requested bypass, OR when the auto-mode availability gate
/// downgraded a requested `auto` (`permissionModeNotification`).
///
/// The auto-mode gate (claude-code `xms` mode-load downgrade + the
/// `kickOutOfAutoIfNeeded` notification) runs AFTER the pure
/// `initialPermissionModeFromCLI` resolution: when the resolved mode is `Auto`
/// but auto mode is unavailable (`disableAutoMode` settings killswitch or the
/// active model does not support it), the mode is downgraded to `Default` and
/// the byte-exact `Jce()` reason (`"auto mode disabled by settings"` /
/// `"auto mode unavailable for this model"`) is surfaced as the startup notice.
/// The local denial circuit-breaker is fresh at boot (never tripped); Statsig
/// remote-disable is a documented omission. The provider is resolved as
/// `"firstParty"` at this CLI surface (multi-provider provider-mapping into the
/// gate is deferred — see [`permission::auto_gate`]).
/// Is `auto` an available Shift+Tab cycle target for this launch?
///
/// `I1(e)` in claude-code 2.1.270 (`src_178976794.js`) is
/// `!!e.isAutoModeAvailable && aC()`, and `sKe`'s `plan` / `bypassPermissions`
/// arms consult it before falling through to `default`. This is the `aC()`
/// half, evaluated from the same inputs [`resolve_permission_mode`] feeds the
/// boot downgrade; the circuit breaker is fresh at boot and the provider is
/// `"firstParty"` at this CLI surface, exactly as there.
pub(crate) fn auto_mode_cycle_available(argv: &Argv) -> bool {
    let settings = read_cli_mode_settings(argv);
    let model = argv
        .model
        .clone()
        .unwrap_or_else(|| harness_runtime::desktop::DesktopConfig::default().default_model);
    permission::auto_mode_available(&permission::AutoGateInputs {
        disabled_by_settings: settings.auto_mode_disabled,
        circuit_broken: false,
        model,
        provider: "firstParty".to_string(),
    })
}

pub(crate) fn resolve_permission_mode(argv: &Argv) -> (permission::PermissionMode, Option<String>) {
    let mut settings = read_cli_mode_settings(argv);
    if let Some(mode) = permission_mode_preference::load(argv) {
        settings.default_mode = Some(mode);
        // A remembered mode is the user's last interactive pick — trusted,
        // same as userSettings for `C(e)`.
        settings.auto_default_from_trusted = mode == permission::PermissionMode::Auto;
        settings.bypass_default_from_trusted =
            mode == permission::PermissionMode::BypassPermissions;
    }
    // MODE-ENV-SCRUB-03: `LINGXI_SUBPROCESS_ENV_SCRUB` (the port's spelling of
    // `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`, `platforms/posix` runner) forces the
    // permission mode to `default` — a hardened / scrubbed subprocess must not
    // inherit a requested bypass/plan/etc.
    let env_scrub_active = platform_api::env::is_env_truthy(
        std::env::var("LINGXI_SUBPROCESS_ENV_SCRUB").ok().as_deref(),
    );
    // MODE-FRONTMATTER-04: the selected main-thread agent's frontmatter
    // `permissionMode` sits between the CLI override and the settings
    // `defaultMode`. The agent catalog is resolved later in
    // `harness_runtime::desktop::build()`, so this early CLI pass cannot see it yet; the
    // composition root re-applies the same precedence once it knows which
    // agent actually won.
    let agent_frontmatter_mode: Option<permission::PermissionMode> = None;
    let (mode, notice) = permission::initial_permission_mode_from_cli(
        argv.permission_mode.as_deref(),
        argv.dangerously_skip_permissions,
        agent_frontmatter_mode,
        env_scrub_active,
        &settings,
    );
    if mode != permission::PermissionMode::Auto {
        return (mode, notice);
    }
    // Auto was requested (CLI flag or settings `defaultMode: auto`). Apply the
    // availability gate; downgrade + notify when closed.
    let model = argv
        .model
        .clone()
        .unwrap_or_else(|| harness_runtime::desktop::DesktopConfig::default().default_model);
    let inputs = permission::AutoGateInputs {
        disabled_by_settings: settings.auto_mode_disabled,
        circuit_broken: false,
        model,
        provider: "firstParty".to_string(),
    };
    match permission::apply_auto_mode_gate(mode, &inputs) {
        (permission::PermissionMode::Auto, _) => (mode, notice),
        (downgraded, Some(reason)) => (downgraded, Some(reason.message().to_string())),
        (downgraded, None) => (downgraded, notice),
    }
}

/// Build [`permission::CliModeSettings`] from the merged user+project
/// `settings.json` files (the bypass-killswitch + settings `defaultMode`
/// inputs the mode resolver reads).
///
/// Reads the user `~/.lingxi/settings.json` then the project
/// `<cwd>/.lingxi/settings.json` raw, deriving the two fields via the existing
/// `permission` helpers: `defaultMode` takes project-wins precedence (project
/// read last), and the bypass-disable killswitch is sticky (set by any tier).
/// On any load failure (missing/unreadable/malformed file) it degrades to the
/// no-op default `{ default_mode: None, bypass_disabled: false }` — a faithful
/// port of TS `getSettings_DEPRECATED() || {}`.
///
/// Explicit `--settings` is read after ambient files so it remains effective
/// in restricted mode even while user/project/local files are suppressed.
pub(crate) fn read_cli_mode_settings(parsed: &Argv) -> permission::CliModeSettings {
    // `--setting-sources <user,project,local>` gates which settings files this
    // permission-mode reader consults too (claude scopes ALL settings loading,
    // not just providers/routing). `None` ⟶ both layers (default).
    let (incl_user, incl_project) = if parsed.restricted_enabled() {
        (false, false)
    } else {
        init::setting_source_flags(parsed.setting_sources.as_deref())
    };
    let project_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut default_mode = None;
    let mut bypass_disabled = false;
    let mut auto_mode_disabled = false;
    // MODE-SETTINGS-AUTO-TRUST-01: track whether a TRUSTED tier declared
    // `defaultMode: auto`. At this CLI surface only the user (`~/.lingxi`) tier
    // is trusted; the project (`.lingxi`) tier is repo-controllable. When the
    // merged `default_mode` ends up `auto` but no trusted tier granted it, the
    // resolver drops it (a committed project settings file cannot enable
    // classifier-driven auto-accept mode).
    let mut auto_default_from_trusted = false;
    // 2.1.257 `C("bypassPermissions")`: sticky true if ANY trusted tier
    // declared bypass. Project/local cannot grant it.
    let mut bypass_default_from_trusted = false;
    // MODE-BG-DISCLAIMER-02: sticky across tiers (any tier accepting wins — `Pq()`).
    let mut skip_dangerous_mode_permission_prompt = false;
    let home = incl_user
        .then(|| crate::run::lingxi_home_dir().join("settings.json"))
        .map(|p| {
            (
                p,
                permission::PermissionRuleSource::Settings(protocol::SettingsScope::User),
            )
        });
    let proj = incl_project
        .then(|| project_dir.join(branding::DOT_DIR).join("settings.json"))
        .map(|p| {
            (
                p,
                permission::PermissionRuleSource::Settings(protocol::SettingsScope::Project),
            )
        });
    // User first, then project (ascending priority): project read last wins on
    // `defaultMode`; `bypass_disabled` / `auto_mode_disabled` are sticky across
    // tiers (any tier disabling wins — `Bpa()`).
    for (path, source) in [home, proj].into_iter().flatten() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(m) = permission::default_mode_from_settings_json(&raw) {
                default_mode = Some(m);
                if permission::loader::auto_mode_grantable_by_source(source) {
                    if m == permission::PermissionMode::Auto {
                        auto_default_from_trusted = true;
                    }
                    if m == permission::PermissionMode::BypassPermissions {
                        bypass_default_from_trusted = true;
                    }
                }
            }
            if permission::bypass_permissions_disabled_from_settings_json(&raw) {
                bypass_disabled = true;
            }
            if permission::auto_mode_disabled_from_settings_json(&raw) {
                auto_mode_disabled = true;
            }
            // `Pq()` reads `skipDangerousModePermissionPrompt` from
            // {userSettings, localSettings, flagSettings, policySettings} —
            // DELIBERATELY EXCLUDING projectSettings, so a repo-controllable
            // `.lingxi/settings.json` cannot suppress the bg-bypass disclaimer
            // downgrade (same repo-trust threat MODE-SETTINGS-AUTO-TRUST-01
            // guards). Only the user tier is loaded here; local/flag/policy are
            // not read at this surface (their omission is over-ask-safe).
            if source
                != permission::PermissionRuleSource::Settings(protocol::SettingsScope::Project)
                && permission::loader::skip_dangerous_mode_permission_prompt_from_settings_json(
                    &raw,
                )
            {
                skip_dangerous_mode_permission_prompt = true;
            }
        }
    }
    if let Some(raw) = parsed
        .settings
        .as_deref()
        .and_then(|_| init::parse_flag_settings(parsed.settings.as_deref()))
        .and_then(|settings| serde_json::to_string(&settings).ok())
    {
        let source = permission::PermissionRuleSource::FlagSettings;
        if let Some(m) = permission::default_mode_from_settings_json(&raw) {
            default_mode = Some(m);
            if permission::loader::auto_mode_grantable_by_source(source) {
                if m == permission::PermissionMode::Auto {
                    auto_default_from_trusted = true;
                }
                if m == permission::PermissionMode::BypassPermissions {
                    bypass_default_from_trusted = true;
                }
            }
        }
        if permission::bypass_permissions_disabled_from_settings_json(&raw) {
            bypass_disabled = true;
        }
        if permission::auto_mode_disabled_from_settings_json(&raw) {
            auto_mode_disabled = true;
        }
        if permission::loader::skip_dangerous_mode_permission_prompt_from_settings_json(&raw) {
            skip_dangerous_mode_permission_prompt = true;
        }
    }
    // MODE-BG-DISCLAIMER-02: bg-session downgrade inputs. `is_bg_session` is
    // `LINGXI_SESSION_KIND == "bg"` (claude-code `CLAUDE_CODE_SESSION_KIND`);
    // `bypass_permissions_mode_accepted` is the persisted global-config flag
    // (`St().bypassPermissionsModeAccepted`), read best-effort (absent ⇒ false,
    // i.e. the gate may trip — over-ask safe).
    let is_bg_session = std::env::var("LINGXI_SESSION_KIND").ok().as_deref() == Some("bg");
    let bypass_permissions_mode_accepted = migrations::global_config::global_config_path()
        .and_then(|p| migrations::global_config::read_map(&p).ok())
        .and_then(|m| {
            m.get("bypassPermissionsModeAccepted")
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false);
    permission::CliModeSettings {
        default_mode,
        bypass_disabled,
        auto_mode_disabled,
        auto_default_from_trusted,
        bypass_default_from_trusted,
        is_bg_session,
        skip_dangerous_mode_permission_prompt,
        bypass_permissions_mode_accepted,
    }
}
