//! `lingxi-cli mcp` — Configure and manage MCP servers.
//!
//! Byte-faithful clap surface (children + options + help) for the whole `mcp`
//! family, matching claude-code 2.1.191's commander surface (only the
//! "lingxi-cli" branding differs from "claude" in the usage line).
//!
//! REAL-IMPLEMENTED, local-config only (no network, no LLM turn):
//!   * `add` / `add-json` — write a server into the chosen scope's config file
//!     (`~/.lingxi.json` for local/user, `<cwd>/.mcp.json` for project) via the
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
    /// Private to you in this project (`~/.lingxi.json` `projects.<key>`).
    Local,
    /// Available across all your projects (`~/.lingxi.json` top-level).
    User,
    /// Shared with everyone via `<cwd>/.mcp.json`.
    Project,
}

impl Scope {
    /// Lowercase scope label used in claude's success strings
    /// (`… to local config`). Project scope is `project` here (the `add`
    /// success line is `… to project config`).
    fn label(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::User => "user",
            Scope::Project => "project",
        }
    }

    /// Suffix for claude's `MCP server X already exists in <suffix>` error
    /// (config.ts): local/user → `<scope> config`, project → `.mcp.json`.
    fn exists_suffix(self) -> &'static str {
        match self {
            Scope::Local => "local config",
            Scope::User => "user config",
            Scope::Project => ".mcp.json",
        }
    }

    /// Suffix for claude's `No MCP server named "X" <suffix>` not-found-in-scope
    /// error: local/user → `in <scope> scope`, project → `in .mcp.json`.
    fn not_found_suffix(self) -> &'static str {
        match self {
            Scope::Local => "in local scope",
            Scope::User => "in user scope",
            Scope::Project => "in .mcp.json",
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
    /// Add an MCP server to LingXi.
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
    /// Start the LingXi MCP server
    Serve(ServeArgs),
}

