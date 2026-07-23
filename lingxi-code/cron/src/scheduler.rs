//! Cron scheduler tick loop wired to [`tasks::TaskRegistry`].
//!
//! The scheduler ticks once per second, finds jobs whose deterministic
//! jittered fire time has arrived, acquires a per-job cross-process lock (A9),
//! and spawns a `Dream` task via the registry.

use crate::lock::{try_acquire_lock, CronLockError};
use crate::schedule::{parse_cron, CronExpression};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};
use std::time::{Duration, SystemTime};
use tasks::registry::TaskRegistry;
use tasks::{TaskSpawnInput, TaskType};
use tokio::sync::{Mutex, RwLock};
use traits::task_registry::TaskRegistryHandle;
use traits::{Clock, FileSystem, FsError, RuntimeSpawner};

/// The session-scoped portion of a cron job. These records never touch the
/// project tasks file; they live for exactly as long as the scheduler attached
/// to the session's task registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCronTask {
    /// Stable job identifier.
    pub id: String,
    /// Original five-field expression, retained for `CronList`.
    pub cron: String,
    /// Prompt submitted when the schedule fires.
    pub prompt: String,
    /// Creation time used for due detection and recurring expiry.
    pub created_at: SystemTime,
    /// Last successful claim, when recurring.
    pub last_fired_at: Option<SystemTime>,
    /// Whether the task repeats.
    pub recurring: bool,
    /// Calling teammate identity. `None` denotes the main thread.
    pub owner: Option<String>,
}

/// Failure to find the live scheduler belonging to a tool context.
#[derive(Debug, thiserror::Error)]
#[error("no active cron scheduler for this session")]
pub struct NoActiveCronScheduler;

static LIVE_SCHEDULERS: LazyLock<StdMutex<HashMap<usize, Weak<CronScheduler>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// Stable identity for an `Arc` allocation, preserved when coercing a concrete
/// task registry to its trait-object handle.
#[must_use]
pub fn task_registry_identity<T: ?Sized>(registry: &Arc<T>) -> usize {
    Arc::as_ptr(registry).cast::<()>() as usize
}

fn scheduler_for(
    registry: &Arc<dyn TaskRegistryHandle>,
) -> Result<Arc<CronScheduler>, NoActiveCronScheduler> {
    let key = task_registry_identity(registry);
    let scheduler = LIVE_SCHEDULERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .and_then(Weak::upgrade);
    scheduler.ok_or(NoActiveCronScheduler)
}

/// Register a newly-created tool job with the already-running scheduler.
/// Durable jobs are still authoritative on disk; session jobs are also added
/// to the scheduler-owned in-memory store used by list/delete.
pub async fn register_live_job(
    registry: &Arc<dyn TaskRegistryHandle>,
    task: SessionCronTask,
    durable: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    scheduler_for(registry)?
        .register_tool_job(task, durable)
        .await
}

/// Snapshot session-only jobs for `CronList` and stop-hook consumers.
pub async fn session_jobs(
    registry: &Arc<dyn TaskRegistryHandle>,
) -> Result<Vec<SessionCronTask>, NoActiveCronScheduler> {
    Ok(scheduler_for(registry)?.session_jobs().await)
}

/// Remove a runtime job. For a durable job this only removes the scheduler's
/// live mirror; the caller remains responsible for its locked file mutation.
pub async fn unregister_live_job(
    registry: &Arc<dyn TaskRegistryHandle>,
    id: &str,
    owner: Option<&str>,
) -> Result<bool, NoActiveCronScheduler> {
    Ok(scheduler_for(registry)?.unregister_job(id, owner).await)
}

/// Default auto-expiry age for a RECURRING cron job — 7 days, matching
/// claude-code `DEFAULT_CRON_JITTER_CONFIG.recurringMaxAgeMs`
/// (`604_800_000` ms) and the Rust `CronCreate` result text. After this long
/// since `created_at`, a recurring job is removed after its next due fire —
/// honoring the `CronCreate` promise "Auto-expires after 7 days. Use
/// `CronDelete` to cancel sooner."
pub const DEFAULT_RECURRING_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Claude Code's recurring jitter is at most half of the schedule interval.
pub const DEFAULT_RECURRING_JITTER_FRACTION: f64 = 0.5;
/// Absolute cap for recurring jitter, even on daily/weekly schedules.
pub const DEFAULT_RECURRING_JITTER_CAP: Duration = Duration::from_secs(30 * 60);
/// Maximum amount a one-shot scheduled on a half-hour boundary fires early.
pub const DEFAULT_ONE_SHOT_JITTER_MAX: Duration = Duration::from_secs(90);
/// One-shots only receive early jitter on minute 0 or 30.
pub const DEFAULT_ONE_SHOT_MINUTE_MOD: u32 = 30;
/// Five-minute recurring schedules run this far before the cache boundary.
pub const DEFAULT_CACHE_LEAD: Duration = Duration::from_secs(15);

const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const SCHEDULER_TICK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug)]
struct CronJitterConfig {
    recurring_fraction: f64,
    recurring_cap: Duration,
    one_shot_floor: Duration,
    one_shot_max: Duration,
    one_shot_minute_mod: u32,
    cache_lead: Duration,
}

impl Default for CronJitterConfig {
    fn default() -> Self {
        Self {
            recurring_fraction: DEFAULT_RECURRING_JITTER_FRACTION,
            recurring_cap: DEFAULT_RECURRING_JITTER_CAP,
            one_shot_floor: Duration::ZERO,
            one_shot_max: DEFAULT_ONE_SHOT_JITTER_MAX,
            one_shot_minute_mod: DEFAULT_ONE_SHOT_MINUTE_MOD,
            cache_lead: DEFAULT_CACHE_LEAD,
        }
    }
}

