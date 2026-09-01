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
pub mod attach;
pub mod auth;
pub mod auto_mode;
/// WIZARD-06 `--propose` adapters, hosted in the composition root (they need a
/// live `ApiService`). Re-exported so this module's path is unchanged.
pub use engine_desktop::auto_mode_propose;
pub mod auto_mode_setup;
pub mod bg_worker;
pub mod daemon;
pub mod doctor;
pub mod gateway;
pub mod import;
pub mod install;
pub mod logs;
pub mod mcp;
pub mod mcp_xaa;
pub mod plugin;
pub mod plugin_eval;
pub mod plugin_eval_mock;
pub mod plugin_init;
pub mod plugin_install;
pub mod plugin_marketplace;
pub mod plugin_policy;
pub mod plugin_prune;
pub mod plugin_settings;
pub mod plugin_tag;
pub mod project;
pub mod remote_control;
pub mod respawn;
pub mod rm;
pub mod sandbox;
pub mod setup_token;
pub mod stop;
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
    /// Inspect or reset auto mode classifier configuration
    #[command(name = "auto-mode")]
    AutoMode(auto_mode::Cli),
    /// Apply a reviewed auto-mode proposal to your settings (non-interactive;
    /// hash-bound via --expect-sha256)
    #[command(name = "auto-mode-setup")]
    AutoModeSetup(auto_mode_setup::Cli),
    // Was: "...your LingXi auto-updater. Note: The workspace trust dialog is
    // skipped and stdio servers from .mcp.json are spawned for health checks."
    // Both halves were wrong about THIS port. `doctor` reports on far more than
    // the updater, and it explicitly does NOT spawn anything — `doctor.rs`:
    // "No connection is attempted — this is purely the parsed config view."
    // The warning also came from the oracle's `-p/--print` option, not its
    // `doctor`. A description that overstates what a command touches is not
    // harmlessly cautious: it steers people away from a safe command.
    /// Check the health of your LingXi installation. Reads settings files in
    /// the current directory without a trust prompt. For a full checkup that
    /// can also fix issues, run /doctor in a session.
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
    /// Start a persistent remote-control server
    #[command(name = "remote-control")]
    RemoteControl(remote_control::Cli),
    /// Set up a long-lived authentication token (requires Claude subscription)
    #[command(name = "setup-token")]
    SetupToken(setup_token::Cli),
    /// Import config from another AI coding agent into LingXi
    //
    // (CLI-14, cc2.1.238) `r.command("import")` — visible, positional
    // `[source]`, `--dry-run`, `--yes[=<digest>]`.
    Import(import::Cli),
    /// Import a session archive into the transcript store (internal).
    //
    // (CLI-14) `r.command("import-conversations <exportPath>",{hidden:!0})`.
    #[command(name = "import-conversations", hide = true)]
    ImportConversations(import::ConversationsCli),
    /// Windows sandbox provisioning (internal).
    //
    // (CLI-10, cc2.1.238) `r.command("sandbox",{hidden:!0})` with the
    // `install` / `status` children — NEW in 2.1.238.
    #[command(name = "sandbox", hide = true)]
    Sandbox(sandbox::Cli),
    /// Manage background agents
    Agents(agents::Cli),
    /// Open a background session here; Ctrl+Z returns to the shell
    Attach(attach::Cli),
    /// Print a background session's recent terminal output
    Logs(logs::Cli),
    /// Stop a background session. Its conversation is kept: `claude attach <id>` opens it again, `claude --resume` works once it is stopped
    #[command(name = "stop", visible_alias = "kill")]
    Stop(stop::Cli),
    /// Restart a background session, or all of them with --all, so it runs the current Claude Code version
    Respawn(respawn::Cli),
    /// Delete a background session and its worktree. Unlike `stop`, works on
    /// already-exited sessions.
    Rm(rm::Cli),
    /// Run a cloud-hosted multi-agent code review of the current branch (or a
    /// PR number / base branch) and print the findings
    Ultrareview(ultrareview::Cli),
    /// Check for updates and install if available
    #[command(name = "update", visible_alias = "upgrade")]
    Update(update::Cli),
    /// Run the background-agent supervisor daemon (internal; spawned by `--bg`).
    #[command(name = "daemon", hide = true)]
    Daemon(daemon::Cli),
    /// Supervise one background PTY child (internal; spawned by the daemon for
    /// each pending `--bg` job).
    #[command(name = "__bg-run", hide = true)]
    BgRun(bg_worker::Cli),
    /// Mount the interactive child inside a worker-owned PTY.
    #[command(name = "__bg-pty-session", hide = true)]
    BgPtySession(bg_worker::PtySessionCli),
}

