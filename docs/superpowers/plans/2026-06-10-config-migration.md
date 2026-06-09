# Config-Migration Subsystem Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port claude-code's startup config-migration subsystem (`runMigrations()`, `CURRENT_MIGRATION_VERSION = 11`, 9 external-reachable sync migrations + async changelog migration) to the Rust port, including the `~/.claude.json` GlobalConfig substrate it requires.

**Architecture:** New desktop-only leaf crate `lingxi-code/migrations/` (mirrors TS `src/migrations/`, one file per migration) with a raw-`serde_json::Map` GlobalConfig reader/writer (preserve-unknown-keys, atomic writes, zero-write-on-no-change), an `updateSettingsForSource` port, and a fail-closed `MigrationContext` for the subscriber-gated migrations. Wired in `apps/cli` `run_cli` pre-REPL. Plus: 9 new tengu telemetry events (order-locked fixture append) and removal of `deny_unknown_fields` from the engine `SettingsJson` (zod-strip parity).

**Tech Stack:** Rust 1.82 / edition 2021, serde_json (workspace, `preserve_order` — key order survives round-trips), tokio, telemetry crate, anthropic-oauth (`SubscriptionType` only). No new external deps.

**Spec:** `docs/superpowers/specs/2026-06-10-config-migration-design.md` (approved). Reference of truth: `claude-code/` TS — `src/main.tsx:323-353`, `src/migrations/*.ts`, `src/utils/config.ts`, `src/utils/settings/settings.ts`, `src/utils/releaseNotes.ts:37-76`, `src/utils/envUtils.ts:7-37`, `src/utils/model/providers.ts:6-14`.

**Branch:** `parity-config-migrations` off `main`, in the main checkout (established pipeline; merged locally after gates).

**Conventions that bite (read first):**
- Cargo workspace root is `lingxi-code/`, NOT the repo root. Run cargo there; run git at the repo root.
- Workspace enforces `-D missing-docs` (rustc deny) — EVERY `pub` item needs a doc comment, including test-support items. Clippy `-D warnings` on `--all-targets` (tests included).
- Commit with `git commit -F <file>` (zsh traps backticks/angle brackets). Footer exactly:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- NEVER run `cargo test --workspace` (runtime) — fs_watch/fseventsd flake. Use per-crate tests + `cargo test --workspace --no-run` (struct-trap compile).
- Tests that mutate process env (`HOME`, `CLAUDE_CONFIG_DIR`, `DISABLE_AUTOUPDATER`, …) MUST hold a shared `Mutex` (pattern below) — cargo runs tests in parallel threads sharing the process env.

---

## File map

| File | Action | Responsibility |
|---|---|---|
| `lingxi-code/Cargo.toml` | Modify | add `"migrations"` to `[workspace.members]` |
| `lingxi-code/migrations/Cargo.toml` | Create | crate manifest |
| `lingxi-code/migrations/src/lib.rs` | Create | module decls + re-exports |
| `lingxi-code/migrations/src/global_config.rs` | Create | `~/.claude.json` substrate (paths, read, save, project config) |
| `lingxi-code/migrations/src/settings_update.rs` | Create | `updateSettingsForSource` port (user + local sources) |
| `lingxi-code/migrations/src/context.rs` | Create | `MigrationContext`, `MigrationEnv`, `is_env_truthy`, `js_truthy` |
| `lingxi-code/migrations/src/migrate_repl_bridge.rs` | Create | migration 9 (rename) |
| `lingxi-code/migrations/src/migrate_sonnet1m_to_sonnet45.rs` | Create | migration 5 |
| `lingxi-code/migrations/src/migrate_legacy_opus.rs` | Create | migration 6 |
| `lingxi-code/migrations/src/migrate_auto_updates.rs` | Create | migration 1 |
| `lingxi-code/migrations/src/migrate_bypass_permissions.rs` | Create | migration 2 |
| `lingxi-code/migrations/src/migrate_mcp_servers.rs` | Create | migration 3 |
| `lingxi-code/migrations/src/migrate_reset_pro_to_opus.rs` | Create | migration 4 |
| `lingxi-code/migrations/src/migrate_sonnet45_to_46.rs` | Create | migration 7 |
| `lingxi-code/migrations/src/migrate_opus_to_opus1m.rs` | Create | migration 8 |
| `lingxi-code/migrations/src/runner.rs` | Create | `run_migrations` + version guard |
| `lingxi-code/migrations/src/changelog.rs` | Create | async `migrate_changelog_from_config` |
| `lingxi-code/telemetry/src/tengu/migration.rs` | Create | 9 event names |
| `lingxi-code/telemetry/src/tengu/mod.rs` | Modify | register block, TOTAL 339→348 |
| `lingxi-code/test-harness/src/parity/fixtures/tengu_events.json` | Modify | append 9 names (order-locked tail) |
| count-assert sites (`tui/src/components/prompt_input/vim.rs`, `tui/tests/behavior_palette.rs`, `telemetry/tests/event_name_completeness_test.rs`, `orchestrator/src/diagnostics.rs`) | Modify | 339→348 |
| `lingxi-code/engine/src/settings/schema.rs` | Modify | drop `deny_unknown_fields` |
| `lingxi-code/engine/src/settings/loader.rs` | Modify | rejection test → tolerance test |
| `lingxi-code/apps/cli/Cargo.toml` + `src/lib.rs` | Modify | wire `run_migrations` pre-REPL |

Shared test helper (env lock + temp HOME), used by every task in the `migrations` crate — defined once in Task 1 as `src/test_support.rs` (`#[cfg(test)]`-gated module):

```rust
//! Test-only helpers: process-env serialization + temp config dirs.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Process-wide lock for tests that mutate env vars (`HOME`,
/// `CLAUDE_CONFIG_DIR`, `DISABLE_AUTOUPDATER`, provider gates). Cargo runs
/// tests in parallel threads sharing the process env; hold this for the
/// test's whole body.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A throwaway config universe: `dir` is a tempdir acting as the Claude
/// config home; `global` is the `~/.claude.json`-equivalent path inside it.
pub struct TempConfig {
    /// Owns the tempdir (deleted on drop).
    pub _tmp: tempfile::TempDir,
    /// Stand-in for `~/.claude` (claude config home).
    pub home: PathBuf,
    /// Stand-in for `~/.claude.json` (global config file).
    pub global: PathBuf,
    /// Stand-in project directory (for settings.local.json).
    pub project: PathBuf,
}

/// Build a fresh [`TempConfig`]. No env mutation — APIs take explicit paths.
pub fn temp_config() -> TempConfig {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("claude-home");
    let global = tmp.path().join("claude.json");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).expect("mk home");
    std::fs::create_dir_all(&project).expect("mk project");
    TempConfig { _tmp: tmp, home, global, project }
}
```

KEY DESIGN RULE (keeps 90% of tests env-free): every migration takes a `MigrationEnv` carrying EXPLICIT paths. Only the path-RESOLUTION helpers and `MigrationContext::from_env()` read process env — those few tests hold `env_lock()`.

---

### Task 1: Crate scaffold + config-path resolution

**Files:**
- Modify: `lingxi-code/Cargo.toml` (workspace members)
- Create: `lingxi-code/migrations/Cargo.toml`
- Create: `lingxi-code/migrations/src/lib.rs`
- Create: `lingxi-code/migrations/src/test_support.rs` (content above)
- Create: `lingxi-code/migrations/src/global_config.rs` (paths only in this task)

- [ ] **Step 1: Workspace member.** In `lingxi-code/Cargo.toml`, add `"migrations",` to `[workspace] members` (alphabetical-ish placement near `"memory",` is fine — the list is not sorted strictly; put it after `"memory",`).

- [ ] **Step 2: Crate manifest** — `lingxi-code/migrations/Cargo.toml`:

```toml
[package]
name = "migrations"
version = "0.12.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
# SubscriptionType for the subscriber-gated migrations' fail-closed context.
anthropic-oauth = { path = "../anthropic-oauth" }
telemetry = { path = "../telemetry" }
serde_json.workspace = true
tracing.workspace = true
tokio = { workspace = true, features = ["fs"] }

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 3: lib.rs skeleton** (modules added per task; start with):

```rust
//! Startup config migrations — port of claude-code `runMigrations()`
//! (`main.tsx:323-353`, `CURRENT_MIGRATION_VERSION = 11`) plus the
//! `~/.claude.json` GlobalConfig substrate it requires (`utils/config.ts`).
//!
//! Desktop-only: wired in `apps/cli` pre-REPL; NEVER part of the
//! engine-mobile dependency tree.
//!
//! Excluded from the port (with reasons): `migrateFennecToOpus` (dead code in
//! the external build — `if ("external" === 'ant')`), and
//! `resetAutoModeOptInForDefaultOffer` (gated on the ant-only
//! `TRANSCRIPT_CLASSIFIER` feature; the classifier is correctly stubbed in
//! this port).

pub mod context;
pub mod global_config;
pub mod settings_update;

#[cfg(test)]
mod test_support;
```

- [ ] **Step 4: Failing tests for path resolution** — bottom of `global_config.rs`:

```rust
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
}
```

- [ ] **Step 5: Run to verify failure.** `cd lingxi-code && cargo test -p migrations` → FAIL (functions undefined).

- [ ] **Step 6: Implement** — top of `global_config.rs`:

```rust
//! `~/.claude.json` GlobalConfig substrate — the first in the Rust port
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

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A raw JSON object — the in-memory shape of `~/.claude.json`.
pub type JsonMap = Map<String, Value>;

