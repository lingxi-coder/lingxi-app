//! Pure conversion between the shared strategy catalog and the setup wizard.
//! Writes go through configuration-admin's revision-checked transaction in the host.

use lingxi_core::host::fusion_setup::FusionModelRoles;
use lingxi_core::host::FusionStrategyRef;
use lingxi_core::settings::schema::FusionStrategy;
use serde_json::Value;

use super::setup::{FusionSettingsSnapshot, FusionSetupEntry, FusionSetupScope};

/// Decode the shared catalog; no legacy singleton configuration is admitted.
pub fn snapshot_from_catalog(catalog: &str) -> Result<FusionSettingsSnapshot, String> {
    let value: Value = serde_json::from_str(catalog).map_err(|error| error.to_string())?;
    let scopes = value
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or("Fusion catalog is missing scopes")?;
    let mut snapshot = FusionSettingsSnapshot::default();
    snapshot.enabled = value["effective_globals"]["enabled"]
        .as_bool()
        .unwrap_or(false);
    for layer in scopes {
        let label = layer["scope"].as_str().ok_or("Fusion scope is missing")?;
        let reference: FusionStrategyRef = format!("{label}:scope")
            .parse()
            .map_err(|error| format!("{error}"))?;
        let revision = layer["revision"]
            .as_str()
            .ok_or("Fusion revision is missing")?
            .to_string();
        snapshot.scopes.push(FusionSetupScope {
            scope: reference.scope,
            revision: revision.clone(),
            enabled: layer["globals"]["enabled"].as_bool(),
            error: layer["diagnostic"].as_str().map(str::to_owned),
        });
        for raw in layer["strategies"]
            .as_array()
            .ok_or("Fusion strategies are missing")?
        {
            let strategy: FusionStrategy =
                serde_json::from_value(raw.clone()).map_err(|error| error.to_string())?;
            let reference = FusionStrategyRef {
                scope: reference.scope,
                id: strategy.id,
            };
            let roles = FusionModelRoles::from_strategy_config(&strategy.config);
            if serde_json::to_value(&reference).map_err(|error| error.to_string())?
                == value["default_strategy"]
            {
                snapshot.roles = roles.clone();
                snapshot.max_panel = strategy
                    .config
                    .max_panel
                    .unwrap_or(lingxi_core::host::FUSION_MAX_PANEL);
            }
            snapshot.entries.push(FusionSetupEntry {
                max_panel: strategy
                    .config
                    .max_panel
                    .unwrap_or(lingxi_core::host::FUSION_MAX_PANEL),
                is_default: layer["default_strategy_id"].as_str() == Some(&reference.id),
                reference,
                name: strategy.name,
                roles,
                revision: revision.clone(),
            });
        }
    }
    Ok(snapshot)
}

/// Create the atomic save payload while retaining advanced settings on edits.
/// A fresh catalog read does not replace the wizard's expected revision; CAS
/// still rejects changes made since the wizard opened.
pub fn save_payload(
    catalog: &str,
    reference: &FusionStrategyRef,
    name: &str,
    roles: &FusionModelRoles,
    enable: Option<bool>,
    make_default: bool,
) -> Result<Value, String> {
    let catalog: Value = serde_json::from_str(catalog).map_err(|error| error.to_string())?;
    let scope = super::setup::scope_label(reference.scope);
    let registry = catalog["scopes"]
        .as_array()
        .and_then(|scopes| scopes.iter().find(|layer| layer["scope"] == scope))
        .ok_or("Fusion scope is missing")?;
    let mut config = registry["strategies"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["id"] == reference.id))
        .map(|entry| serde_json::from_value(entry["config"].clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    roles.write_strategy_config(&mut config);
    let mut payload = serde_json::json!({"id":reference.id,"name":name,"config":config,"make_default":make_default});
    if let Some(enabled) = enable {
        payload["enabled"] = Value::Bool(enabled);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> String {
        serde_json::json!({"effective_globals":{"enabled":true},"default_strategy":{"scope":"project","id":"review"},"scopes":[
            {"scope":"user","revision":"user-rev","globals":{},"strategies":[{"id":"review","name":"Review","config":{}}]},
            {"scope":"project","revision":"project-rev","default_strategy_id":"review","globals":{"enabled":true},"strategies":[{"id":"review","name":"Review","config":{"maxPanel":4,"panelTotalTimeoutMs":1234}}]},
            {"scope":"local","revision":"local-rev","globals":{},"diagnostic":"legacy format","strategies":[]}
        ]}).to_string()
    }

    #[test]
    fn catalog_preserves_scope_identity_revisions_and_diagnostics() {
        let snapshot = snapshot_from_catalog(&catalog()).unwrap();
        assert_eq!(snapshot.entries.len(), 2);
        assert_ne!(snapshot.entries[0].reference, snapshot.entries[1].reference);
        assert_eq!(snapshot.entries[1].revision, "project-rev");
        assert!(snapshot.entries[1].is_default);
        assert_eq!(snapshot.scopes[2].error.as_deref(), Some("legacy format"));
        assert_eq!(snapshot.max_panel, 4);
    }

    #[test]
    fn setup_save_preserves_advanced_settings_and_explicitly_clears_default() {
        let payload = save_payload(
            &catalog(),
            &"project:review".parse().unwrap(),
            "Renamed",
            &FusionModelRoles::default(),
            Some(true),
            false,
        )
        .unwrap();
        assert_eq!(payload["config"]["maxPanel"], 4);
        assert_eq!(payload["config"]["panelTotalTimeoutMs"], 1234);
        assert_eq!(payload["make_default"], false);
        assert_eq!(payload["name"], "Renamed");
    }

    #[test]
    fn inherited_scope_switch_remains_absent_from_an_untouched_save() {
        let snapshot = snapshot_from_catalog(&catalog()).unwrap();
        assert_eq!(snapshot.scopes[0].enabled, None);
        let payload = save_payload(
            &catalog(),
            &"user:review".parse().unwrap(),
            "Renamed",
            &FusionModelRoles::default(),
            None,
            false,
        )
        .unwrap();
        assert!(payload.get("enabled").is_none());
        let disabled = save_payload(
            &catalog(),
            &"user:review".parse().unwrap(),
            "Renamed",
            &FusionModelRoles::default(),
            Some(false),
            false,
        )
        .unwrap();
        assert_eq!(disabled["enabled"], false);
    }
}
