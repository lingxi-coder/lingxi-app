//! Tick-loop-free, single-shot cron firing core.
//!
//! The desktop [`crate::scheduler::CronScheduler`] owns a long-lived one-second
//! tick loop and fires due jobs as `TaskType::Dream` subagents through a
//! [`tasks::TaskRegistry`]. Mobile hosts cannot run that loop — a backgrounded
//! Android process is killed, and the mobile engine binds no `TaskRegistry` /
//! subagent spawner. Instead, an OS scheduler (Android `AlarmManager`) wakes the
//! app and asks the engine to evaluate the persisted tasks file ONCE and fire
//! whatever is due.
//!
//! This module is that single-shot operation. It REUSES the exact pure
//! due-detection + file-bookkeeping helpers the desktop tick loop uses
//! (`is_job_due`, `finalize_fired_job`, `tasks_file_with_last_fired`,
//! `tasks_file_without`, `is_recurring_task_aged` from [`crate::scheduler`], and
//! [`crate::schedule::CronExpression::next_match_after`]), so the semantics —
//! epoch-millisecond timestamps, missed-run catch-up-once, one-shot auto-delete,
//! recurring `lastFiredAt` persistence, deterministic jitter/cache lead, and
//! seven-day recurring auto-expiry — are 1:1 with desktop. The single-shot path
//! takes no per-job A9 firing lock (a phone is single process and the foreground
//! service is the only firer); every scheduled-task file mutation still takes
//! the shared root-confined cross-process lock.
//!
//! Firing itself is abstracted behind [`CronJobFirer`] (`fire(prompt) -> result`)
//! so the same core serves desktop (a Dream subagent) and mobile (a fresh
//! orchestrator turn).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use traits::{Clock, FileSystem};

use crate::schedule::parse_cron;
use crate::scheduler::{
    finalize_fired_job, is_job_due, is_recurring_task_aged, next_fire_time,
    tasks_file_with_last_fired, tasks_file_without, CronTaskDef, DEFAULT_RECURRING_MAX_AGE,
};
use crate::tasks_file::parse_tasks;

/// Process-global serialization for ALL `scheduled_tasks.json` read-modify-write
/// mutations. The mobile single-shot firing path took no A9 cross-process lock
/// (one app process), but the firing pass and the management-UI `cron_create` /
/// `cron_delete` FFI run on DIFFERENT engine handles / tokio runtimes in the SAME
/// process, so without this an in-process lost-update could clobber a new/deleted
/// job or a `lastFiredAt` write-back. Held only across the brief RMW — NEVER
/// during a (≤180s) fire — so a UI edit never blocks for the length of a turn.
static CRON_FILE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Acquire the process-global cron-file lock ([`CRON_FILE_LOCK`]). FFI writers
/// (`cron_create` / `cron_delete`) hold this across their own read-modify-write so
/// they cannot interleave with a concurrent firing pass's write-back.
pub async fn lock_cron_file() -> tokio::sync::MutexGuard<'static, ()> {
    CRON_FILE_LOCK.lock().await
}

/// Caller-supplied seam: run one due job's `prompt` to completion and return a
/// textual result to surface (e.g. an Android notification body).
///
/// Generic over "fire(prompt) -> result" so the firing core is host-agnostic:
/// desktop adapts it to a Dream subagent, mobile to a fresh orchestrator turn.
#[async_trait]
pub trait CronJobFirer: Send + Sync {
    /// Fire `prompt` for job `id`. `Ok(result_text)` on success; `Err(message)`
    /// on failure (surfaced as [`FireStatus::Failed`]).
    async fn fire(&self, id: &str, prompt: &str) -> Result<String, String>;
}

/// Terminal status of one fired job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireStatus {
    /// The job's turn completed.
    Ok,
    /// The job's turn failed; carries a log-safe message.
    Failed(String),
}

/// One fired-job record returned by [`run_due_jobs`]. Mobile lifts these to FFI
/// DTOs for the notification surface.
#[derive(Debug, Clone)]
pub struct FiredJob {
    /// The job id that fired.
    pub id: String,
    /// The prompt that was run.
    pub prompt: String,
    /// The final assistant text, if the firer produced any.
    pub result_text: Option<String>,
    /// Terminal status.
    pub status: FireStatus,
}

