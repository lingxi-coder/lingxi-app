//! `lingxi-cli auth` — Manage authentication
//!
//! Byte-faithful clap surface for the `auth` family (vs claude-code 2.1.191):
//! `login` / `logout` / `status`. A bare `lingxi-cli auth` prints help.
//!
//! Backing (verified against the live crates):
//! - `status` is REAL: it reads the platform credential store (the same
//!   `secure_storage_for_platform` + `CredentialManager` the desktop runtime
//!   uses) plus `ANTHROPIC_API_KEY` and prints claude's status JSON shape
//!   (`loggedIn` / `authMethod` / `apiProvider` …). Exit `0` if authenticated,
//!   `1` otherwise — matching `claude auth status`.
//! - `logout` is REAL for the local-clear path: it deletes the persisted
//!   Anthropic + OpenAI OAuth credentials via `CredentialManager`, then prints
//!   claude's success line.
//! - `login` drives the same browser-PKCE client and secure credential store as
//!   interactive `/login`, including managed login-method and org policy.

use clap::{Args, Subcommand};

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};
use engine::settings::enterprise::{ForceLoginMethod, ForceLoginOrgPin, OrgMembershipCheck};
use llm_client::oauth::anthropic::client::ClaudeAiOAuthClient;
use llm_client::oauth::anthropic::config::ClaudeAiOAuthConfig;
use llm_client::oauth::anthropic::handle::{OAuthHandle, OAuthLoginOptions};
use std::sync::Arc;
use traits::AuthHandle;

/// `auth` — Manage authentication.
///
/// A bare `lingxi-cli auth` (no child) prints help, matching claude's
/// commander behaviour for a parent with subcommands.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Authentication subcommand. `Option` so the bare parent prints help
    /// instead of erroring.
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// `auth` children — byte-faithful with `claude auth --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Sign in to your Anthropic account
    Login(LoginArgs),
    /// Log out from your Anthropic account
    Logout,
    /// Show authentication status
    Status(StatusArgs),
}

/// `auth login` options — byte-faithful with `claude auth login --help`.
#[derive(Debug, Clone, Args)]
pub struct LoginArgs {
    /// Use Claude subscription (default)
    #[arg(long)]
    pub claudeai: bool,

    /// Use Anthropic Console (API usage billing) instead of Claude subscription
    #[arg(long)]
    pub console: bool,

    /// Pre-populate email address on the login page
    #[arg(long, value_name = "email")]
    pub email: Option<String>,

    /// Force SSO login flow
    #[arg(long)]
    pub sso: bool,
}

/// `auth status` options — byte-faithful with `claude auth status --help`.
#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    /// Output as JSON (default)
    #[arg(long)]
    pub json: bool,

    /// Output as human-readable text
    #[arg(long)]
    pub text: bool,
}

/// Run the `auth` family.
pub async fn run(cli: &Cli) -> i32 {
    match &cli.command {
        // Bare `lingxi-cli auth` → print help (commander parity).
        None => {
            print_family_help();
            SUCCESS
        }
        Some(Sub::Login(args)) => run_login(args).await,
        Some(Sub::Logout) => run_logout().await,
        Some(Sub::Status(args)) => run_status(args).await,
    }
}

/// `lingxi-cli auth` (no child) — mirror commander's "print help, exit 0".
fn print_family_help() {
    println!("Usage: lingxi-cli auth [options] [command]");
    println!();
    println!("Manage authentication");
    println!();
    println!("Options:");
    println!("  -h, --help        Display help for command");
    println!();
    println!("Commands:");
    println!("  help [command]    display help for command");
    println!("  login [options]   Sign in to your Anthropic account");
    println!("  logout            Log out from your Anthropic account");
    println!("  status [options]  Show authentication status");
}

// ── login ──────────────────────────────────────────────────────────────────

/// `lingxi-cli auth login` — run the browser-PKCE flow without starting a chat
/// turn.
/// Parse a pasted manual auth code: `"<code>#<state>"`.
///
/// Oracle: `let [m,g] = f.trim().split("#"); if (!m || !g) -> invalid`. Both
/// halves must be non-empty, so `"abc#"`, `"#xyz"` and a bare `"abc"` are all
/// rejected. JS `split` keeps everything after the second `#` in later
/// elements and only the first two are read, so `a#b#c` takes `a` and `b` —
/// reproduced here with `splitn(2)` on the trimmed input... except that JS
/// would give `g = "b"` while a naive `split_once` yields `"b#c"`. Use the
/// first two segments explicitly.
fn parse_manual_auth_code(line: &str) -> Option<(String, String)> {
    let mut parts = line.trim().split('#');
    let code = parts.next().unwrap_or_default();
    let state = parts.next().unwrap_or_default();
    if code.is_empty() || state.is_empty() {
        return None;
    }
    Some((code.to_string(), state.to_string()))
}

