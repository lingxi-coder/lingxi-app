//! Host-local provider account administration. Authentication material is accepted
//! only through stdin or an exact secure-store reference, never argv or output.
use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};
use clap::{Args, Subcommand};
use harness_runtime::provider_accounts::{
    account_store_scope, AccountOAuthCoordinator, ProviderAccountStore,
};
use lingxi_core::host::provider_accounts::{
    AccountAuthKind, AccountSnapshot, CredentialRef, ProviderAccountAction, ProviderAccountCommand,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Args)]
pub struct Cli {
    #[command(subcommand)]
    pub command: ProviderSub,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ProviderSub {
    /// Manage accounts in this host's credential storage domain
    Accounts(AccountsCli),
}

#[derive(Debug, Clone, Args)]
pub struct AccountsCli {
    /// Emit a secret-free machine-readable response
    #[arg(long, global = true)]
    pub json: bool,
    /// Reject a mutation if another process changed the account configuration
    #[arg(long, global = true)]
    pub expected_revision: Option<u64>,
    #[command(subcommand)]
    pub command: AccountSub,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AccountSub {
    /// List accounts in their persistent user priority order
    List(ProviderArgs),
    /// Add an API key account using stdin or an existing secure-store slot
    Add(AddArgs),
    /// Add a new OAuth account
    Login(LoginArgs),
    /// Reauthorize one existing OAuth account
    Reconnect(ReconnectArgs),
    /// Persist the complete ordered list of account IDs for a provider
    Reorder(ReorderArgs),
    /// Include an account in request selection
    Enable(TargetArgs),
    /// Exclude an account from request selection
    Disable(TargetArgs),
    /// Test the selected account
    Test(TargetArgs),
    /// Clear only this account's authentication material
    Logout(TargetArgs),
    /// Remove only this account
    Remove(TargetArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ProviderArgs {
    pub provider: String,
}

#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    pub provider: String,
    #[arg(long)]
    pub name: String,
    /// Read the API key from stdin; the key is never echoed
    #[arg(
        long,
        required_unless_present = "api_key_secret_ref",
        conflicts_with = "api_key_secret_ref"
    )]
    pub api_key_stdin: bool,
    /// Read an existing opaque slot from this host's secure credential store
    #[arg(
        long,
        required_unless_present = "api_key_stdin",
        conflicts_with = "api_key_stdin"
    )]
    pub api_key_secret_ref: Option<String>,
    /// Explicit execution binding from the provider capability catalog
    #[arg(long)]
    pub execution_binding: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct LoginArgs {
    #[command(flatten)]
    pub browser: BrowserArgs,
    pub provider: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub execution_binding: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct BrowserArgs {
    /// Print the authorization URL without opening a local browser
    #[arg(long)]
    pub no_browser: bool,
    /// Read the complete callback URL from piped stdin (for remote shells)
    #[arg(long)]
    pub callback_stdin: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ReconnectArgs {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub browser: BrowserArgs,
}

#[derive(Debug, Clone, Args)]
pub struct TargetArgs {
    pub provider: String,
    pub account_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ReorderArgs {
    pub provider: String,
    #[arg(required = true, num_args = 1..)]
    pub account_ids: Vec<String>,
}

pub async fn run(cli: &Cli) -> i32 {
    let ProviderSub::Accounts(args) = &cli.command;
    match run_accounts(args).await {
        Ok(snapshot) => {
            output_snapshot(&snapshot, args.json);
            SUCCESS
        }
        Err(error) => {
            // Shared authority errors can originate in storage/network adapters.
            // Do not forward their text, which may contain a key or callback URL.
            if args.json {
                println!(
                    "{}",
                    json!({"ok":false,"error":{"code":error.code,"message":error.message}})
                );
            } else {
                eprintln!("{}: {}", error.code, error.message);
            }
            RUNTIME_ERROR
        }
    }
}

#[derive(Debug)]
struct CommandError {
    code: &'static str,
    message: &'static str,
}
impl CommandError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

async fn read_key_stdin() -> Result<String, CommandError> {
    use std::io::IsTerminal;
    use tokio::io::AsyncReadExt;
    if std::io::stdin().is_terminal() {
        return Err(CommandError::new(
            "credential_input_required",
            "Pipe the API key into stdin to avoid displaying it in the terminal.",
        ));
    }
    const MAX_KEY_BYTES: u64 = 65536;
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            CommandError::new("input_unavailable", "Could not read API key from stdin.")
        })?;
    parse_key_input(bytes)
}

fn parse_key_input(bytes: Vec<u8>) -> Result<String, CommandError> {
    if bytes.len() > 65536 {
        return Err(CommandError::new(
            "invalid_credential",
            "API key input exceeds the size limit.",
        ));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| CommandError::new("invalid_credential", "API key input must be UTF-8."))?;
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(CommandError::new(
            "invalid_credential",
            "API key input must contain one nonempty value.",
        ));
    }
    Ok(value.to_owned())
}