/// `getClaudeConfigHomeDir` (`envUtils.ts:7-14`): `$CLAUDE_CONFIG_DIR` if
/// set, else `$HOME/.claude`. `None` when neither env var exists.
pub fn claude_config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
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
pub fn global_config_path() -> Option<PathBuf> {
    let home = claude_config_home()?;
    let legacy = home.join(".config.json");
    if legacy.exists() {
        return Some(legacy);
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    Some(base.join(".claude.json"))
}
```

(Other items in the test referenced below — `read_map` etc. — arrive in Task 2; only paths here.)

- [ ] **Step 7: Run tests.** `cargo test -p migrations` → PASS (4 tests).

- [ ] **Step 8: Gate + commit.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo clippy -p migrations --all-targets --no-deps -- -D warnings
cd /Users/luolingfeng/Projects/LingXi-Next
git add lingxi-code/Cargo.toml lingxi-code/migrations
git commit -F /tmp/commit-msg.txt   # "feat(migrations): crate scaffold + ~/.claude.json path resolution" + footer
```

---

### Task 2: GlobalConfig read/save + project config

**Files:**
- Modify: `lingxi-code/migrations/src/global_config.rs`

- [ ] **Step 1: Failing tests** (append to `tests` module):

```rust
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
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p migrations` → FAIL.

- [ ] **Step 3: Implement** (append to `global_config.rs`):

```rust
/// Errors from the GlobalConfig substrate. All callers treat any error as
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
    std::fs::write(&tmp, serialized + "\n").map_err(GlobalConfigError::Io)?;
    std::fs::rename(&tmp, path).map_err(GlobalConfigError::Io)?;
    Ok(())
}

/// `getProjectPathForConfig` (`config.ts:1588-1601`): the canonical git root
/// of the directory (walk up looking for a `.git` entry — dir OR file, for
/// worktrees), else the canonicalized directory itself; forward slashes for
/// stable JSON keys.
pub fn project_path_for_config(dir: &Path) -> String {
    let resolved = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut cur: Option<&Path> = Some(&resolved);
    while let Some(p) = cur {
        if p.join(".git").exists() {
            return p.to_string_lossy().replace('\\', "/");
        }
        cur = p.parent();
    }
    resolved.to_string_lossy().replace('\\', "/")
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
```

NOTE for the `project_key_git_root_else_cwd` test on macOS: `/tmp` is a symlink to `/private/tmp`, hence comparing against `canonicalize()` output on both sides.

- [ ] **Step 4: Run tests.** `cargo test -p migrations` → PASS.

- [ ] **Step 5: Gate + commit** (same clippy gate; message `feat(migrations): GlobalConfig read/save + project-config accessors`).

---

### Task 3: settings writer (`settings_update.rs`)

**Files:**
- Create: `lingxi-code/migrations/src/settings_update.rs` (+ `pub mod settings_update;` already in lib.rs)

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;

    #[test]
    fn paths_for_sources() {
        let t = temp_config();
        assert_eq!(
            settings_path(SettingsSource::User, &t.home, &t.project),
            t.home.join("settings.json")
        );
        assert_eq!(
            settings_path(SettingsSource::Local, &t.home, &t.project),
            t.project.join(".claude").join("settings.local.json")
        );
    }

    #[test]
    fn update_creates_file_and_merges_and_deletes() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        update_settings(&path, vec![("model".into(), Some(serde_json::json!("opus")))]).unwrap();
        update_settings(&path, vec![("other".into(), Some(serde_json::json!(1)))]).unwrap();
        let map = read_settings_map(&path).unwrap();
        assert_eq!(map["model"], serde_json::json!("opus"));
        assert_eq!(map["other"], serde_json::json!(1));

        update_settings(&path, vec![("model".into(), None)]).unwrap();
        let map = read_settings_map(&path).unwrap();
        assert!(map.get("model").is_none());
        assert_eq!(map["other"], serde_json::json!(1));
    }

    #[test]
    fn update_bails_on_broken_json_without_overwriting() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ broken").unwrap();
        let res = update_settings(&path, vec![("x".into(), Some(serde_json::json!(1)))]);
        assert!(res.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
    }

    #[test]
    fn read_settings_map_missing_and_empty_are_empty() {
        let t = temp_config();
        let path = settings_path(SettingsSource::User, &t.home, &t.project);
        assert!(read_settings_map(&path).unwrap().is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "   \n").unwrap();
        assert!(read_settings_map(&path).unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Verify failure.** `cargo test -p migrations settings_update` → FAIL.

- [ ] **Step 3: Implement:**

```rust
//! `updateSettingsForSource` / `getSettingsForSource` port
//! (`utils/settings/settings.ts` L416/L459 semantics) for the two sources
//! the migrations write. Operates on raw JSON maps — unknown keys in the
//! user's real settings.json are preserved verbatim.
//!
//! Same error contract as the proven `commands/core/effort.rs` port:
//! missing/empty file merges into an empty object; syntactically broken JSON
//! bails WITHOUT overwriting. (effort.rs/permission::persist/tools-meta carry
//! private copies of this logic; consolidating them here is a noted follow-up,
//! out of scope for this batch.)

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Which settings file to address (the migrations only write these two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSource {
    /// `userSettings` → `<claude-config-home>/settings.json`.
    User,
    /// `localSettings` → `<project>/.claude/settings.local.json`.
    Local,
}

/// Resolve the file path for a source (TS `getSettingsFilePathForSource`).
pub fn settings_path(source: SettingsSource, claude_home: &Path, project_dir: &Path) -> PathBuf {
    match source {
        SettingsSource::User => claude_home.join("settings.json"),
        SettingsSource::Local => project_dir.join(".claude").join("settings.local.json"),
    }
}

/// Raw read of a settings file. Missing / blank ⇒ empty map; broken JSON ⇒
/// `Err` (caller decides; migrations treat it as their TS catch path).
pub fn read_settings_map(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(Map::new()),
        Ok(content) => serde_json::from_str(&content)
            .map_err(|_| format!("Invalid JSON syntax in settings file at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(format!(
            "Failed to read raw settings from {}: {e}",
            path.display()
        )),
    }
}

/// `updateSettingsForSource`: apply top-level key updates. `Some(v)` sets the
/// key, `None` deletes it (TS `mergeWith` treats `undefined` as delete).
/// All other keys preserved; pretty-printed + trailing newline.
pub fn update_settings(
    path: &Path,
    updates: Vec<(String, Option<Value>)>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let mut map = read_settings_map(path)?;
    for (key, value) in updates {
        match value {
            Some(v) => {
                map.insert(key, v);
            }
            None => {
                map.remove(&key);
            }
        }
    }
    let serialized = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| format!("Failed to serialize settings for {}: {e}", path.display()))?;
    std::fs::write(path, serialized + "\n")
        .map_err(|e| format!("Failed to write settings to {}: {e}", path.display()))
}
```

- [ ] **Step 4: Run tests → PASS. Gate + commit** (`feat(migrations): updateSettingsForSource port (user+local sources)`).

---

### Task 4: MigrationContext + MigrationEnv (`context.rs`)

**Files:**
- Create: `lingxi-code/migrations/src/context.rs`

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;

    #[test]
    fn env_truthy_values() {
        assert!(is_env_truthy(Some("1")));
        assert!(is_env_truthy(Some("TRUE")));
        assert!(is_env_truthy(Some(" yes ")));
        assert!(is_env_truthy(Some("on")));
        assert!(!is_env_truthy(Some("0")));
        assert!(!is_env_truthy(Some("")));
        assert!(!is_env_truthy(Some("anything")));
        assert!(!is_env_truthy(None));
    }

    #[test]
    fn js_truthy_values() {
        use serde_json::json;
        assert!(!js_truthy(&json!(null)));
        assert!(!js_truthy(&json!(false)));
        assert!(!js_truthy(&json!(0)));
        assert!(!js_truthy(&json!("")));
        assert!(js_truthy(&json!(true)));
        assert!(js_truthy(&json!(1)));
        assert!(js_truthy(&json!("x")));
        assert!(js_truthy(&json!([])));
        assert!(js_truthy(&json!({})));
    }

    #[test]
    fn first_party_unless_third_party_env() {
        let _g = env_lock();
        for var in ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"] {
            std::env::remove_var(var);
        }
        assert!(MigrationContext::from_env().first_party);
        std::env::set_var("CLAUDE_CODE_USE_BEDROCK", "1");
        assert!(!MigrationContext::from_env().first_party);
        std::env::remove_var("CLAUDE_CODE_USE_BEDROCK");
    }
}
```

- [ ] **Step 2: Verify failure**, then **Step 3: Implement:**

```rust
//! Cross-migration context: provider/subscription gates + JS-semantics
//! helpers + the per-run environment bundle.

use std::path::PathBuf;
use std::sync::Arc;

use anthropic_oauth::limits::SubscriptionType;
use serde_json::Value;
use telemetry::AnalyticsBus;

