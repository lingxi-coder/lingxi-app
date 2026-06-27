//! Persisting a [`PermissionUpdate`] to a settings file (enforcement phase 3c).
//!
//! Ports claude-code `persistPermissionUpdate` (`PermissionUpdate.ts`) →
//! `addPermissionRulesToSettings` (`permissionsLoader.ts`) → `updateSettingsForSource`
//! for the `addRules` case, which is the shape the Rust port's simplified
//! [`PermissionUpdate`] (`{ rule, destination }`) models. The flow:
//!
//! 1. Only DESTINATIONS that support persistence are written — `localSettings`
//!    / `userSettings` / `projectSettings` (claude-code `supportsPersistence`).
//!    `Session` / `CliArg` are no-ops (return `Ok(false)`).
//! 2. Read the destination settings file (missing / empty → empty object;
//!    syntactically broken JSON → bail WITHOUT overwriting, matching the TS
//!    `updateSettingsForSource` error contract).
//! 3. Append the rule's string form to `permissions.{allow|deny|ask}` (keyed by
//!    the rule's behavior), de-duplicating against existing entries normalized
//!    via a parse→serialize round-trip (so a legacy alias already on disk
//!    matches its canonical form). Existing entries are preserved verbatim and
//!    ALL other settings keys are preserved (the merge is on a `serde_json::Value`
//!    so unrecognized keys survive — mirroring TS `{ ...settingsData }`).
//! 4. Write back pretty-printed + trailing newline.
//!
//! ## Divergences (documented)
//! - The `allowManagedPermissionRulesOnly` enterprise gate
//!   (`shouldAllowManagedPermissionRulesOnly`) is NOT ported (no managed-policy
//!   seam in-tree) — every persistable destination is writable here. This is
//!   the one gate that can BLOCK persistence in claude-code; absent it, the
//!   port persists where an enterprise claude-code would refuse (a hardening
//!   gap, not a default-path divergence).
//! - The Rust [`PermissionUpdate`] is the single-rule `addRules` case only;
//!   `replaceRules` / `removeRules` / `setMode` / directory updates are not
//!   modeled (the dialog + `/permissions add` only ever add one rule at a time).
//! - The write is a plain truncate-write (like the `/effort` persister), not a
//!   tmp+rename — matching the in-tree `updateSettingsForSource` analogue.
//! - The `localSettings` `.gitignore` side-effect is NOT ported: when writing
//!   `settings.local.json`, claude-code also `addFileGlobRuleToGitignore`s it
//!   (`settings.ts`) so the per-clone file is ignored. We only write the file;
//!   a project that does not already ignore `.lingxi/settings.local.json` could
//!   accidentally commit it. (Porting the gitignore write is a follow-up.)
//! - The `markInternalWrite` file-watcher hint is not ported (no settings
//!   watcher seam in-tree); a future watcher would see this self-write as an
//!   external change.

use crate::result::PermissionUpdateDestination;
use crate::rule::{PermissionBehavior, PermissionRule, PermissionRuleValue};
use crate::update::PermissionUpdate;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Filesystem roots used to resolve a [`PermissionUpdateDestination`] to a
/// concrete settings file (the persistence analogue of [`crate::FsRoots`]).
#[derive(Debug, Clone)]
pub struct PermissionPaths {
    /// Claude config home (`~/.claude`) — holds `userSettings`.
    pub lingxi_home: PathBuf,
    /// Project working directory — holds `.lingxi/settings.json` (project) and
    /// `.lingxi/settings.local.json` (local).
    pub cwd: PathBuf,
}

impl PermissionPaths {
    /// Resolve a destination to its settings file, or `None` when the
    /// destination is not persistable (`Session` / `CliArg`) — claude-code
    /// `supportsPersistence` + `getSettingsFilePathForSource`.
    #[must_use]
    pub fn destination_path(&self, dest: PermissionUpdateDestination) -> Option<PathBuf> {
        match dest {
            PermissionUpdateDestination::UserSettings => Some(self.lingxi_home.join("settings.json")),
            PermissionUpdateDestination::ProjectSettings => {
                Some(self.cwd.join(branding::DOT_DIR).join("settings.json"))
            }
            PermissionUpdateDestination::LocalSettings => {
                Some(self.cwd.join(branding::DOT_DIR).join("settings.local.json"))
            }
            PermissionUpdateDestination::Session | PermissionUpdateDestination::CliArg => None,
        }
    }
}

