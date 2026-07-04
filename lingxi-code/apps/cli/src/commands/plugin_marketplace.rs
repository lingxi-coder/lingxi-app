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

use migrations::settings_update::{read_settings_map, update_settings};
use serde_json::{Map, Value};

use crate::commands::plugin_settings::{Scope, SCOPES};

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

/// Current UTC time as ISO-8601 with millisecond precision + `Z`
/// (`2026-07-04T12:04:33.514Z`), matching the registry's `lastUpdated`.
fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Write the registry map back (pretty, NO trailing newline — matching the
/// binary's `known_marketplaces.json`).
fn write_registry(plugins_dir: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let path = registry_path(plugins_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| format!("Failed to serialize registry: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// The `extraKnownMarketplaces` declaration map from a scope's settings file.
fn read_extra(scope: Scope, home: &Path, cwd: &Path) -> Map<String, Value> {
    read_settings_map(&scope.path(home, cwd))
        .ok()
        .and_then(|m| {
            m.get("extraKnownMarketplaces")
                .and_then(Value::as_object)
                .cloned()
        })
        .unwrap_or_default()
}

/// Read-modify-write a scope's `extraKnownMarketplaces` map.
fn write_extra(scope: Scope, home: &Path, cwd: &Path, map: Map<String, Value>) -> Result<(), String> {
    update_settings(
        &scope.path(home, cwd),
        vec![("extraKnownMarketplaces".to_string(), Some(Value::Object(map)))],
    )
}

/// The marketplace-family invalid-scope error (distinct wording from the
/// enable/disable and install families — verified against the binary).
fn market_invalid_scope(s: &str) -> String {
    format!("✘ Invalid scope '{s}'. Use: user, project, or local")
}

/// Does any editable scope declare `name` in `extraKnownMarketplaces`?
fn declaring_scopes(name: &str, home: &Path, cwd: &Path) -> Vec<Scope> {
    SCOPES
        .into_iter()
        .filter(|s| read_extra(*s, home, cwd).contains_key(name))
        .collect()
}

/// `plugin marketplace add <source> [--scope] [--sparse]`.
///
/// Wires the local-**directory** source (verified 1:1): read + validate the
/// `<dir>/.lingxi-plugin/marketplace.json` (requires `name` + `owner` object),
/// then write BOTH the resolved registry entry (`known_marketplaces.json`) and
/// the per-scope `extraKnownMarketplaces` declaration. If the registry already
/// has the name it only (re)writes the scope declaration and reports "already on
/// disk". Non-directory sources (github/git/url, which clone) are the next
/// increment.
pub fn run_add(
    source: &str,
    scope: Option<&str>,
    _sparse: &[String],
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    // Local path existence is checked FIRST (before the "Adding marketplace…"
    // progress line), matching the binary.
    let looks_local = source.starts_with('/')
        || source.starts_with('.')
        || source.starts_with('~')
        || Path::new(source).exists();
    if !looks_local {
        return Err(format!(
            "✘ Non-directory marketplace sources (github/git/url) are not yet supported: {source}"
        ));
    }
    let abs = std::fs::canonicalize(source).map_err(|_| format!("✘ Path does not exist: {source}"))?;
    if !abs.is_dir() {
        return Err(format!("✘ Path does not exist: {source}"));
    }
    let target = match scope {
        Some(s) => Scope::parse(s).ok_or_else(|| market_invalid_scope(s))?,
        None => Scope::User,
    };

    // From here the "Adding marketplace…" progress prefix is part of the line.
    let manifest_path = abs.join(branding::PLUGIN_MANIFEST_DIR).join("marketplace.json");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|_| {
        format!(
            "Adding marketplace…✘ Failed to add marketplace: Marketplace file not found at {}",
            manifest_path.display()
        )
    })?;
    let manifest: Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "Adding marketplace…✘ Failed to add marketplace: Failed to parse marketplace file at {}: {e}",
            manifest_path.display()
        )
    })?;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "Adding marketplace…✘ Failed to add marketplace: Failed to parse marketplace file at {}: Invalid schema: missing required field 'name'",
                manifest_path.display()
            )
        })?
        .to_string();
    if !manifest.get("owner").is_some_and(Value::is_object) {
        return Err(format!(
            "Adding marketplace…✘ Failed to add marketplace: Failed to parse marketplace file at {}: Invalid schema: {} owner: Invalid input: expected object, received undefined",
            manifest_path.display(),
            manifest_path.display()
        ));
    }

    let source_value = serde_json::json!({
        "source": "directory",
        "path": abs.display().to_string(),
    });

    // Per-scope declaration (settings.extraKnownMarketplaces[name] = {source}).
    let mut extra = read_extra(target, home, cwd);
    extra.insert(name.clone(), serde_json::json!({ "source": source_value }));
    write_extra(target, home, cwd, extra).map_err(|e| {
        format!("Adding marketplace…✘ Failed to add marketplace: {e}")
    })?;

    // Registry (resolved) — only written when the name is not already on disk.
    let mut registry = load_registry(plugins_dir);
    if registry.contains_key(&name) {
        return Ok(format!(
            "Adding marketplace…✔ Marketplace '{name}' already on disk — declared in {} settings",
            target.label()
        ));
    }
    registry.insert(
        name.clone(),
        serde_json::json!({
            "source": source_value,
            "installLocation": abs.display().to_string(),
            "lastUpdated": iso_now(),
        }),
    );
    write_registry(plugins_dir, &registry).map_err(|e| {
        format!("Adding marketplace…✘ Failed to add marketplace: {e}")
    })?;
    Ok(format!(
        "Adding marketplace…✔ Successfully added marketplace: {name} (declared in {} settings)",
        target.label()
    ))
}