/// `isEnvTruthy` (`envUtils.ts:32-37`): unset/empty ⇒ false; else
/// lowercase-trim ∈ {1, true, yes, on}.
pub fn is_env_truthy(value: Option<&str>) -> bool {
    let Some(v) = value else { return false };
    matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Read + truthy-test an env var in one go.
pub fn env_var_truthy(name: &str) -> bool {
    is_env_truthy(std::env::var(name).ok().as_deref())
}

/// JS `Boolean(x)` over a JSON value (for raw-map reads where TS relies on
/// truthiness, e.g. `Boolean(oldValue)` / `!!userSettings.env?.X`).
pub fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Gates derived from the host environment (TS `getAPIProvider` +
/// subscription state).
#[derive(Debug, Clone)]
pub struct MigrationContext {
    /// `getAPIProvider() === 'firstParty'` (`providers.ts:6-14`): true unless
    /// `CLAUDE_CODE_USE_BEDROCK` / `CLAUDE_CODE_USE_VERTEX` /
    /// `CLAUDE_CODE_USE_FOUNDRY` is env-truthy.
    pub first_party: bool,
    /// Subscription tier. STRUCTURALLY `None` today: the Rust keychain token
    /// (`secret/src/credential.rs::OAuthTokens`) has no `subscriptionType`
    /// field. TS itself fails closed on unknown tier
    /// (`model.ts:322-330`), so `None` ⇒ the gated migrations take their
    /// faithful not-eligible branches. A future tier-persistence batch
    /// lights the eligible paths up without API change.
    pub subscription_type: Option<SubscriptionType>,
}

impl MigrationContext {
    /// Derive the context from process env. Tier is `None` (see field doc);
    /// the CLI deliberately does NOT read the keychain pre-boot for this
    /// (avoids a second keychain prompt).
    pub fn from_env() -> Self {
        let third_party = env_var_truthy("CLAUDE_CODE_USE_BEDROCK")
            || env_var_truthy("CLAUDE_CODE_USE_VERTEX")
            || env_var_truthy("CLAUDE_CODE_USE_FOUNDRY");
        Self {
            first_party: !third_party,
            subscription_type: None,
        }
    }
}

/// Everything one migration run needs: explicit paths (so tests never touch
/// process env), gates, and an optional telemetry bus.
pub struct MigrationEnv {
    /// `~/.claude.json` (resolved by `global_config::global_config_path`).
    pub global_config_path: PathBuf,
    /// `~/.claude` (config home — settings.json + cache/ live here).
    pub claude_config_home: PathBuf,
    /// Project directory (for `settings.local.json` + the project-config key).
    pub project_dir: PathBuf,
    /// Provider/subscription gates.
    pub ctx: MigrationContext,
    /// Telemetry sink; `None` = no events (unit tests, and the CLI today —
    /// no pre-boot bus substrate exists, same as the startup deprecation
    /// notice; names are registered for the future wiring).
    pub bus: Option<Arc<AnalyticsBus>>,
}

impl MigrationEnv {
    /// Emit a tengu event if a bus is wired.
    pub(crate) async fn emit(&self, name: &str, metadata: telemetry::sink::LogEventMetadata) {
        if let Some(bus) = &self.bus {
            bus.log_event(name, metadata).await;
        }
    }

    /// Epoch milliseconds (`Date.now()` parity for the `*Timestamp` keys).
    pub(crate) fn now_ms() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(i64::MAX)
    }
}
```

Add `pub mod context;` etc. to lib.rs as modules land (lib.rs from Task 1 already declares context/global_config/settings_update).

- [ ] **Step 4: Run `cargo test -p migrations` → PASS. Gate + commit** (`feat(migrations): MigrationContext/MigrationEnv + JS-semantics helpers`).

---

### Task 5: telemetry — 9 migration event names (registry + fixture + count sweep)

Done BEFORE the migrations so their emit sites reference registered names. **This is the W36/W38 shared-constant hazard task — follow the sweep exactly.**

**Files:**
- Create: `lingxi-code/telemetry/src/tengu/migration.rs`
- Modify: `lingxi-code/telemetry/src/tengu/mod.rs`
- Modify: `lingxi-code/test-harness/src/parity/fixtures/tengu_events.json`
- Modify: every `339` count-assert site (sweep below)

- [ ] **Step 1: New module** `telemetry/src/tengu/migration.rs`:

```rust
//! Config-migration events (`src/migrations/*.ts` `logEvent` sites). Appended
//! as their own registry block (after the FileRead global-tail trio).

/// `tengu_migrate_autoupdates_to_settings` (`migrateAutoUpdatesToSettings.ts:38`).
pub const MIGRATE_AUTOUPDATES_TO_SETTINGS: &str = "tengu_migrate_autoupdates_to_settings";
/// `tengu_migrate_autoupdates_error` (`migrateAutoUpdatesToSettings.ts:56`).
pub const MIGRATE_AUTOUPDATES_ERROR: &str = "tengu_migrate_autoupdates_error";
/// `tengu_migrate_bypass_permissions_accepted` (`migrateBypassPermissionsAcceptedToSettings.ts:29`).
pub const MIGRATE_BYPASS_PERMISSIONS_ACCEPTED: &str = "tengu_migrate_bypass_permissions_accepted";
/// `tengu_migrate_mcp_approval_fields_success` (`migrateEnableAllProjectMcpServersToSettings.ts:110`).
pub const MIGRATE_MCP_APPROVAL_FIELDS_SUCCESS: &str = "tengu_migrate_mcp_approval_fields_success";
/// `tengu_migrate_mcp_approval_fields_error` (`migrateEnableAllProjectMcpServersToSettings.ts:115`).
pub const MIGRATE_MCP_APPROVAL_FIELDS_ERROR: &str = "tengu_migrate_mcp_approval_fields_error";
/// `tengu_reset_pro_to_opus_default` (`resetProToOpusDefault.ts`).
pub const RESET_PRO_TO_OPUS_DEFAULT: &str = "tengu_reset_pro_to_opus_default";
/// `tengu_legacy_opus_migration` (`migrateLegacyOpusToCurrent.ts:53`).
pub const LEGACY_OPUS_MIGRATION: &str = "tengu_legacy_opus_migration";
/// `tengu_sonnet45_to_46_migration` (`migrateSonnet45ToSonnet46.ts:63`).
pub const SONNET45_TO_46_MIGRATION: &str = "tengu_sonnet45_to_46_migration";
/// `tengu_opus_to_opus1m_migration` (`migrateOpusToOpus1m.ts:41`).
pub const OPUS_TO_OPUS1M_MIGRATION: &str = "tengu_opus_to_opus1m_migration";

/// Registry block — order matches TS `runMigrations` execution order
/// (`main.tsx:328-336`), error events directly after their success twin.
pub const NAMES: [&str; 9] = [
    MIGRATE_AUTOUPDATES_TO_SETTINGS,
    MIGRATE_AUTOUPDATES_ERROR,
    MIGRATE_BYPASS_PERMISSIONS_ACCEPTED,
    MIGRATE_MCP_APPROVAL_FIELDS_SUCCESS,
    MIGRATE_MCP_APPROVAL_FIELDS_ERROR,
    RESET_PRO_TO_OPUS_DEFAULT,
    LEGACY_OPUS_MIGRATION,
    SONNET45_TO_46_MIGRATION,
    OPUS_TO_OPUS1M_MIGRATION,
];
```

- [ ] **Step 2: Register in `tengu/mod.rs`:** add `pub mod migration;` next to the other module decls; append a history comment line `// Config migrations: +9 (migration::NAMES, runMigrations port) → 348 total.`; change `const TOTAL: usize = 25 + 30 + 20 + 140 + 10 + 8 + 12 + 3 + 17 + 4 + 54 + 13 + 3;` to end `… + 13 + 3 + 9;` and add the copy loop in `concat_all()` AFTER the FileRead-analytics tail loop (find the last `while i < …NAMES.len()` block and clone its shape):

```rust
        let mut i = 0;
        while i < migration::NAMES.len() {
            out[idx] = migration::NAMES[i];
            idx += 1;
            i += 1;
        }
```

- [ ] **Step 3: Fixture append.** In `lingxi-code/test-harness/src/parity/fixtures/tengu_events.json`, append the 9 names IN `NAMES` ORDER at the very end of the names array (the parity test asserts position-by-position; the registry appends this block last, so the fixture tail must match). Update any `total`/count field in the fixture header if present (open the file to check its exact shape first).

- [ ] **Step 4: THE SWEEP (W36/W38 lesson).** Find every hardcoded count:

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
grep -rn "339" --include="*.rs" . | grep -v target | grep -i "event\|tengu\|ALL_EVENT"
```

Known sites to update 339 → 348 (re-grep regardless — more may have landed):
- `tui/src/components/prompt_input/vim.rs:1602` and `:1613`
- `tui/tests/behavior_palette.rs:147` (+ its history comment line 145)
- `telemetry/tests/event_name_completeness_test.rs:50` (+ `:59` region if it re-counts)
- `telemetry/tests/settings_schema_test.rs:23` region (check whether it hardcodes or derives)
- `orchestrator/src/diagnostics.rs:189` (`let expected = 339;` + append a history comment line)

- [ ] **Step 5: RUN (not `--no-run`) the dependent crates' tests:**

```bash
cargo test -p telemetry
cargo test -p orchestrator diagnostics
cargo test -p tui vim
cargo test -p tui --test behavior_palette
cargo test -p test-harness --test parity_tengu_events
```

Expected: ALL PASS.

- [ ] **Step 6: Gate + commit** (`feat(telemetry): register 9 config-migration tengu events (339→348)`). Clippy: `cargo clippy -p telemetry --all-targets --no-deps -- -D warnings`.

---

### Task 6: migrations trio #1 — replBridge rename, sonnet[1m]→4.5, legacyOpus

**Files:**
- Create: `migrations/src/migrate_repl_bridge.rs`, `migrate_sonnet1m_to_sonnet45.rs`, `migrate_legacy_opus.rs`
- Modify: `migrations/src/lib.rs` (add `pub mod migrate_repl_bridge; pub mod migrate_sonnet1m_to_sonnet45; pub mod migrate_legacy_opus;`)

Test scaffold shared by migration tests (write per-file): build a `MigrationEnv` from `temp_config()`:

```rust
fn test_env(t: &crate::test_support::TempConfig) -> crate::context::MigrationEnv {
    crate::context::MigrationEnv {
        global_config_path: t.global.clone(),
        claude_config_home: t.home.clone(),
        project_dir: t.project.clone(),
        ctx: crate::context::MigrationContext { first_party: true, subscription_type: None },
        bus: None,
    }
}
```

(Define it locally in each migration's `tests` module — three small copies beat a pub-for-tests helper that would need missing-docs ceremony; if the implementer prefers, a `pub(crate)` fn in `test_support.rs` is fine too.)

- [ ] **Step 1: Failing tests — `migrate_repl_bridge.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;
    use serde_json::json;

    // test_env helper as above

    #[tokio::test]
    async fn renames_old_key_when_new_absent() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"replBridgeEnabled": 1}"#).unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["remoteControlAtStartup"], json!(true)); // Boolean(1)
        assert!(m.get("replBridgeEnabled").is_none());
    }

    #[tokio::test]
    async fn no_old_key_is_noop_even_for_null() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"other": 1}"#).unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("remoteControlAtStartup").is_none());
        // JSON null is NOT undefined: TS `oldValue === undefined` only skips
        // a MISSING key — null proceeds and Boolean(null)=false.
        std::fs::write(&t.global, r#"{"replBridgeEnabled": null}"#).unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["remoteControlAtStartup"], json!(false));
        assert!(m.get("replBridgeEnabled").is_none());
    }

    #[tokio::test]
    async fn existing_new_key_blocks_migration() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"replBridgeEnabled": true, "remoteControlAtStartup": false}"#,
        )
        .unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["remoteControlAtStartup"], json!(false));
        assert_eq!(m["replBridgeEnabled"], json!(true)); // untouched
    }
}
```

- [ ] **Step 2: Implement `migrate_repl_bridge.rs`:**

```rust
//! `migrateReplBridgeEnabledToRemoteControlAtStartup.ts` — copy
//! `replBridgeEnabled` to `remoteControlAtStartup` (JS-truthy-coerced) and
//! drop the old key. Idempotent: only acts when the old key exists and the
//! new one doesn't.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use serde_json::Value;