fn provider(args: &AccountSub) -> &str {
    match args {
        AccountSub::List(a) => &a.provider,
        AccountSub::Add(a) => &a.provider,
        AccountSub::Login(a) => &a.provider,
        AccountSub::Reorder(a) => &a.provider,
        AccountSub::Reconnect(a) => &a.target.provider,
        AccountSub::Enable(a)
        | AccountSub::Disable(a)
        | AccountSub::Test(a)
        | AccountSub::Logout(a)
        | AccountSub::Remove(a) => &a.provider,
    }
}

fn mutation(
    args: &AccountsCli,
    snapshot: &AccountSnapshot,
) -> Result<ProviderAccountCommand, CommandError> {
    let action = match &args.command {
        AccountSub::List(_) => unreachable!("list does not mutate"),
        AccountSub::Add(a) => ProviderAccountAction::Add {
            provider_id: a.provider.clone(),
            display_name: a.name.clone(),
            execution_binding_id: a
                .execution_binding
                .clone()
                .unwrap_or_else(|| default_key_binding(&a.provider)),
            auth_kind: AccountAuthKind::ApiKey,
            account_id: None,
        },
        AccountSub::Login(a) => ProviderAccountAction::Add {
            provider_id: a.provider.clone(),
            display_name: a.name.clone(),
            execution_binding_id: a
                .execution_binding
                .clone()
                .unwrap_or_else(|| "openai-chatgpt".into()),
            auth_kind: AccountAuthKind::OAuth,
            account_id: None,
        },
        AccountSub::Reorder(a) => ProviderAccountAction::Reorder {
            provider_id: a.provider.clone(),
            account_ids: a.account_ids.clone(),
        },
        AccountSub::Reconnect(a) => {
            if !snapshot.accounts.iter().any(|account| {
                account.account_id == a.target.account_id
                    && account.provider_id == a.target.provider
            }) {
                return Err(CommandError::new(
                    "account_not_found",
                    "The account does not belong to the requested provider.",
                ));
            }
            ProviderAccountAction::BeginOauth {
                account_id: a.target.account_id.clone(),
            }
        }
        AccountSub::Enable(a)
        | AccountSub::Disable(a)
        | AccountSub::Test(a)
        | AccountSub::Logout(a)
        | AccountSub::Remove(a) => {
            // A mistyped provider must never operate on an account from another provider.
            if !snapshot.accounts.iter().any(|account| {
                account.account_id == a.account_id && account.provider_id == a.provider
            }) {
                return Err(CommandError::new(
                    "account_not_found",
                    "The account does not belong to the requested provider.",
                ));
            }
            let account_id = a.account_id.clone();
            match &args.command {
                AccountSub::Enable(_) => ProviderAccountAction::SetEnabled {
                    account_id,
                    enabled: true,
                },
                AccountSub::Disable(_) => ProviderAccountAction::SetEnabled {
                    account_id,
                    enabled: false,
                },
                AccountSub::Test(_) => ProviderAccountAction::Test { account_id },
                AccountSub::Logout(_) => ProviderAccountAction::Logout { account_id },
                AccountSub::Remove(_) => ProviderAccountAction::Remove { account_id },
                _ => unreachable!(),
            }
        }
    };
    Ok(ProviderAccountCommand {
        expected_revision: args.expected_revision.unwrap_or(snapshot.config_revision),
        operation_id: None,
        action,
    })
}

fn default_key_binding(provider: &str) -> String {
    match provider {
        "anthropic" => "anthropic-api-key".into(),
        other => other.into(),
    }
}

