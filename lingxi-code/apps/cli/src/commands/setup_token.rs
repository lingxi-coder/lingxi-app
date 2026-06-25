//! `lingxi-cli setup_token` — Set up a long-lived authentication token
//!
//! STUB: the clap surface + dispatch are wired; the full byte-faithful
//! children/options and real backing are filled in by the per-family
//! implementation pass. Until then a recognised subcommand prints a
//! not-yet-implemented notice and exits `NOT_IMPLEMENTED` — it never starts a
//! billable chat turn (the historical mis-dispatch this layer fixes).

use clap::Args;

/// `setup_token` args (stub — accepts any trailing tokens so `--help` works and
/// children don't hard-error before the real surface lands).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Subcommand + options (filled in by the implementation pass).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub rest: Vec<String>,
}

/// Run the `setup_token` family. STUB.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!("lingxi-cli setup_token: not yet implemented");
    crate::exit_codes::NOT_IMPLEMENTED
}
