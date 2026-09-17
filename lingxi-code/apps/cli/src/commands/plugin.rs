//! `lingxi-cli plugin` (alias `plugins`) — Manage LingXi plugins.
//!
//! Byte-faithful clap surface vs `claude plugin --help` (claude-code 2.1.191):
//! the family has 12 children plus a nested `marketplace` group with 4 of its
//! own. Bare `lingxi-cli plugin` (no child) prints help, never errors —
//! matching commander's parent-with-subcommands behavior (the `command` field
//! is `Option`, and `run` prints help when it is `None`).
//!
//! All public subcommands are wired: read-only inspection/validation, scoped
//! settings toggles, install/update/uninstall/prune, scaffolding/tagging, and
//! marketplace management. The implementations reuse the CLI's confined
//! settings/archive/git helpers and never start a billable model turn.

use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telemetry::AnalyticsBus;

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

/// `plugin` args — byte-parity with `claude plugin|plugins [options] [command]`.
///
/// `command` is `Option` so a bare `lingxi-cli plugin` prints help instead of
/// erroring (commander's parent-with-children behavior).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// The chosen plugin subcommand (absent ⇒ print help).
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// The `plugin` subcommands. Names, aliases, args, options, and one-line
/// descriptions byte-match `claude plugin --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Show a plugin's component inventory and projected token cost
    Details(DetailsArgs),

    /// Disable an enabled plugin
    Disable(DisableArgs),

    /// Enable a disabled plugin
    Enable(EnableArgs),

    /// Run eval cases against a plugin and report scored results
    #[command(
        long_about = "Run eval cases (<eval dir>/**/case.yaml or prompt.md + graders/*.md; the eval dir is evals/ unless --eval-dir or the manifest says otherwise) against a plugin and report scored results. Target is a path, a plugin name, or a `plugin@marketplace` id — installed and skills-dir plugins both resolve (and add a no-plugin baseline arm)"
    )]
    Eval(crate::commands::plugin_eval::Cli),

    /// Scaffold a new plugin at ~/.lingxi/skills/<name>/ (auto-loads next
    /// session as <name>@skills-dir)
    #[command(name = "init", visible_alias = "new")]
    Init(InitArgs),

    /// Install a plugin from available marketplaces (use plugin@marketplace for
    /// specific marketplace)
    #[command(name = "install", visible_alias = "i")]
    Install(InstallArgs),

    /// List installed plugins
    List(ListArgs),

    /// Manage LingXi marketplaces
    Marketplace(MarketplaceArgs),

    /// Remove auto-installed dependencies that are no longer needed
    #[command(name = "prune", visible_alias = "autoremove")]
    Prune(PruneArgs),

    /// Create a {name}--v{version} git tag for a plugin release, validating that
    /// plugin.json and any enclosing marketplace entry agree
    Tag(TagArgs),

    /// Uninstall an installed plugin
    #[command(name = "uninstall", visible_aliases = ["remove", "rm"])]
    Uninstall(UninstallArgs),

    /// Update a plugin to the latest version (restart required to apply)
    Update(UpdateArgs),

    /// Validate a plugin or marketplace manifest, or the skills, agents, and
    /// commands in a directory
    Validate(ValidateArgs),
}

/// `plugin details <name>` — no options beyond the positional.
#[derive(Debug, Clone, Args)]
pub struct DetailsArgs {
    /// Plugin name to inspect.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin disable [plugin]`.
#[derive(Debug, Clone, Args)]
pub struct DisableArgs {
    /// Disable all enabled plugins
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Installation scope: user, project, local (default: auto-detect)
    #[arg(short = 's', long, value_name = "scope")]
    pub scope: Option<String>,

    /// Plugin to disable.
    #[arg(value_name = "plugin")]
    pub plugin: Option<String>,
}

/// `plugin enable <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct EnableArgs {
    /// Installation scope: user, project, local (default: auto-detect)
    #[arg(short = 's', long, value_name = "scope")]
    pub scope: Option<String>,

    /// Plugin to enable.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin init|new <name>`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Author name (default: git config user.name)
    #[arg(long, value_name = "name")]
    pub author: Option<String>,

    /// Author email (default: git config user.email)
    #[arg(long = "author-email", value_name = "email")]
    pub author_email: Option<String>,

    /// Manifest description
    #[arg(long, value_name = "text")]
    pub description: Option<String>,

    /// Overwrite an existing .lingxi-plugin/ at the target
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Also scaffold: skills, agents, hooks, mcp, lsp, output-style, channel
    #[arg(long = "with", value_name = "components", num_args = 1..)]
    pub with: Vec<String>,

    /// Plugin name to scaffold.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin install|i <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// Set a userConfig option declared in the plugin's manifest (repeatable).
    /// Values are validated against the schema and stored via the same path as
    /// the interactive /plugin configure flow.
    #[arg(long = "config", value_name = "key=value")]
    pub config: Vec<String>,

    /// Installation scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Accept the displayed marketplace-declared command without the
    /// confirmation prompt — a plugin installed by running a command, or one
    /// whose archive is fetched through a headersHelper command (required when
    /// stdin or stdout is not a TTY)
    //
    // CLI-07 (cc 2.1.238, cc-238.js @235156585): NEW in 2.1.238 — 2.1.220's
    // `plugin install` carried only `-s, --scope` and `--config`.
    //
    // `-y` is the terminal consent for a `source:"command"` marketplace entry
    // (oracle `jl({yes})`). `headersHelper` archives remain unimplemented.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Plugin to install (use plugin@marketplace for a specific marketplace).
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin list`.
#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Include available plugins from marketplaces (requires --json)
    #[arg(long)]
    pub available: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// `plugin prune|autoremove`.
#[derive(Debug, Clone, Args)]
pub struct PruneArgs {
    /// List what would be removed without removing
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Prune at scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Skip the confirmation prompt (required when stdin or stdout is not a
    /// TTY)
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// `plugin tag [path]`.
#[derive(Debug, Clone, Args)]
pub struct TagArgs {
    /// Print what would be tagged without creating it
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Skip the dirty-working-tree and tag-already-exists checks
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Tag annotation message (use %s for the version)
    #[arg(short = 'm', long, value_name = "msg")]
    pub message: Option<String>,

    /// Push the tag to --remote after creating it
    #[arg(long)]
    pub push: bool,

    /// Remote to push to with --push (default: "origin")
    #[arg(long, value_name = "name", default_value = "origin")]
    pub remote: String,

    /// Path to the plugin to tag (default: current directory).
    #[arg(value_name = "path")]
    pub path: Option<String>,
}

/// `plugin uninstall|remove <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct UninstallArgs {
    /// Preserve the plugin's persistent data directory
    /// (~/.lingxi/plugins/data/{id}/)
    #[arg(long = "keep-data")]
    pub keep_data: bool,

    /// Also remove auto-installed dependencies that are no longer needed
    /// (requires -y in non-interactive contexts)
    #[arg(long)]
    pub prune: bool,

    /// Uninstall from scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Skip the --prune confirmation prompt (required when stdin or stdout is
    /// not a TTY)
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Plugin to uninstall.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin update <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    /// Installation scope: user, project, local, managed (default: user)
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Accept the displayed marketplace-declared command without the
    /// confirmation prompt — a changed install command, or the headersHelper
    /// command that fetches its archive (required when stdin or stdout is not a
    /// TTY)
    //
    // CLI-08 (cc 2.1.238, cc-238.js @235159327): NEW in 2.1.238 — 2.1.220's
    // `plugin update` carried only `-s, --scope`. PARSED, NOT CONSUMED for the
    // same reason as the `install` twin above: LingXi has no command-declared
    // install and no headersHelper archive fetch, so there is no prompt to
    // waive. See `InstallArgs::yes`.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Plugin to update.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// Validate a plugin or marketplace manifest, or the skills, agents, and
/// commands in a directory.
#[derive(Debug, Clone, Args)]
pub struct ValidateArgs {
    /// Treat warnings as errors (exit 1). Use in CI to fail on unrecognized
    /// fields, missing metadata, and other issues that the runtime tolerates.
    #[arg(long)]
    pub strict: bool,

    /// Path to a plugin/marketplace manifest or a directory of components.
    #[arg(value_name = "path")]
    pub path: String,
}

/// `plugin marketplace [command]` — wraps the marketplace children in an
/// `Option` so a bare `lingxi-cli plugin marketplace` prints help (commander's
/// parent-with-children behavior) instead of erroring on a missing subcommand.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceArgs {
    /// The chosen marketplace subcommand (absent ⇒ print help).
    #[command(subcommand)]
    pub command: Option<MarketplaceSub>,
}

/// The `plugin marketplace` subcommands — byte-parity with
/// `claude plugin marketplace --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum MarketplaceSub {
    /// Add a marketplace from a URL, path, or GitHub repo
    Add(MarketplaceAddArgs),

    /// List all configured marketplaces
    List(MarketplaceListArgs),

    /// Remove a configured marketplace
    #[command(name = "remove", visible_alias = "rm")]
    Remove(MarketplaceRemoveArgs),

    /// Update marketplace(s) from their source - updates all if no name
    /// specified
    Update(MarketplaceUpdateArgs),
}

/// `plugin marketplace add <source>`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceAddArgs {
    /// Where to declare the marketplace: user (default), project, or local
    #[arg(long, value_name = "scope")]
    pub scope: Option<String>,

    /// Limit checkout to specific directories via git sparse-checkout (for
    /// monorepos). Example: --sparse .lingxi-plugin plugins
    #[arg(long, value_name = "paths", num_args = 1..)]
    pub sparse: Vec<String>,

    /// Marketplace source: a URL, path, or GitHub repo.
    #[arg(value_name = "source")]
    pub source: String,
}

/// `plugin marketplace list`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// `plugin marketplace remove|rm <name>`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceRemoveArgs {
    /// Remove the marketplace declaration from a specific settings scope: user,
    /// project, or local. Omit to remove it from every scope.
    #[arg(long, value_name = "scope")]
    pub scope: Option<String>,

    /// Marketplace name to remove.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin marketplace update [name]`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceUpdateArgs {
    /// Marketplace name to update (omit to update all).
    #[arg(value_name = "name")]
    pub name: Option<String>,
}

/// Run the `plugin` family. A bare `lingxi-cli plugin` (no child) prints help
/// and exits success.
pub async fn run(cli: &Cli) -> i32 {
    run_with_analytics_bus(cli, None).await
}

pub async fn run_with_shared_analytics_bus(
    cli: &Cli,
    analytics_bus: Arc<telemetry::AnalyticsBus>,
) -> i32 {
    run_with_analytics_bus(cli, Some(&analytics_bus)).await
}

async fn run_with_analytics_bus(
    cli: &Cli,
    analytics_bus: Option<&Arc<telemetry::AnalyticsBus>>,
) -> i32 {
    let Some(command) = cli.command.as_ref() else {
        print_help();
        return SUCCESS;
    };

    match command {
        Sub::List(args) => run_list_with_bus(args, analytics_bus).await,
        Sub::Details(args) => run_details_with_bus(args, analytics_bus).await,
        Sub::Validate(args) => run_validate(args).await,
        Sub::Eval(args) => {
            crate::commands::plugin_eval::run(
                args,
                &plugins_dir(),
                &crate::run::lingxi_home_dir(),
                &scope_cwd(),
            )
            .await
        }

        // On-disk `enabledPlugins` allowlist toggle (the CLI seam — settings.json
        // read-modify-write at the chosen scope), 1:1 with claude 2.1.201.
        Sub::Enable(args) => run_enable_with_bus(args, analytics_bus).await,
        Sub::Disable(args) => run_disable_with_bus(args, analytics_bus).await,

        // Install/uninstall — marketplace-registry materialization + on-disk state.
        Sub::Install(args) => run_install_command_with_bus(args, analytics_bus).await,
        Sub::Uninstall(args) => run_uninstall_command(args, analytics_bus).await,

        // `init` — scaffold a skill-plugin under ~/.lingxi/skills/ (the default
        // scaffold; `--with` component scaffolds are a tracked follow-up).
        Sub::Init(args) => market_result(crate::commands::plugin_init::run_init(
            &args.name,
            args.author.as_deref(),
            args.author_email.as_deref(),
            args.description.as_deref(),
            args.force,
            &args.with,
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
        )),

        // Prune orphaned auto-installed dependencies from the v2 installed record.
        Sub::Prune(args) => run_prune_command_with_bus(args, analytics_bus).await,
        // Create a `{name}--v{version}` git tag (all output, incl. errors, to STDOUT).
        Sub::Tag(args) => tag_result(crate::commands::plugin_tag::run_tag(
            args.path.as_deref(),
            args.dry_run,
            args.force,
            args.message.as_deref(),
            args.push,
            &args.remote,
        )),
        // Re-materialize an installed plugin from its marketplace + bump the record.
        Sub::Update(args) => run_update_command_with_bus(args, analytics_bus).await,
        Sub::Marketplace(args) => {
            let Some(sub) = args.command.as_ref() else {
                print_marketplace_help();
                return SUCCESS;
            };
            use crate::commands::plugin_marketplace as market;
            match sub {
                // `list` reads the resolved `known_marketplaces.json` registry.
                MarketplaceSub::List(list_args) => {
                    println!("{}", market::run_list(&plugins_dir(), list_args.json));
                    SUCCESS
                }
                MarketplaceSub::Add(add_args) => market_result(market::run_add(
                    &add_args.source,
                    add_args.scope.as_deref(),
                    &add_args.sparse,
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
                MarketplaceSub::Remove(rm_args) => market_result(market::run_remove(
                    &rm_args.name,
                    rm_args.scope.as_deref(),
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
                MarketplaceSub::Update(up_args) => market_result(market::run_update(
                    up_args.name.as_deref(),
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
            }
        }
    }
}

/// Print the `plugin marketplace` group help (bare-parent case).
fn print_marketplace_help() {
    let mut cmd = <MarketplaceArgs as clap::Args>::augment_args(clap::Command::new("marketplace"))
        .bin_name("lingxi-cli plugin marketplace")
        .about("Manage LingXi marketplaces");
    let _ = cmd.print_help();
    println!();
}

/// Print the family help (used for the bare-parent case). Mirrors clap's own
/// help rendering for the parent via `Cli`'s derived command.
fn print_help() {
    // The parent command is the `Plugin(plugin::Cli)` variant of the top-level
    // `Commands` enum. `Cli` derives `clap::Args` (a subcommand payload, not a
    // `Parser`), so render its help by augmenting a fresh `Command` with the
    // derived surface — this gives the children + options block byte-faithfully
    // without reaching across modules into the top-level `Commands`.
    let mut cmd = <Cli as clap::Args>::augment_args(clap::Command::new("plugin"))
        .bin_name("lingxi-cli plugin")
        .about("Manage LingXi plugins");
    let _ = cmd.print_help();
    println!();
}

/// Print a `plugin tag` result. Unlike [`market_result`], BOTH the Ok and Err
/// blocks go to STDOUT — claude's `plugin tag` never writes to stderr; Err maps
/// to exit 1.
fn tag_result(res: Result<String, String>) -> i32 {
    match res {
        Ok(msg) => {
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            println!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// The current working directory used for project/local scope resolution
/// (a failure to read it — unusual — degrades to `.`, matching a bare relative
/// join).
fn scope_cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Print a marketplace command's result (Ok → stdout/SUCCESS, Err →
/// stderr/RUNTIME_ERROR).
fn market_result(res: Result<String, String>) -> i32 {
    match res {
        Ok(msg) => {
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

pub(crate) use configuration_admin::plugin_telemetry::*;

fn plugin_error_kind(message: &str) -> &'static str {
    if message.contains("already installed") {
        "already_installed"
    } else if message.contains("already enabled") {
        "already_enabled"
    } else if message.contains("already disabled") {
        "already_disabled"
    } else if message.contains("latest version") {
        "already_latest"
    } else if message.contains("not found") {
        "not_found"
    } else if message.contains("Invalid scope") {
        "invalid_scope"
    } else if message.contains("requires --json") {
        "invalid_args"
    } else if message.contains("required by enabled plugin") {
        "dependency_blocked"
    } else if message.contains("Not a TTY") {
        "not_tty"
    } else if message.contains("failed to serialize JSON") {
        "serialize_failed"
    } else {
        "other"
    }
}

fn is_noop_error(message: &str) -> bool {
    matches!(
        plugin_error_kind(message),
        "already_installed" | "already_enabled" | "already_disabled"
    )
}

fn disabled_count(message: &str) -> u64 {
    message
        .strip_prefix("✔ Disabled ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

fn removed_count(message: &str) -> u64 {
    message
        .strip_prefix("Removed ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

async fn run_install_command_with_bus(
    args: &InstallArgs,
    analytics_bus: Option<&Arc<telemetry::AnalyticsBus>>,
) -> i32 {
    let emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(Some(&args.scope)),
        yes: args.yes,
        config_count: args.config.len() as u64,
        ..PluginCommandTelemetry::default()
    };
    let result = crate::commands::plugin_install::run_install_secure_with_bus(
        &args.plugin,
        Some(&args.scope),
        args.yes,
        &args.config,
        &plugins_dir(),
        &crate::run::lingxi_home_dir(),
        &scope_cwd(),
        analytics_bus.cloned(),
    )
    .await;
    match &result {
        Ok(_) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::INSTALL_COMMAND,
                PluginCommandOutcome::Success,
                emit_meta,
            )
            .await
        }
        Err(message) if is_noop_error(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::INSTALL_COMMAND,
                PluginCommandOutcome::Noop,
                emit_meta,
            )
            .await
        }
        Err(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::INSTALL_COMMAND,
                PluginCommandOutcome::Failure,
                emit_meta,
            )
            .await;
            emit_plugin_command_failed(
                analytics_bus,
                "install",
                emit_meta.scope,
                plugin_error_kind(message),
            )
            .await;
        }
    }
    market_result(result)
}

async fn run_uninstall_command(
    args: &UninstallArgs,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> i32 {
    let emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(Some(&args.scope)),
        keep_data: args.keep_data,
        prune: args.prune,
        yes: args.yes,
        ..PluginCommandTelemetry::default()
    };
    let result = crate::commands::plugin_install::run_uninstall_secure(
        &args.plugin,
        Some(&args.scope),
        args.keep_data,
        args.prune,
        args.yes,
        &plugins_dir(),
        &crate::run::lingxi_home_dir(),
        &scope_cwd(),
        analytics_bus,
    )
    .await;
    match &result {
        Ok(_) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UNINSTALL_COMMAND,
                PluginCommandOutcome::Success,
                emit_meta,
            )
            .await
        }
        Err(message) if is_noop_error(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UNINSTALL_COMMAND,
                PluginCommandOutcome::Noop,
                emit_meta,
            )
            .await
        }
        Err(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UNINSTALL_COMMAND,
                PluginCommandOutcome::Failure,
                emit_meta,
            )
            .await;
            emit_plugin_command_failed(
                analytics_bus,
                "uninstall",
                emit_meta.scope,
                plugin_error_kind(message),
            )
            .await;
        }
    }
    market_result(result)
}

fn run_update_command(args: &UpdateArgs) -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread tokio runtime");
    runtime.block_on(run_update_command_with_bus(args, None))
}

async fn run_update_command_with_bus(
    args: &UpdateArgs,
    analytics_bus: Option<&Arc<telemetry::AnalyticsBus>>,
) -> i32 {
    let emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(Some(&args.scope)),
        yes: args.yes,
        ..PluginCommandTelemetry::default()
    };
    let result = crate::commands::plugin_install::run_update_async(
        &args.plugin,
        &args.scope,
        &plugins_dir(),
        &crate::run::lingxi_home_dir(),
        &scope_cwd(),
        analytics_bus,
    )
    .await;
    match &result {
        Ok(message) if message.contains("already at the latest version") => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UPDATE_COMMAND,
                PluginCommandOutcome::Noop,
                emit_meta,
            )
            .await
        }
        Ok(_) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UPDATE_COMMAND,
                PluginCommandOutcome::Success,
                emit_meta,
            )
            .await
        }
        Err(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::UPDATE_COMMAND,
                PluginCommandOutcome::Failure,
                emit_meta,
            )
            .await;
            emit_plugin_command_failed(
                analytics_bus,
                "update",
                emit_meta.scope,
                plugin_error_kind(message),
            )
            .await;
        }
    }
    market_result(result)
}

