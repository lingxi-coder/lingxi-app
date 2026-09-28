//! `lingxi-cli update` (alias `upgrade`) — Check for updates and install if
//! available.
//!
//! claude-code's `update` drives the **native auto-updater** (downloads and
//! swaps the prebuilt npm/native binary in place). `lingxi-cli` is built from
//! source, so there is no managed binary to swap and no auto-update channel.
//! We keep the byte-faithful clap surface (no children, no options beyond the
//! clap-injected `-h, --help`), then print a clear "built from source" notice
//! and exit `NOT_IMPLEMENTED` — we never hit the network or fake a success.

use clap::Args;

/// `update` args — no children or options beyond the clap-injected
/// `-h, --help`, matching `claude update|upgrade`.
#[derive(Debug, Clone, Args)]
pub struct Cli {}

/// Run the `update` family.
///
/// Auto-update is NOT-APPLICABLE to a source build of `lingxi-cli`: there is no
/// managed/prebuilt binary to replace. Print a clear message and return
/// `NOT_IMPLEMENTED`.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!(
        "lingxi-cli was built from source; there is no auto-update channel to check or install from."
    );
    eprintln!("To update, pull the latest changes and rebuild (e.g. `git pull && cargo build --release`).");
    crate::exit_codes::NOT_IMPLEMENTED
}