/// `mcp add <name> <commandOrUrl> [args...]` options.
#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    /// Server name.
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
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
    // Parsed as a raw string (NOT a clap `ValueEnum`) so an invalid/non-writable
    // scope produces claude's exact custom message (`Invalid scope: …` /
    // `Cannot add MCP server to scope: …`) instead of clap's enum error, and so
    // claude's full accepted set (local/user/project/dynamic/enterprise/claudeai/
    // managed/agent) is recognized before the writable-scope check. Validated in
    // `run_add` via [`parse_add_scope`].
    #[arg(short = 's', long = "scope", value_name = "scope", default_value = "local")]
    pub scope: String,
    /// Transport type (stdio, sse, http). Defaults to stdio if not specified.
    // Raw string (not a `ValueEnum`) so `streamable-http` is accepted as an
    // `http` alias and an invalid value yields claude's exact `Invalid transport
    // type: …` message. Validated in `run_add` via [`parse_add_transport`].
    #[arg(short = 't', long = "transport", value_name = "transport", default_value = "stdio")]
    pub transport: String,
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
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
    pub name: String,
    /// JSON server config.
    #[arg(value_name = "json")]
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
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `mcp login <name>` options.
#[derive(Debug, Clone, Args)]
pub struct LoginArgs {
    /// Server name.
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
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
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `mcp remove <name>` options.
#[derive(Debug, Clone, Args)]
pub struct RemoveArgs {
    /// Server name.
    // Explicit `value_name` so the usage placeholder and the
    // `missing required argument 'name'` error read `name` (commander parity),
    // not clap's default SCREAMING_SNAKE_CASE `<NAME>`.
    #[arg(value_name = "name")]
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

/// Path to `~/.lingxi.json` (the global config). `None` when `$HOME` is unset.
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
fn build_add_entry(a: &AddArgs, transport: Transport) -> Result<serde_json::Value, String> {
    let env_map = parse_kv_pairs(&a.env)?;
    let header_map = parse_header_pairs(&a.header)?;
    match transport {
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
            let ty = if matches!(transport, Transport::Sse) { "sse" } else { "http" };
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

/// Validate `mcp add`'s `--scope` string the way claude does (in the action
/// handler, not the parser): the writable scopes (local/user/project) map to a
/// [`Scope`]; the other recognized config scopes (dynamic/enterprise/claudeai/
/// managed/agent) are rejected with `Cannot add MCP server to scope: …`; an
/// unrecognized value with `Invalid scope: …. Must be one of: …`.
fn parse_add_scope(s: &str) -> Result<Scope, String> {
    const RECOGNIZED: [&str; 8] =
        ["local", "user", "project", "dynamic", "enterprise", "claudeai", "managed", "agent"];
    match s {
        "local" => Ok(Scope::Local),
        "user" => Ok(Scope::User),
        "project" => Ok(Scope::Project),
        other if RECOGNIZED.contains(&other) => {
            Err(format!("Cannot add MCP server to scope: {other}"))
        }
        other => Err(format!(
            "Invalid scope: {other}. Must be one of: local, user, project, dynamic, enterprise, claudeai, managed, agent"
        )),
    }
}

/// Validate `mcp add`'s `--transport` string the way claude does: stdio/sse/http
/// map directly, `streamable-http` is an alias for `http`, and anything else is
/// rejected with claude's exact `Invalid transport type: …` message.
fn parse_add_transport(t: &str) -> Result<Transport, String> {
    match t {
        "stdio" => Ok(Transport::Stdio),
        "sse" => Ok(Transport::Sse),
        "http" | "streamable-http" => Ok(Transport::Http),
        other => Err(format!(
            "Invalid transport type: {other}. Must be one of: stdio, sse, http (or streamable-http)"
        )),
    }
}

/// claude's `mcp add` success output for an sse/http server WITH `-H` headers:
/// a pretty-printed JSON object in `-H` INSERTION order (NOT sorted), printed
/// between the `Added …` line and the `File modified:` line. `None` when there
/// are no headers (then no block is printed). Built from the ordered pairs so
/// the key order matches claude even though the stored JSON map may sort.
fn headers_display_block(headers: &[String]) -> Option<String> {
    let pairs = parse_header_pairs(headers).ok()?; // validation already passed in build_add_entry
    if pairs.is_empty() {
        return None;
    }
    let mut s = String::from("Headers: {\n");
    for (i, (k, v)) in pairs.iter().enumerate() {
        let comma = if i + 1 < pairs.len() { "," } else { "" };
        s.push_str(&format!(
            "  {}: {}{comma}\n",
            serde_json::Value::String(k.clone()),
            serde_json::Value::String(v.clone()),
        ));
    }
    s.push('}');
    Some(s)
}

/// Implement `mcp add`.
fn run_add(a: &AddArgs) -> i32 {
    // claude validates the `--scope` and `--transport` strings in the action
    // handler (with its own messages), not via the arg parser. Order matters for
    // combined-invalid input: claude reports the SCOPE error first, THEN
    // transport, THEN the env/header config build (verified against 2.1.191).
    let scope = match parse_add_scope(&a.scope) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("{msg}");
            return RUNTIME_ERROR;
        }
    };
    let transport = match parse_add_transport(&a.transport) {
        Ok(t) => t,
        Err(msg) => {
            eprintln!("{msg}");
            return RUNTIME_ERROR;
        }
    };

    let entry = match build_add_entry(a, transport) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{msg}");
            return RUNTIME_ERROR;
        }
    };