fn run_prune_command(args: &PruneArgs) -> i32 {
    current_thread_runtime().block_on(run_prune_command_with_bus(args, None))
}

async fn run_prune_command_with_bus(
    args: &PruneArgs,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> i32 {
    let mut emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(Some(&args.scope)),
        yes: args.yes,
        ..PluginCommandTelemetry::default()
    };
    let result = crate::commands::plugin_prune::run_prune_with_bus(
        args.dry_run,
        args.yes,
        &args.scope,
        &plugins_dir(),
        &crate::run::lingxi_home_dir(),
        &scope_cwd(),
        analytics_bus,
    )
    .await;
    match &result {
        Ok(message) if message.starts_with("Removed ") => {
            emit_meta.removed_count = removed_count(message);
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::PRUNE_COMMAND,
                PluginCommandOutcome::Success,
                emit_meta,
            )
            .await;
        }
        Ok(_) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::PRUNE_COMMAND,
                PluginCommandOutcome::Noop,
                emit_meta,
            )
            .await
        }
        Err(message) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::PRUNE_COMMAND,
                PluginCommandOutcome::Failure,
                emit_meta,
            )
            .await;
            emit_plugin_command_failed(
                analytics_bus,
                "prune",
                emit_meta.scope,
                plugin_error_kind(message),
            )
            .await;
        }
    }
    market_result(result)
}

/// `plugin enable <plugin>` — toggle the on-disk `enabledPlugins` allowlist.
/// Prints the `✔`/`✘` line and maps success/failure to the exit code.
fn run_enable(args: &EnableArgs) -> i32 {
    current_thread_runtime().block_on(run_enable_with_bus(args, None))
}

async fn run_enable_with_bus(args: &EnableArgs, analytics_bus: Option<&Arc<AnalyticsBus>>) -> i32 {
    let home = crate::run::lingxi_home_dir();
    let cwd = scope_cwd();
    let emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(args.scope.as_deref()),
        ..PluginCommandTelemetry::default()
    };
    match crate::commands::plugin_settings::run_enable(
        &args.plugin,
        args.scope.as_deref(),
        &home,
        &cwd,
    ) {
        Ok(msg) => {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::ENABLE_COMMAND,
                PluginCommandOutcome::Success,
                emit_meta,
            )
            .await;
            emit_plugin_cli_result(
                analytics_bus,
                telemetry::tengu::plugin::ENABLED_CLI,
                PluginCommandOutcome::Success,
                emit_meta.scope,
                1,
            )
            .await;
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            let outcome = if is_noop_error(&msg) {
                PluginCommandOutcome::Noop
            } else {
                PluginCommandOutcome::Failure
            };
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::ENABLE_COMMAND,
                outcome,
                emit_meta,
            )
            .await;
            if outcome == PluginCommandOutcome::Failure {
                emit_plugin_command_failed(
                    analytics_bus,
                    "enable",
                    emit_meta.scope,
                    plugin_error_kind(&msg),
                )
                .await;
            }
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// `plugin disable [plugin] [--all]` — toggle the on-disk `enabledPlugins`
/// allowlist off.
fn run_disable(args: &DisableArgs) -> i32 {
    current_thread_runtime().block_on(run_disable_with_bus(args, None))
}

async fn run_disable_with_bus(
    args: &DisableArgs,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> i32 {
    let home = crate::run::lingxi_home_dir();
    let cwd = scope_cwd();
    let emit_meta = PluginCommandTelemetry {
        scope: telemetry_scope(args.scope.as_deref()),
        all: args.all,
        ..PluginCommandTelemetry::default()
    };
    match crate::commands::plugin_settings::run_disable(
        args.plugin.as_deref(),
        args.scope.as_deref(),
        args.all,
        &home,
        &cwd,
    ) {
        Ok(msg) => {
            let count = if args.all { disabled_count(&msg) } else { 1 };
            let outcome = if args.all && count == 0 {
                PluginCommandOutcome::Noop
            } else {
                PluginCommandOutcome::Success
            };
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::DISABLE_COMMAND,
                outcome,
                PluginCommandTelemetry {
                    removed_count: count,
                    ..emit_meta
                },
            )
            .await;
            let event = if args.all {
                telemetry::tengu::plugin::DISABLED_ALL_CLI
            } else {
                telemetry::tengu::plugin::DISABLED_CLI
            };
            emit_plugin_cli_result(analytics_bus, event, outcome, emit_meta.scope, count).await;
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            let outcome = if is_noop_error(&msg) {
                PluginCommandOutcome::Noop
            } else {
                PluginCommandOutcome::Failure
            };
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::DISABLE_COMMAND,
                outcome,
                emit_meta,
            )
            .await;
            if outcome == PluginCommandOutcome::Failure {
                emit_plugin_command_failed(
                    analytics_bus,
                    "disable",
                    emit_meta.scope,
                    plugin_error_kind(&msg),
                )
                .await;
            }
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// The user-tier plugins directory: `$LINGXI_CONFIG_DIR`/`~/.claude` + `plugins`.
fn plugins_dir() -> std::path::PathBuf {
    crate::run::lingxi_home_dir().join("plugins")
}

/// `plugin list` — enumerate installed plugins from the durable on-disk install
/// record (`~/.lingxi/plugins/installed_plugins.json`), resolving each to its
/// versioned cache manifest. `--available --json` additionally reads every
/// configured marketplace's local catalog and excludes installed plugin ids.
/// Read-only; never refreshes or fetches a marketplace.
async fn run_list(args: &ListArgs) -> i32 {
    run_list_with_bus(args, None).await
}

async fn run_list_with_bus(args: &ListArgs, analytics_bus: Option<&Arc<AnalyticsBus>>) -> i32 {
    let emit_meta = PluginCommandTelemetry {
        scope: "user",
        json: args.json,
        available: args.available,
        ..PluginCommandTelemetry::default()
    };
    // `--available` requires `--json` (claude gates it the same way).
    if args.available && !args.json {
        emit_plugin_command(
            analytics_bus,
            telemetry::tengu::plugin::LIST_COMMAND,
            PluginCommandOutcome::Failure,
            emit_meta,
        )
        .await;
        emit_plugin_command_failed(analytics_bus, "list", emit_meta.scope, "invalid_args").await;
        eprintln!("error: --available requires --json");
        return RUNTIME_ERROR;
    }

    let dir = plugins_dir();
    let discovered = plugin::discover_recorded_plugins(&dir).await;

    if args.json {
        let (durable_installed, diagnostics) = installed_json_items(
            &dir,
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
            analytics_bus,
        )
        .await;
        for diagnostic in diagnostics {
            eprintln!("warning: {diagnostic}");
        }
        let installed = durable_installed.unwrap_or_else(|| {
            // Legacy/corrupt records cannot provide Claude's durable metadata;
            // preserve the previous best-effort manifest view as a fallback.
            discovered
                .iter()
                .map(|(_, m, path)| {
                    serde_json::json!({
                        "name": m.name,
                        "version": m.version,
                        "description": m.description,
                        "path": path.display().to_string(),
                    })
                })
                .collect()
        });

        let installed_count = installed.len() as u64;
        let (value, available_count) = if args.available {
            let (available, diagnostics) = available_plugins(&dir);
            for diagnostic in diagnostics {
                eprintln!("warning: {diagnostic}");
            }
            (
                serde_json::json!({
                    "installed": installed,
                    "available": available,
                }),
                available.len() as u64,
            )
        } else {
            (serde_json::Value::Array(installed), 0)
        };

        match serde_json::to_string_pretty(&value) {
            Ok(s) => {
                emit_plugin_command(
                    analytics_bus,
                    telemetry::tengu::plugin::LIST_COMMAND,
                    if installed_count == 0 && available_count == 0 {
                        PluginCommandOutcome::Noop
                    } else {
                        PluginCommandOutcome::Success
                    },
                    PluginCommandTelemetry {
                        installed_count,
                        available_count,
                        ..emit_meta
                    },
                )
                .await;
                println!("{s}");
                SUCCESS
            }
            Err(e) => {
                emit_plugin_command(
                    analytics_bus,
                    telemetry::tengu::plugin::LIST_COMMAND,
                    PluginCommandOutcome::Failure,
                    PluginCommandTelemetry {
                        installed_count,
                        available_count,
                        ..emit_meta
                    },
                )
                .await;
                emit_plugin_command_failed(
                    analytics_bus,
                    "list",
                    emit_meta.scope,
                    "serialize_failed",
                )
                .await;
                eprintln!("lingxi-cli plugin list: failed to serialize JSON: {e}");
                RUNTIME_ERROR
            }
        }
    } else {
        if discovered.is_empty() {
            emit_plugin_command(
                analytics_bus,
                telemetry::tengu::plugin::LIST_COMMAND,
                PluginCommandOutcome::Noop,
                emit_meta,
            )
            .await;
            // Oracle: "No plugins installed. Use `claude plugin install` to
            // install a plugin." The remediation hint names this binary.
            println!("No plugins installed. Use `lingxi-cli plugin install` to install a plugin.");
            return SUCCESS;
        }
        emit_plugin_command(
            analytics_bus,
            telemetry::tengu::plugin::LIST_COMMAND,
            PluginCommandOutcome::Success,
            PluginCommandTelemetry {
                installed_count: discovered.len() as u64,
                ..emit_meta
            },
        )
        .await;
        for (_, m, _) in &discovered {
            let version = if m.version.is_empty() {
                String::new()
            } else {
                format!(" v{}", m.version)
            };
            if m.description.is_empty() {
                println!("{}{version}", m.name);
            } else {
                println!("{}{version} — {}", m.name, m.description);
            }
        }
        SUCCESS
    }
}

/// Resolve the settings file associated with an installed record. Project and
/// local records retain the project root they were installed from, so listing
/// remains correct even when invoked from another working directory.
fn installed_record_settings_path(
    record: &serde_json::Map<String, serde_json::Value>,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use crate::commands::plugin_settings::{scope_path, SettingsScope};

    let scope = record.get("scope")?.as_str()?;
    let project_root = record
        .get("projectPath")
        .and_then(serde_json::Value::as_str)
        .map(std::path::Path::new)
        .unwrap_or(cwd);
    match scope {
        "user" => Some(scope_path(SettingsScope::User, home, cwd)),
        "project" => Some(scope_path(SettingsScope::Project, home, project_root)),
        "local" => Some(scope_path(SettingsScope::Local, home, project_root)),
        _ => None,
    }
}

/// Render current v2 installed records in Claude's public JSON shape. This is
/// intentionally driven by the durable database rather than the cache: a
/// missing plugin directory is still an installed record and must remain
/// visible to repair/uninstall workflows.
async fn installed_json_items(
    plugins_dir: &std::path::Path,
    home: &std::path::Path,
    cwd: &std::path::Path,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> (Option<Vec<serde_json::Value>>, Vec<String>) {
    let path = plugins_dir.join("installed_plugins.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (None, Vec::new()),
        Err(error) => {
            emit_plugin_state_file_error(analytics_bus, "list", "read", "io").await;
            return (
                None,
                vec![format!("failed to read {}: {error}", path.display())],
            );
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            emit_plugin_state_file_error(analytics_bus, "list", "parse", "parse").await;
            return (
                None,
                vec![format!("failed to parse {}: {error}", path.display())],
            );
        }
    };
    let Some(plugins) = value.get("plugins").and_then(serde_json::Value::as_object) else {
        return (
            None,
            vec![format!(
                "{} field 'plugins' must be an object",
                path.display()
            )],
        );
    };
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();

    for (id, records) in plugins {
        if let Some(records) = records.as_array() {
            for (index, record) in records.iter().enumerate() {
                let Some(record) = record.as_object() else {
                    diagnostics.push(format!(
                        "installed plugin '{id}' record[{index}] must be an object; skipping"
                    ));
                    continue;
                };
                out.push(installed_json_item(id, record, home, cwd));
            }
            continue;
        }

        // Legacy schema: `plugins[marketplace][name] = record`. Render these
        // alongside v2 rows instead of letting one old marketplace hide every
        // valid current record.
        let Some(legacy_records) = records.as_object() else {
            diagnostics.push(format!(
                "installed plugin entry '{id}' must be an array or legacy object; skipping"
            ));
            continue;
        };
        for (name, record) in legacy_records {
            let Some(record) = record.as_object() else {
                diagnostics.push(format!(
                    "legacy installed plugin '{name}@{id}' must be an object; skipping"
                ));
                continue;
            };
            out.push(installed_json_item(
                &format!("{name}@{id}"),
                record,
                home,
                cwd,
            ));
        }
    }
    (Some(out), diagnostics)
}

fn installed_json_item(
    id: &str,
    record: &serde_json::Map<String, serde_json::Value>,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> serde_json::Value {
    let settings = installed_record_settings_path(record, home, cwd)
        .and_then(|path| migrations::settings_update::read_settings_map(&path).ok())
        .unwrap_or_default();
    let enabled = settings
        .get("enabledPlugins")
        .and_then(serde_json::Value::as_object)
        .and_then(|map| map.get(id))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let mut item = serde_json::Map::new();
    item.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    for key in ["version", "scope"] {
        if let Some(value) = record.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }
    item.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    for key in ["installPath", "installedAt", "lastUpdated", "projectPath"] {
        if let Some(value) = record.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }
    if let Some(mcp_servers) = settings
        .get("pluginConfigs")
        .and_then(serde_json::Value::as_object)
        .and_then(|configs| configs.get(id))
        .and_then(|config| config.get("mcpServers"))
    {
        item.insert("mcpServers".to_string(), mcp_servers.clone());
    }
    serde_json::Value::Object(item)
}

/// Read the durable install database and return its full `plugin@marketplace`
/// ids. Both the current v2 array shape and the legacy marketplace-nested shape
/// are accepted so an older install is never offered as "available" again.
fn installed_plugin_ids(plugins_dir: &std::path::Path) -> std::collections::BTreeSet<String> {
    let Some(plugins) = std::fs::read_to_string(plugins_dir.join("installed_plugins.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("plugins")
                .and_then(serde_json::Value::as_object)
                .cloned()
        })
    else {
        return std::collections::BTreeSet::new();
    };

    let mut ids = std::collections::BTreeSet::new();
    for (key, value) in plugins {
        if let Some(records) = value.as_array() {
            // Current schema: `plugins["name@marketplace"] = [records...]`.
            // Match `installed_json_items`: empty arrays and arrays containing
            // only malformed scalar records render no installed row, so they
            // must not suppress a healthy marketplace entry.
            if records.iter().any(serde_json::Value::is_object) {
                ids.insert(key);
            }
        } else if let Some(nested) = value.as_object() {
            // Legacy schema: `plugins["marketplace"]["name"] = record`.
            // Mixed-schema rendering skips malformed legacy scalar records;
            // apply the identical rule to availability suppression.
            ids.extend(
                nested
                    .iter()
                    .filter(|(_, record)| record.is_object())
                    .map(|(name, _)| format!("{name}@{key}")),
            );
        }
    }
    ids
}

/// Convert a marketplace catalog entry to the public `--available` JSON shape.
/// Unknown catalog metadata is intentionally not copied: Claude's command
/// exposes only these stable fields.
fn available_json_entry(
    entry: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    marketplace: &str,
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "pluginId".to_string(),
        serde_json::Value::String(format!("{name}@{marketplace}")),
    );
    out.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    if let Some(description) = entry.get("description") {
        out.insert("description".to_string(), description.clone());
    }
    out.insert(
        "marketplaceName".to_string(),
        serde_json::Value::String(marketplace.to_string()),
    );
    for key in ["version", "source", "installCount"] {
        if let Some(value) = entry.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Enumerate locally-resolved marketplace catalogs.
///
/// A broken marketplace must not hide healthy catalogs. It is skipped with a
/// diagnostic returned to the caller (stderr), while duplicate ids keep the
/// first entry and report the conflict. This makes corruption visible without
/// making a read-only list unusable.
fn available_plugins(plugins_dir: &std::path::Path) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let registry_path = plugins_dir.join("known_marketplaces.json");
    let registry = match std::fs::read_to_string(&registry_path) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(registry)) => registry,
            Ok(_) => {
                diagnostics.push(format!(
                    "{} must contain a JSON object",
                    registry_path.display()
                ));
                return (Vec::new(), diagnostics);
            }
            Err(error) => {
                diagnostics.push(format!(
                    "failed to parse {}: {error}",
                    registry_path.display()
                ));
                return (Vec::new(), diagnostics);
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), diagnostics);
        }
        Err(error) => {
            diagnostics.push(format!(
                "failed to read {}: {error}",
                registry_path.display()
            ));
            return (Vec::new(), diagnostics);
        }
    };

    let installed = installed_plugin_ids(plugins_dir);
    let mut seen = std::collections::BTreeSet::new();
    let mut available = Vec::new();

    for (marketplace, registry_entry) in registry {
        let Some(root) = registry_entry
            .get("installLocation")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            diagnostics.push(format!(
                "marketplace '{marketplace}' has no installLocation; skipping"
            ));
            continue;
        };
        let manifest_path = std::path::Path::new(root)
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("marketplace.json");
        let catalog = match std::fs::read_to_string(&manifest_path) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(value) => value,
                Err(error) => {
                    diagnostics.push(format!(
                        "failed to parse marketplace '{marketplace}' at {}: {error}",
                        manifest_path.display()
                    ));
                    continue;
                }
            },
            Err(error) => {
                diagnostics.push(format!(
                    "failed to read marketplace '{marketplace}' at {}: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let Some(entries) = catalog.get("plugins").and_then(serde_json::Value::as_array) else {
            diagnostics.push(format!(
                "marketplace '{marketplace}' manifest field 'plugins' must be an array; skipping"
            ));
            continue;
        };

        for (index, entry) in entries.iter().enumerate() {
            let Some(entry) = entry.as_object() else {
                diagnostics.push(format!(
                    "marketplace '{marketplace}' plugins[{index}] must be an object; skipping"
                ));
                continue;
            };
            let Some(name) = entry
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
            else {
                diagnostics.push(format!(
                    "marketplace '{marketplace}' plugins[{index}] has no non-empty name; skipping"
                ));
                continue;
            };
            let id = format!("{name}@{marketplace}");
            if installed.contains(&id) {
                continue;
            }
            if !seen.insert(id.clone()) {
                diagnostics.push(format!(
                    "duplicate plugin id '{id}' in marketplace catalog; keeping first entry"
                ));
                continue;
            }
            available.push(available_json_entry(entry, name, &marketplace));
        }
    }

    (available, diagnostics)
}

