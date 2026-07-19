//! Managed plugin marketplace policy helpers shared by the CLI plugin flows.
//!
//! CC 2.1.215 treats `blockedMarketplaces` as a managed-only setting and
//! rejects add / install / update / enable operations that target a blocked
//! marketplace. The plugin CLI commands are synchronous, so this module mirrors
//! the managed-tier file walk directly instead of depending on the async engine
//! watcher helper.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The managed settings root, honoring the same test override the engine uses.
fn managed_settings_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LINGXI_MANAGED_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from(branding::MANAGED_DIR_MACOS)
    } else if cfg!(target_os = "windows") {
        PathBuf::from(branding::MANAGED_DIR_WINDOWS)
    } else {
        PathBuf::from(branding::MANAGED_DIR_UNIX)
    }
}

/// Read the raw managed settings tiers in ascending priority, matching the
/// engine's `managed_settings_raw_tiers()`: base file first, then sorted
/// `managed-settings.d/*.json` drop-ins (dotfiles skipped).
fn managed_settings_raw_tiers() -> Vec<String> {
    let managed = managed_settings_dir();
    let mut out = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(managed.join("managed-settings.json")) {
        out.push(raw);
    }
    let drop_in = managed.join("managed-settings.d");
    if let Ok(rd) = std::fs::read_dir(&drop_in) {
        let mut names: Vec<std::ffi::OsString> = rd
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| {
                let n = name.to_string_lossy();
                n.ends_with(".json") && !n.starts_with('.')
            })
            .collect();
        names.sort();
        for name in names {
            if let Ok(raw) = std::fs::read_to_string(drop_in.join(name)) {
                out.push(raw);
            }
        }
    }
    out
}

/// Fold the managed `blockedMarketplaces` setting (last-write-wins by tier).
#[must_use]
pub fn blocked_marketplaces() -> BTreeSet<String> {
    let mut blocked = BTreeSet::new();
    for raw in managed_settings_raw_tiers() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(entries) = value.get("blockedMarketplaces").and_then(|v| v.as_array()) else {
            continue;
        };
        blocked = entries
            .iter()
            .filter_map(|entry| entry.as_str())
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    blocked
}

/// Reject operations targeting a managed-blocked marketplace.
pub fn ensure_marketplace_allowed(marketplace: &str) -> Result<(), String> {
    if blocked_marketplaces().contains(marketplace) {
        Err(format!(
            "Marketplace '{marketplace}' is blocked by managed settings"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn blocked_marketplaces_uses_last_managed_tier() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let drop_in = tmp.path().join("managed-settings.d");
        std::fs::create_dir_all(&drop_in).unwrap();
        std::fs::write(
            tmp.path().join("managed-settings.json"),
            r#"{"blockedMarketplaces":["alpha","beta"]}"#,
        )
        .unwrap();
        std::fs::write(
            drop_in.join("20-org.json"),
            r#"{"blockedMarketplaces":["gamma"]}"#,
        )
        .unwrap();

        std::env::set_var("LINGXI_MANAGED_DIR", tmp.path());
        let blocked = blocked_marketplaces();
        std::env::remove_var("LINGXI_MANAGED_DIR");

        assert_eq!(
            blocked,
            BTreeSet::from(["gamma".to_string()]),
            "later managed tiers should override earlier blockedMarketplaces arrays"
        );
    }
}
