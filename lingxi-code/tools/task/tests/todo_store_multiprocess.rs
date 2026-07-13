//! Multi-process regression for the task store's cross-process lock
//! (parity 2.1.207 P1-13).
//!
//! claude-code serialises concurrent processes on one task list with an on-disk
//! `proper-lockfile`. LingXi's earlier in-process-only mutex gave two OS
//! processes (a `--bg` worker + an `--resume` attach, or an env-shared list id)
//! no protection, so `create` produced duplicate ids and `update` lost writes.
//! This test re-execs the test binary as N worker processes that hammer one
//! shared tasks dir and asserts the store stays race-free.
//!
//! The test double-dispatches on `LINGXI_TODO_MP_ROLE`: the parent run spawns
//! children with that var set; a child performs its worker role and
//! `process::exit`s before the harness proceeds.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map};
use tool_task::todo_store::{TodoStore, TodoTask};

const ROLE_ENV: &str = "LINGXI_TODO_MP_ROLE";
const DIR_ENV: &str = "LINGXI_TODO_MP_DIR";
/// Worker processes.
const N: usize = 4;
/// Tasks each process creates (create phase) / increments (update phase).
const PER_PROC: usize = 25;
/// The single shared task every update-worker races on.
const SHARED_ID: &str = "1";

#[test]
fn multiprocess_create_and_update_is_race_free() {
    // ── child mode ──────────────────────────────────────────────────────────
    if let Ok(role) = std::env::var(ROLE_ENV) {
        let dir = PathBuf::from(std::env::var(DIR_ENV).expect("child needs dir"));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("child runtime");
        let code = rt.block_on(async move {
            match role.as_str() {
                "create" => child_create(&dir).await,
                "update" => child_update(&dir).await,
                other => {
                    eprintln!("unknown role {other}");
                    1
                }
            }
        });
        std::process::exit(code);
    }

    // ── parent mode ─────────────────────────────────────────────────────────
    let dir = unique_dir();
    std::fs::create_dir_all(&dir).expect("mk shared dir");

    // Phase 1: N processes concurrently create PER_PROC tasks each.
    run_workers("create", &dir);

    let total = N * PER_PROC;
    let mut ids: Vec<i64> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".json") {
            // Every task file must be complete, parseable JSON (no torn writes).
            let content = std::fs::read_to_string(entry.path()).expect("read task");
            let _: TodoTask = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("task {name} is not valid JSON: {e}\n{content}"));
            ids.push(stem.parse::<i64>().expect("numeric id"));
        }
    }
    ids.sort_unstable();
    // Unique, gap-free ids 1..=total prove the list lock prevented duplicates.
    assert_eq!(
        ids.len(),
        total,
        "expected {total} distinct task files, got {}: {ids:?}",
        ids.len()
    );
    assert_eq!(
        ids,
        (1..=total as i64).collect::<Vec<_>>(),
        "ids must be the unique sequence 1..={total} (duplicates collapse files)"
    );

    // Phase 2: N processes each apply PER_PROC read-modify-write increments to
    // the SAME task. With the per-task file lock every increment lands.
    run_workers("update", &dir);

    let shared = read_task(&dir, SHARED_ID);
    let counter = shared
        .metadata
        .get("counter")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    assert_eq!(
        counter, total as i64,
        "lost update: {N} procs * {PER_PROC} increments should total {total}, got {counter}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

async fn child_create(dir: &Path) -> i32 {
    let store = TodoStore::in_dir(dir.to_path_buf());
    for i in 0..PER_PROC {
        let task = TodoTask::new(
            format!("proc-{}-{i}", std::process::id()),
            "d".into(),
            None,
            Map::new(),
        );
        if store.create(task).await.is_err() {
            return 1;
        }
    }
    0
}

async fn child_update(dir: &Path) -> i32 {
    let store = TodoStore::in_dir(dir.to_path_buf());
    for _ in 0..PER_PROC {
        let updated = store
            .update(SHARED_ID, |t| {
                let c = t
                    .metadata
                    .get("counter")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                t.metadata.insert("counter".into(), json!(c + 1));
            })
            .await;
        if updated.is_none() {
            return 1;
        }
    }
    0
}

fn read_task(dir: &Path, id: &str) -> TodoTask {
    let content = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("read shared task");
    serde_json::from_str(&content).expect("shared task JSON")
}

/// Spawn N copies of this test binary as `role` workers against `dir` and wait
/// for each to exit successfully.
fn run_workers(role: &str, dir: &Path) {
    let exe = std::env::current_exe().expect("current_exe");
    let children: Vec<_> = (0..N)
        .map(|_| {
            Command::new(&exe)
                .arg("multiprocess_create_and_update_is_race_free")
                .arg("--exact")
                .arg("--nocapture")
                .env(ROLE_ENV, role)
                .env(DIR_ENV, dir)
                // Don't let the harness's own env leak a stale role/dir.
                .spawn()
                .expect("spawn worker")
        })
        .collect();
    for mut child in children {
        let status = child.wait().expect("await worker");
        assert!(status.success(), "{role} worker failed: {status}");
    }
}

fn unique_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "lingxi-todo-mp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    dir
}
