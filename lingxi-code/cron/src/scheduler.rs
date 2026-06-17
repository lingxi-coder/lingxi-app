//! Cron scheduler tick loop wired to [`tasks::TaskRegistry`].
//!
//! The scheduler ticks once per minute, finds due jobs, attempts to acquire a
//! per-job cross-process lock (A9), applies jitter to avoid thundering-herd
//! launches, and spawns a `Dream` task via the registry.

use crate::lock::{try_acquire_lock, CronLockError};
use crate::schedule::{parse_cron, CronExpression};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tasks::registry::TaskRegistry;
use tasks::{TaskSpawnInput, TaskType};
use tokio::sync::{Mutex, RwLock};
use traits::{Clock, FileSystem, RuntimeSpawner};

/// Default auto-expiry age for a RECURRING cron job — 30 days, 1:1 with
/// claude-code `DEFAULT_CRON_JITTER_CONFIG.recurringMaxAgeMs`
/// (`cronJitterConfig.ts`, `30 * 24 * 60 * 60 * 1000` ms) and the Rust
/// `CronCreate` `DEFAULT_MAX_AGE_DAYS = 30`. After this long since `created_at`,
/// a recurring job is auto-expired on the next tick — honoring the `CronCreate`
/// promise "Auto-expires after 30 days. Use CronDelete to cancel sooner."
pub const DEFAULT_RECURRING_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// One registered cron job: its schedule, prompt, and last-fire bookkeeping.
pub struct CronTaskDef {
    /// Stable job identifier (used for lock filenames and logs).
    pub id: String,
    /// Parsed cron schedule.
    pub schedule: CronExpression,
    /// Initial prompt handed to the spawned Dream task.
    pub prompt: String,
    /// Optional agent type hint for the spawned task.
    pub agent_type: Option<String>,
    /// Last time this job fired (used to suppress same-minute duplicates).
    pub last_run: Option<SystemTime>,
    /// When `false`, the scheduler skips this job entirely.
    pub enabled: bool,
    /// When the job was created (the persisted `created_at_unix_secs`). The
    /// anchor for recurring auto-expiry ([`is_recurring_task_aged`]).
    pub created_at: SystemTime,
    /// `true` = fire on every schedule match until deleted or auto-expired;
    /// `false` = one-shot. Only recurring jobs are subject to max-age expiry.
    pub recurring: bool,
}

/// Is a RECURRING cron job past its auto-expiry age? 1:1 with claude-code
/// `isRecurringTaskAged` (`cronScheduler.ts:59`,
/// `recurring && nowMs - createdAt >= maxAgeMs`). A one-shot (`!recurring`) job
/// is never aged-out here (it auto-deletes after firing instead);
/// `max_age == None` disables expiry entirely (unlimited); and a `created_at` in
/// the future (clock skew) is treated as not-yet-aged.
///
/// (claude-code also exempts `permanent` tasks; the Rust `CronCreate` schema has
/// no permanent/exempt flag — its `durable` flag is a persistence toggle, not an
/// expiry exemption — so every recurring job is subject to the max age, matching
/// the unconditional "Auto-expires after 30 days" the tool reports.)
#[must_use]
fn is_recurring_task_aged(
    now: SystemTime,
    created_at: SystemTime,
    recurring: bool,
    max_age: Option<Duration>,
) -> bool {
    let Some(max_age) = max_age else {
        return false;
    };
    recurring
        && now
            .duration_since(created_at)
            .is_ok_and(|age| age >= max_age)
}

/// Owner of the cron tick loop. Constructed with platform trait objects and
/// the shared [`TaskRegistry`].
pub struct CronScheduler {
    tasks: Arc<RwLock<HashMap<String, CronTaskDef>>>,
    task_registry: Arc<TaskRegistry>,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    runtime: Arc<dyn RuntimeSpawner>,
    lock_dir: PathBuf,
    /// Maximum random delay (seconds) applied before launching each due job.
    pub jitter_seconds: u32,
    /// Auto-expiry age for RECURRING jobs; `None` disables expiry (unlimited).
    /// Defaults to [`DEFAULT_RECURRING_MAX_AGE`].
    recurring_max_age: Option<Duration>,
    tick_handle: Mutex<Option<traits::BackgroundTaskHandle>>,
}