/// One registered cron job: its schedule, prompt, and last-fire bookkeeping.
#[derive(Clone)]
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
    /// When the job was created (the persisted `createdAt`, epoch ms on disk).
    /// The anchor for recurring auto-expiry ([`is_recurring_task_aged`]).
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
/// the unconditional "Auto-expires after 7 days" the tool reports.)
#[must_use]
pub(crate) fn is_recurring_task_aged(
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

/// Post-fire bookkeeping for a job that JUST fired. A ONE-SHOT (`!recurring`)
/// job is REMOVED from `tasks` and `true` is returned so the caller also deletes
/// its persisted descriptor — 1:1 with claude-code `recurring: false` ("fire
/// once at the next match, then auto-delete", `schedule_cron.rs` schema). A
/// recurring job records `last_run` in place and returns `false`. A missing id
/// is a no-op (`false`).
pub(crate) fn finalize_fired_job(
    tasks: &mut HashMap<String, CronTaskDef>,
    id: &str,
    now: SystemTime,
) -> bool {
    let recurring = match tasks.get(id) {
        Some(t) => t.recurring,
        None => return false,
    };
    if recurring {
        if let Some(t) = tasks.get_mut(id) {
            t.last_run = Some(now);
        }
        false
    } else {
        tasks.remove(id);
        true
    }
}

/// Set `lastFiredAt` (epoch **milliseconds**) on the task with `id` inside a
/// parsed single-file `{ "tasks": [...] }` document body, returning the
/// re-serialized file (pretty + trailing newline, 1:1 with claude-code). Returns
/// `None` if the body has no such id (so the caller leaves the file untouched).
/// Recording the last-fire time is what makes missed-run CATCH-UP safe ACROSS
/// RESTARTS — without it a reloaded job would re-fire a run it already fired in
/// a prior session (claude-code persists `lastFiredAt` for the same reason).
pub(crate) fn tasks_file_with_last_fired(
    body: &str,
    id: &str,
    last_fired_at_ms: u64,
) -> Option<String> {
    let mut doc = crate::tasks_file::parse_tasks(body);
    let task = doc.tasks.iter_mut().find(|t| t.id == id)?;
    task.last_fired_at = Some(last_fired_at_ms);
    Some(crate::tasks_file::serialize_tasks(&doc))
}

/// Remove the task with `id` from a parsed single-file `{ "tasks": [...] }`
/// document body, returning the re-serialized file. Returns `None` if the id is
/// absent (caller leaves the file untouched). Used when a one-shot job has fired
/// (auto-delete) and when a recurring job ages out.
pub(crate) fn tasks_file_without(body: &str, id: &str) -> Option<String> {
    let mut doc = crate::tasks_file::parse_tasks(body);
    let before = doc.tasks.len();
    doc.tasks.retain(|t| t.id != id);
    if doc.tasks.len() == before {
        return None;
    }
    Some(crate::tasks_file::serialize_tasks(&doc))
}

/// Convert the first eight hexadecimal digits of a job id to the stable
/// `[0, 1)` fraction used by Claude Code's `parseInt(id.slice(0, 8), 16) /
/// 2**32`. `parseInt` accepts a leading run of hex digits, so preserve that
/// behavior for legacy/non-generated ids instead of requiring all eight.
fn deterministic_id_fraction(id: &str) -> f64 {
    let prefix = id
        .as_bytes()
        .iter()
        .copied()
        .take(8)
        .take_while(u8::is_ascii_hexdigit)
        .collect::<Vec<_>>();
    if prefix.is_empty() {
        return 0.0;
    }
    let Ok(prefix) = std::str::from_utf8(&prefix) else {
        return 0.0;
    };
    u32::from_str_radix(prefix, 16).map_or(0.0, |value| f64::from(value) / 4_294_967_296.0)
}

fn scale_duration(duration: Duration, factor: f64) -> Duration {
    Duration::from_secs_f64(duration.as_secs_f64() * factor)
}

fn is_exact_every_n_minutes(expression: &str) -> bool {
    let Some(rest) = expression.strip_prefix("*/") else {
        return false;
    };
    let Some(step) = rest.strip_suffix(" * * * *") else {
        return false;
    };
    !step.is_empty() && step.bytes().all(|byte| byte.is_ascii_digit())
}

fn local_minute_with<F: Fn(u64) -> i64>(at: SystemTime, offset_for: &F) -> Option<u32> {
    let unix_secs = at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let local_secs = i128::from(unix_secs).saturating_add(i128::from(offset_for(unix_secs)));
    u32::try_from(local_secs.rem_euclid(3_600) / 60).ok()
}

fn next_fire_with<F: Fn(u64) -> i64>(
    task: &CronTaskDef,
    offset_for: &F,
    config: CronJitterConfig,
) -> Option<SystemTime> {
    let anchor = task.last_run.unwrap_or(task.created_at);
    let next = task.schedule.next_match_after_with(anchor, offset_for)?;

    if !task.recurring {
        let minute = local_minute_with(next, offset_for)?;
        if minute % config.one_shot_minute_mod != 0 {
            return Some(next);
        }
        let range = config
            .one_shot_max
            .checked_sub(config.one_shot_floor)
            .unwrap_or(Duration::ZERO);
        let early =
            config.one_shot_floor + scale_duration(range, deterministic_id_fraction(&task.id));
        return Some(next.checked_sub(early).unwrap_or(anchor).max(anchor));
    }

    let following = task.schedule.next_match_after_with(next, offset_for);
    let Some(interval) = following.and_then(|following| following.duration_since(next).ok()) else {
        return Some(next);
    };

    // Claude Code's cache-lead exception is intentionally narrow: only a raw
    // `*/N * * * *` expression whose interval is exactly the five-minute cache
    // TTL is shifted to `anchor + interval - 15s`.
    if is_exact_every_n_minutes(&task.schedule.raw)
        && config.cache_lead > Duration::ZERO
        && config.cache_lead < interval
        && interval >= CACHE_TTL
        && interval - config.cache_lead < CACHE_TTL
    {
        return anchor.checked_add(interval - config.cache_lead);
    }

    let jitter = scale_duration(
        interval,
        deterministic_id_fraction(&task.id) * config.recurring_fraction,
    )
    .min(config.recurring_cap);
    next.checked_add(jitter)
}

/// The deterministic next fire time for a job, including Claude Code's
/// recurring/one-shot jitter and five-minute cache-lead exception.
pub(crate) fn next_fire_time(task: &CronTaskDef) -> Option<SystemTime> {
    next_fire_with(
        task,
        &|secs| crate::schedule::local_offset_seconds(i64::try_from(secs).unwrap_or(0)),
        CronJitterConfig::default(),
    )
}

/// Is a job due at `now`? The next jittered fire after the last fire (or
/// creation) must have arrived. An overdue run is caught up once; persisting
/// `last_run = now` advances the anchor so it cannot be repeated after restart.
pub(crate) fn is_job_due(task: &CronTaskDef, now: SystemTime) -> bool {
    task.enabled && next_fire_time(task).is_some_and(|next| next <= now)
}

/// Offset-parameterized twin of [`is_job_due`] for deterministic, host-timezone
/// independent tests: `offset_for` supplies each candidate's UTC offset (use
/// `|_| 0` for UTC). The live [`is_job_due`] resolves the offset per-instant via
/// the system timezone, so its 09:00-LOCAL behavior can't be asserted against
/// fixed UTC timestamps directly.
#[cfg(test)]
fn is_job_due_with<F: Fn(u64) -> i64>(task: &CronTaskDef, now: SystemTime, offset_for: F) -> bool {
    task.enabled
        && next_fire_with(task, &offset_for, CronJitterConfig::default())
            .is_some_and(|next| next <= now)
}

/// Owner of the cron tick loop. Constructed with platform trait objects and
/// the shared [`TaskRegistry`].
pub struct CronScheduler {
    tasks: Arc<RwLock<HashMap<String, CronTaskDef>>>,
    session_tasks: Arc<RwLock<HashMap<String, SessionCronTask>>>,
    task_registry: Arc<TaskRegistry>,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    runtime: Arc<dyn RuntimeSpawner>,
    /// The single project tasks file
    /// (`<project_root>/.lingxi/scheduled_tasks.json`) the scheduler loads from
    /// and writes `lastFiredAt` back to. 1:1 with claude-code `cronTasks.ts`.
    tasks_file: PathBuf,
    /// Directory holding per-job lock files (the tasks file's parent, i.e.
    /// `<project_root>/.claude`). LingXi's A9 cross-process locks live beside the
    /// single tasks file.
    lock_dir: PathBuf,
    /// Auto-expiry age for RECURRING jobs; `None` disables expiry (unlimited).
    /// Defaults to [`DEFAULT_RECURRING_MAX_AGE`].
    recurring_max_age: Option<Duration>,
    tick_handle: Mutex<Option<traits::BackgroundTaskHandle>>,
}

impl CronScheduler {
    /// Construct a new scheduler over the single project tasks file
    /// `<project_root>/.lingxi/scheduled_tasks.json` (1:1 with claude-code
    /// `cronTasks.ts`). The per-job A9 lock files live in that file's parent
    /// directory (`<project_root>/.claude`). The tick loop is not started until
    /// [`Self::start`] is invoked.
    #[must_use]
    pub fn new(
        task_registry: Arc<TaskRegistry>,
        fs: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        runtime: Arc<dyn RuntimeSpawner>,
        tasks_file: PathBuf,
    ) -> Self {
        let lock_dir = tasks_file
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            session_tasks: Arc::new(RwLock::new(HashMap::new())),
            task_registry,
            fs,
            clock,
            runtime,
            tasks_file,
            lock_dir,
            recurring_max_age: Some(DEFAULT_RECURRING_MAX_AGE),
            tick_handle: Mutex::new(None),
        }
    }

    /// Load every persisted (durable) job from the single project tasks file,
    /// registering each into the in-memory schedule. A missing / unparseable
    /// file registers nothing. `createdAt` / `lastFiredAt` are read in epoch
    /// **milliseconds** (claude-code on-disk units). Invalid cron strings are
    /// skipped with a warning. Call once after construction, before
    /// [`Self::start`].
    pub async fn load_persisted(&self) {
        let Some(project_root) = crate::tasks_file::project_root_from_tasks_path(&self.tasks_file)
        else {
            tracing::error!(path = %self.tasks_file.display(), "cron: invalid tasks-file path");
            return;
        };
        let Ok(body) = crate::tasks_file::read_tasks_body(self.fs.as_ref(), project_root).await
        else {
            return; // file absent → nothing to load
        };
        let doc = match serde_json::from_str::<crate::tasks_file::ScheduledTasks>(&body) {
            Ok(doc) => doc,
            Err(error) => {
                // `parse_tasks` intentionally treats corrupt user-facing reads
                // as empty, but scheduler startup must not silently reinterpret
                // corrupt durable state as an empty valid task document.
                tracing::warn!(
                    path = %self.tasks_file.display(),
                    "cron tasks file has invalid authoritative state: {error}; skipping"
                );
                return;
            }
        };
        for t in doc.tasks {
            // Anchor expiry/catch-up off the persisted ms timestamps. A
            // missing/zero `createdAt` falls back to "now" (a fresh window).
            let created_at = if t.created_at > 0 {
                SystemTime::UNIX_EPOCH + Duration::from_millis(t.created_at)
            } else {
                self.clock.now()
            };
            let recurring = t.recurring.unwrap_or(false);
            let last_run = t
                .last_fired_at
                .filter(|ms| *ms > 0)
                .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
            if let Err(e) = self
                .register_with_meta(
                    &t.id, &t.cron, &t.prompt, None, created_at, recurring, last_run,
                )
                .await
            {
                tracing::warn!("cron: skipping job {} with invalid schedule: {e}", t.id);
            }
        }
    }

    /// Override the recurring auto-expiry age (`None` = unlimited / never
    /// expire). Defaults to [`DEFAULT_RECURRING_MAX_AGE`] (7 days).
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
        self.register_with_meta(
            id,
            schedule_str,
            prompt,
            agent_type,
            self.clock.now(),
            true,
            None,
        )
        .await
    }

    /// Like [`Self::register`] but with the persisted `created_at` (the expiry
    /// anchor), `recurring` flag, and `last_run` (the restored last-fire time, or
    /// `None` if never fired) — used by the on-disk job loader so a job created
    /// days ago is aged correctly AND its catch-up anchor survives a restart
    /// (preventing a re-fire of an already-fired run). `register` is the
    /// convenience form (created now, recurring, never fired).
    pub async fn register_with_meta(
        &self,
        id: &str,
        schedule_str: &str,
        prompt: &str,
        agent_type: Option<String>,
        created_at: SystemTime,
        recurring: bool,
        last_run: Option<SystemTime>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let schedule = parse_cron(schedule_str)?;
        self.tasks.write().await.insert(
            id.to_string(),
            CronTaskDef {
                id: id.into(),
                schedule,
                prompt: prompt.into(),
                agent_type,
                last_run,
                enabled: true,
                created_at,
                recurring,
            },
        );
        Ok(())
    }

    async fn register_tool_job(
        &self,
        task: SessionCronTask,
        durable: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let schedule = parse_cron(&task.cron)?;
        let mut tasks = self.tasks.write().await;
        if !tasks.contains_key(&task.id) && tasks.len() >= 50 {
            return Err(std::io::Error::other("too many scheduled jobs (max 50)").into());
        }
        tasks.insert(
            task.id.clone(),
            CronTaskDef {
                id: task.id.clone(),
                schedule,
                prompt: task.prompt.clone(),
                agent_type: None,
                last_run: task.last_fired_at,
                enabled: true,
                created_at: task.created_at,
                recurring: task.recurring,
            },
        );
        drop(tasks);
        if !durable {
            self.session_tasks
                .write()
                .await
                .insert(task.id.clone(), task);
        }
        Ok(())
    }

    async fn session_jobs(&self) -> Vec<SessionCronTask> {
        let mut jobs: Vec<_> = self.session_tasks.read().await.values().cloned().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    async fn unregister_job(&self, id: &str, owner: Option<&str>) -> bool {
        let session_match = {
            let tasks = self.session_tasks.read().await;
            tasks.get(id).is_some_and(|task| match owner {
                Some(owner) => task.owner.as_deref() == Some(owner),
                None => true,
            })
        };
        let is_session = self.session_tasks.read().await.contains_key(id);
        if is_session && !session_match {
            return false;
        }
        self.session_tasks.write().await.remove(id);
        self.tasks.write().await.remove(id).is_some()
    }

    /// Spawn the tick loop on the configured [`RuntimeSpawner`]. Safe to call
    /// once; calling again replaces the handle without stopping the prior loop.
    pub async fn start(self: Arc<Self>) -> Result<(), traits::RuntimeError> {
        let registry_key = task_registry_identity(&self.task_registry);
        LIVE_SCHEDULERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(registry_key, Arc::downgrade(&self));
        let me = self.clone();
        let handle = match self
            .runtime
            .spawn(
                "cron-tick",
                Box::pin(async move {
                    loop {
                        me.tick().await;
                        me.runtime.sleep(SCHEDULER_TICK_INTERVAL).await;
                    }
                }),
            )
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                LIVE_SCHEDULERS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&registry_key);
                return Err(error);
            }
        };
        *self.tick_handle.lock().await = Some(handle);
        Ok(())
    }

    async fn process_due_ids(&self, now: SystemTime, due_ids: Vec<String>) {
        for id in due_ids {
            if self.session_tasks.read().await.contains_key(&id) {
                if let Some(task_input) = self.claim_in_memory_due_job(&id, now).await {
                    if let Err(e) = self
                        .task_registry
                        .create(TaskType::Dream, task_input, format!("cron: {id}"))
                        .await
                    {
                        tracing::error!("cron task {id} create failed: {e}");
                    }
                }
                continue;
            }
            // Per-job lock with PID liveness check.
            let lock_path = self.lock_dir.join(format!("{id}.lock"));
            let our_pid = std::process::id();
            let acquired =
                try_acquire_lock(self.fs.clone(), &lock_path, our_pid, &id, pid_alive_check).await;
            match acquired {
                Ok(()) => {}
                Err(CronLockError::HeldByLivePid { pid }) => {
                    tracing::debug!("cron job {id} held by live PID {pid}; skipping");
                    continue;
                }
                Err(error) => {
                    // Fail closed: running without the intended ownership lock
                    // turns a transient filesystem/parse failure into duplicate
                    // task execution across scheduler processes.
                    tracing::warn!("cron job {id} lock acquisition failed: {error}; skipping");
                    continue;
                }
            }

            // Re-read the authoritative state after lock acquisition and
            // claim the run BEFORE launch so a peer that already persisted
            // `lastFiredAt`/deletion suppresses this stale due snapshot.
            if let Some(task_input) = self.claim_due_job_if_still_due(&id, now).await {
                if let Err(e) = self
                    .task_registry
                    .create(TaskType::Dream, task_input, format!("cron: {id}"))
                    .await
                {
                    tracing::error!("cron task {id} create failed: {e}");
                }
            }

            // Release the lock so other peers see "stale" if we crash mid-task.
            let _ = crate::lock::release_lock(self.fs.clone(), &lock_path).await;
        }
    }

    async fn tick(&self) {
        let now = self.clock.now();

        // Due-detection with missed-run CATCH-UP (`is_job_due`): fires at the
        // deterministic jittered target and catches up a run missed while the
        // scheduler was down (once). Advancing and persisting the anchor
        // prevents cross-restart re-fires.
        let due_ids: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks
                .values()
                .filter(|t| is_job_due(t, now))
                .map(|t| t.id.clone())
                .collect()
        };

        self.process_due_ids(now, due_ids).await;
    }

    /// Cancel the tick loop, if running. Idempotent.
    pub async fn stop(&self) -> Result<(), traits::RuntimeError> {
        let cancel_result = if let Some(h) = self.tick_handle.lock().await.take() {
            self.runtime.cancel(&h).await
        } else {
            Ok(())
        };
        let key = task_registry_identity(&self.task_registry);
        LIVE_SCHEDULERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
        self.session_tasks.write().await.clear();
        self.tasks.write().await.clear();
        cancel_result
    }

    async fn claim_due_job_if_still_due(
        &self,
        id: &str,
        now: SystemTime,
    ) -> Option<TaskSpawnInput> {
        let local = { self.tasks.read().await.get(id).cloned()? };

        let Some(project_root) = crate::tasks_file::project_root_from_tasks_path(&self.tasks_file)
        else {
            return self.claim_in_memory_due_job(id, now).await;
        };

        let _process_guard = crate::lock_cron_file().await;
        let Ok(_file_guard) =
            crate::tasks_file::lock_scheduled_tasks(self.fs.as_ref(), project_root).await
        else {
            tracing::warn!("cron job {id} failed to lock scheduled tasks file; skipping");
            return None;
        };

        let body = match crate::tasks_file::read_tasks_body(self.fs.as_ref(), project_root).await {
            Ok(body) => body,
            Err(FsError::NotFound(_)) => return self.claim_in_memory_due_job(id, now).await,
            Err(error) => {
                // An unreadable durable state file is not evidence that this is
                // an in-memory-only job. Fail closed or a transient I/O error
                // can make multiple schedulers execute the same stale record.
                tracing::warn!(
                    "cron job {id} could not read authoritative state: {error}; skipping"
                );
                return None;
            }
        };

        let doc = match serde_json::from_str::<crate::tasks_file::ScheduledTasks>(&body) {
            Ok(doc) => doc,
            Err(error) => {
                // User-facing listing deliberately treats malformed JSON as an
                // empty document. Execution claims cannot: doing so would make
                // a stale durable job look in-memory-only and execute it.
                tracing::warn!("cron job {id} has invalid authoritative state: {error}; skipping");
                return None;
            }
        };
        let Some(on_disk) = doc.tasks.into_iter().find(|task| task.id == id) else {
            return self.claim_in_memory_due_job(id, now).await;
        };

        let authoritative = match parse_cron(&on_disk.cron) {
            Ok(schedule) => CronTaskDef {
                id: on_disk.id,
                schedule,
                prompt: on_disk.prompt,
                agent_type: local.agent_type,
                last_run: on_disk
                    .last_fired_at
                    .filter(|ms| *ms > 0)
                    .map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms)),
                enabled: local.enabled,
                created_at: if on_disk.created_at > 0 {
                    SystemTime::UNIX_EPOCH + Duration::from_millis(on_disk.created_at)
                } else {
                    local.created_at
                },
                recurring: on_disk.recurring.unwrap_or(false),
            },
            Err(e) => {
                tracing::warn!("cron job {id} has invalid persisted schedule during claim: {e}");
                return None;
            }
        };

        if !is_job_due(&authoritative, now) {
            self.sync_task_from_authoritative(&authoritative).await;
            return None;
        }

        // Claude Code checks recurringMaxAge only after a due job has entered
        // the fire path. An aged job therefore gets one final fire; an aged job
        // whose next run has not arrived remains registered.
        let expires_after_fire = is_recurring_task_aged(
            now,
            authoritative.created_at,
            authoritative.recurring,
            self.recurring_max_age,
        );

        let updated = if authoritative.recurring && !expires_after_fire {
            tasks_file_with_last_fired(&body, id, unix_epoch_ms(now))
        } else {
            tasks_file_without(&body, id)
        };
        let Some(updated) = updated else {
            return None;
        };
        if let Err(e) =
            crate::tasks_file::write_tasks_body(self.fs.as_ref(), project_root, &updated).await
        {
            tracing::warn!("cron job {id} failed to persist claimed run: {e}");
            return None;
        }

        self.apply_claimed_task_state(&authoritative, now, expires_after_fire)
            .await
    }

    async fn claim_in_memory_due_job(&self, id: &str, now: SystemTime) -> Option<TaskSpawnInput> {
        let mut tasks = self.tasks.write().await;
        let prompt = {
            let task = tasks.get(id)?;
            if !is_job_due(task, now) {
                return None;
            }
            task.prompt.clone()
        };
        let expires_after_fire = tasks.get(id).is_some_and(|task| {
            is_recurring_task_aged(now, task.created_at, task.recurring, self.recurring_max_age)
        });
        let remove_after_fire = finalize_fired_job(&mut tasks, id, now) || expires_after_fire;
        if expires_after_fire {
            tasks.remove(id);
        }
        let mut session_tasks = self.session_tasks.write().await;
        if remove_after_fire {
            session_tasks.remove(id);
        } else if let Some(task) = session_tasks.get_mut(id) {
            task.last_fired_at = Some(now);
        }
        if expires_after_fire {
            tracing::info!(
                event = "tengu_scheduled_task_expired",
                cron_id = %id,
                "cron job fired its final run after exceeding the recurring max age"
            );
        }
        Some(TaskSpawnInput::Dream {
            prompt,
            max_iterations: None,
        })
    }

    async fn sync_task_from_authoritative(&self, authoritative: &CronTaskDef) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(&authoritative.id) {
            *task = authoritative.clone();
        }
    }

    async fn apply_claimed_task_state(
        &self,
        authoritative: &CronTaskDef,
        now: SystemTime,
        expires_after_fire: bool,
    ) -> Option<TaskSpawnInput> {
        let mut tasks = self.tasks.write().await;
        let remove_after_fire =
            finalize_fired_job(&mut tasks, &authoritative.id, now) || expires_after_fire;
        if expires_after_fire {
            tasks.remove(&authoritative.id);
        }
        if !remove_after_fire {
            if let Some(task) = tasks.get_mut(&authoritative.id) {
                *task = authoritative.clone();
                task.last_run = Some(now);
            }
        }
        if expires_after_fire {
            tracing::info!(
                event = "tengu_scheduled_task_expired",
                cron_id = %authoritative.id,
                "cron job fired its final run after exceeding the recurring max age"
            );
        }
        Some(TaskSpawnInput::Dream {
            prompt: authoritative.prompt.clone(),
            max_iterations: None,
        })
    }
}

