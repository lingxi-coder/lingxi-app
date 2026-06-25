//! `lingxi-cli mcp` — Configure and manage MCP servers.
//!
//! Byte-faithful clap surface (children + options + help) for the whole `mcp`
//! family, matching claude-code 2.1.191's commander surface (only the
//! "lingxi-cli" branding differs from "claude" in the usage line).
//!
//! REAL-IMPLEMENTED, local-config only (no network, no LLM turn):
//!   * `add` / `add-json` — write a server into the chosen scope's config file
//!     (`~/.claude.json` for local/user, `<cwd>/.mcp.json` for project) via the
//!     `migrations::global_config` read-modify-write substrate + serde_json.
//!   * `remove` — delete a server from a scope (or from whichever scope holds it
//!     when `--scope` is omitted).
//!   * `list` — read configured servers across all three scopes and print them.
//!   * `get` — print one server's details.
//!   * `reset-project-choices` — clear the project's approved/rejected
//!     `.mcp.json` server choices in the global config.
//!
//! NOTICE (parse faithfully, then `NOT_IMPLEMENTED`): `serve` (stdio MCP SERVER
//! — lingxi's `mcp` crate is client-side), `login` / `logout` (OAuth browser
//! flow), `add-from-claude-desktop` (Claude Desktop import). These print a
//! not-yet-implemented notice and never start a billable chat turn.

use clap::{Args, Subcommand};
use mcp::connection::ConfigScope;
use std::path::PathBuf;

use crate::exit_codes::{NOT_IMPLEMENTED, RUNTIME_ERROR, SUCCESS};

/// `mcp` family payload (a commander subcommand group). Bare `lingxi-cli mcp`
/// (no child) prints help, matching claude.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// The chosen child command (`Option` so a bare parent prints help instead
    /// of erroring).
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// Configuration scope for a server entry. Mirrors commander's
/// `local | user | project` choice (default `local`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    /// Private to you in this project (`~/.claude.json` `projects.<key>`).
    Local,
    /// Available across all your projects (`~/.claude.json` top-level).
    User,
    /// Shared with everyone via `<cwd>/.mcp.json`.
    Project,
}

impl Scope {
    /// Lowercase scope label used in claude's success/error strings
    /// (`… to local config`, `… already exists in user config`).
    fn label(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// Transport selector for `add` (`stdio | sse | http`, default `stdio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Transport {
    /// Local subprocess over stdin/stdout.
    Stdio,
    /// Server-Sent Events HTTP endpoint.
    Sse,
    /// JSON-over-HTTP endpoint.
    Http,
}

/// The `mcp` child commands. Names, args, and one-line descriptions byte-match
/// `claude mcp --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Add an MCP server to Claude Code.
    ///
    /// Examples:
    ///   # Add HTTP server:
    ///   claude mcp add --transport http sentry https://mcp.sentry.dev/mcp
    ///
    ///   # Add HTTP server with headers:
    ///   claude mcp add --transport http corridor https://app.corridor.dev/api/mcp --header "Authorization: Bearer ..."
    ///
    ///   # Add stdio server with environment variables:
    ///   claude mcp add my-server -e API_KEY=xxx -- npx my-mcp-server
    ///
    ///   # Add stdio server with subprocess flags:
    ///   claude mcp add my-server -- my-command --some-flag arg1
    Add(AddArgs),
    /// Import MCP servers from Claude Desktop (Mac and WSL only)
    #[command(name = "add-from-claude-desktop")]
    AddFromClaudeDesktop(AddFromClaudeDesktopArgs),
    /// Add an MCP server (stdio or SSE) with a JSON string
    #[command(name = "add-json")]
    AddJson(AddJsonArgs),
    /// Get details about an MCP server. Unapproved .mcp.json servers are shown
    /// as ⏸ Pending approval and not connected to; approved servers are
    /// health-checked.
    Get(GetArgs),
    /// List configured MCP servers. Unapproved .mcp.json servers are shown as ⏸
    /// Pending approval and not connected to; approved servers are
    /// health-checked.
    List,
    /// Authenticate with an MCP server (HTTP, SSE, or claude.ai connector)
    Login(LoginArgs),
    /// Clear stored OAuth credentials for an MCP server
    Logout(LogoutArgs),
    /// Remove an MCP server
    Remove(RemoveArgs),
    /// Reset all approved and rejected project-scoped (.mcp.json) servers within
    /// this project
    #[command(name = "reset-project-choices")]
    ResetProjectChoices,
    /// Start the Claude Code MCP server
    Serve(ServeArgs),
}

