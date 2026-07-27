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
//! `serve` enters a real long-running CLI path; `login` / `logout` drive the
//! configured MCP OAuth flow via `mcp::oauth`; `add-from-claude-desktop` imports
//! Claude Desktop server entries into the selected scope.

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};
use clap::{Args, Subcommand};
use mcp::connection::ConfigScope;
use mcp::oauth;
use platform_posix::{self, PosixClock, PosixHttp};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

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
    /// Manage the XAA (SEP-990) IdP connection
    ///
    /// Hidden because the oracle registers this group only under
    /// `CLAUDE_CODE_ENABLE_XAA` (see `mcp_xaa::xaa_enabled`); listing it
    /// unconditionally would advertise a surface a default install refuses.
    #[command(hide = true)]
    Xaa {
        #[command(subcommand)]
        sub: crate::commands::mcp_xaa::Sub,
    },
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
    #[arg(
        short = 's',
        long = "scope",
        value_name = "scope",
        default_value = "local"
    )]
    pub scope: String,
    /// Transport type (stdio, sse, http). Defaults to stdio if not specified.
    // Raw string (not a `ValueEnum`) so `streamable-http` is accepted as an
    // `http` alias and an invalid value yields claude's exact `Invalid transport
    // type: …` message. Validated in `run_add` via [`parse_add_transport`].
    #[arg(
        short = 't',
        long = "transport",
        value_name = "transport",
        default_value = "stdio"
    )]
    pub transport: String,
}

/// `mcp add-from-claude-desktop` options.
#[derive(Debug, Clone, Args)]
pub struct AddFromClaudeDesktopArgs {
    /// Configuration scope (local, user, or project)
    #[arg(
        short = 's',
        long = "scope",
        value_name = "scope",
        default_value = "local"
    )]
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
    #[arg(
        short = 's',
        long = "scope",
        value_name = "scope",
        default_value = "local"
    )]
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
        Sub::List => run_list().await,
        Sub::Get(a) => run_get(a),
        Sub::ResetProjectChoices => run_reset_project_choices(),
        Sub::Serve(a) => run_serve(a).await,
        Sub::Login(a) => run_login(a).await,
        Sub::Logout(a) => run_logout(a).await,
        Sub::AddFromClaudeDesktop(a) => run_add_from_claude_desktop(a),
        Sub::Xaa { sub } => crate::commands::mcp_xaa::run(sub).await,
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
// serve / login / logout / add-from-claude-desktop
// ──────────────────────────────────────────────────────────────────────────

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

type ProtocolWriter = Arc<Mutex<BufWriter<tokio::io::Stdout>>>;

/// MCP stdio output adapter. Normal turn text/tool events stay silent; only
/// protocol-authorized progress notifications are written to stdout.
struct McpProtocolOutput {
    writer: ProtocolWriter,
    progress_tokens: Mutex<HashMap<String, serde_json::Value>>,
}

impl McpProtocolOutput {
    fn new(writer: ProtocolWriter) -> Self {
        Self {
            writer,
            progress_tokens: Mutex::new(HashMap::new()),
        }
    }

    async fn register_progress(
        &self,
        tool_use_id: &protocol::ToolUseId,
        token: Option<serde_json::Value>,
    ) {
        if let Some(token) = token {
            self.progress_tokens
                .lock()
                .await
                .insert(tool_use_id.as_str().to_string(), token);
        }
    }

    async fn unregister_progress(&self, tool_use_id: &protocol::ToolUseId) {
        self.progress_tokens
            .lock()
            .await
            .remove(tool_use_id.as_str());
    }
}

#[async_trait::async_trait]
impl traits::OutputStream for McpProtocolOutput {
    async fn emit_text(&self, _text: &str) {}

    async fn emit_tool_call(
        &self,
        _id: &protocol::ToolUseId,
        _tool: &str,
        _input: &serde_json::Value,
    ) {
    }

    async fn emit_tool_result(
        &self,
        _id: &protocol::ToolUseId,
        _tool: &str,
        _model_text: &str,
        _result: &serde_json::Value,
    ) {
    }

    async fn emit_tool_heartbeat(&self, id: &protocol::ToolUseId, tool: &str, elapsed_ms: u64) {
        let token = self.progress_tokens.lock().await.get(id.as_str()).cloned();
        if let Some(token) = token {
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {
                    "progressToken": token,
                    "progress": elapsed_ms,
                    "message": format!("{tool} is still running"),
                }
            });
            let _ = write_protocol_frame(&self.writer, &frame).await;
        }
    }

    async fn emit_end_turn(&self, _stop_reason: &str, _cost: &traits::CostSnapshot) {}
}

struct McpServePermissionSink;

#[async_trait::async_trait]
impl client_adapter::PermissionRequestSink for McpServePermissionSink {
    async fn emit_request(&self, _request: client_protocol::permission::PermissionRequest) {
        eprintln!("MCP serve denied an interactive permission prompt in headless mode.");
    }
}

/// Implement `mcp serve` as a newline-delimited JSON-RPC 2.0 MCP server over
/// stdin/stdout. Runtime diagnostics are intentionally restricted to stderr.
async fn run_serve(a: &ServeArgs) -> i32 {
    let writer = Arc::new(Mutex::new(BufWriter::new(tokio::io::stdout())));
    let output = Arc::new(McpProtocolOutput::new(writer.clone()));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut cfg = engine_desktop::DesktopConfig::default();
    cfg.api_base = crate::init::resolve_api_base();
    cfg.api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    cfg.cwd = cwd;
    cfg.lingxi_home = crate::run::lingxi_home_dir();
    // This command exports LingXi's own tool surface. Configured outbound MCP
    // servers are not recursively re-exported.
    cfg.mcp_paths.clear();
    cfg.cli_mcp_servers.clear();
    cfg.use_noop_permission_gate = true;
    cfg.deny_unresolved_ask = true;
    cfg.session_persistence = false;

    let permission_sink: Arc<dyn client_adapter::PermissionRequestSink> =
        Arc::new(McpServePermissionSink);
    let runtime = match engine_desktop::build(
        cfg,
        output.clone() as Arc<dyn traits::OutputStream>,
        permission_sink,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Failed to initialize LingXi MCP server: {error}");
            return RUNTIME_ERROR;
        }
    };
    if a.debug || a.verbose {
        eprintln!("LingXi MCP stdio server initialized.");
    }

    match serve_stdio(runtime.orchestrator, output, writer).await {
        Ok(()) => SUCCESS,
        Err(error) => {
            eprintln!("LingXi MCP stdio server failed: {error}");
            RUNTIME_ERROR
        }
    }
}

