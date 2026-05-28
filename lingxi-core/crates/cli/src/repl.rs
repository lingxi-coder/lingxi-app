//! M5-13 REPL stub. Replaced wholesale by M5-13 Task 5 with the real
//! read-line loop. For M5-12 the entrypoint prints a friendly stderr
//! marker and returns [`crate::exit_codes::NOT_IMPLEMENTED`].

use crate::argv::Argv;
use crate::exit_codes;

/// REPL entry point. Currently a stub.
#[allow(clippy::unused_async)] // becomes async in M5-13
pub async fn run_repl(_argv: &Argv) -> i32 {
    eprintln!("lingxi-cli: REPL mode not yet wired (M5-13)");
    exit_codes::NOT_IMPLEMENTED
}