/// `mcp add <name> <commandOrUrl> [args...]` options.
#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    /// Server name.
    pub name: String,
    /// Command (stdio) or URL (sse/http).
    #[arg(value_name = "commandOrUrl")]
    pub command_or_url: String,
    /// Subprocess args (after `--`), or extra positionals.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
    /// Fixed port for OAuth callback (for servers requiring pre-registered
    /// redirect URIs)
    #[arg(long = "callback-port", value_name = "port")]
    pub callback_port: Option<u16>,
    /// OAuth client ID for HTTP/SSE servers
    #[arg(long = "client-id", value_name = "clientId")]
    pub client_id: Option<String>,
    /// Prompt for OAuth client secret (or set MCP_CLIENT_SECRET env var)
    #[arg(long = "client-secret")]
    pub client_secret: bool,
    /// Set environment variables (e.g. -e KEY=value)
    #[arg(short = 'e', long = "env", value_name = "env")]
    pub env: Vec<String>,
    /// Set WebSocket headers (e.g. -H "X-Api-Key: abc123" -H "X-Custom: value")
    #[arg(short = 'H', long = "header", value_name = "header")]
    pub header: Vec<String>,
    /// Configuration scope (local, user, or project)
    #[arg(short = 's', long = "scope", value_name = "scope", default_value = "local")]
    pub scope: Scope,
    /// Transport type (stdio, sse, http). Defaults to stdio if not specified.
    #[arg(short = 't', long = "transport", value_name = "transport", default_value = "stdio")]
    pub transport: Transport,
}

/// `mcp add-from-claude-desktop` options.
#[derive(Debug, Clone, Args)]
pub struct AddFromClaudeDesktopArgs {
    /// Configuration scope (local, user, or project)
    #[arg(short = 's', long = "scope", value_name = "scope", default_value = "local")]
    pub scope: Scope,
}

/// `mcp add-json <name> <json>` options.
#[derive(Debug, Clone, Args)]
pub struct AddJsonArgs {
    /// Server name.
    pub name: String,
    /// JSON server config.
    pub json: String,
    /// Prompt for OAuth client secret (or set MCP_CLIENT_SECRET env var)
    #[arg(long = "client-secret")]
    pub client_secret: bool,
    /// Configuration scope (local, user, or project)
    #[arg(short = 's', long = "scope", value_name = "scope", default_value = "local")]
    pub scope: Scope,
}

/// `mcp get <name>` options.
#[derive(Debug, Clone, Args)]
pub struct GetArgs {
    /// Server name.
    pub name: String,
}

/// `mcp login <name>` options.
#[derive(Debug, Clone, Args)]
pub struct LoginArgs {
    /// Server name.
    pub name: String,
    /// Print the authorization URL instead of opening a browser (for
    /// SSH/headless sessions — paste the redirect URL back when prompted)
    #[arg(long = "no-browser")]
    pub no_browser: bool,
}

/// `mcp logout <name>` options.
#[derive(Debug, Clone, Args)]
pub struct LogoutArgs {
    /// Server name.
    pub name: String,
}

/// `mcp remove <name>` options.
#[derive(Debug, Clone, Args)]
pub struct RemoveArgs {
    /// Server name.
    pub name: String,
    /// Configuration scope (local, user, or project) - if not specified,
    /// removes from whichever scope it exists in
    #[arg(short = 's', long = "scope", value_name = "scope")]
    pub scope: Option<Scope>,
}

/// `mcp serve` options.
#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Enable debug mode
    #[arg(short = 'd', long = "debug")]
    pub debug: bool,
    /// Override verbose mode setting from config
    #[arg(long = "verbose")]
    pub verbose: bool,
}

