//! Best-effort recent-model persistence via `~/.claude/settings.json`
//! `recentModels` field (mirrors `theme_persist.rs`; no new persistence engine).
//!
//! Stored as an ordered array (most-recent-first) of `{ "provider": ..,
//! "model": .. }` objects, capped at [`MAX_RECENT`]. Save/load degrade to a
//! no-op on any error — recents are a convenience, never load-bearing.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Max recent entries retained.
pub const MAX_RECENT: usize = 8;

/// One recent selection: the provider grouping key + the wire model id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentModel {
    /// Provider grouping key (matches `ModelRow.provider_id`).
    pub provider_id: String,
    /// Wire model id (what `switch_model` accepts; matches `ModelRow.request_model`).
    pub request_model: String,
}

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| memory::claude_md::user_config_dir(&h).join("settings.json"))
}

/// Load the recent list (most-recent-first). Empty on any error.
#[must_use]
pub fn load_recent_models() -> Vec<RecentModel> {
    settings_path()
        .as_deref()
        .map(load_recent_models_from)
        .unwrap_or_default()
}

/// Test seam: load from an explicit path.
#[must_use]
pub fn load_recent_models_from(path: &Path) -> Vec<RecentModel> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(obj) = serde_json::from_str::<Map<String, Value>>(&body) else {
        return Vec::new();
    };
    let Some(arr) = obj.get("recentModels").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            Some(RecentModel {
                provider_id: v.get("provider")?.as_str()?.to_string(),
                request_model: v.get("model")?.as_str()?.to_string(),
            })
        })
        .take(MAX_RECENT)
        .collect()
}

/// Record a selection at the front (dedup by `request_model`), cap at
/// [`MAX_RECENT`], best-effort persist. Logs + swallows errors.
pub fn record_recent_model(provider_id: &str, request_model: &str) {
    let Some(path) = settings_path() else {
        tracing::debug!("recent-model persist skipped: no home dir");
        return;
    };
    if let Err(e) = record_recent_model_to(&path, provider_id, request_model) {
        tracing::debug!(error = %e, "recent-model persist failed (session-only)");
    }
}

/// Test seam: read-modify-write `recentModels` at an explicit path, preserving
/// other keys. Pretty JSON + trailing newline (config-tool shape).
pub fn record_recent_model_to(
    path: &Path,
    provider_id: &str,
    request_model: &str,
) -> std::io::Result<()> {
    let mut list = load_recent_models_from(path);
    list.retain(|r| r.request_model != request_model);
    list.insert(
        0,
        RecentModel {
            provider_id: provider_id.to_string(),
            request_model: request_model.to_string(),
        },
    );
    list.truncate(MAX_RECENT);

    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let arr: Vec<Value> = list
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("provider".to_string(), Value::String(r.provider_id.clone()));
            m.insert("model".to_string(), Value::String(r.request_model.clone()));
            Value::Object(m)
        })
        .collect();
    obj.insert("recentModels".to_string(), Value::Array(arr));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}

/// Persist `qualified_model` (the profile-qualified id the picker committed,
/// e.g. `deepseek/deepseek-chat`) as the `model` setting in
/// `~/.claude/settings.json`, so the NEXT launch defaults to the model the user
/// picked instead of the built-in default. Best-effort; logs + swallows errors.
pub fn record_default_model(qualified_model: &str) {
    let Some(path) = settings_path() else {
        tracing::debug!("default-model persist skipped: no home dir");
        return;
    };
    if let Err(e) = record_default_model_to(&path, qualified_model) {
        tracing::debug!(error = %e, "default-model persist failed (session-only)");
    }
}

/// Test seam: read-modify-write the `model` key at an explicit path, preserving
/// every other key. Pretty JSON + trailing newline (config-tool shape).
pub fn record_default_model_to(path: &Path, qualified_model: &str) -> std::io::Result<()> {
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    obj.insert(
        "model".to_string(),
        Value::String(qualified_model.to_string()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&obj)?;
    body.push('\n');
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-recent-{}-{tag}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn roundtrip_dedup_order_and_cap() {
        let path = tmp("roundtrip");
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        record_recent_model_to(&path, "github-copilot", "gpt-5.4-nano").unwrap();
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        let got = load_recent_models_from(&path);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].request_model, "deepseek-chat");
        assert_eq!(got[0].provider_id, "deepseek");
        assert_eq!(got[1].request_model, "gpt-5.4-nano");

        for i in 0..(MAX_RECENT + 4) {
            record_recent_model_to(&path, "p", &format!("m{i}")).unwrap();
        }
        assert_eq!(load_recent_models_from(&path).len(), MAX_RECENT);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preserves_other_keys() {
        let path = tmp("preserve");
        std::fs::write(&path, "{\n  \"theme\": \"dark\"\n}\n").unwrap();
        record_recent_model_to(&path, "deepseek", "deepseek-chat").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"theme\": \"dark\""));
        assert!(body.contains("\"recentModels\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_missing_or_garbage_is_empty() {
        assert!(load_recent_models_from(Path::new("/nonexistent/lingxi-x.json")).is_empty());
    }
}