async fn serve_stdio(
    orchestrator: Arc<orchestrator::ConversationOrchestrator>,
    output: Arc<McpProtocolOutput>,
    writer: ProtocolWriter,
) -> Result<(), String> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let active = Arc::new(Mutex::new(HashMap::<String, CancellationToken>::new()));
    let mut calls = JoinSet::new();
    let mut initialize_seen = false;
    let mut client_initialized = false;

    loop {
        tokio::select! {
            joined = calls.join_next(), if !calls.is_empty() => {
                if let Some(Err(error)) = joined {
                    eprintln!("MCP tool task failed: {error}");
                }
            }
            line = lines.next_line() => {
                let line = line.map_err(|e| format!("read stdin: {e}"))?;
                let Some(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let message: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(error) => {
                        let response = jsonrpc_error(serde_json::Value::Null, -32700, "Parse error", Some(serde_json::json!({ "detail": error.to_string() })));
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                        continue;
                    }
                };
                let Some(object) = message.as_object() else {
                    let response = jsonrpc_error(serde_json::Value::Null, -32600, "Invalid Request", None);
                    write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    continue;
                };
                let request_id = object.get("id").cloned();
                let method = object.get("method").and_then(serde_json::Value::as_str);
                if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") || method.is_none() {
                    if request_id.is_some() {
                        let response = jsonrpc_error(request_id.unwrap_or(serde_json::Value::Null), -32600, "Invalid Request", None);
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                    continue;
                }
                let method = method.unwrap_or_default();
                let params = object.get("params").cloned().unwrap_or_else(|| serde_json::json!({}));

                match method {
                    "notifications/initialized" => {
                        if initialize_seen {
                            client_initialized = true;
                        }
                    }
                    "notifications/cancelled" => {
                        if let Some(id) = params.get("requestId") {
                            if let Some(cancel) = active.lock().await.get(&request_id_key(id)).cloned() {
                                cancel.cancel();
                            }
                        }
                    }
                    _ if request_id.is_none() => {}
                    "initialize" => {
                        let id = request_id.unwrap_or(serde_json::Value::Null);
                        let requested = params.get("protocolVersion").and_then(serde_json::Value::as_str);
                        let response = if initialize_seen {
                            jsonrpc_error(id, -32600, "Server is already initialized", None)
                        } else if requested != Some(MCP_PROTOCOL_VERSION) {
                            jsonrpc_error(
                                id,
                                -32602,
                                "Unsupported protocol version",
                                Some(serde_json::json!({
                                    "supported": [MCP_PROTOCOL_VERSION],
                                    "requested": requested,
                                })),
                            )
                        } else {
                            initialize_seen = true;
                            initialize_response(id)
                        };
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                    "ping" => {
                        let response = jsonrpc_result(request_id.unwrap_or(serde_json::Value::Null), serde_json::json!({}));
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                    _ if !client_initialized => {
                        let response = jsonrpc_error(
                            request_id.unwrap_or(serde_json::Value::Null),
                            -32002,
                            "Server is not initialized",
                            None,
                        );
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                    "tools/list" => {
                        let tools = orchestrator.mcp_tool_definitions().await;
                        let response = jsonrpc_result(
                            request_id.unwrap_or(serde_json::Value::Null),
                            serde_json::json!({ "tools": tools }),
                        );
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                    "tools/call" => {
                        let id = request_id.unwrap_or(serde_json::Value::Null);
                        let key = request_id_key(&id);
                        let cancel = CancellationToken::new();
                        let duplicate = {
                            let mut active = active.lock().await;
                            match active.entry(key.clone()) {
                                Entry::Occupied(_) => true,
                                Entry::Vacant(entry) => {
                                    entry.insert(cancel.clone());
                                    false
                                }
                            }
                        };
                        if duplicate {
                            let response = jsonrpc_error(id, -32600, "Duplicate request id", None);
                            write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                            continue;
                        }
                        let progress_token = params
                            .get("_meta")
                            .and_then(|meta| meta.get("progressToken"))
                            .cloned();
                        let orchestrator = orchestrator.clone();
                        let output = output.clone();
                        let writer = writer.clone();
                        let active = active.clone();
                        calls.spawn(async move {
                            let response = call_tool_response(
                                orchestrator,
                                output,
                                id,
                                params,
                                progress_token,
                                cancel,
                            )
                            .await;
                            active.lock().await.remove(&key);
                            let _ = write_protocol_frame(&writer, &response).await;
                        });
                    }
                    _ => {
                        let response = jsonrpc_error(
                            request_id.unwrap_or(serde_json::Value::Null),
                            -32601,
                            "Method not found",
                            Some(serde_json::json!({ "method": method })),
                        );
                        write_protocol_frame(&writer, &response).await.map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }

    for cancel in active.lock().await.values() {
        cancel.cancel();
    }
    if tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while calls.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        calls.shutdown().await;
    }
    Ok(())
}

async fn call_tool_response(
    orchestrator: Arc<orchestrator::ConversationOrchestrator>,
    output: Arc<McpProtocolOutput>,
    request_id: serde_json::Value,
    params: serde_json::Value,
    progress_token: Option<serde_json::Value>,
    cancel: CancellationToken,
) -> serde_json::Value {
    let Some(name) = params.get("name").and_then(serde_json::Value::as_str) else {
        return jsonrpc_error(request_id, -32602, "Missing tool name", None);
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !arguments.is_object() {
        return jsonrpc_error(request_id, -32602, "Tool arguments must be an object", None);
    }

    let tool_use_id = protocol::ToolUseId::new();
    output.register_progress(&tool_use_id, progress_token).await;
    let result = orchestrator
        .call_tool_from_host(
            tool_use_id.clone(),
            name.to_string(),
            arguments,
            Some(cancel),
        )
        .await;
    output.unregister_progress(&tool_use_id).await;

    match result {
        Ok(None) => jsonrpc_error(request_id, -32602, &format!("Unknown tool: {name}"), None),
        Ok(Some(protocol::ContentBlock::ToolResult {
            content,
            is_error,
            content_blocks,
            ..
        })) => {
            let content = content_blocks
                .unwrap_or_else(|| vec![serde_json::json!({ "type": "text", "text": content })]);
            jsonrpc_result(
                request_id,
                serde_json::json!({ "content": content, "isError": is_error }),
            )
        }
        Ok(Some(_)) => jsonrpc_result(
            request_id,
            serde_json::json!({
                "content": [{ "type": "text", "text": "Tool returned an unsupported result" }],
                "isError": true,
            }),
        ),
        Err(error) => jsonrpc_result(
            request_id,
            serde_json::json!({
                "content": [{ "type": "text", "text": format!("Tool execution failed: {error}") }],
                "isError": true,
            }),
        ),
    }
}

fn initialize_response(id: serde_json::Value) -> serde_json::Value {
    jsonrpc_result(
        id,
        serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "LingXi",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    )
}

fn jsonrpc_result(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn jsonrpc_error(
    id: serde_json::Value,
    code: i64,
    message: &str,
    data: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut error = serde_json::json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn request_id_key(id: &serde_json::Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| "null".into())
}

async fn write_protocol_frame(
    writer: &ProtocolWriter,
    frame: &serde_json::Value,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    bytes.push(b'\n');
    let mut stdout = writer.lock().await;
    stdout.write_all(&bytes).await?;
    stdout.flush().await
}

fn open_native_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let command: Option<(&str, &[&str])> = Some(("open", &[]));
    #[cfg(target_os = "linux")]
    let command: Option<(&str, &[&str])> = Some(("xdg-open", &[]));
    #[cfg(target_os = "windows")]
    let command: Option<(&str, &[&str])> = Some(("cmd", &["/c", "start", ""]));
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let command: Option<(&str, &[&str])> = None;
    if let Some((program, prefix)) = command {
        let _ = std::process::Command::new(program)
            .args(prefix)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Implement `mcp login <name>`.
async fn run_login(a: &LoginArgs) -> i32 {
    let Some(cfg) = find_loaded_server(&a.name) else {
        eprintln!(
            "No MCP server named \"{}\". Run `lingxi-cli mcp add` to add one.",
            a.name
        );
        return RUNTIME_ERROR;
    };

    let (server_url, oauth_cfg) = match extract_oauth_spec(&cfg) {
        Some(v) => v,
        None => {
            eprintln!(
                "MCP server \"{}\" does not have OAuth configuration.",
                a.name
            );
            return RUNTIME_ERROR;
        }
    };

    let storage = match mcp_token_storage().await {
        Ok(storage) => storage,
        Err(_) => {
            eprintln!("Failed to initialize MCP credential storage.");
            return RUNTIME_ERROR;
        }
    };
    let clock: Arc<dyn traits::Clock> = Arc::new(PosixClock::new());
    let http: Arc<dyn traits::HttpTransport> = Arc::new(PosixHttp::new());

    let server_key = oauth::server_key(&cfg.name, &cfg.spec);
    let no_browser = a.no_browser;
    let on_auth_url: oauth::OnAuthorizationUrl = Arc::new(move |url: &str| {
        if no_browser {
            println!("Open this URL in your browser to authenticate:");
            println!("{url}");
            println!("Paste the callback URL or authorization code, then press Enter:");
        } else {
            println!("Opening this URL for authentication:");
            println!("{url}");
            open_native_browser(url);
        }
    });
    let tokens = if no_browser {
        let mut input = BufReader::new(tokio::io::stdin());
        oauth::perform_oauth_flow_with_manual_input(
            &http,
            &clock,
            oauth_cfg,
            &cfg.name,
            server_url,
            &on_auth_url,
            None,
            &mut input,
        )
        .await
    } else {
        oauth::perform_oauth_flow(
            &http,
            &clock,
            oauth_cfg,
            &cfg.name,
            server_url,
            &on_auth_url,
            None,
        )
        .await
    };
    let tokens = match tokens {
        Ok(tokens) => tokens,
        Err(err) => {
            eprintln!("MCP login failed for \"{}\": {err}", a.name);
            return RUNTIME_ERROR;
        }
    };

    match oauth::save_tokens(&storage, &clock, &server_key, &tokens).await {
        Ok(()) => {
            println!("Successfully authenticated MCP server \"{}\".", a.name);
            SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to persist MCP tokens for \"{}\": {e}", a.name);
            RUNTIME_ERROR
        }
    }
}

/// Implement `mcp logout <name>`.
async fn run_logout(a: &LogoutArgs) -> i32 {
    let storage = match mcp_token_storage().await {
        Ok(storage) => storage,
        Err(_) => {
            eprintln!("Failed to initialize MCP credential storage.");
            return RUNTIME_ERROR;
        }
    };
    let http: Arc<dyn traits::HttpTransport> = Arc::new(PosixHttp::new());

    // Remote revocation is best-effort and only possible while the server
    // configuration still exists. Local deletion is authoritative and runs for
    // every matching credential even when the server is missing/disconnected.
    if let Some(cfg) = find_loaded_server(&a.name) {
        if let Some((server_url, oauth_cfg)) = extract_oauth_spec(&cfg) {
            let server_key = oauth::server_key(&cfg.name, &cfg.spec);
            oauth::revoke_server_tokens(&storage, &http, &server_key, server_url, oauth_cfg).await;
        }
    }

    let accounts = match storage.list(oauth::MCP_OAUTH_SERVICE).await {
        Ok(accounts) => accounts,
        Err(e) => {
            eprintln!("Failed to enumerate MCP tokens for \"{}\": {e}", a.name);
            return RUNTIME_ERROR;
        }
    };
    for account in credential_accounts_for_server(accounts, &a.name) {
        if let Err(e) = storage.delete(oauth::MCP_OAUTH_SERVICE, &account).await {
            eprintln!("Failed to clear MCP tokens for \"{}\": {e}", a.name);
            return RUNTIME_ERROR;
        }
    }
    println!("Successfully logged out from MCP server \"{}\".", a.name);
    SUCCESS
}

fn credential_accounts_for_server(accounts: Vec<String>, server_name: &str) -> Vec<String> {
    let prefix = format!("{server_name}|");
    accounts
        .into_iter()
        .filter(|account| account == server_name || account.starts_with(&prefix))
        .collect()
}

/// Implement `mcp add-from-claude-desktop`. Reads Claude Desktop's
/// `claude_desktop_config.json`, imports entries into the selected scope, and
/// reuses `write_server` to preserve the existing policy and name validation.
fn run_add_from_claude_desktop(a: &AddFromClaudeDesktopArgs) -> i32 {
    let Some(path) = claude_desktop_config_path() else {
        eprintln!("Claude Desktop config is unavailable on this platform.");
        return RUNTIME_ERROR;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!(
            "Could not read Claude Desktop config at {}.",
            path.display()
        );
        return RUNTIME_ERROR;
    };

    let Ok(imported) = mcp::json_config::parse_mcp_json_string(&raw, ConfigScope::Project) else {
        eprintln!("Could not parse Claude Desktop config.");
        return RUNTIME_ERROR;
    };
    if imported.is_empty() {
        eprintln!("No MCP servers found in {}.", path.display());
        return RUNTIME_ERROR;
    }

    let mut existing: HashSet<String> =
        load_all_servers().into_iter().map(|cfg| cfg.name).collect();
    let mut failures = 0usize;
    let mut added = 0usize;
    for cfg in imported {
        if existing.contains(&cfg.name) {
            eprintln!(
                "MCP server {} already exists in another configuration scope; skipping import.",
                cfg.name
            );
            failures += 1;
            continue;
        }
        let Some(entry) = cfg_to_entry_value(&cfg) else {
            eprintln!("Skipping unsupported MCP server \"{}\".", cfg.name);
            failures += 1;
            continue;
        };
        match write_server(&cfg.name, &entry, a.scope) {
            Ok(WriteOutcome::Added(path)) => {
                println!(
                    "Added MCP server {} to {} config.",
                    cfg.name,
                    a.scope.label()
                );
                print_file_modified(a.scope, &path);
                existing.insert(cfg.name.clone());
                added += 1;
            }
            Ok(WriteOutcome::AlreadyExists) => {
                eprintln!(
                    "MCP server {} already exists in {}.",
                    cfg.name,
                    a.scope.exists_suffix()
                );
                failures += 1;
            }
            Err(e) => {
                eprintln!("Failed to import MCP server {}: {e}", cfg.name);
                failures += 1;
            }
        }
    }

    if failures == 0 || added > 0 {
        SUCCESS
    } else {
        RUNTIME_ERROR
    }
}

/// Resolve one MCP config entry by name from the merged, precedence-ordered
/// current view used by `mcp list` / `mcp get`.
fn find_loaded_server(name: &str) -> Option<mcp::connection::McpServerConfig> {
    load_all_servers().into_iter().find(|cfg| cfg.name == name)
}

/// Extract `(url, oauth_cfg)` when `cfg` is an OAuth-capable MCP remote transport.
fn extract_oauth_spec(
    cfg: &mcp::connection::McpServerConfig,
) -> Option<(&str, &traits::McpOAuthConfigDto)> {
    match &cfg.spec {
        traits::McpTransportSpec::Sse {
            url,
            oauth: Some(cfg),
            ..
        } => Some((url.as_str(), cfg)),
        traits::McpTransportSpec::Http {
            url,
            oauth: Some(cfg),
            ..
        } => Some((url.as_str(), cfg)),
        _ => None,
    }
}

/// Build a CLI-local secure-storage backend used for MCP OAuth token persistence.
async fn mcp_token_storage() -> Result<Arc<dyn traits::SecureStorage>, String> {
    let home = crate::run::lingxi_home_dir();
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    platform_posix::secure_storage_for_platform(user, home.clone(), home.join(".credentials.json"))
        .await
        .map_err(|e| e.to_string())
}

/// Convert a parsed MCP server config into the JSON object shape accepted by
/// `write_server` (`command` / `args` / `env` or `type` + `url` + `headers`).
fn cfg_to_entry_value(cfg: &mcp::connection::McpServerConfig) -> Option<serde_json::Value> {
    match &cfg.spec {
        traits::McpTransportSpec::Stdio { command, args, env } => Some(serde_json::json!({
            "type": "stdio",
            "command": command,
            "args": args,
            "env": env,
        })),
        traits::McpTransportSpec::Sse { url, headers, .. } => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), "sse".into());
            obj.insert("url".into(), serde_json::Value::String(url.clone()));
            if !headers.is_empty() {
                obj.insert(
                    "headers".into(),
                    serde_json::Value::Object(
                        headers
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                            .collect(),
                    ),
                );
            }
            Some(serde_json::Value::Object(obj))
        }
        traits::McpTransportSpec::Http { url, headers, .. } => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".into(), "http".into());
            obj.insert("url".into(), serde_json::Value::String(url.clone()));
            if !headers.is_empty() {
                obj.insert(
                    "headers".into(),
                    serde_json::Value::Object(
                        headers
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                            .collect(),
                    ),
                );
            }
            Some(serde_json::Value::Object(obj))
        }
        _ => None,
    }
}

/// Resolve Claude Desktop config path for supported host families.
fn claude_desktop_config_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir()?;
        return Some(home.join("Library/Application Support/Claude/claude_desktop_config.json"));
    }
    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return wsl_claude_desktop_config_path();
    }
    None
}

fn wsl_claude_desktop_config_path() -> Option<PathBuf> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if let Some(appdata) = windows_path_to_wsl(&appdata) {
            return Some(appdata.join("Claude/claude_desktop_config.json"));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if let Some(profile) = windows_path_to_wsl(&profile) {
            return Some(profile.join("AppData/Roaming/Claude/claude_desktop_config.json"));
        }
    }

    // WSL's Linux username is not required to match the Windows account name.
    // Prefer asking Windows for its roaming profile before using the historical
    // same-name fallback.
    if let Ok(output) = std::process::Command::new("cmd.exe")
        .args(["/D", "/S", "/C", "echo %APPDATA%"])
        .output()
    {
        if output.status.success() {
            let appdata = String::from_utf8_lossy(&output.stdout);
            if let Some(appdata) = windows_path_to_wsl(appdata.trim()) {
                return Some(appdata.join("Claude/claude_desktop_config.json"));
            }
        }
    }

    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()?;
    Some(
        Path::new("/mnt/c/Users")
            .join(user)
            .join("AppData/Roaming/Claude/claude_desktop_config.json"),
    )
}

