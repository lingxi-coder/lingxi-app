//! `lingxi-cli doctor` — Check the health of your LingXi auto-updater.
//!
//! claude-code's `doctor` checks the NATIVE auto-updater install (the
//! self-updating binary distribution) and, as a side effect, spawns the stdio
//! MCP servers declared in `.mcp.json` to verify they start. The workspace
//! trust dialog is skipped for that spawn — hence the "Only use this command
//! in directories you trust." warning in the help text.
//!
//! lingxi-cli is built from source (cargo), so there is NO native auto-updater
//! to diagnose: that portion is NOT-APPLICABLE and we say so plainly. Instead
//! we emit a useful, fully READ-ONLY + LOCAL health summary:
//!   * lingxi-cli version,
//!   * config-dir (`~/.claude`) + global-config (`~/.lingxi.json`) presence,
//!   * the MCP servers configured across the user/project/local scopes
//!     (parsed from `.mcp.json` / `~/.lingxi.json` — we do NOT spawn them, do
//!     NOT touch the network, and start NO LLM turn).
//!
//! The command exits `SUCCESS`. It has no children: a bare `doctor` runs the
//! summary, matching claude's leaf command shape (claude's `doctor` likewise
//! takes no subcommands, only `-h/--help`).

use clap::Args;

use crate::exit_codes::SUCCESS;

/// `doctor` args. Like claude's `doctor` this is a leaf command with no
/// children and no options beyond the implicit `-h/--help` clap injects, so
/// `Cli` carries no fields. The byte-faithful one-line description lives on the
/// [`crate::commands::Commands::Doctor`] variant (clap sources a subcommand's
/// `about` from the enum-variant doc comment).
#[derive(Debug, Clone, Args)]
pub struct Cli {}

/// Run the `doctor` family: print a read-only, local-only health summary and
/// exit `SUCCESS`. Never spawns MCP servers, never hits the network, never
/// starts an LLM turn.
pub async fn run(_cli: &Cli) -> i32 {
    println!("lingxi-cli doctor");
    println!();

    // ── Version ───────────────────────────────────────────────────────────
    println!("Version: lingxi-cli {}", env!("CARGO_PKG_VERSION"));

    // ── Auto-updater (NOT APPLICABLE) ─────────────────────────────────────
    // claude-code's doctor diagnoses its native self-updating binary. lingxi
    // is a source build with no such updater, so there is nothing to check.
    println!(
        "Auto-updater: not applicable (lingxi-cli is built from source; no native auto-updater)"
    );

    // ── Config locations ──────────────────────────────────────────────────
    let config_home = crate::run::lingxi_home_dir();
    let config_home_exists = config_home.is_dir();
    println!(
        "Config directory: {} ({})",
        config_home.display(),
        if config_home_exists {
            "found"
        } else {
            "missing"
        }
    );

    let global_config = migrations::global_config::global_config_path();
    match &global_config {
        Some(path) => println!(
            "Global config: {} ({})",
            path.display(),
            if path.is_file() { "found" } else { "missing" }
        ),
        None => println!("Global config: unavailable (no home directory)"),
    }

    // ── MCP servers (READ-ONLY — parsed, never spawned) ───────────────────
    // Mirror the workspace's standard three-scope resolution
    // (user `~/.lingxi.json` `mcpServers`, project `<cwd>/.mcp.json`, and local
    // `~/.lingxi.json` `projects.<cwd>.mcpServers`) WITHOUT connecting to any
    // server. claude's doctor *spawns* stdio servers; we deliberately do not —
    // we only report what is configured, which is the safe, local health view.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_mcp_path = cwd.join(".mcp.json");
    // `load_mcp_servers` tolerates missing files; when there is no global
    // config path point its global slot at the (possibly absent) project file
    // so only project-scope `.mcp.json` servers are reported.
    let global_for_load = global_config
        .clone()
        .unwrap_or_else(|| project_mcp_path.clone());
    let servers = mcp::json_config::load_mcp_servers(&project_mcp_path, &global_for_load, &cwd);
    report_mcp_servers(&servers);
    report_mcp_config_warnings(&cwd, global_config.as_deref());

    SUCCESS
}

/// Report MCP config-load diagnostics (claude-code `F7t`): per-entry problems
/// that cause a server to be skipped (unknown type, url-without-type, invalid
/// entry, reserved name, missing env vars) plus the `servers`-vs-`mcpServers`
/// shape error. Nothing is printed when every config is clean.
fn report_mcp_config_warnings(cwd: &std::path::Path, global_config: Option<&std::path::Path>) {
    let warnings = mcp::config_diagnostics::collect_all_mcp_config_warnings(cwd, global_config);
    if warnings.is_empty() {
        return;
    }
    println!("MCP config warnings: {}", warnings.len());
    for w in &warnings {
        match &w.file {
            Some(f) => println!("  - [{}] {}", f, w.message),
            None => println!("  - {}", w.message),
        }
        if let Some(s) = &w.suggestion {
            println!("      {s}");
        }
    }
}

/// Print the configured MCP servers (name, transport kind, scope, enabled
/// state). No connection is attempted — this is purely the parsed config view.
fn report_mcp_servers(servers: &[mcp::McpServerConfig]) {
    if servers.is_empty() {
        println!("MCP servers: none configured");
        return;
    }
    println!("MCP servers: {} configured (not spawned)", servers.len());
    for s in servers {
        let state = if s.disabled { "disabled" } else { "enabled" };
        println!(
            "  - {} [{}] scope={:?} ({})",
            s.name,
            s.spec.kind(),
            s.scope,
            state
        );
    }
}