impl CronScheduler {
    /// Construct a new scheduler. The tick loop is not started until
    /// [`Self::start`] is invoked.
    #[must_use]
    pub fn new(
        task_registry: Arc<TaskRegistry>,
        fs: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        runtime: Arc<dyn RuntimeSpawner>,
        lock_dir: PathBuf,
    ) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            task_registry,
            fs,
            clock,
            runtime,
            lock_dir,
            jitter_seconds: 30,
            recurring_max_age: Some(DEFAULT_RECURRING_MAX_AGE),
            tick_handle: Mutex::new(None),
        }
    }

    /// Override the recurring auto-expiry age (`None` = unlimited / never
    /// expire). Defaults to [`DEFAULT_RECURRING_MAX_AGE`] (30 days).
    #[must_use]
    pub fn with_recurring_max_age(mut self, age: Option<Duration>) -> Self {
        self.recurring_max_age = age;
        self
    }

    /// Register a new cron job. The schedule string is parsed eagerly; an
    /// invalid expression is returned as a boxed error.
    pub async fn register(
        &self,
        id: &str,
        schedule_str: &str,
        prompt: &str,
        agent_type: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.register_with_meta(id, schedule_str, prompt, agent_type, self.clock.now(), true)
            .await
    }

    /// Like [`Self::register`] but with the persisted `created_at` (the expiry
    /// anchor) and `recurring` flag — used by the on-disk job loader so a job
    /// created days ago is correctly aged-out on load instead of resetting its
    /// clock to "now". `register` is the convenience form (created now,
    /// recurring).
    pub async fn register_with_meta(
        &self,
        id: &str,
        schedule_str: &str,
        prompt: &str,
        agent_type: Option<String>,
        created_at: SystemTime,
        recurring: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let schedule = parse_cron(schedule_str)?;
        self.tasks.write().await.insert(
            id.to_string(),
            CronTaskDef {
                id: id.into(),
                schedule,
                prompt: prompt.into(),
                agent_type,
                last_run: None,
                enabled: true,
                created_at,
                recurring,
            },
        );
        Ok(())
    }

    /// Spawn the tick loop on the configured [`RuntimeSpawner`]. Safe to call
    /// once; calling again replaces the handle without stopping the prior loop.
    pub async fn start(self: Arc<Self>) -> Result<(), traits::RuntimeError> {
        let me = self.clone();
        let handle = self
            .runtime
            .spawn(
                "cron-tick",
                Box::pin(async move {
                    loop {
                        me.tick().await;
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                }),
            )
            .await?;
        *self.tick_handle.lock().await = Some(handle);
        Ok(())
    }

    async fn tick(&self) {
        let now = self.clock.now();

        // Auto-expire aged RECURRING jobs (claude-code `isRecurringTaskAged` →
        // remove + `tengu_scheduled_task_expired`). Runs BEFORE the due-job scan,
        // so an expired job never fires again; its persisted descriptor is
        // deleted too, so it does not reload and re-expire every tick.
        let expired_ids: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks
                .values()
                .filter(|t| {
                    is_recurring_task_aged(now, t.created_at, t.recurring, self.recurring_max_age)
                })
                .map(|t| t.id.clone())
                .collect()
        };
        if !expired_ids.is_empty() {
            {
                let mut tasks = self.tasks.write().await;
                for id in &expired_ids {
                    tasks.remove(id);
                }
            }
            for id in &expired_ids {
                let job_path = self.lock_dir.join(format!("{id}.json"));
                let _ = self.fs.delete_file(&job_path.to_string_lossy()).await;
                tracing::info!(
                    event = "tengu_scheduled_task_expired",
                    cron_id = %id,
                    "cron job auto-expired after exceeding the recurring max age"
                );
            }
        }

        let due_ids: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks
                .values()
                .filter(|t| t.enabled && t.schedule.matches(now))
                .filter(|t| t.last_run.map_or(true, |lr| !same_minute(lr, now)))
                .map(|t| t.id.clone())
                .collect()
        };

        for id in due_ids {
            // Per-job lock with PID liveness check.
            let lock_path = self.lock_dir.join(format!("{id}.lock"));
            let our_pid = std::process::id();
            let acquired =
                try_acquire_lock(self.fs.clone(), &lock_path, our_pid, &id, pid_alive_check).await;
            if let Err(CronLockError::HeldByLivePid { pid }) = acquired {
                tracing::debug!("cron job {id} held by live PID {pid}; skipping");
                continue;
            }

            // Jitter to avoid thundering herd.
            if self.jitter_seconds > 0 {
                use rand::Rng;
                let delay = rand::rng().random_range(0..self.jitter_seconds);
                tokio::time::sleep(Duration::from_secs(u64::from(delay))).await;
            }

            // Spawn task via §11 TaskRegistry.
            let task_input = TaskSpawnInput::Dream {
                prompt: self
                    .tasks
                    .read()
                    .await
                    .get(&id)
                    .map(|t| t.prompt.clone())
                    .unwrap_or_default(),
                max_iterations: None,
            };
            if let Err(e) = self
                .task_registry
                .create(TaskType::Dream, task_input, format!("cron: {id}"))
                .await
            {
                tracing::error!("cron task {id} create failed: {e}");
            }

            // Update last_run.
            if let Some(t) = self.tasks.write().await.get_mut(&id) {
                t.last_run = Some(now);
            }

            // Release the lock so other peers see "stale" if we crash mid-task.
            let _ = crate::lock::release_lock(self.fs.clone(), &lock_path).await;
        }
    }

    /// Cancel the tick loop, if running. Idempotent.
    pub async fn stop(&self) -> Result<(), traits::RuntimeError> {
        if let Some(h) = self.tick_handle.lock().await.take() {
            self.runtime.cancel(&h).await?;
        }
        Ok(())
    }
}