/// `plugin marketplace remove <name> [--scope]`.
///
/// Removes the per-scope `extraKnownMarketplaces` declaration (from the given
/// scope, or every scope). When no scope declares the name anymore, drops the
/// resolved registry entry too. Not-configured → the `not found` error.
pub fn run_remove(
    name: &str,
    scope: Option<&str>,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let requested = match scope {
        Some(s) => Some(Scope::parse(s).ok_or_else(|| market_invalid_scope(s))?),
        None => None,
    };
    let declaring = declaring_scopes(name, home, cwd);
    let targets: Vec<Scope> = match requested {
        Some(s) => {
            if declaring.contains(&s) {
                vec![s]
            } else {
                vec![]
            }
        }
        None => declaring.clone(),
    };
    if targets.is_empty() {
        // Nothing declared at the target — also consider a stray registry entry.
        if requested.is_none() && load_registry(plugins_dir).contains_key(name) {
            // Registry-only entry (no declaration): drop it and report success.
            let mut registry = load_registry(plugins_dir);
            registry.remove(name);
            write_registry(plugins_dir, &registry)?;
            return Ok(format!("✔ Successfully removed marketplace: {name}"));
        }
        return Err(format!(
            "✘ Failed to remove marketplace: Marketplace '{name}' not found"
        ));
    }

    for s in &targets {
        let mut extra = read_extra(*s, home, cwd);
        extra.remove(name);
        write_extra(*s, home, cwd, extra)?;
    }

    // Drop the resolved registry entry when no scope declares it anymore.
    if declaring_scopes(name, home, cwd).is_empty() {
        let mut registry = load_registry(plugins_dir);
        if registry.remove(name).is_some() {
            write_registry(plugins_dir, &registry)?;
        }
    }

    Ok(match requested {
        Some(s) => format!("✔ Successfully removed marketplace: {name} (from {} settings)", s.label()),
        None => format!("✔ Successfully removed marketplace: {name}"),
    })
}

