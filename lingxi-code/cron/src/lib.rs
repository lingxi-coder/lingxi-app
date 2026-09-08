//! Cron scheduler subsystem (spec §28).
//!
//! Provides four modules:
//! - [`schedule`] — 5-field cron expression parser + minute-boundary matcher.
//! - [`lock`] — cross-process file lock with PID-liveness detection (A9).
//! - [`scheduler`] — tick loop wired to [`tasks::TaskRegistry`].
//! - [`tasks_file`] — single-file `scheduled_tasks.json` persistence shape
//!   (1:1 with claude-code `cronTasks.ts`), shared by the tools and scheduler.
//!
//! Unsafe code is permitted in this crate (workspace override) because
//! [`scheduler::pid_alive_check`] uses platform FFI (`libc::kill` on Unix,
//! `OpenProcess` on Windows) to detect stale lockholders. The unsafe block is
//! isolated to that single function.

pub mod autonomous_loop;
pub mod lock;
pub mod run_due;
pub mod schedule;
pub mod scheduler;
pub mod tasks_file;

// `autonomous_loop` was relocated here from `tool-cron` to satisfy §8.1
// dependency layering (command / tool crates may depend on this root-level
// `cron` crate but not on the `tool-cron` tool crate). The re-exports below
// mirror `tool-cron`'s crate-root API so `cron::<Symbol>` resolves for every
// symbol the module previously exposed.
pub use autonomous_loop::{
    begin_loop_tick, get_autonomous_loop_preamble, is_autonomous_loop_sentinel,
    is_loop_default_prompt_enabled, is_loop_default_sentinel, is_loop_dynamic_enabled,
    forget_dynamic_loop, is_loop_file_sentinel, is_loop_keepalive_enabled, is_push_notif_enabled,
    log_autonomous_loop_activation, loop_consecutive_keepalives, loop_tick_in_flight_prompt,
    mark_loop_rescheduled, note_loop_invoked, read_loop_file, reset_autonomous_loop_delivered,
    reset_loop_runtime_state, resolve_autonomous_loop_fire, resolve_loop_default_fire,
    resolve_loop_file_fire, set_loop_consecutive_keepalives, take_loop_rescheduled,
    take_loop_tick_in_flight_prompt, LoopFile, LoopFoldOutcome, LoopFoldVeto, LoopRuntime,
    AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
    AUTONOMOUS_LOOP_PREAMBLE, AUTONOMOUS_LOOP_SENTINEL, LOOP_FILE_DYNAMIC_SENTINEL,
    LOOP_FILE_SENTINEL,
};
pub use lock::{release_lock, try_acquire_lock, CronLockError, LockRecord};
pub use run_due::{
    default_recurring_max_age, lock_cron_file, next_fire_epoch_ms, next_fire_epoch_ms_for_task,
    run_due_jobs, CronJobFirer, FireStatus, FiredJob,
};
pub use schedule::{
    human_schedule, iso_8601_utc, local_date_time_string, parse_cron, short_local_timestamp,
    CronExpression, CronField, CronParseError,
};
pub use scheduler::{
    register_live_job, session_jobs, task_registry_identity, unregister_live_job, CronScheduler,
    CronTaskDef, NoActiveCronScheduler, SessionCronTask,
};
pub use tasks_file::{
    lock_scheduled_tasks, parse_tasks, project_root_from_tasks_path, read_tasks_body,
    scheduled_tasks_lock_path, scheduled_tasks_lock_relative_path, scheduled_tasks_path,
    parse_tasks_strict, scheduled_tasks_relative_path, serialize_tasks, write_tasks_body, CronTask,
    ScheduledTasks, UnmodeledTask,
};