/// Run the `mcp` family.
pub async fn run(cli: &Cli) -> i32 {
    let Some(command) = &cli.command else {
        // Bare `lingxi-cli mcp`: print help, matching claude's commander group.
        print_family_help();
        return SUCCESS;
    };
    match command {
        Sub::Add(a) => run_add(a),
        Sub::AddJson(a) => run_add_json(a),
        Sub::Remove(a) => run_remove(a),
        Sub::List => run_list(),
        Sub::Get(a) => run_get(a),
        Sub::ResetProjectChoices => run_reset_project_choices(),
        Sub::Serve(_) => {
            eprintln!("lingxi-cli mcp serve: not yet implemented");
            NOT_IMPLEMENTED
        }
        Sub::Login(_) => {
            eprintln!("lingxi-cli mcp login: not yet implemented");
            NOT_IMPLEMENTED
        }
        Sub::Logout(_) => {
            eprintln!("lingxi-cli mcp logout: not yet implemented");
            NOT_IMPLEMENTED
        }
        Sub::AddFromClaudeDesktop(_) => {
            eprintln!("lingxi-cli mcp add-from-claude-desktop: not yet implemented");
            NOT_IMPLEMENTED
        }
    }
}

/// Help text shown for a bare `lingxi-cli mcp` (clap renders the full help via
/// the derived surface; here we trigger it through clap's own machinery by
/// printing the long-form summary). Kept minimal — the canonical surface comes
/// from `--help`, which clap renders from the derive above.
fn print_family_help() {
    // `Cli` derives `clap::Args` (subcommand payload), not `Parser`, so it has
    // no `CommandFactory::command()`. Build the Command via the `Args` trait's
    // `augment_args` (same pattern as the plugin family) and render its help.
    let mut cmd = <Cli as clap::Args>::augment_args(clap::Command::new("mcp"))
        .bin_name("lingxi-cli mcp")
        .name("lingxi-cli mcp");
    let _ = cmd.print_help();
    println!();
}

// ──────────────────────────────────────────────────────────────────────────
// Config-file location helpers (mirror `migrations::global_config`).
// ──────────────────────────────────────────────────────────────────────────

/// Path to `~/.claude.json` (the global config). `None` when `$HOME` is unset.
fn global_config_path() -> Option<PathBuf> {
    migrations::global_config::global_config_path()
}

/// Canonical project key for the current working directory (the same key
/// claude-code stores per-project config under).
fn project_key() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    Some(migrations::global_config::project_path_for_config(&cwd))
}

/// Path to the project `.mcp.json` (in the cwd).
fn project_mcp_json_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".mcp.json"))
}

// ──────────────────────────────────────────────────────────────────────────
// add / add-json
// ──────────────────────────────────────────────────────────────────────────

/// Build the JSON server-entry object for an `add` invocation.
///
/// stdio  → `{ "type": "stdio", "command": <cmd>, "args": [...], "env": {} }`
/// sse/http → `{ "type": <t>, "url": <url>[, "headers": {...}] }`
fn build_add_entry(a: &AddArgs) -> Result<serde_json::Value, String> {
    let env_map = parse_kv_pairs(&a.env)?;
    let header_map = parse_header_pairs(&a.header)?;
    match a.transport {
        Transport::Stdio => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), "stdio".into());
            obj.insert("command".into(), a.command_or_url.clone().into());
            obj.insert(
                "args".into(),
                serde_json::Value::Array(
                    a.args.iter().map(|s| serde_json::Value::String(s.clone())).collect(),
                ),
            );
            obj.insert(
                "env".into(),
                serde_json::Value::Object(
                    env_map.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect(),
                ),
            );
            Ok(serde_json::Value::Object(obj))
        }
        Transport::Sse | Transport::Http => {
            let ty = if matches!(a.transport, Transport::Sse) { "sse" } else { "http" };
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), ty.into());
            obj.insert("url".into(), a.command_or_url.clone().into());
            if !header_map.is_empty() {
                obj.insert(
                    "headers".into(),
                    serde_json::Value::Object(
                        header_map
                            .into_iter()
                            .map(|(k, v)| (k, serde_json::Value::String(v)))
                            .collect(),
                    ),
                );
            }
            Ok(serde_json::Value::Object(obj))
        }
    }
}

