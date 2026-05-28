//! Library surface of the `lingxi-cli` crate.
//!
//! Integration tests import the public API instead of shelling out to the
//! binary so they don't need the workspace target dir hot.
//!
//! M5-12 task 1: scaffold only — flag parsing + dispatch wired in later tasks.

#![forbid(unsafe_code)]

pub mod argv;
pub mod exit_codes;

use std::ffi::OsString;

/// Top-level entrypoint. Returns the process exit code.
///
/// Detailed Task 2..10 implementations build on this stub.
#[allow(clippy::unused_async)] // becomes async-effective once Task 5+ wire the orchestrator
pub async fn run_cli(_args: Vec<OsString>) -> i32 {
    // Placeholder until Task 2 lands clap parsing.
    eprintln!("lingxi-cli: scaffold (M5-12 Task 1) — flag parsing not yet wired");
    exit_codes::NOT_IMPLEMENTED
}