/// `plugin details <name>` — show the component inventory of an installed
/// plugin. Read-only; resolves the named plugin from the install record and
/// counts the components auto-detected by the discovery loader.
async fn run_details(args: &DetailsArgs) -> i32 {
    run_details_with_bus(args, None).await
}

async fn run_details_with_bus(
    args: &DetailsArgs,
    analytics_bus: Option<&Arc<AnalyticsBus>>,
) -> i32 {
    let dir = plugins_dir();
    let discovered = plugin::discover_recorded_plugins(&dir).await;

    // `name` may be bare (`foo`) or qualified (`foo@marketplace`); match on the
    // manifest's own name (the discovery loader stores the manifest `name`).
    let wanted = args.name.split('@').next().unwrap_or(&args.name);
    let Some((_, manifest, path)) = discovered.iter().find(|(_, m, _)| m.name == wanted) else {
        emit_plugin_command(
            analytics_bus,
            telemetry::tengu::plugin::DETAILS_COMMAND,
            PluginCommandOutcome::Failure,
            PluginCommandTelemetry::default(),
        )
        .await;
        emit_plugin_command_failed(analytics_bus, "details", "auto", "not_found").await;
        eprintln!("lingxi-cli plugin details: plugin not found: {}", args.name);
        return RUNTIME_ERROR;
    };

    let c = &manifest.components;
    emit_plugin_command(
        analytics_bus,
        telemetry::tengu::plugin::DETAILS_COMMAND,
        PluginCommandOutcome::Success,
        PluginCommandTelemetry {
            component_count: (c.commands.len()
                + c.agents.len()
                + c.skills.len()
                + c.output_styles.len()
                + c.hooks.len()
                + c.mcp_servers.len()
                + c.lsp_servers.len()) as u64,
            ..PluginCommandTelemetry::default()
        },
    )
    .await;
    println!("{}", manifest.name);
    if !manifest.version.is_empty() {
        println!("  version: {}", manifest.version);
    }
    if !manifest.description.is_empty() {
        println!("  description: {}", manifest.description);
    }
    if let Some(author) = &manifest.author {
        println!("  author: {author}");
    }
    println!("  path: {}", path.display());
    println!("  components:");
    println!("    commands: {}", c.commands.len());
    println!("    agents: {}", c.agents.len());
    println!("    skills: {}", c.skills.len());
    println!("    output-styles: {}", c.output_styles.len());
    println!("    hooks: {}", c.hooks.len());
    println!("    mcp-servers: {}", c.mcp_servers.len());
    println!("    lsp-servers: {}", c.lsp_servers.len());

    // Projected token cost is computed by claude from the loaded component
    // markdown; that estimator is not ported here, so we do not fabricate a
    // number. The component inventory above is the read-only, faithful part.
    SUCCESS
}

/// `plugin validate <path>` — validate a plugin or marketplace manifest on
/// disk. Pure local validation: no network, no install. Resolves the manifest
/// (`.lingxi-plugin/plugin.json` or `.lingxi-plugin/marketplace.json`, whether
/// `path` is the plugin root or the manifest file itself), parses it as JSON,
/// validates its declared and auto-discovered components, and checks component
/// frontmatter. `--strict` promotes warnings (unrecognized-but-tolerated
/// shapes) to a non-zero exit.
async fn run_validate(args: &ValidateArgs) -> i32 {
    let root = std::path::Path::new(&args.path);

    // Resolve the manifest path. Accept either a directory (probe its
    // `.lingxi-plugin/{plugin,marketplace}.json`) or a direct manifest file.
    let (manifest_path, kind) = match resolve_manifest(root).await {
        Some(v) => v,
        None if root.is_dir() => {
            let mut errors = Vec::new();
            let mut warnings = Vec::new();
            validate_component_directory(root, "", &mut errors, &mut warnings).await;
            println!("Validating components in: {}", root.display());
            let result = finish_validation(
                &root.display().to_string(),
                "components",
                args.strict,
                errors,
                warnings,
            );
            if result == SUCCESS {
                println!("OK: {} is a valid components directory", root.display());
            }
            return result;
        }
        None => {
            eprintln!(
                "lingxi-cli plugin validate: no plugin.json or marketplace.json found at {}",
                args.path
            );
            return RUNTIME_ERROR;
        }
    };

    let raw = match tokio::fs::read_to_string(&manifest_path).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "lingxi-cli plugin validate: cannot read {}: {e}",
                manifest_path.display()
            );
            return RUNTIME_ERROR;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "lingxi-cli plugin validate: invalid JSON in {}: {e}",
                manifest_path.display()
            );
            return RUNTIME_ERROR;
        }
    };

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    match kind {
        ManifestKind::Plugin => {
            validate_plugin_manifest_fields(&value, &mut errors, &mut warnings);
            let plugin_root = manifest_root(&manifest_path);
            let plugin_root = match resolve_safe_path(&plugin_root, Path::new(".")) {
                Ok(path) => path,
                Err(error) => {
                    push_safe_path_error(&mut errors, "", "plugin root", error);
                    plugin_root
                }
            };
            validate_component_directory_with_manifest(
                &plugin_root,
                &value,
                "",
                &mut errors,
                &mut warnings,
            )
            .await;
        }
        ManifestKind::Marketplace => {
            let marketplace_root = manifest_root(&manifest_path);
            validate_marketplace_manifest(&marketplace_root, &value, &mut errors, &mut warnings)
                .await;
        }
    }
    println!(
        "Validating {} manifest: {}",
        kind.label(),
        manifest_path.display()
    );
    let result = finish_validation(
        &manifest_path.display().to_string(),
        kind.label(),
        args.strict,
        errors,
        warnings,
    );
    if result != SUCCESS {
        return result;
    }
    println!(
        "OK: {} is a valid {} manifest",
        manifest_path.display(),
        kind.label()
    );
    SUCCESS
}

fn finish_validation(
    target: &str,
    _kind: &str,
    strict: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
) -> i32 {
    for error in &errors {
        eprintln!("error: {error}");
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    if !errors.is_empty() {
        eprintln!("Validation failed for {target}");
        return RUNTIME_ERROR;
    }
    if strict && !warnings.is_empty() {
        eprintln!("Validation failed (--strict) for {target}");
        return RUNTIME_ERROR;
    }
    SUCCESS
}

fn manifest_root(manifest_path: &Path) -> PathBuf {
    let parent = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if parent.file_name().and_then(|name| name.to_str()) == Some(branding::PLUGIN_MANIFEST_DIR) {
        parent
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

fn json_kind(value: Option<&serde_json::Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(serde_json::Value::Null) => "null",
        Some(serde_json::Value::Bool(_)) => "boolean",
        Some(serde_json::Value::Number(_)) => "number",
        Some(serde_json::Value::String(_)) => "string",
        Some(serde_json::Value::Array(_)) => "array",
        Some(serde_json::Value::Object(_)) => "object",
    }
}

fn validate_plugin_manifest_fields(
    value: &serde_json::Value,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        errors.push(format!(
            "manifest: Invalid input: expected object, received {}",
            json_kind(Some(value))
        ));
        return;
    };

    match map.get("name") {
        Some(serde_json::Value::String(name)) if !name.is_empty() => {
            if let Err(reason) = plugin::validate_plugin_name(name) {
                errors.push(format!("name: {reason}"));
            }
        }
        Some(serde_json::Value::String(_)) => {
            errors.push("name: Plugin name cannot be empty".to_string());
        }
        None => errors.push("name: Invalid input: expected string, received undefined".to_string()),
        Some(value) => errors.push(format!(
            "name: Invalid input: expected string, received {}",
            json_kind(Some(value))
        )),
    }

    validate_optional_string(map, "version", errors);
    validate_optional_string(map, "description", errors);
    validate_optional_string(map, "displayName", errors);
    validate_optional_string(map, "homepage", errors);
    validate_optional_string(map, "repository", errors);
    if is_missing_or_empty_string(map.get("version")) {
        warnings.push(
            "version: No version specified. Consider adding a version following semver (e.g., \"1.0.0\")"
                .to_string(),
        );
    }
    if is_missing_or_empty_string(map.get("description")) {
        warnings.push(
            "description: No description provided. Adding a description helps users understand what your plugin does"
                .to_string(),
        );
    }

    match map.get("author") {
        None => warnings.push(
            "author: No author information provided. Consider adding author details for plugin attribution"
                .to_string(),
        ),
        Some(serde_json::Value::Object(author)) => match author.get("name") {
            Some(serde_json::Value::String(name)) if !name.is_empty() => {}
            Some(serde_json::Value::String(_)) => {
                errors.push("author.name: Author name cannot be empty".to_string())
            }
            other => errors.push(format!(
                "author.name: Invalid input: expected string, received {}",
                json_kind(other)
            )),
        },
        Some(value) => errors.push(format!(
            "author: Invalid input: expected object, received {}",
            json_kind(Some(value))
        )),
    }

    if let Some(value) = map.get("defaultEnabled") {
        if !value.is_boolean() {
            errors.push(format!(
                "defaultEnabled: Invalid input: expected boolean, received {}",
                json_kind(Some(value))
            ));
        }
    }

    // The schema is intentionally strict about a small set of known
    // top-level fields while remaining lenient about unknown keys. Validation
    // reports the latter as warnings because the runtime ignores them.
    const KNOWN: &[&str] = &[
        "name",
        "displayName",
        "defaultEnabled",
        "version",
        "description",
        "author",
        "homepage",
        "skills",
        "commands",
        "agents",
        "outputStyles",
        "themes",
        "workflows",
        "experimental",
        "binaries",
        "monitors",
        "mcpServers",
        "lspServers",
        "hooks",
        "userConfig",
        "settings",
        "channels",
        "dependencies",
        "keywords",
        "license",
        "repository",
        "metadata",
    ];
    for field in map.keys() {
        if KNOWN.contains(&field.as_str()) {
            continue;
        }
        let message = match field.as_str() {
            "source" | "strict" => format!(
                "Field '{field}' belongs in the marketplace entry (marketplace.json), not plugin.json. It's harmless here but unused — Claude Code ignores it at load time."
            ),
            "capabilities" => "'capabilities' is no longer read: what a hooks module hooks and calls on $ is read from its source and listed under its hooks.json below. Delete the field.".to_string(),
            "experimental" => unreachable!("experimental is in KNOWN"),
            _ => format!("Unknown field '{field}'. Claude Code ignores it at load time."),
        };
        warnings.push(message);
    }
    if let Some(value) = map.get("metadata") {
        if !value.is_object() {
            warnings.push(format!(
                "'metadata' must be a free-form object; got {}. It will be ignored at load time.",
                json_kind(Some(value))
            ));
        }
    }
    if let Some(value) = map.get("experimental") {
        if !value.is_object() {
            warnings.push(format!(
                "'experimental' must be an object containing component declarations; got {}. It will be ignored at load time.",
                json_kind(Some(value))
            ));
        }
    }
}

fn is_missing_or_empty_string(value: Option<&serde_json::Value>) -> bool {
    match value {
        None => true,
        Some(serde_json::Value::String(value)) => value.is_empty(),
        Some(_) => false,
    }
}

fn validate_optional_string(
    map: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    errors: &mut Vec<String>,
) {
    if let Some(value) = map.get(field) {
        if !value.is_string() {
            errors.push(format!(
                "{field}: Invalid input: expected string, received {}",
                json_kind(Some(value))
            ));
        }
    }
}

async fn validate_component_directory(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    report_nested_symlinks(&root.join("skills"), "skills", prefix, errors);
    report_nested_symlinks(&root.join("agents"), "agents", prefix, errors);
    report_nested_symlinks(&root.join("commands"), "commands", prefix, errors);
    let mut skills = collect_skill_files(&root.join("skills"), false);
    let mut agents = collect_markdown_files(&root.join("agents"));
    let mut commands = collect_markdown_files(&root.join("commands"));
    skills.sort();
    agents.sort();
    commands.sort();
    for path in skills {
        validate_component_file(&path, ComponentKind::Skill, prefix, errors, warnings).await;
    }
    for path in agents {
        validate_component_file(&path, ComponentKind::Agent, prefix, errors, warnings).await;
    }
    for path in commands {
        validate_component_file(&path, ComponentKind::Command, prefix, errors, warnings).await;
    }
}

async fn validate_component_directory_with_manifest(
    root: &Path,
    manifest: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let Some(map) = manifest.as_object() else {
        return;
    };
    let mut skills = if map.contains_key("skills") {
        Vec::new()
    } else {
        collect_skill_files(&root.join("skills"), false)
    };
    let mut agents = if map.contains_key("agents") {
        Vec::new()
    } else {
        collect_markdown_files(&root.join("agents"))
    };
    let mut commands = if map.contains_key("commands") {
        Vec::new()
    } else {
        report_nested_symlinks(&root.join("commands"), "commands", prefix, errors);
        collect_markdown_files(&root.join("commands"))
    };
    let mut output_styles = if map.contains_key("outputStyles") {
        Vec::new()
    } else {
        report_nested_symlinks(&root.join("output-styles"), "outputStyles", prefix, errors);
        collect_markdown_files(&root.join("output-styles"))
    };
    if !map.contains_key("skills") {
        report_nested_symlinks(&root.join("skills"), "skills", prefix, errors);
    }
    if !map.contains_key("agents") {
        report_nested_symlinks(&root.join("agents"), "agents", prefix, errors);
    }

    skills.extend(
        validate_declared_paths(
            root,
            "skills",
            map.get("skills"),
            ComponentKind::Skill,
            prefix,
            errors,
        )
        .await,
    );
    agents.extend(
        validate_declared_paths(
            root,
            "agents",
            map.get("agents"),
            ComponentKind::Agent,
            prefix,
            errors,
        )
        .await,
    );
    commands.extend(validate_declared_commands(root, map.get("commands"), prefix, errors).await);
    output_styles.extend(
        validate_declared_paths(
            root,
            "outputStyles",
            map.get("outputStyles"),
            ComponentKind::OutputStyle,
            prefix,
            errors,
        )
        .await,
    );
    skills.sort();
    skills.dedup();
    agents.sort();
    agents.dedup();
    commands.sort();
    commands.dedup();
    for path in skills {
        validate_component_file(&path, ComponentKind::Skill, prefix, errors, warnings).await;
    }
    for path in agents {
        validate_component_file(&path, ComponentKind::Agent, prefix, errors, warnings).await;
    }
    for path in commands {
        validate_component_file(&path, ComponentKind::Command, prefix, errors, warnings).await;
    }
    for path in output_styles {
        validate_component_file(&path, ComponentKind::OutputStyle, prefix, errors, warnings).await;
    }
    validate_manifest_component_payloads(root, map, prefix, errors, warnings).await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComponentKind {
    Skill,
    Agent,
    Command,
    OutputStyle,
}

impl ComponentKind {
    fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::Command => "command",
            Self::OutputStyle => "output style",
        }
    }
}