fn unix_epoch_ms(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    use super::{
        deterministic_id_fraction, is_recurring_task_aged, next_fire_with, CronJitterConfig,
        DEFAULT_RECURRING_MAX_AGE,
    };
    use std::time::{Duration, SystemTime};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// `n` days after the UNIX epoch.
    fn at(days: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(days * 24 * 60 * 60)
    }

    #[test]
    fn default_max_age_is_7_days() {
        assert_eq!(DEFAULT_RECURRING_MAX_AGE, DAY * 7);
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
        assert!(!is_recurring_task_aged(
            at(100),
            at(0),
            false,
            Some(DAY * 30)
        ));
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

    #[test]
    fn finalize_one_shot_removes_recurring_keeps() {
        use super::{finalize_fired_job, CronTaskDef};
        use crate::schedule::parse_cron;
        use std::collections::HashMap;

        let mk = |id: &str, recurring: bool| CronTaskDef {
            id: id.into(),
            schedule: parse_cron("* * * * *").unwrap(),
            prompt: "p".into(),
            agent_type: None,
            last_run: None,
            enabled: true,
            created_at: at(0),
            recurring,
        };
        let mut tasks = HashMap::new();
        tasks.insert("rec".to_string(), mk("rec", true));
        tasks.insert("once".to_string(), mk("once", false));

        // One-shot: removed, returns true (caller deletes the descriptor).
        assert!(finalize_fired_job(&mut tasks, "once", at(1)));
        assert!(!tasks.contains_key("once"));

        // Recurring: kept, last_run recorded, returns false.
        assert!(!finalize_fired_job(&mut tasks, "rec", at(1)));
        assert_eq!(tasks.get("rec").unwrap().last_run, Some(at(1)));

        // Missing id: no-op false.
        assert!(!finalize_fired_job(&mut tasks, "ghost", at(1)));
    }

    #[test]
    fn tasks_file_with_last_fired_sets_ms_and_preserves_other_tasks() {
        use super::tasks_file_with_last_fired;
        // Two tasks; set lastFiredAt (ms) on the second only.
        let orig = r#"{"tasks":[
            {"id":"a","cron":"0 9 * * *","prompt":"pa","createdAt":100,"recurring":true},
            {"id":"j","cron":"0 9 * * *","prompt":"p","createdAt":200,"recurring":true,"permanent":true}
        ]}"#;
        let updated = tasks_file_with_last_fired(orig, "j", 555_000).unwrap();
        let doc = crate::tasks_file::parse_tasks(&updated);
        let a = doc.tasks.iter().find(|t| t.id == "a").unwrap();
        let j = doc.tasks.iter().find(|t| t.id == "j").unwrap();
        // The targeted task gets lastFiredAt in MS; the other is untouched.
        assert_eq!(j.last_fired_at, Some(555_000));
        assert_eq!(a.last_fired_at, None);
        // Unrelated fields preserved (incl. permanent).
        assert_eq!(j.created_at, 200);
        assert_eq!(j.permanent, Some(true));
        // Serialized form is camelCase with a trailing newline.
        assert!(updated.contains("\"lastFiredAt\": 555000"));
        assert!(updated.ends_with("}\n"));
        // Missing id → None (caller leaves the file untouched).
        assert!(tasks_file_with_last_fired(orig, "ghost", 1).is_none());
        assert!(tasks_file_with_last_fired("not json", "j", 1).is_none());
    }

    #[test]
    fn tasks_file_without_drops_only_the_target() {
        use super::tasks_file_without;
        let orig = r#"{"tasks":[
            {"id":"a","cron":"* * * * *","prompt":"pa","createdAt":1},
            {"id":"b","cron":"* * * * *","prompt":"pb","createdAt":2}
        ]}"#;
        let updated = tasks_file_without(orig, "a").unwrap();
        let doc = crate::tasks_file::parse_tasks(&updated);
        assert_eq!(doc.tasks.len(), 1);
        assert_eq!(doc.tasks[0].id, "b");
        // Missing id → None.
        assert!(tasks_file_without(orig, "ghost").is_none());
        assert!(tasks_file_without("not json", "a").is_none());
    }

    #[test]
    fn is_job_due_fires_live_catches_up_and_never_double_fires() {
        use super::{is_job_due_with, CronTaskDef};
        use crate::schedule::parse_cron;

        let sec = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let nine_am = sec(1_700_038_800); // 2023-11-15 09:00 UTC
        let eight_am = sec(1_700_035_200); // same day 08:00
        let ten_am = sec(1_700_042_400); // same day 10:00
        let yest_nine = sec(1_700_038_800 - 86_400); // 2023-11-14 09:00

        let mk =
            |last_run: Option<SystemTime>, created_at: SystemTime, enabled: bool| CronTaskDef {
                id: "j".into(),
                schedule: parse_cron("0 9 * * *").unwrap(),
                prompt: "p".into(),
                agent_type: None,
                last_run,
                enabled,
                created_at,
                recurring: true,
            };

        // UTC (offset 0) for deterministic assertions; the live `is_job_due`
        // resolves the offset per-instant via the system timezone.
        let utc = |_: u64| 0_i64;
        // LIVE: last fired yesterday 09:00, now today 09:00 → due.
        assert!(is_job_due_with(
            &mk(Some(yest_nine), eight_am, true),
            nine_am,
            utc
        ));
        // Before the scheduled minute (now 08:00) → not due.
        assert!(!is_job_due_with(
            &mk(Some(yest_nine), eight_am, true),
            eight_am,
            utc
        ));
        // CATCH-UP: never fired, created 08:00, now 10:00 (missed 09:00) → due.
        assert!(is_job_due_with(&mk(None, eight_am, true), ten_am, utc));
        // NO DOUBLE-FIRE: just fired at 09:00, still 09:00 → next run tomorrow → not due.
        assert!(!is_job_due_with(
            &mk(Some(nine_am), eight_am, true),
            nine_am,
            utc
        ));
        // Per-task disabled → never due.
        assert!(!is_job_due_with(
            &mk(Some(yest_nine), eight_am, false),
            nine_am,
            utc
        ));
    }

    #[test]
    fn deterministic_fraction_matches_js_parse_int_prefix() {
        let close = |actual: f64, expected: f64| (actual - expected).abs() < f64::EPSILON;
        assert!(close(deterministic_id_fraction("00000000"), 0.0));
        assert!(close(deterministic_id_fraction("80000000"), 0.5));
        assert!(close(deterministic_id_fraction("80000000suffix"), 0.5));
        assert!(close(deterministic_id_fraction("not-hex"), 0.0));
        assert!(
            close(
                deterministic_id_fraction("aZ-not-all-hex"),
                10.0 / 4_294_967_296.0,
            ),
            "JavaScript parseInt stops at the first non-hex digit"
        );
    }

    #[test]
    fn recurring_jitter_is_deterministic_and_capped_at_30_minutes() {
        use super::CronTaskDef;
        use crate::schedule::parse_cron;

        let eight_am = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_035_200);
        let nine_thirty = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_040_600);
        let task = CronTaskDef {
            id: "80000000".into(),
            schedule: parse_cron("0 9 * * *").unwrap(),
            prompt: "p".into(),
            agent_type: None,
            last_run: None,
            enabled: true,
            created_at: eight_am,
            recurring: true,
        };

        // Hash fraction 0.5 × recurring fraction 0.5 × 24h = 6h, capped to 30m.
        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            Some(nine_thirty)
        );
        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            Some(nine_thirty),
            "the same id and schedule always produce the same fire time"
        );
    }

    #[test]
    fn recurring_five_minute_schedule_uses_15_second_cache_lead() {
        use super::CronTaskDef;
        use crate::schedule::parse_cron;

        let anchor = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_035_200);
        let task = CronTaskDef {
            id: "ffffffff".into(),
            schedule: parse_cron("*/5 * * * *").unwrap(),
            prompt: "p".into(),
            agent_type: None,
            last_run: None,
            enabled: true,
            created_at: anchor,
            recurring: true,
        };

        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            anchor.checked_add(Duration::from_secs(4 * 60 + 45)),
            "cache lead takes precedence over ordinary recurring jitter"
        );
    }

    #[test]
    fn one_shot_half_hour_jitter_is_early_and_clamped_to_creation() {
        use super::CronTaskDef;
        use crate::schedule::parse_cron;

        let eight_am = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_035_200);
        let eight_fifty_nine_fifteen = eight_am + Duration::from_secs(59 * 60 + 15);
        let mut task = CronTaskDef {
            id: "80000000".into(),
            schedule: parse_cron("0 * * * *").unwrap(),
            prompt: "p".into(),
            agent_type: None,
            last_run: None,
            enabled: true,
            created_at: eight_am,
            recurring: false,
        };

        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            Some(eight_fifty_nine_fifteen),
            "hash fraction 0.5 pulls a half-hour-boundary one-shot forward 45s"
        );

        let created_too_late = eight_am + Duration::from_secs(59 * 60 + 40);
        task.created_at = created_too_late;
        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            Some(created_too_late),
            "early jitter never schedules before the task was created"
        );
    }

    #[test]
    fn one_shot_non_half_hour_minute_is_not_jittered() {
        use super::CronTaskDef;
        use crate::schedule::parse_cron;

        let eight_oh_six = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_035_560);
        let nine_oh_five = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_039_100);
        let task = CronTaskDef {
            id: "ffffffff".into(),
            schedule: parse_cron("5 * * * *").unwrap(),
            prompt: "p".into(),
            agent_type: None,
            last_run: None,
            enabled: true,
            created_at: eight_oh_six,
            recurring: false,
        };

        assert_eq!(
            next_fire_with(&task, &|_| 0, CronJitterConfig::default()),
            Some(nine_oh_five)
        );
    }
}