fn windows_path_to_wsl(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim().trim_matches('"');
    if raw.starts_with("/mnt/") {
        return Some(PathBuf::from(raw));
    }
    let bytes = raw.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let suffix = raw[3..].replace('\\', "/");
    Some(Path::new("/mnt").join(drive.to_string()).join(suffix))
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

/// Path a project `.mcp.json` WRITE targets: always `<cwd>/.mcp.json`.
///
/// Deliberately does NOT walk ancestors, even though discovery does. `mcp add
/// --scope project` and `mcp remove` mutate a file, and editing a `.mcp.json`
/// that lives above the working directory — possibly in `$HOME`, shared by
/// every repo underneath it — is a side effect no one asked for. Reading a
/// config you inherit is ordinary; silently rewriting it is not.
fn project_mcp_json_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".mcp.json"))
}

/// Path project `.mcp.json` DISCOVERY reads: the nearest one at or above cwd.
///
/// See [`nearest_project_mcp_json`] for why this walks. Split from the write
/// path so the asymmetry is explicit at every call site rather than implied.
fn project_mcp_json_read_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(nearest_project_mcp_json(&cwd))
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
                    a.args
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ),
            );
            obj.insert(
                "env".into(),
                serde_json::Value::Object(
                    env_map
                        .into_iter()
                        .map(|(k, v)| (k, serde_json::Value::String(v)))
                        .collect(),
                ),
            );
            Ok(serde_json::Value::Object(obj))
        }
        Transport::Sse | Transport::Http => {
            let ty = if matches!(transport, Transport::Sse) {
                "sse"
            } else {
                "http"
            };
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
    const RECOGNIZED: [&str; 8] = [
        "local",
        "user",
        "project",
        "dynamic",
        "enterprise",
        "claudeai",
        "managed",
        "agent",
    ];
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
                    let kind = if matches!(transport, Transport::Sse) {
                        "SSE"
                    } else {
                        "HTTP"
                    };
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
            eprintln!(
                "MCP server {} already exists in {}",
                a.name,
                scope.exists_suffix()
            );
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
            println!(
                "Added {} MCP server {} to {} config",
                ty,
                a.name,
                a.scope.label()
            );
            SUCCESS
        }
        Ok(WriteOutcome::AlreadyExists) => {
            eprintln!(
                "MCP server {} already exists in {}",
                a.name,
                a.scope.exists_suffix()
            );
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

use mcp::normalization::is_reserved_mcp_server_name;

/// Write a server entry into the chosen scope's config file, returning whether
/// it was newly added or already present.
/// Validate an MCP server name the way TS `addMcpServer` does before any write,
/// in the same order: first reject any char outside `[a-zA-Z0-9_-]`
/// (TS `/[^a-zA-Z0-9_-]/`), then reject a reserved name (TS `TEt`, see
/// [`is_reserved_mcp_server_name`]) — both with the byte-faithful message.
fn validate_mcp_server_name(name: &str) -> Result<(), String> {
    if name
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err(format!(
            "Invalid name {name}. Names can only contain letters, numbers, hyphens, and underscores."
        ));
    }
    if is_reserved_mcp_server_name(name) {
        return Err(format!(
            "Cannot add MCP server \"{name}\": this name is reserved."
        ));
    }
    Ok(())
}