fn authority_error(error: String) -> CommandError {
    // The engine uses sanitized local messages. Keep this boundary explicit so
    // future storage/network error wording cannot expose callback or key data.
    if error.contains("upstream account is already registered") {
        CommandError::new("duplicate_account", "This upstream identity is already registered on this host. Reconnect that account instead.")
    } else if error.contains("different upstream account") || error.contains("different workspace")
    {
        CommandError::new("oauth_subject_mismatch", "The selected identity differs from this account. Add a new account to use that identity.")
    } else if error.contains("revision")
        || error.contains("configuration conflict")
        || error.contains("changed during authorization")
    {
        CommandError::new(
            "revision_conflict",
            "Account configuration changed. Read a fresh snapshot and retry.",
        )
    } else if error.contains("authentication rejected") || error.contains("requires authentication")
    {
        CommandError::new("needs_auth", "This account requires authentication.")
    } else if error.contains("rate limited") {
        CommandError::new(
            "rate_limited",
            "This account is rate limited. Its recovery time is unknown.",
        )
    } else if error.contains("unknown account") || error.contains("account was removed") {
        CommandError::new(
            "account_not_found",
            "The account no longer exists on this host.",
        )
    } else if error.contains("state or destination") || error.contains("invalid OAuth callback") {
        CommandError::new(
            "oauth_callback_rejected",
            "The authorization callback does not match this flow.",
        )
    } else if error.contains("flow expired") {
        CommandError::new(
            "oauth_timeout",
            "Authorization timed out. Start a new flow.",
        )
    } else if error.contains("refresh failed") {
        CommandError::new(
            "oauth_refresh_failed",
            "The selected account could not refresh its authorization.",
        )
    } else if error.contains("exchange failed") {
        CommandError::new(
            "oauth_exchange_failed",
            "The provider could not complete authorization.",
        )
    } else if error.contains("OAuth") || error.contains("binding") {
        CommandError::new("unsupported_auth", "This authentication operation is unavailable for the selected account or execution binding.")
    } else if error.contains("storage")
        || error.contains("directory")
        || error.contains("persistence")
    {
        CommandError::new(
            "storage_unavailable",
            "This host's account or credential storage is unavailable.",
        )
    } else {
        CommandError::new(
            "account_operation_failed",
            "The host could not complete the account operation.",
        )
    }
}

fn account_profiles(
    config: &harness_runtime::desktop::DesktopConfig,
    managed: &lingxi_core::types::utf16_json::Utf16JsonProjection,
) -> Result<
    (
        Vec<llm_runtime::ProviderProfile>,
        Vec<llm_runtime::ProviderProfile>,
    ),
    CommandError,
> {
    let region =
        harness_runtime::desktop::resolve_provider_region(config, managed).map_err(|_| {
            CommandError::new(
                "settings_unavailable",
                "Could not resolve this host's provider region.",
            )
        })?;
    let inputs = provider_config::AssembleInputs {
        official_catalog: config
            .startup_catalog
            .as_ref()
            .map(|source| (*source.snapshot()).clone()),
        anthropic_api_base: config.api_base.clone(),
        anthropic_models: llm_runtime::builtin_presets()
            .providers
            .into_iter()
            .find(|profile| profile.profile_name == "anthropic")
            .map(|profile| profile.models)
            .unwrap_or_default(),
        anthropic_has_api_key: false,
        anthropic_has_oauth: false,
        user_providers: config.provider_profiles.clone().unwrap_or_default(),
        routing: config.routing.clone(),
    };
    let management = provider_config::assemble_management_for_region(inputs.clone(), region);
    let execution = provider_config::assemble_for_region(inputs, region);
    Ok((
        management.client_config.providers,
        execution.client_config.providers,
    ))
}

async fn configured_account_profiles(
    config: &harness_runtime::desktop::DesktopConfig,
) -> Result<
    (
        Vec<llm_runtime::ProviderProfile>,
        Vec<llm_runtime::ProviderProfile>,
    ),
    CommandError,
> {
    let managed = harness_runtime::desktop::managed_settings_overlay(config)
        .await
        .map_err(|_| {
            CommandError::new(
                "settings_unavailable",
                "Could not load this host's provider settings.",
            )
        })?
        .unwrap_or_else(|| {
            lingxi_core::types::utf16_json::Utf16JsonProjection::parse("{}")
                .expect("empty settings source")
        });
    account_profiles(config, &managed)
}

/// The interactive connect picker uses the same account authority as the CLI.
pub(crate) async fn connect_account(
    config: &harness_runtime::desktop::DesktopConfig,
    credentials: Arc<secret::CredentialManager>,
    provider_id: String,
    key: Option<String>,
) -> Result<AccountSnapshot, String> {
    let home = config.lingxi_home.clone();
    let scope = account_store_scope(
        &home,
        &format!("{:?}", credentials.account_storage_backend()),
    );
    let (management, execution) = configured_account_profiles(config)
        .await
        .map_err(|error| error.message.to_owned())?;
    let store = Arc::new(
        ProviderAccountStore::new(home, scope, credentials)
            .with_profiles(management)
            .with_execution_profiles(execution),
    );
    connect_account_in_store(store, provider_id, key)
        .await
        .map_err(|error| error.message.to_owned())
}

