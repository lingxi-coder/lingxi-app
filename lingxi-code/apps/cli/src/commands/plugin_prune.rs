//! `plugin prune` (alias `autoremove`) — remove auto-installed plugin
//! dependencies that are no longer reachable from any explicitly-installed
//! plugin, 1:1 with claude-code 2.1.201 (probed against the real binary in an
//! isolated `$CLAUDE_CONFIG_DIR`).
//!
//! A v2 `installed_plugins.json` record carries an `auto: true` marker
//! ("True when pulled in as a dependency. Eligible for orphan sweep."). Prune
//! computes, at the chosen scope, the set of `auto`-installed plugins that are
//! NOT reachable — via the `manifest.dependencies` graph — from any
//! non-`auto` (explicitly installed) plugin, and removes them.
//!
//! Output (branding-adjusted — the oracle's `claude` → `lingxi-cli`):
//!
//! * unresolvable graph (some record's plugin fails to load) →
//!   `Skipped — cannot determine orphans: <ids> failed to load. Fix or uninstall, then retry.`
//! * nothing auto-installed → `Nothing to prune (no auto-installed plugins at <scope> scope).`
//! * auto-installed present but all still needed →
//!   `Nothing to prune (N auto-installed plugin(s) at <scope> scope, all still needed).`
//! * orphans found →
//!   ```text
//!   N auto-installed plugin(s) no longer needed at <scope> scope:
//!     <id> (<version>)
//!   ```
//!   then, per mode: `\n(dry run — nothing removed)` (`--dry-run`);
//!   `\nNot a TTY — run \`lingxi-cli plugin prune[ --scope S] -y\` to remove.`
//!   (non-interactive without `-y`); an interactive `Remove? [y/N]` prompt
//!   (`Aborted.` on decline); otherwise the removal runs and returns
//!   `Removed N auto-installed plugin(s): <names>`.
//!
//! An unknown `--scope` yields the install-family wording
//! `Invalid scope: <s>. Must be one of: user, project, local.`
//!
//! Residual: this port's `plugin install` does not yet write the `auto: true`
//! marker (dependency auto-install is not ported), so in the common case
//! `autoCount == 0` and the empty-case line is what runs end-to-end. The full
//! scan/removal path is implemented and unit-tested against synthetic v2 DBs;
//! the "loaded plugins" set is sourced from each record's own `installPath`
//! manifest (rather than the live `PluginManager` registry, which is not wired
//! into the CLI seam), so a record with a missing/unreadable `installPath`
//! manifest is treated as failed-to-load (matching the oracle's unloadable
//! branch).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use migrations::settings_update::{read_settings_map, update_settings};
use serde_json::{Map, Value};

use crate::commands::plugin_settings::Scope;

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

/// Parse the prune `--scope` (default `user`); invalid-scope wording matches the
/// install family (`Invalid scope: <s>. Must be one of: user, project, local.`).
fn parse_scope(scope: &str) -> Result<Scope, String> {
    Scope::parse(scope)
        .ok_or_else(|| format!("Invalid scope: {scope}. Must be one of: user, project, local."))
}

/// The scope's `projectPath` selector: `Some(cwd)` for project/local,
/// `None` for user (mirrors the oracle's `Svt`, which returns the originalCwd
/// for project/local and `undefined` for user).
fn scope_project_path(scope: Scope, cwd: &Path) -> Option<String> {
    match scope {
        Scope::User => None,
        Scope::Project | Scope::Local => Some(cwd.display().to_string()),
    }
}

/// `singular`/`plural` selector (mirrors the oracle's `on(count, s, p)`).
fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// The pre-`@` display name of a `plugin@marketplace` id (mirrors `zo(id).name`).
fn name_of(id: &str) -> &str {
    id.split('@').next().unwrap_or(id)
}

/// The marketplace segment of a `plugin@marketplace` id (the SECOND `@`-part,
/// matching the oracle's `zo`, which keys off `split("@")[1]`).
fn marketplace_of(id: &str) -> Option<&str> {
    id.split('@').nth(1)
}

/// Resolve a bare dependency spec to a full id in the context of its declaring
/// plugin's id (mirrors the oracle's `e1`): a spec that already names a
/// marketplace is returned verbatim; otherwise it inherits the source plugin's
/// marketplace — unless that marketplace is a non-catalog sentinel
/// (`inline` / `skills-dir`), in which case the bare spec is kept.
fn resolve_dep(dep: &str, source_id: &str) -> String {
    if marketplace_of(dep).is_some() {
        return dep.to_string();
    }
    match marketplace_of(source_id) {
        Some(m) if m != "inline" && m != "skills-dir" => format!("{dep}@{m}"),
        _ => dep.to_string(),
    }
}