/// Error persisting a permission update.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// The destination file cannot be merged into safely — either not valid
    /// JSON, or valid JSON whose `permissions`/`permissions.{behavior}` is the
    /// wrong type (a string/object where an object/array is expected). In every
    /// case the file is left UNTOUCHED (claude-code's `updateSettingsForSource`
    /// "Invalid JSON syntax" + the `.map()`-throws-on-non-array contract).
    #[error("settings file at {0} cannot be merged (invalid or wrong-typed JSON); not overwriting")]
    BrokenJson(PathBuf),
    /// A filesystem error reading/creating/writing the settings file.
    #[error("io error on {path}: {source}")]
    Io {
        /// The file being read or written.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}

/// `permissions.{allow|deny|ask}` key for a behavior.
fn behavior_key(behavior: PermissionBehavior) -> &'static str {
    match behavior {
        PermissionBehavior::Allow => "allow",
        PermissionBehavior::Deny => "deny",
        PermissionBehavior::Ask => "ask",
    }
}

/// Merge `rule`'s string form into `permissions.{behavior}` of `raw` settings
/// JSON (the pure core of `addPermissionRulesToSettings`). Returns:
/// - `Ok(Some(new_json))` — the rule was new; `new_json` is the pretty-printed
///   updated settings (with a trailing newline) preserving all other keys.
/// - `Ok(None)` — the rule (normalized) is already present; no write needed.
/// - `Err(())` — `raw` is non-empty and not a JSON object (caller maps to
///   [`PersistError::BrokenJson`] and must NOT overwrite).
///
/// Existing entries are preserved verbatim; only de-duplication normalizes via
/// [`PermissionRuleValue::from_rule_string`]→[`PermissionRuleValue::to_rule_string`].
fn apply_rule_to_settings_json(raw: &str, rule: &PermissionRule) -> Result<Option<String>, ()> {
    let mut root: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).map_err(|_| ())?
    };
    let obj = root.as_object_mut().ok_or(())?;

    let perms = obj.entry("permissions").or_insert_with(|| json!({}));
    let perms_obj = perms.as_object_mut().ok_or(())?;

    let key = behavior_key(rule.behavior);
    let arr = perms_obj.entry(key).or_insert_with(|| json!([]));
    let arr_vec = arr.as_array_mut().ok_or(())?;

    // The new rule's canonical string. De-dup against existing entries
    // normalized the same way (so a legacy alias on disk still matches).
    // NOTE: a tool-wide rule (`rule_content: None`) serializes its `tool_name`
    // verbatim, so this assumes real tool names are paren-free — a name like
    // `Read(x)` would persist `"Read(x)"` and re-parse on next load as
    // Read+content=x (a different, narrower rule). Built-in tool names never
    // contain parens, and this matches claude-code's `permissionRuleValueToString`.
    let new_str = rule.value.to_rule_string();
    let already_present = arr_vec.iter().filter_map(Value::as_str).any(|existing| {
        PermissionRuleValue::from_rule_string(existing).to_rule_string() == new_str
    });
    if already_present {
        return Ok(None);
    }

    arr_vec.push(json!(new_str));
    let serialized = serde_json::to_string_pretty(&root).map_err(|_| ())?;
    Ok(Some(serialized + "\n"))
}

/// Persist `update` to its destination settings file (3c). Best-effort and
/// idempotent: returns `Ok(true)` when a rule was written, `Ok(false)` when the
/// destination is not persistable or the rule is already present.
///
/// # Errors
/// [`PersistError::BrokenJson`] if the destination file is not valid JSON (it is
/// left untouched); [`PersistError::Io`] on a read/create/write failure.
pub async fn persist_permission_update(
    update: &PermissionUpdate,
    paths: &PermissionPaths,
) -> Result<bool, PersistError> {
    let Some(path) = paths.destination_path(update.destination) else {
        return Ok(false); // Session / CliArg — not persistable.
    };

    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(PersistError::Io {
                path: path.clone(),
                source: e,
            })
        }
    };

    let Some(new_json) = apply_rule_to_settings_json(&raw, &update.rule)
        .map_err(|()| PersistError::BrokenJson(path.clone()))?
    else {
        return Ok(false); // already present — nothing to write.
    };

    if let Some(parent) = path.parent() {
        ensure_dir(parent, &path).await?;
    }
    tokio::fs::write(&path, new_json)
        .await
        .map_err(|e| PersistError::Io {
            path: path.clone(),
            source: e,
        })?;
    Ok(true)
}