/// Evaluate the persisted tasks file ONCE and fire every due job via `firer`,
/// applying the EXACT desktop post-fire bookkeeping (recurring `lastFiredAt`
/// write-back, one-shot auto-delete, aged-recurring auto-expiry). Returns one
/// [`FiredJob`] per job that fired this call.
///
/// `recurring_max_age` mirrors [`crate::scheduler::CronScheduler`]'s field
/// (`Some(`[`DEFAULT_RECURRING_MAX_AGE`]`)` on both hosts; `None` disables
/// expiry). A missing / unparseable tasks file fires nothing. Unlike the desktop
/// tick loop, firing takes no per-job lock; deterministic fire-time jitter is
/// already included by the shared due predicate. Persistence mutations remain
/// serialized in-process and cross-process.
pub async fn run_due_jobs(
    tasks_file: &Path,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    firer: &dyn CronJobFirer,
    recurring_max_age: Option<Duration>,
) -> Vec<FiredJob> {
    let now = clock.now();
    let Some(project_root) = crate::tasks_file::project_root_from_tasks_path(tasks_file) else {
        return Vec::new();
    };

    // Load the persisted (durable) jobs into a transient in-memory map, mirroring
    // `CronScheduler::load_persisted` (epoch-ms timestamps; invalid cron skipped).
    let Ok(body) = crate::tasks_file::read_tasks_body(fs.as_ref(), project_root).await else {
        return Vec::new(); // file absent → nothing to fire
    };
    let mut tasks: HashMap<String, CronTaskDef> = HashMap::new();
    for t in parse_tasks(&body).tasks {
        let schedule = match parse_cron(&t.cron) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("cron: skipping job {} with invalid schedule: {e}", t.id);
                continue;
            }
        };
        let created_at = if t.created_at > 0 {
            SystemTime::UNIX_EPOCH + Duration::from_millis(t.created_at)
        } else {
            now
        };
        let last_run = t
            .last_fired_at
            .filter(|ms| *ms > 0)
            .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
        tasks.insert(
            t.id.clone(),
            CronTaskDef {
                id: t.id,
                schedule,
                prompt: t.prompt,
                agent_type: None,
                last_run,
                enabled: true,
                created_at,
                recurring: t.recurring.unwrap_or(false),
            },
        );
    }

    // (1) Due-detection with missed-run catch-up (`is_job_due`). Age is not
    // checked here: Claude Code lets an aged recurring job keep waiting until
    // its next due time, then gives it one final fire before deletion.
    let due_ids: Vec<String> = tasks
        .values()
        .filter(|t| is_job_due(t, now))
        .map(|t| t.id.clone())
        .collect();

    // (2) Fire each due job, then apply post-fire bookkeeping identically to
    //     `CronScheduler::tick` (one-shot auto-delete vs recurring lastFiredAt).
    let now_ms = system_time_to_epoch_ms(now);
    let mut fired = Vec::with_capacity(due_ids.len());
    for id in due_ids {
        let prompt = tasks.get(&id).map(|t| t.prompt.clone()).unwrap_or_default();
        let (status, result_text) = match firer.fire(&id, &prompt).await {
            Ok(text) => (FireStatus::Ok, Some(text)),
            Err(message) => (FireStatus::Failed(message), None),
        };

        let expires_after_fire = tasks.get(&id).is_some_and(|task| {
            is_recurring_task_aged(now, task.created_at, task.recurring, recurring_max_age)
        });
        let remove_after_fire = finalize_fired_job(&mut tasks, &id, now) || expires_after_fire;
        if expires_after_fire {
            tasks.remove(&id);
        }
        if remove_after_fire {
            remove_task_from_file(fs.as_ref(), project_root, &id).await;
            if expires_after_fire {
                tracing::info!(
                    event = "tengu_scheduled_task_expired",
                    cron_id = %id,
                    "cron job fired its final run after exceeding the recurring max age"
                );
            } else {
                tracing::info!(cron_id = %id, "one-shot cron job fired and auto-deleted");
            }
        } else {
            set_last_fired_in_file(fs.as_ref(), project_root, &id, now_ms).await;
        }

        fired.push(FiredJob {
            id,
            prompt,
            result_text,
            status,
        });
    }
    fired
}