    match write_server(&a.name, &entry, scope) {
        Ok(WriteOutcome::Added(path)) => {
            // Byte-faithful success string per transport (see live-binary probe).
            match transport {
                Transport::Stdio => {
                    // claude's template is `${p} ${i.join(" ")}` — the space after
                    // the command is ALWAYS emitted, so with no args there is a
                    // DOUBLE space before ` to` (`…with command: mycmd  to … config`).
                    let cmd = format!("{} {}", a.command_or_url, a.args.join(" "));
                    println!(
                        "Added stdio MCP server {} with command: {} to {} config",
                        a.name,
                        cmd,
                        scope.label()
                    );
                }
                Transport::Sse | Transport::Http => {
                    let kind = if matches!(transport, Transport::Sse) { "SSE" } else { "HTTP" };
                    // claude displays the URL through `kme()`: clear userinfo/query/
                    // fragment then strip a trailing slash. This redacts any
                    // `user:secret@` credentials and normalizes the byte output;
                    // the STORED value (in the config) stays raw, which is correct.
                    println!(
                        "Added {} MCP server {} with URL: {} to {} config",
                        kind,
                        a.name,
                        redact_url_for_display(&a.command_or_url),
                        scope.label()
                    );
                    // When `-H` headers were supplied, claude echoes the stored
                    // header map as a `Headers: {…}` block before `File modified:`.
                    if let Some(block) = headers_display_block(&a.header) {
                        println!("{block}");
                    }
                }
            }
            print_file_modified(scope, &path);
            SUCCESS
        }
        Ok(WriteOutcome::AlreadyExists) => {
            // claude routes the duplicate as an error: stderr + exit 1.
            eprintln!("MCP server {} already exists in {}", a.name, scope.exists_suffix());
            RUNTIME_ERROR
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// Whether a parsed `mcp add-json` value satisfies claude's server-config
/// schema. claude validates the JSON and emits ONE root-level message for any
/// failure, so callers only need a boolean. Rules (verified against the live
/// 2.1.191 binary): must be an object; `type` defaults to `stdio` and must be
/// one of stdio/sse/http/streamable-http (any other value, or a non-string
/// type, fails); stdio requires a string `command` (with optional array `args`
/// and object `env`); sse/http/streamable-http require a string `url` (with
/// optional object `headers`). Unknown extra fields are allowed.
fn mcp_json_config_is_valid(v: &serde_json::Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    let ty = match obj.get("type") {
        None => "stdio",
        Some(serde_json::Value::String(s)) => s.as_str(),
        Some(_) => return false,
    };
    match ty {
        "stdio" => {
            if !matches!(obj.get("command"), Some(serde_json::Value::String(_))) {
                return false;
            }
            if obj.get("args").is_some_and(|a| !a.is_array()) {
                return false;
            }
            if obj.get("env").is_some_and(|e| !e.is_object()) {
                return false;
            }
            true
        }
        "sse" | "http" | "streamable-http" => {
            if !matches!(obj.get("url"), Some(serde_json::Value::String(_))) {
                return false;
            }
            if obj.get("headers").is_some_and(|h| !h.is_object()) {
                return false;
            }
            true
        }
        _ => false,
    }
}

/// Implement `mcp add-json`.
fn run_add_json(a: &AddJsonArgs) -> i32 {
    // claude parses the JSON and validates it against its server-config schema;
    // a parse error, a non-object, or any schema violation all surface as the
    // same root-level message on stderr (exit 1).
    let entry: serde_json::Value = match serde_json::from_str(&a.json) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Invalid configuration: : Invalid input");
            return RUNTIME_ERROR;
        }
    };
    if !mcp_json_config_is_valid(&entry) {
        eprintln!("Invalid configuration: : Invalid input");
        return RUNTIME_ERROR;
    }

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
            eprintln!("MCP server {} already exists in {}", a.name, a.scope.exists_suffix());
            RUNTIME_ERROR
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

/// User scope: `~/.lingxi.json` top-level `mcpServers.<name>`.
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

/// Local scope: `~/.lingxi.json` `projects.<key>.mcpServers.<name>`.
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
    // With an explicit `--scope`, operate on exactly that scope.
    if let Some(scope) = a.scope {
        return match remove_server(&a.name, scope) {
            Ok(Some(path)) => {
                println!("Removed MCP server {} from {} config", a.name, scope.label());
                print_file_modified(scope, &path);
                SUCCESS
            }
            Ok(None) => {
                eprintln!("No MCP server named \"{}\" {}", a.name, scope.not_found_suffix());
                RUNTIME_ERROR
            }
            Err(msg) => {
                eprintln!("{msg}");
                RUNTIME_ERROR
            }
        };
    }

    // Without `--scope`, count how many scopes hold the server. claude refuses
    // to auto-resolve when it lives in more than one scope (otherwise the user
    // gets a misleading exit 0 with the other copies left behind).
    let present = scopes_containing(&a.name);
    match present.as_slice() {
        // Not found in any scope → not-found message + exit 1.
        [] => {
            eprintln!("{}", not_found_message(&a.name));
            RUNTIME_ERROR
        }
        // Exactly one scope → remove it (name QUOTED in the auto-detect path).
        [scope] => {
            let scope = *scope;
            match remove_server(&a.name, scope) {
                Ok(Some(path)) => {
                    println!("Removed MCP server \"{}\" from {} config", a.name, scope.label());
                    print_file_modified(scope, &path);
                    SUCCESS
                }
                // Race: vanished between count and remove → not-found, still exit 1.
                Ok(None) => {
                    eprintln!("{}", not_found_message(&a.name));
                    RUNTIME_ERROR
                }
                Err(msg) => {
                    eprintln!("{msg}");
                    RUNTIME_ERROR
                }
            }
        }
        // Multiple scopes → refuse and ask the user to disambiguate (no removal).
        scopes => {
            eprintln!("MCP server \"{}\" exists in multiple scopes:", a.name);
            for &scope in scopes {
                eprintln!("  - {} ({})", scope_remove_label(scope), scope_config_path_desc(scope));
            }
            eprintln!();
            eprintln!("To remove from a specific scope, use:");
            for &scope in scopes {
                // claude prints the command hint with the name UNQUOTED (the
                // surrounding message text quotes it, but the copy-paste command
                // does not — verified against the live 2.1.191 binary).
                eprintln!("  claude mcp remove {} -s {}", a.name, scope.label());
            }
            RUNTIME_ERROR
        }
    }
}

/// Human scope label for the multi-scope disambiguation list (matches claude's
/// `getScopeLabel`).
fn scope_remove_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "Local config (private to you in this project)",
        Scope::User => "User config (available in all your projects)",
        Scope::Project => "Project config (shared via .mcp.json)",
    }
}

