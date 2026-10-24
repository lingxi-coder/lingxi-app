//! `lingxi-bridge-server` (M10 S2) — the real local remote-engine entrypoint.
//!
//! Boots one local conversation over a loopback WebSocket for the Electron / iOS
//! shell, wiring the proven transport ([`bridge::McpEndpoint`]) to a real desktop
//! engine ([`harness_runtime::desktop::build`]) through the connection-scoped routing in
//! [`bridge_server`]:
//!
//! 1. Parse flags and resolve a deterministic
//!    [`harness_runtime::desktop::DesktopConfig`] from env/argv ([`bridge_server::boot`]).
//!    With `--api-key-stdin` or `--credential-stdin`, one bounded credential
//!    payload is read directly from stdin and is NEVER logged or copied into
//!    env/argv. A missing accepted credential source only warns (the server still boots for
//!    transport testing; a live turn 401s and surfaces as a `ClientEvent::Error`).
//! 2. Assemble a fully-bound [`bridge_server::server::BridgeConnection`] from a
//!    real [`harness_runtime::desktop::DesktopRuntime`] (turn driver + command router +
//!    connection-scoped `AdapterPermissionGate`).
//! 3. Start [`bridge::McpEndpoint::start_on_ephemeral_port_with_pump`] on
//!    `127.0.0.1:0`.
//! 4. Write the F2-04 discovery lockfile `~/.lingxi/bridge/<port>.lock` (port in
//!    the filename, a fresh `authToken` in the body) and enforce that SAME token
//!    on the endpoint. The lockfile is removed on shutdown (Drop-guard).
//! 5. Log the chosen port + lockfile path, then block until ctrl-c.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use bridge::McpEndpoint;
use bridge_server::boot::{self, BridgeArgs};
use client::protocol::events::{ClientEvent, ErrorKindDto};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--local-app-service-json") {
        return bridge_server::local_app_service::run().await;
    }
    if std::env::args().nth(1).as_deref() == Some("--app-agent") {
        return bridge_server::app_agent::run().await;
    }
    if std::env::args().nth(1).as_deref() == Some("--audio-service-json") {
        return bridge_server::audio_provider::run().await;
    }
    if std::env::args().nth(1).as_deref() == Some("--desktop-terminal") {
        return bridge_server::desktop_terminal::run().await;
    }
    #[cfg(any(unix, windows))]
    if harness_runtime::desktop::shell_supervisor::is_supervisor_invocation() {
        return harness_runtime::desktop::shell_supervisor::run_supervisor(
            harness_runtime::desktop::supervisor_exit_sink,
        )
        .await
        .map_err(Into::into);
    }
    #[cfg(any(unix, windows))]
    if let Ok(executable) = std::env::current_exe() {
        harness_runtime::desktop::shell_supervisor::enable_supervisor(executable);
    }
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

    if args.product_paths_json {
        let cwd = std::env::current_dir()
            .map_err(|_| anyhow::anyhow!("failed to resolve product paths directory"))?;
        let json = boot::product_paths_json(&cwd).map_err(|error| anyhow::anyhow!(error))?;
        println!("{json}");
        return Ok(());
    }

    if args.workspace_environment_json {
        let cwd = std::env::current_dir()
            .map_err(|_| anyhow::anyhow!("failed to resolve workspace environment directory"))?;
        let json = boot::workspace_environment_json(&cwd)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        println!("{json}");
        return Ok(());
    }

    if args.list_sessions_json {
        let cwd = std::env::current_dir()
            .map_err(|_| anyhow::anyhow!("failed to resolve session catalog directory"))?;
        let session_id = args
            .catalog_session_id
            .as_deref()
            .or(args.session_id.as_deref());
        let json = match args.session_scope {
            Some(scope) => boot::list_sessions_json_in_scope(&cwd, session_id, scope).await,
            None => boot::list_sessions_json(&cwd, session_id).await,
        }
        .map_err(|e| anyhow::anyhow!(e))?;
        println!("{json}");
        return Ok(());
    }

    if let Some(path) = &args.session_global_config_path {
        migrations::global_config::set_global_config_path_override(path.clone())
            .map_err(|_| anyhow::anyhow!("session global configuration owner is already fixed"))?;
    }
    if let Some(home) = &args.session_config_home {
        migrations::global_config::set_config_home_override(home.clone())
            .map_err(|_| anyhow::anyhow!("session configuration owner is already fixed"))?;
        lingxi_core::settings::loader::set_user_settings_path_override(home.join("settings.json"))
            .map_err(|_| anyhow::anyhow!("session user settings owner is already fixed"))?;
    }

    let _telemetry_guard = telemetry::otel::install_process("bridge-server", false);
    // Every HTTP and WebSocket client assembly builds trusts the OS store;
    // read it once, off this thread, while config and credentials resolve.
    platform_posix::http::prewarm_native_roots();

    // (2) Resolve config, then receive credentials over the dedicated stdin
    //     channel before assembly. They are never placed in argv/env or logged.
    let mut cfg = boot::resolve_desktop_config(&args);
    let mut provider_keys = BTreeMap::new();
    let mut openai_oauth = None;
    if args.credential_stdin {
        let envelope = boot::read_credential_envelope(&mut std::io::stdin().lock())
            .map_err(|e| anyhow::anyhow!(e))?;
        if let Some(api_key) = envelope.api_key {
            cfg.api_key = api_key;
        }
        provider_keys = envelope.provider_keys;
        openai_oauth = envelope.openai_oauth;
        cfg.injected_plugin_secrets = envelope.plugin_secrets;
    }
    if args.api_key_stdin {
        cfg.api_key = boot::read_api_key_line(&mut std::io::stdin().lock())
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    if boot::has_no_credential_source(&cfg) && provider_keys.is_empty() {
        tracing::info!("no parent credential supplied for this bridge session");
    }
    tracing::info!(
        cwd = %cfg.cwd.display(),
        model = %cfg.default_model,
        api_base = %cfg.api_base,
        "bridge-server: resolved desktop config"
    );

    let bound = boot::assemble_with_credentials(cfg, provider_keys, openai_oauth)
        .await
        .map_err(|e| anyhow::anyhow!("failed to assemble bridge runtime: {e}"))?;

    // Capture the ordered runtime lifecycle before moving the connection into
    // the endpoint. It outlives the pump and drains all accepted work before
    // the session claim and discovery record are released.
    let session_lifecycle = bound.session_lifecycle();
    let requested_exit = bound.wait_for_exit_requested();
    let semantic_worker_failure = bound.wait_for_policy_semantic_worker_failure();
    let semantic_failure_sink = bound.connection.policy_semantic_worker_failure_sink();

    // (3) Start the loopback WebSocket endpoint on an ephemeral port.
    let endpoint =
        match McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection)).await {
            Ok(endpoint) => endpoint,
            Err(error) => {
                drain_session_lifecycle(&session_lifecycle).await;
                return Err(anyhow::anyhow!("failed to bind loopback endpoint: {error}"));
            }
        };
    let port = endpoint.port();

    // (4) Write the F2-04 discovery lockfile and enforce its token on the
    //     endpoint. The workspace folder is the resolved cwd; the lockfile lives
    //     in `~/.lingxi/bridge` (created here). The token-enforce + Drop-guard
    //     reap are factored into `boot::publish_lockfile` so the headless serve
    //     test drives the SAME code with a temp dir.
    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let bridge_dir = match &args.bridge_dir {
        Some(path) => path.clone(),
        None => match boot::lingxi_config_home() {
            Some(home) => home.join("bridge"),
            None => {
                endpoint.shutdown().await;
                drain_session_lifecycle(&session_lifecycle).await;
                return Err(anyhow::anyhow!(
                    "no home directory to root the bridge lockfile"
                ));
            }
        },
    };
    if !bridge_dir.is_absolute() {
        endpoint.shutdown().await;
        drain_session_lifecycle(&session_lifecycle).await;
        return Err(anyhow::anyhow!(
            "bridge lockfile directory must be absolute"
        ));
    }
    let boot::ServedEndpoint {
        endpoint,
        lockfile_path,
        // `lock_guard` reaps the lockfile on shutdown OR panic (Drop-guard); it
        // is held to end-of-scope rather than dropped early.
        lock_guard,
    } = match boot::publish_lockfile_recoverable(endpoint, bridge_dir, vec![workspace]) {
        Ok(served) => served,
        Err(error) => {
            let (error, endpoint) = error.into_parts();
            endpoint.shutdown().await;
            drain_session_lifecycle(&session_lifecycle).await;
            return Err(anyhow::anyhow!(
                "failed to publish bridge lockfile: {error}"
            ));
        }
    };

    // (5) Log the chosen port + lockfile path (NEVER the token), then block.
    tracing::info!(
        port,
        lockfile = %lockfile_path.display(),
        url = %format!("ws://127.0.0.1:{port}/mcp"),
        "bridge-server: listening (ctrl-c to stop)"
    );

    // Desktop hosts terminate sidecars with SIGTERM on Unix. Listen for both
    // interactive Ctrl-C and host termination so the lockfile guard and socket
    // endpoint always receive a graceful teardown opportunity.
    let mut policy_refusal = None;
    let mut semantic_worker_error = None;
    let shutdown_signal = match tokio::select! {
        biased;
        failure = semantic_worker_failure => {
            semantic_failure_sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: failure.to_string().into(),
            }).await;
            semantic_worker_error = Some(failure);
            Ok("policy-semantic-worker-failure")
        },
        signal = wait_for_shutdown_signal() => signal,
        refusal = requested_exit => {
            policy_refusal = refusal;
            Ok("request-exit")
        },
    } {
        Ok(signal) => signal,
        Err(error) => {
            endpoint.shutdown().await;
            drain_session_lifecycle(&session_lifecycle).await;
            drop(lock_guard);
            return Err(error);
        }
    };
    tracing::info!(shutdown_signal, "bridge-server: shutdown signal received");

    // Explicit teardown: stop accepting, then `lock_guard` drops at end of scope
    // (removing the discovery file).
    endpoint.shutdown().await;
    drain_session_lifecycle(&session_lifecycle).await;
    drop(lock_guard);
    if let Some(refusal) = policy_refusal {
        return Err(anyhow::Error::new(refusal));
    }
    if let Some(failure) = semantic_worker_error {
        return Err(anyhow::Error::new((*failure).clone()));
    }
    Ok(())
}

async fn drain_session_lifecycle(lifecycle: &harness_runtime::desktop::DesktopSessionLifecycle) {
    let report = lifecycle.shutdown_and_drain().await;
    for error in report.errors {
        tracing::warn!(%error, "bridge-server session shutdown was not fully durable");
    }
}

async fn wait_for_shutdown_signal() -> anyhow::Result<&'static str> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| anyhow::anyhow!("failed to install SIGTERM handler: {e}"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|e| anyhow::anyhow!("failed to install ctrl-c handler: {e}"))?;
                Ok("ctrl-c")
            }
            signal = terminate.recv() => {
                signal.ok_or_else(|| anyhow::anyhow!("SIGTERM handler closed unexpectedly"))?;
                Ok("SIGTERM")
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|e| anyhow::anyhow!("failed to install ctrl-c handler: {e}"))?;
        Ok("ctrl-c")
    }
}