async fn validate_declared_paths(
    root: &Path,
    field: &str,
    value: Option<&serde_json::Value>,
    kind: ComponentKind,
    prefix: &str,
    errors: &mut Vec<String>,
) -> Vec<PathBuf> {
    let Some(value) = value else {
        return Vec::new();
    };
    let (entries, array) = match value {
        serde_json::Value::String(path) => (vec![path.as_str()], false),
        serde_json::Value::Array(paths) => {
            let Some(paths) = paths
                .iter()
                .map(serde_json::Value::as_str)
                .collect::<Option<Vec<_>>>()
            else {
                push_issue(errors, prefix, field, "Invalid input");
                return Vec::new();
            };
            (paths, true)
        }
        _ => {
            push_issue(errors, prefix, field, "Invalid input");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (index, raw) in entries.into_iter().enumerate() {
        let field_name = if array {
            format!("{field}[{index}]")
        } else {
            field.to_string()
        };
        if contains_parent_path(raw) {
            push_issue(
                errors,
                prefix,
                &field_name,
                &format!("Path contains \"..\" which could be a path traversal attempt: {raw}"),
            );
            if !raw.starts_with("./") {
                push_issue(errors, prefix, field, "Invalid input");
            }
            continue;
        }
        if !raw.starts_with("./") || Path::new(raw).is_absolute() {
            push_issue(errors, prefix, field, "Invalid input");
            continue;
        }
        let relative = Path::new(raw.trim_start_matches("./"));
        let resolved = match resolve_safe_path(root, relative) {
            Ok(path) => path,
            Err(SafePathError::Missing) => {
                push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!(
                        "Path not found: {raw}. The runtime loader will report this as a load failure."
                    ),
                );
                continue;
            }
            Err(SafePathError::Symlink(path)) => {
                push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!(
                        "Symlinked component path is not allowed: {}",
                        path.display()
                    ),
                );
                continue;
            }
            Err(SafePathError::Escapes(path)) => {
                push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!("Path resolves outside the plugin root: {}", path.display()),
                );
                continue;
            }
        };
        if kind == ComponentKind::Agent && resolved.is_dir() {
            push_issue(errors, prefix, field, "Invalid input");
            continue;
        }
        if kind == ComponentKind::Skill && resolved.is_file() {
            let detail = if resolved.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")
            {
                format!(
                    "Path is a file; skills entries must be directories containing SKILL.md — point to the parent directory '{}' instead: {raw}",
                    raw.strip_suffix("/SKILL.md").unwrap_or(raw)
                )
            } else {
                format!(
                    "Path is a file; skills entries must be directories containing SKILL.md: {raw}"
                )
            };
            push_issue(errors, prefix, &field_name, &detail);
            continue;
        }
        if kind == ComponentKind::Skill {
            report_nested_symlinks(&resolved, &field_name, prefix, errors);
            let skill_files = collect_skill_files(&resolved, true);
            if skill_files.is_empty() {
                push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!(
                        "No SKILL.md found in directory: {raw}. Skills must be directories containing SKILL.md"
                    ),
                );
            } else {
                out.extend(skill_files);
            }
        } else if resolved.is_dir() {
            report_nested_symlinks(&resolved, &field_name, prefix, errors);
            out.extend(collect_markdown_files(&resolved));
        } else {
            out.push(resolved);
        }
    }
    out
}

async fn validate_declared_commands(
    root: &Path,
    value: Option<&serde_json::Value>,
    prefix: &str,
    errors: &mut Vec<String>,
) -> Vec<PathBuf> {
    let Some(value) = value else {
        return Vec::new();
    };
    if value.is_object() {
        let mut files = Vec::new();
        for (name, entry) in value.as_object().expect("object checked above") {
            let Some(entry) = entry.as_object() else {
                push_issue(errors, prefix, &format!("commands.{name}"), "Invalid input");
                continue;
            };
            let source = entry.get("source");
            let content = entry.get("content");
            match (source, content) {
                (Some(serde_json::Value::String(path)), None) => {
                    files.extend(
                        validate_declared_paths(
                            root,
                            &format!("commands.{name}.source"),
                            Some(&serde_json::Value::String(path.clone())),
                            ComponentKind::Command,
                            prefix,
                            errors,
                        )
                        .await,
                    );
                }
                (None, Some(serde_json::Value::String(_))) => {}
                _ => push_issue(
                    errors,
                    prefix,
                    &format!("commands.{name}"),
                    "Command must have either source or content, but not both",
                ),
            }
        }
        return files;
    }
    validate_declared_paths(
        root,
        "commands",
        Some(value),
        ComponentKind::Command,
        prefix,
        errors,
    )
    .await
}

async fn validate_manifest_component_payloads(
    root: &Path,
    map: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if let Some(value) = map.get("monitors") {
        validate_monitors_value(root, value, prefix, errors, warnings).await;
    } else {
        validate_optional_monitors_file(root, prefix, errors, warnings).await;
    }

    let experimental_themes = map
        .get("experimental")
        .and_then(serde_json::Value::as_object)
        .and_then(|experimental| experimental.get("themes"));
    if let Some(value) = experimental_themes.or_else(|| map.get("themes")) {
        validate_extension_paths(
            root,
            "themes",
            value,
            "json",
            prefix,
            errors,
            warnings,
            |path, raw, errors, warnings| validate_theme_file(path, raw, prefix, errors, warnings),
        )
        .await;
    } else {
        validate_optional_theme_directory(root, prefix, errors, warnings).await;
    }

    if let Some(value) = map.get("workflows") {
        validate_extension_paths(
            root,
            "workflows",
            value,
            "js",
            prefix,
            errors,
            warnings,
            |path, raw, errors, warnings| {
                validate_workflow_file(path, raw, prefix, errors, warnings)
            },
        )
        .await;
    } else {
        validate_optional_workflow_directory(root, prefix, errors, warnings).await;
    }

    validate_optional_hooks_file(root, prefix, errors, warnings).await;
    if let Some(value) = map.get("hooks") {
        validate_hooks_value(root, value, prefix, errors, warnings).await;
    }
    validate_optional_mcp_file(root, prefix, errors, warnings).await;
    if let Some(value) = map.get("mcpServers") {
        validate_mcp_value(root, value, prefix, errors, warnings).await;
    }
    validate_optional_lsp_file(root, prefix, errors, warnings).await;
    if let Some(value) = map.get("lspServers") {
        validate_lsp_value(root, value, prefix, errors, warnings).await;
    }
    validate_optional_settings_file(root, prefix, errors).await;

    if let Some(value) = map.get("userConfig") {
        validate_user_config_value(value, prefix, errors);
    }
    if let Some(value) = map.get("settings") {
        validate_settings_value(value, prefix, errors);
    }
    if let Some(value) = map.get("channels") {
        validate_channels_value(value, prefix, errors);
    }
    if let Some(value) = map.get("dependencies") {
        validate_dependencies_value(value, prefix, errors);
    }
    if let Some(value) = map.get("binaries") {
        validate_binaries_value(value, prefix, errors, warnings);
    }
    if let Some(value) = map.get("experimental") {
        validate_experimental_value(value, prefix, errors, warnings);
    }
}

async fn validate_optional_settings_file(root: &Path, prefix: &str, errors: &mut Vec<String>) {
    let relative = Path::new("settings.json");
    let path = root.join(relative);
    if !path.exists() {
        return;
    }
    let path = match resolve_safe_path(root, relative) {
        Ok(path) => path,
        Err(error) => {
            push_safe_path_error(errors, prefix, "settings.json", error);
            return;
        }
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                "settings.json",
                &format!("Cannot read settings: {error}"),
            );
            return;
        }
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) if value.is_object() => {}
        Ok(_) => push_issue(
            errors,
            prefix,
            "settings.json",
            "Settings must be an object",
        ),
        Err(error) => push_issue(
            errors,
            prefix,
            "settings.json",
            &format!("Invalid settings JSON: {error}"),
        ),
    }
}

async fn validate_optional_monitors_file(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let path = root.join("monitors").join("monitors.json");
    if !path.exists() {
        return;
    }
    match resolve_safe_path(root, Path::new("monitors/monitors.json")) {
        Ok(path) => match tokio::fs::read_to_string(&path).await {
            Ok(raw) => validate_monitor_json(&path, &raw, prefix, errors, warnings),
            Err(error) => push_issue(
                errors,
                prefix,
                "monitors/monitors.json",
                &format!("Cannot read monitor configuration: {error}"),
            ),
        },
        Err(error) => push_safe_path_error(errors, prefix, "monitors/monitors.json", error),
    }
}

async fn validate_monitors_value(
    root: &Path,
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(raw) => {
            if !raw.starts_with("./") || !raw.ends_with(".json") {
                push_issue(errors, prefix, "monitors", "Invalid input");
                return;
            }
            let Some(path) = declared_component_file(root, "monitors", raw, prefix, errors) else {
                return;
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(raw_json) => validate_monitor_json(&path, &raw_json, prefix, errors, warnings),
                Err(error) => push_issue(
                    errors,
                    prefix,
                    "monitors",
                    &format!("Cannot read monitor configuration: {error}"),
                ),
            }
        }
        serde_json::Value::Array(entries) => {
            let mut names = std::collections::HashSet::new();
            for (index, entry) in entries.iter().enumerate() {
                if let Some(raw) = entry.as_str() {
                    if !raw.starts_with("./") || !raw.ends_with(".json") {
                        push_issue(
                            errors,
                            prefix,
                            &format!("monitors[{index}]"),
                            "Invalid input",
                        );
                        continue;
                    }
                    let Some(path) = declared_component_file(
                        root,
                        &format!("monitors[{index}]"),
                        raw,
                        prefix,
                        errors,
                    ) else {
                        continue;
                    };
                    match tokio::fs::read_to_string(&path).await {
                        Ok(raw_json) => {
                            validate_monitor_json(&path, &raw_json, prefix, errors, warnings)
                        }
                        Err(error) => push_issue(
                            errors,
                            prefix,
                            &format!("monitors[{index}]"),
                            &format!("Cannot read monitor configuration: {error}"),
                        ),
                    }
                } else if let Some(object) = entry.as_object() {
                    validate_monitor_entry(
                        object,
                        &format!("monitors[{index}]"),
                        prefix,
                        &mut names,
                        errors,
                    );
                } else {
                    push_issue(
                        errors,
                        prefix,
                        &format!("monitors[{index}]"),
                        "Invalid input",
                    );
                }
            }
        }
        _ => push_issue(errors, prefix, "monitors", "Invalid input"),
    }
}

fn validate_monitor_json(
    path: &Path,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &path.display().to_string(),
                &format!("Invalid monitor JSON: {error}"),
            );
            return;
        }
    };
    let Some(entries) = value.as_array() else {
        push_issue(
            errors,
            prefix,
            &path.display().to_string(),
            "Monitor configuration must be an array",
        );
        return;
    };
    let mut names = std::collections::HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(object) = entry.as_object() else {
            push_issue(
                errors,
                prefix,
                &format!("{}[{index}]", path.display()),
                "Monitor entry must be an object",
            );
            continue;
        };
        validate_monitor_entry(
            object,
            &format!("{}[{index}]", path.display()),
            prefix,
            &mut names,
            errors,
        );
    }
    let _ = warnings;
}

fn validate_monitor_entry(
    entry: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    prefix: &str,
    names: &mut std::collections::HashSet<String>,
    errors: &mut Vec<String>,
) {
    let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
        push_issue(errors, prefix, field, "Monitor name must be a string");
        return;
    };
    if name.is_empty() {
        push_issue(errors, prefix, field, "Monitor name must not be empty");
    } else if !names.insert(name.to_string()) {
        push_issue(
            errors,
            prefix,
            field,
            "Monitor names must be unique within a plugin",
        );
    }
    for key in ["command", "description"] {
        if entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        {
            push_issue(
                errors,
                prefix,
                field,
                &format!("Monitor {key} must be a non-empty string"),
            );
        }
    }
    if let Some(when) = entry.get("when") {
        let Some(when) = when.as_str() else {
            push_issue(errors, prefix, field, "Monitor when must be a string");
            return;
        };
        if let Err(error) = plugin::manifest::MonitorTrigger::parse(when) {
            push_issue(errors, prefix, field, &error);
        }
    }
    for key in entry.keys() {
        if !matches!(key.as_str(), "name" | "command" | "description" | "when") {
            push_issue(
                errors,
                prefix,
                field,
                &format!("Unknown monitor field: {key}"),
            );
        }
    }
}

fn declared_component_file(
    root: &Path,
    field: &str,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) -> Option<PathBuf> {
    if contains_parent_path(raw) {
        push_issue(
            errors,
            prefix,
            field,
            &format!("Path contains \"..\": {raw}"),
        );
        return None;
    }
    if !raw.starts_with("./") || Path::new(raw).is_absolute() {
        push_issue(errors, prefix, field, "Invalid input");
        return None;
    }
    match resolve_safe_path(root, Path::new(raw.trim_start_matches("./"))) {
        Ok(path) => Some(path),
        Err(error) => {
            push_safe_path_error(errors, prefix, field, error);
            None
        }
    }
}

fn push_safe_path_error(errors: &mut Vec<String>, prefix: &str, field: &str, error: SafePathError) {
    match error {
        SafePathError::Missing => push_issue(errors, prefix, field, "Path not found"),
        SafePathError::Symlink(path) => push_issue(
            errors,
            prefix,
            field,
            &format!(
                "Symlinked component path is not allowed: {}",
                path.display()
            ),
        ),
        SafePathError::Escapes(path) => push_issue(
            errors,
            prefix,
            field,
            &format!("Path resolves outside the plugin root: {}", path.display()),
        ),
    }
}

async fn validate_extension_paths<F>(
    root: &Path,
    field: &str,
    value: &serde_json::Value,
    extension: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
    validate_file: F,
) where
    F: Fn(&Path, &str, &mut Vec<String>, &mut Vec<String>),
{
    let entries = match value {
        serde_json::Value::String(path) => vec![(field.to_string(), path.as_str())],
        serde_json::Value::Array(paths) => {
            let Some(paths) = paths
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value
                        .as_str()
                        .map(|path| (format!("{field}[{index}]"), path))
                })
                .collect::<Option<Vec<_>>>()
            else {
                push_issue(errors, prefix, field, "Invalid input");
                return;
            };
            paths
        }
        _ => {
            push_issue(errors, prefix, field, "Invalid input");
            return;
        }
    };
    for (field_name, raw) in entries {
        let Some(path) = declared_component_file(root, &field_name, raw, prefix, errors) else {
            continue;
        };
        if path.is_dir() {
            report_nested_symlinks(&path, &field_name, prefix, errors);
            for file in collect_extension_files(&path, extension) {
                match std::fs::read_to_string(&file) {
                    Ok(raw) => validate_file(&file, &raw, errors, warnings),
                    Err(error) => push_issue(
                        errors,
                        prefix,
                        &file.display().to_string(),
                        &format!("Cannot read component: {error}"),
                    ),
                }
            }
        } else {
            if path
                .extension()
                .and_then(|extension_value| extension_value.to_str())
                != Some(extension)
            {
                push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!("Expected a .{extension} file or directory"),
                );
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(raw) => validate_file(&path, &raw, errors, warnings),
                Err(error) => push_issue(
                    errors,
                    prefix,
                    &field_name,
                    &format!("Cannot read component: {error}"),
                ),
            }
        }
    }
}

fn collect_extension_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() || file_type.is_symlink() {
                return None;
            }
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some(extension)).then_some(path)
        })
        .collect::<Vec<_>>();
    out.sort();
    out
}

fn validate_theme_file(
    path: &Path,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    match plugin::theme_registry::parse_theme_json(&path.display().to_string(), raw) {
        plugin::theme_registry::ThemeParseOutcome::Valid(_) => {}
        plugin::theme_registry::ThemeParseOutcome::InvalidJson => push_issue(
            errors,
            prefix,
            &path.display().to_string(),
            "Theme file contains invalid JSON",
        ),
        plugin::theme_registry::ThemeParseOutcome::WrongShape => push_issue(
            errors,
            prefix,
            &path.display().to_string(),
            "Theme file must contain a JSON object",
        ),
    }
}

