//! `lingxi-cli ultrareview` — Run a cloud-hosted multi-agent code review
//!
//! claude-code's `ultrareview` ships the current branch (or a PR number / base
//! branch) to an Anthropic-operated CLOUD reviewer that fans out a swarm of
//! review agents and returns a `bugs.json` payload, which the CLI then renders
//! as formatted findings (or, with `--json`, prints raw).
//!
//! lingxi-cli does NOT run that cloud backend, so this command parses its
//! surface (`[target]`, `--json`, `--timeout`, `--post`/`--no-post`)
//! byte-faithfully and then prints a clear notice that the cloud reviewer is
//! unavailable here, pointing the user at the local equivalent — the
//! interactive `/code-review ultra` slash command.
//! It NEVER fabricates findings and NEVER starts a billable LLM turn.

use clap::Args;

/// `ultrareview` args — byte-faithful with `claude ultrareview --help`:
/// positional `[target]` (PR number / base branch), `--json`,
/// `--timeout <minutes>` (default 30), and the 2.1.238 `--post` / `--no-post`
/// pair (CLI-11; both are 0-hit in 2.1.220).
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

    /// Post the finished review's findings to the PR as you (PR targets only;
    /// one plain comment, not a review)
    //
    // (CLI-11, cc2.1.238 @183737504 — verified byte-exact, 0 hits in 2.1.220.)
    // The oracle registers these as two INDEPENDENT commander options, not a
    // clap-style negation pair: `--post` is declared first, so the pair's
    // default stays `undefined` rather than flipping to `true` the way a lone
    // `--no-post` would. Commander does not reject `--post --no-post` (the
    // later assignment simply wins), so neither flag declares `conflicts_with`
    // here — the port must not hard-error on an argv the oracle accepts.
    #[arg(long = "post")]
    pub post: bool,

    /// Do not post the findings to the PR (the default; accepted for parity
    /// with the /ultrareview and /code-review ultra flags)
    #[arg(long = "no-post")]
    pub no_post: bool,
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