/// Run the migration. Errors are swallowed with a `tracing::warn!` —
/// migrations never abort startup.
pub async fn run(env: &MigrationEnv) {
    let result = global_config::save_map(&env.global_config_path, |mut cfg| {
        let Some(old) = cfg.get("replBridgeEnabled").cloned() else {
            return cfg;
        };
        if cfg.get("remoteControlAtStartup").is_some() {
            return cfg;
        }
        cfg.insert("remoteControlAtStartup".into(), Value::Bool(js_truthy(&old)));
        cfg.remove("replBridgeEnabled");
        cfg
    });
    if let Err(e) = result {
        tracing::warn!(error = %e, "migrate_repl_bridge failed");
    }
}
```

- [ ] **Step 3: Failing tests — `migrate_sonnet1m_to_sonnet45.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{settings_path, read_settings_map, SettingsSource};
    use crate::test_support::temp_config;
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn rewrites_sonnet1m_and_sets_flag() {
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "sonnet[1m]", "keep": true}"#).unwrap();
        run(&test_env(&t)).await;
        let s = read_settings_map(&sp).unwrap();
        assert_eq!(s["model"], json!("sonnet-4-5-20250929[1m]"));
        assert_eq!(s["keep"], json!(true));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
    }

    #[tokio::test]
    async fn other_model_untouched_but_flag_still_set() {
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "opus"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("opus"));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
    }

    #[tokio::test]
    async fn completion_flag_short_circuits() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"sonnet1m45MigrationComplete": true}"#).unwrap();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "sonnet[1m]"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("sonnet[1m]"));
    }
}
```

- [ ] **Step 4: Implement `migrate_sonnet1m_to_sonnet45.rs`:**

```rust
//! `migrateSonnet1mToSonnet45.ts` — pin users who saved `sonnet[1m]` to the
//! explicit `sonnet-4-5-20250929[1m]` (the bare alias now resolves to 4.6).
//! Reads userSettings specifically (NOT merged) so a project-scoped pin isn't
//! promoted to the global default. Run-once via the
//! `sonnet1m45MigrationComplete` global-config flag (set even when the model
//! didn't match — TS parity).
//!
//! NOT ported: the TS in-memory `MainLoopModelOverride` sub-step
//! (`migrateSonnet1mToSonnet45.ts:39-42`) — no equivalent pre-boot in-memory
//! model state exists in the Rust CLI.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Value};

/// Run the migration (errors swallowed with a warn, never abort startup).
pub async fn run(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: config read failed");
            return;
        }
    };
    if cfg.get("sonnet1m45MigrationComplete").is_some_and(js_truthy) {
        return;
    }

    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let model = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from));
    if model.as_deref() == Some("sonnet[1m]") {
        if let Err(e) = update_settings(
            &sp,
            vec![("model".into(), Some(json!("sonnet-4-5-20250929[1m]")))],
        ) {
            tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: settings write failed");
        }
    }

    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.insert("sonnet1m45MigrationComplete".into(), Value::Bool(true));
        m
    }) {
        tracing::warn!(error = %e, "migrate_sonnet1m_to_sonnet45: flag write failed");
    }
}
```

- [ ] **Step 5: Failing tests — `migrate_legacy_opus.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
    use crate::test_support::{env_lock, temp_config};
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn rewrites_each_legacy_string_and_stamps_timestamp() {
        let _g = env_lock(); // reads CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        for legacy in [
            "claude-opus-4-20250514",
            "claude-opus-4-1-20250805",
            "claude-opus-4-0",
            "claude-opus-4-1",
        ] {
            let t = temp_config();
            let sp = settings_path(SettingsSource::User, &t.home, &t.project);
            std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
            std::fs::write(&sp, format!(r#"{{"model": "{legacy}"}}"#)).unwrap();
            run(&test_env(&t)).await;
            assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("opus"));
            let m = crate::global_config::read_map(&t.global).unwrap();
            assert!(m["legacyOpusMigrationTimestamp"].is_i64());
        }
    }

    #[tokio::test]
    async fn non_first_party_or_optout_or_other_model_noop() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "claude-opus-4-0"}"#).unwrap();

        // not first-party
        let mut env = test_env(&t);
        env.ctx.first_party = false;
        run(&env).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("claude-opus-4-0"));

        // env opt-out
        std::env::set_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP", "1");
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("claude-opus-4-0"));
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");

        // non-legacy model
        std::fs::write(&sp, r#"{"model": "sonnet"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], json!("sonnet"));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("legacyOpusMigrationTimestamp").is_none());
    }
}
```

- [ ] **Step 6: Implement `migrate_legacy_opus.rs`:**

```rust
//! `migrateLegacyOpusToCurrent.ts` — move first-party users off explicit
//! Opus 4.0/4.1 strings to the `opus` alias; stamp
//! `legacyOpusMigrationTimestamp` for the (unported) REPL one-time notice.
//! Idempotent by construction: once rewritten, the model no longer matches.

use crate::context::{env_var_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Value};
use telemetry::sink::AnalyticsValue;