async fn connect_account_in_store(
    store: Arc<ProviderAccountStore>,
    provider_id: String,
    key: Option<String>,
) -> Result<AccountSnapshot, CommandError> {
    if key.is_none() && provider_id != "openai" {
        return Err(CommandError::new(
            "unsupported_auth",
            "Select an OpenAI account for ChatGPT sign-in.",
        ));
    }
    let before = store.snapshot().await.map_err(authority_error)?;
    let oauth = key.is_none();
    let created = store
        .execute(
            ProviderAccountCommand {
                expected_revision: before.config_revision,
                operation_id: None,
                action: ProviderAccountAction::Add {
                    provider_id: provider_id.clone(),
                    display_name: format!("{} account", provider_id),
                    execution_binding_id: if oauth {
                        "openai-chatgpt".into()
                    } else {
                        default_key_binding(&provider_id)
                    },
                    auth_kind: if oauth {
                        AccountAuthKind::OAuth
                    } else {
                        AccountAuthKind::ApiKey
                    },
                    account_id: None,
                },
            },
            key,
        )
        .await
        .map_err(authority_error)?;
    if oauth {
        let account = created
            .accounts
            .iter()
            .find(|account| {
                !before
                    .accounts
                    .iter()
                    .any(|previous| previous.account_id == account.account_id)
            })
            .ok_or_else(|| {
                CommandError::new("account_operation_failed", "Could not create the account.")
            })?;
        run_oauth(
            store,
            account.account_id.clone(),
            created.config_revision,
            &BrowserArgs {
                no_browser: false,
                callback_stdin: false,
            },
        )
        .await
    } else {
        store.snapshot().await.map_err(authority_error)
    }
}

async fn run_accounts(args: &AccountsCli) -> Result<AccountSnapshot, CommandError> {
    let home = crate::run::lingxi_home_dir();
    let stack = harness_runtime::desktop::build_shared_credential_stack(&home, false)
        .await
        .map_err(|_| {
            CommandError::new(
                "storage_unavailable",
                "Could not open this host's credential store.",
            )
        })?;
    let scope = account_store_scope(
        &home,
        &format!("{:?}", stack.credentials.account_storage_backend()),
    );
    let config = crate::init::resolve_desktop_config(
        &crate::argv::Argv::default(),
        permission::PermissionMode::Default,
    );
    let (management, execution) = configured_account_profiles(&config).await?;
    let store = Arc::new(
        ProviderAccountStore::new(home, scope.clone(), stack.credentials.clone())
            .with_profiles(management)
            .with_execution_profiles(execution),
    );
    let snapshot = store.snapshot().await.map_err(authority_error)?;
    let _test_authority = if matches!(args.command, AccountSub::Test(_)) {
        Some(oauth_authority(store.clone())?)
    } else {
        None
    };
    let result = if matches!(args.command, AccountSub::List(_)) {
        snapshot
    } else {
        let request = mutation(args, &snapshot)?;
        let credential = if let AccountSub::Add(add) = &args.command {
            Some(if add.api_key_stdin {
                read_key_stdin().await?
            } else {
                let reference = CredentialRef {
                    store_scope: scope,
                    secret_ref: add.api_key_secret_ref.clone().ok_or_else(|| {
                        CommandError::new(
                            "invalid_credential",
                            "An API key input source is required.",
                        )
                    })?,
                };
                let stored = stack
                    .credentials
                    .get_account_credential(&reference)
                    .await
                    .map_err(|_| {
                        CommandError::new(
                            "storage_unavailable",
                            "Could not read the selected credential slot.",
                        )
                    })?
                    .ok_or_else(|| {
                        CommandError::new(
                            "credential_missing",
                            "The selected credential slot is empty.",
                        )
                    })?;
                match stored.kind {
                    secret::AccountCredentialKind::ApiKey { key } => key.expose_secret().clone(),
                    _ => {
                        return Err(CommandError::new(
                            "invalid_credential",
                            "The selected credential slot is not an API key.",
                        ))
                    }
                }
            })
        } else {
            None
        };
        match &args.command {
            AccountSub::Login(login) => {
                let created = store
                    .execute(request, None)
                    .await
                    .map_err(authority_error)?;
                let account = created
                    .accounts
                    .iter()
                    .find(|account| {
                        !snapshot
                            .accounts
                            .iter()
                            .any(|old| old.account_id == account.account_id)
                    })
                    .ok_or_else(|| {
                        CommandError::new(
                            "account_operation_failed",
                            "The host did not return the new account.",
                        )
                    })?;
                run_oauth(
                    store.clone(),
                    account.account_id.clone(),
                    created.config_revision,
                    &login.browser,
                )
                .await?
            }
            AccountSub::Reconnect(reconnect) => {
                run_oauth(
                    store.clone(),
                    reconnect.target.account_id.clone(),
                    request.expected_revision,
                    &reconnect.browser,
                )
                .await?
            }
            _ => {
                store
                    .execute(request, credential)
                    .await
                    .map_err(authority_error)?;
                store.snapshot().await.map_err(authority_error)?
            }
        }
    };
    Ok(provider_snapshot(result, provider(&args.command)))
}