/// Remove `rule`'s string form from `permissions.{behavior}` of `raw` settings
/// JSON (the inverse of [`apply_rule_to_settings_json`]). Returns:
/// - `Ok(Some(new_json))` — at least one matching entry was removed; `new_json`
///   is the pretty-printed updated settings (trailing newline).
/// - `Ok(None)` — no matching entry (nothing to write).
/// - `Err(())` — `raw` is non-empty and not a JSON object (caller maps to
///   [`PersistError::BrokenJson`] and must NOT overwrite).
///
/// Matching is canonical (both sides normalized via `PermissionRuleValue`), so a
/// legacy on-disk alias of the same rule is removed too.
fn remove_rule_from_settings_json(raw: &str, rule: &PermissionRule) -> Result<Option<String>, ()> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let mut root: Value = serde_json::from_str(raw).map_err(|_| ())?;
    let obj = root.as_object_mut().ok_or(())?;
    let Some(perms) = obj.get_mut("permissions") else {
        return Ok(None);
    };
    let perms_obj = perms.as_object_mut().ok_or(())?;
    let key = behavior_key(rule.behavior);
    let Some(arr) = perms_obj.get_mut(key) else {
        return Ok(None);
    };
    let arr_vec = arr.as_array_mut().ok_or(())?;

    let target = rule.value.to_rule_string();
    let before = arr_vec.len();
    arr_vec.retain(|existing| {
        existing.as_str().is_none_or(|s| {
            PermissionRuleValue::from_rule_string(s).to_rule_string() != target
        })
    });
    if arr_vec.len() == before {
        return Ok(None); // nothing matched.
    }
    let serialized = serde_json::to_string_pretty(&root).map_err(|_| ())?;
    Ok(Some(serialized + "\n"))
}

/// Remove `update.rule` from its destination settings file — the inverse of
/// [`persist_permission_update`], for the interactive `/permissions` delete
/// (PERM-1). Best-effort + idempotent: `Ok(true)` when an entry was removed,
/// `Ok(false)` when the destination is not persistable or no entry matched.
///
/// # Errors
/// [`PersistError::BrokenJson`] if the destination is non-empty and not valid
/// JSON (left untouched); [`PersistError::Io`] on a read/write failure.
pub async fn remove_permission_update(
    update: &PermissionUpdate,
    paths: &PermissionPaths,
) -> Result<bool, PersistError> {
    let Some(path) = paths.destination_path(update.destination) else {
        return Ok(false);
    };
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(PersistError::Io {
                path: path.clone(),
                source: e,
            })
        }
    };
    let Some(new_json) = remove_rule_from_settings_json(&raw, &update.rule)
        .map_err(|()| PersistError::BrokenJson(path.clone()))?
    else {
        return Ok(false);
    };
    tokio::fs::write(&path, new_json)
        .await
        .map_err(|e| PersistError::Io {
            path: path.clone(),
            source: e,
        })?;
    Ok(true)
}

