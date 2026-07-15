//! `plugin install` / `plugin uninstall` — materialize a plugin from a
//! configured marketplace and toggle the on-disk state, 1:1 with claude-code
//! 2.1.201 (directory-source marketplaces; verified end-to-end).
//!
//! install (`<plugin>[@<market>]`):
//! 1. resolve the marketplace from the `known_marketplaces.json` registry
//!    (an explicit `@market`, else the first registry whose `marketplace.json`
//!    lists a plugin of that name);
//! 2. copy the plugin tree `<installLocation>/<entry.source>` → the versioned
//!    cache `cache/{market}/{plugin}/{version}/` (version from the plugin's
//!    `plugin.json`);
//! 3. write the v2 `installed_plugins.json` record
//!    (`plugins["<plugin>@<market>"] = [{scope, installPath, version,
//!    installedAt, lastUpdated}]`) and set `enabledPlugins[id]=true` at scope.
//!
//! uninstall drops the installed record, DELETES the `enabledPlugins[id]` key
//! (note: NOT set to `false` — that is what `disable` does), and ORPHANS the
//! cache (writes a `.orphaned_at` marker rather than deleting immediately).
//!
//! Residuals (follow-ups): non-directory marketplace sources (git/github/url),
//! `--config` userConfig storage/validation, and `--prune` dependency GC (the
//! orphan marker is written; the deferred sweep that deletes it is not ported).

use std::path::{Path, PathBuf};

use migrations::settings_update::{read_settings_map, update_settings};
use serde_json::{Map, Value};

use crate::commands::plugin_settings::Scope;

/// Now as ISO-8601 with milliseconds + `Z`.
fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `<plugins>/installed_plugins.json` path.
fn installed_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join("installed_plugins.json")
}