fn validate_workflow_file(
    path: &Path,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    const MAX_WORKFLOW_SCRIPT_BYTES: usize = 524_288;
    if raw.len() > MAX_WORKFLOW_SCRIPT_BYTES {
        push_issue(
            errors,
            prefix,
            &path.display().to_string(),
            &format!(
                "Workflow exceeds {} bytes and will be skipped by the runtime loader",
                MAX_WORKFLOW_SCRIPT_BYTES
            ),
        );
        return;
    }
    if plugin::extract_meta_name(raw).is_none() {
        push_issue(
            errors,
            prefix,
            &path.display().to_string(),
            "Workflow must begin with a literal export const meta containing a non-empty name",
        );
    }
}

async fn validate_optional_theme_directory(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let path = root.join("themes");
    if !path.exists() {
        return;
    }
    let path = match resolve_safe_path(root, Path::new("themes")) {
        Ok(path) => path,
        Err(error) => {
            push_safe_path_error(errors, prefix, "themes", error);
            return;
        }
    };
    report_nested_symlinks(&path, "themes", prefix, errors);
    for file in collect_extension_files(&path, "json") {
        match std::fs::read_to_string(&file) {
            Ok(raw) => validate_theme_file(&file, &raw, prefix, errors, warnings),
            Err(error) => push_issue(
                errors,
                prefix,
                &file.display().to_string(),
                &format!("Cannot read theme: {error}"),
            ),
        }
    }
}

async fn validate_optional_workflow_directory(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let path = root.join("workflows");
    if !path.exists() {
        return;
    }
    let path = match resolve_safe_path(root, Path::new("workflows")) {
        Ok(path) => path,
        Err(error) => {
            push_safe_path_error(errors, prefix, "workflows", error);
            return;
        }
    };
    report_nested_symlinks(&path, "workflows", prefix, errors);
    for file in collect_extension_files(&path, "js") {
        match std::fs::read_to_string(&file) {
            Ok(raw) => validate_workflow_file(&file, &raw, prefix, errors, warnings),
            Err(error) => push_issue(
                errors,
                prefix,
                &file.display().to_string(),
                &format!("Cannot read workflow: {error}"),
            ),
        }
    }
}

async fn validate_optional_hooks_file(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let relative = Path::new("hooks/hooks.json");
    let path = root.join(relative);
    if !path.exists() {
        return;
    }
    let Ok(path) = resolve_safe_path(root, relative) else {
        push_issue(errors, prefix, "hooks/hooks.json", "Unsafe hooks path");
        return;
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(raw) => validate_hooks_json(&path, &raw, root, prefix, errors, warnings),
        Err(error) => push_issue(
            errors,
            prefix,
            "hooks/hooks.json",
            &format!("Cannot read hooks: {error}"),
        ),
    }
}

async fn validate_hooks_value(
    root: &Path,
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(raw) => {
            let Some(path) = declared_component_file(root, "hooks", raw, prefix, errors) else {
                return;
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(raw_json) => {
                    validate_hooks_json(&path, &raw_json, root, prefix, errors, warnings)
                }
                Err(error) => push_issue(
                    errors,
                    prefix,
                    "hooks",
                    &format!("Cannot read hooks: {error}"),
                ),
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(raw) = item.as_str() {
                    let field = format!("hooks[{index}]");
                    let Some(path) = declared_component_file(root, &field, raw, prefix, errors)
                    else {
                        continue;
                    };
                    match tokio::fs::read_to_string(&path).await {
                        Ok(raw_json) => {
                            validate_hooks_json(&path, &raw_json, root, prefix, errors, warnings)
                        }
                        Err(error) => push_issue(
                            errors,
                            prefix,
                            &field,
                            &format!("Cannot read hooks: {error}"),
                        ),
                    }
                } else if item.is_object() {
                    validate_hooks_object(item, root, &format!("hooks[{index}]"), prefix, errors);
                } else {
                    push_issue(errors, prefix, &format!("hooks[{index}]"), "Invalid input");
                }
            }
        }
        object if object.is_object() => {
            validate_hooks_object(object, root, "hooks", prefix, errors)
        }
        _ => push_issue(errors, prefix, "hooks", "Invalid input"),
    }
    let _ = warnings;
}

fn validate_hooks_json(
    path: &Path,
    raw: &str,
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &path.display().to_string(),
                &format!("Invalid hooks JSON: {error}"),
            );
            return;
        }
    };
    validate_hooks_object(&value, root, &path.display().to_string(), prefix, errors);
    let _ = warnings;
}

fn validate_hooks_object(
    value: &serde_json::Value,
    root: &Path,
    field: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        push_issue(errors, prefix, field, "Hooks must be an object");
        return;
    };
    if let Some(modules) = map.get("modules") {
        let Some(modules) = modules.as_array() else {
            push_issue(
                errors,
                prefix,
                field,
                "hooks.modules must be an array of paths",
            );
            return;
        };
        if modules.len() > 1 {
            push_issue(
                errors,
                prefix,
                field,
                "hooks.modules may contain at most one module",
            );
        }
        for (index, module) in modules.iter().enumerate() {
            let Some(module) = module.as_str() else {
                push_issue(
                    errors,
                    prefix,
                    &format!("{field}.modules[{index}]"),
                    "Invalid input",
                );
                continue;
            };
            let module_path = if field.ends_with("hooks.json") {
                Path::new(field)
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or(root)
                    .join(module)
            } else {
                root.join(module.trim_start_matches("./"))
            };
            let relative = module_path.strip_prefix(root).unwrap_or(Path::new(module));
            if contains_parent_path(module) || resolve_safe_path(root, relative).is_err() {
                push_issue(
                    errors,
                    prefix,
                    &format!("{field}.modules[{index}]"),
                    "Hooks module path is outside the plugin root or unsafe",
                );
            }
        }
    }
    if let Some(hooks) = map.get("hooks") {
        let wrapped = serde_json::json!({"hooks": hooks});
        if let Err(error) = hooks::loader::parse_hooks_from_settings_json(
            &wrapped.to_string(),
            hooks::HookSource::Plugin,
        ) {
            push_issue(
                errors,
                prefix,
                &format!("{field}.hooks"),
                &format!("Invalid hook definition: {error}"),
            );
        }
    } else if !map.contains_key("modules") {
        push_issue(
            errors,
            prefix,
            field,
            "hooks.json must contain hooks or modules",
        );
    }
}

async fn validate_optional_mcp_file(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let relative = Path::new(".mcp.json");
    let path = root.join(relative);
    if !path.exists() {
        return;
    }
    let Ok(path) = resolve_safe_path(root, relative) else {
        push_issue(errors, prefix, ".mcp.json", "Unsafe MCP path");
        return;
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(raw) => validate_mcp_json(&path, &raw, prefix, errors, warnings),
        Err(error) => push_issue(
            errors,
            prefix,
            ".mcp.json",
            &format!("Cannot read MCP configuration: {error}"),
        ),
    }
}

async fn validate_mcp_value(
    root: &Path,
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(raw) => {
            if raw.starts_with("http://") || raw.starts_with("https://") {
                warnings.push(format!(
                    "mcpServers: external MCP source is not fetched during validation: {raw}"
                ));
                return;
            }
            if raw.ends_with(".mcpb") || raw.ends_with(".dxt") {
                warnings.push(format!(
                    "mcpServers: MCP bundle is validated at install time: {raw}"
                ));
                return;
            }
            let Some(path) = declared_component_file(root, "mcpServers", raw, prefix, errors)
            else {
                return;
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(raw_json) => validate_mcp_json(&path, &raw_json, prefix, errors, warnings),
                Err(error) => push_issue(
                    errors,
                    prefix,
                    "mcpServers",
                    &format!("Cannot read MCP configuration: {error}"),
                ),
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(raw) = item.as_str() {
                    if raw.starts_with("http://") || raw.starts_with("https://") {
                        warnings.push(format!(
                            "mcpServers[{index}]: external MCP source is not fetched during validation: {raw}"
                        ));
                        continue;
                    }
                    if raw.ends_with(".mcpb") || raw.ends_with(".dxt") {
                        warnings.push(format!(
                            "mcpServers[{index}]: MCP bundle is validated at install time: {raw}"
                        ));
                        continue;
                    }
                    let field = format!("mcpServers[{index}]");
                    let Some(path) = declared_component_file(root, &field, raw, prefix, errors)
                    else {
                        continue;
                    };
                    match tokio::fs::read_to_string(&path).await {
                        Ok(raw_json) => {
                            validate_mcp_json(&path, &raw_json, prefix, errors, warnings)
                        }
                        Err(error) => push_issue(
                            errors,
                            prefix,
                            &field,
                            &format!("Cannot read MCP configuration: {error}"),
                        ),
                    }
                } else if item.is_object() {
                    validate_mcp_object(item, &format!("mcpServers[{index}]"), prefix, errors);
                } else {
                    push_issue(
                        errors,
                        prefix,
                        &format!("mcpServers[{index}]"),
                        "Invalid input",
                    );
                }
            }
        }
        object if object.is_object() => validate_mcp_object(value, "mcpServers", prefix, errors),
        _ => push_issue(errors, prefix, "mcpServers", "Invalid input"),
    }
}

fn validate_mcp_json(
    path: &Path,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &path.display().to_string(),
                &format!("Invalid MCP JSON: {error}"),
            );
            return;
        }
    };
    validate_mcp_object(&value, &path.display().to_string(), prefix, errors);
}

fn validate_mcp_object(
    value: &serde_json::Value,
    field: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    if mcp::parse_plugin_mcp_json_string(&value.to_string(), mcp::ConfigScope::Dynamic).is_err() {
        push_issue(errors, prefix, field, "Invalid MCP server configuration");
    }
}

async fn validate_optional_lsp_file(
    root: &Path,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let relative = Path::new(".lsp.json");
    let path = root.join(relative);
    if !path.exists() {
        return;
    }
    let Ok(path) = resolve_safe_path(root, relative) else {
        push_issue(errors, prefix, ".lsp.json", "Unsafe LSP path");
        return;
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(raw) => validate_lsp_json(&path, &raw, prefix, errors, warnings),
        Err(error) => push_issue(
            errors,
            prefix,
            ".lsp.json",
            &format!("Cannot read LSP configuration: {error}"),
        ),
    }
}

async fn validate_lsp_value(
    root: &Path,
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(raw) => {
            let Some(path) = declared_component_file(root, "lspServers", raw, prefix, errors)
            else {
                return;
            };
            match tokio::fs::read_to_string(&path).await {
                Ok(raw_json) => validate_lsp_json(&path, &raw_json, prefix, errors, warnings),
                Err(error) => push_issue(
                    errors,
                    prefix,
                    "lspServers",
                    &format!("Cannot read LSP configuration: {error}"),
                ),
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(raw) = item.as_str() {
                    let field = format!("lspServers[{index}]");
                    let Some(path) = declared_component_file(root, &field, raw, prefix, errors)
                    else {
                        continue;
                    };
                    match tokio::fs::read_to_string(&path).await {
                        Ok(raw_json) => {
                            validate_lsp_json(&path, &raw_json, prefix, errors, warnings)
                        }
                        Err(error) => push_issue(
                            errors,
                            prefix,
                            &field,
                            &format!("Cannot read LSP configuration: {error}"),
                        ),
                    }
                } else if item.is_object() {
                    validate_lsp_object(item, &format!("lspServers[{index}]"), prefix, errors);
                } else {
                    push_issue(
                        errors,
                        prefix,
                        &format!("lspServers[{index}]"),
                        "Invalid input",
                    );
                }
            }
        }
        object if object.is_object() => validate_lsp_object(value, "lspServers", prefix, errors),
        _ => push_issue(errors, prefix, "lspServers", "Invalid input"),
    }
}

fn validate_lsp_json(
    path: &Path,
    raw: &str,
    prefix: &str,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &path.display().to_string(),
                &format!("Invalid LSP JSON: {error}"),
            );
            return;
        }
    };
    validate_lsp_object(&value, &path.display().to_string(), prefix, errors);
}

fn validate_lsp_object(
    value: &serde_json::Value,
    field: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        push_issue(errors, prefix, field, "LSP servers must be an object map");
        return;
    };
    for (name, config) in map {
        let parsed = match serde_json::from_value::<platform_api::LspServerConfig>(config.clone()) {
            Ok(config) => config,
            Err(error) => {
                push_issue(
                    errors,
                    prefix,
                    &format!("{field}.{name}"),
                    &format!("Invalid LSP server configuration: {error}"),
                );
                continue;
            }
        };
        if parsed.command.trim().is_empty() {
            push_issue(
                errors,
                prefix,
                &format!("{field}.{name}"),
                "LSP command must not be empty",
            );
        }
        if parsed.extension_to_language.is_empty() {
            push_issue(
                errors,
                prefix,
                &format!("{field}.{name}"),
                "LSP extensionToLanguage must not be empty",
            );
        }
        if !matches!(parsed.transport.as_str(), "stdio" | "socket") {
            push_issue(
                errors,
                prefix,
                &format!("{field}.{name}"),
                "LSP transport must be 'stdio' or 'socket'",
            );
        }
        if parsed.startup_timeout == Some(0) || parsed.shutdown_timeout == Some(0) {
            push_issue(
                errors,
                prefix,
                &format!("{field}.{name}"),
                "LSP timeouts must be greater than zero",
            );
        }
    }
}

fn validate_user_config_value(value: &serde_json::Value, prefix: &str, errors: &mut Vec<String>) {
    let Some(map) = value.as_object() else {
        push_issue(errors, prefix, "userConfig", "Invalid input");
        return;
    };
    for (name, field) in map {
        if !is_identifier(name) {
            push_issue(
                errors,
                prefix,
                &format!("userConfig.{name}"),
                "Option keys must be valid identifiers",
            );
            continue;
        }
        if let Err(error) = serde_json::from_value::<plugin::UserConfigField>(field.clone()) {
            push_issue(
                errors,
                prefix,
                &format!("userConfig.{name}"),
                &format!("Invalid userConfig field: {error}"),
            );
        }
    }
}

fn validate_settings_value(value: &serde_json::Value, prefix: &str, errors: &mut Vec<String>) {
    if !value.is_object() {
        push_issue(errors, prefix, "settings", "Settings must be an object");
    }
}

fn validate_channels_value(value: &serde_json::Value, prefix: &str, errors: &mut Vec<String>) {
    let Some(channels) = value.as_array() else {
        push_issue(errors, prefix, "channels", "Channels must be an array");
        return;
    };
    for (index, channel) in channels.iter().enumerate() {
        let field = format!("channels[{index}]");
        let Some(channel) = channel.as_object() else {
            push_issue(errors, prefix, &field, "Channel must be an object");
            continue;
        };
        if channel
            .get("server")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        {
            push_issue(
                errors,
                prefix,
                &field,
                "Channel server must be a non-empty string",
            );
        }
        if let Some(display_name) = channel.get("displayName") {
            if !display_name.is_string() {
                push_issue(
                    errors,
                    prefix,
                    &field,
                    "Channel displayName must be a string",
                );
            }
        }
        if let Some(user_config) = channel.get("userConfig") {
            let Some(user_config) = user_config.as_object() else {
                push_issue(
                    errors,
                    prefix,
                    &field,
                    "Channel userConfig must be an object",
                );
                continue;
            };
            for (name, field_value) in user_config {
                if let Err(error) =
                    serde_json::from_value::<plugin::UserConfigField>(field_value.clone())
                {
                    push_issue(
                        errors,
                        prefix,
                        &format!("{field}.userConfig.{name}"),
                        &format!("Invalid channel userConfig field: {error}"),
                    );
                }
            }
        }
    }
}

fn validate_dependencies_value(value: &serde_json::Value, prefix: &str, errors: &mut Vec<String>) {
    if let Err(error) = plugin::parse_dependencies(Some(value)) {
        push_issue(errors, prefix, "dependencies", &error);
    }
}

fn validate_binaries_value(
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        push_issue(errors, prefix, "binaries", "Binaries must be an object map");
        return;
    };
    if map.len() > 64 {
        push_issue(
            errors,
            prefix,
            "binaries",
            "Binaries may contain at most 64 entries",
        );
    }
    for (name, entry) in map {
        let valid_name = !name.is_empty()
            && name.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || "._-".contains(character)
            });
        if !valid_name {
            warnings.push(format!(
                "binaries.{name}: invalid binary basename; runtime will ignore it"
            ));
        }
        let Some(entry) = entry.as_object() else {
            push_issue(
                errors,
                prefix,
                &format!("binaries.{name}"),
                "Binary entry must be an object",
            );
            continue;
        };
        let Some(sha256) = entry.get("sha256").and_then(serde_json::Value::as_str) else {
            push_issue(
                errors,
                prefix,
                &format!("binaries.{name}"),
                "Binary entry requires sha256",
            );
            continue;
        };
        if sha256.len() != 64
            || !sha256
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            push_issue(
                errors,
                prefix,
                &format!("binaries.{name}"),
                "sha256 must be 64 lowercase hexadecimal characters",
            );
        }
    }
}

