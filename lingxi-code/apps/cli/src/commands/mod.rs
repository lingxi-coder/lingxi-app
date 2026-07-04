//! Top-level subcommand layer — byte-parity with claude-code 2.1.191.
//!
//! claude-code registers `mcp`, `auth`, `agents`, `auto-mode`, `doctor`,
//! `gateway` (2.1.198), `install`, `plugin|plugins`, `project`, `setup-token`,
//! `ultrareview`, and `update|upgrade` as commander subcommands. lingxi-cli historically had NO
//! subcommand layer (flat clap struct), so a bare token like `mcp`/`auth` was
//! swallowed as a chat prompt and started a (billable) LLM turn, while a
//! two-token form like `mcp serve` hard-errored.
//!
//! This module restores the surface via clap's native `#[command(subcommand)]`
//! wired into [`crate::argv::Argv`]. A leading token matching a command name
//! routes to that family (and global flags before it still bind to the parent,
//! as in commander); any other leading token is the normal chat `[prompt]`.
//!
//! Per-module contract (so the families are independent + parallelizable):
//! every family module exports `pub struct Cli` (deriving [`clap::Args`], with
//! the byte-faithful options/children) and `pub async fn run(cli: &Cli) ->
//! i32`. [`Commands::run`] fans out to them.

use clap::Subcommand;

pub mod agents;
pub mod auth;
pub mod auto_mode;
pub mod doctor;
pub mod gateway;
pub mod install;
pub mod mcp;
pub mod plugin;
pub mod plugin_init;
pub mod plugin_install;
pub mod plugin_marketplace;
pub mod plugin_prune;
pub mod plugin_settings;
pub mod plugin_tag;
pub mod project;
pub mod setup_token;
pub mod ultrareview;
pub mod update;

/// The top-level subcommands. Names + descriptions byte-match
/// `claude --help`'s Commands section; aliases match commander
/// (`plugin|plugins`, `update|upgrade`).
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Configure and manage MCP servers
    Mcp(mcp::Cli),
    /// Manage authentication
    Auth(auth::Cli),
    /// Inspect auto mode classifier configuration
    #[command(name = "auto-mode")]
    AutoMode(auto_mode::Cli),
    /// Check the health of your LingXi auto-updater. Note: The workspace
    /// trust dialog is skipped and stdio servers from .mcp.json are spawned for
    /// health checks. Only use this command in directories you trust.
    Doctor(doctor::Cli),
    /// Run the enterprise auth/telemetry gateway
    Gateway(gateway::Cli),
    /// Install LingXi native build. Use [target] to specify version
    /// (stable, latest, or specific version)
    Install(install::Cli),
    /// Manage LingXi plugins
    #[command(name = "plugin", visible_alias = "plugins")]
    Plugin(plugin::Cli),
    /// Manage LingXi project state
    Project(project::Cli),
    /// Set up a long-lived authentication token (requires Claude subscription)
    #[command(name = "setup-token")]
    SetupToken(setup_token::Cli),
    /// Manage background agents
    Agents(agents::Cli),
    /// Run a cloud-hosted multi-agent code review of the current branch (or a
    /// PR number / base branch) and print the findings
    Ultrareview(ultrareview::Cli),
    /// Check for updates and install if available
    #[command(name = "update", visible_alias = "upgrade")]
    Update(update::Cli),
}

impl Commands {
    /// Execute the chosen subcommand, returning the process exit code.
    pub async fn run(&self) -> i32 {
        match self {
            Commands::Mcp(c) => mcp::run(c).await,
            Commands::Auth(c) => auth::run(c).await,
            Commands::AutoMode(c) => auto_mode::run(c).await,
            Commands::Doctor(c) => doctor::run(c).await,
            Commands::Gateway(c) => gateway::run(c).await,
            Commands::Install(c) => install::run(c).await,
            Commands::Plugin(c) => plugin::run(c).await,
            Commands::Project(c) => project::run(c).await,
            Commands::SetupToken(c) => setup_token::run(c).await,
            Commands::Agents(c) => agents::run(c).await,
            Commands::Ultrareview(c) => ultrareview::run(c).await,
            Commands::Update(c) => update::run(c).await,
        }
    }
}
