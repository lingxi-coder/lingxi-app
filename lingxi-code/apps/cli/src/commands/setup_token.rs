//! `lingxi-cli setup-token` — Set up a long-lived authentication token
//! (requires Claude subscription).
//!
//! Byte oracle (claude-code 2.1.191):
//!
//! ```text
//! Usage: claude setup-token [options]
//!
//! Set up a long-lived authentication token (requires Claude subscription)
//!
//! Options:
//!   -h, --help  Display help for command
//! ```
//!
//! The family has NO children and NO options beyond `-h/--help`, so the clap
//! payload is a fieldless `Args` struct — anything else (e.g. a trailing
//! positional) would make our usage line diverge from the oracle's
//! `setup-token [options]`.
//!
//! The action itself is the interactive OAuth *long-lived-token mint* flow:
//! claude opens a browser to the Claude-subscription authorize endpoint, the
//! user pastes back the authorization code, and an `sk-ant-oat…` long-lived
//! token is minted and printed for use in headless/CI environments. The
//! Anthropic OAuth subsystem present in `llm-client`
//! (`oauth::anthropic::OAuthHandle`) only exposes the *interactive* PKCE
//! Authorization-Code `login` flow — it binds a loopback listener, shells a
//! real browser, and awaits a redirect callback; it does **not** expose a
//! clean, verifiable, non-interactive long-lived-token setup routine that we
//! could wire here without guessing an OAuth API. Per the conservative parity
//! policy we therefore parse the surface faithfully and emit a clear
//! not-implemented notice instead of starting an interactive flow or faking a
//! mint.
//!
//! The KEY fix this layer delivers: `setup-token` now dispatches to this
//! handler instead of being swallowed as a chat prompt — it never starts a
//! billable model turn.

use clap::Args;

/// `setup-token` payload — no children, no options beyond the clap-provided
/// `-h/--help` (matches the byte oracle exactly).
#[derive(Debug, Clone, Args)]
pub struct Cli {}

/// Run the `setup-token` family.
///
/// Minting a long-lived Claude-subscription token is an interactive browser
/// flow that is not wired here (see module docs). We print a clear notice and
/// exit `NOT_IMPLEMENTED` — never a chat turn, never a faked success.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!("lingxi-cli setup-token: not yet implemented");
    crate::exit_codes::NOT_IMPLEMENTED
}