fn validate_experimental_value(
    value: &serde_json::Value,
    prefix: &str,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        // RawExperimental intentionally preprocesses non-object values to an
        // omitted value, so the manifest-level warning is sufficient.
        return;
    };
    let Some(syntax) = map.get("syntaxHighlighting") else {
        return;
    };
    let Some(syntax) = syntax.as_object() else {
        push_issue(
            errors,
            prefix,
            "experimental.syntaxHighlighting",
            "Invalid input",
        );
        return;
    };
    let Some(languages) = syntax.get("hljsLanguages") else {
        push_issue(
            errors,
            prefix,
            "experimental.syntaxHighlighting",
            "hljsLanguages is required",
        );
        return;
    };
    let Some(languages) = languages.as_array() else {
        push_issue(
            errors,
            prefix,
            "experimental.syntaxHighlighting.hljsLanguages",
            "Invalid input",
        );
        return;
    };
    if languages.len() > 16 {
        push_issue(
            errors,
            prefix,
            "experimental.syntaxHighlighting.hljsLanguages",
            "At most 16 language entries are allowed",
        );
    }
    for (index, language) in languages.iter().enumerate() {
        let field = format!("experimental.syntaxHighlighting.hljsLanguages[{index}]");
        let Some(language) = language.as_object() else {
            push_issue(errors, prefix, &field, "Language entry must be an object");
            continue;
        };
        let Some(id) = language.get("id").and_then(serde_json::Value::as_str) else {
            push_issue(errors, prefix, &field, "Language entry id must be a string");
            continue;
        };
        if id.is_empty()
            || id.len() > 64
            || !id.chars().enumerate().all(|(position, character)| {
                if position == 0 {
                    character.is_ascii_lowercase()
                } else {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '_' | '-')
                }
            })
        {
            push_issue(errors, prefix, &field, "Language entry id is invalid");
        }
        if let Some(remote) = language.get("remote") {
            let valid = remote.as_str().is_some_and(|remote| {
                remote.len() <= 256 && (remote.starts_with("npm:") || remote.starts_with("github:"))
            });
            if !valid {
                push_issue(errors, prefix, &field, "Language entry remote is invalid");
            }
        }
        if let Some(integrity) = language.get("integrity") {
            let valid = integrity.as_str().is_some_and(|integrity| {
                integrity.len() <= 512
                    && ["sha256-", "sha384-", "sha512-"]
                        .iter()
                        .any(|prefix| integrity.starts_with(prefix))
            });
            if !valid {
                push_issue(
                    errors,
                    prefix,
                    &field,
                    "Language entry integrity is invalid",
                );
            }
        }
        for key in language.keys() {
            if !matches!(key.as_str(), "id" | "remote" | "integrity") {
                push_issue(
                    errors,
                    prefix,
                    &field,
                    &format!("Unknown language entry field: {key}"),
                );
            }
        }
    }
    for key in syntax.keys() {
        if key != "hljsLanguages" {
            push_issue(
                errors,
                prefix,
                "experimental.syntaxHighlighting",
                &format!("Unknown syntax highlighting field: {key}"),
            );
        }
    }
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(character) if character.is_ascii_alphabetic() || character == '_' => {}
        _ => return false,
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

async fn validate_component_file(
    path: &Path,
    kind: ComponentKind,
    prefix: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &path.display().to_string(),
                &format!("Cannot read component: {error}"),
            );
            return;
        }
    };
    // Keep the production parsers as the source of truth for accepted
    // frontmatter fields/coercions; this pass adds diagnostics around their
    // intentionally fail-open behavior.
    match kind {
        ComponentKind::Skill => {
            let _ = skill_api::parse_skill_markdown(
                &raw,
                path.to_path_buf(),
                skill_api::SkillSource::Plugin,
                skill_api::LoadedFrom::Plugin,
            );
        }
        ComponentKind::Agent => {
            let _ = agent::parse_agent_markdown(
                &raw,
                agent::AgentSource::Plugin,
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
                path,
            );
        }
        ComponentKind::Command => {
            let _ = command_api::parse_command_markdown(
                &raw,
                path.to_path_buf(),
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
                command_api::CommandSource::Plugin,
            );
        }
        ComponentKind::OutputStyle => {
            // The CLI does not depend directly on outputstyles; the same
            // frontmatter envelope checks below still cover malformed files,
            // while the runtime registry performs its richer parse on enable.
        }
    }

    let issue_path = path.display().to_string();
    let Some(yaml) = frontmatter_yaml(&raw) else {
        push_issue(
            warnings,
            prefix,
            &format!("{issue_path}: frontmatter"),
            "No frontmatter block found. Add YAML frontmatter between --- delimiters at the top of the file to set description and other metadata.",
        );
        return;
    };
    let yaml = match yaml {
        Ok(yaml) => yaml,
        Err(_) => {
            let runtime = kind.label();
            push_issue(
                errors,
                prefix,
                &format!("{issue_path}: frontmatter"),
                &format!(
                    "YAML frontmatter failed to parse: YAML Parse error: Unexpected EOF. At runtime this {runtime} loads {}",
                    match kind {
                        ComponentKind::Agent => "with its name taken from the filename and every other frontmatter field silently dropped.",
                        _ => "with empty metadata (all frontmatter fields silently dropped).",
                    }
                ),
            );
            return;
        }
    };
    let parsed: serde_yaml::Value = match serde_yaml::from_str(yaml) {
        Ok(parsed) => parsed,
        Err(error) => {
            push_issue(
                errors,
                prefix,
                &format!("{issue_path}: frontmatter"),
                &format!(
                    "YAML frontmatter failed to parse: YAML Parse error: {}",
                    yaml_error_summary(&error)
                ),
            );
            return;
        }
    };
    let Some(map) = parsed.as_mapping() else {
        push_issue(
            errors,
            prefix,
            &format!("{issue_path}: frontmatter"),
            "YAML frontmatter must be a mapping of fields",
        );
        return;
    };

    if matches!(kind, ComponentKind::Agent) {
        let name_key = serde_yaml::Value::String("name".to_string());
        let has_name = map
            .get(&name_key)
            .and_then(serde_yaml::Value::as_str)
            .is_some_and(|name| !name.is_empty());
        if !has_name {
            // Co-located markdown without a name is deliberately ignored by
            // the runtime parser, so validation stays silent here.
            return;
        }
    }

    let description_key = serde_yaml::Value::String("description".to_string());
    if !map.contains_key(&description_key)
        && matches!(
            kind,
            ComponentKind::Skill | ComponentKind::Agent | ComponentKind::Command
        )
    {
        push_issue(
            warnings,
            prefix,
            &format!("{issue_path}: description"),
            &format!(
                "No description in frontmatter. A description helps users and Claude understand when to use this {}.",
                kind.label()
            ),
        );
    }
    if matches!(kind, ComponentKind::Command | ComponentKind::Skill) {
        validate_allowed_tools(map, &issue_path, prefix, errors);
        validate_shell(map, &issue_path, prefix, errors);
    }
}

fn frontmatter_yaml(raw: &str) -> Option<Result<&str, ()>> {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let rest = raw.strip_prefix("---")?;
    if let Some(index) = rest.find("\n---\n") {
        return Some(Ok(&rest[..index]));
    }
    if let Some(index) = rest.find("\n---") {
        return Some(Ok(&rest[..index]));
    }
    Some(Err(()))
}

fn yaml_error_summary(error: &serde_yaml::Error) -> String {
    let text = error.to_string();
    if text.to_ascii_lowercase().contains("unexpected end")
        || text.to_ascii_lowercase().contains("end of stream")
    {
        "Unexpected EOF.".to_string()
    } else {
        text
    }
}

fn yaml_kind(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "boolean",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "array",
        serde_yaml::Value::Mapping(_) => "object",
        serde_yaml::Value::Tagged(_) => "tagged value",
    }
}

fn validate_allowed_tools(
    map: &serde_yaml::Mapping,
    issue_path: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    let key = serde_yaml::Value::String("allowed-tools".to_string());
    let alias = serde_yaml::Value::String("allowedTools".to_string());
    let Some(value) = map.get(&key).or_else(|| map.get(&alias)) else {
        return;
    };
    match value {
        serde_yaml::Value::String(_) => {}
        serde_yaml::Value::Sequence(values)
            if values
                .iter()
                .all(|value| matches!(value, serde_yaml::Value::String(_))) => {}
        serde_yaml::Value::Sequence(_) => push_issue(
            errors,
            prefix,
            &format!("{issue_path}: allowed-tools"),
            "allowed-tools array must contain only strings.",
        ),
        _ => push_issue(
            errors,
            prefix,
            &format!("{issue_path}: allowed-tools"),
            &format!(
                "allowed-tools must be a string or array of strings, got {}.",
                yaml_kind(value)
            ),
        ),
    }
}

fn validate_shell(
    map: &serde_yaml::Mapping,
    issue_path: &str,
    prefix: &str,
    errors: &mut Vec<String>,
) {
    let key = serde_yaml::Value::String("shell".to_string());
    let Some(value) = map.get(&key) else {
        return;
    };
    match value {
        serde_yaml::Value::String(shell) if matches!(shell.as_str(), "bash" | "powershell") => {}
        serde_yaml::Value::String(shell) => push_issue(
            errors,
            prefix,
            &format!("{issue_path}: shell"),
            &format!("shell must be 'bash' or 'powershell', got '{shell}'."),
        ),
        _ => push_issue(
            errors,
            prefix,
            &format!("{issue_path}: shell"),
            &format!("shell must be a string, got {}.", yaml_kind(value)),
        ),
    }
}

fn push_issue(issues: &mut Vec<String>, prefix: &str, field: &str, message: &str) {
    if prefix.is_empty() {
        issues.push(format!("{field}: {message}"));
    } else {
        issues.push(format!("{prefix}{field}: {message}"));
    }
}

fn contains_parent_path(raw: &str) -> bool {
    // Claude's validator intentionally uses a broad `path.includes("..")`
    // guard, so preserve that behavior even for names such as `foo..bar`.
    raw.contains("..")
}

#[derive(Debug)]
enum SafePathError {
    Missing,
    Symlink(PathBuf),
    Escapes(PathBuf),
}

/// Resolve a manifest-relative path without following a symlink. Validation
/// must inspect the same tree that loading will read; otherwise a symlinked
/// component can make an apparently relative declaration escape the plugin.
fn resolve_safe_path(root: &Path, relative: &Path) -> Result<PathBuf, SafePathError> {
    let root_metadata = std::fs::symlink_metadata(root).map_err(|_| SafePathError::Missing)?;
    if root_metadata.file_type().is_symlink() {
        return Err(SafePathError::Symlink(root.to_path_buf()));
    }
    let canonical_root = root.canonicalize().map_err(|_| SafePathError::Missing)?;
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => candidate.push(name),
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return Err(SafePathError::Escapes(candidate)),
        }
        let metadata = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SafePathError::Missing)
            }
            Err(_) => return Err(SafePathError::Missing),
        };
        if metadata.file_type().is_symlink() {
            return Err(SafePathError::Symlink(candidate));
        }
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|_| SafePathError::Missing)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(SafePathError::Escapes(canonical));
    }
    Ok(canonical)
}

fn report_nested_symlinks(root: &Path, field: &str, prefix: &str, errors: &mut Vec<String>) {
    for path in collect_symlink_paths(root) {
        push_issue(
            errors,
            prefix,
            field,
            &format!("Symlinked component is not allowed: {}", path.display()),
        );
    }
}

fn collect_symlink_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            out.push(current);
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            stack.push(entry.path());
        }
    }
    out.sort();
    out
}

fn collect_markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if std::fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
    }
    out
}

fn collect_skill_files(root: &Path, include_root: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        if std::fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        let skill_file = current.join("SKILL.md");
        if (include_root || current != root)
            && std::fs::symlink_metadata(&skill_file)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
        {
            out.push(skill_file);
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry
                .file_type()
                .is_ok_and(|file_type| file_type.is_dir() && !file_type.is_symlink())
            {
                stack.push(entry.path());
            }
        }
    }
    out
}

async fn validate_marketplace_manifest(
    root: &Path,
    value: &serde_json::Value,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        errors.push(format!(
            "manifest: Invalid input: expected object, received {}",
            json_kind(Some(value))
        ));
        return;
    };
    match map.get("name") {
        Some(serde_json::Value::String(name)) if !name.is_empty() => {
            if let Err(reason) = plugin::validate_marketplace_name(name) {
                errors.push(format!("name: {reason}"));
            }
        }
        Some(serde_json::Value::String(_)) | None => {
            errors.push("name: Invalid input: expected string, received undefined".to_string())
        }
        Some(value) => errors.push(format!(
            "name: Invalid input: expected string, received {}",
            json_kind(Some(value))
        )),
    }
    match map.get("owner") {
        Some(serde_json::Value::Object(owner)) => match owner.get("name") {
            Some(serde_json::Value::String(name)) if !name.is_empty() => {}
            Some(serde_json::Value::String(_)) => {
                errors.push("owner.name: Author name cannot be empty".to_string())
            }
            other => errors.push(format!(
                "owner.name: Invalid input: expected string, received {}",
                json_kind(other)
            )),
        },
        other => errors.push(format!(
            "owner: Invalid input: expected object, received {}",
            json_kind(other)
        )),
    }
    let marketplace_plugin_root = match map.get("metadata") {
        None => None,
        Some(serde_json::Value::Object(metadata)) => match metadata.get("pluginRoot") {
            None => None,
            Some(serde_json::Value::String(plugin_root)) if !plugin_root.is_empty() => {
                if contains_parent_path(plugin_root)
                    || Path::new(plugin_root).is_absolute()
                    || plugin_root.starts_with("./")
                {
                    errors.push(format!(
                        "metadata.pluginRoot: invalid path outside the marketplace root: {plugin_root}"
                    ));
                    None
                } else {
                    Some(plugin_root.clone())
                }
            }
            Some(value) => {
                errors.push(format!(
                    "metadata.pluginRoot: Invalid input: expected string, received {}",
                    json_kind(Some(value))
                ));
                None
            }
        },
        Some(value) => {
            errors.push(format!(
                "metadata: Invalid input: expected object, received {}",
                json_kind(Some(value))
            ));
            None
        }
    };
    let Some(entries) = map.get("plugins") else {
        errors.push("plugins: Invalid input: expected array, received undefined".to_string());
        return;
    };
    let Some(entries) = entries.as_array() else {
        errors.push(format!(
            "plugins: Invalid input: expected array, received {}",
            json_kind(Some(entries))
        ));
        return;
    };
    let canonical_marketplace_root = match resolve_safe_path(root, Path::new(".")) {
        Ok(path) => path,
        Err(error) => {
            push_safe_path_error(errors, "", "marketplace root", error);
            root.to_path_buf()
        }
    };
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("plugins[{index}] plugin.json → ");
        let Some(entry) = entry.as_object() else {
            errors.push(format!("plugins[{index}]: Invalid input"));
            continue;
        };
        let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
            errors.push(format!("plugins[{index}].name: Invalid input"));
            continue;
        };
        if let Err(reason) = plugin::validate_plugin_name(name) {
            errors.push(format!("plugins[{index}].name: {reason}"));
        }

        let source_value = entry.get("source");
        let mut source = match source_value {
            Some(serde_json::Value::String(source)) => Some(source.clone()),
            Some(serde_json::Value::Object(source)) => {
                let source_kind = source
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match source_kind {
                    "directory" | "file" => {
                        let path = source
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string);
                        if path.is_none() {
                            errors.push(format!(
                                "plugins[{index}].source: local source object requires path"
                            ));
                        }
                        path
                    }
                    "github" | "git" | "url" | "git-subdir" | "archive" | "npm" => {
                        warnings.push(format!(
                            "plugins[{index}].source: external source '{source_kind}' is not fetched during validation"
                        ));
                        None
                    }
                    _ => {
                        errors.push(format!(
                            "plugins[{index}].source: unsupported external source object"
                        ));
                        None
                    }
                }
            }
            Some(value) => {
                errors.push(format!(
                    "plugins[{index}].source: Invalid input: expected string or object, received {}",
                    json_kind(Some(value))
                ));
                None
            }
            None => entry
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
        if source.is_none() {
            if entry.get("external").and_then(serde_json::Value::as_bool) == Some(true) {
                warnings.push(format!(
                    "plugins[{index}].source: external plugin is not fetched during validation"
                ));
            } else if source_value.is_none() && entry.get("path").is_none() {
                errors.push(format!(
                    "plugins[{index}].source: a local source is required"
                ));
            }
            continue;
        }
        let source = source.take().expect("source checked above");
        let source = if source == "." || source.starts_with("./") {
            source
        } else if let Some(plugin_root) = marketplace_plugin_root.as_deref() {
            format!("./{plugin_root}/{source}")
        } else {
            errors.push(format!("plugins.{index}.source: Invalid input"));
            continue;
        };
        if contains_parent_path(&source) {
            errors.push(format!(
                "plugins[{index}].source: Path contains \"..\": {source}. Plugin source paths are resolved relative to the marketplace root (the directory containing {}), not relative to marketplace.json. Use \"./outside\" instead of \"../outside\".",
                branding::PLUGIN_MANIFEST_DIR
            ));
            errors.push(format!("plugins.{index}.source: Invalid input"));
            continue;
        }
        if !source.starts_with("./") && source != "." {
            errors.push(format!("plugins.{index}.source: Invalid input"));
            continue;
        }
        let plugin_root = match resolve_safe_path(
            &canonical_marketplace_root,
            Path::new(source.trim_start_matches("./")),
        ) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => {
                errors.push(format!(
                    "plugins[{index}].source: Path is not a directory: {source}"
                ));
                continue;
            }
            Err(error) => {
                push_safe_path_error(errors, "", &format!("plugins[{index}].source"), error);
                continue;
            }
        };
        let manifest_path = match resolve_safe_path(
            &plugin_root,
            Path::new(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json")
                .as_path(),
        ) {
            Ok(path) => path,
            Err(SafePathError::Missing) => {
                errors.push(format!(
                    "plugins[{index}].source: plugin.json not found at {}/plugin.json",
                    plugin_root.display()
                ));
                continue;
            }
            Err(error) => {
                push_safe_path_error(errors, "", &format!("plugins[{index}].source"), error);
                continue;
            }
        };
        if !manifest_path.is_file() {
            errors.push(format!(
                "plugins[{index}].source: plugin.json not found at {}",
                manifest_path.display()
            ));
            continue;
        }
        let Ok(raw) = tokio::fs::read_to_string(&manifest_path).await else {
            errors.push(format!(
                "plugins[{index}].source: plugin.json not found at {}",
                manifest_path.display()
            ));
            continue;
        };
        let Ok(plugin_value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            errors.push(format!(
                "plugins[{index}] plugin.json: Invalid JSON at {}",
                manifest_path.display()
            ));
            continue;
        };
        // Re-label nested diagnostics with the marketplace entry's index so
        // errors from multiple local plugins remain attributable.
        let before_errors = errors.len();
        let before_warnings = warnings.len();
        validate_plugin_manifest_fields(&plugin_value, errors, warnings);
        validate_component_directory_with_manifest(
            &plugin_root,
            &plugin_value,
            &prefix,
            errors,
            warnings,
        )
        .await;
        for issue in errors[before_errors..].iter_mut() {
            if !issue.starts_with(&prefix) {
                *issue = format!("{prefix}{issue}");
            }
        }
        for issue in warnings[before_warnings..].iter_mut() {
            if !issue.starts_with(&prefix) {
                *issue = format!("{prefix}{issue}");
            }
        }
    }
}

