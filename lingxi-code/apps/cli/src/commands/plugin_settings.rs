//! On-disk `settings.enabledPlugins` toggle backing `plugin enable/disable`.
//!
//! This is the CLI seam claude-code's `plugin enable/disable` uses: a
//! read-modify-write of the `enabledPlugins` allowlist in a settings.json file
//! at the chosen scope — NOT the heavy [`plugin::PluginManager`] engine-registry
//! materialization (that runs at bootstrap from the merged allowlist). The
//! allowlist is a pure map `plugin@marketplace → bool`; enabling/disabling a
//! plugin that is not installed is allowed (claude writes the entry regardless).
//!
//! Behavior is 1:1 with `claude plugin enable/disable` 2.1.201 (probed against
//! the real binary in an isolated `$CLAUDE_CONFIG_DIR`):
//!
//! * `enable a@b`   → `{"enabledPlugins":{"a@b":true}}`; stdout
//!   `✔ Successfully enabled plugin: a (scope: user)` (name = the pre-`@` part).
//! * `disable a@b`  → sets the entry to `false` (it is NOT deleted).
//! * a bare `name` (no `@`) resolves to an existing `name@*` key across the
//!   editable scopes; if none matches →
//!   `Plugin "name" not found in any editable settings scope. Use plugin@marketplace format.`
//! * enabling an already-`true` entry → `Plugin "a@b" is already enabled`;
//!   disabling an absent/`false` entry → `Plugin "a@b" is already disabled`.
//! * `disable --all` flips every currently-`true` entry to `false` across the
//!   editable scopes and reports `✔ Disabled N plugins`.
//! * an unknown `--scope` →
//!   `Invalid scope "x". Valid scopes: user, project, local`.

use std::path::{Path, PathBuf};

use migrations::settings_update::{read_settings_map, update_settings};
use serde_json::{Map, Value};

use crate::commands::plugin_policy;

/// An editable settings scope for the `enabledPlugins` allowlist.
///
/// `user` = `<lingxi-home>/settings.json`; `project` =
/// `<cwd>/.lingxi/settings.json`; `local` = `<cwd>/.lingxi/settings.local.json`.
/// (`managed` is a read-only enterprise scope and is not editable here — it is
/// only a valid `--scope` for `plugin update`, handled separately.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
    Local,
}

/// The editable scopes, in auto-detect / `--all` search order.
const EDITABLE: [Scope; 3] = [Scope::User, Scope::Project, Scope::Local];

/// The editable scopes, in auto-detect / iteration order (shared with the
/// marketplace command).
pub(crate) const SCOPES: [Scope; 3] = EDITABLE;

impl Scope {
    /// The scope's wire label (matches the `(scope: …)` success suffix).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
            Scope::Local => "local",
        }
    }

    /// Parse a `--scope` value; `None` when unrecognized (the caller emits the
    /// `Invalid scope …` error). `managed` is intentionally NOT accepted here.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Scope::User),
            "project" => Some(Scope::Project),
            "local" => Some(Scope::Local),
            _ => None,
        }
    }

    /// Resolve the settings.json path for this scope.
    pub(crate) fn path(self, home: &Path, cwd: &Path) -> PathBuf {
        match self {
            Scope::User => home.join("settings.json"),
            Scope::Project => cwd.join(branding::DOT_DIR).join("settings.json"),
            Scope::Local => cwd.join(branding::DOT_DIR).join("settings.local.json"),
        }
    }
}

/// The display name of a `plugin@marketplace` id: the segment before the first
/// `@` (a bare name is returned as-is).
fn name_of(id: &str) -> &str {
    id.split('@').next().unwrap_or(id)
}

/// Marketplace segment of a `plugin@marketplace` id.
fn marketplace_of(id: &str) -> Option<&str> {
    id.split_once('@').map(|(_, marketplace)| marketplace)
}