/// Implement `mcp add`.
fn run_add(a: &AddArgs) -> i32 {
    let entry = match build_add_entry(a) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{msg}");
            return RUNTIME_ERROR;
        }
    };

    match write_server(&a.name, &entry, a.scope) {
        Ok(WriteOutcome::Added(path)) => {
            // Byte-faithful success string per transport (see live-binary probe).
            match a.transport {
                Transport::Stdio => {
                    let mut cmd = a.command_or_url.clone();
                    if !a.args.is_empty() {
                        cmd.push(' ');
                        cmd.push_str(&a.args.join(" "));
                    }
                    println!(
                        "Added stdio MCP server {} with command: {} to {} config",
                        a.name,
                        cmd,
                        a.scope.label()
                    );
                }
                Transport::Sse | Transport::Http => {
                    let kind = if matches!(a.transport, Transport::Sse) { "SSE" } else { "HTTP" };
                    println!(
                        "Added {} MCP server {} with URL: {} to {} config",
                        kind,
                        a.name,
                        a.command_or_url,
                        a.scope.label()
                    );
                }
            }
            print_file_modified(a.scope, &path);
            SUCCESS
        }
        Ok(WriteOutcome::AlreadyExists) => {
            println!("MCP server {} already exists in {} config", a.name, a.scope.label());
            SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// Implement `mcp add-json`.
fn run_add_json(a: &AddJsonArgs) -> i32 {
    let entry: serde_json::Value = match serde_json::from_str(&a.json) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        Ok(_) => {
            eprintln!("Invalid JSON: expected an object describing the server config");
            return RUNTIME_ERROR;
        }
        Err(e) => {
            eprintln!("Invalid JSON: {e}");
            return RUNTIME_ERROR;
        }
    };

    // Transport label for the success string mirrors claude: read the `type`
    // field (default "stdio") and lowercase it.
    let ty = entry
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio")
        .to_string();

    match write_server(&a.name, &entry, a.scope) {
        Ok(WriteOutcome::Added(_path)) => {
            println!("Added {} MCP server {} to {} config", ty, a.name, a.scope.label());
            SUCCESS
        }
        Ok(WriteOutcome::AlreadyExists) => {
            println!("MCP server {} already exists in {} config", a.name, a.scope.label());
            SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// Outcome of a config write.
enum WriteOutcome {
    /// Server written; carries the file path that was modified.
    Added(PathBuf),
    /// A server with that name already existed in the scope (no write).
    AlreadyExists,
}

/// Write a server entry into the chosen scope's config file, returning whether
/// it was newly added or already present.
fn write_server(name: &str, entry: &serde_json::Value, scope: Scope) -> Result<WriteOutcome, String> {
    match scope {
        Scope::User => write_user_server(name, entry),
        Scope::Local => write_local_server(name, entry),
        Scope::Project => write_project_server(name, entry),
    }
}

/// User scope: `~/.claude.json` top-level `mcpServers.<name>`.
fn write_user_server(name: &str, entry: &serde_json::Value) -> Result<WriteOutcome, String> {
    let path = global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
    let map = migrations::global_config::read_map(&path).map_err(|e| e.to_string())?;
    if map
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|m| m.contains_key(name))
    {
        return Ok(WriteOutcome::AlreadyExists);
    }
    let name = name.to_string();
    let entry = entry.clone();
    migrations::global_config::save_map(&path, move |mut m| {
        let servers = m
            .entry("mcpServers")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(servers) = servers {
            servers.insert(name, entry);
        }
        m
    })
    .map_err(|e| e.to_string())?;
    Ok(WriteOutcome::Added(path))
}

/// Local scope: `~/.claude.json` `projects.<key>.mcpServers.<name>`.
fn write_local_server(name: &str, entry: &serde_json::Value) -> Result<WriteOutcome, String> {
    let path = global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
    let key = project_key().ok_or_else(|| "Could not resolve project directory".to_string())?;
    let existing = migrations::global_config::get_project_config(&path, &key).map_err(|e| e.to_string())?;
    if existing
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|m| m.contains_key(name))
    {
        return Ok(WriteOutcome::AlreadyExists);
    }
    let name = name.to_string();
    let entry = entry.clone();
    migrations::global_config::save_project_config(&path, &key, move |mut proj| {
        let servers = proj
            .entry("mcpServers")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(servers) = servers {
            servers.insert(name, entry);
        }
        proj
    })
    .map_err(|e| e.to_string())?;
    Ok(WriteOutcome::Added(path))
}

/// Project scope: `<cwd>/.mcp.json` `mcpServers.<name>`.
fn write_project_server(name: &str, entry: &serde_json::Value) -> Result<WriteOutcome, String> {
    let path = project_mcp_json_path().ok_or_else(|| "Could not resolve project directory".to_string())?;
    let mut root = read_json_object(&path)?;
    let already = root
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|m| m.contains_key(name));
    if already {
        return Ok(WriteOutcome::AlreadyExists);
    }
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(servers) = servers {
        servers.insert(name.to_string(), entry.clone());
    }
    write_json_object(&path, &root)?;
    Ok(WriteOutcome::Added(path))
}

// ──────────────────────────────────────────────────────────────────────────
// remove
// ──────────────────────────────────────────────────────────────────────────

/// Implement `mcp remove`. With `--scope`, removes from that scope; without it,
/// removes from whichever scope holds the server (local → project → user).
fn run_remove(a: &RemoveArgs) -> i32 {
    let scopes: Vec<Scope> = match a.scope {
        Some(s) => vec![s],
        None => vec![Scope::Local, Scope::Project, Scope::User],
    };

    for scope in scopes {
        match remove_server(&a.name, scope) {
            Ok(Some(path)) => {
                println!("Removed MCP server {} from {} config", a.name, scope.label());
                print_file_modified(scope, &path);
                return SUCCESS;
            }
            Ok(None) => continue,
            Err(msg) => {
                eprintln!("{msg}");
                return RUNTIME_ERROR;
            }
        }
    }

    // Not found in any candidate scope: claude prints the configured-server list
    // and exits 0.
    println!(
        "No MCP server named \"{}\". Configured servers: {}",
        a.name,
        configured_server_names_csv()
    );
    SUCCESS
}

/// Remove a server from one scope; `Ok(Some(path))` when it existed and was
/// removed, `Ok(None)` when absent in that scope.
fn remove_server(name: &str, scope: Scope) -> Result<Option<PathBuf>, String> {
    match scope {
        Scope::User => {
            let path =
                global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
            let map = migrations::global_config::read_map(&path).map_err(|e| e.to_string())?;
            let present = map
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|m| m.contains_key(name));
            if !present {
                return Ok(None);
            }
            let name = name.to_string();
            migrations::global_config::save_map(&path, move |mut m| {
                if let Some(serde_json::Value::Object(servers)) = m.get_mut("mcpServers") {
                    servers.remove(&name);
                }
                m
            })
            .map_err(|e| e.to_string())?;
            Ok(Some(path))
        }
        Scope::Local => {
            let path =
                global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
            let key = project_key().ok_or_else(|| "Could not resolve project directory".to_string())?;
            let existing =
                migrations::global_config::get_project_config(&path, &key).map_err(|e| e.to_string())?;
            let present = existing
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|m| m.contains_key(name));
            if !present {
                return Ok(None);
            }
            let name = name.to_string();
            migrations::global_config::save_project_config(&path, &key, move |mut proj| {
                if let Some(serde_json::Value::Object(servers)) = proj.get_mut("mcpServers") {
                    servers.remove(&name);
                }
                proj
            })
            .map_err(|e| e.to_string())?;
            Ok(Some(path))
        }
        Scope::Project => {
            let path =
                project_mcp_json_path().ok_or_else(|| "Could not resolve project directory".to_string())?;
            if !path.exists() {
                return Ok(None);
            }
            let mut root = read_json_object(&path)?;
            let present = root
                .get("mcpServers")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|m| m.contains_key(name));
            if !present {
                return Ok(None);
            }
            if let Some(serde_json::Value::Object(servers)) = root.get_mut("mcpServers") {
                servers.remove(name);
            }
            write_json_object(&path, &root)?;
            Ok(Some(path))
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// list / get
// ──────────────────────────────────────────────────────────────────────────

/// Implement `mcp list`. Reads the merged server set across all three scopes
/// and prints each as `<name>: <summary>`.
///
/// NOTE: claude health-checks each server over the network and appends a status
/// (`✔ Connected` / `✘ Failed to connect` / `! Needs authentication`). lingxi's
/// `mcp` crate is client-side and a real probe would require live connections,
/// so this prints the configured transport summary WITHOUT the network probe —
/// the server inventory itself is byte-faithful.
fn run_list() -> i32 {
    let servers = load_all_servers();
    if servers.is_empty() {
        println!("No MCP servers configured. Use `claude mcp add` to add a server.");
        return SUCCESS;
    }
    for cfg in &servers {
        println!("{}: {}", cfg.name, transport_summary(&cfg.spec));
    }
    SUCCESS
}

/// Implement `mcp get`. Prints one server's details, or the
/// no-such-server message (exit 0) when absent.
fn run_get(a: &GetArgs) -> i32 {
    let servers = load_all_servers();
    let Some(cfg) = servers.iter().find(|c| c.name == a.name) else {
        println!(
            "No MCP server named \"{}\". Configured servers: {}",
            a.name,
            configured_server_names_csv()
        );
        return SUCCESS;
    };

    println!("{}:", cfg.name);
    println!("  Scope: {}", scope_detail(cfg.scope));
    match &cfg.spec {
        traits::McpTransportSpec::Stdio { command, args, env } => {
            println!("  Type: stdio");
            println!("  Command: {command}");
            println!("  Args: {}", args.join(" "));
            println!("  Environment:");
            let mut keys: Vec<&String> = env.keys().collect();
            keys.sort();
            for k in keys {
                println!("    {k}={}", env[k]);
            }
        }
        traits::McpTransportSpec::Sse { url, .. } => {
            println!("  Type: sse");
            println!("  URL: {url}");
        }
        traits::McpTransportSpec::Http { url, .. } => {
            println!("  Type: http");
            println!("  URL: {url}");
        }
        other => {
            println!("  Type: {}", other.kind());
        }
    }
    println!();
    let scope_flag = scope_flag_label(cfg.scope);
    println!("To remove this server, run: claude mcp remove {} -s {scope_flag}", cfg.name);
    SUCCESS
}

/// Load every configured server across local + project + user scopes.
fn load_all_servers() -> Vec<mcp::connection::McpServerConfig> {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => PathBuf::from("."),
    };
    let project_mcp = cwd.join(".mcp.json");
    let Some(global) = global_config_path() else {
        // No home: only a project .mcp.json could exist.
        return mcp::json_config::load_mcp_servers(&project_mcp, &PathBuf::from("/nonexistent"), &cwd);
    };
    mcp::json_config::load_mcp_servers(&project_mcp, &global, &cwd)
}