/// Which manifest flavor `validate` resolved.
#[derive(Clone, Copy)]
enum ManifestKind {
    Plugin,
    Marketplace,
}

impl ManifestKind {
    fn label(self) -> &'static str {
        match self {
            ManifestKind::Plugin => "plugin",
            ManifestKind::Marketplace => "marketplace",
        }
    }
}

/// Resolve `path` to a concrete manifest file + its kind.
///
/// Accepts: a direct `plugin.json`/`marketplace.json` file; a `.lingxi-plugin/`
/// directory; or a plugin/marketplace root holding `.lingxi-plugin/`.
async fn resolve_manifest(path: &std::path::Path) -> Option<(std::path::PathBuf, ManifestKind)> {
    // Direct manifest file?
    if path.is_file() {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        return match name {
            "plugin.json" => Some((path.to_path_buf(), ManifestKind::Plugin)),
            "marketplace.json" => Some((path.to_path_buf(), ManifestKind::Marketplace)),
            _ => None,
        };
    }

    // Directory: probe `<dir>/.lingxi-plugin/{plugin,marketplace}.json` and
    // also the case where `dir` already IS the `.lingxi-plugin` directory.
    for base in [path.join(branding::PLUGIN_MANIFEST_DIR), path.to_path_buf()] {
        let plugin = base.join("plugin.json");
        if tokio::fs::try_exists(&plugin).await.unwrap_or(false) {
            return Some((plugin, ManifestKind::Plugin));
        }
        let market = base.join("marketplace.json");
        if tokio::fs::try_exists(&market).await.unwrap_or(false) {
            return Some((market, ManifestKind::Marketplace));
        }
    }
    None
}

/// CLI-07 / CLI-08 / CLI-09 (cc 2.1.238): the argv-surface deltas 2.1.238 added
/// to the `plugin` family, each verified absent from the 2.1.220 baseline.
#[cfg(test)]
mod surface_2_1_238_tests {
    use crate::argv::Argv;

    fn plugin_sub(argv: &[&str]) -> super::Sub {
        Argv::from_iter(argv.iter().copied())
            .unwrap()
            .command
            .and_then(|command| match command {
                crate::commands::Commands::Plugin(plugin) => plugin.command,
                _ => None,
            })
            .expect("expected a plugin subcommand")
    }

    /// CLI-07 (cc-238.js @235156585): `-y, --yes` is NEW on `plugin install`
    /// (2.1.220's install carried only `-s, --scope` and `--config`).
    #[test]
    fn install_accepts_the_new_yes_flag() {
        let super::Sub::Install(args) =
            plugin_sub(&["lingxi-cli", "plugin", "install", "-y", "p@m"])
        else {
            panic!("expected plugin install");
        };
        assert!(args.yes);
        assert_eq!(args.plugin, "p@m");

        let super::Sub::Install(args) =
            plugin_sub(&["lingxi-cli", "plugin", "install", "--yes", "p@m"])
        else {
            panic!("expected plugin install");
        };
        assert!(args.yes);

        // Absent ⇒ false (the flag is opt-in, never implied by --scope).
        let super::Sub::Install(args) = plugin_sub(&["lingxi-cli", "plugin", "install", "p@m"])
        else {
            panic!("expected plugin install");
        };
        assert!(!args.yes);
    }

    /// CLI-08 (cc-238.js @235159327): `-y, --yes` is NEW on `plugin update`
    /// (2.1.220's update carried only `-s, --scope`).
    #[test]
    fn update_accepts_the_new_yes_flag() {
        let super::Sub::Update(args) = plugin_sub(&["lingxi-cli", "plugin", "update", "-y", "p"])
        else {
            panic!("expected plugin update");
        };
        assert!(args.yes);

        let super::Sub::Update(args) = plugin_sub(&["lingxi-cli", "plugin", "update", "p"]) else {
            panic!("expected plugin update");
        };
        assert!(!args.yes);
    }

    /// CLI-09 (binary @261478912): `uninstall` gained the `rm` alias in 2.1.238
    /// — 2.1.220's alias slot held only `remove`. All three spellings must
    /// reach the same variant.
    #[test]
    fn uninstall_answers_to_remove_and_the_new_rm_alias() {
        for spelling in ["uninstall", "remove", "rm"] {
            let super::Sub::Uninstall(args) = plugin_sub(&["lingxi-cli", "plugin", spelling, "p"])
            else {
                panic!("expected plugin uninstall for `{spelling}`");
            };
            assert_eq!(args.plugin, "p");
        }
    }
}

#[cfg(test)]
mod list_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{available_plugins, installed_json_items, installed_plugin_ids};

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().expect("fixture file parent")).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn add_marketplace(plugins_dir: &Path, root: &Path, name: &str, entries: serde_json::Value) {
        write_json(
            &root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("marketplace.json"),
            &json!({"name": name, "plugins": entries}),
        );
        let mut registry = serde_json::Map::new();
        registry.insert(
            name.to_string(),
            json!({
                "source": {"source": "directory", "path": root},
                "installLocation": root,
            }),
        );
        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &serde_json::Value::Object(registry),
        );
    }

    #[test]
    fn available_excludes_current_and_legacy_installed_ids() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let marketplace = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &marketplace,
            "example",
            json!([
                {"name":"current", "description":"already here", "source":"./current"},
                {"name":"legacy", "source":"./legacy"},
                {"name":"fresh", "description":"new", "version":"1.2.3", "source":{"source":"url", "url":"https://example.test/fresh.git"}, "installCount":7}
            ]),
        );
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "current@example": [{"scope":"user"}],
                    "example": {"legacy": {"version":"0.1.0"}}
                }
            }),
        );

        assert_eq!(
            installed_plugin_ids(&plugins_dir),
            ["current@example".to_string(), "legacy@example".to_string()]
                .into_iter()
                .collect()
        );
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            available,
            vec![json!({
                "pluginId": "fresh@example",
                "name": "fresh",
                "description": "new",
                "marketplaceName": "example",
                "version": "1.2.3",
                "source": {"source":"url", "url":"https://example.test/fresh.git"},
                "installCount": 7
            })]
        );
    }

    #[test]
    fn empty_or_malformed_install_records_do_not_hide_available_plugins() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let marketplace = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &marketplace,
            "example",
            json!([
                {"name":"valid", "source":"./valid"},
                {"name":"empty", "source":"./empty"},
                {"name":"malformed", "source":"./malformed"},
                {"name":"legacy-valid", "source":"./legacy-valid"},
                {"name":"legacy-malformed", "source":"./legacy-malformed"},
                {"name":"fresh", "source":"./fresh"}
            ]),
        );
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "valid@example": [{}],
                    "empty@example": [],
                    "malformed@example": [null, "not-an-object"],
                    "example": {
                        "legacy-valid": {"version":"0.9.0"},
                        "legacy-malformed": "not-an-object"
                    }
                }
            }),
        );

        assert_eq!(
            installed_plugin_ids(&plugins_dir),
            [
                "legacy-valid@example".to_string(),
                "valid@example".to_string()
            ]
            .into_iter()
            .collect()
        );
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let ids: Vec<&str> = available
            .iter()
            .filter_map(|entry| entry.get("pluginId").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![
                "empty@example",
                "malformed@example",
                "legacy-malformed@example",
                "fresh@example"
            ]
        );
    }

    #[test]
    fn installed_json_uses_durable_records_and_scope_settings() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugins_dir = home.join("plugins");
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "demo@example": [{
                        "scope": "user",
                        "installPath": "/missing/cache/is-still-listed",
                        "version": "1.0.0",
                        "installedAt": "2026-01-01T00:00:00.000Z",
                        "lastUpdated": "2026-01-02T00:00:00.000Z"
                    }]
                }
            }),
        );
        write_json(
            &home.join("settings.json"),
            &json!({
                "enabledPlugins": {"demo@example": true},
                "pluginConfigs": {
                    "demo@example": {"mcpServers": {"demo": {"command": "demo-mcp"}}}
                }
            }),
        );

        let (installed, diagnostics) = super::current_thread_runtime()
            .block_on(installed_json_items(&plugins_dir, &home, &cwd, None));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            installed.unwrap(),
            vec![json!({
                "id": "demo@example",
                "version": "1.0.0",
                "scope": "user",
                "enabled": true,
                "installPath": "/missing/cache/is-still-listed",
                "installedAt": "2026-01-01T00:00:00.000Z",
                "lastUpdated": "2026-01-02T00:00:00.000Z",
                "mcpServers": {"demo": {"command": "demo-mcp"}}
            })]
        );
    }

    #[test]
    fn mixed_installed_schema_keeps_v2_and_legacy_records_independently() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugins_dir = home.join("plugins");
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "current@example": [{"scope":"user", "version":"2.0.0"}],
                    "example": {
                        "legacy": {"version":"0.9.0"},
                        "broken": "not-an-object"
                    }
                }
            }),
        );

        let (installed, diagnostics) = super::current_thread_runtime()
            .block_on(installed_json_items(&plugins_dir, &home, &cwd, None));
        let installed = installed.expect("valid top-level database");
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0]["id"], "current@example");
        assert_eq!(installed[1]["id"], "legacy@example");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].contains("broken@example"));
    }

    #[test]
    fn bad_catalog_is_diagnosed_without_hiding_healthy_catalogs() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let good_root = temp.path().join("good");
        let bad_root = temp.path().join("bad");
        write_json(
            &good_root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("marketplace.json"),
            &json!({"plugins":[{"name":"ok", "source":"./ok"}]}),
        );
        let bad_manifest = bad_root
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("marketplace.json");
        fs::create_dir_all(bad_manifest.parent().unwrap()).unwrap();
        fs::write(&bad_manifest, b"{not-json").unwrap();
        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &json!({
                "bad": {"installLocation": bad_root},
                "good": {"installLocation": good_root}
            }),
        );

        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["pluginId"], "ok@good");
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("failed to parse marketplace 'bad'")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn duplicate_or_malformed_entries_are_skipped_with_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let root = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &root,
            "example",
            json!([
                {"name":"one", "source":"./one"},
                {"name":"one", "source":"./duplicate"},
                {"description":"missing name"},
                "not-an-object"
            ]),
        );

        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["source"], "./one");
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics.iter().any(|line| line.contains("duplicate")));
        assert!(diagnostics
            .iter()
            .any(|line| line.contains("non-empty name")));
        assert!(diagnostics
            .iter()
            .any(|line| line.contains("must be an object")));
    }

    #[test]
    fn absent_registry_is_an_empty_success() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = PathBuf::from(temp.path()).join("plugins");
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(available.is_empty());
        assert!(diagnostics.is_empty());
    }
}

#[cfg(test)]
pub(crate) mod telemetry_test_support {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    use tracing_subscriber::layer::SubscriberExt;

    static CLI_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    }

    impl EventCapture {
        fn records(&self) -> Vec<BTreeMap<String, String>> {
            self.events.lock().unwrap().clone()
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Visitor<'a>(&'a mut BTreeMap<String, String>);
            impl tracing::field::Visit for Visitor<'_> {
                fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                    self.0.insert(field.name().to_string(), value.to_string());
                }

                fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
                    self.0.insert(field.name().to_string(), value.to_string());
                }

                fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                    self.0.insert(field.name().to_string(), value.to_string());
                }

                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    self.0.insert(field.name().to_string(), value.to_string());
                }

                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0
                        .entry(field.name().to_string())
                        .or_insert_with(|| format!("{value:?}"));
                }
            }

            let mut record = BTreeMap::new();
            event.record(&mut Visitor(&mut record));
            self.events.lock().unwrap().push(record);
        }
    }

    pub(crate) struct IsolatedCliEnv {
        _tmp: tempfile::TempDir,
        _guard: MutexGuard<'static, ()>,
        previous_config_dir: Option<OsString>,
        previous_cwd: PathBuf,
        pub home: PathBuf,
        pub cwd: PathBuf,
    }

    impl IsolatedCliEnv {
        pub fn new() -> Self {
            let guard = CLI_ENV_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            let cwd = tmp.path().join("project");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&cwd).unwrap();
            let previous_config_dir = std::env::var_os(branding::CONFIG_DIR_ENV);
            let previous_cwd = std::env::current_dir().unwrap();
            std::env::set_var(branding::CONFIG_DIR_ENV, &home);
            std::env::set_current_dir(&cwd).unwrap();
            Self {
                _tmp: tmp,
                _guard: guard,
                previous_config_dir,
                previous_cwd,
                home,
                cwd,
            }
        }
    }

    impl Drop for IsolatedCliEnv {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.previous_cwd).unwrap();
            if let Some(previous) = self.previous_config_dir.as_ref() {
                std::env::set_var(branding::CONFIG_DIR_ENV, previous);
            } else {
                std::env::remove_var(branding::CONFIG_DIR_ENV);
            }
        }
    }

    pub(crate) fn capture_events<R>(f: impl FnOnce() -> R) -> (R, Vec<BTreeMap<String, String>>) {
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let result = tracing::subscriber::with_default(subscriber, || {
            // Callsite interest is global and may have been cached while no
            // subscriber was active by another concurrently running CLI
            // test. Rebuild only after this thread-local capture subscriber
            // is installed so the event macros cannot be skipped entirely.
            tracing::callsite::rebuild_interest_cache();
            f()
        });
        (result, capture.records())
    }

    pub(crate) fn event<'a>(
        events: &'a [BTreeMap<String, String>],
        event_name: &str,
    ) -> &'a BTreeMap<String, String> {
        events
            .iter()
            .find(|record| record.get("event").is_some_and(|event| event == event_name))
            .unwrap_or_else(|| panic!("missing telemetry event {event_name}: {events:?}"))
    }
}