async fn run_login(args: &LoginArgs) -> i32 {
    // claude: "Error: --console and --claudeai cannot be used together." (exit 1)
    if args.console && args.claudeai {
        eprintln!("Error: --console and --claudeai cannot be used together.");
        return RUNTIME_ERROR;
    }
    let forced_method = effective_force_login_method().await;
    if forced_method == Some(ForceLoginMethod::Gateway) {
        eprintln!(
            "{}",
            engine::settings::enterprise::GATEWAY_NONINTERACTIVE_LOCKOUT
        );
        return RUNTIME_ERROR;
    }
    let use_console = match forced_method {
        Some(ForceLoginMethod::Console) => true,
        Some(ForceLoginMethod::ClaudeAi) => false,
        Some(ForceLoginMethod::Gateway) => unreachable!(),
        None => args.console,
    };
    let org_pin = engine_desktop::managed_force_login_org_pin().await;
    let org_uuid = match &org_pin {
        ForceLoginOrgPin::Pinned(ids) if ids.len() == 1 => ids.first().cloned(),
        _ => None,
    };
    let auth = match build_oauth_handle(use_console).await {
        Ok(auth) => auth,
        Err(error) => {
            eprintln!("Login failed: {error}");
            return RUNTIME_ERROR;
        }
    };

    // Manual fallback (oracle @246875900): a readline over stdin accepts
    // `code#state` pasted from the manual redirect page, racing the loopback
    // listener. Without it, a user whose browser cannot reach the loopback
    // (headless box, SSH session, locked-down desktop) had NO way to finish
    // login — the flow simply timed out after 300s.
    let (manual_tx, manual_rx) = tokio::sync::oneshot::channel::<(String, String)>();
    let stdin_reader = tokio::task::spawn_blocking(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut tx = Some(manual_tx);
        for line in stdin.lock().lines().map_while(Result::ok) {
            match parse_manual_auth_code(&line) {
                Some((code, state)) => {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send((code, state));
                    }
                    return;
                }
                // Byte-exact oracle text, on stderr, and the loop CONTINUES —
                // a typo must not end the login attempt.
                None => eprintln!("Invalid code. Please make sure the full code was copied."),
            }
        }
    });

    let login = auth.login_with_options(OAuthLoginOptions {
        login_hint: args.email.clone(),
        sso: args.sso,
        org_uuid,
        on_authorize_url: Some(std::sync::Arc::new(|url: &str| {
            // All three lines print here, in the oracle's order.
            println!("Opening browser to sign in\u{2026}");
            println!("If the browser didn't open, visit: {url}");
            print!("Paste code here if prompted > ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        })),
        manual_code: Some(manual_rx),
    });
    let info = match tokio::time::timeout(std::time::Duration::from_secs(300), login).await {
        Ok(Ok(info)) => info,
        Ok(Err(error)) => {
            eprintln!("Login failed: {error}");
            return RUNTIME_ERROR;
        }
        Err(_) => {
            eprintln!("Login failed: OAuth flow timed out.");
            return RUNTIME_ERROR;
        }
    };

    // The flow is done; stop reading stdin (oracle `p.close()` in `finally`).
    stdin_reader.abort();

    match engine::settings::enterprise::check_org_membership(
        &org_pin,
        std::slice::from_ref(&info.org_id),
    ) {
        OrgMembershipCheck::Permitted => {
            println!("Login successful.");
            SUCCESS
        }
        OrgMembershipCheck::Denied(message) => {
            let _ = auth.logout().await;
            eprintln!("{message}");
            RUNTIME_ERROR
        }
    }
}

// ── logout ───────────────────────────────────────────────────────────────────

/// `lingxi-cli auth logout` — clear the persisted OAuth credentials from the
/// platform secure store. Matches claude's success / failure lines.
async fn run_logout() -> i32 {
    let manager = match build_credential_manager().await {
        Ok(m) => m,
        Err(_) => {
            // claude prints this on any logout failure (incl. storage init).
            eprintln!("Failed to log out.");
            return RUNTIME_ERROR;
        }
    };

    // Clear both the Anthropic and the OpenAI/ChatGPT OAuth credential sets.
    // Each delete is idempotent (deleting a missing entry is not an error).
    if manager.delete_oauth_tokens().await.is_err()
        || manager.delete_openai_oauth_tokens().await.is_err()
    {
        eprintln!("Failed to log out.");
        return RUNTIME_ERROR;
    }

    println!("Successfully logged out from your Anthropic account.");
    SUCCESS
}

// ── status ───────────────────────────────────────────────────────────────────

/// `lingxi-cli auth status` — read the credential store + `ANTHROPIC_API_KEY`
/// and print the status. JSON is the default; `--text` is human-readable.
///
/// Exit `0` when authenticated, `1` otherwise — matching `claude auth status`.
async fn run_status(args: &StatusArgs) -> i32 {
    // `ANTHROPIC_API_KEY` env var (claude additionally suppresses this on its
    // managed homespace; that managed environment does not apply here).
    let has_api_key_env = std::env::var("ANTHROPIC_API_KEY")
        .map(|v| !v.is_empty())
        .unwrap_or(false);

    // Read persisted credentials from the platform secure store. One manager
    // serves both the OAuth-token and the stored-API-key probes.
    let (has_oauth, oauth_email, oauth_org, has_stored_api_key) =
        match build_credential_manager().await {
            Ok(manager) => {
                let (has_oauth, email, org) = match manager.get_oauth_tokens().await {
                    Ok(Some(tokens)) => (true, Some(tokens.email), Some(tokens.org_id)),
                    _ => (false, None, None),
                };
                // A stored Anthropic API key (config / managed `/login` key) also
                // counts as a key source for `apiKeySource`.
                let has_stored_key = manager
                    .get_anthropic_api_key()
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                (has_oauth, email, org, has_stored_key)
            }
            Err(_) => (false, None, None, false),
        };

    let logged_in = has_oauth || has_api_key_env || has_stored_api_key;

    // Determine the auth method, mirroring claude's precedence.
    let auth_method: &str = if has_oauth {
        "claude.ai"
    } else if has_api_key_env {
        "api_key"
    } else if has_stored_api_key {
        // A managed `/login` key is reported as the claude.ai method by claude.
        "claude.ai"
    } else {
        "none"
    };

    let api_provider = resolve_api_provider();

    if args.text {
        print_status_text(
            logged_in,
            auth_method,
            has_api_key_env,
            oauth_email.as_deref(),
            oauth_org.as_deref(),
        );
    } else {
        print_status_json(
            logged_in,
            auth_method,
            api_provider,
            has_api_key_env,
            has_stored_api_key,
            oauth_email.as_deref(),
            oauth_org.as_deref(),
        );
    }

    if logged_in {
        SUCCESS
    } else {
        RUNTIME_ERROR
    }
}

/// JSON status output — claude's `jsonStringify(output, null, 2)` shape.
#[allow(clippy::too_many_arguments)]
fn print_status_json(
    logged_in: bool,
    auth_method: &str,
    api_provider: &str,
    has_api_key_env: bool,
    has_stored_api_key: bool,
    oauth_email: Option<&str>,
    oauth_org: Option<&str>,
) {
    // Preserve claude's key ordering: loggedIn, authMethod, apiProvider,
    // [apiKeySource], then the claude.ai block (email/orgId/orgName/subscriptionType).
    let mut map = serde_json::Map::new();
    map.insert("loggedIn".to_string(), serde_json::Value::Bool(logged_in));
    map.insert(
        "authMethod".to_string(),
        serde_json::Value::String(auth_method.to_string()),
    );
    map.insert(
        "apiProvider".to_string(),
        serde_json::Value::String(api_provider.to_string()),
    );

    // apiKeySource: the stored key source first, else the env var, else absent.
    let api_key_source: Option<&str> = if has_stored_api_key {
        Some("/login managed key")
    } else if has_api_key_env {
        Some("ANTHROPIC_API_KEY")
    } else {
        None
    };
    if let Some(src) = api_key_source {
        map.insert(
            "apiKeySource".to_string(),
            serde_json::Value::String(src.to_string()),
        );
    }

    if auth_method == "claude.ai" {
        map.insert(
            "email".to_string(),
            oauth_email
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );
        map.insert(
            "orgId".to_string(),
            oauth_org
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null),
        );
        // orgName / subscriptionType live in global config metadata that this
        // thin entry point does not load; claude emits `?? null` for absent.
        map.insert("orgName".to_string(), serde_json::Value::Null);
        map.insert("subscriptionType".to_string(), serde_json::Value::Null);
    }

    let value = serde_json::Value::Object(map);
    // Two-space pretty print, matching claude's `jsonStringify(_, null, 2)`.
    match serde_json::to_string_pretty(&value) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{{}}"),
    }
}

/// Human-readable status output — claude's `--text` form.
fn print_status_text(
    logged_in: bool,
    auth_method: &str,
    has_api_key_env: bool,
    oauth_email: Option<&str>,
    oauth_org: Option<&str>,
) {
    let mut printed_any = false;

    if auth_method == "claude.ai" {
        if let Some(email) = oauth_email {
            println!("Email: {email}");
            printed_any = true;
        }
        if let Some(org) = oauth_org {
            println!("Organization: {org}");
            printed_any = true;
        }
    }

    if !printed_any && has_api_key_env {
        // claude: prints "API key: ANTHROPIC_API_KEY" when no account property
        // is available but the env key is set.
        println!("API key: ANTHROPIC_API_KEY");
        printed_any = true;
    }

    if !logged_in {
        // claude's exact not-logged-in line (branding kept as lingxi-cli).
        println!("Not logged in. Run lingxi-cli auth login to authenticate.");
    }
    // No `Authenticated.` fallback: claude's `authStatus` text branch emits only
    // surfaced properties, the optional `API key:` line, and (when not logged in)
    // the not-logged-in line — nothing extra when logged in with no property.
    let _ = printed_any;
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Build a `CredentialManager` over the platform secure store, addressing the
/// SAME `~/.claude` (or `$LINGXI_CONFIG_DIR`) location the desktop runtime and
/// `/login` write to, so `status` / `logout` observe real credentials.
async fn build_credential_manager() -> Result<secret::CredentialManager, anyhow::Error> {
    let lingxi_home = crate::run::lingxi_home_dir();
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    let storage = platform_posix::secure_storage_for_platform(
        user,
        lingxi_home.clone(),
        lingxi_home.join(".credentials.json"),
    )
    .await
    .map_err(|e| anyhow::anyhow!("secure storage: {e}"))?;

    let clock = std::sync::Arc::new(platform_posix::PosixClock::new());
    let http = std::sync::Arc::new(platform_posix::PosixHttp::new());

    Ok(secret::CredentialManager::new(storage, clock, http))
}

pub(crate) async fn build_oauth_handle(use_console: bool) -> Result<OAuthHandle, anyhow::Error> {
    let credentials = Arc::new(build_credential_manager().await?);
    let http: Arc<dyn traits::HttpTransport> = Arc::new(platform_posix::PosixHttp::new());
    let config = if use_console {
        ClaudeAiOAuthConfig::console_with_port(0)
    } else {
        ClaudeAiOAuthConfig::default_with_port(0)
    };
    let client = Arc::new(ClaudeAiOAuthClient::new(config, http, credentials));
    Ok(OAuthHandle::new(client))
}

pub(crate) async fn effective_force_login_method() -> Option<ForceLoginMethod> {
    let mut method = None;
    for raw in engine_desktop::settings_watch::managed_settings_raw_tiers().await {
        let Ok(settings) = serde_json::from_str::<engine::settings::schema::SettingsJson>(&raw)
        else {
            continue;
        };
        if settings.force_login_method.is_some() {
            method = settings.force_login_method_parsed();
        }
    }
    method
}

/// Resolve the API provider label, mirroring claude's `getAPIProvider()`:
/// `bedrock` / `vertex` / `foundry` / `firstParty` selected by env vars.
fn resolve_api_provider() -> &'static str {
    if env_truthy("CLAUDE_CODE_USE_BEDROCK") {
        "bedrock"
    } else if env_truthy("CLAUDE_CODE_USE_VERTEX") {
        "vertex"
    } else if env_truthy("CLAUDE_CODE_USE_FOUNDRY") {
        "foundry"
    } else {
        "firstParty"
    }
}

/// claude's `isEnvTruthy`: a value is truthy iff (case-insensitive, trimmed)
/// it is one of `1` / `true` / `yes` / `on`. Empty / unset is falsy.
fn env_truthy(var: &str) -> bool {
    match std::env::var(var) {
        Ok(v) => {
            let normalized = v.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod manual_auth_code_tests {
    /// Oracle: `let [m,g]=f.trim().split("#"); if(!m||!g) -> invalid`.
    #[test]
    fn a_well_formed_manual_code_parses() {
        assert_eq!(
            super::parse_manual_auth_code("abc123#state456"),
            Some(("abc123".into(), "state456".into()))
        );
        // Surrounding whitespace is trimmed before splitting.
        assert_eq!(
            super::parse_manual_auth_code("  abc#st  \n"),
            Some(("abc".into(), "st".into()))
        );
    }

    #[test]
    fn a_missing_half_is_rejected() {
        for line in ["abc", "abc#", "#state", "", "   ", "#"] {
            assert_eq!(
                super::parse_manual_auth_code(line),
                None,
                "{line:?} must be rejected"
            );
        }
    }

    /// JS `split("#")` then reading only the first two elements: `a#b#c`
    /// yields `("a","b")`, NOT `("a","b#c")` — a `split_once` would get this
    /// wrong and exchange a state the server never issued.
    #[test]
    fn extra_hashes_take_only_the_first_two_segments() {
        assert_eq!(
            super::parse_manual_auth_code("a#b#c"),
            Some(("a".into(), "b".into()))
        );
    }
}