fn write_server(
    name: &str,
    entry: &serde_json::Value,
    scope: Scope,
) -> Result<WriteOutcome, String> {
    // TS `addMcpServer` (`TPe`) runs these gates BEFORE any scope write, in this
    // order; both `mcp add` and `mcp add-json` route through here.
    // 1. name char + reserved-name validation.
    validate_mcp_server_name(name)?;
    // 2. `D1()` — when a managed MCP configuration is active it has exclusive
    //    control and every add is refused (this fires before the allow/deny
    //    matchers, so those never run while it holds).
    if mcp::enterprise_policy::enterprise_mcp_active() {
        return Err(mcp::enterprise_policy::ENTERPRISE_EXCLUSIVE_CONTROL_MESSAGE.to_string());
    }
    // 3. `bPe`/`gPe` — enterprise allow/deny policy (from the managed settings
    //    tiers). Inert when no policy is configured (nothing denied, all
    //    allowed).
    let policy = mcp::enterprise_policy::read_managed_mcp_policy();
    if mcp::enterprise_policy::is_denied(name, entry, &policy) {
        return Err(mcp::enterprise_policy::denied_message(name));
    }
    if !mcp::enterprise_policy::is_allowed(name, entry, &policy) {
        return Err(mcp::enterprise_policy::not_allowed_message(name));
    }
    match scope {
        Scope::User => write_user_server(name, entry),
        Scope::Local => write_local_server(name, entry),
        Scope::Project => write_project_server(name, entry),
    }
}