#[cfg(test)]
mod telemetry_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::json;
    use telemetry::{AnalyticsBus, AnalyticsValue, InMemorySink};

    use super::telemetry_test_support::{capture_events, event, IsolatedCliEnv};
    use super::*;
    use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

    struct Env {
        _iso: IsolatedCliEnv,
        home: PathBuf,
        cwd: PathBuf,
        plugins: PathBuf,
        market: PathBuf,
    }

    fn env() -> Env {
        let iso = IsolatedCliEnv::new();
        let home = iso.home.clone();
        let cwd = iso.cwd.clone();
        let plugins = home.join("plugins");
        let market = home.join("marketplaces").join("mymkt");
        fs::create_dir_all(&plugins).unwrap();
        Env {
            _iso: iso,
            home,
            cwd,
            plugins,
            market,
        }
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn seed_marketplace(e: &Env) {
        let manifest_dir = e.market.join(branding::PLUGIN_MANIFEST_DIR);
        fs::create_dir_all(&manifest_dir).unwrap();
        write_json(
            &manifest_dir.join("marketplace.json"),
            &json!({
                "name": "mymkt",
                "owner": {"name": "me"},
                "plugins": [{"name":"hello","source":"./plugins/hello"}]
            }),
        );
        let plugin_dir = e.market.join("plugins").join("hello");
        fs::create_dir_all(plugin_dir.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin_dir.join("commands")).unwrap();
        fs::write(
            plugin_dir
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            r#"{"name":"hello","version":"1.2.3"}"#,
        )
        .unwrap();
        fs::write(plugin_dir.join("commands").join("hi.md"), "# hi").unwrap();
        write_json(
            &e.plugins.join("known_marketplaces.json"),
            &json!({
                "mymkt": {
                    "source": {"source":"directory", "path": e.market.display().to_string()},
                    "installLocation": e.market.display().to_string()
                }
            }),
        );
    }

    fn seed_installed_plugin(e: &Env, id: &str, version: &str, auto_installed: bool) -> PathBuf {
        let (name, market) = id.split_once('@').unwrap();
        let install_path = e
            .plugins
            .join("cache")
            .join(market)
            .join(name)
            .join(version);
        fs::create_dir_all(install_path.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::write(
            install_path
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            format!(r#"{{"name":"{name}","version":"{version}"}}"#),
        )
        .unwrap();
        fs::create_dir_all(install_path.join("commands")).unwrap();
        fs::write(install_path.join("commands").join("hi.md"), "# hi").unwrap();
        write_json(
            &e.plugins.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    id: [{
                        "scope": "user",
                        "installPath": install_path.display().to_string(),
                        "version": version,
                        "installedAt": "2026-09-01T00:00:00.000Z",
                        "lastUpdated": "2026-09-01T00:00:00.000Z",
                        "autoInstalled": auto_installed
                    }]
                }
            }),
        );
        install_path
    }

    fn current_thread_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn runtime_bus() -> (
        tokio::runtime::Runtime,
        Arc<AnalyticsBus>,
        Arc<InMemorySink>,
    ) {
        let runtime = current_thread_runtime();
        let sink = Arc::new(InMemorySink::new());
        let bus = Arc::new(AnalyticsBus::new());
        runtime.block_on(bus.attach_sink(sink.clone() as Arc<dyn telemetry::AnalyticsSink>));
        (runtime, bus, sink)
    }

    fn bus_event<'a>(
        events: &'a [telemetry::sinks::in_memory::RecordedEvent],
        name: &str,
    ) -> &'a telemetry::sinks::in_memory::RecordedEvent {
        events
            .iter()
            .find(|event| event.name == name)
            .unwrap_or_else(|| panic!("missing telemetry event {name}: {events:?}"))
    }

    fn string_field<'a>(metadata: &'a telemetry::LogEventMetadata, key: &str) -> &'a str {
        match metadata.get(key) {
            Some(AnalyticsValue::String(value)) => value.as_str(),
            other => panic!("missing string field {key}: {other:?}"),
        }
    }

    fn int_field(metadata: &telemetry::LogEventMetadata, key: &str) -> i64 {
        match metadata.get(key) {
            Some(AnalyticsValue::Int(value)) => *value,
            other => panic!("missing int field {key}: {other:?}"),
        }
    }

    fn run_cli_with_bus(cli: Cli) -> (i32, Vec<telemetry::sinks::in_memory::RecordedEvent>) {
        let (runtime, bus, sink) = runtime_bus();
        let code = runtime.block_on(run_with_shared_analytics_bus(&cli, bus));
        let events = runtime.block_on(sink.events());
        (code, events)
    }

    #[test]
    fn list_empty_emits_noop() {
        let _env = env();
        let args = ListArgs {
            available: false,
            json: false,
        };
        let runtime = current_thread_runtime();
        let (code, events) = capture_events(|| runtime.block_on(run_list(&args)));
        assert_eq!(code, SUCCESS);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::LIST_COMMAND)["outcome"],
            "noop"
        );
    }

    #[test]
    fn details_success_emits_success() {
        let e = env();
        seed_installed_plugin(&e, "hello@mymkt", "1.2.3", false);
        let args = DetailsArgs {
            name: "hello".to_string(),
        };
        let runtime = current_thread_runtime();
        let (code, events) = capture_events(|| runtime.block_on(run_details(&args)));
        assert_eq!(code, SUCCESS);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::DETAILS_COMMAND)["outcome"],
            "success"
        );
    }

    #[test]
    fn enable_success_emits_command_and_cli_events() {
        let e = env();
        seed_installed_plugin(&e, "hello@mymkt", "1.2.3", false);
        let args = EnableArgs {
            scope: None,
            plugin: "hello@mymkt".to_string(),
        };
        let (code, events) = capture_events(|| run_enable(&args));
        assert_eq!(code, SUCCESS);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::ENABLE_COMMAND)["outcome"],
            "success"
        );
        assert_eq!(
            event(&events, telemetry::tengu::plugin::ENABLED_CLI)["outcome"],
            "success"
        );
    }

    #[test]
    fn disable_noop_emits_noop_without_failure_event() {
        let e = env();
        seed_installed_plugin(&e, "hello@mymkt", "1.2.3", false);
        let args = DisableArgs {
            all: false,
            scope: None,
            plugin: Some("hello@mymkt".to_string()),
        };
        let (code, events) = capture_events(|| run_disable(&args));
        assert_eq!(code, RUNTIME_ERROR);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::DISABLE_COMMAND)["outcome"],
            "noop"
        );
        assert!(
            !events.iter().any(|record| {
                record
                    .get("event")
                    .is_some_and(|event| event == telemetry::tengu::plugin::COMMAND_FAILED)
            }),
            "{events:?}"
        );
    }

    #[test]
    fn uninstall_failure_emits_failure_and_command_failed() {
        let e = env();
        seed_marketplace(&e);
        let args = UninstallArgs {
            keep_data: false,
            prune: false,
            scope: "user".to_string(),
            yes: true,
            plugin: "hello@mymkt".to_string(),
        };
        let runtime = current_thread_runtime();
        let (code, events) =
            capture_events(|| runtime.block_on(run_uninstall_command(&args, None)));
        assert_eq!(code, RUNTIME_ERROR);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::UNINSTALL_COMMAND)["outcome"],
            "failure"
        );
        assert_eq!(
            event(&events, telemetry::tengu::plugin::COMMAND_FAILED)["outcome"],
            "failure"
        );
    }

    #[test]
    fn update_noop_emits_noop() {
        let e = env();
        seed_marketplace(&e);
        crate::commands::plugin_install::run_install(
            "hello@mymkt",
            Some("user"),
            &[],
            &e.plugins,
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let args = UpdateArgs {
            scope: "user".to_string(),
            yes: false,
            plugin: "hello@mymkt".to_string(),
        };
        let (code, events) = capture_events(|| run_update_command(&args));
        assert_eq!(code, SUCCESS);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::UPDATE_COMMAND)["outcome"],
            "noop"
        );
    }

    #[test]
    fn prune_success_emits_command_success_and_prune_cli() {
        let e = env();
        let install_path = seed_installed_plugin(&e, "dep@mymkt", "1.0.0", true);
        fs::write(
            e.home.join("settings.json"),
            r#"{"enabledPlugins":{"dep@mymkt":true}}"#,
        )
        .unwrap();
        let args = PruneArgs {
            dry_run: false,
            scope: "user".to_string(),
            yes: true,
        };
        let (code, events) = capture_events(|| run_prune_command(&args));
        assert_eq!(code, SUCCESS);
        assert_eq!(
            event(&events, telemetry::tengu::plugin::PRUNE_COMMAND)["outcome"],
            "success"
        );
        assert_eq!(
            event(&events, telemetry::tengu::plugin::PRUNE_CLI)["removed_count"],
            "1"
        );
        assert!(!install_path.exists());
    }

    #[test]
    fn install_success_reaches_analytics_bus() {
        let e = env();
        seed_marketplace(&e);
        let (code, events) = run_cli_with_bus(Cli {
            command: Some(Sub::Install(InstallArgs {
                config: Vec::new(),
                scope: "user".to_string(),
                yes: true,
                plugin: "hello@mymkt".to_string(),
            })),
        });
        assert_eq!(code, SUCCESS);
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::INSTALL_COMMAND).metadata,
                "outcome",
            ),
            "success"
        );
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::INSTALLED_CLI).metadata,
                "outcome",
            ),
            "success"
        );
    }

    #[test]
    fn uninstall_failure_reaches_analytics_bus() {
        let e = env();
        seed_marketplace(&e);
        let (code, events) = run_cli_with_bus(Cli {
            command: Some(Sub::Uninstall(UninstallArgs {
                keep_data: false,
                prune: false,
                scope: "user".to_string(),
                yes: true,
                plugin: "hello@mymkt".to_string(),
            })),
        });
        assert_eq!(code, RUNTIME_ERROR);
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::UNINSTALL_COMMAND).metadata,
                "outcome",
            ),
            "failure"
        );
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::COMMAND_FAILED).metadata,
                "command",
            ),
            "uninstall"
        );
    }

    #[test]
    fn disable_noop_reaches_analytics_bus_without_command_failed() {
        let e = env();
        seed_installed_plugin(&e, "hello@mymkt", "1.2.3", false);
        let (code, events) = run_cli_with_bus(Cli {
            command: Some(Sub::Disable(DisableArgs {
                all: false,
                scope: None,
                plugin: Some("hello@mymkt".to_string()),
            })),
        });
        assert_eq!(code, RUNTIME_ERROR);
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::DISABLE_COMMAND).metadata,
                "outcome",
            ),
            "noop"
        );
        assert!(
            events
                .iter()
                .all(|event| event.name != telemetry::tengu::plugin::COMMAND_FAILED),
            "{events:?}"
        );
    }

    #[test]
    fn list_read_error_reaches_analytics_bus() {
        let e = env();
        fs::create_dir_all(e.plugins.join("installed_plugins.json")).unwrap();
        let (code, events) = run_cli_with_bus(Cli {
            command: Some(Sub::List(ListArgs {
                available: false,
                json: true,
            })),
        });
        assert_eq!(code, SUCCESS);
        let state_file_error = bus_event(&events, telemetry::tengu::plugin::STATE_FILE_ERROR);
        assert_eq!(string_field(&state_file_error.metadata, "command"), "list");
        assert_eq!(
            string_field(&state_file_error.metadata, "operation"),
            "read"
        );
        assert_eq!(string_field(&state_file_error.metadata, "error_kind"), "io");
    }

    #[test]
    fn prune_success_reaches_analytics_bus() {
        let e = env();
        let install_path = seed_installed_plugin(&e, "dep@mymkt", "1.0.0", true);
        fs::write(
            e.home.join("settings.json"),
            r#"{"enabledPlugins":{"dep@mymkt":true}}"#,
        )
        .unwrap();
        let (code, events) = run_cli_with_bus(Cli {
            command: Some(Sub::Prune(PruneArgs {
                dry_run: false,
                scope: "user".to_string(),
                yes: true,
            })),
        });
        assert_eq!(code, SUCCESS);
        assert_eq!(
            string_field(
                &bus_event(&events, telemetry::tengu::plugin::PRUNE_COMMAND).metadata,
                "outcome",
            ),
            "success"
        );
        assert_eq!(
            int_field(
                &bus_event(&events, telemetry::tengu::plugin::PRUNE_CLI).metadata,
                "removed_count",
            ),
            1
        );
        assert!(!install_path.exists());
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn deep_validation_aggregates_paths_and_component_frontmatter() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("commands")).unwrap();
        fs::create_dir_all(root.join("skills/demo")).unwrap();
        fs::write(
            root.join("commands/bad.md"),
            "---\ndescription: \"unterminated\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            root.join("skills/demo/SKILL.md"),
            "---\nname: demo\n---\nbody\n",
        )
        .unwrap();
        let manifest = json!({
            "name": "demo",
            "commands": ["../outside", "./commands"],
            "skills": "./skills"
        });
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_plugin_manifest_fields(&manifest, &mut errors, &mut warnings);
        validate_component_directory_with_manifest(root, &manifest, "", &mut errors, &mut warnings)
            .await;

        assert!(
            errors
                .iter()
                .any(|error| error.contains("commands[0]: Path contains \"..\"")),
            "path traversal must be reported with its array index: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.contains("YAML frontmatter failed to parse")),
            "malformed command YAML must be aggregated: {errors:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("skills/demo/SKILL.md: description")),
            "skill metadata warnings must include the resolved component: {warnings:?}"
        );
    }

    #[tokio::test]
    async fn marketplace_validation_walks_local_plugin_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let plugin_root = root.join("plugins/demo");
        fs::create_dir_all(plugin_root.join(branding::PLUGIN_MANIFEST_DIR)).unwrap();
        fs::create_dir_all(plugin_root.join("commands")).unwrap();
        fs::write(
            plugin_root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("plugin.json"),
            serde_json::to_vec(&json!({
                "name": "demo",
                "commands": "./commands"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            plugin_root.join("commands/bad.md"),
            "---\nallowed-tools: [Read, 7]\n---\nbody\n",
        )
        .unwrap();
        let manifest = json!({
            "name": "catalog",
            "owner": {"name": "owner"},
            "plugins": [{"name": "demo", "source": "./plugins/demo"}]
        });
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_marketplace_manifest(root, &manifest, &mut errors, &mut warnings).await;

        assert!(
            errors.iter().any(|error| {
                error.contains("plugins[0] plugin.json")
                    && error.contains("allowed-tools array must contain only strings")
            }),
            "nested component errors must identify their marketplace entry: {errors:?}"
        );
        assert!(!errors.is_empty());
    }

    #[tokio::test]
    async fn standalone_component_directories_are_accepted_without_a_manifest() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("commands")).unwrap();
        fs::write(
            temp.path().join("commands/hello.md"),
            "---\ndescription: hi\n---\n",
        )
        .unwrap();
        assert!(resolve_manifest(temp.path()).await.is_none());
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_component_directory(temp.path(), "", &mut errors, &mut warnings).await;
        assert!(errors.is_empty(), "{errors:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            finish_validation("target", "components", false, Vec::new(), Vec::new()),
            SUCCESS
        );
    }

    #[tokio::test]
    async fn explicitly_declared_skill_directory_requires_skill_md() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("skills/incomplete")).unwrap();
        fs::write(
            temp.path().join("skills/incomplete/README.md"),
            "This is not a skill entry.",
        )
        .unwrap();
        let manifest = json!({"name": "demo", "skills": "./skills/incomplete"});
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_component_directory_with_manifest(
            temp.path(),
            &manifest,
            "",
            &mut errors,
            &mut warnings,
        )
        .await;
        assert!(
            errors
                .iter()
                .any(|error| error.contains("No SKILL.md found")),
            "missing skill entry must be diagnosed: {errors:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn validation_rejects_symlinked_component_and_marketplace_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugin");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("commands")).unwrap();
        fs::create_dir_all(root.join("skills/demo")).unwrap();
        fs::create_dir_all(root.join("declared-target")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("escape.md"), "---\ndescription: escape\n---\n").unwrap();
        fs::write(outside.join("SKILL.md"), "---\ndescription: escape\n---\n").unwrap();
        fs::write(
            root.join("declared-target/SKILL.md"),
            "---\ndescription: target\n---\n",
        )
        .unwrap();
        symlink(outside.join("escape.md"), root.join("commands/escape.md")).unwrap();
        symlink(outside.join("SKILL.md"), root.join("skills/demo/SKILL.md")).unwrap();
        symlink(root.join("declared-target"), root.join("declared-link")).unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_component_directory(&root, "", &mut errors, &mut warnings).await;
        assert!(
            errors
                .iter()
                .filter(|error| error.contains("Symlinked component"))
                .count()
                >= 2,
            "nested markdown/SKILL symlinks must be rejected before reads: {errors:?}"
        );

        let manifest = json!({"name": "demo", "skills": "./declared-link"});
        errors.clear();
        warnings.clear();
        validate_component_directory_with_manifest(
            &root,
            &manifest,
            "",
            &mut errors,
            &mut warnings,
        )
        .await;
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Symlinked component path")),
            "declared component directory symlink must be rejected: {errors:?}"
        );

        let market = temp.path().join("market");
        fs::create_dir_all(&market).unwrap();
        symlink(&root, market.join("plugin-link")).unwrap();
        let marketplace = json!({
            "name": "catalog",
            "owner": {"name": "owner"},
            "plugins": [{"name": "demo", "source": "./plugin-link"}]
        });
        errors.clear();
        warnings.clear();
        validate_marketplace_manifest(&market, &marketplace, &mut errors, &mut warnings).await;
        assert!(
            errors
                .iter()
                .any(|error| error.contains("Symlinked component path")),
            "marketplace plugin source symlink must be rejected: {errors:?}"
        );
    }

    #[tokio::test]
    async fn validation_walks_non_markdown_component_slots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("themes")).unwrap();
        fs::create_dir_all(root.join("workflows")).unwrap();
        fs::write(root.join("themes/bad.json"), "[1, 2, 3]").unwrap();
        fs::write(
            root.join("workflows/bad.js"),
            "export const notMeta = true;",
        )
        .unwrap();
        fs::write(
            root.join(".lsp.json"),
            "{\"rust\": {\"command\": \"\", \"extensionToLanguage\": {}}}",
        )
        .unwrap();
        fs::write(root.join("settings.json"), "[]").unwrap();
        let manifest = json!({
            "name": "demo",
            "mcpServers": {"bad": {"command": "echo"}},
            "lspServers": {"rust": {"command": "", "extensionToLanguage": {}}},
            "channels": [{"server": 7}],
            "dependencies": [7],
            "binaries": {"tool": {"sha256": "bad"}}
        });
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_plugin_manifest_fields(&manifest, &mut errors, &mut warnings);
        validate_component_directory_with_manifest(root, &manifest, "", &mut errors, &mut warnings)
            .await;
        assert!(
            errors.iter().any(|error| error.contains("Theme file")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("Workflow")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("LSP")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("settings.json")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("channels")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("dependencies")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("binaries")),
            "{errors:?}"
        );
    }
}