/// The four explicit legacy strings (`migrateLegacyOpusToCurrent.ts:41-46`).
const LEGACY_MODELS: [&str; 4] = [
    "claude-opus-4-20250514",
    "claude-opus-4-1-20250805",
    "claude-opus-4-0",
    "claude-opus-4-1",
];

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    if !env.ctx.first_party {
        return;
    }
    // isLegacyModelRemapEnabled (`model.ts:552-554`) = NOT env-truthy opt-out.
    if env_var_truthy("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP") {
        return;
    }

    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let Some(model) = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from))
    else {
        return;
    };
    if !LEGACY_MODELS.contains(&model.as_str()) {
        return;
    }

    if let Err(e) = update_settings(&sp, vec![("model".into(), Some(json!("opus")))]) {
        tracing::warn!(error = %e, "migrate_legacy_opus: settings write failed");
        return;
    }
    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.insert(
            "legacyOpusMigrationTimestamp".into(),
            json!(MigrationEnv::now_ms()),
        );
        m
    }) {
        tracing::warn!(error = %e, "migrate_legacy_opus: timestamp write failed");
    }
    env.emit(
        telemetry::tengu::migration::LEGACY_OPUS_MIGRATION,
        std::collections::HashMap::from([(
            "from_model".to_string(),
            AnalyticsValue::String(model),
        )]),
    )
    .await;
}
```

(`AnalyticsValue` variants: `Bool(bool)/Int(i64)/Float(f64)/String(String)/None` — `telemetry/src/sink.rs:22-33`.)

- [ ] **Step 7: Run `cargo test -p migrations` → PASS. Gate + commit** (`feat(migrations): replBridge rename + sonnet[1m]→4.5 + legacy-opus migrations`).

---

### Task 7: autoUpdates + bypassPermissions migrations

**Files:**
- Create: `migrations/src/migrate_auto_updates.rs`, `migrations/src/migrate_bypass_permissions.rs` (+ lib.rs decls)

- [ ] **Step 1: Failing tests — `migrate_auto_updates.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
    use crate::test_support::{env_lock, temp_config};
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn migrates_explicit_false_to_settings_env() {
        let _g = env_lock(); // sets process env DISABLE_AUTOUPDATER
        std::env::remove_var("DISABLE_AUTOUPDATER");
        let t = temp_config();
        std::fs::write(&t.global, r#"{"autoUpdates": false, "keep": 1}"#).unwrap();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"env": {"EXISTING": "x"}}"#).unwrap();

        run(&test_env(&t)).await;

        let s = read_settings_map(&sp).unwrap();
        assert_eq!(s["env"]["DISABLE_AUTOUPDATER"], json!("1"));
        assert_eq!(s["env"]["EXISTING"], json!("x")); // spread-merge preserved
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("autoUpdates").is_none());
        assert!(m.get("autoUpdatesProtectedForNative").is_none());
        assert_eq!(m["keep"], json!(1));
        assert_eq!(std::env::var("DISABLE_AUTOUPDATER").unwrap(), "1");
        std::env::remove_var("DISABLE_AUTOUPDATER");
    }

    #[tokio::test]
    async fn skips_true_missing_or_protected() {
        let _g = env_lock();
        // autoUpdates true → skip
        let t = temp_config();
        std::fs::write(&t.global, r#"{"autoUpdates": true}"#).unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["autoUpdates"], json!(true));

        // protected → skip
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"autoUpdates": false, "autoUpdatesProtectedForNative": true}"#,
        )
        .unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["autoUpdates"], json!(false)); // untouched
    }
}
```

- [ ] **Step 2: Implement `migrate_auto_updates.rs`:**

```rust
//! `migrateAutoUpdatesToSettings.ts` — move a user-set `autoUpdates: false`
//! preference into `settings.json env.DISABLE_AUTOUPDATER = "1"`, set the
//! process env var so it takes effect immediately, and drop the old config
//! keys. The Rust port has no auto-updater consumer; the file/env effects
//! are the faithful contract.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Map, Value};
use telemetry::sink::AnalyticsValue;

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "migrate_auto_updates: config read failed");
            return;
        }
    };
    // Only when autoUpdates was EXPLICITLY false and not native-protected
    // (`migrateAutoUpdatesToSettings.ts:19-24`).
    if cfg.get("autoUpdates") != Some(&Value::Bool(false))
        || cfg.get("autoUpdatesProtectedForNative") == Some(&Value::Bool(true))
    {
        return;
    }

    // TS try block: settings write + event + env var + config-key removal;
    // any failure → tengu_migrate_autoupdates_error.
    let result: Result<bool, String> = async {
        let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
        let user = read_settings_map(&sp)?;
        let already_had = user
            .get("env")
            .and_then(|e| e.get("DISABLE_AUTOUPDATER"))
            .is_some_and(js_truthy);
        let mut env_map: Map<String, Value> = user
            .get("env")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        env_map.insert("DISABLE_AUTOUPDATER".into(), json!("1"));
        update_settings(&sp, vec![("env".into(), Some(Value::Object(env_map)))])?;
        Ok(already_had)
    }
    .await;

    match result {
        Ok(already_had) => {
            env.emit(
                telemetry::tengu::migration::MIGRATE_AUTOUPDATES_TO_SETTINGS,
                std::collections::HashMap::from([
                    ("was_user_preference".to_string(), AnalyticsValue::Bool(true)),
                    ("already_had_env_var".to_string(), AnalyticsValue::Bool(already_had)),
                ]),
            )
            .await;
            // explicitly set, so this takes effect immediately (TS:44)
            std::env::set_var("DISABLE_AUTOUPDATER", "1");
            if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
                m.remove("autoUpdates");
                m.remove("autoUpdatesProtectedForNative");
                m
            }) {
                tracing::warn!(error = %e, "migrate_auto_updates: config cleanup failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "migrate_auto_updates failed");
            env.emit(
                telemetry::tengu::migration::MIGRATE_AUTOUPDATES_ERROR,
                std::collections::HashMap::from([(
                    "has_error".to_string(),
                    AnalyticsValue::Bool(true),
                )]),
            )
            .await;
        }
    }
}
```

- [ ] **Step 3: Failing tests — `migrate_bypass_permissions.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
    use crate::test_support::temp_config;
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn moves_flag_to_user_settings_and_removes_config_key() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"bypassPermissionsModeAccepted": true}"#).unwrap();
        run(&test_env(&t)).await;
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        let s = read_settings_map(&sp).unwrap();
        assert_eq!(s["skipDangerousModePermissionPrompt"], json!(true));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("bypassPermissionsModeAccepted").is_none());
    }

    #[tokio::test]
    async fn existing_skip_flag_in_local_settings_is_not_overwritten() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"bypassPermissionsModeAccepted": true}"#).unwrap();
        let lp = settings_path(SettingsSource::Local, &t.home, &t.project);
        std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
        std::fs::write(&lp, r#"{"skipDangerousModePermissionPrompt": true}"#).unwrap();
        run(&test_env(&t)).await;
        // userSettings must NOT gain the key (TS: hasSkip… short-circuits)
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        let s = read_settings_map(&sp).unwrap();
        assert!(s.get("skipDangerousModePermissionPrompt").is_none());
        // config key still removed
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("bypassPermissionsModeAccepted").is_none());
    }

    #[tokio::test]
    async fn absent_or_falsy_config_flag_is_noop() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"bypassPermissionsModeAccepted": false}"#).unwrap();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["bypassPermissionsModeAccepted"], json!(false)); // untouched
    }
}
```

- [ ] **Step 4: Implement `migrate_bypass_permissions.rs`:**

```rust
//! `migrateBypassPermissionsAcceptedToSettings.ts` — move
//! `bypassPermissionsModeAccepted` from global config to
//! `settings.json skipDangerousModePermissionPrompt`. The written key has no
//! Rust consumer yet (the `--dangerously-skip-permissions` posture is the
//! separately-confirmed remainder item B) — the file-level move is the
//! faithful contract.
//!
//! `hasSkipDangerousModePermissionPrompt` (`settings.ts:882-889`) checks
//! user/local/flag/policy sources; this port checks user+local (the flag and
//! policy sources have no Rust substrate — documented).

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::{json, Value};

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "migrate_bypass_permissions: config read failed");
            return;
        }
    };
    if !cfg.get("bypassPermissionsModeAccepted").is_some_and(js_truthy) {
        return;
    }

    let has_skip = [SettingsSource::User, SettingsSource::Local].iter().any(|s| {
        let p = settings_path(*s, &env.claude_config_home, &env.project_dir);
        read_settings_map(&p)
            .ok()
            .and_then(|m| m.get("skipDangerousModePermissionPrompt").map(js_truthy))
            .unwrap_or(false)
    });
    if !has_skip {
        let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
        if let Err(e) = update_settings(
            &sp,
            vec![("skipDangerousModePermissionPrompt".into(), Some(json!(true)))],
        ) {
            tracing::warn!(error = %e, "migrate_bypass_permissions: settings write failed");
            return; // TS catch: config key NOT removed on failure
        }
    }

    env.emit(
        telemetry::tengu::migration::MIGRATE_BYPASS_PERMISSIONS_ACCEPTED,
        std::collections::HashMap::new(),
    )
    .await;

    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.remove("bypassPermissionsModeAccepted");
        m
    }) {
        tracing::warn!(error = %e, "migrate_bypass_permissions: config cleanup failed");
    }
}
```

(`Value` import used by `json!` expansion only — drop it if clippy flags unused.)

- [ ] **Step 5: Run `cargo test -p migrations` → PASS. Gate + commit** (`feat(migrations): autoUpdates→settings-env + bypassPermissions→settings`).

---

### Task 8: MCP approval-fields migration

**Files:**
- Create: `migrations/src/migrate_mcp_servers.rs` (+ lib.rs decl)

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
    use crate::test_support::temp_config;
    use serde_json::json;

    // test_env helper — NOTE: the project key for temp dirs:
    fn project_key(t: &crate::test_support::TempConfig) -> String {
        crate::global_config::project_path_for_config(&t.project)
    }

    #[tokio::test]
    async fn moves_all_three_fields_to_local_settings() {
        let t = temp_config();
        let key = project_key(&t);
        std::fs::write(
            &t.global,
            serde_json::to_string(&json!({"projects": {key.clone(): {
                "enableAllProjectMcpServers": true,
                "enabledMcpjsonServers": ["a", "b"],
                "disabledMcpjsonServers": ["c"],
                "other": 1
            }}}))
            .unwrap(),
        )
        .unwrap();
        run(&test_env(&t)).await;

        let lp = settings_path(SettingsSource::Local, &t.home, &t.project);
        let s = read_settings_map(&lp).unwrap();
        assert_eq!(s["enableAllProjectMcpServers"], json!(true));
        assert_eq!(s["enabledMcpjsonServers"], json!(["a", "b"]));
        assert_eq!(s["disabledMcpjsonServers"], json!(["c"]));

        let proj = crate::global_config::get_project_config(&t.global, &key).unwrap();
        assert!(proj.get("enableAllProjectMcpServers").is_none());
        assert!(proj.get("enabledMcpjsonServers").is_none());
        assert!(proj.get("disabledMcpjsonServers").is_none());
        assert_eq!(proj["other"], json!(1));
    }

    #[tokio::test]
    async fn merges_server_lists_dedup_preserving_existing_order() {
        let t = temp_config();
        let key = project_key(&t);
        std::fs::write(
            &t.global,
            serde_json::to_string(&json!({"projects": {key: {
                "enabledMcpjsonServers": ["b", "c"]
            }}}))
            .unwrap(),
        )
        .unwrap();
        let lp = settings_path(SettingsSource::Local, &t.home, &t.project);
        std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
        std::fs::write(&lp, r#"{"enabledMcpjsonServers": ["a", "b"]}"#).unwrap();
        run(&test_env(&t)).await;
        let s = read_settings_map(&lp).unwrap();
        // [...new Set([...existing, ...incoming])] = a, b, c
        assert_eq!(s["enabledMcpjsonServers"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn already_migrated_enable_all_is_removed_but_not_overwritten() {
        let t = temp_config();
        let key = project_key(&t);
        std::fs::write(
            &t.global,
            serde_json::to_string(&json!({"projects": {key.clone(): {
                "enableAllProjectMcpServers": true
            }}}))
            .unwrap(),
        )
        .unwrap();
        let lp = settings_path(SettingsSource::Local, &t.home, &t.project);
        std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
        std::fs::write(&lp, r#"{"enableAllProjectMcpServers": false}"#).unwrap();
        run(&test_env(&t)).await;
        let s = read_settings_map(&lp).unwrap();
        assert_eq!(s["enableAllProjectMcpServers"], json!(false)); // kept
        let proj = crate::global_config::get_project_config(&t.global, &key).unwrap();
        assert!(proj.get("enableAllProjectMcpServers").is_none()); // removed
    }

    #[tokio::test]
    async fn nothing_to_migrate_is_noop() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"projects": {}}"#).unwrap();
        let before = std::fs::read_to_string(&t.global).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), before);
    }
}
```

- [ ] **Step 2: Implement `migrate_mcp_servers.rs`:**

```rust
//! `migrateEnableAllProjectMcpServersToSettings.ts` — move the three MCP
//! approval fields from the project config (inside `~/.claude.json
//! projects[<key>]`) into `<project>/.claude/settings.local.json`.
//! No Rust reader consumes these settings keys yet; the file-level move is
//! the faithful contract.

use crate::context::MigrationEnv;
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use serde_json::Value;
use telemetry::sink::AnalyticsValue;

/// JS `[...new Set([...a, ...b])]`: a's order, then b's not-already-present.
fn union_preserving_order(existing: &[Value], incoming: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = existing.to_vec();
    for v in incoming {
        if !out.contains(v) {
            out.push(v.clone());
        }
    }
    out
}