/// Load the v2 installed DB (`{version:2, plugins:{...}}`); missing/malformed ⇒
/// a fresh empty v2 doc.
fn load_installed(plugins_dir: &Path) -> Value {
    std::fs::read_to_string(installed_path(plugins_dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(|v| v.get("plugins").is_some())
        .unwrap_or_else(|| serde_json::json!({"version": 2, "plugins": {}}))
}

/// Write the installed DB (pretty, no trailing newline).
fn write_installed(plugins_dir: &Path, doc: &Value) -> Result<(), String> {
    let path = installed_path(plugins_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// The resolved-marketplaces registry (`name → {source, installLocation, …}`).
fn load_registry(plugins_dir: &Path) -> Map<String, Value> {
    std::fs::read_to_string(plugins_dir.join("known_marketplaces.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// A marketplace's on-disk root (directory source → `installLocation`).
fn install_location(entry: &Value) -> Option<PathBuf> {
    entry
        .get("installLocation")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Split `name@market` → `(name, Some(market))`; bare → `(name, None)`
/// (only the first `@` separates, mirroring `parsePluginIdentifier`).
fn split_id(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('@') {
        Some((n, rest)) => (n, Some(rest.split('@').next().unwrap_or(rest))),
        None => (arg, None),
    }
}

/// The pre-`@` display name.
fn name_of(id: &str) -> &str {
    id.split('@').next().unwrap_or(id)
}

/// Read a marketplace's `plugins[]` entry for `name`, returning its `source`
/// (the plugin's path within the marketplace repo, a relative string).
fn marketplace_entry_source(market_root: &Path, name: &str) -> Option<String> {
    let manifest = market_root
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("marketplace.json");
    let raw = std::fs::read_to_string(manifest).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let plugins = value.get("plugins").and_then(Value::as_array)?;
    let entry = plugins
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))?;
    // `source` may be a string (relative path) or an object (git/github — not
    // yet supported here). Default to "." (repo root) when absent.
    match entry.get("source") {
        Some(Value::String(s)) => Some(s.clone()),
        None => Some(".".to_string()),
        Some(_) => None, // object source (external) — unsupported for now
    }
}

/// Recursive directory copy (sync).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Sanitize a path segment for the cache layout (mirrors the plugin crate's
/// `getVersionedCachePath` sanitizer: keep `[A-Za-z0-9-_]`, plus `.` for the
/// version segment; empty/`.`/`..` collapse to `-`).
fn sanitize(segment: &str, allow_dot: bool) -> String {
    let mapped: String = segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || (allow_dot && c == '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() || mapped == "." || mapped == ".." {
        "-".to_string()
    } else {
        mapped
    }
}

/// Parse the install-family `--scope` (default `user`); its invalid-scope
/// wording differs from enable/disable and marketplace.
fn parse_scope(scope: Option<&str>) -> Result<Scope, String> {
    match scope {
        None => Ok(Scope::User),
        Some(s) => Scope::parse(s)
            .ok_or_else(|| format!("Invalid scope: {s}. Must be one of: user, project, local.")),
    }
}

/// Read a scope's `enabledPlugins` map.
fn read_enabled(scope: Scope, home: &Path, cwd: &Path) -> Map<String, Value> {
    read_settings_map(&scope.path(home, cwd))
        .ok()
        .and_then(|m| m.get("enabledPlugins").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// Set (`Some(true/false)`) or delete (`None`) an `enabledPlugins` entry at scope.
fn edit_enabled(
    scope: Scope,
    home: &Path,
    cwd: &Path,
    id: &str,
    value: Option<bool>,
) -> Result<(), String> {
    let mut map = read_enabled(scope, home, cwd);
    match value {
        Some(b) => {
            map.insert(id.to_string(), Value::Bool(b));
        }
        None => {
            map.remove(id);
        }
    }
    update_settings(
        &scope.path(home, cwd),
        vec![("enabledPlugins".to_string(), Some(Value::Object(map)))],
    )
}

/// `✘ Failed to <verb> plugin "<arg>": <reason>`.
fn fail(verb: &str, arg: &str, reason: &str) -> String {
    format!("✘ Failed to {verb} plugin \"{arg}\": {reason}")
}

/// The `projectPath` an install record carries at this scope: the realpath of
/// `cwd` for `project`/`local`, `None` for `user` (which is cwd-independent).
/// Records are keyed per (scope, projectPath), so this identifies the slot.
fn project_path(scope: Scope, cwd: &Path) -> Option<String> {
    match scope {
        Scope::User => None,
        Scope::Project | Scope::Local => Some(
            std::fs::canonicalize(cwd)
                .unwrap_or_else(|_| cwd.to_path_buf())
                .display()
                .to_string(),
        ),
    }
}

/// Does an installed record occupy the (scope, projectPath) slot? For `user` the
/// scope match suffices; for project/local the record's `projectPath` must match
/// the current one too (distinct projects install the same plugin independently).
fn record_matches(rec: &Value, scope: Scope, proj: &Option<String>) -> bool {
    if rec.get("scope").and_then(Value::as_str) != Some(scope.label()) {
        return false;
    }
    match proj {
        None => true,
        Some(p) => rec.get("projectPath").and_then(Value::as_str) == Some(p.as_str()),
    }
}

/// `plugin install <plugin[@market]> [--scope] [--config]`.
pub fn run_install(
    arg: &str,
    scope: Option<&str>,
    _config: &[String],
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    // Scope is validated BEFORE the "Installing plugin …" progress prefix — the
    // binary emits the bare `Invalid scope: …` line with no prefix.
    let scope = parse_scope(scope)?;
    let (name, market) = split_id(arg);
    let registry = load_registry(plugins_dir);

    // Resolve the marketplace + the plugin's source dir within it.
    let (market_name, plugin_src) = match market {
        Some(market) => {
            let root = registry.get(market).and_then(install_location);
            let src = root
                .as_deref()
                .and_then(|r| marketplace_entry_source(r, name).map(|s| (r.to_path_buf(), s)));
            match src {
                Some((root, rel)) => (market.to_string(), root.join(rel)),
                None => {
                    return Err(fail(
                        "install",
                        arg,
                        &format!(
                            "Plugin \"{name}\" not found in marketplace \"{market}\". Your local copy may be out of date — try `lingxi-cli plugin marketplace update {market}`."
                        ),
                    ))
                    .map_err(|e| format!("Installing plugin \"{arg}\"...{e}"));
                }
            }
        }
        None => {
            // Bare name: first registry marketplace that lists it.
            let found = registry.iter().find_map(|(mkt, entry)| {
                let root = install_location(entry)?;
                let rel = marketplace_entry_source(&root, name)?;
                Some((mkt.clone(), root.join(rel)))
            });
            match found {
                Some(v) => v,
                None => {
                    return Err(fail(
                        "install",
                        arg,
                        &format!("Plugin \"{name}\" not found in any configured marketplace"),
                    ))
                    .map_err(|e| format!("Installing plugin \"{arg}\"...{e}"));
                }
            }
        }
    };

    let full_id = format!("{name}@{market_name}");

    // Version from the plugin's own manifest.
    let version = std::fs::read_to_string(
        plugin_src
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
    )
    .ok()
    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from))
    .unwrap_or_else(|| "unknown".to_string());

    let mut installed = load_installed(plugins_dir);
    let proj = project_path(scope, cwd);
    // Already installed AT THIS (scope, projectPath) slot? A record at a
    // different scope does NOT block — install appends a second per-scope record.
    if installed
        .get("plugins")
        .and_then(|p| p.get(&full_id))
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|r| record_matches(r, scope, &proj)))
    {
        return Ok(format!(
            "Installing plugin \"{arg}\"...✔ Plugin \"{full_id}\" is already installed (scope: {})",
            scope.label()
        ));
    }

    // Materialize into the versioned cache.
    let dest = plugins_dir
        .join("cache")
        .join(sanitize(&market_name, false))
        .join(sanitize(name, false))
        .join(sanitize(&version, true));
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(&plugin_src, &dest).map_err(|e| {
        format!(
            "Installing plugin \"{arg}\"...{}",
            fail("install", arg, &e.to_string())
        )
    })?;

    // Record (v2) — `projectPath` is the LAST field, present only for
    // project/local scope (matching the binary's on-disk shape).
    let now = iso_now();
    let mut record = serde_json::Map::new();
    record.insert(
        "scope".to_string(),
        Value::String(scope.label().to_string()),
    );
    record.insert(
        "installPath".to_string(),
        Value::String(dest.display().to_string()),
    );
    record.insert("version".to_string(), Value::String(version.clone()));
    record.insert("installedAt".to_string(), Value::String(now.clone()));
    record.insert("lastUpdated".to_string(), Value::String(now));
    if let Some(p) = &proj {
        record.insert("projectPath".to_string(), Value::String(p.clone()));
    }
    // Append to the plugin's record array (create it if this is the first scope).
    if let Some(plugins) = installed.get_mut("plugins").and_then(Value::as_object_mut) {
        let arr = plugins
            .entry(full_id.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(a) = arr.as_array_mut() {
            a.push(Value::Object(record));
        }
    }
    write_installed(plugins_dir, &installed)
        .map_err(|e| format!("Installing plugin \"{arg}\"...{}", fail("install", arg, &e)))?;
    edit_enabled(scope, home, cwd, &full_id, Some(true))
        .map_err(|e| format!("Installing plugin \"{arg}\"...{}", fail("install", arg, &e)))?;

    Ok(format!(
        "Installing plugin \"{arg}\"...✔ Successfully installed plugin: {full_id} (scope: {})",
        scope.label()
    ))
}

/// `plugin uninstall <plugin> [--keep-data] [--prune] [-y] [--scope]`.
pub fn run_uninstall(
    arg: &str,
    scope: Option<&str>,
    _keep_data: bool,
    _prune: bool,
    _yes: bool,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let scope = parse_scope(scope)?;
    let proj = project_path(scope, cwd);
    let (name, market) = split_id(arg);
    let mut installed = load_installed(plugins_dir);

    // Resolve the full id: explicit `@market`, else the first installed key
    // whose name-part matches.
    let full_id = match market {
        Some(market) => format!("{name}@{market}"),
        None => installed
            .get("plugins")
            .and_then(Value::as_object)
            .and_then(|p| p.keys().find(|k| name_of(k) == name).cloned())
            .unwrap_or_else(|| name.to_string()),
    };

    let records = installed
        .get("plugins")
        .and_then(|p| p.get(&full_id))
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty());
    let Some(records) = records else {
        return Err(fail(
            "uninstall",
            arg,
            &format!("Plugin \"{full_id}\" not found in installed plugins"),
        ));
    };

    // Records at the requested (scope, projectPath) slot. If none, the plugin is
    // installed at some OTHER scope — name it, matching the binary.
    let matching: Vec<Value> = records
        .iter()
        .filter(|r| record_matches(r, scope, &proj))
        .cloned()
        .collect();
    if matching.is_empty() {
        let mut other: Vec<&str> = Vec::new();
        for s in records
            .iter()
            .filter_map(|r| r.get("scope").and_then(Value::as_str))
        {
            if !other.contains(&s) {
                other.push(s);
            }
        }
        let installed_in = other.join(", ");
        let first = other.first().copied().unwrap_or("user");
        return Err(fail(
            "uninstall",
            arg,
            &format!(
                "Plugin \"{full_id}\" is installed in {installed_in} scope, not {}. \
                 Use --scope {first} to uninstall.",
                scope.label()
            ),
        ));
    }

    // Orphan only the matched records' cache dirs (marker; deferred sweep deletes).
    for rec in &matching {
        if let Some(path) = rec.get("installPath").and_then(Value::as_str) {
            let _ = std::fs::write(Path::new(path).join(".orphaned_at"), iso_now());
        }
    }

    // Drop only the matched records; remove the key once the array is empty.
    if let Some(plugins) = installed.get_mut("plugins").and_then(Value::as_object_mut) {
        if let Some(arr) = plugins.get_mut(&full_id).and_then(Value::as_array_mut) {
            arr.retain(|r| !record_matches(r, scope, &proj));
            if arr.is_empty() {
                plugins.remove(&full_id);
            }
        }
    }
    write_installed(plugins_dir, &installed).map_err(|e| fail("uninstall", arg, &e))?;

    // DELETE the enabledPlugins key at THIS scope only (uninstall removes the
    // entry entirely, unlike `disable` which sets it to false).
    let _ = edit_enabled(scope, home, cwd, &full_id, None);

    Ok(format!(
        "✔ Successfully uninstalled plugin: {} (scope: {})",
        name_of(&full_id),
        scope.label()
    ))
}

/// Validate a `plugin update` `--scope`. Unlike the install family, update's
/// valid set INCLUDES `managed` (a read-only enterprise scope that update may
/// name but never has an editable record at), and its invalid-scope wording is
/// distinct: `Invalid scope "<s>". Valid scopes: user, project, local, managed`
/// (no ✘ prefix; emitted before the `Checking for updates…` line).
fn parse_update_scope(scope: &str) -> Result<&str, String> {
    match scope {
        "user" | "project" | "local" | "managed" => Ok(scope),
        _ => Err(format!(
            "Invalid scope \"{scope}\". Valid scopes: user, project, local, managed"
        )),
    }
}

/// The project-root path recorded for a scope (`project`/`local` → cwd; `user`/
/// `managed` → none), used both to disambiguate multi-install records and to
/// render the `not installed at scope <scope> (<path>)` suffix.
fn scope_project_path(scope: &str, cwd: &Path) -> Option<PathBuf> {
    if scope == "project" || scope == "local" {
        Some(cwd.to_path_buf())
    } else {
        None
    }
}

/// Resolve a full `name@market` id to its marketplace + on-disk plugin source
/// dir (mirrors `iP`): a bare id (no `@`) or a market/plugin the registry can't
/// resolve ⇒ `None` (⇒ the caller's `Plugin "<name>" not found`).
fn resolve_source(plugins_dir: &Path, id: &str) -> Option<(String, PathBuf)> {
    let (name, market) = split_id(id);
    let market = market?;
    let registry = load_registry(plugins_dir);
    let root = registry.get(market).and_then(install_location)?;
    let rel = marketplace_entry_source(&root, name)?;
    Some((market.to_string(), root.join(rel)))
}

/// The version string from a plugin's own manifest (`unknown` when absent).
fn plugin_version(plugin_src: &Path) -> String {
    std::fs::read_to_string(
        plugin_src
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("plugin.json"),
    )
    .ok()
    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from))
    .unwrap_or_else(|| "unknown".to_string())
}

/// `plugin update <plugin> [--scope]` — re-materialize an installed plugin from
/// its marketplace and bump the on-disk record. 1:1 with claude-code 2.1.201
/// (`ukc`/`Rvt`/`Prf`, directory sources; probed against the real binary).
///
/// Output is two lines: a `Checking for updates for plugin "<arg>" at <scope>
/// scope…` header (always, once the scope parses) followed by the result:
///
/// * up-to-date → `✔ <name> is already at the latest version (<version>).`
/// * a newer marketplace version → re-copy the plugin tree into the versioned
///   cache `cache/<market>/<name>/<version>/`, point the record's `installPath`
///   at it, set `version` + bump `lastUpdated` (keeping `installedAt`), orphan
///   the previous cache dir (a `.orphaned_at` marker, when unreferenced) →
///   `✔ Plugin "<name>" updated from <old> to <new> for scope <scope>. Restart
///   to apply changes.` (`enabledPlugins` is NOT touched.)
///
/// Errors (each after the header, `✘ Failed to update plugin "<arg>": …`):
/// a plugin not resolvable in any marketplace → `Plugin "<name>" not found`;
/// resolvable but never installed → `Plugin "<name>" is not installed`; installed
/// but not at the requested scope → `Plugin "<name>" is not installed at scope
/// <scope>` (with ` (<cwd>)` for project/local). An unknown `--scope` errors
/// (no header) with the update-family `Invalid scope …` wording.
pub fn run_update(
    arg: &str,
    scope: &str,
    plugins_dir: &Path,
    _home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let scope = parse_update_scope(scope)?;
    let header = format!("Checking for updates for plugin \"{arg}\" at {scope} scope\u{2026}\n");
    match update_inner(arg, scope, plugins_dir, cwd) {
        Ok(msg) => Ok(format!("{header}\u{2714} {msg}")),
        Err(reason) => Err(format!("{header}{}", fail("update", arg, &reason))),
    }
}

/// The core update resolution + materialization (sans the header/`✔`/`✘`
/// framing). `Ok` carries the bare success sentence; `Err` the bare reason.
fn update_inner(arg: &str, scope: &str, plugins_dir: &Path, cwd: &Path) -> Result<String, String> {
    // Display name for every message = the ORIGINAL arg's name-part (matches the
    // binary's `n` from `zo(e)`, which case-resolution does not rewrite).
    let (name, market) = split_id(arg);

    // Resolve the id against the installed keys (exact, then case-insensitive —
    // `Loe`); a bare name that matches no key stays bare (⇒ "not found").
    let mut installed = load_installed(plugins_dir);
    let base = match market {
        Some(m) => format!("{name}@{m}"),
        None => arg.to_string(),
    };
    let id = installed
        .get("plugins")
        .and_then(Value::as_object)
        .and_then(|p| {
            p.keys()
                .find(|k| k.as_str() == base)
                .cloned()
                .or_else(|| p.keys().find(|k| k.eq_ignore_ascii_case(&base)).cloned())
        })
        .unwrap_or(base);

    // `iP`: the plugin must resolve to a marketplace source, else "not found".
    let Some((market_name, plugin_src)) = resolve_source(plugins_dir, &id) else {
        return Err(format!("Plugin \"{name}\" not found"));
    };

    // Must have at least one installed record for the id.
    let has_records = installed
        .get("plugins")
        .and_then(|p| p.get(&id))
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    if !has_records {
        return Err(format!("Plugin \"{name}\" is not installed"));
    }

    // Filter by scope; the projectPath only disambiguates WHICH record when
    // several share the scope (an empty scope set ⇒ "not installed at scope").
    let project_path = scope_project_path(scope, cwd);
    let want_pp = project_path.as_ref().map(|p| p.display().to_string());
    let (idx, old_version, old_path) = {
        let records = installed
            .get("plugins")
            .and_then(|p| p.get(&id))
            .and_then(Value::as_array)
            .unwrap();
        let scoped: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.get("scope").and_then(Value::as_str) == Some(scope))
            .map(|(i, _)| i)
            .collect();
        if scoped.is_empty() {
            let disp = match &want_pp {
                Some(p) => format!("{scope} ({p})"),
                None => scope.to_string(),
            };
            return Err(format!(
                "Plugin \"{name}\" is not installed at scope {disp}"
            ));
        }
        let idx = scoped
            .iter()
            .copied()
            .find(|&i| {
                let rp = records[i].get("projectPath").and_then(Value::as_str);
                match &want_pp {
                    Some(w) => rp == Some(w.as_str()),
                    None => rp.is_none(),
                }
            })
            .unwrap_or(scoped[0]);
        let ov = records[idx]
            .get("version")
            .and_then(Value::as_str)
            .map(String::from);
        let op = records[idx]
            .get("installPath")
            .and_then(Value::as_str)
            .map(String::from);
        (idx, ov, op)
    };

    // The marketplace's current version + the cache dir it would land in.
    let new_version = plugin_version(&plugin_src);
    let id_name = split_id(&id).0;
    let dest = plugins_dir
        .join("cache")
        .join(sanitize(&market_name, false))
        .join(sanitize(id_name, false))
        .join(sanitize(&new_version, true));
    let dest_str = dest.display().to_string();

    // Already current (same version, or the record already points at the target
    // cache dir) — no copy, no record change.
    if new_version != "unknown"
        && (old_version.as_deref() == Some(new_version.as_str())
            || old_path.as_deref() == Some(dest_str.as_str()))
    {
        return Ok(format!(
            "{name} is already at the latest version ({new_version})."
        ));
    }

    // Re-materialize into the (new) versioned cache.
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(&plugin_src, &dest).map_err(|e| e.to_string())?;

    // Bump the record in place (installPath/version/lastUpdated; installedAt and
    // scope are preserved).
    let now = iso_now();
    if let Some(rec) = installed
        .get_mut("plugins")
        .and_then(Value::as_object_mut)
        .and_then(|p| p.get_mut(&id))
        .and_then(Value::as_array_mut)
        .and_then(|a| a.get_mut(idx))
        .and_then(Value::as_object_mut)
    {
        rec.insert("installPath".to_string(), Value::String(dest_str.clone()));
        rec.insert("version".to_string(), Value::String(new_version.clone()));
        rec.insert("lastUpdated".to_string(), Value::String(now));
    }
    write_installed(plugins_dir, &installed)?;

    // Orphan the previous cache dir when it changed and no other record still
    // references it (marker only; the deferred sweep that deletes it is not
    // ported — same as uninstall).
    if let Some(old) = old_path.as_deref() {
        if old != dest_str {
            let still_referenced = installed
                .get("plugins")
                .and_then(Value::as_object)
                .is_some_and(|p| {
                    p.values()
                        .filter_map(Value::as_array)
                        .flatten()
                        .any(|r| r.get("installPath").and_then(Value::as_str) == Some(old))
                });
            if !still_referenced {
                let _ = std::fs::write(Path::new(old).join(".orphaned_at"), iso_now());
            }
        }
    }

    let scope_disp = match &want_pp {
        Some(p) => format!("{scope} ({p})"),
        None => scope.to_string(),
    };
    let old_disp = old_version.as_deref().unwrap_or("unknown");
    Ok(format!(
        "Plugin \"{name}\" updated from {old_disp} to {new_version} for scope {scope_disp}. Restart to apply changes."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        cwd: PathBuf,
        plugins: PathBuf,
        market: PathBuf,
    }

    /// A home/cwd/plugins env with a registered directory marketplace `mymkt`
    /// carrying a plugin `hello` v1.2.3 (a `commands/hi.md` component).
    fn env() -> Env {
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
            r#"{"name":"mymkt","owner":{"name":"me"},"plugins":[{"name":"hello","source":"./plugins/hello"}]}"#,
        )
        .unwrap();
        let pdir = market.join("plugins").join("hello");
        std::fs::create_dir_all(pdir.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::create_dir_all(pdir.join("commands")).unwrap();
        std::fs::write(
            pdir.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{"name":"hello","version":"1.2.3"}"#,
        )
        .unwrap();
        std::fs::write(pdir.join("commands").join("hi.md"), "# hi").unwrap();
        // Register the marketplace (directory source).
        let abs = std::fs::canonicalize(&market)
            .unwrap()
            .display()
            .to_string();
        std::fs::write(
            plugins.join("known_marketplaces.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "mymkt": {"source": {"source": "directory", "path": abs}, "installLocation": abs}
            }))
            .unwrap(),
        )
        .unwrap();
        Env {
            _tmp: tmp,
            home,
            cwd,
            plugins,
            market,
        }
    }

    fn user_settings(e: &Env) -> Value {
        serde_json::from_str(&std::fs::read_to_string(e.home.join("settings.json")).unwrap())
            .unwrap()
    }

    fn installed_db(e: &Env) -> Value {
        serde_json::from_str(&std::fs::read_to_string(installed_path(&e.plugins)).unwrap()).unwrap()
    }

    #[test]
    fn install_materializes_records_and_enables() {
        let e = env();
        let msg = run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Installing plugin \"hello@mymkt\"...✔ Successfully installed plugin: hello@mymkt (scope: user)"
        );
        // Cache materialized with the component.
        let cached = e.plugins.join("cache/mymkt/hello/1.2.3");
        assert!(cached.join(".lingxi-plugin/plugin.json").exists());
        assert!(cached.join("commands/hi.md").exists());
        // v2 record.
        let db = installed_db(&e);
        let rec = &db["plugins"]["hello@mymkt"][0];
        assert_eq!(rec["scope"], "user");
        assert_eq!(rec["version"], "1.2.3");
        assert_eq!(rec["installPath"], cached.display().to_string());
        assert!(rec["installedAt"].is_string());
        // Enabled.
        assert_eq!(
            user_settings(&e)["enabledPlugins"]["hello@mymkt"],
            Value::Bool(true)
        );
    }

    #[test]
    fn install_bare_name_resolves_marketplace() {
        let e = env();
        let msg = run_install("hello", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Installing plugin \"hello\"...✔ Successfully installed plugin: hello@mymkt (scope: user)"
        );
    }

    #[test]
    fn install_already_installed() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Installing plugin \"hello@mymkt\"...✔ Plugin \"hello@mymkt\" is already installed (scope: user)"
        );
    }

    #[test]
    fn install_no_marketplace() {
        let e = env();
        std::fs::remove_file(e.plugins.join("known_marketplaces.json")).unwrap();
        let err = run_install("foo", None, &[], &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Installing plugin \"foo\"...✘ Failed to install plugin \"foo\": Plugin \"foo\" not found in any configured marketplace"
        );
    }

    #[test]
    fn install_unknown_marketplace() {
        let e = env();
        let err = run_install("foo@bar", None, &[], &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Installing plugin \"foo@bar\"...✘ Failed to install plugin \"foo@bar\": Plugin \"foo\" not found in marketplace \"bar\". Your local copy may be out of date — try `lingxi-cli plugin marketplace update bar`."
        );
    }

    #[test]
    fn install_invalid_scope() {
        let e = env();
        let err = run_install(
            "hello@mymkt",
            Some("bogus"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap_err();
        // Bare scope error — NO "Installing plugin …" prefix (matches the binary).
        assert_eq!(
            err,
            "Invalid scope: bogus. Must be one of: user, project, local."
        );
    }

    #[test]
    fn uninstall_removes_record_deletes_key_and_orphans() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_uninstall(
            "hello", None, false, false, true, &e.plugins, &e.home, &e.cwd,
        )
        .unwrap();
        assert_eq!(
            msg,
            "✔ Successfully uninstalled plugin: hello (scope: user)"
        );
        // Record gone.
        assert_eq!(installed_db(&e)["plugins"], serde_json::json!({}));
        // enabledPlugins KEY DELETED (not set false).
        assert_eq!(user_settings(&e)["enabledPlugins"], serde_json::json!({}));
        // Cache orphaned (marker written, tree kept).
        assert!(e
            .plugins
            .join("cache/mymkt/hello/1.2.3/.orphaned_at")
            .exists());
        assert!(e
            .plugins
            .join("cache/mymkt/hello/1.2.3/commands/hi.md")
            .exists());
    }

    #[test]
    fn uninstall_not_installed() {
        let e = env();
        let err = run_uninstall("foo", None, false, false, true, &e.plugins, &e.home, &e.cwd)
            .unwrap_err();
        assert_eq!(
            err,
            "✘ Failed to uninstall plugin \"foo\": Plugin \"foo\" not found in installed plugins"
        );
    }

    /// Bump the marketplace's `hello` plugin to `version`, adding a marker file.
    fn bump_market_hello(e: &Env, version: &str) {
        std::fs::write(
            e.market
                .join("plugins")
                .join("hello")
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            format!(r#"{{"name":"hello","version":"{version}"}}"#),
        )
        .unwrap();
        std::fs::write(
            e.market
                .join("plugins")
                .join("hello")
                .join("commands")
                .join("new.md"),
            "# new",
        )
        .unwrap();
    }

    #[test]
    fn update_bumps_version_recopies_and_orphans_old() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let installed_at = installed_db(&e)["plugins"]["hello@mymkt"][0]["installedAt"].clone();
        bump_market_hello(&e, "2.0.0");

        let msg = run_update("hello@mymkt", "user", &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Checking for updates for plugin \"hello@mymkt\" at user scope\u{2026}\n\
             \u{2714} Plugin \"hello\" updated from 1.2.3 to 2.0.0 for scope user. Restart to apply changes."
        );

        // New version materialized (with the new component), old version orphaned.
        let new_cache = e.plugins.join("cache/mymkt/hello/2.0.0");
        assert!(new_cache.join(".lingxi-plugin/plugin.json").exists());
        assert!(new_cache.join("commands/new.md").exists());
        assert!(e
            .plugins
            .join("cache/mymkt/hello/1.2.3/.orphaned_at")
            .exists());
        assert!(e
            .plugins
            .join("cache/mymkt/hello/1.2.3/commands/hi.md")
            .exists());

        // Record bumped: version + installPath + lastUpdated changed; installedAt kept.
        let db = installed_db(&e);
        let rec = &db["plugins"]["hello@mymkt"][0];
        assert_eq!(rec["scope"], "user");
        assert_eq!(rec["version"], "2.0.0");
        assert_eq!(rec["installPath"], new_cache.display().to_string());
        assert_eq!(rec["installedAt"], installed_at); // installedAt preserved
        assert!(rec["lastUpdated"].is_string());
        // enabledPlugins untouched by update.
        assert_eq!(
            user_settings(&e)["enabledPlugins"]["hello@mymkt"],
            Value::Bool(true)
        );
    }

    #[test]
    fn update_already_latest() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let msg = run_update("hello@mymkt", "user", &e.plugins, &e.home, &e.cwd).unwrap();
        assert_eq!(
            msg,
            "Checking for updates for plugin \"hello@mymkt\" at user scope\u{2026}\n\
             \u{2714} hello is already at the latest version (1.2.3)."
        );
    }

    #[test]
    fn update_bare_name_not_found() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let err = run_update("hello", "user", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Checking for updates for plugin \"hello\" at user scope\u{2026}\n\
             ✘ Failed to update plugin \"hello\": Plugin \"hello\" not found"
        );
    }

    #[test]
    fn update_unknown_plugin_in_marketplace_not_found() {
        let e = env();
        let err = run_update("foo@mymkt", "user", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Checking for updates for plugin \"foo@mymkt\" at user scope\u{2026}\n\
             ✘ Failed to update plugin \"foo@mymkt\": Plugin \"foo\" not found"
        );
    }

    #[test]
    fn update_in_marketplace_but_not_installed() {
        let e = env();
        // Add a `world` plugin to the marketplace but never install it.
        std::fs::write(
            e.market.join(branding::PLUGIN_MANIFEST_DIR).join("marketplace.json"),
            r#"{"name":"mymkt","owner":{"name":"me"},"plugins":[{"name":"hello","source":"./plugins/hello"},{"name":"world","source":"./plugins/world"}]}"#,
        )
        .unwrap();
        let err = run_update("world@mymkt", "user", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Checking for updates for plugin \"world@mymkt\" at user scope\u{2026}\n\
             ✘ Failed to update plugin \"world@mymkt\": Plugin \"world\" is not installed"
        );
    }

    #[test]
    fn update_wrong_scope_managed() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let err = run_update("hello@mymkt", "managed", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Checking for updates for plugin \"hello@mymkt\" at managed scope\u{2026}\n\
             ✘ Failed to update plugin \"hello@mymkt\": Plugin \"hello\" is not installed at scope managed"
        );
    }

    #[test]
    fn update_wrong_scope_project_shows_cwd() {
        let e = env();
        run_install("hello@mymkt", None, &[], &e.plugins, &e.home, &e.cwd).unwrap();
        let err = run_update("hello@mymkt", "project", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            format!(
                "Checking for updates for plugin \"hello@mymkt\" at project scope\u{2026}\n\
                 ✘ Failed to update plugin \"hello@mymkt\": Plugin \"hello\" is not installed at scope project ({})",
                e.cwd.display()
            )
        );
    }

    #[test]
    fn update_invalid_scope_has_no_header() {
        let e = env();
        let err = run_update("hello@mymkt", "bogus", &e.plugins, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Invalid scope \"bogus\". Valid scopes: user, project, local, managed"
        );
    }

    #[test]
    fn install_second_scope_appends_with_project_path() {
        let e = env();
        run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let msg = run_install(
            "hello@mymkt",
            Some("project"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        assert!(
            msg.contains("Successfully installed plugin: hello@mymkt (scope: project)"),
            "{msg}"
        );
        let db = installed_db(&e);
        let arr = db["plugins"]["hello@mymkt"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["scope"], "user");
        assert_eq!(arr[1]["scope"], "project");
        // project record carries projectPath; user record does not.
        assert!(arr[0].get("projectPath").is_none());
        assert!(arr[1].get("projectPath").is_some());
    }

    #[test]
    fn install_same_scope_twice_is_already_installed() {
        let e = env();
        run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let msg = run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        assert!(msg.contains("already installed (scope: user)"), "{msg}");
    }

    #[test]
    fn uninstall_scope_mismatch_names_actual_scope() {
        let e = env();
        run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let err = run_uninstall(
            "hello@mymkt",
            Some("project"),
            false,
            false,
            true,
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap_err();
        assert_eq!(
            err,
            "✘ Failed to uninstall plugin \"hello@mymkt\": Plugin \"hello@mymkt\" is installed in user scope, not project. Use --scope user to uninstall."
        );
        // The user record is untouched.
        assert!(installed_db(&e)["plugins"].get("hello@mymkt").is_some());
    }

    #[test]
    fn uninstall_removes_only_matching_scope() {
        let e = env();
        run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        run_install(
            "hello@mymkt",
            Some("project"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        run_uninstall(
            "hello@mymkt",
            Some("project"),
            false,
            false,
            true,
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let db = installed_db(&e);
        let arr = db["plugins"]["hello@mymkt"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["scope"], "user");
    }
}