/// (PERM-1 Workspace tab) Add or remove `dir` in
/// `permissions.additionalDirectories` of `raw` settings JSON. Returns:
/// - `Ok(Some(new_json))` — the array changed; pretty-printed + trailing
///   newline, preserving every other key.
/// - `Ok(None)` — no change needed (add: already present; remove: absent).
/// - `Err(())` — `raw` is non-empty and not a JSON object, or
///   `permissions`/`additionalDirectories` is the wrong type (caller maps to
///   [`PersistError::BrokenJson`] and must NOT overwrite).
///
/// Directory entries are compared verbatim (claude-code stores the path
/// string as-typed; no canonicalization).
fn apply_directory_to_settings_json(raw: &str, dir: &str, add: bool) -> Result<Option<String>, ()> {
    if !add && raw.trim().is_empty() {
        return Ok(None);
    }
    let mut root: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).map_err(|_| ())?
    };
    let obj = root.as_object_mut().ok_or(())?;

    if add {
        let perms = obj.entry("permissions").or_insert_with(|| json!({}));
        let perms_obj = perms.as_object_mut().ok_or(())?;
        let arr = perms_obj
            .entry("additionalDirectories")
            .or_insert_with(|| json!([]));
        let arr_vec = arr.as_array_mut().ok_or(())?;
        if arr_vec.iter().filter_map(Value::as_str).any(|d| d == dir) {
            return Ok(None); // already present.
        }
        arr_vec.push(json!(dir));
    } else {
        let Some(perms) = obj.get_mut("permissions") else {
            return Ok(None);
        };
        let perms_obj = perms.as_object_mut().ok_or(())?;
        let Some(arr) = perms_obj.get_mut("additionalDirectories") else {
            return Ok(None);
        };
        let arr_vec = arr.as_array_mut().ok_or(())?;
        let before = arr_vec.len();
        arr_vec.retain(|d| d.as_str() != Some(dir));
        if arr_vec.len() == before {
            return Ok(None); // nothing matched.
        }
    }
    let serialized = serde_json::to_string_pretty(&root).map_err(|_| ())?;
    Ok(Some(serialized + "\n"))
}

/// (PERM-1 Workspace tab) Add (`add = true`) or remove (`add = false`) a
/// workspace directory in the destination settings file's
/// `permissions.additionalDirectories` array. Best-effort + idempotent:
/// `Ok(true)` when the file changed, `Ok(false)` when the destination is not
/// persistable or no change was needed.
///
/// # Errors
/// [`PersistError::BrokenJson`] if the destination is non-empty and not valid
/// JSON (left untouched); [`PersistError::Io`] on a read/create/write failure.
pub async fn persist_workspace_directory(
    dir: &str,
    add: bool,
    dest: PermissionUpdateDestination,
    paths: &PermissionPaths,
) -> Result<bool, PersistError> {
    let Some(path) = paths.destination_path(dest) else {
        return Ok(false);
    };
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !add {
                return Ok(false); // nothing to remove from a missing file.
            }
            String::new()
        }
        Err(e) => {
            return Err(PersistError::Io {
                path: path.clone(),
                source: e,
            })
        }
    };
    let Some(new_json) = apply_directory_to_settings_json(&raw, dir, add)
        .map_err(|()| PersistError::BrokenJson(path.clone()))?
    else {
        return Ok(false);
    };
    if let Some(parent) = path.parent() {
        ensure_dir(parent, &path).await?;
    }
    tokio::fs::write(&path, new_json)
        .await
        .map_err(|e| PersistError::Io {
            path: path.clone(),
            source: e,
        })?;
    Ok(true)
}