#[cfg(test)]
mod scheduler_tick_tests {
    use super::{CronScheduler, SessionCronTask};
    use async_trait::async_trait;
    use futures::Stream;
    use std::collections::HashMap;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
    use tasks::output_manager::TaskOutputManager;
    use tasks::registry::TaskRegistry;
    use traits::filesystem::{FileContent, FileEvent, FlockGuard, FsError};
    use traits::{BackgroundTaskHandle, Clock, FileSystem, RuntimeError, RuntimeSpawner};

    const NOW: u64 = 1_700_000_000;
    const TASKS_PATH: &str = "/proj/.lingxi/scheduled_tasks.json";
    const OUTPUT_DIR: &str = "/proj/task-output";

    struct MemFs {
        files: tokio::sync::Mutex<HashMap<String, String>>,
    }

    struct MemFlockGuard(String);

    impl FlockGuard for MemFlockGuard {
        fn path(&self) -> &str {
            &self.0
        }
    }

    impl MemFs {
        fn with(path: &str, body: &str) -> Arc<Self> {
            let mut files = HashMap::new();
            files.insert(path.to_string(), body.to_string());
            Arc::new(Self {
                files: tokio::sync::Mutex::new(files),
            })
        }

        async fn get(&self, path: &str) -> Option<String> {
            self.files.lock().await.get(path).cloned()
        }

