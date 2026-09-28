use super::*;
use std::sync::{Mutex, OnceLock};

/// `read_cli_mode_settings` reads process-global state (`$HOME` via
/// `dirs::home_dir` + the process cwd), so the two tests that mutate those
/// must not run concurrently. A local mutex serializes them (the rest of the
/// resolver is pure and tested env-free in `permission::cli_mode`).
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn argv() -> Argv {
    Argv::default()
}

/// With no `~/.lingxi/settings.json` and no `<cwd>/.lingxi/settings.json`,
/// the helper degrades to the no-op default (the faithful TS
/// `getSettings_DEPRECATED() || {}` fallback).
#[test]
fn degrades_to_default_when_no_settings_files() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    // Point HOME + cwd at fresh empty dirs (no `.lingxi/settings.json`).
    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let s = read_cli_mode_settings(&argv());
    assert!(s.default_mode.is_none());
    assert!(!s.bypass_disabled);

    // Restore process-global state.
    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// The project `<cwd>/.lingxi/settings.json` `defaultMode` is read and wins
/// over the user tier, and `disableBypassPermissionsMode: "disable"` sets the
/// killswitch — exercising the parse path, not just the empty degrade.
#[test]
fn reads_project_settings_default_mode_and_killswitch() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    let proj_lingxi = proj.path().join(".lingxi");
    std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
    std::fs::write(
        proj_lingxi.join("settings.json"),
        r#"{"permissions":{"defaultMode":"acceptEdits","disableBypassPermissionsMode":"disable"}}"#,
    )
    .expect("write settings");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let s = read_cli_mode_settings(&argv());
    assert_eq!(
        s.default_mode,
        Some(permission::PermissionMode::AcceptEdits)
    );
    assert!(s.bypass_disabled);
    assert!(!s.bypass_default_from_trusted);

    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// 2.1.257: a project `.lingxi/settings.json` `defaultMode: bypassPermissions`
/// is recorded as the merged defaultMode but is NOT a trusted grant.
#[test]
fn project_bypass_default_mode_is_not_a_trusted_grant() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    let proj_lingxi = proj.path().join(".lingxi");
    std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
    std::fs::write(
        proj_lingxi.join("settings.json"),
        r#"{"permissions":{"defaultMode":"bypassPermissions"}}"#,
    )
    .expect("write settings");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let s = read_cli_mode_settings(&argv());
    assert_eq!(
        s.default_mode,
        Some(permission::PermissionMode::BypassPermissions)
    );
    assert!(!s.bypass_default_from_trusted);
    let (mode, notice) = resolve_permission_mode(&argv());
    assert_eq!(mode, permission::PermissionMode::Default);
    assert!(notice.is_none());

    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// A settings `disableAutoMode: "disable"` (either position) sets the
/// auto-mode killswitch in the resolved [`permission::CliModeSettings`].
#[test]
fn reads_auto_mode_killswitch() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    let proj_lingxi = proj.path().join(".lingxi");
    std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
    std::fs::write(
        proj_lingxi.join("settings.json"),
        r#"{"permissions":{"disableAutoMode":"disable"}}"#,
    )
    .expect("write settings");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let s = read_cli_mode_settings(&argv());
    assert!(s.auto_mode_disabled);

    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// `--permission-mode auto` + `disableAutoMode: "disable"` → the session
/// boots `Default` with the byte-exact `auto mode disabled by settings`
/// notice (claude-code `xms` downgrade + `Jce("settings")`).
#[test]
fn resolve_downgrades_auto_when_disabled_by_settings() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    let proj_lingxi = proj.path().join(".lingxi");
    std::fs::create_dir_all(&proj_lingxi).expect("mkdir .lingxi");
    std::fs::write(
        proj_lingxi.join("settings.json"),
        r#"{"disableAutoMode":"disable"}"#,
    )
    .expect("write settings");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let mut a = argv();
    a.permission_mode = Some("auto".to_string());
    let (mode, notice) = resolve_permission_mode(&a);
    assert_eq!(mode, permission::PermissionMode::Default);
    assert_eq!(notice.as_deref(), Some("auto mode disabled by settings"));

    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}

/// `--permission-mode auto` on an auto-UNSUPPORTED model (no settings) →
/// boots `Default` with `auto mode unavailable for this model`
/// (`Jce("model")`, the `dUe` deny-list).
#[test]
fn resolve_downgrades_auto_on_unsupported_model() {
    let _g = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prior_home = std::env::var_os("HOME");
    let prior_cwd = std::env::current_dir().ok();

    // Empty settings dirs (no killswitch) — the ONLY closed gate is the model.
    let home = tempfile::tempdir().expect("home tempdir");
    let proj = tempfile::tempdir().expect("proj tempdir");
    std::env::set_var("HOME", home.path());
    std::env::set_current_dir(proj.path()).expect("chdir proj");

    let mut a = argv();
    a.permission_mode = Some("auto".to_string());
    a.model = Some("claude-sonnet-4-5".to_string()); // legacy → auto-unsupported
    let (mode, notice) = resolve_permission_mode(&a);
    assert_eq!(mode, permission::PermissionMode::Default);
    assert_eq!(
        notice.as_deref(),
        Some("auto mode unavailable for this model")
    );

    if let Some(cwd) = prior_cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    match prior_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
}