/// Config-file path description for the multi-scope disambiguation list
/// (matches claude's `describeMcpConfigFilePath`).
fn scope_config_path_desc(scope: Scope) -> String {
    let cwd = std::env::current_dir().map(|c| c.display().to_string()).unwrap_or_default();
    let global = global_config_path().map(|p| p.display().to_string()).unwrap_or_default();
    match scope {
        Scope::User => global,
        Scope::Project => format!("{cwd}/.mcp.json"),
        Scope::Local => format!("{global} [project: {cwd}]"),
    }
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

/// Byte-exact claude-code status for an unapproved `.mcp.json` server (binary
/// `SSc = "\u23F8 Pending approval (run \`claude\` to approve)"`). Shown by
/// `mcp list` / `mcp get` for pending project servers, which are NEVER
/// health-checked or spawned (the binary skips `ySc` for the pending branch).
const PENDING_APPROVAL: &str = "\u{23F8} Pending approval (run `claude` to approve)";

/// Implement `mcp list`. Reads the merged server set across all three scopes
/// and prints each as `<name>: <summary>`.
///
/// Unapproved (repo-self-approved) project `.mcp.json` servers in an untrusted
/// workspace are shown as `\u23F8 Pending approval (run `claude` to approve)`
/// and are NOT connected to — 1:1 with the binary `mcp list` (`$Tf`):
/// `status: n.has(i) ? SSc : (await ySc(i,a)).status`, where the pending branch
/// skips the health check (`ySc` = the spawn/connect probe).
///
/// NOTE: for APPROVED servers claude health-checks each over the network and
/// appends a status (`✔ Connected` / `✘ Failed to connect` /
/// `! Needs authentication`). lingxi's `mcp` crate is client-side and a real
/// probe would require live connections, so this prints the configured transport
/// summary WITHOUT the network probe — the server inventory itself is
/// byte-faithful. (The pending-approval status, unlike a health check, is
/// derived purely from config and so is surfaced exactly.)
fn run_list() -> i32 {
    let servers = load_all_servers();
    if servers.is_empty() {
        println!("No MCP servers configured. Use `claude mcp add` to add a server.");
        return SUCCESS;
    }
    let (_, pending) = project_server_approval();
    for cfg in &servers {
        if is_pending_project_server(cfg, &pending) {
            // Unapproved project server: Pending approval, never spawned.
            println!("{}: {PENDING_APPROVAL}", cfg.name);
        } else {
            println!("{}: {}", cfg.name, transport_summary(&cfg.spec));
        }
    }
    SUCCESS
}

/// Implement `mcp get`. Prints one server's details, or the
/// no-such-server message (exit 0) when absent.
fn run_get(a: &GetArgs) -> i32 {
    let servers = load_all_servers();
    let Some(cfg) = servers.iter().find(|c| c.name == a.name) else {
        // claude: not-found → stderr + exit 1. `mcp get` uses the LOADED view
        // (user + local + approved project), flagging pending `.mcp.json` servers
        // separately — distinct from `mcp remove`'s full config-level listing.
        eprintln!("{}", not_found_message_get(&a.name));
        return RUNTIME_ERROR;
    };

    // A pending (unapproved) project `.mcp.json` server in an untrusted
    // workspace is shown with the Pending-approval status and is NOT connected
    // to — 1:1 with the binary `mcp get` (`qTf`): `i==="pending" ?
    // {status:SSc} : … : await ySc(t,s)`, where the pending branch SKIPS the
    // `ySc` health-check/spawn. The transport details are still printed (the
    // binary keeps appending Type/URL/etc. after the Status line). Only PROJECT
    // servers can be pending (user/local servers never require approval).
    let is_pending = is_pending_project_server(cfg, &project_server_approval().1);

    println!("{}:", cfg.name);
    println!("  Scope: {}", scope_detail(cfg.scope));
    if is_pending {
        println!("  Status: {PENDING_APPROVAL}");
    }
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

/// Sorted list of all configured server names across every scope.
fn configured_server_names_sorted() -> Vec<String> {
    let mut names: Vec<String> = load_all_servers().into_iter().map(|c| c.name).collect();
    names.sort();
    names
}

/// Build claude's not-found message (`g$o(name, names)`): when NO servers are
/// configured, point the user at `claude mcp add`; otherwise list the
/// configured servers, capped at 8 names with an `(and N more …)` suffix.
fn not_found_message(name: &str) -> String {
    let names = configured_server_names_sorted();
    if names.is_empty() {
        return format!("No MCP server named \"{name}\". Run `claude mcp add` to add one.");
    }
    const CAP: usize = 8;
    let shown = names[..names.len().min(CAP)].join(", ");
    let suffix = if names.len() > CAP {
        format!(" (and {} more — run `claude mcp list` to see all)", names.len() - CAP)
    } else {
        String::new()
    };
    format!("No MCP server named \"{name}\". Configured servers: {shown}{suffix}")
}

/// User-scope (`~/.lingxi.json` top-level `mcpServers`) server names.
fn user_server_names() -> Vec<String> {
    global_config_path()
        .and_then(|p| migrations::global_config::read_map(&p).ok())
        .and_then(|m| m.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Local-scope (`~/.lingxi.json` `projects.<key>.mcpServers`) server names.
fn local_server_names() -> Vec<String> {
    global_config_path()
        .zip(project_key())
        .and_then(|(p, k)| migrations::global_config::get_project_config(&p, &k).ok())
        .and_then(|m| m.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether a LOADED server is a pending (unapproved) project `.mcp.json` server
/// — the only servers shown as `\u23F8 Pending approval` and never spawned by
/// `mcp list` / `mcp get`. A server qualifies iff its effective (loaded) scope is
/// `Project` AND its name is in the project pending set. The scope guard is
/// SECURITY-RELEVANT: a same-named USER or LOCAL server (which take precedence in
/// [`mcp::json_config::load_mcp_servers`] and never require approval) must NOT be
/// mislabelled pending nor have its details hidden.
fn is_pending_project_server(cfg: &mcp::connection::McpServerConfig, pending: &[String]) -> bool {
    cfg.scope == ConfigScope::Project && pending.iter().any(|n| n == &cfg.name)
}

/// Partition the project `.mcp.json` servers into `(approved, pending)` using the
/// per-project approval state in `~/.lingxi.json` `projects.<key>`: a server is
/// approved iff it is NOT in `disabledMcpjsonServers` AND
/// (`enableAllProjectMcpServers` is true OR it is in `enabledMcpjsonServers`).
/// `pending` (unapproved) project servers are the ones claude's `mcp get`
/// not-found message omits from the list and flags with an awaiting-approval
/// note.
fn project_server_approval() -> (Vec<String>, Vec<String>) {
    let all: Vec<String> = project_mcp_json_path()
        .filter(|p| p.exists())
        .and_then(|p| read_json_object(&p).ok())
        .and_then(|m| m.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if all.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let str_array = |cfg: &serde_json::Map<String, serde_json::Value>, key: &str| -> Vec<String> {
        cfg.get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };
    let (enable_all, enabled, disabled) = global_config_path()
        .zip(project_key())
        .and_then(|(p, k)| migrations::global_config::get_project_config(&p, &k).ok())
        .map(|cfg| {
            (
                cfg.get("enableAllProjectMcpServers").and_then(|v| v.as_bool()).unwrap_or(false),
                str_array(&cfg, "enabledMcpjsonServers"),
                str_array(&cfg, "disabledMcpjsonServers"),
            )
        })
        .unwrap_or((false, Vec::new(), Vec::new()));
    let mut approved = Vec::new();
    let mut pending = Vec::new();
    for name in all {
        if project_server_is_approved(&name, enable_all, &enabled, &disabled) {
            approved.push(name);
        } else {
            pending.push(name);
        }
    }
    (approved, pending)
}

/// Pure approval predicate for a project `.mcp.json` server, 1:1 with claude's
/// approval rule: approved iff NOT in `disabledMcpjsonServers` AND
/// (`enableAllProjectMcpServers` is true OR in `enabledMcpjsonServers`).
///
/// After `mcp reset-project-choices` clears all three (empty `enabled`, empty
/// `disabled`, `enable_all=false`), EVERY project server reverts to
/// NOT-approved → pending (the trust-reset invariant). `disabled` always wins
/// (an explicit rejection is not overridden by `enable_all`).
fn project_server_is_approved(
    name: &str,
    enable_all: bool,
    enabled: &[String],
    disabled: &[String],
) -> bool {
    !disabled.iter().any(|d| d == name)
        && (enable_all || enabled.iter().any(|e| e == name))
}

/// `mcp get`'s not-found message. Unlike `mcp remove` (which lists every
/// config-level server across all scopes), claude's `mcp get` lists only the
/// LOADED servers — user + local + APPROVED project — and, when there are
/// pending (unapproved) `.mcp.json` servers, OMITS them from the list and
/// appends an awaiting-approval note instead. Verified against the live 2.1.191
/// binary.
fn not_found_message_get(name: &str) -> String {
    let (approved_project, pending_project) = project_server_approval();
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    set.extend(user_server_names());
    set.extend(local_server_names());
    set.extend(approved_project);
    let names: Vec<String> = set.into_iter().collect(); // BTreeSet ⇒ already sorted/unique
    let clause = if pending_project.is_empty() {
        ""
    } else {
        " (.mcp.json servers are awaiting approval — run `claude` in this directory to review them.)"
    };
    if names.is_empty() && clause.is_empty() {
        return format!("No MCP server named \"{name}\". Run `claude mcp add` to add one.");
    }
    const CAP: usize = 8;
    let shown = names[..names.len().min(CAP)].join(", ");
    let suffix = if names.len() > CAP {
        format!(" (and {} more — run `claude mcp list` to see all)", names.len() - CAP)
    } else {
        String::new()
    };
    format!("No MCP server named \"{name}\". Configured servers: {shown}{suffix}{clause}")
}

/// Which writable scopes (local/project/user) currently hold a server named
/// `name`. Used by `mcp remove` without `--scope` to detect the multi-scope
/// case that claude refuses to auto-resolve.
fn scopes_containing(name: &str) -> Vec<Scope> {
    [Scope::Local, Scope::Project, Scope::User]
        .into_iter()
        .filter(|&scope| scope_contains_server(name, scope))
        .collect()
}

/// Presence-only check for a server in one scope (no mutation). Mirrors the
/// per-scope read in [`remove_server`]; a read error is treated as "absent" so
/// the disambiguation logic never blocks on a transient read failure.
fn scope_contains_server(name: &str, scope: Scope) -> bool {
    match scope {
        Scope::User => {
            let Some(path) = global_config_path() else { return false };
            migrations::global_config::read_map(&path)
                .ok()
                .and_then(|m| m.get("mcpServers").and_then(serde_json::Value::as_object).cloned())
                .is_some_and(|m| m.contains_key(name))
        }
        Scope::Local => {
            let Some(path) = global_config_path() else { return false };
            let Some(key) = project_key() else { return false };
            migrations::global_config::get_project_config(&path, &key)
                .ok()
                .and_then(|m| m.get("mcpServers").and_then(serde_json::Value::as_object).cloned())
                .is_some_and(|m| m.contains_key(name))
        }
        Scope::Project => {
            let Some(path) = project_mcp_json_path() else { return false };
            if !path.exists() {
                return false;
            }
            read_json_object(&path)
                .ok()
                .and_then(|m| m.get("mcpServers").and_then(serde_json::Value::as_object).cloned())
                .is_some_and(|m| m.contains_key(name))
        }
    }
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
        ConfigScope::User => "User config (available in all your projects)",
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

/// Implement `mcp reset-project-choices`. Resets the project's recorded
/// `.mcp.json` server choices in the global config.
///
/// claude (mcp.tsx) resets EXACTLY three per-project keys via
/// `saveCurrentProjectConfig`: `enabledMcpjsonServers: []`,
/// `disabledMcpjsonServers: []`, `enableAllProjectMcpServers: false`. (The
/// `approvedMcpjsonServers`/`rejectedMcpjsonServers` keys do NOT exist in
/// claude — an earlier port fabricated them.)
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
        proj.insert("enabledMcpjsonServers".into(), serde_json::json!([]));
        proj.insert("disabledMcpjsonServers".into(), serde_json::json!([]));
        proj.insert("enableAllProjectMcpServers".into(), serde_json::json!(false));
        proj
    });
    match result {
        Ok(_) => {
            println!(
                "Project-scoped (.mcp.json) server approvals and rejections stored for this project have been reset."
            );
            // The binary prints the "prompted next time" line ONLY when the
            // project `.mcp.json` actually has servers to re-approve. Verified vs
            // live 2.1.191: a missing / empty / `{}` / empty-`mcpServers` .mcp.json
            // ⇒ first line only; ≥1 project server ⇒ both lines. (The approval
            // keys cleared above in ~/.lingxi.json do NOT affect this.)
            if project_mcp_json_has_servers() {
                println!("You will be prompted for approval next time you start LingXi.");
            }
            SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            RUNTIME_ERROR
        }
    }
}

/// Whether the project `.mcp.json` exists and declares at least one server.
/// `mcp reset-project-choices` only prints its "prompted next time" follow-up
/// line when there are project servers that will need re-approval.
fn project_mcp_json_has_servers() -> bool {
    project_mcp_json_path()
        .filter(|p| p.exists())
        .and_then(|p| read_json_object(&p).ok())
        .and_then(|m| m.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .is_some_and(|m| !m.is_empty())
}

// ──────────────────────────────────────────────────────────────────────────
// Shared helpers.
// ──────────────────────────────────────────────────────────────────────────

/// Parse `KEY=VALUE` pairs (used for `-e`/`--env`).
fn parse_kv_pairs(pairs: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::with_capacity(pairs.len());
    for p in pairs {
        let Some((k, v)) = p.split_once('=') else {
            return Err(format!(
                "Invalid environment variable format: {p}, environment variables should be added as: -e KEY1=value1 -e KEY2=value2"
            ));
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
            return Err(format!(
                "Invalid header format: {}. Expected format: \"Header-Name: value\"",
                serde_json::Value::String(p.clone())
            ));
        };
        out.push((k.trim().to_string(), v.trim().to_string()));
    }
    Ok(out)
}

/// Mirror claude's `kme()`: parse the URL, clear userinfo/query/fragment, and
/// strip a trailing slash, returning the normalized/redacted string used in the
/// `mcp add` success line. Falls back to the raw input when it does not parse as
/// an `scheme://…` URL (matching `new URL()` throwing → claude would not reach
/// this display, but a graceful raw fallback is the safe choice here). Only the
/// DISPLAY is affected; the stored config value stays raw.
fn redact_url_for_display(raw: &str) -> String {
    // Split off the scheme (`scheme://`). Without `://` there is no authority to
    // redact and no canonical form to compute → return the raw string.
    let Some(scheme_end) = raw.find("://") else {
        return raw.to_string();
    };
    let scheme = &raw[..scheme_end];
    let rest = &raw[scheme_end + 3..];

    // Authority ends at the first `/`, `?`, or `#`.
    let authority_end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let after_authority = &rest[authority_end..];

    // Drop userinfo (`user:pass@host` → `host`). A `@` inside the authority
    // separates credentials from the host:port.
    let host = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };

    // Path = everything after the authority up to `?`/`#`; query (`?…`) and
    // fragment (`#…`) are dropped.
    let path_end = after_authority
        .find(|c| c == '?' || c == '#')
        .unwrap_or(after_authority.len());
    let mut path = &after_authority[..path_end];

    // `new URL("scheme://host").toString()` yields `scheme://host/` (a trailing
    // slash), which kme's `.replace(/\/$/,"")` then strips. Strip a single
    // trailing `/` from the path so `https://h/` and `https://h` both render as
    // `https://h`.
    if path == "/" {
        path = "";
    } else if let Some(stripped) = path.strip_suffix('/') {
        path = stripped;
    }

    format!("{scheme}://{host}{path}")
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
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let serialized = serde_json::to_string_pretty(&serde_json::Value::Object(map.clone()))
        .map_err(|e| format!("could not serialize JSON: {e}"))?;
    // Atomic write (mirrors the global-config writer): write to a sibling temp
    // file in the SAME directory, then rename over `path`. A crash / disk-full /
    // kill mid-write leaves the temp file truncated but the real `.mcp.json`
    // (a shared, often version-controlled project file) intact. A plain
    // truncate-in-place `std::fs::write` could corrupt it into invalid JSON,
    // breaking every later `mcp add/remove` on that scope.
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mcp.json".to_string());
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let write_result = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, serialized.as_bytes())?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = write_result {
        // Best-effort temp cleanup on any failure (the tmp may or may not exist).
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write {}: {e}", path.display()));
    }
    Ok(())
}

/// Print the trailing `File modified: <path>[ [project: <cwd>]]` line, matching
/// claude's per-scope format:
///   * local   → `File modified: ~/.lingxi.json [project: <cwd>]`
///   * user    → `File modified: ~/.lingxi.json`
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

#[cfg(test)]
mod url_redaction_tests {
    use super::redact_url_for_display;

    #[test]
    fn strips_userinfo_query_fragment_and_trailing_slash() {
        // Credentials in the URL are redacted on the success line (the STORED
        // value stays raw — only the display is affected).
        assert_eq!(
            redact_url_for_display("https://user:secret@host/mcp?token=abc#frag"),
            "https://host/mcp"
        );
        // Trailing slash is dropped (claude's `kme().replace(/\/$/,"")`).
        assert_eq!(redact_url_for_display("https://host/"), "https://host");
        assert_eq!(redact_url_for_display("https://host"), "https://host");
        // Query/fragment alone are dropped; a non-trailing-slash path is kept.
        assert_eq!(redact_url_for_display("http://h:8080/a/b?x=1"), "http://h:8080/a/b");
        // userinfo with no path normalizes to just the host.
        assert_eq!(redact_url_for_display("https://u:p@host"), "https://host");
    }

    #[test]
    fn non_url_input_falls_back_to_raw() {
        // No `://` ⟶ return the raw string unchanged (graceful fallback).
        assert_eq!(redact_url_for_display("not-a-url"), "not-a-url");
    }
}

#[cfg(test)]
mod pending_approval_tests {
    use super::{is_pending_project_server, project_server_is_approved, PENDING_APPROVAL};
    use mcp::connection::{ConfigScope, McpServerConfig};
    use std::collections::HashMap;

    fn stdio(name: &str, scope: ConfigScope) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            spec: traits::McpTransportSpec::Stdio {
                command: "srv".into(),
                args: vec![],
                env: HashMap::new(),
            },
            scope,
            disabled: false,
        }
    }

    /// The status string is byte-exact with the binary `SSc` (U+23F8 pause glyph,
    /// backtick-quoted `claude`).
    #[test]
    fn pending_approval_string_is_byte_exact() {
        assert_eq!(
            PENDING_APPROVAL,
            "\u{23F8} Pending approval (run `claude` to approve)"
        );
        // The leading glyph is the PAUSE symbol, not e.g. a play/stop glyph.
        assert!(PENDING_APPROVAL.starts_with('\u{23F8}'));
    }

    /// An unapproved PROJECT `.mcp.json` server (name in the pending set) is
    /// pending — shown as Pending approval and never spawned.
    #[test]
    fn unapproved_project_server_is_pending() {
        let cfg = stdio("repo-srv", ConfigScope::Project);
        let pending = vec!["repo-srv".to_string()];
        assert!(is_pending_project_server(&cfg, &pending));
    }

    /// An APPROVED project server (not in the pending set) is NOT pending — it
    /// is listed/health-checked normally (lingxi shows its transport summary).
    #[test]
    fn approved_project_server_is_not_pending() {
        let cfg = stdio("repo-srv", ConfigScope::Project);
        let pending: Vec<String> = vec![]; // approved ⇒ absent from pending
        assert!(!is_pending_project_server(&cfg, &pending));
    }

    /// A project server is approved when explicitly enabled OR when
    /// `enableAllProjectMcpServers` is on; `disabledMcpjsonServers` always wins.
    #[test]
    fn approval_predicate_matches_claude_rule() {
        // Explicitly enabled.
        assert!(project_server_is_approved("s", false, &["s".into()], &[]));
        // enable-all.
        assert!(project_server_is_approved("s", true, &[], &[]));
        // Disabled overrides enable-all (explicit rejection wins).
        assert!(!project_server_is_approved("s", true, &["s".into()], &["s".into()]));
        // Neither enabled nor enable-all ⇒ not approved.
        assert!(!project_server_is_approved("s", false, &["other".into()], &[]));
    }

    /// TRUST-RESET: after `mcp reset-project-choices` clears every choice
    /// (empty enabled, empty disabled, enable_all=false), a previously-approved
    /// project server reverts to NOT-approved → pending, so `mcp list`/`get`
    /// once again show it as Pending approval and never spawn it.
    #[test]
    fn reset_project_choices_reverts_approved_server_to_pending() {
        // Before reset: enabled ⇒ approved.
        assert!(project_server_is_approved("repo-srv", false, &["repo-srv".into()], &[]));
        // After reset: all choices cleared ⇒ not approved (pending).
        assert!(!project_server_is_approved("repo-srv", false, &[], &[]));
        // And the loaded project server would now be flagged pending.
        let cfg = stdio("repo-srv", ConfigScope::Project);
        assert!(is_pending_project_server(&cfg, &["repo-srv".to_string()]));
    }

    /// SECURITY NEIGHBOR: a same-named USER or LOCAL server (which take
    /// precedence in the loaded view and never require approval) must NOT be
    /// mislabelled pending — even if a project `.mcp.json` server of that name
    /// is pending, the overriding user/local server is fully shown.
    #[test]
    fn same_named_user_or_local_server_is_never_pending() {
        let pending = vec!["srv".to_string()];
        for scope in [ConfigScope::User, ConfigScope::Local] {
            let cfg = stdio("srv", scope);
            assert!(
                !is_pending_project_server(&cfg, &pending),
                "{scope:?} server must not be treated as pending"
            );
        }
    }
}
