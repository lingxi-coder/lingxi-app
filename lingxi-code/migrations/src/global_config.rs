//! `~/.claude.json` `GlobalConfig` substrate — the first in the Rust port
//! (`tools/meta/src/config.rs:17` records "no substrate" prior to this).
//!
//! Ports the path/read/save mechanics of `utils/config.ts` +
//! `utils/env.ts getGlobalClaudeFile` + `utils/envUtils.ts
//! getClaudeConfigHomeDir`, operating on a raw [`serde_json::Map`] so unknown
//! keys (the real file carries dozens: `numStartups`, `oauthAccount`, …) are
//! NEVER dropped. `serde_json`'s workspace `preserve_order` feature keeps key
//! order stable across round-trips.
//!
//! Documented simplifications vs TS (`config.ts:797-864`):
//! - No `proper-lockfile` cross-process lock and no in-memory mtime cache —
//!   migrations run once at startup before any concurrent writer exists in
//!   this process. The GH #3117 auth-loss fallback guard is therefore N/A:
//!   we never write defaults over a failed read (a broken file aborts the
//!   write instead).
//! - TS NFC-normalizes the config-home path; macOS paths are already NFC, so
//!   this port uses the path as-is.

use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

/// A raw JSON object — the in-memory shape of `~/.claude.json`.
pub type JsonMap = Map<String, Value>;

/// `$CLAUDE_CONFIG_DIR`, treating set-but-EMPTY as unset: the TS reads are
/// all `process.env.CLAUDE_CONFIG_DIR || homedir()`-shaped, and `""` is falsy.
fn claude_config_dir_env() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `getClaudeConfigHomeDir` (`envUtils.ts:7-14`): `$CLAUDE_CONFIG_DIR` if
/// set (non-empty), else `$HOME/.claude`. `None` when neither env var exists.
#[must_use]
pub fn claude_config_home() -> Option<PathBuf> {
    if let Some(dir) = claude_config_dir_env() {
        return Some(dir);
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
}

/// `getGlobalClaudeFile` (`env.ts:14-26`): legacy `<config-home>/.config.json`
/// when it exists, else `($CLAUDE_CONFIG_DIR || $HOME)/.claude.json`.
///
/// The TS oauth filename suffix (`fileSuffixForOauthConfig()` →
/// `-custom-oauth`/`-local-oauth`/`-staging-oauth`) only applies under custom
/// OAuth env vars this port does not model (`anthropic-oauth` has no
/// `getOauthConfigType` substrate) — the default build resolves it to `""`,
/// so `.claude.json` is hardcoded here.
#[must_use]
pub fn global_config_path() -> Option<PathBuf> {
    let home = claude_config_home()?;
    let legacy = home.join(".config.json");
    if legacy.exists() {
        return Some(legacy);
    }
    let base =
        claude_config_dir_env().or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    Some(base.join(".claude.json"))
}

/// Errors from the `GlobalConfig` substrate. All callers treat any error as
/// "skip this write / skip this run" — never destructive.
#[derive(Debug)]
pub enum GlobalConfigError {
    /// The file exists but is not valid JSON (or not a JSON object). The
    /// migration runner skips the startup entirely rather than overwrite
    /// (stricter than TS, which falls back to defaults under a guard —
    /// documented divergence).
    Broken(String),
    /// I/O failure reading or writing.
    Io(std::io::Error),
}

impl std::fmt::Display for GlobalConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Broken(e) => write!(f, "invalid global config JSON: {e}"),
            Self::Io(e) => write!(f, "global config I/O error: {e}"),
        }
    }
}

impl std::error::Error for GlobalConfigError {}

/// Read `~/.claude.json` into a raw map. Missing file ⇒ empty map (TS
/// `getConfig` falls back to defaults; the typed getters below default per
/// key). Broken JSON ⇒ [`GlobalConfigError::Broken`].
pub fn read_map(path: &Path) -> Result<JsonMap, GlobalConfigError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(JsonMap::new()),
        Err(e) => return Err(GlobalConfigError::Io(e)),
    };
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| GlobalConfigError::Broken(e.to_string()))?;
    match value {
        Value::Object(map) => Ok(map),
        other => Err(GlobalConfigError::Broken(format!(
            "expected a JSON object, got {other}"
        ))),
    }
}

/// `saveGlobalConfig(prev => next)` (`config.ts:797-864`): read-modify-write.
/// The mutator's output is compared by VALUE — unchanged ⇒ zero write (the TS
/// same-reference skip). On write: strip legacy per-project `history` keys
/// (`removeProjectHistory`, `config.ts:966-989`) and write atomically
/// (same-dir tmp file + rename), pretty-printed + trailing newline.
///
/// Returns `Ok(true)` if the file was written.
pub fn save_map(
    path: &Path,
    mutator: impl FnOnce(JsonMap) -> JsonMap,
) -> Result<bool, GlobalConfigError> {
    let current = read_map(path)?;
    let mut next = mutator(current.clone());
    if next == current {
        return Ok(false);
    }
    remove_project_history(&mut next);
    write_atomic(path, &next)?;
    Ok(true)
}