/// Run the migration.
#[allow(clippy::too_many_lines)]
pub async fn run(env: &MigrationEnv) {
    let key = global_config::project_path_for_config(&env.project_dir);
    let proj = match global_config::get_project_config(&env.global_config_path, &key) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "migrate_mcp_servers: config read failed");
            return;
        }
    };

    let has_enable_all = proj.get("enableAllProjectMcpServers").is_some();
    let enabled: Vec<Value> = proj
        .get("enabledMcpjsonServers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let disabled: Vec<Value> = proj
        .get("disabledMcpjsonServers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !has_enable_all && enabled.is_empty() && disabled.is_empty() {
        return;
    }

    // TS try block — any failure routes to the error event.
    let result: Result<usize, String> = async {
        let lp = settings_path(SettingsSource::Local, &env.claude_config_home, &env.project_dir);
        let existing = read_settings_map(&lp)?;
        let mut updates: Vec<(String, Option<Value>)> = Vec::new();
        let mut fields_to_remove = 0usize;

        if has_enable_all {
            if existing.get("enableAllProjectMcpServers").is_none() {
                updates.push((
                    "enableAllProjectMcpServers".into(),
                    proj.get("enableAllProjectMcpServers").cloned(),
                ));
            }
            fields_to_remove += 1;
        }
        if !enabled.is_empty() {
            let merged = union_preserving_order(
                existing
                    .get("enabledMcpjsonServers")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &enabled,
            );
            updates.push(("enabledMcpjsonServers".into(), Some(Value::Array(merged))));
            fields_to_remove += 1;
        }
        if !disabled.is_empty() {
            let merged = union_preserving_order(
                existing
                    .get("disabledMcpjsonServers")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &disabled,
            );
            updates.push(("disabledMcpjsonServers".into(), Some(Value::Array(merged))));
            fields_to_remove += 1;
        }

        if !updates.is_empty() {
            update_settings(&lp, updates)?;
        }
        // TS removes ALL THREE keys from project config in one destructure.
        global_config::save_project_config(&env.global_config_path, &key, |mut p| {
            p.remove("enableAllProjectMcpServers");
            p.remove("enabledMcpjsonServers");
            p.remove("disabledMcpjsonServers");
            p
        })
        .map_err(|e| e.to_string())?;
        Ok(fields_to_remove)
    }
    .await;

    match result {
        Ok(migrated) => {
            env.emit(
                telemetry::tengu::migration::MIGRATE_MCP_APPROVAL_FIELDS_SUCCESS,
                std::collections::HashMap::from([(
                    "migratedCount".to_string(),
                    AnalyticsValue::Int(i64::try_from(migrated).unwrap_or(i64::MAX)),
                )]),
            )
            .await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "migrate_mcp_servers failed");
            env.emit(
                telemetry::tengu::migration::MIGRATE_MCP_APPROVAL_FIELDS_ERROR,
                std::collections::HashMap::new(),
            )
            .await;
        }
    }
}
```

(`AnalyticsValue` variants are `Bool/Int/Float/String/None` — `telemetry/src/sink.rs:22-33`; the code above uses them directly.)

- [ ] **Step 3: Run tests → PASS. Gate + commit** (`feat(migrations): MCP approval fields project-config→settings.local`).

---

### Task 9: subscriber-gated trio — resetProToOpus, sonnet45→46, opus→opus[1m]

**Files:**
- Create: `migrations/src/migrate_reset_pro_to_opus.rs`, `migrate_sonnet45_to_46.rs`, `migrate_opus_to_opus1m.rs` (+ lib.rs decls)

All three are gated on `ctx.subscription_type`, which is structurally `None` today — TS itself fails closed on unknown tier, so the not-eligible branches ARE the faithful behavior. Eligible bodies are still ported (dormant) with `Some(tier)` covered in tests.

- [ ] **Step 1: Implement `migrate_reset_pro_to_opus.rs`** (with tests):

```rust
//! `resetProToOpusDefault.ts` — one-shot flag/timestamp bookkeeping for the
//! Pro→Opus default switch. Run-once via `opusProMigrationComplete`.
//!
//! Tier is structurally `None` in this port (no keychain subscriptionType) ⇒
//! the not-Pro branch runs: mark complete + `skipped: true` event — which is
//! TS's own behavior for non-Pro/non-firstParty users, not a stub.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, SettingsSource};
use anthropic_oauth::limits::SubscriptionType;
use serde_json::{json, Value};
use telemetry::sink::AnalyticsValue;

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "reset_pro_to_opus: config read failed");
            return;
        }
    };
    if cfg.get("opusProMigrationComplete").is_some_and(js_truthy) {
        return;
    }

    let is_pro = env.ctx.subscription_type == Some(SubscriptionType::Pro);
    if !env.ctx.first_party || !is_pro {
        if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
            m.insert("opusProMigrationComplete".into(), Value::Bool(true));
            m
        }) {
            tracing::warn!(error = %e, "reset_pro_to_opus: flag write failed");
        }
        env.emit(
            telemetry::tengu::migration::RESET_PRO_TO_OPUS_DEFAULT,
            std::collections::HashMap::from([("skipped".to_string(), AnalyticsValue::Bool(true))]),
        )
        .await;
        return;
    }

    // DORMANT until tier persistence lands. TS reads getSettings_DEPRECATED
    // (merged settings); the user-settings model is the in-port stand-in
    // (doc'd: the merged read has no substrate at this pre-boot point).
    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let has_custom_model = read_settings_map(&sp)
        .ok()
        .is_some_and(|m| m.get("model").is_some());
    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        m.insert("opusProMigrationComplete".into(), Value::Bool(true));
        if !has_custom_model {
            m.insert("opusProMigrationTimestamp".into(), json!(MigrationEnv::now_ms()));
        }
        m
    }) {
        tracing::warn!(error = %e, "reset_pro_to_opus: flag write failed");
    }
    env.emit(
        telemetry::tengu::migration::RESET_PRO_TO_OPUS_DEFAULT,
        std::collections::HashMap::from([
            ("skipped".to_string(), AnalyticsValue::Bool(false)),
            ("had_custom_model".to_string(), AnalyticsValue::Bool(has_custom_model)),
        ]),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn tier_none_marks_complete_and_skips() {
        let t = temp_config();
        run(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["opusProMigrationComplete"], json!(true));
        assert!(m.get("opusProMigrationTimestamp").is_none());
    }

    #[tokio::test]
    async fn complete_flag_short_circuits() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"opusProMigrationComplete": true}"#).unwrap();
        let before = std::fs::read_to_string(&t.global).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), before);
    }

    #[tokio::test]
    async fn pro_first_party_default_model_stamps_timestamp() {
        let t = temp_config();
        let mut env = test_env(&t);
        env.ctx.subscription_type = Some(SubscriptionType::Pro);
        run(&env).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["opusProMigrationComplete"], json!(true));
        assert!(m["opusProMigrationTimestamp"].is_i64());
    }
}
```

- [ ] **Step 2: Implement `migrate_sonnet45_to_46.rs`** (with tests):

```rust
//! `migrateSonnet45ToSonnet46.ts` — move Pro/Max/Team-Premium first-party
//! users off explicit Sonnet 4.5 strings to the `sonnet` alias. Tier `None`
//! ⇒ early-return (TS non-subscriber path). Rust `SubscriptionType::Team`
//! cannot distinguish Premium from Standard (TS gates on Team PREMIUM) —
//! `Team` is accepted, documented divergence in the dormant path.

use crate::context::{js_truthy, MigrationEnv};
use crate::global_config;
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use anthropic_oauth::limits::SubscriptionType;
use serde_json::{json, Value};
use telemetry::sink::AnalyticsValue;

/// The explicit Sonnet 4.5 strings (`migrateSonnet45ToSonnet46.ts:40-45`).
const SONNET45_MODELS: [&str; 4] = [
    "claude-sonnet-4-5-20250929",
    "claude-sonnet-4-5-20250929[1m]",
    "sonnet-4-5-20250929",
    "sonnet-4-5-20250929[1m]",
];

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    if !env.ctx.first_party {
        return;
    }
    let eligible = matches!(
        env.ctx.subscription_type,
        Some(SubscriptionType::Pro | SubscriptionType::Max | SubscriptionType::Team)
    );
    if !eligible {
        return;
    }

    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let Some(model) = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from))
    else {
        return;
    };
    if !SONNET45_MODELS.contains(&model.as_str()) {
        return;
    }

    let has_1m = model.ends_with("[1m]");
    let target = if has_1m { "sonnet[1m]" } else { "sonnet" };
    if let Err(e) = update_settings(&sp, vec![("model".into(), Some(json!(target)))]) {
        tracing::warn!(error = %e, "migrate_sonnet45_to_46: settings write failed");
        return;
    }

    // Skip notification for brand-new users (numStartups <= 1).
    let num_startups = global_config::read_map(&env.global_config_path)
        .ok()
        .and_then(|m| m.get("numStartups").and_then(Value::as_u64))
        .unwrap_or(0);
    if num_startups > 1 {
        if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
            m.insert("sonnet45To46MigrationTimestamp".into(), json!(MigrationEnv::now_ms()));
            m
        }) {
            tracing::warn!(error = %e, "migrate_sonnet45_to_46: timestamp write failed");
        }
    }
    env.emit(
        telemetry::tengu::migration::SONNET45_TO_46_MIGRATION,
        std::collections::HashMap::from([
            ("from_model".to_string(), AnalyticsValue::String(model)),
            ("has_1m".to_string(), AnalyticsValue::Bool(has_1m)),
        ]),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::read_settings_map;
    use crate::test_support::temp_config;

    // test_env helper

    #[tokio::test]
    async fn tier_none_is_noop() {
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "claude-sonnet-4-5-20250929"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(
            read_settings_map(&sp).unwrap()["model"],
            serde_json::json!("claude-sonnet-4-5-20250929")
        );
    }

    #[tokio::test]
    async fn max_tier_rewrites_preserving_1m_and_gates_timestamp_on_startups() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"numStartups": 5}"#).unwrap();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "sonnet-4-5-20250929[1m]"}"#).unwrap();
        let mut env = test_env(&t);
        env.ctx.subscription_type = Some(SubscriptionType::Max);
        run(&env).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], serde_json::json!("sonnet[1m]"));
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m["sonnet45To46MigrationTimestamp"].is_i64());

        // fresh user (numStartups missing → 0) gets no timestamp
        let t2 = temp_config();
        let sp2 = settings_path(SettingsSource::User, &t2.home, &t2.project);
        std::fs::create_dir_all(sp2.parent().unwrap()).unwrap();
        std::fs::write(&sp2, r#"{"model": "claude-sonnet-4-5-20250929"}"#).unwrap();
        let mut env2 = test_env(&t2);
        env2.ctx.subscription_type = Some(SubscriptionType::Max);
        run(&env2).await;
        assert_eq!(read_settings_map(&sp2).unwrap()["model"], serde_json::json!("sonnet"));
        let m2 = crate::global_config::read_map(&t2.global).unwrap();
        assert!(m2.get("sonnet45To46MigrationTimestamp").is_none());
    }
}
```

- [ ] **Step 3: Implement `migrate_opus_to_opus1m.rs`** (with tests):

```rust
//! `migrateOpusToOpus1m.ts` — rewrite a pinned `opus` to `opus[1m]` for
//! merge-eligible users. `isOpus1mMergeEnabled` (`model.ts:314-332`) fails
//! closed on unknown tier — structurally always false in this port today
//! (tier `None`) ⇒ dormant; the body is ported for when tier persistence
//! lands.
//!
//! Stand-ins for unported helpers (doc'd, dormant path only):
//! - `getDefaultMainLoopModelSetting` (`model.ts:178-200`): Max/Team(≈Team
//!   Premium) → `opus[1m]` under merge (getDefaultOpusModel alias level),
//!   else the sonnet default — represented as the literal setting strings
//!   `"opus[1m]"` / `"sonnet"`.
//! - `parseUserSpecifiedModel` comparison: direct setting-string equality
//!   (alias-level), sufficient for the `opus[1m]` vs default comparison.