        async fn count_suffix(&self, suffix: &str) -> usize {
            self.files
                .lock()
                .await
                .keys()
                .filter(|path| path.ends_with(suffix))
                .count()
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
                Some(content) => Ok(FileContent {
                    content: content.clone(),
                    truncated: false,
                    total_lines: content.lines().count() as u64,
                }),
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
                .map_or(0, |content| content.len() as u64))
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

    struct FixedClock(SystemTime);

    impl FixedClock {
        fn at_secs(secs: u64) -> Arc<Self> {
            Arc::new(Self(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)))
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    struct UnusedRuntime;

    #[async_trait]
    impl RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _: &str,
            _: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        ) -> Result<BackgroundTaskHandle, RuntimeError> {
            panic!("spawn should not be called in scheduler tick tests");
        }

        async fn sleep(&self, duration: Duration) {
            tokio::time::sleep(duration).await;
        }

        async fn cancel(&self, _: &BackgroundTaskHandle) -> Result<(), RuntimeError> {
            panic!("cancel should not be called in scheduler tick tests");
        }
    }

    fn registry(fs: Arc<dyn FileSystem>) -> Arc<TaskRegistry> {
        Arc::new(TaskRegistry::new(
            Arc::new(UnusedRuntime),
            fs.clone(),
            Arc::new(TaskOutputManager::new(PathBuf::from(OUTPUT_DIR), fs)),
        ))
    }