/// `removeProjectHistory` (`config.ts:966-989`): drop the legacy `history`
/// key from every entry under `projects`. The `needsCleaning` gate in TS is
/// subsumed by `save_map`'s value-equality skip (we only reach here when a
/// write is happening anyway).
fn remove_project_history(map: &mut JsonMap) {
    if let Some(Value::Object(projects)) = map.get_mut("projects") {
        for (_path, proj) in projects.iter_mut() {
            if let Value::Object(p) = proj {
                p.remove("history");
            }
        }
    }
}

fn write_atomic(path: &Path, map: &JsonMap) -> Result<(), GlobalConfigError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(GlobalConfigError::Io)?;
    let serialized = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| GlobalConfigError::Broken(e.to_string()))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    write_secure(&tmp, &(serialized + "\n")).map_err(GlobalConfigError::Io)?;
    std::fs::rename(&tmp, path).map_err(GlobalConfigError::Io)?;
    Ok(())
}

/// Write `contents`, creating the file with mode `0o600` on unix — the TS
/// `writeFileSyncAndFlush_DEPRECATED(file, …, { mode: 0o600 })`
/// (`config.ts:1134-1141`). Like the TS `mode` option, the mode only applies
/// to NEWLY created files; an existing file keeps its permissions. No-op
/// mode-wise on non-unix.
pub(crate) fn write_secure(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())
}

/// `getProjectPathForConfig` (`config.ts:1588-1601`): the CANONICAL git root
/// of the directory — walk up looking for a `.git` entry (dir OR file), then
/// resolve a linked worktree's `.git` file through `gitdir:` → `commondir`
/// to the MAIN repo root (`findCanonicalGitRoot`, `git.ts:123-210`) so all
/// worktrees of a repo share one project key — else the canonicalized
/// directory itself; forward slashes for stable JSON keys.
#[must_use]
pub fn project_path_for_config(dir: &Path) -> String {
    let resolved = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut cur: Option<&Path> = Some(&resolved);
    while let Some(p) = cur {
        if p.join(".git").exists() {
            return resolve_canonical_root(p).to_string_lossy().replace('\\', "/");
        }
        cur = p.parent();
    }
    resolved.to_string_lossy().replace('\\', "/")
}

/// `resolveCanonicalRoot` (`git.ts:123-185`): for a regular repo (`.git` is a
/// directory) this is a no-op. For a linked worktree (`.git` is a file with a
/// `gitdir: <path>` pointer) follow `gitdir:` → `commondir` to the main
/// repo's working directory. Submodules (`.git` file, no `commondir`) and any
/// read/validation failure fall through to the input root, exactly as the TS
/// `catch` does. (The TS NFC-normalize is skipped per the module-level note.)
fn resolve_canonical_root(git_root: &Path) -> PathBuf {
    resolve_worktree_main_root(git_root).unwrap_or_else(|| git_root.to_path_buf())
}

/// The fallible body of [`resolve_canonical_root`]; `None` ⇒ keep `git_root`.
fn resolve_worktree_main_root(git_root: &Path) -> Option<PathBuf> {
    // In a worktree, `.git` is a file containing `gitdir: <path>`. In a
    // regular repo it is a directory: read fails (TS EISDIR) ⇒ fall through.
    let git_content = std::fs::read_to_string(git_root.join(".git")).ok()?;
    let pointer = git_content.trim().strip_prefix("gitdir:")?;
    // TS `resolve(gitRoot, pointer)` — relative pointers resolve against the
    // worktree root, lexically (no symlink traversal).
    let worktree_git_dir = lexical_resolve(git_root, Path::new(pointer.trim()));
    // `commondir` points at the shared .git dir, RELATIVE TO the worktree
    // gitdir. Submodules have no commondir (TS ENOENT) ⇒ fall through.
    let commondir_raw = std::fs::read_to_string(worktree_git_dir.join("commondir")).ok()?;
    let common_dir = lexical_resolve(&worktree_git_dir, Path::new(commondir_raw.trim()));
    // SECURITY (per TS): validate the structure matches `git worktree add`.
    // 1) worktreeGitDir must be a direct child of <commonDir>/worktrees/.
    if worktree_git_dir.parent()? != common_dir.join("worktrees") {
        return None;
    }
    // 2) <worktreeGitDir>/gitdir must point back to <gitRoot>/.git. TS
    //    realpaths the backlink, and realpaths the root DIRECTORY then joins
    //    `.git` (never realpathing the `.git` file itself).
    let backlink_raw = std::fs::read_to_string(worktree_git_dir.join("gitdir")).ok()?;
    let backlink = std::fs::canonicalize(backlink_raw.trim()).ok()?;
    if backlink != std::fs::canonicalize(git_root).ok()?.join(".git") {
        return None;
    }
    // Bare-repo worktrees: the common dir isn't a `.git` inside a working
    // directory — use the common dir itself as the stable identity.
    if common_dir.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return Some(common_dir);
    }
    Some(common_dir.parent()?.to_path_buf())
}

