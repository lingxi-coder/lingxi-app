//! Remote Desktop Local App catalog and browser bootstrap DTOs.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_SAFE: u64 = 9_007_199_254_740_991;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A Local App explicitly shared by the desktop.
pub struct RemoteLocalApp {
    /// Stable application identifier.
    pub app_id: String,
    /// Application display name.
    pub name: String,
    /// Application description.
    pub summary: String,
    /// Desktop application revision.
    pub revision: u64,
    /// Desktop execution phase.
    pub phase: String,
    /// Whether background execution is enabled.
    pub background: bool,
    /// Execution host, always desktop.
    pub execution_host: String,
    /// Whether the desktop shared the app.
    pub remote_enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Single-use browser bootstrap for a shared Local App.
pub struct RemoteLocalAppOpen {
    /// Stable application identifier.
    pub app_id: String,
    /// Desktop application revision.
    pub revision: u64,
    /// HTTPS browser entry with a single-use fragment.
    pub url: String,
    /// Ticket expiration in Unix milliseconds.
    pub expires_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppsList {
    apps: Vec<RemoteLocalApp>,
}
fn text(value: &str, max: usize, empty: bool) -> bool {
    (empty || !value.is_empty())
        && value.encode_utf16().count() <= max
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
pub(crate) fn valid_id(value: &str) -> bool {
    value.len() == 33
        && value.starts_with('a')
        && value[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn parse_list(value: serde_json::Value) -> Option<Vec<RemoteLocalApp>> {
    let list: AppsList = serde_json::from_value(value).ok()?;
    if list.apps.len() > 256 {
        return None;
    }
    let mut ids = HashSet::new();
    if !list.apps.iter().all(|a| {
        valid_id(&a.app_id)
            && ids.insert(&a.app_id)
            && text(&a.name, 256, false)
            && text(&a.summary, 4096, true)
            && a.revision > 0
            && a.revision <= MAX_SAFE
            && matches!(
                a.phase.as_str(),
                "stopped"
                    | "installing"
                    | "building"
                    | "starting"
                    | "running"
                    | "stopping"
                    | "failed"
            )
            && a.execution_host == "desktop"
            && a.remote_enabled
    }) {
        return None;
    }
    Some(list.apps)
}
pub(crate) fn parse_open(value: serde_json::Value, app_id: &str) -> Option<RemoteLocalAppOpen> {
    let result: RemoteLocalAppOpen = serde_json::from_value(value).ok()?;
    if !valid_id(&result.app_id)
        || result.app_id != app_id
        || result.revision == 0
        || result.revision > MAX_SAFE
        || result.expires_at == 0
        || result.expires_at > MAX_SAFE
        || !text(&result.url, 8192, false)
    {
        return None;
    }
    let url = reqwest::Url::parse(&result.url).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
        || !url.host_str().is_some_and(|host| {
            host.split_once('.').is_some_and(|(label, domain)| {
                !domain.is_empty()
                    && label
                        .strip_prefix(&format!("{}-", result.app_id))
                        .is_some_and(|session| {
                            session.len() == 20
                                && session
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        })
            })
        })
        || url.path() != "/__lingxi_remote_bootstrap"
        || url.query().is_some()
        || !url.fragment().is_some_and(|f| {
            f.len() == 43
                && f.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return None;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_catalog_and_open() {
        let app = json!({"app_id":"a00000000000000000000000000000000","name":"App","summary":"","revision":1,"phase":"running","background":false,"execution_host":"desktop","remote_enabled":true});
        assert!(parse_list(json!({"apps":[app.clone()]})).is_some());
        let mut invalid = app.clone();
        invalid["revision"] = json!(0);
        assert!(parse_list(json!({"apps":[invalid]})).is_none());
        assert!(parse_list(json!({"apps":[app.clone(), app]})).is_none());
        assert!(parse_open(
            json!({"app_id":"a00000000000000000000000000000000","revision":1,"url":"https://a00000000000000000000000000000000-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","expires_at":1}),
            "a00000000000000000000000000000000"
        )
        .is_some());
        assert!(parse_open(
            json!({"app_id":"a00000000000000000000000000000000","revision":1,"url":"http://a00000000000000000000000000000000-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","expires_at":1}),
            "a00000000000000000000000000000000"
        )
        .is_none());
        assert!(parse_open(json!({"app_id":"a00000000000000000000000000000000","revision":1,"url":"https://a00000000000000000000000000000000.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","expires_at":1}), "a00000000000000000000000000000000").is_none());
        assert!(parse_open(json!({"app_id":"a00000000000000000000000000000000","revision":1,"url":"https://a00000000000000000000000000000000-0123456789abcdef012g.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","expires_at":1}), "a00000000000000000000000000000000").is_none());
        assert!(parse_open(json!({"app_id":"a00000000000000000000000000000000","revision":1,"url":"https://user:pass@a00000000000000000000000000000000-0123456789abcdef0123.example.com/__lingxi_remote_bootstrap#xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","expires_at":1}), "a00000000000000000000000000000000").is_none());
    }
}