fn same_minute(a: SystemTime, b: SystemTime) -> bool {
    let secs_a = a
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    let secs_b = b
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0);
    secs_a == secs_b
}

/// Detect whether `pid` corresponds to a live process. Used by
/// [`crate::lock::try_acquire_lock`] to override stale lock files left behind
/// by crashed schedulers (A9).
#[cfg(unix)]
#[allow(unsafe_code)]
#[must_use]
pub fn pid_alive_check(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) tests existence without delivering a signal.
    // The libc function takes only POD args; there are no aliasing concerns,
    // and a non-zero return is interpreted as "process does not exist or
    // permission denied", which we conservatively treat as "not alive".
    #[allow(clippy::cast_possible_wrap)]
    unsafe {
        libc::kill(pid as libc::pid_t, 0) == 0
    }
}

/// Detect whether `pid` corresponds to a live process (Windows variant).
#[cfg(windows)]
#[allow(unsafe_code)]
#[must_use]
pub fn pid_alive_check(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: OpenProcess returns a HANDLE (pointer-sized) we immediately
    // close. We do not dereference or share the handle; passing FALSE (0)
    // for bInheritHandle and a known constant for access flags is well-defined.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            false
        } else {
            CloseHandle(handle);
            true
        }
    }
}

#[cfg(test)]
mod expiry_tests {
    use super::{is_recurring_task_aged, DEFAULT_RECURRING_MAX_AGE};
    use std::time::{Duration, SystemTime};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// `n` days after the UNIX epoch.
    fn at(days: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(days * 24 * 60 * 60)
    }

    #[test]
    fn default_max_age_is_30_days() {
        assert_eq!(DEFAULT_RECURRING_MAX_AGE, DAY * 30);
    }

    #[test]
    fn recurring_job_past_max_age_is_aged() {
        // created day 0, now day 31, max 30d → aged.
        assert!(is_recurring_task_aged(at(31), at(0), true, Some(DAY * 30)));
    }

    #[test]
    fn recurring_job_at_exact_boundary_is_aged() {
        // age == max_age → aged (`>=`, matching claude-code's `nowMs - createdAt >= maxAgeMs`).
        assert!(is_recurring_task_aged(at(30), at(0), true, Some(DAY * 30)));
    }

    #[test]
    fn fresh_recurring_job_is_not_aged() {
        assert!(!is_recurring_task_aged(at(5), at(0), true, Some(DAY * 30)));
    }

    #[test]
    fn one_shot_job_is_never_aged_by_max_age() {
        // !recurring → never aged out here (one-shot auto-deletes after firing).
        assert!(!is_recurring_task_aged(at(100), at(0), false, Some(DAY * 30)));
    }

    #[test]
    fn none_max_age_disables_expiry() {
        assert!(!is_recurring_task_aged(at(10_000), at(0), true, None));
    }

    #[test]
    fn future_created_at_is_not_aged() {
        // Clock skew: created_at after now → duration_since errs → treated as not aged.
        assert!(!is_recurring_task_aged(at(0), at(5), true, Some(DAY * 30)));
    }
}