/// The record within an id's record-array that matches the active scope +
/// projectPath (mirrors `find(u => u.scope===scope && u.projectPath===pp)`).
fn scoped_record<'a>(
    records: &'a [Value],
    scope: Scope,
    project_path: &Option<String>,
) -> Option<&'a Value> {
    records.iter().find(|r| {
        let rec_scope = r.get("scope").and_then(Value::as_str);
        if rec_scope != Some(scope.label()) {
            return false;
        }
        let rec_pp = r.get("projectPath").and_then(Value::as_str);
        match project_path {
            Some(p) => rec_pp == Some(p.as_str()),
            None => rec_pp.is_none(),
        }
    })
}

/// The outcome of the orphan scan (mirrors the oracle's `Ria`).
struct Scan {
    /// Auto-installed ids that are no longer reachable (in DB order).
    orphans: Vec<String>,
    /// Ids whose plugin failed to load (blocks any pruning when non-empty).
    unloadable: Vec<String>,
    /// Total number of auto-installed plugins at the scope.
    auto_count: usize,
}

/// The plugins object of the DB (`{id: [record, …]}`), or an empty map.
fn plugins_obj(db: &Value) -> Map<String, Value> {
    db.get("plugins")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A plugin's declared dependency specs, read from its `installPath` manifest
/// (`<installPath>/.lingxi-plugin/plugin.json` → `dependencies: []`). `None`
/// when the manifest is missing or unreadable (⇒ the plugin failed to load).
fn load_dependencies(install_path: &str) -> Option<Vec<String>> {
    let manifest = Path::new(install_path)
        .join(branding::PLUGIN_MANIFEST_DIR)
        .join("plugin.json");
    let raw = std::fs::read_to_string(manifest).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    // `dependencies` is optional; an absent key is an empty dependency list (a
    // loadable plugin), NOT a load failure.
    let deps = value
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Some(deps)
}

/// Compute the orphan set at `scope` (mirrors `Ria`): auto-installed plugins
/// unreachable from any manual plugin via the `dependencies` graph.
fn scan(db: &Value, scope: Scope, project_path: &Option<String>) -> Scan {
    let plugins = plugins_obj(db);

    // Partition the scope's records into manual (`o`) and auto (`s`), in DB order.
    let mut manual: Vec<String> = Vec::new();
    let mut auto: Vec<String> = Vec::new();
    for (id, records) in &plugins {
        let arr = match records.as_array() {
            Some(a) => a,
            None => continue,
        };
        let rec = match scoped_record(arr, scope, project_path) {
            Some(r) => r,
            None => continue,
        };
        if rec.get("auto").and_then(Value::as_bool) == Some(true) {
            auto.push(id.clone());
        } else {
            manual.push(id.clone());
        }
    }

    if auto.is_empty() {
        return Scan {
            orphans: Vec::new(),
            unloadable: Vec::new(),
            auto_count: 0,
        };
    }
    let auto_count = auto.len();

    // Load every in-scope plugin's dependency list from its installPath manifest.
    // Any that fails to load blocks pruning entirely.
    let mut loaded: HashMap<String, Vec<String>> = HashMap::new();
    let mut unloadable: Vec<String> = Vec::new();
    for id in manual.iter().chain(auto.iter()) {
        let install_path = plugins
            .get(id)
            .and_then(Value::as_array)
            .and_then(|a| scoped_record(a, scope, project_path))
            .and_then(|r| r.get("installPath").and_then(Value::as_str))
            .unwrap_or("");
        match load_dependencies(install_path) {
            Some(deps) => {
                loaded.insert(id.clone(), deps);
            }
            None => unloadable.push(id.clone()),
        }
    }
    if !unloadable.is_empty() {
        return Scan {
            orphans: Vec::new(),
            unloadable,
            auto_count,
        };
    }

    // Reachability DFS from every manual plugin, following resolved dependencies.
    let mut reachable: HashSet<String> = HashSet::new();
    for id in &manual {
        dfs(id, &loaded, &mut reachable);
    }

    // Orphans = auto plugins not reached (DB order preserved).
    let orphans: Vec<String> = auto
        .into_iter()
        .filter(|id| !reachable.contains(id))
        .collect();

    Scan {
        orphans,
        unloadable: Vec::new(),
        auto_count,
    }
}

/// Mark `id` and everything it transitively depends on as reachable.
fn dfs(id: &str, loaded: &HashMap<String, Vec<String>>, reachable: &mut HashSet<String>) {
    if reachable.contains(id) {
        return;
    }
    reachable.insert(id.to_string());
    if let Some(deps) = loaded.get(id) {
        for dep in deps {
            let resolved = resolve_dep(dep, id);
            dfs(&resolved, loaded, reachable);
        }
    }
}

/// Read a scope's `enabledPlugins` map.
fn read_enabled(path: &Path) -> Map<String, Value> {
    read_settings_map(path)
        .ok()
        .and_then(|m| m.get("enabledPlugins").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// Delete an `enabledPlugins` entry at a scope (prune drops the key, matching
/// the oracle's `u[id] = void 0`).
fn delete_enabled(scope: Scope, home: &Path, cwd: &Path, id: &str) -> Result<(), String> {
    let path = scope.path(home, cwd);
    let mut map = read_enabled(&path);
    if map.remove(id).is_none() {
        return Ok(());
    }
    update_settings(
        &path,
        vec![("enabledPlugins".to_string(), Some(Value::Object(map)))],
    )
}

/// Remove the orphan records at the scope, delete their `enabledPlugins` keys,
/// and delete their materialized `installPath` (and per-id data) directories
/// (mirrors the oracle's `ZWl` with `deleteDataDir: true`).
fn remove_orphans(
    orphans: &[String],
    scope: Scope,
    project_path: &Option<String>,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<(), String> {
    let mut db = load_installed(plugins_dir);

    for id in orphans {
        // Delete the on-disk versioned dir (and data dir) for the scoped record.
        if let Some(arr) = db
            .get("plugins")
            .and_then(|p| p.get(id))
            .and_then(Value::as_array)
        {
            if let Some(rec) = scoped_record(arr, scope, project_path) {
                if let Some(path) = rec.get("installPath").and_then(Value::as_str) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
        let _ = std::fs::remove_dir_all(plugins_dir.join("data").join(id));

        // Drop the scope+projectPath record; remove the key if none remain.
        if let Some(plugins) = db.get_mut("plugins").and_then(Value::as_object_mut) {
            if let Some(Value::Array(arr)) = plugins.get_mut(id) {
                arr.retain(|r| {
                    scoped_record(std::slice::from_ref(r), scope, project_path).is_none()
                });
                if arr.is_empty() {
                    plugins.remove(id);
                }
            }
        }

        let _ = delete_enabled(scope, home, cwd, id);
    }

    write_installed(plugins_dir, &db)
}

/// Read a `y`/`yes` (case-insensitive) confirmation line from stdin.
fn read_yes_from_stdin() -> bool {
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// `plugin prune [--dry-run] [-y] [--scope S]`.
///
/// Returns the message to print (`Ok`) or the formatted error line (`Err`).
pub fn run_prune(
    dry_run: bool,
    yes: bool,
    scope: &str,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    run_prune_inner(
        dry_run,
        yes,
        scope,
        plugins_dir,
        home,
        cwd,
        is_tty,
        &mut read_yes_from_stdin,
    )
}

/// Testable core: `is_tty` and the confirmation reader are injected so the
/// interactive path can be exercised deterministically.
#[allow(clippy::too_many_arguments)]
fn run_prune_inner(
    dry_run: bool,
    yes: bool,
    scope_str: &str,
    plugins_dir: &Path,
    home: &Path,
    cwd: &Path,
    is_tty: bool,
    confirm: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    let scope = parse_scope(scope_str)?;
    let label = scope.label();
    let project_path = scope_project_path(scope, cwd);
    let db = load_installed(plugins_dir);
    let result = scan(&db, scope, &project_path);

    // Unresolvable graph — cannot safely determine orphans.
    if !result.unloadable.is_empty() {
        return Ok(format!(
            "Skipped \u{2014} cannot determine orphans: {} failed to load. Fix or uninstall, then retry.",
            result.unloadable.join(", ")
        ));
    }

    // Nothing to remove.
    if result.orphans.is_empty() {
        if result.auto_count == 0 {
            return Ok(format!(
                "Nothing to prune (no auto-installed plugins at {label} scope)."
            ));
        }
        return Ok(format!(
            "Nothing to prune ({} auto-installed {} at {label} scope, all still needed).",
            result.auto_count,
            plural(result.auto_count, "plugin", "plugins"),
        ));
    }

    // Build the orphan listing (2-space indent; version from the scoped record).
    let plugins = plugins_obj(&db);
    let lines: Vec<String> = result
        .orphans
        .iter()
        .map(|id| {
            let version = plugins
                .get(id)
                .and_then(Value::as_array)
                .and_then(|a| scoped_record(a, scope, &project_path))
                .and_then(|r| r.get("version").and_then(Value::as_str))
                .filter(|v| !v.is_empty());
            match version {
                Some(v) => format!("  {id} ({v})"),
                None => format!("  {id}"),
            }
        })
        .collect();
    let listing = format!(
        "{} auto-installed {} no longer needed at {label} scope:\n{}",
        result.orphans.len(),
        plural(result.orphans.len(), "plugin", "plugins"),
        lines.join("\n"),
    );

    if dry_run {
        return Ok(format!("{listing}\n(dry run \u{2014} nothing removed)"));
    }

    if !yes {
        if !is_tty {
            let scope_flag = if scope == Scope::User {
                String::new()
            } else {
                format!(" --scope {label}")
            };
            return Ok(format!(
                "{listing}\nNot a TTY \u{2014} run `lingxi-cli plugin prune{scope_flag} -y` to remove."
            ));
        }
        print!("{listing}\nRemove? [y/N] ");
        let _ = std::io::stdout().flush();
        if !confirm() {
            return Ok("Aborted.".to_string());
        }
    }

    remove_orphans(
        &result.orphans,
        scope,
        &project_path,
        plugins_dir,
        home,
        cwd,
    )?;

    Ok(format!(
        "Removed {} auto-installed {}: {}",
        result.orphans.len(),
        plural(result.orphans.len(), "plugin", "plugins"),
        result
            .orphans
            .iter()
            .map(|id| name_of(id).to_string())
            .collect::<Vec<_>>()
            .join(", "),
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
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("proj");
        let plugins = home.join("plugins");
        for d in [&home, &cwd, &plugins] {
            std::fs::create_dir_all(d).unwrap();
        }
        Env {
            _tmp: tmp,
            home,
            cwd,
            plugins,
        }
    }

    /// Materialize a plugin's installPath manifest (`.lingxi-plugin/plugin.json`)
    /// with the given name/version/dependencies, and return the installPath.
    fn materialize(e: &Env, id: &str, version: &str, deps: &[&str]) -> String {
        let path = e.plugins.join("cache").join(id.replace('@', "__"));
        std::fs::create_dir_all(path.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        let manifest = serde_json::json!({
            "name": name_of(id),
            "version": version,
            "dependencies": deps,
        });
        std::fs::write(
            path.join(branding::PLUGIN_MANIFEST_DIR).join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        path.display().to_string()
    }

    /// Write a v2 installed DB from `(id, version, installPath, auto)` records
    /// (all at `user` scope).
    fn write_db(e: &Env, records: &[(&str, &str, &str, bool)]) {
        let mut plugins = serde_json::Map::new();
        for (id, version, install_path, auto) in records {
            plugins.insert(
                (*id).to_string(),
                serde_json::json!([{
                    "scope": "user",
                    "installPath": install_path,
                    "version": version,
                    "installedAt": "2026-01-01T00:00:00.000Z",
                    "lastUpdated": "2026-01-01T00:00:00.000Z",
                    "auto": auto,
                }]),
            );
        }
        std::fs::write(
            installed_path(&e.plugins),
            serde_json::to_string_pretty(&serde_json::json!({"version": 2, "plugins": plugins}))
                .unwrap(),
        )
        .unwrap();
    }

    fn db_json(e: &Env) -> Value {
        serde_json::from_str(&std::fs::read_to_string(installed_path(&e.plugins)).unwrap()).unwrap()
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn empty_case_no_auto_installed() {
        let e = env();
        let msg = run_prune_inner(
            false, false, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "Nothing to prune (no auto-installed plugins at user scope)."
        );
    }

    #[test]
    fn empty_case_dry_run_and_scope_wording() {
        let e = env();
        let msg = run_prune_inner(
            true, false, "project", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "Nothing to prune (no auto-installed plugins at project scope)."
        );
    }

    #[test]
    fn invalid_scope() {
        let e = env();
        let err = run_prune_inner(
            false, false, "bogus", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap_err();
        assert_eq!(
            err,
            "Invalid scope: bogus. Must be one of: user, project, local."
        );
    }

    #[test]
    fn all_still_needed_when_reachable() {
        let e = env();
        // Manual `app@mkt` depends on auto `dep@mkt`; dep is reachable ⇒ kept.
        let app = materialize(&e, "app@mkt", "2.0.0", &["dep"]);
        let dep = materialize(&e, "dep@mkt", "1.0.0", &[]);
        write_db(
            &e,
            &[
                ("app@mkt", "2.0.0", &app, false),
                ("dep@mkt", "1.0.0", &dep, true),
            ],
        );
        let msg = run_prune_inner(
            false, false, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "Nothing to prune (1 auto-installed plugin at user scope, all still needed)."
        );
    }

    #[test]
    fn dry_run_lists_orphan_without_removing() {
        let e = env();
        // Auto `dep@mkt` with no manual dependant ⇒ orphan.
        let dep = materialize(&e, "dep@mkt", "1.0.0", &[]);
        write_db(&e, &[("dep@mkt", "1.0.0", &dep, true)]);
        let msg = run_prune_inner(
            true, false, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "1 auto-installed plugin no longer needed at user scope:\n  dep@mkt (1.0.0)\n(dry run \u{2014} nothing removed)"
        );
        // Nothing removed.
        assert!(db_json(&e)["plugins"].get("dep@mkt").is_some());
        assert!(Path::new(&dep).exists());
    }

    #[test]
    fn non_tty_without_yes_reports_how_to_remove() {
        let e = env();
        let dep = materialize(&e, "dep@mkt", "1.0.0", &[]);
        write_db(&e, &[("dep@mkt", "1.0.0", &dep, true)]);
        let msg = run_prune_inner(
            false, false, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "1 auto-installed plugin no longer needed at user scope:\n  dep@mkt (1.0.0)\nNot a TTY \u{2014} run `lingxi-cli plugin prune -y` to remove."
        );
        // Nothing removed.
        assert!(db_json(&e)["plugins"].get("dep@mkt").is_some());
    }

    #[test]
    fn unloadable_blocks_prune() {
        let e = env();
        // installPath manifest missing ⇒ failed to load.
        write_db(&e, &[("dep@mkt", "1.0.0", "/nonexistent/dep", true)]);
        let msg = run_prune_inner(
            false, false, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(
            msg,
            "Skipped \u{2014} cannot determine orphans: dep@mkt failed to load. Fix or uninstall, then retry."
        );
    }

    #[test]
    fn yes_removes_orphan_and_cleans_state() {
        let e = env();
        // Seed enabledPlugins so we can assert the key is dropped.
        std::fs::write(
            e.home.join("settings.json"),
            r#"{"enabledPlugins":{"dep@mkt":true,"keep@mkt":true}}"#,
        )
        .unwrap();
        let dep = materialize(&e, "dep@mkt", "1.0.0", &[]);
        write_db(&e, &[("dep@mkt", "1.0.0", &dep, true)]);

        let msg = run_prune_inner(
            false, true, "user", &e.plugins, &e.home, &e.cwd, false, &mut never,
        )
        .unwrap();
        assert_eq!(msg, "Removed 1 auto-installed plugin: dep");
        // Record dropped, cache deleted, enabledPlugins key removed (others kept).
        assert!(db_json(&e)["plugins"].get("dep@mkt").is_none());
        assert!(!Path::new(&dep).exists());
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(e.home.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            settings["enabledPlugins"],
            serde_json::json!({"keep@mkt": true})
        );
    }

    #[test]
    fn interactive_decline_aborts() {
        let e = env();
        let dep = materialize(&e, "dep@mkt", "1.0.0", &[]);
        write_db(&e, &[("dep@mkt", "1.0.0", &dep, true)]);
        let mut declined = || false;
        let msg = run_prune_inner(
            false,
            false,
            "user",
            &e.plugins,
            &e.home,
            &e.cwd,
            true, // TTY ⇒ prompt path
            &mut declined,
        )
        .unwrap();
        assert_eq!(msg, "Aborted.");
        assert!(db_json(&e)["plugins"].get("dep@mkt").is_some());
    }
}