use crate::context::{env_var_truthy, MigrationEnv};
use crate::settings_update::{read_settings_map, settings_path, update_settings, SettingsSource};
use anthropic_oauth::limits::SubscriptionType;
use serde_json::{json, Value};

/// `isOpus1mMergeEnabled` port: false when 1M disabled by env, on Pro, off
/// first-party, or tier unknown (fail closed).
fn is_opus1m_merge_enabled(env: &MigrationEnv) -> bool {
    if env_var_truthy("CLAUDE_CODE_DISABLE_1M_CONTEXT") {
        return false;
    }
    match env.ctx.subscription_type {
        None => false, // fail closed (model.ts:328-330)
        Some(SubscriptionType::Pro) => false,
        Some(_) => env.ctx.first_party,
    }
}

/// `getDefaultMainLoopModelSetting` stand-in (dormant path; see module doc).
fn default_main_loop_model_setting(env: &MigrationEnv) -> &'static str {
    match env.ctx.subscription_type {
        Some(SubscriptionType::Max | SubscriptionType::Team) => "opus[1m]",
        _ => "sonnet",
    }
}

/// Run the migration.
pub async fn run(env: &MigrationEnv) {
    if !is_opus1m_merge_enabled(env) {
        return;
    }
    let sp = settings_path(SettingsSource::User, &env.claude_config_home, &env.project_dir);
    let model = read_settings_map(&sp)
        .ok()
        .and_then(|m| m.get("model").and_then(Value::as_str).map(String::from));
    if model.as_deref() != Some("opus") {
        return;
    }

    // modelToSet: undefined (delete) when opus[1m] IS the default, else set.
    let migrated = "opus[1m]";
    let update = if migrated == default_main_loop_model_setting(env) {
        ("model".to_string(), None)
    } else {
        ("model".to_string(), Some(json!(migrated)))
    };
    if let Err(e) = update_settings(&sp, vec![update]) {
        tracing::warn!(error = %e, "migrate_opus_to_opus1m: settings write failed");
        return;
    }
    env.emit(
        telemetry::tengu::migration::OPUS_TO_OPUS1M_MIGRATION,
        std::collections::HashMap::new(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_update::read_settings_map;
    use crate::test_support::{env_lock, temp_config};

    // test_env helper

    #[tokio::test]
    async fn tier_none_fails_closed() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_1M_CONTEXT");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "opus"}"#).unwrap();
        run(&test_env(&t)).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], serde_json::json!("opus"));
    }

    #[tokio::test]
    async fn max_tier_deletes_pinned_opus_when_default_matches() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_1M_CONTEXT");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "opus", "keep": 1}"#).unwrap();
        let mut env = test_env(&t);
        env.ctx.subscription_type = Some(SubscriptionType::Max);
        run(&env).await;
        let s = read_settings_map(&sp).unwrap();
        assert!(s.get("model").is_none()); // modelToSet === undefined → delete
        assert_eq!(s["keep"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn enterprise_tier_writes_opus1m() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_1M_CONTEXT");
        let t = temp_config();
        let sp = settings_path(SettingsSource::User, &t.home, &t.project);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        std::fs::write(&sp, r#"{"model": "opus"}"#).unwrap();
        let mut env = test_env(&t);
        env.ctx.subscription_type = Some(SubscriptionType::Enterprise);
        run(&env).await;
        assert_eq!(read_settings_map(&sp).unwrap()["model"], serde_json::json!("opus[1m]"));
    }
}
```

- [ ] **Step 4: Run `cargo test -p migrations` → PASS. Gate + commit** (`feat(migrations): subscriber-gated trio (resetProToOpus, sonnet45→46, opus→opus1m) fail-closed`).

---

### Task 10: runner + changelog migration

**Files:**
- Create: `migrations/src/runner.rs`, `migrations/src/changelog.rs`
- Modify: `migrations/src/lib.rs` (decls + re-export `pub use runner::{run_migrations, CURRENT_MIGRATION_VERSION}; pub use changelog::migrate_changelog_from_config; pub use context::{MigrationContext, MigrationEnv};`)

- [ ] **Step 1: Failing tests — `runner.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{env_lock, temp_config};
    use serde_json::json;

    // test_env helper

    #[tokio::test]
    async fn at_version_11_is_a_pure_readonly_noop() {
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"migrationVersion": 11, "replBridgeEnabled": true}"#,
        )
        .unwrap();
        let before = std::fs::read_to_string(&t.global).unwrap();
        run_migrations(&test_env(&t)).await;
        // version guard: nothing runs, nothing written (replBridge NOT renamed)
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), before);
    }

    #[tokio::test]
    async fn below_version_runs_set_and_bumps_to_11() {
        let _g = env_lock(); // legacy-opus migration reads env opt-out
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        std::fs::write(&t.global, r#"{"replBridgeEnabled": true}"#).unwrap();
        run_migrations(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert_eq!(m["migrationVersion"], json!(11));
        assert_eq!(m["remoteControlAtStartup"], json!(true));
        assert!(m.get("replBridgeEnabled").is_none());
        // run-once flags from the always-mark migrations
        assert_eq!(m["sonnet1m45MigrationComplete"], json!(true));
        assert_eq!(m["opusProMigrationComplete"], json!(true));
    }

    #[tokio::test]
    async fn second_run_is_noop_after_bump() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        run_migrations(&test_env(&t)).await;
        let after_first = std::fs::read_to_string(&t.global).unwrap();
        run_migrations(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), after_first);
    }

    #[tokio::test]
    async fn version_above_11_reruns_like_ts() {
        let _g = env_lock();
        std::env::remove_var("CLAUDE_CODE_DISABLE_LEGACY_MODEL_REMAP");
        let t = temp_config();
        std::fs::write(
            &t.global,
            r#"{"migrationVersion": 12, "replBridgeEnabled": true}"#,
        )
        .unwrap();
        run_migrations(&test_env(&t)).await;
        let m = crate::global_config::read_map(&t.global).unwrap();
        // TS guard is `!== CURRENT`, so 12 re-runs and lands on 11
        assert_eq!(m["migrationVersion"], json!(11));
        assert!(m.get("replBridgeEnabled").is_none());
    }

    #[tokio::test]
    async fn broken_global_config_skips_run_untouched() {
        let t = temp_config();
        std::fs::write(&t.global, "{ broken").unwrap();
        run_migrations(&test_env(&t)).await;
        assert_eq!(std::fs::read_to_string(&t.global).unwrap(), "{ broken");
    }
}
```

- [ ] **Step 2: Implement `runner.rs`:**

```rust
//! `runMigrations` (`main.tsx:323-353`) — version-guarded startup migration
//! set. Bump [`CURRENT_MIGRATION_VERSION`] when adding a sync migration so
//! existing users re-run the set.

use crate::context::MigrationEnv;
use crate::global_config;
use serde_json::{json, Value};

/// `CURRENT_MIGRATION_VERSION` (`main.tsx:325`).
pub const CURRENT_MIGRATION_VERSION: u64 = 11;

/// Run the sync migration set if `migrationVersion != 11`, then bump.
/// Mirrors the TS guard exactly (`!==`, so a downgrade re-runs too).
///
/// SAFETY CONTRACT: a broken/unreadable `~/.claude.json` skips the whole run
/// (no writes, no version bump — stricter than TS's defaults-fallback,
/// documented divergence); individual migration failures are swallowed
/// (warn-logged) and never abort startup; the version bump still happens.
/// The async changelog migration is NOT part of this fn — the caller spawns
/// [`crate::changelog::migrate_changelog_from_config`] fire-and-forget.
pub async fn run_migrations(env: &MigrationEnv) {
    let cfg = match global_config::read_map(&env.global_config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "run_migrations: global config unreadable; skipping this startup");
            return;
        }
    };
    let version = cfg.get("migrationVersion").and_then(Value::as_u64);
    if version == Some(CURRENT_MIGRATION_VERSION) {
        return;
    }

    // TS execution order (main.tsx:328-336). migrateFennecToOpus (ant-only
    // dead code) and resetAutoModeOptInForDefaultOffer (TRANSCRIPT_CLASSIFIER
    // gate) are intentionally absent — see lib.rs module docs.
    crate::migrate_auto_updates::run(env).await;
    crate::migrate_bypass_permissions::run(env).await;
    crate::migrate_mcp_servers::run(env).await;
    crate::migrate_reset_pro_to_opus::run(env).await;
    crate::migrate_sonnet1m_to_sonnet45::run(env).await;
    crate::migrate_legacy_opus::run(env).await;
    crate::migrate_sonnet45_to_46::run(env).await;
    crate::migrate_opus_to_opus1m::run(env).await;
    crate::migrate_repl_bridge::run(env).await;

    if let Err(e) = global_config::save_map(&env.global_config_path, |mut m| {
        if m.get("migrationVersion").and_then(Value::as_u64) == Some(CURRENT_MIGRATION_VERSION) {
            return m; // TS: prev.migrationVersion === CURRENT ? prev : …
        }
        m.insert("migrationVersion".into(), json!(CURRENT_MIGRATION_VERSION));
        m
    }) {
        tracing::warn!(error = %e, "run_migrations: version bump failed");
    }
}
```

- [ ] **Step 3: Failing tests — `changelog.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_config;

    // test_env helper

    #[tokio::test]
    async fn moves_cached_changelog_to_file_and_drops_key() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"cachedChangelog": "# v1", "keep": 1}"#).unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        let cache = t.home.join("cache").join("changelog.md");
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "# v1");
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("cachedChangelog").is_none());
        assert_eq!(m["keep"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn existing_cache_file_is_not_overwritten_but_key_still_dropped() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"cachedChangelog": "# old"}"#).unwrap();
        let cache_dir = t.home.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("changelog.md"), "# newer").unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        assert_eq!(
            std::fs::read_to_string(cache_dir.join("changelog.md")).unwrap(),
            "# newer"
        );
        let m = crate::global_config::read_map(&t.global).unwrap();
        assert!(m.get("cachedChangelog").is_none());
    }

    #[tokio::test]
    async fn no_key_is_noop() {
        let t = temp_config();
        std::fs::write(&t.global, r#"{"a": 1}"#).unwrap();
        migrate_changelog_from_config(&test_env(&t)).await;
        assert!(!t.home.join("cache").join("changelog.md").exists());
    }
}
```

- [ ] **Step 4: Implement `changelog.rs`:**

```rust
//! `migrateChangelogFromConfig` (`releaseNotes.ts:55-76`) — move the
//! deprecated `cachedChangelog` config field to
//! `<claude-config-home>/cache/changelog.md`. Fire-and-forget at startup
//! (the caller `tokio::spawn`s this); errors are silent (TS `.catch(() => {})`),
//! retried next startup.

