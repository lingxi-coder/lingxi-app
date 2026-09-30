//! `/fusion setup` persistence: locate `~/.lingxi/settings.json` and merge the
//! chosen model roles into it without disturbing anything else in the file.
//!
//! Mirrors [`crate::web::persist`] — the async orchestration (and the result
//! notice) lives in the CLI composition root, which calls these.

use lingxi_core::host::fusion_setup::FusionModelRoles;

/// `~/.lingxi/settings.json` — the same file `/web`, `/config` and the startup
/// settings load read and write.
#[must_use]
pub fn fusion_settings_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| memory::lingxi_md::user_config_dir(&home).join("settings.json"))
}

/// Merge `roles` (and, when `enable` is `Some`, `fusion.enabled`) into the JSON
/// object at `path`, then write it back pretty-printed with a trailing newline.
///
/// Unrelated keys — including other `fusion.*` settings — are preserved: this
/// is a read-modify-write of one settings file a human also edits by hand.
///
/// # Errors
///
/// Any IO error from creating the parent directory or writing the file.
pub fn save_fusion_settings_to(
    path: &std::path::Path,
    roles: &FusionModelRoles,
    enable: Option<bool>,
) -> std::io::Result<()> {
    // A file that exists but does not parse is deliberately treated as `{}`
    // rather than an error — the same choice `save_web_settings_to` makes.
    let mut value: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    roles.write_settings_json(&mut value);
    if let Some(enable) = enable {
        lingxi_core::host::fusion_setup::write_enabled_settings_json(&mut value, enable);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&value)?;
    body.push('\n');
    std::fs::write(path, body)
}

/// Read the currently configured roles (and the master switch) from `path`.
/// A missing or unparseable file reads as "nothing configured".
#[must_use]
pub fn load_fusion_settings_from(path: &std::path::Path) -> (FusionModelRoles, bool) {
    let value: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    (
        FusionModelRoles::from_settings_json(&value),
        lingxi_core::host::fusion_setup::enabled_from_settings_json(&value),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lingxi_core::host::FusionModelChoice;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lingxi-fusion-persist-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn roles() -> FusionModelRoles {
        FusionModelRoles {
            panels: vec![
                FusionModelChoice::new("anthropic", "claude-opus-5"),
                FusionModelChoice::new("openai", "gpt-5.6-sol"),
            ],
            analyst: Some(FusionModelChoice::new("openai", "gpt-5.6-terra")),
        }
    }

    #[test]
    fn a_save_round_trips_and_keeps_unrelated_settings() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","fusion":{"qualityPanelCount":3},"permissions":{"allow":["Bash"]}}"#,
        )
        .unwrap();
        save_fusion_settings_to(&path, &roles(), Some(true)).unwrap();

        let (read_roles, enabled) = load_fusion_settings_from(&path);
        assert_eq!(read_roles, roles());
        assert!(enabled);
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["theme"], serde_json::json!("dark"));
        assert_eq!(raw["permissions"]["allow"], serde_json::json!(["Bash"]));
        assert_eq!(raw["fusion"]["qualityPanelCount"], serde_json::json!(3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_roles_without_an_enable_decision_leaves_the_switch_untouched() {
        let dir = temp_dir("switch");
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"fusion":{"enabled":true}}"#).unwrap();
        save_fusion_settings_to(&path, &roles(), None).unwrap();
        assert!(load_fusion_settings_from(&path).1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A settings file a human has broken must not silently lose its contents
    /// AND must not stop the wizard from writing a working configuration; the
    /// engine's own loader reports the parse error separately.
    #[test]
    fn an_unparseable_file_is_replaced_rather_than_failing_the_save() {
        let dir = temp_dir("broken");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        save_fusion_settings_to(&path, &roles(), Some(false)).unwrap();
        let (read_roles, enabled) = load_fusion_settings_from(&path);
        assert_eq!(read_roles, roles());
        assert!(!enabled);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_and_a_missing_directory_are_both_created() {
        let dir = temp_dir("missing");
        let path = dir.join("nested").join("settings.json");
        save_fusion_settings_to(&path, &roles(), None).unwrap();
        assert_eq!(load_fusion_settings_from(&path).0, roles());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_was_never_written_reads_as_nothing_configured() {
        let (roles, enabled) =
            load_fusion_settings_from(std::path::Path::new("/nonexistent/lingxi/settings.json"));
        assert!(roles.is_untouched());
        assert!(!enabled);
    }
}