/// The earliest next fire across all persisted ENABLED jobs, as epoch
/// **milliseconds**, or `None` if there are no jobs / none ever fire again. The
/// host arms its next OS alarm at this instant. The anchor for each job is
/// `lastFiredAt ?? createdAt`; the returned instant includes the same
/// deterministic recurring/one-shot jitter and cache-lead exception as the
/// desktop scheduler. An overdue job yields a past instant, so the host fires
/// immediately and re-arms.
pub async fn next_fire_epoch_ms(
    tasks_file: &Path,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
) -> Option<u64> {
    let now = clock.now();
    let project_root = crate::tasks_file::project_root_from_tasks_path(tasks_file)?;
    let body = crate::tasks_file::read_tasks_body(fs.as_ref(), project_root)
        .await
        .ok()?;

    let mut earliest: Option<SystemTime> = None;
    for t in parse_tasks(&body).tasks {
        let Ok(schedule) = parse_cron(&t.cron) else {
            continue;
        };
        let created_at = if t.created_at > 0 {
            SystemTime::UNIX_EPOCH + Duration::from_millis(t.created_at)
        } else {
            now
        };
        let last_run = t
            .last_fired_at
            .filter(|ms| *ms > 0)
            .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
        let task = CronTaskDef {
            id: t.id,
            schedule,
            prompt: t.prompt,
            agent_type: None,
            last_run,
            enabled: true,
            created_at,
            recurring: t.recurring.unwrap_or(false),
        };
        if let Some(next) = next_fire_time(&task) {
            earliest = Some(match earliest {
                Some(e) if e <= next => e,
                _ => next,
            });
        }
    }
    earliest.map(system_time_to_epoch_ms)
}

/// `SystemTime` → epoch **milliseconds** (the on-disk / alarm unit). A
/// pre-epoch instant clamps to 0.
#[must_use]
fn system_time_to_epoch_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Remove `id` from the single tasks file (read-modify-write via `fs`). A missing
/// file / id is a no-op. Mirrors `CronScheduler::remove_task_from_file`.
async fn remove_task_from_file(fs: &dyn FileSystem, project_root: &Path, id: &str) {
    let _guard = CRON_FILE_LOCK.lock().await;
    let Ok(_file_guard) = crate::tasks_file::lock_scheduled_tasks(fs, project_root).await else {
        return;
    };
    if let Ok(body) = crate::tasks_file::read_tasks_body(fs, project_root).await {
        if let Some(updated) = tasks_file_without(&body, id) {
            let _ = crate::tasks_file::write_tasks_body(fs, project_root, &updated).await;
        }
    }
}

/// Set `lastFiredAt` (epoch ms) on `id` in the single tasks file (read-modify-
/// write via `fs`). A missing file / id is a no-op. Mirrors
/// `CronScheduler::set_last_fired_in_file`.
async fn set_last_fired_in_file(
    fs: &dyn FileSystem,
    project_root: &Path,
    id: &str,
    last_fired_at_ms: u64,
) {
    let _guard = CRON_FILE_LOCK.lock().await;
    let Ok(_file_guard) = crate::tasks_file::lock_scheduled_tasks(fs, project_root).await else {
        return;
    };
    if let Ok(body) = crate::tasks_file::read_tasks_body(fs, project_root).await {
        if let Some(updated) = tasks_file_with_last_fired(&body, id, last_fired_at_ms) {
            let _ = crate::tasks_file::write_tasks_body(fs, project_root, &updated).await;
        }
    }
}