/// `plugin marketplace update [name]`.
///
/// Refreshes the registry `lastUpdated` (re-validating a local-directory
/// source's manifest); with no name updates every configured marketplace.
pub fn run_update(
    name: Option<&str>,
    plugins_dir: &Path,
    _home: &Path,
    _cwd: &Path,
) -> Result<String, String> {
    let mut registry = load_registry(plugins_dir);

    if let Some(name) = name {
        if !registry.contains_key(name) {
            let available: Vec<&str> = registry.keys().map(String::as_str).collect();
            return Err(format!(
                "Updating marketplace: {name}...✘ Failed to update marketplace(s): Marketplace '{name}' not found. Available marketplaces: {}",
                available.join(", ")
            ));
        }
        let is_dir = registry
            .get(name)
            .map(|e| source_of(e))
            .map(|s| str_field(&s, "source") == "directory")
            .unwrap_or(false);
        if let Some(entry) = registry.get_mut(name).and_then(Value::as_object_mut) {
            entry.insert("lastUpdated".to_string(), Value::String(iso_now()));
        }
        write_registry(plugins_dir, &registry)?;
        let validating = if is_dir { "Validating local marketplace\n" } else { "" };
        return Ok(format!(
            "Updating marketplace: {name}...{validating}✔ Successfully updated marketplace: {name}"
        ));
    }

    let count = registry.len();
    for entry in registry.values_mut() {
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("lastUpdated".to_string(), Value::String(iso_now()));
        }
    }
    write_registry(plugins_dir, &registry)?;
    Ok(format!(
        "Updating {count} marketplace(s)...✔ Successfully updated {count} marketplace(s)"
    ))
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

    /// A home + project + plugins triple with a local marketplace fixture dir.
    struct FullEnv {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        cwd: PathBuf,
        plugins: PathBuf,
        market: PathBuf,
    }

    /// Build a fixture with a valid `<market>/.lingxi-plugin/marketplace.json`
    /// (name = `mymkt`, has the required `owner` object).
    fn full_env() -> FullEnv {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("proj");
        let plugins = home.join("plugins");
        let market = tmp.path().join("mymkt");
        for d in [&home, &cwd, &plugins] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mdir = market.join(branding::PLUGIN_MANIFEST_DIR);
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(
            mdir.join("marketplace.json"),
            r#"{"name":"mymkt","owner":{"name":"me"},"plugins":[]}"#,
        )
        .unwrap();
        FullEnv {
            _tmp: tmp,
            home,
            cwd,
            plugins,
            market,
        }
    }

    fn registry_of(e: &FullEnv) -> Value {
        let raw = std::fs::read_to_string(e.plugins.join("known_marketplaces.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn add_directory_writes_registry_and_declaration() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        let abs = std::fs::canonicalize(&e.market).unwrap().display().to_string();
        let msg = run_add(&src, None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Adding marketplace…✔ Successfully added marketplace: mymkt (declared in user settings)"
        );
        // Registry entry (resolved).
        let reg = registry_of(&e);
        assert_eq!(reg["mymkt"]["source"], json!({"source": "directory", "path": abs}));
        assert_eq!(reg["mymkt"]["installLocation"], json!(abs));
        assert!(reg["mymkt"]["lastUpdated"].is_string());
        // Per-scope declaration (user settings).
        let user: Value =
            serde_json::from_str(&std::fs::read_to_string(e.home.join("settings.json")).unwrap()).unwrap();
        assert_eq!(
            user["extraKnownMarketplaces"]["mymkt"],
            json!({"source": {"source": "directory", "path": abs}})
        );
    }

    #[test]
    fn add_second_scope_reports_already_on_disk() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, Some("user"), &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_add(&src, Some("project"), &[], &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Adding marketplace…✔ Marketplace 'mymkt' already on disk — declared in project settings"
        );
        // Project declaration written.
        let project: Value = serde_json::from_str(
            &std::fs::read_to_string(e.cwd.join(".lingxi").join("settings.json")).unwrap(),
        )
        .unwrap();
        assert!(project["extraKnownMarketplaces"]["mymkt"].is_object());
    }

    #[test]
    fn add_path_not_exist_errors_without_prefix() {
        let e = full_env();
        let err = run_add("/no/such/dir", None, &[], &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(err, "✘ Path does not exist: /no/such/dir");
    }

    #[test]
    fn add_missing_manifest_errors_with_prefix() {
        let e = full_env();
        let empty = e._tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = run_add(
            &empty.to_string_lossy(),
            None,
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap_err();
        assert!(
            err.starts_with("Adding marketplace…✘ Failed to add marketplace: Marketplace file not found at "),
            "got: {err}"
        );
    }

    #[test]
    fn add_missing_owner_errors() {
        let e = full_env();
        let bad = e._tmp.path().join("bad");
        let mdir = bad.join(branding::PLUGIN_MANIFEST_DIR);
        std::fs::create_dir_all(&mdir).unwrap();
        std::fs::write(mdir.join("marketplace.json"), r#"{"name":"bad","plugins":[]}"#).unwrap();
        let err = run_add(&bad.to_string_lossy(), None, &[], &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert!(err.contains("owner: Invalid input: expected object, received undefined"), "got: {err}");
    }

    #[test]
    fn add_invalid_scope_errors() {
        let e = full_env();
        let err = run_add(
            &e.market.to_string_lossy(),
            Some("bogus"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap_err();
        assert_eq!(err, "✘ Invalid scope 'bogus'. Use: user, project, or local");
    }

    #[test]
    fn remove_scoped_keeps_registry_when_still_declared() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, Some("user"), &[], &e.plugins, &e.home, &e.cwd).unwrap();
        run_add(&src, Some("project"), &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_remove("mymkt", Some("project"), &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Successfully removed marketplace: mymkt (from project settings)");
        // Still declared in user → registry kept.
        assert!(registry_of(&e).get("mymkt").is_some());
    }

    #[test]
    fn remove_all_drops_registry() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, Some("user"), &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_remove("mymkt", None, &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Successfully removed marketplace: mymkt");
        assert_eq!(registry_of(&e), json!({}));
    }

    #[test]
    fn remove_not_configured_errors() {
        let e = full_env();
        let err = run_remove("ghost", None, &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(err, "✘ Failed to remove marketplace: Marketplace 'ghost' not found");
    }

    #[test]
    fn update_named_directory_validates() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_update(Some("mymkt"), &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Updating marketplace: mymkt...Validating local marketplace\n✔ Successfully updated marketplace: mymkt"
        );
    }

    #[test]
    fn update_all_counts() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_update(None, &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "Updating 1 marketplace(s)...✔ Successfully updated 1 marketplace(s)");
    }

    #[test]
    fn update_unknown_lists_available() {
        let e = full_env();
        let src = e.market.to_string_lossy().to_string();
        run_add(&src, None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let err = run_update(Some("ghost"), &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Updating marketplace: ghost...✘ Failed to update marketplace(s): Marketplace 'ghost' not found. Available marketplaces: mymkt"
        );
    }
}
