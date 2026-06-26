//! `lingxi-bridge-server` (M10 S2) — the real local remote-engine entrypoint.
//!
//! Boots one local conversation over a loopback WebSocket for the Electron / iOS
//! shell, wiring the proven transport ([`bridge::McpEndpoint`]) to a real desktop
//! engine ([`engine_desktop::build`]) through the connection-scoped routing in
//! [`bridge_server`]:
//!
//! 1. Parse flags (`--cwd`, `--model`, `--help`) and resolve a deterministic
//!    [`engine_desktop::DesktopConfig`] from env/argv ([`bridge_server::boot`]).
//!    The LLM API key is read from `ANTHROPIC_API_KEY` at runtime and is NEVER
//!    logged. A missing key / provider only warns (the server still boots for
//!    transport testing; a live turn 401s and surfaces as a `ClientEvent::Error`).
//! 2. Assemble a fully-bound [`bridge_server::server::BridgeConnection`] from a
//!    real [`engine_desktop::DesktopRuntime`] (turn driver + command router +
//!    connection-scoped `AdapterPermissionGate`).
//! 3. Start [`bridge::McpEndpoint::start_on_ephemeral_port_with_pump`] on
//!    `127.0.0.1:0`.
//! 4. Write the F2-04 discovery lockfile `~/.lingxi/bridge/<port>.lock` (port in
//!    the filename, a fresh `authToken` in the body) and enforce that SAME token
//!    on the endpoint. The lockfile is removed on shutdown (Drop-guard).
//! 5. Log the chosen port + lockfile path, then block until ctrl-c.

#![forbid(unsafe_code)]

use std::sync::Arc;

use bridge::McpEndpoint;
use bridge_server::boot::{self, BridgeArgs};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // (1) Parse flags. `--help` (or a parse error) prints usage and exits.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match BridgeArgs::parse(&raw) {
        Ok(a) => a,
        Err(e) => {
            // Usage / error go to stdout so `--help` is greppable; not an error
            // for `--help`, an error (exit 2) for a bad flag.
            eprintln!("error: {e}\n");
            print!("{}", boot::usage());
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", boot::usage());
        return Ok(());
    }

    // (1.5) Honour `--cwd` BEFORE resolving the rest of the config so the
    //       settings / hook / `.mcp.json` loaders read the chosen dir.
    if let Some(cwd) = &args.cwd {
        std::env::set_current_dir(cwd)
            .map_err(|e| anyhow::anyhow!("failed to chdir into --cwd {}: {e}", cwd.display()))?;
    }

    // (2) Resolve config + assemble the real runtime/connection. The key value
    //     is read from the environment inside `resolve_desktop_config` and is
    //     never logged.
    let cfg = boot::resolve_desktop_config(&args);
    if boot::has_no_credential_source(&cfg) {
        // NON-SECRET warning: names the env var, never a value.
        tracing::warn!(
            "no {} set and no settings provider configured — the server will boot for transport \
             testing, but live turns will fail with a 401 until a credential is supplied",
            boot::API_KEY_ENV
        );
    }
    tracing::info!(
        cwd = %cfg.cwd.display(),
        model = %cfg.default_model,
        api_base = %cfg.api_base,
        "bridge-server: resolved desktop config"
    );

    let bound = boot::assemble(cfg)
        .await
        .map_err(|e| anyhow::anyhow!("failed to assemble bridge runtime: {e}"))?;

    // (3) Start the loopback WebSocket endpoint on an ephemeral port.
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind loopback endpoint: {e}"))?;
    let port = endpoint.port();

    // (4) Write the F2-04 discovery lockfile and enforce its token on the
    //     endpoint. The workspace folder is the resolved cwd; the lockfile lives
    //     in `~/.lingxi/bridge` (created here). The token-enforce + Drop-guard
    //     reap are factored into `boot::publish_lockfile` so the headless serve
    //     test drives the SAME code with a temp dir.
    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let bridge_dir = boot::claude_config_home()
        .ok_or_else(|| anyhow::anyhow!("no home directory to root the bridge lockfile"))?
        .join("bridge");
    std::fs::create_dir_all(&bridge_dir)
        .map_err(|e| anyhow::anyhow!("failed to create bridge lockfile dir: {e}"))?;
    let boot::ServedEndpoint {
        endpoint,
        lockfile_path,
        // `lock_guard` reaps the lockfile on shutdown OR panic (Drop-guard); it
        // is held to end-of-scope rather than dropped early.
        lock_guard,
    } = boot::publish_lockfile(endpoint, bridge_dir, vec![workspace]);

    // (5) Log the chosen port + lockfile path (NEVER the token), then block.
    tracing::info!(
        port,
        lockfile = %lockfile_path.display(),
        url = %format!("ws://127.0.0.1:{port}/mcp"),
        "bridge-server: listening (ctrl-c to stop)"
    );

    // Block until ctrl-c.
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| anyhow::anyhow!("failed to install ctrl-c handler: {e}"))?;
    tracing::info!("bridge-server: ctrl-c received, shutting down");

    // Explicit teardown: stop accepting, then `lock_guard` drops at end of scope
    // (removing the discovery file).
    endpoint.shutdown().await;
    drop(lock_guard);
    Ok(())
}
