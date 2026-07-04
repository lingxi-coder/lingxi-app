//! `plugin marketplace list` — render the configured-marketplaces registry.
//!
//! claude-code's `plugin marketplace list` reads ONLY the resolved registry
//! `<plugins>/known_marketplaces.json` (probed: a settings-only
//! `extraKnownMarketplaces` declaration WITHOUT a registry entry renders
//! nothing; a registry entry WITHOUT a settings declaration still renders). The
//! registry is a map `name → { source, installLocation, lastUpdated }` where
//! `source` is one of:
//! `{source:"directory",path}` / `{source:"git",url,ref?}` /
//! `{source:"github",repo,ref?}` / `{source:"url",url}`.
//!
//! Output is 1:1 with the binary (verified live for the `directory` case; the
//! git/github/url human render mirrors the binary's exact template
//! `Source: Git (${url}${ref?`@${ref}`:""})` etc.).
//!
//! `add` / `remove` / `update` (which WRITE the registry + the per-scope
//! `extraKnownMarketplaces` settings declaration, and clone/fetch sources) are
//! the follow-up increment — see `.omo/plans/2026-07-04-plugin-cli-port.md`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The resolved-marketplaces registry file under the plugins root.
fn registry_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join("known_marketplaces.json")
}

/// Load the registry `name → entry` map (missing / malformed / non-object ⇒
/// empty — resilient, matching the read-only boot).
fn load_registry(plugins_dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(registry_path(plugins_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The `source` sub-object of a registry entry (`{}` when absent).
fn source_of(entry: &Value) -> Map<String, Value> {
    entry
        .get("source")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A source string field (`path` / `url` / `repo` / `ref`).
fn str_field(source: &Map<String, Value>, key: &str) -> String {
    source.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The `Source: …` human line for one entry, mirroring the binary's template.
fn source_render(entry: &Value) -> String {
    let source = source_of(entry);
    let kind = str_field(&source, "source");
    let ref_suffix = {
        let r = str_field(&source, "ref");
        if r.is_empty() {
            String::new()
        } else {
            format!("@{r}")
        }
    };
    match kind.as_str() {
        "directory" => format!("Directory ({})", str_field(&source, "path")),
        "git" => format!("Git ({}{ref_suffix})", str_field(&source, "url")),
        "github" => format!("GitHub ({}{ref_suffix})", str_field(&source, "repo")),
        "url" => format!("URL ({})", str_field(&source, "url")),
        // Unknown/absent source kind: render the raw kind for visibility rather
        // than fabricate a label.
        other => format!("{other} ()"),
    }
}

/// The `--json` object for one entry: `{name, source, <path|url|repo>, installLocation}`
/// (field set verified live for `directory`; git/github/url mirror the same
/// shape with their source-specific locator).
fn json_entry(name: &str, entry: &Value) -> Value {
    let source = source_of(entry);
    let kind = str_field(&source, "source");
    let mut out = Map::new();
    out.insert("name".to_string(), Value::String(name.to_string()));
    out.insert("source".to_string(), Value::String(kind.clone()));
    match kind.as_str() {
        "directory" => {
            out.insert("path".to_string(), Value::String(str_field(&source, "path")));
        }
        "github" => {
            out.insert("repo".to_string(), Value::String(str_field(&source, "repo")));
        }
        // git + url both locate via `url`.
        _ => {
            out.insert("url".to_string(), Value::String(str_field(&source, "url")));
        }
    }
    if let Some(loc) = entry.get("installLocation").and_then(Value::as_str) {
        out.insert("installLocation".to_string(), Value::String(loc.to_string()));
    }
    Value::Object(out)
}

/// `plugin marketplace list [--json]` — returns the text to print to stdout.
pub fn run_list(plugins_dir: &Path, json: bool) -> String {
    let registry = load_registry(plugins_dir);

    if json {
        let arr: Vec<Value> = registry
            .iter()
            .map(|(name, entry)| json_entry(name, entry))
            .collect();
        return serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_else(|_| "[]".to_string());
    }

    if registry.is_empty() {
        return "No marketplaces configured".to_string();
    }

    let mut lines = vec!["Configured marketplaces:".to_string(), String::new()];
    for (name, entry) in &registry {
        lines.push(format!("  ❯ {name}"));
        lines.push(format!("    Source: {}", source_render(entry)));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Env {
        _tmp: tempfile::TempDir,
        plugins: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        Env { _tmp: tmp, plugins }
    }

    fn write_registry(e: &Env, v: &Value) {
        std::fs::write(
            e.plugins.join("known_marketplaces.json"),
            serde_json::to_string_pretty(v).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn empty_registry_human_and_json() {
        let e = env();
        assert_eq!(run_list(&e.plugins, false), "No marketplaces configured");
        assert_eq!(run_list(&e.plugins, true), "[]");
    }

    #[test]
    fn missing_registry_file_is_empty() {
        let e = env();
        std::fs::remove_dir_all(&e.plugins).unwrap();
        assert_eq!(run_list(&e.plugins, false), "No marketplaces configured");
    }

    #[test]
    fn directory_human_matches_oracle() {
        let e = env();
        write_registry(
            &e,
            &json!({
                "mymkt": {
                    "source": {"source": "directory", "path": "/abs/mymkt"},
                    "installLocation": "/abs/mymkt",
                    "lastUpdated": "2026-07-04T10:28:13.751Z"
                }
            }),
        );
        assert_eq!(
            run_list(&e.plugins, false),
            "Configured marketplaces:\n\n  ❯ mymkt\n    Source: Directory (/abs/mymkt)"
        );
    }

    #[test]
    fn directory_json_matches_oracle() {
        let e = env();
        write_registry(
            &e,
            &json!({
                "mymkt": {
                    "source": {"source": "directory", "path": "/abs/mymkt"},
                    "installLocation": "/abs/mymkt",
                    "lastUpdated": "2026-07-04T10:28:13.751Z"
                }
            }),
        );
        let expected = serde_json::to_string_pretty(&json!([{
            "name": "mymkt",
            "source": "directory",
            "path": "/abs/mymkt",
            "installLocation": "/abs/mymkt"
        }]))
        .unwrap();
        assert_eq!(run_list(&e.plugins, true), expected);
    }

    #[test]
    fn git_and_github_and_url_human_render() {
        let e = env();
        write_registry(
            &e,
            &json!({
                "gh":  {"source": {"source": "github", "repo": "acme/plugins", "ref": "v2"}},
                "g2":  {"source": {"source": "git", "url": "https://x/y.git"}},
                "web": {"source": {"source": "url", "url": "https://x/cat.json"}}
            }),
        );
        // Registry order is insertion order (serde_json preserve_order).
        assert_eq!(
            run_list(&e.plugins, false),
            "Configured marketplaces:\n\n  \
             ❯ gh\n    Source: GitHub (acme/plugins@v2)\n  \
             ❯ g2\n    Source: Git (https://x/y.git)\n  \
             ❯ web\n    Source: URL (https://x/cat.json)"
        );
    }
}
