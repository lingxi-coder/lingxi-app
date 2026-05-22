//! Cron scheduler tick loop wired to [`lingxi_tasks::TaskRegistry`].
//!
//! The scheduler ticks once per minute, finds due jobs, attempts to acquire a
//! per-job cross-process lock (A9), applies jitter to avoid thundering-herd
//! launches, and spawns a `Dream` task via the registry.

use crate::lock::{try_acquire_lock, CronLockError};
use crate::schedule::{parse_cron, CronExpression};
use lingxi_tasks::registry::TaskRegistry;
use lingxi_tasks::{TaskSpawnInput, TaskType};
use lingxi_traits::{Clock, FileSystem, RuntimeSpawner};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock};

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
    tick_handle: Mutex<Option<lingxi_traits::BackgroundTaskHandle>>,
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
            tick_handle: Mutex::new(None),
        }
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
            },
        );
        Ok(())
    }

    /// Spawn the tick loop on the configured [`RuntimeSpawner`]. Safe to call
    /// once; calling again replaces the handle without stopping the prior loop.
    pub async fn start(self: Arc<Self>) -> Result<(), lingxi_traits::RuntimeError> {
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
    pub async fn stop(&self) -> Result<(), lingxi_traits::RuntimeError> {
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
