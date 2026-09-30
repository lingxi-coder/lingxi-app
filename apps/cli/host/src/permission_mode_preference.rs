//! The CLI's last user-selected mode is a device preference, not a permission
//! grant. In particular, remembering bypass never authorizes a future launch.
use crate::Argv;
use permission::PermissionMode;
use std::path::Path;

const KEY: &str = "cliLastPermissionMode";

fn parse(value: &str) -> Option<PermissionMode> {
    match value {
        "default" | "acceptEdits" | "plan" | "auto" | "dontAsk" | "bypassPermissions" => {
            Some(permission::permission_mode_from_cli_string(value))
        }
        _ => None,
    }
}

pub(crate) fn load(argv: &Argv) -> Option<PermissionMode> {
    if argv.restricted_enabled()
        || lingxi_core::host::env::is_env_truthy(
            std::env::var("LINGXI_SUBPROCESS_ENV_SCRUB").ok().as_deref(),
        )
        || !crate::init::setting_source_flags(argv.setting_sources.as_deref()).0
    {
        return None;
    }
    if crate::init::parse_flag_settings(argv.settings.as_deref())
        .and_then(|settings| serde_json::to_string(&settings).ok())
        .is_some_and(|raw| permission::default_mode_from_settings_json(&raw).is_some())
    {
        return None;
    }
    let path = migrations::global_config::global_config_path()?;
    load_at(
        &path,
        argv.allow_dangerously_skip_permissions || argv.dangerously_skip_permissions,
    )
}

fn load_at(path: &Path, bypass_authorized: bool) -> Option<PermissionMode> {
    let config = migrations::global_config::read_map(path).ok()?;
    let mode = parse(config.get(KEY)?.as_str()?)?;
    if mode == PermissionMode::BypassPermissions && !bypass_authorized {
        return None;
    }
    Some(mode)
}

/// Call only after the live gate accepted a user-requested mode change.
/// A disk failure must not pretend that the already-applied mode was rejected.
pub(crate) fn remember(mode: &str) {
    let Some(path) = migrations::global_config::global_config_path() else {
        return;
    };
    if let Err(error) = remember_at(&path, mode) {
        tracing::warn!(%error, "could not save the CLI permission mode preference");
    }
}

fn remember_at(path: &Path, mode: &str) -> Result<(), String> {
    let mode = parse(mode).ok_or_else(|| format!("Invalid permission mode: {mode}"))?;
    migrations::global_config::save_map(path, |mut config| {
        config.insert(
            KEY.to_string(),
            serde_json::Value::String(mode.wire_str().to_string()),
        );
        config
    })
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_and_explicit_settings_do_not_load_device_preferences() {
        assert_eq!(
            load(&Argv {
                restricted: true,
                ..Argv::default()
            }),
            None
        );
        assert_eq!(
            load(&Argv {
                setting_sources: Some("project".into()),
                ..Argv::default()
            }),
            None
        );
        assert_eq!(
            load(&Argv {
                settings: Some(r#"{"permissions":{"defaultMode":"plan"}}"#.into()),
                ..Argv::default()
            }),
            None
        );
    }

    #[test]
    fn last_selection_round_trips_and_preserves_other_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"other":true}"#).unwrap();
        for mode in ["acceptEdits", "plan", "auto", "dontAsk", "default"] {
            remember_at(&path, mode).unwrap();
            assert_eq!(load_at(&path, false), parse(mode));
        }
        assert_eq!(
            migrations::global_config::read_map(&path).unwrap()["other"],
            true
        );
    }

    #[test]
    fn remembered_bypass_requires_launch_authorization() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        remember_at(&path, "bypassPermissions").unwrap();
        assert_eq!(load_at(&path, false), None);
        assert_eq!(
            load_at(&path, true),
            Some(PermissionMode::BypassPermissions)
        );
    }

    #[test]
    fn invalid_selection_cannot_overwrite_last_successful_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        remember_at(&path, "plan").unwrap();
        assert!(remember_at(&path, "invalid").is_err());
        assert_eq!(load_at(&path, false), Some(PermissionMode::Plan));
        std::fs::write(&path, "broken").unwrap();
        assert_eq!(load_at(&path, true), None);
        assert!(remember_at(&path, "default").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }
}