/// Comma-separated, name-sorted list of configured servers (used by the
/// "No MCP server named …" message). Matches claude's sorted display.
fn configured_server_names_csv() -> String {
    let mut names: Vec<String> = load_all_servers().into_iter().map(|c| c.name).collect();
    names.sort();
    names.join(", ")
}

/// One-line transport summary for `list` (e.g. `echo hello`,
/// `https://x/mcp (HTTP)`).
fn transport_summary(spec: &traits::McpTransportSpec) -> String {
    match spec {
        traits::McpTransportSpec::Stdio { command, args, .. } => {
            if args.is_empty() {
                command.clone()
            } else {
                format!("{command} {}", args.join(" "))
            }
        }
        traits::McpTransportSpec::Sse { url, .. } => format!("{url} (SSE)"),
        traits::McpTransportSpec::Http { url, .. } => format!("{url} (HTTP)"),
        traits::McpTransportSpec::WebSocket { url, .. } => format!("{url} (WebSocket)"),
        other => other.kind().to_string(),
    }
}

/// Human label for `get`'s `Scope:` line (matches claude's wording).
fn scope_detail(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "Local config (private to you in this project)",
        ConfigScope::User => "User config (available across all your projects)",
        ConfigScope::Project => "Project config (shared via .mcp.json)",
        ConfigScope::Dynamic => "Dynamic",
        ConfigScope::Enterprise => "Enterprise managed config",
        ConfigScope::ClaudeAi => "claude.ai connector",
        ConfigScope::Managed => "Managed config",
    }
}

