//! Library surface of the `lingxi-cli` crate.
//!
//! Integration tests import the public API instead of shelling out to the
//! binary so they don't need the workspace target dir hot.
//!
//! M5-12 Tasks 1..14: progressively wire clap argv → cwd → logging →
//! orchestrator pipeline → one-shot dispatch → output sinks → resume.

#![forbid(unsafe_code)]

pub mod argv;
pub mod cwd;
pub mod exit_codes;

use crate::argv::Argv;
use clap::error::ErrorKind;
use std::ffi::OsString;

/// Top-level entrypoint. Returns the process exit code.
///
/// Detailed Task 3..10 implementations fold on top of this.
#[allow(clippy::unused_async)] // becomes async-effective in Task 5+
pub async fn run_cli(args: Vec<OsString>) -> i32 {
    let parsed = match Argv::from_iter(args) {
        Ok(a) => a,
        Err(e) => {
            // clap prints its own help/usage to stdout/stderr; we just
            // return the locked code. Help/version are not errors.
            e.print().ok();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit_codes::SUCCESS,
                _ => exit_codes::ARGV_ERROR,
            };
        }
    };

    if let Err(e) = cwd::apply_cwd(parsed.cwd.as_deref()) {
        eprintln!("lingxi-cli: {e}");
        return exit_codes::RUNTIME_ERROR;
    }

    if parsed.is_repl_mode() {
        eprintln!("lingxi-cli: REPL mode not yet wired (M5-13)");
        return exit_codes::NOT_IMPLEMENTED;
    }

    // One-shot / resume paths wired in Tasks 5..8.
    eprintln!("lingxi-cli: one-shot dispatch not yet wired (M5-12 Task 5+)");
    exit_codes::NOT_IMPLEMENTED
}