impl Commands {
    /// The commander/clap top-level command name — the value CC feeds to the
    /// managed startup version gate as `topLevelCommand` (`a1p`'s `t?.name()`),
    /// which exempts `update`/`install`/`doctor`. Matches each variant's clap
    /// command name.
    #[must_use]
    pub fn top_level_name(&self) -> &'static str {
        match self {
            Commands::Mcp(_) => "mcp",
            Commands::Auth(_) => "auth",
            Commands::AutoMode(_) => "auto-mode",
            Commands::AutoModeSetup(_) => "auto-mode-setup",
            Commands::Doctor(_) => "doctor",
            Commands::Gateway(_) => "gateway",
            Commands::Install(_) => "install",
            Commands::Plugin(_) => "plugin",
            Commands::Project(_) => "project",
            Commands::RemoteControl(_) => "remote-control",
            Commands::SetupToken(_) => "setup-token",
            Commands::Import(_) => "import",
            Commands::ImportConversations(_) => "import-conversations",
            Commands::Sandbox(_) => "sandbox",
            Commands::Agents(_) => "agents",
            Commands::Attach(_) => "attach",
            Commands::Logs(_) => "logs",
            Commands::Stop(_) => "stop",
            Commands::Respawn(_) => "respawn",
            Commands::Rm(_) => "rm",
            Commands::Ultrareview(_) => "ultrareview",
            Commands::Update(_) => "update",
            Commands::Daemon(_) => "daemon",
            Commands::BgRun(_) => "__bg-run",
            Commands::BgPtySession(_) => "__bg-pty-session",
        }
    }

    /// Execute the chosen subcommand, returning the process exit code.
    pub async fn run(&self) -> i32 {
        match self {
            Commands::Mcp(c) => mcp::run(c).await,
            Commands::Auth(c) => auth::run(c).await,
            Commands::AutoMode(c) => auto_mode::run(c).await,
            Commands::AutoModeSetup(c) => auto_mode_setup::run(c).await,
            Commands::Doctor(c) => doctor::run(c).await,
            Commands::Gateway(c) => gateway::run(c).await,
            Commands::Install(c) => install::run(c).await,
            Commands::Plugin(c) => plugin::run(c).await,
            Commands::Project(c) => project::run(c).await,
            Commands::RemoteControl(c) => remote_control::run(c).await,
            Commands::SetupToken(c) => setup_token::run(c).await,
            Commands::Import(c) => import::run(c).await,
            Commands::ImportConversations(c) => import::run_conversations(c).await,
            Commands::Sandbox(c) => sandbox::run(c).await,
            Commands::Agents(c) => agents::run(c).await,
            Commands::Attach(c) => attach::run(c).await,
            Commands::Logs(c) => logs::run(c).await,
            Commands::Stop(c) => stop::run(c).await,
            Commands::Respawn(c) => respawn::run(c).await,
            Commands::Rm(c) => rm::run(c).await,
            Commands::Ultrareview(c) => ultrareview::run(c).await,
            Commands::Update(c) => update::run(c).await,
            Commands::Daemon(c) => daemon::run(c).await,
            Commands::BgRun(c) => bg_worker::run(c).await,
            Commands::BgPtySession(c) => bg_worker::run_pty_session(c).await,
        }
    }
}

#[cfg(test)]
mod top_level_name_tests {
    use crate::argv::Argv;

    /// The startup version gate (parity 2.1.207 H-BIN-09) exempts
    /// `update`/`install`/`doctor` by matching `Commands::top_level_name`
    /// against them; lock the names those variants report from the real argv
    /// parse path so the exemption can't silently break.
    #[test]
    fn exempt_command_names_route_through_argv() {
        for (token, expected) in [
            ("update", "update"),
            ("install", "install"),
            ("doctor", "doctor"),
            ("mcp", "mcp"),
            ("auth", "auth"),
            ("remote-control", "remote-control"),
            ("logs", "logs"),
            ("stop", "stop"),
            ("respawn", "respawn"),
        ] {
            let a = Argv::from_iter(["lingxi-cli", token]).unwrap();
            let cmd = a.command.expect("token must parse as a subcommand");
            assert_eq!(cmd.top_level_name(), expected, "for token `{token}`");
        }
        let a = Argv::from_iter(["lingxi-cli", "attach", "bead0001"]).unwrap();
        assert_eq!(a.command.unwrap().top_level_name(), "attach");
        let a = Argv::from_iter(["lingxi-cli", "kill", "bead0001"]).unwrap();
        assert_eq!(a.command.unwrap().top_level_name(), "stop");
        // `upgrade` is an alias of `update` — it must still report `update`.
        let a = Argv::from_iter(["lingxi-cli", "upgrade"]).unwrap();
        assert_eq!(a.command.unwrap().top_level_name(), "update");
    }
}
