//! Host-wide provider settings, independent of project sessions.
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::io::Read;
use std::path::Path;

const FIELDS: [&str; 3] = ["providers", "routing", "providerRegion"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub patch: Option<Map<String, Value>>,
    pub expected: Option<Map<String, Value>>,
}

fn selection(source: &Utf16JsonProjection) -> Result<Map<String, Value>, String> {
    let mut result = Map::new();
    for field in FIELDS {
        let key: Vec<u16> = field.encode_utf16().collect();
        if source
            .object_entries()
            .is_some_and(|entries| entries.iter().any(|entry| entry.code_units == key))
        {
            let value = source
                .subprojection_property_units(&key)
                .map_err(|_| "Invalid provider settings")?;
            let json = value
                .to_json_string()
                .map_err(|_| "Invalid provider settings")?;
            result.insert(
                field.into(),
                serde_json::from_str(&json).map_err(|_| "Invalid provider settings")?,
            );
        }
    }
    Ok(result)
}

pub(crate) fn response(
    source: &Utf16JsonProjection,
    managed: Option<&Utf16JsonProjection>,
) -> Result<Value, String> {
    let raw = selection(source)?;
    let policy = managed.map(selection).transpose()?.unwrap_or_default();
    let locked: Vec<String> = policy.keys().cloned().collect();
    let base: lingxi_core::settings::schema::SettingsJson =
        serde_json::from_value(Value::Object(raw.clone()))
            .map_err(|_| "Invalid provider settings")?;
    base.validate().map_err(|_| "Invalid provider settings")?;
    let overlay: lingxi_core::settings::schema::SettingsJson =
        serde_json::from_value(Value::Object(policy))
            .map_err(|_| "Invalid managed provider settings")?;
    overlay
        .validate()
        .map_err(|_| "Invalid managed provider settings")?;
    let merged = lingxi_core::settings::merger::merge(base, overlay)
        .map_err(|_| "Invalid managed provider settings")?;
    let effective = serde_json::to_value(merged).map_err(|_| "Invalid provider settings")?;
    let effective = FIELDS
        .into_iter()
        .filter_map(|key| {
            effective
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect::<Map<_, _>>();
    Ok(serde_json::json!({"raw": raw, "effective": effective, "locked": locked}))
}

pub async fn apply_managed(
    cfg: &mut harness_runtime::desktop::DesktopConfig,
) -> Result<(), String> {
    let managed = harness_runtime::desktop::managed_settings_overlay(cfg)
        .await
        .map_err(|_| "Managed provider settings are unavailable")?;
    let source =
        migrations::settings_update::read_settings_source(&cfg.lingxi_home.join("settings.json"))?;
    let effective = response(&source, managed.as_ref())?["effective"].clone();
    let settings: lingxi_core::settings::schema::SettingsJson =
        serde_json::from_value(effective).map_err(|_| "Invalid provider settings")?;
    cfg.provider_profiles = settings.providers.map(|mut profiles| {
        for profile in profiles.values_mut() {
            if let Some(fields) = profile.as_object_mut() {
                fields.remove("apiKeyEnv");
            }
        }
        profiles
    });
    cfg.routing = settings.routing;
    Ok(())
}

pub fn run(
    home: &Path,
    managed: Option<&Utf16JsonProjection>,
    mut input: impl Read,
) -> Result<String, String> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Provider settings input unavailable")?;
    if bytes.len() > 1024 * 1024 {
        return Err("Provider settings input too large".into());
    }
    let request: Request =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid provider settings request")?;
    let path = home.join("settings.json");
    if let Some(patch) = request.patch {
        if patch.keys().any(|key| !FIELDS.contains(&key.as_str())) {
            return Err("Unsupported provider settings field".into());
        }
        if patch
            .keys()
            .any(|key| managed.is_some_and(|policy| policy.get_property(key).is_some()))
        {
            return Err("Provider setting is controlled by managed policy".into());
        }
        let expected = request
            .expected
            .ok_or("Provider settings version required")?;
        migrations::settings_update::transform_settings_source_with_receipt(&path, |source| {
            if selection(source)? != expected {
                return Err("Provider settings changed; reload and try again".into());
            }
            for (key, value) in patch {
                let projection = Utf16JsonProjection::from_serde_value(&value)
                    .map_err(|_| "Invalid provider settings")?;
                source
                    .set_field(&key, projection)
                    .map_err(|_| "Invalid provider settings")?;
            }
            let raw = source.to_json_string().map_err(|_| "Invalid settings")?;
            let settings: lingxi_core::settings::schema::SettingsJson =
                serde_json::from_str(&raw).map_err(|_| "Invalid provider settings")?;
            settings
                .validate()
                .map_err(|_| "Invalid provider settings")?;
            Ok(())
        })?;
    }
    let source = migrations::settings_update::read_settings_source(&path)?;
    serde_json::to_string(&response(&source, managed)?)
        .map_err(|_| "Invalid provider settings".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_updates_are_atomic_checked_and_preserve_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"model":"keep","providers":{"a":{"type":"openai"}}}"#,
        )
        .unwrap();
        let read = run(dir.path(), None, br#"{}"#.as_slice()).unwrap();
        let expected: Value = serde_json::from_str::<Value>(&read).unwrap()["raw"].clone();
        let request =
            serde_json::json!({"expected": expected, "patch": {"providerRegion":"china_mainland"}});
        run(dir.path(), None, request.to_string().as_bytes()).unwrap();
        let file: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(file["model"], "keep");
        assert_eq!(file["providerRegion"], "china_mainland");
        assert!(run(dir.path(), None, request.to_string().as_bytes()).is_err());
        let policy = Utf16JsonProjection::parse(r#"{"providerRegion":"international"}"#).unwrap();
        let view: Value =
            serde_json::from_str(&run(dir.path(), Some(&policy), br#"{}"#.as_slice()).unwrap())
                .unwrap();
        assert_eq!(view["raw"]["providerRegion"], "china_mainland");
        assert_eq!(view["effective"]["providerRegion"], "international");
        assert_eq!(view["locked"], serde_json::json!(["providerRegion"]));
        let before_denied = std::fs::read(dir.path().join("settings.json")).unwrap();
        let current =
            serde_json::from_str::<Value>(&run(dir.path(), None, br#"{}"#.as_slice()).unwrap())
                .unwrap()["raw"]
                .clone();
        let denied =
            serde_json::json!({"expected":current,"patch":{"providerRegion":"international"}});
        assert!(run(dir.path(), Some(&policy), denied.to_string().as_bytes()).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("settings.json")).unwrap(),
            before_denied
        );
    }
}