    fn scheduler(
        registry: Arc<TaskRegistry>,
        fs: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
    ) -> CronScheduler {
        CronScheduler::new(
            registry,
            fs,
            clock,
            Arc::new(UnusedRuntime),
            PathBuf::from(TASKS_PATH),
        )
    }

    #[tokio::test]
    async fn stale_due_snapshot_from_peer_only_creates_one_task() {
        let created_ms = (NOW - 120) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"d11111111","cron":"* * * * *","prompt":"hello","createdAt":{created_ms},"recurring":true}}]}}"#
        );
        let fs = MemFs::with(TASKS_PATH, &body);
        let clock = FixedClock::at_secs(NOW);

        let scheduler_a = scheduler(registry(fs.clone()), fs.clone(), clock.clone());
        let scheduler_b = scheduler(registry(fs.clone()), fs.clone(), clock.clone());

        scheduler_a.load_persisted().await;
        scheduler_b.load_persisted().await;

        let stale_due_ids = {
            let tasks = scheduler_b.tasks.read().await;
            tasks
                .values()
                .filter(|task| super::is_job_due(task, clock.now()))
                .map(|task| task.id.clone())
                .collect::<Vec<_>>()
        };

        scheduler_a.tick().await;
        scheduler_b
            .process_due_ids(clock.now(), stale_due_ids)
            .await;

        assert_eq!(
            fs.count_suffix(".output").await,
            1,
            "the stale second scheduler snapshot must not create a duplicate task"
        );

        let after = crate::tasks_file::parse_tasks(&fs.get(TASKS_PATH).await.unwrap());
        assert_eq!(after.tasks.len(), 1);
        assert_eq!(after.tasks[0].last_fired_at, Some(NOW * 1000));

        let scheduler_b_last_run = scheduler_b
            .tasks
            .read()
            .await
            .get("d11111111")
            .and_then(|task| task.last_run);
        assert_eq!(
            scheduler_b_last_run,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(NOW))
        );
    }

    #[tokio::test]
    async fn corrupt_authoritative_state_never_executes_stale_in_memory_job() {
        let created_ms = (NOW - 120) * 1000;
        let body = format!(
            r#"{{"tasks":[{{"id":"d22222222","cron":"* * * * *","prompt":"must not run","createdAt":{created_ms},"recurring":true}}]}}"#
        );
        let fs = MemFs::with(TASKS_PATH, &body);
        let clock = FixedClock::at_secs(NOW);
        let scheduler = scheduler(registry(fs.clone()), fs.clone(), clock.clone());
        scheduler.load_persisted().await;
        let stale_due_ids = vec!["d22222222".to_string()];
        fs.files
            .lock()
            .await
            .insert(TASKS_PATH.to_string(), "{".to_string());

        scheduler.process_due_ids(clock.now(), stale_due_ids).await;

        assert_eq!(
            fs.count_suffix(".output").await,
            0,
            "corrupt durable state must fail closed instead of running stale memory"
        );
    }

    #[tokio::test]
    async fn session_one_shot_is_listed_claimed_and_removed_without_disk_state() {
        let fs = MemFs::with(TASKS_PATH, r#"{"tasks":[]}"#);
        let clock = FixedClock::at_secs(NOW);
        let scheduler = scheduler(registry(fs.clone()), fs.clone(), clock.clone());
        scheduler
            .register_tool_job(
                SessionCronTask {
                    id: "dsession1".into(),
                    cron: "* * * * *".into(),
                    prompt: "wake".into(),
                    created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(NOW - 120),
                    last_fired_at: None,
                    recurring: false,
                    owner: Some("agent:one".into()),
                },
                false,
            )
            .await
            .unwrap();

        assert_eq!(scheduler.session_jobs().await.len(), 1);
        scheduler.tick().await;
        assert!(scheduler.session_jobs().await.is_empty());
        assert!(!scheduler.tasks.read().await.contains_key("dsession1"));
        let persisted = crate::tasks_file::parse_tasks(&fs.get(TASKS_PATH).await.unwrap());
        assert!(persisted.tasks.is_empty());
    }

    #[tokio::test]
    async fn session_recurring_claim_updates_memory_and_owner_guards_delete() {
        let fs = MemFs::with(TASKS_PATH, r#"{"tasks":[]}"#);
        let clock = FixedClock::at_secs(NOW);
        let scheduler = scheduler(registry(fs.clone()), fs, clock.clone());
        scheduler
            .register_tool_job(
                SessionCronTask {
                    id: "dsession2".into(),
                    cron: "* * * * *".into(),
                    prompt: "repeat".into(),
                    created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(NOW - 120),
                    last_fired_at: None,
                    recurring: true,
                    owner: Some("agent:one".into()),
                },
                false,
            )
            .await
            .unwrap();

        assert!(
            !scheduler
                .unregister_job("dsession2", Some("agent:two"))
                .await
        );
        scheduler.tick().await;
        let jobs = scheduler.session_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].last_fired_at, Some(clock.now()));
        assert!(
            scheduler
                .unregister_job("dsession2", Some("agent:one"))
                .await
        );
        assert!(scheduler.session_jobs().await.is_empty());
    }

    #[tokio::test]
    async fn aged_session_recurring_fires_once_then_expires() {
        let fs = MemFs::with(TASKS_PATH, r#"{"tasks":[]}"#);
        let clock = FixedClock::at_secs(NOW);
        let scheduler = scheduler(registry(fs.clone()), fs.clone(), clock.clone());
        scheduler
            .register_tool_job(
                SessionCronTask {
                    id: "00000000".into(),
                    cron: "* * * * *".into(),
                    prompt: "final".into(),
                    created_at: SystemTime::UNIX_EPOCH
                        + Duration::from_secs(NOW - 8 * 24 * 60 * 60),
                    last_fired_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(NOW - 120)),
                    recurring: true,
                    owner: None,
                },
                false,
            )
            .await
            .unwrap();

        scheduler.tick().await;

        assert!(scheduler.session_jobs().await.is_empty());
        assert!(!scheduler.tasks.read().await.contains_key("00000000"));
        assert_eq!(fs.count_suffix(".output").await, 1);
    }

    #[tokio::test]
    async fn aged_session_recurring_not_due_is_not_predeleted() {
        let fs = MemFs::with(TASKS_PATH, r#"{"tasks":[]}"#);
        let clock = FixedClock::at_secs(NOW);
        let scheduler = scheduler(registry(fs.clone()), fs, clock.clone());
        scheduler
            .register_tool_job(
                SessionCronTask {
                    id: "00000000".into(),
                    cron: "* * * * *".into(),
                    prompt: "later".into(),
                    created_at: SystemTime::UNIX_EPOCH
                        + Duration::from_secs(NOW - 8 * 24 * 60 * 60),
                    last_fired_at: Some(clock.now()),
                    recurring: true,
                    owner: None,
                },
                false,
            )
            .await
            .unwrap();

        scheduler.tick().await;

        assert_eq!(scheduler.session_jobs().await.len(), 1);
        assert!(scheduler.tasks.read().await.contains_key("00000000"));
    }

    #[tokio::test]
    async fn public_session_api_resolves_same_registry_after_trait_coercion() {
        let fs = MemFs::with(TASKS_PATH, r#"{"tasks":[]}"#);
        let concrete_registry = registry(fs.clone());
        let trait_registry =
            concrete_registry.clone() as Arc<dyn traits::task_registry::TaskRegistryHandle>;
        assert_eq!(
            super::task_registry_identity(&concrete_registry),
            super::task_registry_identity(&trait_registry)
        );
        let scheduler = Arc::new(scheduler(
            concrete_registry.clone(),
            fs,
            FixedClock::at_secs(NOW),
        ));
        let key = super::task_registry_identity(&concrete_registry);
        super::LIVE_SCHEDULERS
            .lock()
            .unwrap()
            .insert(key, Arc::downgrade(&scheduler));

        super::register_live_job(
            &trait_registry,
            SessionCronTask {
                id: "dpublic01".into(),
                cron: "0 9 * * *".into(),
                prompt: "visible".into(),
                created_at: SystemTime::UNIX_EPOCH,
                last_fired_at: None,
                recurring: true,
                owner: None,
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(super::session_jobs(&trait_registry).await.unwrap().len(), 1);
        assert!(
            super::unregister_live_job(&trait_registry, "dpublic01", None)
                .await
                .unwrap()
        );
        super::LIVE_SCHEDULERS.lock().unwrap().remove(&key);
    }
}
