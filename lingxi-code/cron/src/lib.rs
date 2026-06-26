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

pub mod lock;
pub mod run_due;
pub mod schedule;
pub mod scheduler;
pub mod tasks_file;

pub use lock::{release_lock, try_acquire_lock, CronLockError, LockRecord};
pub use run_due::{
    default_recurring_max_age, lock_cron_file, next_fire_epoch_ms, run_due_jobs, CronJobFirer,
    FireStatus, FiredJob,
};
pub use schedule::{parse_cron, CronExpression, CronField, CronParseError};
pub use scheduler::{CronScheduler, CronTaskDef};
pub use tasks_file::{
    parse_tasks, scheduled_tasks_lock_path, scheduled_tasks_path, serialize_tasks, CronTask,
    ScheduledTasks,
};
