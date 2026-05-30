//! Cron scheduler subsystem (spec §28).
//!
//! Provides three modules:
//! - [`schedule`] — 5-field cron expression parser + minute-boundary matcher.
//! - [`lock`] — cross-process file lock with PID-liveness detection (A9).
//! - [`scheduler`] — tick loop wired to [`lingxi_tasks::TaskRegistry`].
//!
//! Unsafe code is permitted in this crate (workspace override) because
//! [`scheduler::pid_alive_check`] uses platform FFI (`libc::kill` on Unix,
//! `OpenProcess` on Windows) to detect stale lockholders. The unsafe block is
//! isolated to that single function.

pub mod lock;
pub mod schedule;
pub mod scheduler;

pub use lock::{release_lock, try_acquire_lock, CronLockError, LockRecord};
pub use schedule::{parse_cron, CronExpression, CronField, CronParseError};
pub use scheduler::{CronScheduler, CronTaskDef};