/// Convenience: the desktop-equal recurring expiry age both hosts pass.
#[must_use]
pub const fn default_recurring_max_age() -> Duration {
    DEFAULT_RECURRING_MAX_AGE
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex as TokioMutex;
    use traits::filesystem::{FileContent, FileEvent, FlockGuard, FsError};

    // ---- Minimal in-memory FileSystem (only read/write/delete are exercised) ----
    struct MemFs {
        files: TokioMutex<HashMap<String, String>>,
    }
    struct MemFlockGuard(String);
    impl FlockGuard for MemFlockGuard {
        fn path(&self) -> &str {
            &self.0
        }
    }
    impl MemFs {
        fn with(path: &str, body: &str) -> Arc<Self> {
            let mut m = HashMap::new();
            m.insert(path.to_string(), body.to_string());
            Arc::new(Self {
                files: TokioMutex::new(m),
            })
        }
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                files: TokioMutex::new(HashMap::new()),
            })
        }
        async fn get(&self, path: &str) -> Option<String> {
            self.files.lock().await.get(path).cloned()
        }
    }
    #[async_trait]
    impl FileSystem for MemFs {
        async fn read_file(
            &self,
            path: &str,
            _offset: Option<u64>,
            _limit: Option<u64>,
        ) -> Result<FileContent, FsError> {
            match self.files.lock().await.get(path) {
                Some(content) => {
                    let total_lines = content.lines().count() as u64;
                    Ok(FileContent {
                        content: content.clone(),
                        truncated: false,
                        total_lines,
                    })
                }
                None => Err(FsError::NotFound(path.to_string())),
            }
        }
        async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .insert(path.to_string(), content.to_string());
            Ok(())
        }
        fn is_within_workspace(&self, _: &str) -> bool {
            true
        }
        async fn watch(
            &self,
            _: &str,
        ) -> Result<Pin<Box<dyn Stream<Item = FileEvent> + Send>>, FsError> {
            Err(FsError::Io("unsupported".into()))
        }
        async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
            self.files
                .lock()
                .await
                .entry(path.to_string())
                .or_default()
                .push_str(content);
            Ok(())
        }
        async fn truncate(&self, _: &str, _: u64) -> Result<(), FsError> {
            Ok(())
        }
        async fn file_mtime(&self, _: &str) -> Result<SystemTime, FsError> {
            Ok(SystemTime::UNIX_EPOCH)
        }
        async fn file_size(&self, path: &str) -> Result<u64, FsError> {
            Ok(self
                .files
                .lock()
                .await
                .get(path)
                .map_or(0, |s| s.len() as u64))
        }
        async fn delete_file(&self, path: &str) -> Result<(), FsError> {
            self.files.lock().await.remove(path);
            Ok(())
        }
        async fn symlink(&self, _: &str, _: &str) -> Result<(), FsError> {
            Ok(())
        }
        async fn flock_exclusive(&self, _: &str) -> Result<Box<dyn FlockGuard>, FsError> {
            Err(FsError::Io("unsupported".into()))
        }
        async fn flock_exclusive_rooted(
            &self,
            root: &Path,
            relative: &Path,
        ) -> Result<Box<dyn FlockGuard>, FsError> {
            Ok(Box::new(MemFlockGuard(
                root.join(relative).display().to_string(),
            )))
        }
        async fn fsync(&self, _: &str) -> Result<(), FsError> {
            Ok(())
        }
    }

    // ---- Fixed clock (local, to avoid a test-harness dep) ----
    struct FixedClock(SystemTime);
    impl FixedClock {
        fn at_secs(s: u64) -> Arc<Self> {
            Arc::new(Self(SystemTime::UNIX_EPOCH + Duration::from_secs(s)))
        }
    }
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    // ---- Recording firer ----
    struct RecordingFirer {
        canned: String,
        calls: StdMutex<Vec<(String, String)>>,
    }
    impl RecordingFirer {
        fn new(canned: &str) -> Self {
            Self {
                canned: canned.to_string(),
                calls: StdMutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl CronJobFirer for RecordingFirer {
        async fn fire(&self, id: &str, prompt: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((id.to_string(), prompt.to_string()));
            Ok(self.canned.clone())
        }
    }

    const NOW: u64 = 1_700_000_000; // not on a minute boundary (…020 seconds)
    const PATH: &str = "/proj/.lingxi/scheduled_tasks.json";

    fn file_with(tasks_json: &str) -> Arc<MemFs> {
        MemFs::with(PATH, tasks_json)
    }

    #[tokio::test]
    async fn fires_due_recurring_job_and_writes_last_fired() {
        // Recurring `* * * * *`, created 2 minutes ago, never fired → due now.
        let created_ms = (NOW - 120) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"d11111111","cron":"* * * * *","prompt":"hello","createdAt":{created_ms},"recurring":true}}]}}"#
        );
        let fs = file_with(&body);
        let firer = RecordingFirer::new("done");

        let fired = run_due_jobs(
            Path::new(PATH),
            fs.clone(),
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;

        assert_eq!(fired.len(), 1, "the due recurring job fires");
        assert_eq!(fired[0].id, "d11111111");
        assert_eq!(fired[0].prompt, "hello");
        assert_eq!(fired[0].result_text.as_deref(), Some("done"));
        assert_eq!(fired[0].status, FireStatus::Ok);
        assert_eq!(firer.calls(), vec![("d11111111".into(), "hello".into())]);

        // lastFiredAt (epoch ms) was written back; the job is still present.
        let after = parse_tasks(&fs.get(PATH).await.unwrap());
        assert_eq!(after.tasks.len(), 1);
        assert_eq!(after.tasks[0].last_fired_at, Some(NOW * 1000));
    }

    #[tokio::test]
    async fn one_shot_job_is_deleted_after_firing() {
        // A one-shot (recurring:false) job that is due → fires once, then removed.
        let created_ms = (NOW - 120) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"d22222222","cron":"* * * * *","prompt":"once","createdAt":{created_ms}}}]}}"#
        );
        let fs = file_with(&body);
        let firer = RecordingFirer::new("ok");

        let fired = run_due_jobs(
            Path::new(PATH),
            fs.clone(),
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;

        assert_eq!(fired.len(), 1);
        assert_eq!(firer.calls().len(), 1);
        // The one-shot was auto-deleted from the file.
        let after = parse_tasks(&fs.get(PATH).await.unwrap());
        assert!(after.tasks.is_empty(), "one-shot job removed after firing");
    }

    #[tokio::test]
    async fn job_not_yet_due_does_not_fire() {
        // Recurring `* * * * *` fired THIS minute → next run is next minute → not due.
        let body = format!(
            r#"{{"tasks":[{{"id":"d33333333","cron":"* * * * *","prompt":"p","createdAt":{c},"recurring":true,"lastFiredAt":{f}}}]}}"#,
            c = (NOW - 600) * 1000,
            f = NOW * 1000,
        );
        let fs = file_with(&body);
        let firer = RecordingFirer::new("x");

        let fired = run_due_jobs(
            Path::new(PATH),
            fs.clone(),
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;

        assert!(
            fired.is_empty(),
            "a job already fired this minute is not due"
        );
        assert!(firer.calls().is_empty());
    }

    #[tokio::test]
    async fn aged_due_recurring_job_fires_final_run_then_is_deleted() {
        let created_ms = (NOW - 8 * 24 * 60 * 60) * 1000;
        let last_fired_ms = (NOW - 120) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"00000000","cron":"* * * * *","prompt":"final","createdAt":{created_ms},"recurring":true,"lastFiredAt":{last_fired_ms}}}]}}"#
        );
        let fs = file_with(&body);
        let firer = RecordingFirer::new("done");

        let fired = run_due_jobs(
            Path::new(PATH),
            fs.clone(),
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;

        assert_eq!(fired.len(), 1, "an aged due job gets one final fire");
        assert_eq!(firer.calls(), vec![("00000000".into(), "final".into())]);
        assert!(
            parse_tasks(&fs.get(PATH).await.unwrap()).tasks.is_empty(),
            "the final fire removes the aged recurring descriptor"
        );
    }

    #[tokio::test]
    async fn aged_recurring_job_not_yet_due_is_retained() {
        let created_ms = (NOW - 8 * 24 * 60 * 60) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"00000000","cron":"* * * * *","prompt":"later","createdAt":{created_ms},"recurring":true,"lastFiredAt":{last}}}]}}"#,
            last = NOW * 1000,
        );
        let fs = file_with(&body);
        let firer = RecordingFirer::new("unused");

        let fired = run_due_jobs(
            Path::new(PATH),
            fs.clone(),
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;

        assert!(fired.is_empty());
        assert!(firer.calls().is_empty());
        assert_eq!(
            parse_tasks(&fs.get(PATH).await.unwrap()).tasks.len(),
            1,
            "age alone must not delete a recurring job before its due time"
        );
    }

    #[tokio::test]
    async fn missing_file_fires_nothing() {
        let fs = MemFs::empty();
        let firer = RecordingFirer::new("x");
        let fired = run_due_jobs(
            Path::new(PATH),
            fs,
            FixedClock::at_secs(NOW),
            &firer,
            Some(default_recurring_max_age()),
        )
        .await;
        assert!(fired.is_empty());
        assert!(firer.calls().is_empty());
    }

    #[tokio::test]
    async fn next_fire_epoch_ms_returns_earliest_in_ms() {
        // Two `* * * * *` jobs created now → both next-fire at the next minute
        // boundary; the earliest is that boundary, strictly after NOW, in ms.
        let created_ms = NOW * 1000;
        let body = format!(
            r#"{{"tasks":[
                {{"id":"da","cron":"* * * * *","prompt":"a","createdAt":{created_ms},"recurring":true}},
                {{"id":"db","cron":"* * * * *","prompt":"b","createdAt":{created_ms},"recurring":true}}
            ]}}"#
        );
        let fs = file_with(&body);
        let next = next_fire_epoch_ms(Path::new(PATH), fs, FixedClock::at_secs(NOW))
            .await
            .expect("a next fire exists");
        // Next whole minute strictly after NOW (…020s) is …040s → ms.
        let expected = ((NOW / 60 + 1) * 60) * 1000;
        assert_eq!(next, expected);
        assert!(next > NOW * 1000);
    }

    #[tokio::test]
    async fn next_fire_epoch_ms_none_when_empty() {
        let fs = file_with(r#"{"tasks":[]}"#);
        assert_eq!(
            next_fire_epoch_ms(Path::new(PATH), fs, FixedClock::at_secs(NOW)).await,
            None
        );
    }
}