/// Node `path.resolve(base, p)`: absolute `p` wins, else join onto `base`;
/// then normalize `.`/`..` LEXICALLY (no filesystem access), dropping any
/// `..` that would climb above the root.
fn lexical_resolve(base: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `getCurrentProjectConfig` (`config.ts:1602-1623`): the `projects[<key>]`
/// sub-object, empty map when absent. (The TS `allowedTools`
/// string-coercion quirk is not ported — no Rust reader consumes it.)
pub fn get_project_config(path: &Path, project_key: &str) -> Result<JsonMap, GlobalConfigError> {
    let map = read_map(path)?;
    Ok(map
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|p| p.get(project_key))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

/// `saveCurrentProjectConfig` (`config.ts:1625-1700`): mutate the
/// `projects[<key>]` sub-object in place (no history-strip on this path —
/// mirrors TS, whose project-save writes `projects` directly).
pub fn save_project_config(
    path: &Path,
    project_key: &str,
    mutator: impl FnOnce(JsonMap) -> JsonMap,
) -> Result<bool, GlobalConfigError> {
    let current = read_map(path)?;
    let current_proj = current
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|p| p.get(project_key))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let next_proj = mutator(current_proj.clone());
    if next_proj == current_proj {
        return Ok(false);
    }
    let mut next = current;
    let projects = next
        .entry("projects")
        .or_insert_with(|| Value::Object(JsonMap::new()));
    if let Value::Object(projects) = projects {
        projects.insert(project_key.to_string(), Value::Object(next_proj));
    }
    write_atomic(path, &next)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    #[test]
    fn config_home_prefers_claude_config_dir() {
        let _g = env_lock();
        std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/cc-test-home");
        assert_eq!(
            claude_config_home(),
            Some(std::path::PathBuf::from("/tmp/cc-test-home"))
        );
        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn config_home_falls_back_to_home_dot_claude() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HOME", "/tmp/cc-test-h2");
        assert_eq!(
            claude_config_home(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h2/.claude"))
        );
    }

    #[test]
    fn config_home_treats_empty_claude_config_dir_as_unset() {
        let _g = env_lock();
        // TS: `process.env.CLAUDE_CONFIG_DIR || homedir()` — "" is falsy.
        std::env::set_var("CLAUDE_CONFIG_DIR", "");
        std::env::set_var("HOME", "/tmp/cc-test-h4");
        assert_eq!(
            claude_config_home(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h4/.claude"))
        );
        assert_eq!(
            global_config_path(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h4/.claude.json"))
        );
        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn global_path_prefers_legacy_config_json_when_present() {
        let _g = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());
        std::fs::write(tmp.path().join(".config.json"), "{}").unwrap();
        assert_eq!(global_config_path(), Some(tmp.path().join(".config.json")));
        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn global_path_is_claude_json_under_config_dir_else_home() {
        let _g = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", tmp.path());
        assert_eq!(global_config_path(), Some(tmp.path().join(".claude.json")));
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::set_var("HOME", "/tmp/cc-test-h3");
        assert_eq!(
            global_config_path(),
            Some(std::path::PathBuf::from("/tmp/cc-test-h3/.claude.json"))
        );
    }

    use crate::test_support::temp_config;

    #[test]
    fn read_map_missing_file_is_empty() {
        let t = temp_config();
        assert!(read_map(&t.global).unwrap().is_empty());
    }

    #[test]
    fn read_map_broken_json_is_error() {
        let t = temp_config();
        std::fs::write(&t.global, "{ not json").unwrap();
        assert!(read_map(&t.global).is_err());
    }

    #[test]
    fn save_map_roundtrips_and_preserves_unknown_keys() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"zeta":1,"oauthAccount":{"id":"x"},"numStartups":42,"alpha":true}"#,
        )
        .unwrap();
        let wrote = save_map(&t.global, |mut m| {
            m.insert("migrationVersion".into(), serde_json::json!(11));
            m
        })
        .unwrap();
        assert!(wrote);
        let back = read_map(&t.global).unwrap();
        assert_eq!(back["zeta"], serde_json::json!(1));
        assert_eq!(back["oauthAccount"]["id"], serde_json::json!("x"));
        assert_eq!(back["numStartups"], serde_json::json!(42));
        assert_eq!(back["migrationVersion"], serde_json::json!(11));
        // preserve_order: original keys keep their relative order.
        let keys: Vec<&String> = back.keys().collect();
        assert!(keys.iter().position(|k| *k == "zeta").unwrap()
            < keys.iter().position(|k| *k == "alpha").unwrap());
    }

    #[test]
    fn save_map_no_change_writes_nothing() {
        let t = temp_config();
        std::fs::write(&t.global, "{\"a\": 1}\n").unwrap();
        let before = std::fs::metadata(&t.global).unwrap().modified().unwrap();
        let wrote = save_map(&t.global, |m| m).unwrap();
        assert!(!wrote);
        let after = std::fs::metadata(&t.global).unwrap().modified().unwrap();
        assert_eq!(before, after, "file must be untouched");
    }

    #[test]
    fn save_map_broken_json_refuses_to_write() {
        let t = temp_config();
        std::fs::write(&t.global, "{ broken").unwrap();
        let res = save_map(&t.global, |mut m| {
            m.insert("x".into(), serde_json::json!(1));
            m
        });
        assert!(res.is_err());
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), "{ broken");
    }

    #[test]
    fn save_map_strips_legacy_project_history() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"projects":{"/p":{"history":["old"],"allowedTools":[]}}}"#,
        )
        .unwrap();
        save_map(&t.global, |mut m| {
            m.insert("migrationVersion".into(), serde_json::json!(11));
            m
        })
        .unwrap();
        let back = read_map(&t.global).unwrap();
        assert!(back["projects"]["/p"].get("history").is_none());
        assert!(back["projects"]["/p"].get("allowedTools").is_some());
    }

    #[test]
    fn project_config_get_and_save() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"projects":{"/proj":{"enabledMcpjsonServers":["a"]}}}"#,
        )
        .unwrap();
        let proj = get_project_config(&t.global, "/proj").unwrap();
        assert_eq!(proj["enabledMcpjsonServers"], serde_json::json!(["a"]));
        // unknown project → empty
        assert!(get_project_config(&t.global, "/other").unwrap().is_empty());

        save_project_config(&t.global, "/proj", |mut p| {
            p.remove("enabledMcpjsonServers");
            p
        })
        .unwrap();
        let proj = get_project_config(&t.global, "/proj").unwrap();
        assert!(proj.get("enabledMcpjsonServers").is_none());
    }

    #[test]
    fn project_key_git_root_else_cwd() {
        let t = temp_config();
        let repo = t.project.join("repo");
        let nested = repo.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let key = project_path_for_config(&nested);
        let canon = repo.canonicalize().unwrap();
        assert_eq!(key, canon.to_string_lossy().replace('\\', "/"));

        let bare = t.project.join("loose");
        std::fs::create_dir_all(&bare).unwrap();
        let key2 = project_path_for_config(&bare);
        assert_eq!(key2, bare.canonicalize().unwrap().to_string_lossy().replace('\\', "/"));
    }

    /// Simulated `git worktree add` layout: the worktree's key must be the
    /// MAIN repo root (TS `findCanonicalGitRoot`), not the worktree dir.
    #[test]
    fn project_key_resolves_worktree_to_main_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let main = root.join("main");
        let wt_git_dir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&wt_git_dir).unwrap();
        std::fs::write(wt_git_dir.join("commondir"), "../..\n").unwrap();
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt_git_dir.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}\n", wt_git_dir.display()),
        )
        .unwrap();

        let key = project_path_for_config(&wt);
        assert_eq!(key, main.to_string_lossy().replace('\\', "/"));

        // Nested dirs inside the worktree resolve to the same key.
        let nested = wt.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(project_path_for_config(&nested), key);
    }

    /// Submodule shape — `.git` file but NO `commondir` — keeps the
    /// directory's own root (TS falls through on ENOENT).
    #[test]
    fn project_key_submodule_git_file_keeps_own_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let sub = root.join("sub");
        std::fs::create_dir_all(root.join("parent/.git/modules/sub")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join(".git"),
            format!("gitdir: {}\n", root.join("parent/.git/modules/sub").display()),
        )
        .unwrap();
        let key = project_path_for_config(&sub);
        assert_eq!(key, sub.to_string_lossy().replace('\\', "/"));
    }

    #[cfg(unix)]
    #[test]
    fn save_map_creates_file_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let t = temp_config();
        save_map(&t.global, |mut m| {
            m.insert("a".into(), serde_json::json!(1));
            m
        })
        .unwrap();
        let mode = std::fs::metadata(&t.global).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
