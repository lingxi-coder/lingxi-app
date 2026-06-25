//! `lingxi-cli ultrareview` — Run a cloud-hosted multi-agent code review
//!
//! claude-code's `ultrareview` ships the current branch (or a PR number / base
//! branch) to an Anthropic-operated CLOUD reviewer that fans out a swarm of
//! review agents and returns a `bugs.json` payload, which the CLI then renders
//! as formatted findings (or, with `--json`, prints raw).
//!
//! lingxi-cli does NOT run that cloud backend, so this command parses its
//! surface (`[target]`, `--json`, `--timeout`) byte-faithfully and then prints
//! a clear notice that the cloud reviewer is unavailable here, pointing the user
//! at the local equivalent — the interactive `/code-review ultra` slash command.
//! It NEVER fabricates findings and NEVER starts a billable LLM turn.

use clap::Args;

/// `ultrareview` args — byte-faithful with `claude ultrareview --help`:
/// positional `[target]` (PR number / base branch), `--json`, and
/// `--timeout <minutes>` (default 30).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// PR number or base branch to review (defaults to the current branch).
    #[arg(value_name = "target")]
    pub target: Option<String>,

    /// Print the raw bugs.json payload instead of formatted findings
    #[arg(long)]
    pub json: bool,

    /// Maximum minutes to wait for the review to finish
    #[arg(long, value_name = "minutes", default_value_t = 30)]
    pub timeout: u64,
}

/// Run the `ultrareview` family.
///
/// The cloud-hosted reviewer is Anthropic-operated and not part of lingxi-cli,
/// so we report it as unavailable and return `NOT_IMPLEMENTED` rather than
/// faking a review or contacting the network.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!(
        "lingxi-cli ultrareview: the cloud-hosted multi-agent reviewer is not \
         available in lingxi-cli."
    );
    eprintln!(
        "Use the local equivalent instead: the interactive `/code-review ultra` \
         slash command."
    );
    crate::exit_codes::NOT_IMPLEMENTED
}