fn provider_snapshot(mut result: AccountSnapshot, provider_id: &str) -> AccountSnapshot {
    result
        .accounts
        .retain(|account| account.provider_id == provider_id);
    result.accounts.sort_by_key(|account| account.user_priority);
    let account_ids: std::collections::HashSet<_> = result
        .accounts
        .iter()
        .map(|account| account.account_id.clone())
        .collect();
    result
        .health
        .retain(|health| account_ids.contains(&health.account_id));
    result
        .model_entitlements
        .retain(|entry| account_ids.contains(&entry.account_id));
    result
        .model_availability
        .retain(|entry| entry.provider_id == provider_id);
    result.provider_policies.retain(|key, _| key == provider_id);
    result
}

fn oauth_authority(
    store: Arc<ProviderAccountStore>,
) -> Result<Arc<AccountOAuthCoordinator>, CommandError> {
    let transport = http_client::provider_transport().map_err(|_| {
        CommandError::new(
            "transport_unavailable",
            "Could not initialize the authorization transport.",
        )
    })?;
    let handle = Arc::new(llm_runtime::auth::openai::AccountOAuthClient::new(
        lingxi_llm_client::auth::oauth::openai::OpenAiOAuthConfig::default(),
        Arc::new(transport),
    ));
    let authority = Arc::new(AccountOAuthCoordinator::new(store.clone(), handle));
    store
        .credentials()
        .set_account_credential_refresher(authority.clone());
    Ok(authority)
}

async fn run_oauth(
    store: Arc<ProviderAccountStore>,
    account_id: String,
    revision: u64,
    browser: &BrowserArgs,
) -> Result<AccountSnapshot, CommandError> {
    use llm_runtime::auth::openai::callback::CallbackListener;
    let config = lingxi_llm_client::auth::oauth::openai::OpenAiOAuthConfig::default();
    let listener = if browser.callback_stdin {
        None
    } else {
        Some(CallbackListener::bind().await.map_err(|_| {
            CommandError::new(
                "oauth_listener_unavailable",
                "OAuth callback ports are unavailable; use --callback-stdin for a remote shell.",
            )
        })?)
    };
    let redirect = config.redirect_uri(listener.as_ref().map_or(1455, CallbackListener::port));
    let authority = oauth_authority(store)?;
    let session = authority
        .begin(account_id.clone(), revision, redirect.clone())
        .await
        .map_err(authority_error)?;
    eprintln!("Authorize this account: {}", session.authorization_url);
    if !browser.no_browser {
        open_browser(&session.authorization_url).await;
    }
    let callback = async {
        if let Some(listener) = listener {
            let authorize = reqwest::Url::parse(&session.authorization_url).map_err(|_| {
                CommandError::new("oauth_invalid_flow", "Invalid authorization flow.")
            })?;
            let state = authorize
                .query_pairs()
                .find(|(name, _)| name == "state")
                .map(|(_, value)| value.into_owned())
                .ok_or_else(|| {
                    CommandError::new("oauth_invalid_flow", "Invalid authorization flow.")
                })?;
            let params = listener.accept(&state).await.map_err(|_| {
                CommandError::new(
                    "oauth_callback_rejected",
                    "The authorization callback was rejected.",
                )
            })?;
            let mut callback = reqwest::Url::parse(&redirect).map_err(|_| {
                CommandError::new("oauth_invalid_flow", "Invalid authorization flow.")
            })?;
            callback
                .query_pairs_mut()
                .append_pair("code", &params.code)
                .append_pair("state", &params.state);
            Ok(callback.to_string())
        } else {
            eprintln!("Pipe the complete callback URL into stdin. It will not be printed.");
            read_callback_stdin().await
        }
    };
    let outcome = tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(600), callback) => {
            result.map_err(|_| CommandError::new("oauth_timeout", "Authorization timed out."))
                .and_then(|result| result)
        }
        _ = tokio::signal::ctrl_c() => Err(CommandError::new("cancelled", "Authorization cancelled.")),
    };
    match outcome {
        Ok(callback) => authority
            .complete(session.flow_id, callback)
            .await
            .map_err(authority_error),
        Err(error) => {
            let _ = authority.cancel(&account_id, &session.flow_id).await;
            Err(error)
        }
    }
}

async fn read_callback_stdin() -> Result<String, CommandError> {
    use std::io::IsTerminal;
    use tokio::io::AsyncReadExt;
    if std::io::stdin().is_terminal() {
        return Err(CommandError::new(
            "callback_input_required",
            "Pipe the callback URL into stdin to avoid displaying it in the terminal.",
        ));
    }
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(65537)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| {
            CommandError::new(
                "callback_input_unavailable",
                "Could not read the authorization callback.",
            )
        })?;
    if bytes.len() > 65536 {
        return Err(CommandError::new(
            "oauth_callback_rejected",
            "The authorization callback exceeds the size limit.",
        ));
    }
    let callback = String::from_utf8(bytes).map_err(|_| {
        CommandError::new(
            "oauth_callback_rejected",
            "The authorization callback must be UTF-8.",
        )
    })?;
    Ok(callback.trim().to_owned())
}

async fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = tokio::process::Command::new("xdg-open");
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        command
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await;
}

fn output_snapshot(snapshot: &AccountSnapshot, machine: bool) {
    if machine {
        println!("{}", json!({"ok":true,"snapshot":snapshot}));
    } else {
        println!(
            "Account configuration revision {}",
            snapshot.config_revision
        );
        for account in &snapshot.accounts {
            let health = snapshot
                .health
                .iter()
                .find(|health| health.account_id == account.account_id);
            let status = health
                .map(|health| json!(health.status))
                .unwrap_or(Value::Null);
            println!(
                "{}\t{}\t{}\t{}\t{}",
                account.account_id,
                account.user_priority,
                account.display_name,
                if account.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                status
            );
            if let Some(health) = health {
                if let Some(reason) = &health.reason {
                    println!("  reason: {reason}");
                }
                if let Some(until) = health.cooldown_until_ms {
                    println!("  recoveryTimeMs: {until}");
                }
            }
        }
    }
}

// Host composition and OAuth execution live below; no account selection is
// performed by the command layer.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::argv::Argv;

    fn fixture() -> AccountSnapshot {
        serde_json::from_value(json!({"storeScope":"fixture","configRevision":7,
            "accounts":[{"accountId":"a","providerId":"chatgpt","displayName":"A","authKind":"oauth",
            "executionBindingId":"chatgpt","credentialRef":{"storeScope":"fixture","secretRef":"acct.a"},
            "enabled":true,"userPriority":0,"configRevision":7,"identityGeneration":1}],"health":[]})).unwrap()
    }

    #[test]
    fn disabled_bindings_remain_manageable_without_becoming_execution_routes() {
        let home = tempfile::tempdir().unwrap();
        let mut config = harness_runtime::desktop::DesktopConfig::default();
        config.lingxi_home = home.path().into();
        config.cwd = home.path().into();
        config.restricted = true;
        config.provider_profiles = Some(std::collections::BTreeMap::from([(
            "openai".into(),
            json!({"enabled":false}),
        )]));
        let managed = lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
            r#"{"providerRegion":"international"}"#,
        )
        .unwrap();
        let (management, execution) = account_profiles(&config, &managed).unwrap();
        assert!(management
            .iter()
            .any(|profile| profile.profile_name == "openai"));
        assert!(!execution
            .iter()
            .any(|profile| profile.profile_name == "openai"));
        assert!(execution
            .iter()
            .find(|profile| profile.profile_name == "anthropic")
            .is_some_and(|profile| !profile.models.is_empty()));
    }

    #[test]
    fn account_profiles_follow_effective_region_with_and_without_custom_definitions() {
        let home = tempfile::tempdir().unwrap();
        let mut config = harness_runtime::desktop::DesktopConfig::default();
        config.lingxi_home = home.path().into();
        config.cwd = home.path().into();
        config.restricted = true;
        for definitions in [
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::from([(
                "custom".into(),
                json!({"type":"openai", "baseUrl":"https://custom.example/v1",
                    "models":[{"id":"custom-model"}]}),
            )]),
        ] {
            config.provider_profiles = Some(definitions);
            for (region, allowed) in [("china_mainland", true), ("international", false)] {
                let managed = lingxi_core::types::utf16_json::Utf16JsonProjection::parse(
                    &json!({"providerRegion":region}).to_string(),
                )
                .unwrap();
                let (management, execution) = account_profiles(&config, &managed).unwrap();
                for profiles in [&management, &execution] {
                    assert_eq!(
                        profiles
                            .iter()
                            .any(|profile| profile.profile_name == "glm-coding"),
                        allowed,
                        "glm-coding region eligibility must match the effective settings"
                    );
                }
            }
        }
    }

    #[test]
    fn json_snapshot_preserves_shared_model_entitlements_for_selected_provider() {
        let mut snapshot = fixture();
        snapshot.model_entitlements = serde_json::from_value(json!([
            {"accountId":"a","identityGeneration":1,"modelId":"allowed","status":"confirmed","observedAtMs":1},
            {"accountId":"a","identityGeneration":1,"modelId":"unknown","status":"unverified","observedAtMs":1},
            {"accountId":"a","identityGeneration":1,"modelId":"blocked","status":"forbidden","observedAtMs":2,"retryAtMs":300,"reason":"model_forbidden"},
            {"accountId":"other","identityGeneration":1,"modelId":"other-model","status":"confirmed","observedAtMs":1}
        ])).unwrap();
        snapshot.model_availability = serde_json::from_value(json!([
            {"providerId":"chatgpt","executionBindingId":"chatgpt","modelId":"allowed","eligibleAccountIds":["a"],"confirmedAccountIds":["a"],"unverifiedAccountIds":[],"forbiddenAccountIds":[]},
            {"providerId":"other","executionBindingId":"other","modelId":"other-model","eligibleAccountIds":["other"],"confirmedAccountIds":["other"],"unverifiedAccountIds":[],"forbiddenAccountIds":[]}
        ])).unwrap();
        let json = serde_json::to_value(provider_snapshot(snapshot, "chatgpt")).unwrap();
        assert_eq!(json["modelEntitlements"].as_array().unwrap().len(), 3);
        assert_eq!(json["modelEntitlements"][2]["status"], "forbidden");
        assert_eq!(json["modelEntitlements"][2]["retryAtMs"], 300);
        assert_eq!(json["modelAvailability"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["modelAvailability"][0]["confirmedAccountIds"],
            json!(["a"])
        );
    }

    #[tokio::test]
    async fn interactive_connect_creates_an_isolated_named_account() {
        let root = tempfile::tempdir().unwrap();
        let store = fixture_store(root.path());
        let first =
            connect_account_in_store(store.clone(), "openai".into(), Some("fixture-key-a".into()))
                .await
                .unwrap();
        let second =
            connect_account_in_store(store.clone(), "openai".into(), Some("fixture-key-b".into()))
                .await
                .unwrap();
        assert_eq!(second.accounts.len(), 2);
        assert_ne!(
            first.accounts[0].credential_ref,
            second.accounts[1].credential_ref
        );
        let reference = &first.accounts[0].credential_ref;
        assert!(reference.secret_ref.starts_with("acct."));
        assert!(store
            .credentials()
            .get_account_credential(reference)
            .await
            .unwrap()
            .is_some());
        let serialized = serde_json::to_string(&second).unwrap();
        assert!(!serialized.contains("fixture-key"));
    }

    fn fixture_store(root: &std::path::Path) -> Arc<ProviderAccountStore> {
        let credentials = Arc::new(secret::CredentialManager::new(
            Arc::new(lingxi_core::host::InMemorySecureStorage::new()),
            Arc::new(platform_posix::PosixClock::new()),
            Arc::new(platform_posix::PosixHttp::new()),
        ));
        let scope = account_store_scope(root, "MemorySession");
        Arc::new(ProviderAccountStore::new(
            root.to_owned(),
            scope,
            credentials,
        ))
    }

    async fn fixture_command(
        store: &ProviderAccountStore,
        command: AccountSub,
        key: Option<String>,
    ) -> AccountSnapshot {
        let snapshot = store.snapshot().await.unwrap();
        let args = AccountsCli {
            json: true,
            expected_revision: None,
            command,
        };
        store
            .execute(mutation(&args, &snapshot).unwrap(), key)
            .await
            .unwrap();
        store.snapshot().await.unwrap()
    }

    #[tokio::test]
    async fn cli_crud_mapping_persists_order_and_logs_out_one_account() {
        let directory = tempfile::tempdir().unwrap();
        let store = fixture_store(directory.path());
        for name in ["A", "B"] {
            fixture_command(
                &store,
                AccountSub::Add(AddArgs {
                    provider: "openai".into(),
                    name: name.into(),
                    api_key_stdin: true,
                    api_key_secret_ref: None,
                    execution_binding: None,
                }),
                Some(format!("fixture-key-{name}")),
            )
            .await;
        }
        let before = store.snapshot().await.unwrap();
        assert_eq!(before.accounts.len(), 2);
        let a = before
            .accounts
            .iter()
            .find(|account| account.display_name == "A")
            .unwrap()
            .clone();
        let b = before
            .accounts
            .iter()
            .find(|account| account.display_name == "B")
            .unwrap()
            .clone();
        let reordered = fixture_command(
            &store,
            AccountSub::Reorder(ReorderArgs {
                provider: "openai".into(),
                account_ids: vec![b.account_id.clone(), a.account_id.clone()],
            }),
            None,
        )
        .await;
        assert_eq!(
            reordered
                .accounts
                .iter()
                .find(|account| account.account_id == b.account_id)
                .unwrap()
                .user_priority,
            0
        );
        let reopened = ProviderAccountStore::new(
            directory.path().into(),
            store.store_scope().into(),
            store.credentials(),
        );
        assert_eq!(reopened.snapshot().await.unwrap(), reordered);
        let output = serde_json::to_string(&json!({"ok": true, "snapshot": &reordered})).unwrap();
        assert!(!output.contains("fixture-key-A"));
        assert!(!output.contains("fixture-key-B"));
        fixture_command(
            &store,
            AccountSub::Logout(TargetArgs {
                provider: "openai".into(),
                account_id: a.account_id.clone(),
            }),
            None,
        )
        .await;
        assert!(!store
            .credentials()
            .has_account_credential(&a.credential_ref)
            .await
            .unwrap());
        assert!(store
            .credentials()
            .has_account_credential(&b.credential_ref)
            .await
            .unwrap());
        fixture_command(
            &store,
            AccountSub::Remove(TargetArgs {
                provider: "openai".into(),
                account_id: a.account_id,
            }),
            None,
        )
        .await;
        let remaining = store.snapshot().await.unwrap();
        assert_eq!(remaining.accounts.len(), 1);
        assert_eq!(remaining.accounts[0].account_id, b.account_id);
    }

    #[tokio::test]
    async fn listing_an_empty_host_does_not_create_account_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = fixture_store(directory.path());
        assert!(store.snapshot().await.unwrap().accounts.is_empty());
        assert!(!directory.path().join("provider-accounts").exists());
    }

    #[test]
    fn api_keys_cannot_be_passed_as_command_line_values() {
        assert!(
            Argv::from_iter(["cli", "provider", "accounts", "add", "chatgpt", "--name", "A"])
                .is_err()
        );
        assert!(Argv::from_iter([
            "cli",
            "provider",
            "accounts",
            "add",
            "chatgpt",
            "--name",
            "A",
            "--api-key",
            "secret"
        ])
        .is_err());
        assert!(Argv::from_iter([
            "cli",
            "provider",
            "accounts",
            "add",
            "chatgpt",
            "--name",
            "A",
            "--api-key-stdin",
            "--api-key-secret-ref",
            "acct.a"
        ])
        .is_err());
        assert!(Argv::from_iter([
            "cli",
            "provider",
            "accounts",
            "add",
            "chatgpt",
            "--name",
            "A",
            "--api-key-stdin",
            "--json"
        ])
        .is_ok());
    }

    #[test]
    fn all_account_commands_parse_without_starting_a_turn() {
        let parsed =
            Argv::from_iter(["cli", "provider", "accounts", "list", "openai", "--json"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(super::super::Commands::Provider(_))
        ));
        for child in ["reconnect", "enable", "disable", "test", "logout", "remove"] {
            assert!(
                Argv::from_iter(["cli", "provider", "accounts", child, "chatgpt", "a", "--json"])
                    .is_ok(),
                "{child}"
            );
        }
        assert!(
            Argv::from_iter(["cli", "provider", "accounts", "list", "chatgpt", "--json"]).is_ok()
        );
        assert!(Argv::from_iter([
            "cli", "provider", "accounts", "login", "chatgpt", "--name", "A"
        ])
        .is_ok());
        assert!(
            Argv::from_iter(["cli", "provider", "accounts", "reorder", "chatgpt", "a", "b"])
                .is_ok()
        );
    }

    #[test]
    fn targeted_logout_cannot_cross_provider_boundaries() {
        let snapshot = fixture();
        let mut args = AccountsCli {
            json: true,
            expected_revision: None,
            command: AccountSub::Logout(TargetArgs {
                provider: "anthropic".into(),
                account_id: "a".into(),
            }),
        };
        assert!(mutation(&args, &snapshot).is_err());
        args.command = AccountSub::Logout(TargetArgs {
            provider: "chatgpt".into(),
            account_id: "a".into(),
        });
        let request = mutation(&args, &snapshot).unwrap();
        assert!(
            matches!(&request.action, ProviderAccountAction::Logout { account_id } if account_id == "a")
        );
        assert_eq!(request.expected_revision, 7);
        args.expected_revision = Some(6);
        assert_eq!(mutation(&args, &snapshot).unwrap().expected_revision, 6);
    }

    #[test]
    fn authority_failures_do_not_echo_adapter_error_material() {
        let error = authority_error("credential storage unavailable: fixture-secret-value".into());
        assert_eq!(error.code, "storage_unavailable");
        assert!(!error.message.contains("fixture-secret-value"));
        let error = authority_error(
            "authorization returned a different upstream account; add a new account".into(),
        );
        assert_eq!(error.code, "oauth_subject_mismatch");
    }

    #[test]
    fn key_input_rejects_multiple_values_and_never_quotes_input_in_errors() {
        assert_eq!(
            parse_key_input(b"fixture-key\n".to_vec()).unwrap(),
            "fixture-key"
        );
        for input in [
            b"\n".to_vec(),
            b"fixture-key\nsecond".to_vec(),
            vec![b'x'; 65537],
            vec![255],
        ] {
            let error = parse_key_input(input).err().unwrap();
            assert!(!error.message.contains("fixture-key"));
        }
    }
}