use crate::context::MigrationEnv;
use crate::global_config;
use serde_json::Value;

/// Run the async changelog migration.
pub async fn migrate_changelog_from_config(env: &MigrationEnv) {
    let Ok(cfg) = global_config::read_map(&env.global_config_path) else {
        return;
    };
    let Some(Value::String(changelog)) = cfg.get("cachedChangelog").cloned() else {
        return;
    };

    let cache_dir = env.claude_config_home.join("cache");
    let cache_path = cache_dir.join("changelog.md");
    if tokio::fs::create_dir_all(&cache_dir).await.is_ok() {
        // `wx` flag parity (`releaseNotes.ts:66`): write only if the file
        // doesn't exist; an existing file (or any write error) is silently fine.
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&cache_path)
            .await
        {
            Ok(mut f) => {
                use tokio::io::AsyncWriteExt;
                let _ = f.write_all(changelog.as_bytes()).await;
            }
            Err(_) => {} // exists already (or unwritable) — silently fine
        }
    }

    // Remove the deprecated field regardless (TS does this after the try).
    let _ = global_config::save_map(&env.global_config_path, |mut m| {
        m.remove("cachedChangelog");
        m
    });
}
```

(If clippy flags the empty `Err(_) => {}` arm, `if let Ok(mut f) = …` is equivalent.)

- [ ] **Step 5: Run `cargo test -p migrations` → PASS. Gate + commit** (`feat(migrations): runMigrations runner (v11 guard) + async changelog migration`).

---

### Task 11: engine SettingsJson — remove `deny_unknown_fields`

**Files:**
- Modify: `lingxi-code/engine/src/settings/schema.rs:73` (the `#[serde(...)]` attr on `SettingsJson`)
- Modify: `lingxi-code/engine/src/settings/loader.rs` (tests at ~line 101)

- [ ] **Step 1: Failing test FIRST.** In `loader.rs` tests, REPLACE `returns_schema_violation_for_unknown_field` (which asserts a ParseError for unknown keys) with:

```rust
    #[test]
    fn tolerates_unknown_fields_like_zod_strip() {
        // claude-code's zod SettingsSchema().safeParse STRIPS unknown keys
        // (non-strict object schema, settings.ts:219) — strictness was a
        // parity divergence that made e.g. /effort's persisted `effortLevel`
        // silently kill the whole settings load.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"model": "opus", "effortLevel": "high", "skipDangerousModePermissionPrompt": true, "env": {"DISABLE_AUTOUPDATER": "1"}, "futureKey": [1,2]}"#,
        )
        .unwrap();
        let settings = read_settings_file(&path)
            .expect("must load")
            .expect("must be Some");
        assert_eq!(settings.model.as_deref(), Some("opus"));
    }
```

Run `cargo test -p engine settings` → the new test FAILS (ParseError) while `deny_unknown_fields` is still present.

- [ ] **Step 2: Implement.** In `schema.rs`, change

```rust
#[serde(rename_all = "camelCase", deny_unknown_fields)]
```

to

```rust
#[serde(rename_all = "camelCase")]
```

and update the struct's doc comment: replace the strictness rationale with: unknown keys are tolerated-and-ignored, matching claude-code's zod `safeParse` (non-strict ⇒ strip; `settings.ts:219`); known fields keep their typed parses; this also un-breaks settings files carrying keys written by ConfigTool//effort/the migration subsystem. Sweep `schema.rs` + `loader.rs` + `engine/src/settings/mod.rs` for now-stale comments that say `deny_unknown_fields` REJECTS (schema.rs:104-113 `permissions` field comment, schema.rs:149, schema.rs:207/223 history notes, loader.rs:16 error-doc) — reword each to reflect tolerance (the `permissions`/`output_style` fields stay typed for ACCESS, no longer load-bearing for acceptance).

- [ ] **Step 3: Run.** `cargo test -p engine` → ALL PASS (the old rejection test was replaced; check no OTHER test asserts unknown-key rejection: `grep -rn "unknown" lingxi-code/engine/src/settings/`).

- [ ] **Step 4: Gate + commit** (`fix(engine): SettingsJson tolerates unknown fields (zod-strip parity; un-breaks effortLevel et al)`). Clippy: `cargo clippy -p engine --all-targets --no-deps -- -D warnings`.

---

### Task 12: CLI wiring + final gates

**Files:**
- Modify: `lingxi-code/apps/cli/Cargo.toml` (add `migrations = { path = "../../migrations" }` to `[dependencies]`)
- Modify: `lingxi-code/apps/cli/src/lib.rs` (insert after the `startup_deprecation_notice` block, ~line 155)

- [ ] **Step 1: Wire** — insert directly after the `if let Some(notice) = startup_deprecation_notice(&parsed) { eprintln!("{notice}"); }` block:

```rust
    // Config migrations (`main.tsx runMigrations`, CURRENT_MIGRATION_VERSION
    // = 11) — the same pre-REPL point as the deprecation notice above, common
    // to Print/Tui/StdioRepl. On a machine where real claude-code already
    // migrated `~/.claude.json` to v11 this is a read-only no-op (version
    // guard). `bus: None`: no pre-boot telemetry bus substrate exists (same
    // as the deprecation notice); the 9 event names are registered for when
    // one does. Tier is structurally None (no keychain subscriptionType) —
    // the subscriber-gated migrations take their faithful fail-closed
    // branches; the CLI deliberately does not read the keychain pre-boot
    // (avoids a second keychain prompt).
    if let (Some(global_config_path), Some(claude_home)) = (
        migrations::global_config::global_config_path(),
        migrations::global_config::claude_config_home(),
    ) {
        let project_dir =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let env = migrations::MigrationEnv {
            global_config_path,
            claude_config_home: claude_home,
            project_dir,
            ctx: migrations::MigrationContext::from_env(),
            bus: None,
        };
        migrations::run_migrations(&env).await;
        // Async fire-and-forget (TS `.catch(() => {})`): retried next startup.
        tokio::spawn(async move {
            migrations::migrate_changelog_from_config(&env).await;
        });
    }
```

(If `parsed`/cwd handling in `run_cli` already exposes an original-cwd value — check `cwd.rs` usage nearby — prefer it over `std::env::current_dir()` and say so in the comment.)

- [ ] **Step 2: Build + targeted tests:**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo build -p cli
cargo test -p migrations
cargo clippy -p cli -p migrations --all-targets --no-deps -- -D warnings
```

- [ ] **Step 3: FULL GATE RITUAL:**

```bash
cargo test -p engine
cargo test -p telemetry
cargo test -p orchestrator diagnostics
cargo test -p tui vim
cargo test -p tui --test behavior_palette
cargo test -p test-harness --test parity_tengu_events
cargo test --workspace --no-run          # struct-trap compile (~2-3 min)
cargo build -p engine-desktop
cargo build -p engine-mobile        # package name verified: apps/engine-mobile
cargo tree -p engine-mobile | grep -c "^migrations\| migrations " # expect 0
```

Expected: all green; engine-mobile pulls ZERO `migrations`.

- [ ] **Step 4: Live smoke (safe on this machine).** `~/.claude.json` here is already at migrationVersion 11 (real claude-code) — verify read-only no-op:

```bash
shasum ~/.claude.json
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code && cargo run -p cli -- --help >/dev/null 2>&1 || true
shasum ~/.claude.json   # MUST be identical
```

(If `--help` exits before `run_cli`'s migration point, use a trivial `--print "hi"`-style invocation that reaches the pre-REPL block but doesn't need an API key — check `argv.rs`; an invocation erroring AFTER the migration point is fine for the hash check. If the version is NOT 11 on this machine, skip the live smoke and note it.)

- [ ] **Step 5: Commit** (`feat(cli): run config migrations at startup (pre-REPL)`).

---

## Final verification (whole-branch)

1. `cargo test -p migrations` — full crate green (expect ~40 tests).
2. The Task 12 gate ritual, all green.
3. `git diff main -- lingxi-code/traits lingxi-code/protocol` — MUST be empty (frozen surfaces untouched).
4. Re-read the spec's "Safety invariants" section against the code: preserve-unknown-keys (Task 2 test), atomic writes (write_atomic), zero-write no-op (runner test `at_version_11_is_a_pure_readonly_noop`), broken-JSON never overwritten (Tasks 2/3/10 tests), failures never abort startup (every migration swallows).
5. Update memory (`parity-1to1-effort.md`: config-migration remainder item → DONE; new entry or update for this batch).