/// The `-s <scope>` flag label used in `get`'s "To remove …" hint.
fn scope_flag_label(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Local => "local",
        ConfigScope::User => "user",
        ConfigScope::Project => "project",
        // Non-writable scopes have no remove flag; default to local for the hint.
        _ => "local",
    }
}

// ──────────────────────────────────────────────────────────────────────────
// reset-project-choices
// ──────────────────────────────────────────────────────────────────────────

/// Implement `mcp reset-project-choices`. Clears the project's recorded
/// approved/rejected `.mcp.json` server choices in the global config.
///
/// claude stores these under the per-project config keys
/// `enabledMcpjsonServers` / `disabledMcpjsonServers` (and the legacy
/// `approvedMcpjsonServers` / `rejectedMcpjsonServers`). We clear all four.
fn run_reset_project_choices() -> i32 {
    let Some(path) = global_config_path() else {
        eprintln!("Could not resolve home directory");
        return RUNTIME_ERROR;
    };
    let Some(key) = project_key() else {
        eprintln!("Could not resolve project directory");
        return RUNTIME_ERROR;
    };
    let result = migrations::global_config::save_project_config(&path, &key, |mut proj| {
        for k in [
            "enabledMcpjsonServers",
            "disabledMcpjsonServers",
            "approvedMcpjsonServers",
            "rejectedMcpjsonServers",
        ] {
            proj.remove(k);
        }
        proj
    });
    match result {
        Ok(_) => {
            println!("Reset project-scoped (.mcp.json) server approval choices for this project");
            SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            RUNTIME_ERROR
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Shared helpers.
// ──────────────────────────────────────────────────────────────────────────

/// Parse `KEY=VALUE` pairs (used for `-e`/`--env`).
fn parse_kv_pairs(pairs: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::with_capacity(pairs.len());
    for p in pairs {
        let Some((k, v)) = p.split_once('=') else {
            return Err(format!("Invalid environment variable (expected KEY=value): {p}"));
        };
        out.push((k.to_string(), v.to_string()));
    }
    Ok(out)
}

/// Parse `Name: Value` header pairs (used for `-H`/`--header`).
fn parse_header_pairs(pairs: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::with_capacity(pairs.len());
    for p in pairs {
        let Some((k, v)) = p.split_once(':') else {
            return Err(format!("Invalid header (expected Name: Value): {p}"));
        };
        out.push((k.trim().to_string(), v.trim().to_string()));
    }
    Ok(out)
}

/// Read a JSON object from `path`; missing file ⇒ empty object. Errors on
/// malformed JSON or a non-object root.
fn read_json_object(path: &std::path::Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?;
            match value {
                serde_json::Value::Object(m) => Ok(m),
                _ => Err(format!("{} is not a JSON object", path.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

/// Pretty-write a JSON object to `path` (2-space indent, no trailing newline —
/// matching the global-config writer's format), creating parent dirs.
fn write_json_object(
    path: &std::path::Path,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
        .map_err(|e| format!("could not serialize JSON: {e}"))?;
    std::fs::write(path, serialized).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(())
}

/// Print the trailing `File modified: <path>[ [project: <cwd>]]` line, matching
/// claude's per-scope format:
///   * local   → `File modified: ~/.claude.json [project: <cwd>]`
///   * user    → `File modified: ~/.claude.json`
///   * project → `File modified: <cwd>/.mcp.json`
fn print_file_modified(scope: Scope, path: &std::path::Path) {
    match scope {
        Scope::Local => {
            let cwd = std::env::current_dir()
                .map(|c| c.display().to_string())
                .unwrap_or_else(|_| ".".to_string());
            println!("File modified: {} [project: {}]", path.display(), cwd);
        }
        Scope::User | Scope::Project => {
            println!("File modified: {}", path.display());
        }
    }
}