async fn ensure_dir(parent: &Path, path: &Path) -> Result<(), PersistError> {
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| PersistError::Io {
            path: path.to_path_buf(),
            source: e,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::PermissionRuleSource;

    fn allow_rule(spec: &str, dest: PermissionUpdateDestination) -> PermissionUpdate {
        PermissionUpdate {
            rule: PermissionRule {
                value: PermissionRuleValue::from_rule_string(spec),
                behavior: PermissionBehavior::Allow,
                source: PermissionRuleSource::LocalSettings,
            },
            destination: dest,
        }
    }

    // ── pure merge core ──────────────────────────────────────────────────

    #[test]
    fn adds_rule_to_empty_settings() {
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        let out = apply_rule_to_settings_json("", &rule).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Bash"]));
        assert!(out.ends_with('\n'), "trailing newline");
    }

    #[test]
    fn appends_to_existing_array_and_preserves_other_keys() {
        let raw = r#"{ "model": "claude-opus-4-7", "permissions": { "allow": ["Read"], "deny": ["Bash(rm:*)"] } }"#;
        let rule = allow_rule("Edit(src/**)", PermissionUpdateDestination::LocalSettings).rule;
        let out = apply_rule_to_settings_json(raw, &rule).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        // appended, existing allow + deny + unrelated key preserved.
        assert_eq!(v["permissions"]["allow"], json!(["Read", "Edit(src/**)"]));
        assert_eq!(v["permissions"]["deny"], json!(["Bash(rm:*)"]));
        assert_eq!(v["model"], json!("claude-opus-4-7"));
    }

    #[test]
    fn dedups_exact_and_legacy_normalized() {
        // exact duplicate → no change.
        let raw = r#"{ "permissions": { "allow": ["Bash"] } }"#;
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        assert!(apply_rule_to_settings_json(raw, &rule).unwrap().is_none());

        // legacy alias on disk ("Task") normalizes to "Agent"; adding "Agent"
        // (or "Task", which parses to Agent) is a no-op.
        let raw2 = r#"{ "permissions": { "allow": ["Task"] } }"#;
        let agent = allow_rule("Agent", PermissionUpdateDestination::LocalSettings).rule;
        assert!(apply_rule_to_settings_json(raw2, &agent).unwrap().is_none());
    }

    #[test]
    fn creates_permissions_object_when_absent() {
        let raw = r#"{ "model": "x" }"#;
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        let out = apply_rule_to_settings_json(raw, &rule).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Bash"]));
        assert_eq!(v["model"], json!("x"));
    }

    // ── remove (PERM-1) ──────────────────────────────────────────────────

    #[test]
    fn removes_rule_preserving_others() {
        let raw = r#"{ "model": "x", "permissions": { "allow": ["Read", "Edit(src/**)"], "deny": ["Bash(rm:*)"] } }"#;
        let rule = allow_rule("Read", PermissionUpdateDestination::LocalSettings).rule;
        let out = remove_rule_from_settings_json(raw, &rule).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Edit(src/**)"]));
        assert_eq!(v["permissions"]["deny"], json!(["Bash(rm:*)"]));
        assert_eq!(v["model"], json!("x"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn remove_is_noop_when_absent_or_empty() {
        // Rule not present → None.
        let raw = r#"{ "permissions": { "allow": ["Read"] } }"#;
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        assert!(remove_rule_from_settings_json(raw, &rule).unwrap().is_none());
        // Empty / missing permissions → None.
        assert!(remove_rule_from_settings_json("", &rule).unwrap().is_none());
        assert!(remove_rule_from_settings_json(r#"{ "model": "x" }"#, &rule)
            .unwrap()
            .is_none());
    }

    #[test]
    fn remove_matches_legacy_alias() {
        // On-disk "Task" normalizes to "Agent"; removing "Agent" removes it.
        let raw = r#"{ "permissions": { "allow": ["Task", "Read"] } }"#;
        let agent = allow_rule("Agent", PermissionUpdateDestination::LocalSettings).rule;
        let out = remove_rule_from_settings_json(raw, &agent).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Read"]));
    }

    #[test]
    fn remove_broken_json_errors_without_write() {
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        assert!(remove_rule_from_settings_json("{not json", &rule).is_err());
    }

    #[test]
    fn broken_json_errors() {
        let rule = allow_rule("Bash", PermissionUpdateDestination::LocalSettings).rule;
        assert!(apply_rule_to_settings_json("{not json", &rule).is_err());
        // a top-level array is not an object → error (don't clobber).
        assert!(apply_rule_to_settings_json("[]", &rule).is_err());
    }

    #[test]
    fn deny_rule_goes_to_deny_array() {
        let rule = PermissionRule {
            value: PermissionRuleValue::from_rule_string("Read(./secrets/**)"),
            behavior: PermissionBehavior::Deny,
            source: PermissionRuleSource::LocalSettings,
        };
        let out = apply_rule_to_settings_json("{}", &rule).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["deny"], json!(["Read(./secrets/**)"]));
    }

    // ── path resolution ──────────────────────────────────────────────────

    #[test]
    fn destination_paths() {
        let p = PermissionPaths {
            lingxi_home: PathBuf::from("/home/u/.lingxi"),
            cwd: PathBuf::from("/proj"),
        };
        assert_eq!(
            p.destination_path(PermissionUpdateDestination::UserSettings),
            Some(PathBuf::from("/home/u/.lingxi/settings.json"))
        );
        assert_eq!(
            p.destination_path(PermissionUpdateDestination::ProjectSettings),
            Some(PathBuf::from("/proj/.lingxi/settings.json"))
        );
        assert_eq!(
            p.destination_path(PermissionUpdateDestination::LocalSettings),
            Some(PathBuf::from("/proj/.lingxi/settings.local.json"))
        );
        assert!(p
            .destination_path(PermissionUpdateDestination::Session)
            .is_none());
        assert!(p
            .destination_path(PermissionUpdateDestination::CliArg)
            .is_none());
    }

    // ── async I/O wrapper ────────────────────────────────────────────────

    #[tokio::test]
    async fn persist_creates_local_settings_and_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("lx-3c-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = PermissionPaths {
            lingxi_home: tmp.join("home/.lingxi"),
            cwd: tmp.join("proj"),
        };
        let update = allow_rule("Bash", PermissionUpdateDestination::LocalSettings);

        // First persist writes the file + creates .lingxi/.
        assert!(persist_permission_update(&update, &paths).await.unwrap());
        let path = tmp.join("proj/.lingxi/settings.local.json");
        let written = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["permissions"]["allow"], json!(["Bash"]));

        // Second persist of the same rule is a no-op (idempotent).
        assert!(!persist_permission_update(&update, &paths).await.unwrap());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn persist_session_destination_is_noop() {
        let paths = PermissionPaths {
            lingxi_home: PathBuf::from("/nonexistent/.lingxi"),
            cwd: PathBuf::from("/nonexistent/proj"),
        };
        let update = allow_rule("Bash", PermissionUpdateDestination::Session);
        // No file resolved → Ok(false), nothing written, no error.
        assert!(!persist_permission_update(&update, &paths).await.unwrap());
    }

    #[tokio::test]
    async fn persist_broken_json_errors_without_clobber() {
        let tmp = std::env::temp_dir().join(format!("lx-3c-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("proj/.lingxi");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.local.json");
        std::fs::write(&path, "{ broken").unwrap();

        let paths = PermissionPaths {
            lingxi_home: tmp.join("home/.lingxi"),
            cwd: tmp.join("proj"),
        };
        let update = allow_rule("Bash", PermissionUpdateDestination::LocalSettings);
        let err = persist_permission_update(&update, &paths).await.unwrap_err();
        assert!(matches!(err, PersistError::BrokenJson(_)));
        // File left untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── (PERM-1 Workspace tab) additionalDirectories add/remove ──────────

    #[test]
    fn adds_directory_to_empty_settings() {
        let out = apply_directory_to_settings_json("", "/extra/dir", true)
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["permissions"]["additionalDirectories"],
            json!(["/extra/dir"])
        );
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn adds_directory_preserving_other_keys_and_dedups() {
        let raw = r#"{ "model": "x", "permissions": { "allow": ["Read"], "additionalDirectories": ["/a"] } }"#;
        let out = apply_directory_to_settings_json(raw, "/b", true)
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["permissions"]["additionalDirectories"],
            json!(["/a", "/b"])
        );
        assert_eq!(v["permissions"]["allow"], json!(["Read"]));
        assert_eq!(v["model"], json!("x"));
        // Adding an already-present dir is a no-op.
        assert!(apply_directory_to_settings_json(raw, "/a", true)
            .unwrap()
            .is_none());
    }

    #[test]
    fn removes_directory_and_noop_when_absent() {
        let raw = r#"{ "permissions": { "additionalDirectories": ["/a", "/b"] } }"#;
        let out = apply_directory_to_settings_json(raw, "/a", false)
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["additionalDirectories"], json!(["/b"]));
        // Removing an absent dir is a no-op.
        assert!(apply_directory_to_settings_json(raw, "/nope", false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn directory_broken_json_errors_without_overwrite() {
        assert!(apply_directory_to_settings_json("{ broken", "/x", true).is_err());
    }

    #[tokio::test]
    async fn persist_workspace_directory_roundtrips_to_local_settings() {
        let tmp = std::env::temp_dir().join(format!("lx-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("proj")).unwrap();
        let paths = PermissionPaths {
            lingxi_home: tmp.join("home/.lingxi"),
            cwd: tmp.join("proj"),
        };
        // Add → file created with the dir.
        let added = persist_workspace_directory(
            "/work/extra",
            true,
            PermissionUpdateDestination::LocalSettings,
            &paths,
        )
        .await
        .unwrap();
        assert!(added);
        let path = tmp.join("proj/.lingxi/settings.local.json");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("/work/extra"));
        // Remove → gone.
        let removed = persist_workspace_directory(
            "/work/extra",
            false,
            PermissionUpdateDestination::LocalSettings,
            &paths,
        )
        .await
        .unwrap();
        assert!(removed);
        let body2 = std::fs::read_to_string(&path).unwrap();
        assert!(!body2.contains("/work/extra"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