fn marketplace_source(
    home: &Path,
    marketplace: &str,
) -> Option<plugin_policy::MarketplaceSourceIdentity> {
    std::fs::read_to_string(home.join("plugins").join("known_marketplaces.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|registry| registry.get(marketplace).cloned())
        .as_ref()
        .and_then(plugin_policy::MarketplaceSourceIdentity::from_value)
}

/// The `enabledPlugins` map from a scope's settings file (missing key / missing
/// / malformed file ⇒ empty map).
fn read_enabled(path: &Path) -> Map<String, Value> {
    read_settings_map(path)
        .ok()
        .and_then(|m| m.get("enabledPlugins").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// The current allowlist value of `id` in a scope (`None` when the key is
/// absent or is not a bool).
fn current_value(path: &Path, id: &str) -> Option<bool> {
    read_enabled(path).get(id).and_then(Value::as_bool)
}

/// Read-modify-write `enabledPlugins[id] = value` into a scope's settings file,
/// preserving every other key (delegates to the JSON-faithful settings writer).
fn set_value(path: &Path, id: &str, value: bool) -> Result<(), String> {
    let mut map = read_enabled(path);
    map.insert(id.to_string(), Value::Bool(value));
    update_settings(
        path,
        vec![("enabledPlugins".to_string(), Some(Value::Object(map)))],
    )
}

/// Resolve a user-supplied plugin argument to a full `plugin@marketplace` id.
///
/// A value already containing `@` is a full id (returned verbatim). A bare
/// `name` is resolved by searching the editable scopes' allowlists for a key
/// whose name-part matches; the first match wins. No match →
/// `Plugin "name" not found in any editable settings scope. Use plugin@marketplace format.`
fn resolve_id(plugin: &str, home: &Path, cwd: &Path) -> Result<String, String> {
    if plugin.contains('@') {
        return Ok(plugin.to_string());
    }
    for scope in EDITABLE {
        let map = read_enabled(&scope.path(home, cwd));
        if let Some(key) = map.keys().find(|k| name_of(k) == plugin) {
            return Ok(key.clone());
        }
    }
    Err(format!(
        "Plugin \"{plugin}\" not found in any editable settings scope. \
         Use plugin@marketplace format."
    ))
}

/// The first editable scope whose allowlist carries `id` as a key (any value),
/// for auto-detect when no `--scope` is given.
fn scope_holding(id: &str, home: &Path, cwd: &Path) -> Option<Scope> {
    EDITABLE
        .into_iter()
        .find(|scope| read_enabled(&scope.path(home, cwd)).contains_key(id))
}

/// The formatted `✘ Failed to <verb> plugin "<id>": <reason>` line.
fn fail(verb: &str, id: &str, reason: &str) -> String {
    format!("✘ Failed to {verb} plugin \"{id}\": {reason}")
}

/// The `Invalid scope …` line (no ✘ prefix — matches the binary).
fn invalid_scope(s: &str) -> String {
    format!("Invalid scope \"{s}\". Valid scopes: user, project, local")
}

/// Parse an optional `--scope`, surfacing the `Invalid scope …` error.
fn parse_scope(scope: Option<&str>) -> Result<Option<Scope>, String> {
    match scope {
        None => Ok(None),
        Some(s) => Scope::parse(s).map(Some).ok_or_else(|| invalid_scope(s)),
    }
}

/// `plugin enable <plugin>` — set the allowlist entry to `true`.
///
/// Returns the `✔ Successfully enabled …` line on success, or the full
/// (already-formatted) error line for the caller to print to stderr.
pub fn run_enable(
    plugin: &str,
    scope: Option<&str>,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let requested = parse_scope(scope)?;
    let id = resolve_id(plugin, home, cwd).map_err(|reason| fail("enable", plugin, &reason))?;
    if let Some(marketplace) = marketplace_of(&id) {
        let source = marketplace_source(home, marketplace);
        plugin_policy::ensure_marketplace_source_allowed(Some(marketplace), source.as_ref())
            .map_err(|reason| fail("enable", &id, &reason))?;
    }
    let scope = requested
        .or_else(|| scope_holding(&id, home, cwd))
        .unwrap_or(Scope::User);
    let path = scope.path(home, cwd);
    if current_value(&path, &id) == Some(true) {
        return Err(fail(
            "enable",
            &id,
            &format!("Plugin \"{id}\" is already enabled"),
        ));
    }
    set_value(&path, &id, true).map_err(|e| fail("enable", &id, &e))?;
    Ok(format!(
        "✔ Successfully enabled plugin: {} (scope: {})",
        name_of(&id),
        scope.label()
    ))
}

/// `plugin disable [plugin]` / `plugin disable --all` — set entries to `false`.
///
/// With `all`, flips every currently-`true` entry to `false` across the
/// requested scope (or all editable scopes) and reports the count. Otherwise
/// disables the single resolved id at its holding scope. Returns the `✔` line
/// or the formatted error line.
pub fn run_disable(
    plugin: Option<&str>,
    scope: Option<&str>,
    all: bool,
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    let requested = parse_scope(scope)?;

    if all {
        let scopes: Vec<Scope> = requested.map_or_else(|| EDITABLE.to_vec(), |s| vec![s]);
        let mut count = 0usize;
        for scope in scopes {
            let path = scope.path(home, cwd);
            let mut map = read_enabled(&path);
            let mut changed = false;
            for value in map.values_mut() {
                if value.as_bool() == Some(true) {
                    *value = Value::Bool(false);
                    count += 1;
                    changed = true;
                }
            }
            if changed {
                update_settings(
                    &path,
                    vec![("enabledPlugins".to_string(), Some(Value::Object(map)))],
                )
                .map_err(|e| fail("disable", "--all", &e))?;
            }
        }
        return Ok(format!("✔ Disabled {count} plugins"));
    }

    let Some(plugin) = plugin else {
        return Err(
            "✘ Failed to disable plugin: no plugin specified (pass a plugin id or --all)"
                .to_string(),
        );
    };
    let id = resolve_id(plugin, home, cwd).map_err(|reason| fail("disable", plugin, &reason))?;
    // Auto-detect: the scope where the id is currently enabled (value == true).
    let scope = match requested {
        Some(s) => s,
        None => EDITABLE
            .into_iter()
            .find(|s| current_value(&s.path(home, cwd), &id) == Some(true))
            .ok_or_else(|| {
                fail(
                    "disable",
                    &id,
                    &format!("Plugin \"{id}\" is already disabled"),
                )
            })?,
    };
    let path = scope.path(home, cwd);
    if current_value(&path, &id) != Some(true) {
        return Err(fail(
            "disable",
            &id,
            &format!("Plugin \"{id}\" is already disabled"),
        ));
    }
    set_value(&path, &id, false).map_err(|e| fail("disable", &id, &e))?;
    Ok(format!(
        "✔ Successfully disabled plugin: {} (scope: {})",
        name_of(&id),
        scope.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A throwaway home + project dir pair.
    struct Env {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        cwd: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        Env {
            _tmp: tmp,
            home,
            cwd,
        }
    }

    fn user_settings(e: &Env) -> Value {
        let raw = std::fs::read_to_string(e.home.join("settings.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn enable_writes_true_to_user_and_reports_name() {
        let e = env();
        let msg = run_enable("foo@bar", None, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Successfully enabled plugin: foo (scope: user)");
        assert_eq!(
            user_settings(&e),
            json!({"enabledPlugins": {"foo@bar": true}})
        );
    }

    #[test]
    fn enable_preserves_other_top_level_keys() {
        let e = env();
        std::fs::write(
            e.home.join("settings.json"),
            r#"{"model":"opus","enabledPlugins":{"a@m":true}}"#,
        )
        .unwrap();
        run_enable("b@m", None, &e.home, &e.cwd).unwrap();
        let v = user_settings(&e);
        assert_eq!(v["model"], json!("opus"));
        assert_eq!(v["enabledPlugins"], json!({"a@m": true, "b@m": true}));
    }

    #[test]
    fn enable_already_enabled_errors() {
        let e = env();
        run_enable("foo@bar", None, &e.home, &e.cwd).unwrap();
        let err = run_enable("foo@bar", None, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "✘ Failed to enable plugin \"foo@bar\": Plugin \"foo@bar\" is already enabled"
        );
    }

    #[test]
    fn enable_bare_name_not_found_errors() {
        let e = env();
        let err = run_enable("solo", None, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "✘ Failed to enable plugin \"solo\": Plugin \"solo\" not found in any \
             editable settings scope. Use plugin@marketplace format."
        );
    }

    #[test]
    fn enable_bare_name_resolves_existing_entry() {
        let e = env();
        // Seed a disabled entry, then enable by bare name.
        std::fs::write(
            e.home.join("settings.json"),
            r#"{"enabledPlugins":{"foo@bar":false}}"#,
        )
        .unwrap();
        let msg = run_enable("foo", None, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Successfully enabled plugin: foo (scope: user)");
        assert_eq!(
            user_settings(&e),
            json!({"enabledPlugins": {"foo@bar": true}})
        );
    }

    #[test]
    fn disable_sets_false_not_delete() {
        let e = env();
        run_enable("foo@bar", None, &e.home, &e.cwd).unwrap();
        let msg = run_disable(Some("foo@bar"), None, false, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Successfully disabled plugin: foo (scope: user)");
        assert_eq!(
            user_settings(&e),
            json!({"enabledPlugins": {"foo@bar": false}})
        );
    }

    #[test]
    fn disable_absent_is_already_disabled() {
        let e = env();
        let err = run_disable(Some("neverset@x"), None, false, &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "✘ Failed to disable plugin \"neverset@x\": Plugin \"neverset@x\" is already disabled"
        );
    }

    #[test]
    fn disable_all_flips_true_entries_and_counts() {
        let e = env();
        run_enable("a@m", None, &e.home, &e.cwd).unwrap();
        run_enable("b@m", None, &e.home, &e.cwd).unwrap();
        let msg = run_disable(None, None, true, &e.home, &e.cwd).unwrap();
        assert_eq!(msg, "✔ Disabled 2 plugins");
        assert_eq!(
            user_settings(&e),
            json!({"enabledPlugins": {"a@m": false, "b@m": false}})
        );
    }

    #[test]
    fn scope_local_and_project_paths() {
        let e = env();
        run_enable("loc@m", Some("local"), &e.home, &e.cwd).unwrap();
        run_enable("prj@m", Some("project"), &e.home, &e.cwd).unwrap();
        let local: Value = serde_json::from_str(
            &std::fs::read_to_string(e.cwd.join(".lingxi").join("settings.local.json")).unwrap(),
        )
        .unwrap();
        let project: Value = serde_json::from_str(
            &std::fs::read_to_string(e.cwd.join(".lingxi").join("settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(local, json!({"enabledPlugins": {"loc@m": true}}));
        assert_eq!(project, json!({"enabledPlugins": {"prj@m": true}}));
    }

    #[test]
    fn invalid_scope_errors() {
        let e = env();
        let err = run_enable("a@b", Some("bogus"), &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "Invalid scope \"bogus\". Valid scopes: user, project, local"
        );
    }
}
