//! `lingxi-cli install` — Install LingXi native build
//!
//! NOT-APPLICABLE to lingxi-cli. claude-code's `install` drives Anthropic's
//! native-binary auto-installer (downloads + swaps a prebuilt `claude`
//! executable for a given channel/version). `lingxi-cli` is built from source
//! via cargo and ships no native auto-installer, so there is nothing to
//! install at runtime.
//!
//! We still parse the full byte-faithful surface (`[target]` positional +
//! `--force`) so `install --help` matches claude-code exactly and the dispatch
//! is correct, then print a clear message and return `NOT_IMPLEMENTED`. We
//! never fake an install and never start a billable chat turn (the historical
//! mis-dispatch this layer fixes).

use clap::Args;

/// `install` args — byte-parity with `claude install [options] [target]`.
///
/// claude-code:
/// ```text
/// Usage: claude install [options] [target]
/// Options:
///   --force     Force installation even if already installed
/// ```
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Force installation even if already installed
    #[arg(long)]
    pub force: bool,

    /// Version to install (stable, latest, or a specific version).
    #[arg(value_name = "target")]
    pub target: Option<String>,
}

/// Run the `install` family.
///
/// NOT-APPLICABLE: `lingxi-cli` is built from source and has no native
/// auto-installer to drive. We parse the request faithfully, explain why there
/// is nothing to do, and return `NOT_IMPLEMENTED` — without faking an install.
pub async fn run(cli: &Cli) -> i32 {
    let target = cli.target.as_deref().unwrap_or("stable");
    eprintln!(
        "lingxi-cli install: not applicable — lingxi-cli is built from source \
         (cargo) and has no native auto-installer to drive."
    );
    eprintln!(
        "  (requested target: {target}{force}). To update, rebuild from source.",
        force = if cli.force { ", --force" } else { "" },
    );
    crate::exit_codes::NOT_IMPLEMENTED
}