/// User scope: `~/.lingxi.json` top-level `mcpServers.<name>`.
fn write_user_server(name: &str, entry: &serde_json::Value) -> Result<WriteOutcome, String> {
    let path =
        global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
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
    let path =
        global_config_path().ok_or_else(|| "Could not resolve home directory".to_string())?;
    let key = project_key().ok_or_else(|| "Could not resolve project directory".to_string())?;
    let existing =
        migrations::global_config::get_project_config(&path, &key).map_err(|e| e.to_string())?;
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
    let path =
        project_mcp_json_path().ok_or_else(|| "Could not resolve project directory".to_string())?;
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
                println!(
                    "Removed MCP server {} from {} config",
                    a.name,
                    scope.label()
                );
                print_file_modified(scope, &path);
                SUCCESS
            }
            Ok(None) => {
                eprintln!(
                    "No MCP server named \"{}\" {}",
                    a.name,
                    scope.not_found_suffix()
                );
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
                    println!(
                        "Removed MCP server \"{}\" from {} config",
                        a.name,
                        scope.label()
                    );
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
                eprintln!(
                    "  - {} ({})",
                    scope_remove_label(scope),
                    scope_config_path_desc(scope)
                );
            }
            eprintln!();
            eprintln!("To remove from a specific scope, use:");
            for &scope in scopes {
                // claude prints the command hint with the name UNQUOTED (the
                // surrounding message text quotes it, but the copy-paste command
                // does not — verified against the live 2.1.191 binary).
                eprintln!("  lingxi-cli mcp remove {} -s {}", a.name, scope.label());
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
    let cwd = std::env::current_dir()
        .map(|c| c.display().to_string())
        .unwrap_or_default();
    let global = global_config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
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
            let path = global_config_path()
                .ok_or_else(|| "Could not resolve home directory".to_string())?;
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
            let path = global_config_path()
                .ok_or_else(|| "Could not resolve home directory".to_string())?;
            let key =
                project_key().ok_or_else(|| "Could not resolve project directory".to_string())?;
            let existing = migrations::global_config::get_project_config(&path, &key)
                .map_err(|e| e.to_string())?;
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
            let path = project_mcp_json_path()
                .ok_or_else(|| "Could not resolve project directory".to_string())?;
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
const PENDING_APPROVAL: &str = "\u{23F8} Pending approval (run `lingxi-cli` to approve)";

/// Implement `mcp list`. Reads the merged server set across all three scopes
/// and prints each as `<name>: <summary>`.
///
/// Unapproved (repo-self-approved) project `.mcp.json` servers in an untrusted
/// workspace are shown as `\u23F8 Pending approval (run `claude` to approve)`
/// and are NOT connected to — 1:1 with the binary `mcp list` (`$Tf`):
/// `status: n.has(i) ? SSc : (await ySc(i,a)).status`, where the pending branch
/// skips the health check (`ySc` = the spawn/connect probe).
///
/// APPROVED servers are health-checked over the network (`✔ Connected` /
/// `✘ Failed to connect`); pending ones show the approval status, servers
/// with a config-level error show `- Not configured`, and explicitly-REJECTED
/// project servers are omitted entirely (the binary's list-path `J9` call has
/// no `includeRejectedProjectServers`). A config-diagnostics footer (`vgn`)
/// follows the rows.
///
/// How long a single server gets to complete its initialize handshake.
///
/// A wedged server must not hang the listing: it reports as a timeout and the
/// remaining servers are still checked.
const HEALTH_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

const CONNECTED: &str = "\u{2714} Connected";
const FAILED_TO_CONNECT: &str = "\u{2718} Failed to connect";

/// Byte-exact claude-code status for an explicitly-rejected `.mcp.json` server
/// (binary `fJy = "${qe.cross} Rejected (see disabledMcpjsonServers in
/// settings)"`, `qe.cross` = U+2718). Shown by `mcp get`; `mcp list` OMITS
/// rejected servers entirely (its `J9` call passes no
/// `includeRejectedProjectServers`, so they never enter the server map).
/// Rejected servers are never health-checked or spawned.
const REJECTED: &str = "\u{2718} Rejected (see disabledMcpjsonServers in settings)";

/// Byte-exact claude-code status for a server whose config makes it
/// unconnectable (`yEp`'s `Qee(r)` branch — connect skipped with
/// `errorCode:"UNCONFIGURED"`, e.g. a `url` that expanded to an empty string).
const NOT_CONFIGURED: &str = "- Not configured";

async fn run_list() -> i32 {
    let servers = load_all_servers();
    if servers.is_empty() {
        println!("No MCP servers configured. Use `lingxi-cli mcp add` to add a server.");
        // The oracle renders the config-diagnostics panel (`vgn`) under the
        // empty-state message too.
        print_config_diagnostics();
        return SUCCESS;
    }
    let approval = project_server_approval();

    // The health check SPAWNS stdio servers and opens network connections, so
    // it runs only for servers the user has actually accepted: user- and
    // local-scope servers were added by an explicit `mcp add`, and a project
    // `.mcp.json` server stays PENDING until approved. That bound is what makes
    // this safe now that discovery walks up to ancestor `.mcp.json` files — an
    // inherited config is listed, never executed, until someone approves it.
    println!("Checking MCP server health\u{2026}");
    println!();

    let transport: std::sync::Arc<dyn traits::McpTransport> =
        std::sync::Arc::new(platform_posix::PosixMcpTransport::new());
    let registry = mcp::McpRegistry::new(transport);

    for cfg in &servers {
        // Explicitly-rejected project server: OMITTED from the listing — the
        // binary's list-path `J9` call passes no `includeRejectedProjectServers`,
        // so a rejected server never enters the server map at all.
        if is_rejected_project_server(cfg, &approval.rejected) {
            continue;
        }
        let summary = transport_summary(&cfg.spec);
        if is_pending_project_server(cfg, &approval.pending) {
            // Unapproved project server: never spawned, so never health-checked
            // — 1:1 with the binary, whose pending branch skips `ySc`. The
            // transport summary still prints: hiding the URL makes it
            // impossible to see WHAT you are being asked to approve.
            println!("{}: {summary} - {PENDING_APPROVAL}", cfg.name);
            continue;
        }
        if cfg.config_error.is_some() {
            // configError (url expanded to empty): connect is skipped with
            // `errorCode:"UNCONFIGURED"` → `yEp` reports `- Not configured`
            // (no issue text), without dialing anything.
            println!("{}: {summary} - {NOT_CONFIGURED}", cfg.name);
            continue;
        }
        let status =
            match tokio::time::timeout(HEALTH_CHECK_TIMEOUT, registry.connect(cfg.clone())).await {
                Ok(Ok(_)) => {
                    // Tear the probe connection down; `mcp list` must not leave
                    // a spawned child behind.
                    let _ = registry.disconnect(&cfg.name).await;
                    CONNECTED.to_string()
                }
                Ok(Err(e)) => format!("{FAILED_TO_CONNECT} \u{2014} {e}"),
                Err(_) => format!(
                    "{FAILED_TO_CONNECT} \u{2014} timed out after {}s",
                    HEALTH_CHECK_TIMEOUT.as_secs()
                ),
            };
        println!("{}: {summary} - {status}", cfg.name);
    }
    // Config diagnostics footer — the oracle's `vgn` panel rendered under the
    // rows (missing env vars, whitespace, skipped entries, `servers` typo).
    print_config_diagnostics();
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
    // to — 1:1 with the binary `mcp get` (`hJy`): `s==="pending" ?
    // {status:TEp} : s==="rejected" ? {status:fJy} : await yEp(t,i)`, where the
    // pending AND rejected branches SKIP the `yEp` health-check/spawn. The
    // transport details are still printed (the binary keeps appending
    // Type/URL/etc. after the Status line). Only PROJECT servers can be
    // pending/rejected (user/local servers never require approval).
    let approval = project_server_approval();
    let is_pending = is_pending_project_server(cfg, &approval.pending);
    let is_rejected = is_rejected_project_server(cfg, &approval.rejected);

    println!("{}:", cfg.name);
    println!("  Scope: {}", scope_detail(cfg.scope));
    if is_pending {
        println!("  Status: {PENDING_APPROVAL}");
    } else if is_rejected {
        println!("  Status: {REJECTED}");
    } else if cfg.config_error.is_some() {
        // configError → `yEp` skips the connect and reports `- Not configured`
        // (`Qee`'s UNCONFIGURED branch carries no issue text).
        println!("  Status: {NOT_CONFIGURED}");
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
    println!(
        "To remove this server, run: lingxi-cli mcp remove {} -s {scope_flag}",
        cfg.name
    );
    SUCCESS
}

/// Load every configured server across local + project + user scopes.
/// The nearest `.mcp.json` at or above `cwd`, or `<cwd>/.mcp.json` when none
/// exists anywhere up the tree.
///
/// The oracle resolves the project config by walking UP from the working
/// directory, so `claude mcp list` from `repo/src/foo/` still sees
/// `repo/.mcp.json`. This port only looked at `<cwd>/.mcp.json`, which meant a
/// user anywhere but the repo root silently got NO project servers — verified
/// against 2.1.220 with a parent/child fixture.
///
/// Discovery is not trust: a server found this way is still subject to the
/// same project-approval gate, so it lists as "Pending approval" until the
/// user accepts it. Falling back to `<cwd>/.mcp.json` keeps the "where would a
/// new one be written" answer unchanged.
fn nearest_project_mcp_json(cwd: &std::path::Path) -> PathBuf {
    for dir in cwd.ancestors() {
        let candidate = dir.join(".mcp.json");
        if candidate.is_file() {
            return candidate;
        }
    }
    cwd.join(".mcp.json")
}

fn load_all_servers() -> Vec<mcp::connection::McpServerConfig> {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(_) => PathBuf::from("."),
    };
    let project_mcp = nearest_project_mcp_json(&cwd);
    let Some(global) = global_config_path() else {
        // No home: only a project .mcp.json could exist.
        return mcp::json_config::load_mcp_servers(
            &project_mcp,
            &PathBuf::from("/nonexistent"),
            &cwd,
        );
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
        return format!("No MCP server named \"{name}\". Run `lingxi-cli mcp add` to add one.");
    }
    const CAP: usize = 8;
    let shown = names[..names.len().min(CAP)].join(", ");
    let suffix = if names.len() > CAP {
        format!(
            " (and {} more — run `lingxi-cli mcp list` to see all)",
            names.len() - CAP
        )
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

/// Print the oracle's "MCP config diagnostics" panel (`vgn`) as plain text
/// under the `mcp list` output: title + docs link, then per-scope groups in
/// the panel's order (user, project, local) with `[Error]` / `[Warning]` rows
/// (`JQs`). Per-server WARNING rows whose server is overridden by a later
/// scope are suppressed (`vSp`: user < project-when-approved < local); fatal
/// rows never are. Silent when every config is clean, so a healthy setup
/// prints nothing. (The ink panel's status glyph and tree guides have no
/// plain-text equivalent; the strings themselves are byte-faithful.)
fn print_config_diagnostics() {
    use mcp::config_diagnostics::{McpConfigSeverity, McpConfigWarning};

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_mcp = nearest_project_mcp_json(&cwd);
    let global = global_config_path();
    let warnings = mcp::config_diagnostics::collect_all_mcp_config_warnings_at(
        &project_mcp,
        &cwd,
        global.as_deref(),
    );
    if warnings.is_empty() {
        return;
    }

    // `vSp` suppression inputs: server names defined (and active) in the
    // scopes that override each source.
    let local_names = local_server_names();
    let approved_project = project_server_approval().approved;

    let mut printed_header = false;
    for scope in [ConfigScope::User, ConfigScope::Project, ConfigScope::Local] {
        let rows: Vec<&McpConfigWarning> = warnings
            .iter()
            .filter(|w| w.scope == scope)
            .filter(|w| {
                if w.severity == McpConfigSeverity::Fatal {
                    return true;
                }
                let Some(name) = &w.server_name else {
                    return true;
                };
                match scope {
                    // A user-scope server is overridden by an APPROVED project
                    // server (`gKy`) or a local one.
                    ConfigScope::User => {
                        !approved_project.contains(name) && !local_names.contains(name)
                    }
                    ConfigScope::Project => !local_names.contains(name),
                    _ => true,
                }
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        if !printed_header {
            printed_header = true;
            println!();
            println!("MCP config diagnostics");
            println!(
                "For help configuring MCP servers, see: https://code.claude.com/docs/en/mcp"
            );
        }
        let has_fatal = rows
            .iter()
            .any(|w| w.severity == McpConfigSeverity::Fatal);
        println!();
        println!(
            "[{}] {}",
            if has_fatal {
                "Failed to parse"
            } else {
                "Contains warnings"
            },
            scope_detail(scope)
        );
        if let Some(f) = rows.iter().find_map(|w| w.file.as_ref()) {
            println!("Location: {f}");
        }
        for w in rows {
            let tag = match w.severity {
                McpConfigSeverity::Fatal => "[Error]",
                McpConfigSeverity::Warning => "[Warning]",
            };
            let name = w
                .server_name
                .as_deref()
                .map(|n| format!("[{n}] "))
                .unwrap_or_default();
            let path = if w.path.is_empty() {
                String::new()
            } else {
                format!("{}: ", w.path)
            };
            println!("  {tag} {name}{path}{}", w.message);
        }
    }
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

/// Whether a LOADED server is an explicitly-rejected project `.mcp.json`
/// server (in `disabledMcpjsonServers`). Same scope guard as
/// [`is_pending_project_server`]: a same-named USER or LOCAL server (which
/// takes precedence in the loaded view and never requires approval) must NOT
/// be suppressed or labelled rejected.
fn is_rejected_project_server(cfg: &mcp::connection::McpServerConfig, rejected: &[String]) -> bool {
    cfg.scope == ConfigScope::Project && rejected.iter().any(|n| n == &cfg.name)
}

/// Per-project approval state of the `.mcp.json` servers, mirroring claude's
/// three-way classifier (`SZr`): approved / pending / rejected.
#[derive(Debug, Default)]
struct ProjectServerApproval {
    /// NOT in `disabledMcpjsonServers` AND (`enableAllProjectMcpServers` OR in
    /// `enabledMcpjsonServers`) — loaded and health-checked normally.
    approved: Vec<String>,
    /// Neither approved nor rejected — shown as `⏸ Pending approval` and never
    /// spawned.
    pending: Vec<String>,
    /// In `disabledMcpjsonServers` — explicitly rejected: OMITTED from `mcp
    /// list` entirely and shown by `mcp get` as `✘ Rejected (see
    /// disabledMcpjsonServers in settings)`, never spawned.
    rejected: Vec<String>,
}

/// Partition the project `.mcp.json` servers into approved / pending /
/// rejected using the per-project approval state in `~/.lingxi.json`
/// `projects.<key>`: a server is REJECTED when in `disabledMcpjsonServers`
/// (an explicit rejection always wins), APPROVED when
/// `enableAllProjectMcpServers` is true OR it is in `enabledMcpjsonServers`,
/// and PENDING otherwise. `pending` project servers are the ones claude's
/// `mcp get` not-found message omits from the list and flags with an
/// awaiting-approval note; `rejected` ones are omitted without the note.
fn project_server_approval() -> ProjectServerApproval {
    let all: Vec<String> = project_mcp_json_read_path()
        .filter(|p| p.exists())
        .and_then(|p| read_json_object(&p).ok())
        .and_then(|m| m.get("mcpServers").and_then(|v| v.as_object()).cloned())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if all.is_empty() {
        return ProjectServerApproval::default();
    }
    let str_array = |cfg: &serde_json::Map<String, serde_json::Value>, key: &str| -> Vec<String> {
        cfg.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (enable_all, enabled, disabled) = global_config_path()
        .zip(project_key())
        .and_then(|(p, k)| migrations::global_config::get_project_config(&p, &k).ok())
        .map(|cfg| {
            (
                cfg.get("enableAllProjectMcpServers")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                str_array(&cfg, "enabledMcpjsonServers"),
                str_array(&cfg, "disabledMcpjsonServers"),
            )
        })
        .unwrap_or((false, Vec::new(), Vec::new()));
    let mut out = ProjectServerApproval::default();
    for name in all {
        match classify_project_server(&name, enable_all, &enabled, &disabled) {
            ProjectServerState::Approved => out.approved.push(name),
            ProjectServerState::Pending => out.pending.push(name),
            ProjectServerState::Rejected => out.rejected.push(name),
        }
    }
    out
}

/// Three-way project-server state, 1:1 with claude's `SZr` classifier values
/// (`"approved" | "pending" | "rejected"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectServerState {
    Approved,
    Pending,
    Rejected,
}

/// claude `SZr`: an explicit rejection (`disabledMcpjsonServers`) always wins;
/// otherwise approved per [`project_server_is_approved`], else pending.
fn classify_project_server(
    name: &str,
    enable_all: bool,
    enabled: &[String],
    disabled: &[String],
) -> ProjectServerState {
    if disabled.iter().any(|d| d == name) {
        ProjectServerState::Rejected
    } else if project_server_is_approved(name, enable_all, enabled, disabled) {
        ProjectServerState::Approved
    } else {
        ProjectServerState::Pending
    }
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
    !disabled.iter().any(|d| d == name) && (enable_all || enabled.iter().any(|e| e == name))
}

/// `mcp get`'s not-found message. Unlike `mcp remove` (which lists every
/// config-level server across all scopes), claude's `mcp get` lists only the
/// LOADED servers — user + local + APPROVED project — and, when there are
/// pending (unapproved) `.mcp.json` servers, OMITS them from the list and
/// appends an awaiting-approval note instead. Verified against the live 2.1.191
/// binary.
fn not_found_message_get(name: &str) -> String {
    let approval = project_server_approval();
    let pending_project = approval.pending;
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    set.extend(user_server_names());
    set.extend(local_server_names());
    set.extend(approval.approved);
    let names: Vec<String> = set.into_iter().collect(); // BTreeSet ⇒ already sorted/unique
    let clause = if pending_project.is_empty() {
        ""
    } else {
        " (.mcp.json servers are awaiting approval — run `lingxi-cli` in this directory to review them.)"
    };
    if names.is_empty() && clause.is_empty() {
        return format!("No MCP server named \"{name}\". Run `lingxi-cli mcp add` to add one.");
    }
    if names.is_empty() {
        // Pending servers exist but NOTHING is loaded. Falling through would
        // print an empty "Configured servers: " list and then parenthesise the
        // note as an aside to a list that is not there. The oracle drops both
        // and states the pending situation directly.
        return format!(
            "No MCP server named \"{name}\". \
             .mcp.json servers are awaiting approval \u{2014} run `lingxi-cli` in this directory to review them."
        );
    }
    const CAP: usize = 8;
    let shown = names[..names.len().min(CAP)].join(", ");
    let suffix = if names.len() > CAP {
        format!(
            " (and {} more — run `lingxi-cli mcp list` to see all)",
            names.len() - CAP
        )
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
            let Some(path) = global_config_path() else {
                return false;
            };
            migrations::global_config::read_map(&path)
                .ok()
                .and_then(|m| {
                    m.get("mcpServers")
                        .and_then(serde_json::Value::as_object)
                        .cloned()
                })
                .is_some_and(|m| m.contains_key(name))
        }
        Scope::Local => {
            let Some(path) = global_config_path() else {
                return false;
            };
            let Some(key) = project_key() else {
                return false;
            };
            migrations::global_config::get_project_config(&path, &key)
                .ok()
                .and_then(|m| {
                    m.get("mcpServers")
                        .and_then(serde_json::Value::as_object)
                        .cloned()
                })
                .is_some_and(|m| m.contains_key(name))
        }
        Scope::Project => {
            let Some(path) = project_mcp_json_path() else {
                return false;
            };
            if !path.exists() {
                return false;
            }
            read_json_object(&path)
                .ok()
                .and_then(|m| {
                    m.get("mcpServers")
                        .and_then(serde_json::Value::as_object)
                        .cloned()
                })
                .is_some_and(|m| m.contains_key(name))
        }
    }
}

/// One-line transport summary for `list` (e.g. `echo hello`,
/// `https://x/mcp (HTTP)`).
fn transport_summary(spec: &traits::McpTransportSpec) -> String {
    match spec {
        traits::McpTransportSpec::Stdio { command, args, .. } => {
            // Unconditional `{command} {args}`, matching the oracle — an
            // argless stdio server renders WITH a trailing space
            // (`mock_stdio_mcp  - ✘ …`). Special-casing the empty-args case to
            // trim it looks tidier and is a byte divergence.
            format!("{command} {}", args.join(" "))
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
        proj.insert(
            "enableAllProjectMcpServers".into(),
            serde_json::json!(false),
        );
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
    project_mcp_json_read_path()
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
fn read_json_object(
    path: &std::path::Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?;
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
mod transport_summary_tests {
    use traits::McpTransportSpec;

    #[test]
    fn argless_stdio_keeps_the_oracle_trailing_space() {
        // The oracle formats `{command} {args}` unconditionally, so an argless
        // stdio server renders with a trailing space before the ` - status`
        // separator (`mock_stdio_mcp  - ✘ …`). Trimming it looks tidier and is
        // a byte divergence.
        let spec = McpTransportSpec::Stdio {
            command: "/bin/thing".into(),
            args: vec![],
            env: Default::default(),
        };
        assert_eq!(super::transport_summary(&spec), "/bin/thing ");
    }

    #[test]
    fn stdio_with_args_joins_them() {
        let spec = McpTransportSpec::Stdio {
            command: "echo".into(),
            args: vec!["hi".into(), "there".into()],
            env: Default::default(),
        };
        assert_eq!(super::transport_summary(&spec), "echo hi there");
    }

    #[test]
    fn http_and_sse_carry_their_transport_label() {
        let http = McpTransportSpec::Http {
            url: "https://x.example/mcp".into(),
            headers: Default::default(),
            oauth: None,
        };
        assert_eq!(super::transport_summary(&http), "https://x.example/mcp (HTTP)");
    }
}

#[cfg(test)]
mod project_discovery_tests {
    /// Project `.mcp.json` discovery walks ANCESTORS, matching the oracle.
    /// This port used to look only at `<cwd>/.mcp.json`, so a user in any
    /// subdirectory of their repo silently got no project servers — verified
    /// against 2.1.220 with a parent/child fixture.

    #[test]
    fn discovery_finds_a_parent_mcp_json() {
        let root = tempfile::tempdir().expect("tempdir");
        let parent = root.path().join("repo");
        let child = parent.join("src").join("deep");
        std::fs::create_dir_all(&child).unwrap();
        let cfg = parent.join(".mcp.json");
        std::fs::write(&cfg, "{}").unwrap();
        assert_eq!(super::nearest_project_mcp_json(&child), cfg);
    }

    #[test]
    fn discovery_prefers_the_nearest_ancestor() {
        let root = tempfile::tempdir().expect("tempdir");
        let outer = root.path().join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(outer.join(".mcp.json"), "{}").unwrap();
        let near = inner.join(".mcp.json");
        std::fs::write(&near, "{}").unwrap();
        assert_eq!(super::nearest_project_mcp_json(&inner), near);
    }

    #[test]
    fn discovery_falls_back_to_cwd_when_no_ancestor_has_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("a").join("b");
        std::fs::create_dir_all(&dir).unwrap();
        // Keeps "where would a new one be written" unchanged.
        assert_eq!(super::nearest_project_mcp_json(&dir), dir.join(".mcp.json"));
    }

    #[test]
    fn a_directory_named_mcp_json_is_not_a_config() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("proj");
        std::fs::create_dir_all(dir.join(".mcp.json")).unwrap();
        // `is_file()`, not `exists()`.
        assert_eq!(super::nearest_project_mcp_json(&dir), dir.join(".mcp.json"));
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
        assert_eq!(
            redact_url_for_display("http://h:8080/a/b?x=1"),
            "http://h:8080/a/b"
        );
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
mod serve_protocol_tests {
    use super::{
        credential_accounts_for_server, initialize_response, jsonrpc_error, windows_path_to_wsl,
    };

    #[test]
    fn initialize_response_is_a_stable_2025_06_18_frame() {
        let encoded = serde_json::to_string(&initialize_response(serde_json::json!(1))).unwrap();
        assert_eq!(
            encoded,
            format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{\"tools\":{{\"listChanged\":false}}}},\"serverInfo\":{{\"name\":\"LingXi\",\"version\":\"{}\"}}}}}}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn protocol_error_preserves_request_id_and_optional_data() {
        assert_eq!(
            jsonrpc_error(
                serde_json::json!("req-7"),
                -32602,
                "bad params",
                Some(serde_json::json!({ "field": "name" })),
            ),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "req-7",
                "error": {
                    "code": -32602,
                    "message": "bad params",
                    "data": { "field": "name" },
                }
            })
        );
    }

    #[test]
    fn logout_matches_all_hashed_credentials_for_only_one_server() {
        assert_eq!(
            credential_accounts_for_server(
                vec![
                    "alpha|1111".into(),
                    "beta|2222".into(),
                    "alpha|3333".into(),
                    "alpha".into(),
                ],
                "alpha",
            ),
            vec!["alpha|1111", "alpha|3333", "alpha"]
        );
    }

    #[test]
    fn translates_windows_profile_paths_for_wsl_without_assuming_linux_username() {
        assert_eq!(
            windows_path_to_wsl(r#"C:\Users\Windows User\AppData\Roaming"#).as_deref(),
            Some(std::path::Path::new(
                "/mnt/c/Users/Windows User/AppData/Roaming"
            ))
        );
        assert_eq!(
            windows_path_to_wsl(r#"D:/Profiles/Alice/AppData/Roaming"#).as_deref(),
            Some(std::path::Path::new(
                "/mnt/d/Profiles/Alice/AppData/Roaming"
            ))
        );
        assert!(windows_path_to_wsl("relative/path").is_none());
    }
}

#[cfg(test)]
mod name_validation_tests {
    use super::{is_reserved_mcp_server_name, validate_mcp_server_name};

    #[test]
    fn rejects_invalid_name_chars_with_byte_exact_message() {
        for bad in ["my server", "srv!", "a/b", "dot.name", "caf\u{00e9}"] {
            let err = validate_mcp_server_name(bad).expect_err("invalid name must be rejected");
            assert_eq!(
                err,
                format!("Invalid name {bad}. Names can only contain letters, numbers, hyphens, and underscores.")
            );
        }
    }

    #[test]
    fn accepts_valid_name_chars() {
        for ok in ["srv", "my-server_1", "ABC123", "a", "___", "---"] {
            assert!(
                validate_mcp_server_name(ok).is_ok(),
                "valid name rejected: {ok}"
            );
        }
    }

    #[test]
    fn rejects_reserved_names_with_byte_exact_message() {
        // TS `TEt`: gE / "computer-use" / W2h (Chrome preview/browser) / "workspace".
        // Chrome-preview names carry a space so they're matched via `Bc`-normalized
        // input; `workspace` is matched raw. All are valid-char, so they pass the
        // char check and hit the reserved check.
        for reserved in [
            "claude-in-chrome",
            "computer-use",
            "Claude_Preview",
            "Claude_Browser",
            "workspace",
        ] {
            let err =
                validate_mcp_server_name(reserved).expect_err("reserved name must be rejected");
            assert_eq!(
                err,
                format!("Cannot add MCP server \"{reserved}\": this name is reserved.")
            );
        }
    }

    #[test]
    fn reserved_predicate_matches_via_normalizer_and_raw() {
        // `Bc`-normalized matches (a raw space normalizes to `_`).
        assert!(is_reserved_mcp_server_name("Claude Preview"));
        assert!(is_reserved_mcp_server_name("Claude Browser"));
        assert!(is_reserved_mcp_server_name("computer-use"));
        assert!(is_reserved_mcp_server_name("claude-in-chrome"));
        // `workspace` is the only RAW (un-normalized) match — a decorated variant
        // normalizes differently and is NOT reserved.
        assert!(is_reserved_mcp_server_name("workspace"));
        assert!(!is_reserved_mcp_server_name("workspaces"));
        assert!(!is_reserved_mcp_server_name("my-workspace"));
        // Ordinary names stay allowed.
        assert!(!is_reserved_mcp_server_name("filesystem"));
        assert!(!is_reserved_mcp_server_name("sentry"));
    }
}

#[cfg(test)]
mod pending_approval_tests {
    use super::{
        classify_project_server, is_pending_project_server, is_rejected_project_server,
        project_server_is_approved, ProjectServerState, NOT_CONFIGURED, PENDING_APPROVAL, REJECTED,
    };
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
            timeout_ms: None,
            always_load: false,
            config_error: None,
        }
    }

    /// The status string is byte-exact with the binary `SSc` (U+23F8 pause glyph,
    /// backtick-quoted `claude`).
    #[test]
    fn pending_approval_string_is_byte_exact() {
        assert_eq!(
            PENDING_APPROVAL,
            "\u{23F8} Pending approval (run `lingxi-cli` to approve)"
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
        assert!(!project_server_is_approved(
            "s",
            true,
            &["s".into()],
            &["s".into()]
        ));
        // Neither enabled nor enable-all ⇒ not approved.
        assert!(!project_server_is_approved(
            "s",
            false,
            &["other".into()],
            &[]
        ));
    }

    /// TRUST-RESET: after `mcp reset-project-choices` clears every choice
    /// (empty enabled, empty disabled, enable_all=false), a previously-approved
    /// project server reverts to NOT-approved → pending, so `mcp list`/`get`
    /// once again show it as Pending approval and never spawn it.
    #[test]
    fn reset_project_choices_reverts_approved_server_to_pending() {
        // Before reset: enabled ⇒ approved.
        assert!(project_server_is_approved(
            "repo-srv",
            false,
            &["repo-srv".into()],
            &[]
        ));
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

    /// The rejected status string is byte-exact with the binary `fJy` —
    /// `${qe.cross} Rejected (see disabledMcpjsonServers in settings)` with
    /// `qe.cross` = U+2718 (HEAVY BALLOT X, same glyph as Failed to connect).
    #[test]
    fn rejected_string_is_byte_exact() {
        assert_eq!(
            REJECTED,
            "\u{2718} Rejected (see disabledMcpjsonServers in settings)"
        );
        // And the UNCONFIGURED status (`yEp`'s `Qee` branch).
        assert_eq!(NOT_CONFIGURED, "- Not configured");
    }

    /// `SZr` three-way classification: disabled always wins (rejected, even
    /// against enable-all AND an explicit enable), enabled/enable-all is
    /// approved, everything else is pending.
    #[test]
    fn classifier_matches_szr() {
        // Rejected: in disabled, regardless of the other knobs.
        assert_eq!(
            classify_project_server("s", true, &["s".into()], &["s".into()]),
            ProjectServerState::Rejected
        );
        // Approved: explicitly enabled, or enable-all.
        assert_eq!(
            classify_project_server("s", false, &["s".into()], &[]),
            ProjectServerState::Approved
        );
        assert_eq!(
            classify_project_server("s", true, &[], &[]),
            ProjectServerState::Approved
        );
        // Pending: no choice recorded.
        assert_eq!(
            classify_project_server("s", false, &[], &[]),
            ProjectServerState::Pending
        );
    }

    /// A REJECTED project server is distinct from a pending one: `mcp list`
    /// omits it entirely and `mcp get` shows the Rejected status. Same
    /// scope guard as pending — a same-named user/local server is never
    /// suppressed.
    #[test]
    fn rejected_project_server_scope_guard() {
        let rejected = vec!["srv".to_string()];
        assert!(is_rejected_project_server(
            &stdio("srv", ConfigScope::Project),
            &rejected
        ));
        for scope in [ConfigScope::User, ConfigScope::Local] {
            assert!(
                !is_rejected_project_server(&stdio("srv", scope), &rejected),
                "{scope:?} server must not be treated as rejected"
            );
        }
    }

    /// A server carrying a parse-time `configError` (url expanded to empty)
    /// reports `- Not configured` and must never be dialed: the registry
    /// connect path short-circuits to a Connection error holding the
    /// configError text.
    #[tokio::test]
    async fn config_error_server_is_not_dialed() {
        let mut cfg = stdio("broken", ConfigScope::User);
        cfg.config_error = Some(
            "'url' \"${X}\" expanded to an empty string. Set the referenced \
             environment variable, or update the server's config and reconnect."
                .to_string(),
        );
        let transport: std::sync::Arc<dyn traits::McpTransport> =
            std::sync::Arc::new(platform_posix::PosixMcpTransport::new());
        let registry = mcp::McpRegistry::new(transport);
        let err = registry.connect(cfg.clone()).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("connection failed: {}", cfg.config_error.unwrap())
        );
    }
}
