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
//! The action drives the same PKCE client as `auth login`, but requests only
//! `user:inference`, asks the token endpoint for the public one-year lifetime,
//! does not persist the token, and prints it once for headless/CI use.

use clap::Args;
use engine::settings::enterprise::{ForceLoginMethod, ForceLoginOrgPin};

/// `setup-token` payload — no children, no options beyond the clap-provided
/// `-h/--help` (matches the byte oracle exactly).
#[derive(Debug, Clone, Args)]
pub struct Cli {}

/// Run the `setup-token` family.
///
pub async fn run(_cli: &Cli) -> i32 {
    let forced_method = super::auth::effective_force_login_method().await;
    if matches!(
        forced_method,
        Some(ForceLoginMethod::Console | ForceLoginMethod::Gateway)
    ) {
        eprintln!(
            "setup-token creates a long-lived Claude.ai subscription token, which this policy does not permit."
        );
        return crate::exit_codes::RUNTIME_ERROR;
    }

    let org_pin = engine_desktop::managed_force_login_org_pin().await;
    let org_uuid = match &org_pin {
        ForceLoginOrgPin::Pinned(ids) if ids.len() == 1 => ids.first().cloned(),
        ForceLoginOrgPin::Invalid => {
            eprintln!(
                "{}",
                engine::settings::enterprise::FORCE_LOGIN_ORG_UUID_INVALID
            );
            return crate::exit_codes::RUNTIME_ERROR;
        }
        ForceLoginOrgPin::EmptyArray => {
            eprintln!(
                "{}",
                engine::settings::enterprise::FORCE_LOGIN_ORG_UUID_EMPTY_ARRAY
            );
            return crate::exit_codes::RUNTIME_ERROR;
        }
        _ => None,
    };

    let auth = match super::auth::build_oauth_handle(false).await {
        Ok(auth) => auth,
        Err(error) => {
            eprintln!("Failed to start token setup: {error}");
            return crate::exit_codes::RUNTIME_ERROR;
        }
    };

    println!(
        "This will guide you through long-lived (1-year) auth token setup for your Claude account. Claude subscription required."
    );
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        auth.mint_long_lived_token(org_uuid),
    )
    .await;
    let token = match result {
        Ok(Ok(token)) => token,
        Ok(Err(error)) => {
            eprintln!("OAuth error: {error}");
            return crate::exit_codes::RUNTIME_ERROR;
        }
        Err(_) => {
            eprintln!("OAuth error: token setup timed out.");
            return crate::exit_codes::RUNTIME_ERROR;
        }
    };

    println!("✓ Long-lived authentication token created successfully!");
    println!("Your OAuth token (valid for 1 year):");
    println!("{}", token.expose_secret());
    println!("Store this token securely. You won't be able to see it again.");
    println!("Use this token by setting: export CLAUDE_CODE_OAUTH_TOKEN=<token>");
    crate::exit_codes::SUCCESS
}
