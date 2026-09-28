//! `/web` persistence helpers — ported verbatim (visibility only adjusted)
//! from the deleted iocraft `tui` crate's `root.rs` (git ref `f4ddad16f`,
//! `web_credential_id`/`web_settings_path`/`save_web_settings_to`).
//!
//! These are the pure/IO-only halves of the `/web` effect chain: mapping a
//! [`WebSearchProvider`] to its credential-store id, locating
//! `~/.lingxi/settings.json`, and merging a [`WebSearchConfig`] into it
//! without clobbering unrelated top-level keys. The async orchestration
//! (credential-store writes, test search) lives in the CLI composition root
//! (`apps/cli/src/mode.rs::run_web_action`), which calls these.

use tool_web::web_search_config::{WebSearchConfig, WebSearchProvider};

/// Map a `/web` provider to its credential-store id (`"web:tavily"` /
/// `"web:brave"`). `Auto`/`DuckDuckGo`/`Searxng` need no stored secret and
/// return `None`.
#[must_use]
pub fn web_credential_id(provider: WebSearchProvider) -> Option<&'static str> {
    match provider {
        WebSearchProvider::Tavily => Some("web:tavily"),
        WebSearchProvider::Brave => Some("web:brave"),
        _ => None,
    }
}

/// `~/.lingxi/settings.json` — the same file `/config` and startup settings
/// load read/write, via [`memory::lingxi_md::user_config_dir`].
#[must_use]
pub fn web_settings_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| memory::lingxi_md::user_config_dir(&h).join("settings.json"))
}

/// Merge `cfg` into the JSON object at `path` (parsing the existing file if
/// present, defaulting to `{}` otherwise) and write it back pretty-printed
/// with a trailing newline. Unrelated top-level keys already in the file are
/// preserved — only the keys [`WebSearchConfig::write_settings_json`] touches
/// are overwritten.
pub fn save_web_settings_to(path: &std::path::Path, cfg: &WebSearchConfig) -> std::io::Result<()> {
    let mut value: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    cfg.write_settings_json(&mut value);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&value)?;
    body.push('\n');
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_credential_id_maps_tavily_and_brave_only() {
        assert_eq!(
            web_credential_id(WebSearchProvider::Tavily),
            Some("web:tavily")
        );
        assert_eq!(
            web_credential_id(WebSearchProvider::Brave),
            Some("web:brave")
        );
        assert_eq!(web_credential_id(WebSearchProvider::Auto), None);
        assert_eq!(web_credential_id(WebSearchProvider::DuckDuckGo), None);
        assert_eq!(web_credential_id(WebSearchProvider::Searxng), None);
    }

    /// Round-trip through a real file: write a config, re-read it via
    /// [`WebSearchConfig::from_settings_json`], and confirm an unrelated
    /// pre-existing top-level key survives the merge.
    #[test]
    fn save_web_settings_to_preserves_other_keys() {
        let dir = std::env::temp_dir().join(format!(
            "lingxi-web-persist-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "unrelatedTopLevelKey": "keep-me"
            }))
            .unwrap(),
        )
        .unwrap();

        let cfg = WebSearchConfig {
            provider: WebSearchProvider::Brave,
            searxng_url: Some("https://searx.example.com".to_string()),
        };
        save_web_settings_to(&path, &cfg).expect("save should succeed");

        let body = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["unrelatedTopLevelKey"], "keep-me");

        let read_back = WebSearchConfig::from_settings_json(&value);
        assert_eq!(read_back.provider, WebSearchProvider::Brave);
        assert_eq!(
            read_back.searxng_url.as_deref(),
            Some("https://searx.example.com")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
