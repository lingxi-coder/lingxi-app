//! S2 — end-to-end boot of the REAL bridge-server over a loopback WebSocket.
//!
//! Where the F2-06 suite (`e2e_permission_test.rs`) drives a hand-wired
//! orchestrator with a MOCK streaming client, this proves the PRODUCTION boot
//! path: `bridge_server::boot::assemble` builds a real
//! `harness_runtime::desktop::DesktopRuntime` from a deterministic `DesktopConfig`, binds
//! the production turn driver + command router, and the resulting connection is
//! served by the same `McpEndpoint` the binary uses.
//!
//! Two things are asserted end-to-end (no live network for the turn — the config
//! carries an EMPTY api key, so the bridge rejects the turn before entering the
//! provider retry path and surfaces one terminal `ClientEvent::Error`):
//!
//! - the opening `hello` handshake replies with a `ServerHello` (compatible
//!   versions), proving the served connection is the real F2-07 pump;
//! - a `SendPrompt` with no credential produces a terminal `Error` event rather
//!   than hanging — proving env-config → `harness_runtime::desktop::build` → serve is wired
//!   and the orchestrator's output stream reaches the connection's event sink.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge::wire::Frame;
use bridge::{BridgeRequest, Capabilities, ClientHello, McpEndpoint, BRIDGE_PROTOCOL_VERSION};
use bridge_server::boot;
use client::protocol::commands::ClientCommand;
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use harness_runtime::desktop::DesktopConfig;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::Message;

const TEST_TOKEN: &str = "serve-e2e-token-32chars0000000000";

/// A deterministic, env-free `DesktopConfig` rooted at a sandbox temp dir, with
/// an EMPTY api key (so a live turn fails fast rather than reaching the network).
fn sandbox_config() -> (tempfile::TempDir, DesktopConfig) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cwd = tmp.path().to_path_buf();
    let cfg = DesktopConfig {
        build_info: harness_runtime::desktop::BuildInfo::default(),
        enable_automation_scheduler: true,
        host_workspace_trusted: None,
        api_base: "https://api.anthropic.com".to_string(),
        api_key: String::new(),
        api_key_helper: None,
        // (M13) Inert auth-resolver inputs: no managed OAuth forcing, no
        // FD-inherited key.
        managed_oauth_only: false,
        anthropic_key_fd_present: false,
        cwd: cwd.clone(),
        lingxi_home: cwd.join(".lingxi"),
        default_model: "claude-sonnet-4-20250514".to_string(),
        // Deterministic across host machines: a dev keychain with real provider
        // keys must not trigger the connected-provider fallback mid-test.
        default_model_explicit: true,
        // Same hazard, one layer deeper. The native keychain is keyed by OS
        // USER, not by `lingxi_home`, so pointing that at a tempdir does NOT
        // isolate this boot: any provider key in the developer's login keychain
        // lands in `provider_availability`, `needs_credential_driver` then
        // returns false, the REAL turn driver runs, and the provider's auth
        // failure arrives as a text delta instead of the bridge's terminal
        // credential-required Error. Without this the credential assertions
        // below pass only on a machine that has never logged in.
        isolated_credential_storage: true,
        credential_storage_policy: platform_api::CredentialStoragePolicy::NativePreferred,
        recent_models: Vec::new(),
        fallback_model: None,
        custom_betas: Vec::new(),
        flag_settings: None,
        provider_profiles: None,
        routing: None,
        mcp_paths: vec![cwd.join(".mcp.json")],
        use_noop_permission_gate: false,
        deny_unresolved_ask: false,
        is_tty: false,
        initial_teammate_team_name: None,
        injected_permission_gate: None,
        injected_plugin_secrets: Default::default(),
        ask_user_question_tx: None,
        computer_access_tx: None,
        session_agent_observer: None,
        // `assemble` fills this with the connection's own `AudioBridge`.
        audio: None,
        session_started_as_coordinator: false,
        // Deterministic e2e: empty memory, never the real FS.
        memory_provider: None,
        permission_mode: permission::PermissionMode::Default,
        permission_mode_cli: None,
        permission_mode_preference: None,
        permission_mode_cli_explicit: false,
        allow_dangerously_skip_permissions: false,
        connect_prompt: None,
        max_turns: None,
        plan_mode_instructions: None,
        plans_directory: None,
        max_budget_usd: None,
        json_schema: None,
        // CLI headless system-prompt flags (715adc4e); None for this e2e fixture.
        system_prompt_override: None,
        append_system_prompt: None,
        session_id_override: None,
        session_writer_lease: None,
        disable_slash_commands: false,
        add_dir: Vec::new(),
        cli_mcp_servers: Vec::new(),
        strict_mcp_config: false,
        restricted: false,
        restricted_tools: None,
        exclude_dynamic_system_prompt_sections: false,
        setting_source_scope: (true, true),
        customization_gates: harness_runtime::desktop::CustomizationGates::default(),
        session_persistence: true,
        cli_agents_json: None,
        cli_agent: None,
        cli_plugin_dirs: Vec::new(),
        initial_effort: None,
        default_model_env_pinned: false,
        session_thinking: Default::default(),
        parent_session_id: None,
        bg_session_forker: None,
        worktree_launch: None,
        tmux_launch: None,
    };
    (tmp, cfg)
}

/// Open an authenticated WS connection to `port`.
async fn connect(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let req = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header("sec-websocket-protocol", "mcp")
        .header("x-lingxi-ide-authorization", TEST_TOKEN)
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    ws
}

async fn next_frame<S>(ws: &mut S) -> Frame
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("a frame must arrive within the timeout")
            .expect("stream must yield a message")
            .expect("message must not be a ws error");
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).expect("decode Frame"),
            _ => continue,
        }
    }
}

async fn send_frame<S>(ws: &mut S, frame: &Frame)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = serde_json::to_string(frame).expect("serialize frame");
    ws.send(Message::Text(text)).await.expect("send frame");
}

fn hello_frame() -> Frame {
    Frame::Request(BridgeRequest {
        id: 1,
        method: "hello".into(),
        params: serde_json::to_value(ClientHello {
            protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
            client_name: "serve-e2e-test/0.0.0".into(),
            capabilities: Capabilities::default(),
        })
        .unwrap(),
    })
}

fn submit(command: &ClientCommand) -> Frame {
    Frame::Request(BridgeRequest {
        id: 2,
        method: "submit".into(),
        params: serde_json::to_value(command).expect("serialize command"),
    })
}

/// The real boot path serves a connection whose opening `hello` is answered with
/// a `ServerHello`, and a credential-less turn terminates once, fast, through
/// upstream's own auth copy.
///
/// This used to assert a port-only `CredentialRequiredTurnDriver` that failed
/// the turn at the bridge boundary with a bespoke message naming
/// `--api-key-stdin`. That driver had been unreachable since the cold-boot
/// route was kept live (its boot-time predicate reduced to a catalog constant),
/// and claude-code has no such stage: an auth failure renders through
/// `orchestrator::api_error_copy`, which this port already mirrors byte for
/// byte. The port-only stage is gone; what is asserted here is the aligned
/// behaviour, and the properties that stage existed to protect are asserted
/// directly instead — fast, no provider call, nothing leaked, one terminal.
#[tokio::test]
async fn real_boot_handshakes_and_surfaces_turn_error() {
    let (_tmp, cfg) = sandbox_config();
    let bound = boot::assemble(cfg).await.expect("assemble must succeed");

    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(bound.connection))
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token(TEST_TOKEN.to_string());
    let mut ws = connect(endpoint.port()).await;

    // (1) Handshake — a compatible `hello` is answered with a `ServerHello`.
    send_frame(&mut ws, &hello_frame()).await;
    match next_frame(&mut ws).await {
        Frame::Response(resp) => {
            assert_eq!(resp.id, 1, "ServerHello echoes the hello request id");
            assert!(resp.error.is_none(), "compatible hello must not error");
            let server_hello: bridge::ServerHello =
                serde_json::from_value(resp.result.expect("ServerHello result"))
                    .expect("decode ServerHello");
            assert_eq!(server_hello.protocol_version, BRIDGE_PROTOCOL_VERSION);
        }
        other => panic!("expected ServerHello response, got {other:?}"),
    }

    // (2) A turn with no credential still runs, and the provider's rejection is
    //     rendered as upstream's auth copy rather than a port-specific error.
    send_frame(
        &mut ws,
        &submit(&ClientCommand::SendPrompt {
            text: "hello".into(),
            prompt_mode: None,
            images: Vec::new(),
            turn_id: None,
        }),
    )
    .await;

    // The 2s budget is the real assertion here: a credential rejection must not
    // enter provider retry backoff. Measured at ~41ms for the whole sequence.
    let frame = tokio::time::timeout(Duration::from_secs(2), next_frame(&mut ws))
        .await
        .expect("credential rejection must not enter provider retry backoff");
    match frame {
        Frame::Event(ClientEvent::TextDelta { text }) => {
            assert!(
                text.starts_with("Failed to authenticate."),
                "the turn must carry upstream's auth copy, got {text:?}"
            );
            assert!(!text.contains("sk-"), "error must not embed a key");
            assert!(
                !text.contains("api.anthropic.com"),
                "error must not expose provider details"
            );
        }
        other => panic!("expected upstream's auth copy, got {other:?}"),
    }

    // The turn ends itself, and ends charged for nothing: `api_calls == 0` is
    // what proves no provider request was issued, let alone retried.
    let mut ended = false;
    for _ in 0..8 {
        match tokio::time::timeout(Duration::from_secs(2), next_frame(&mut ws))
            .await
            .expect("the turn must terminate without backoff")
        {
            Frame::Event(ClientEvent::CostUpdate {
                api_calls,
                total_usd,
                ..
            }) => {
                assert_eq!(api_calls, 0, "no provider request may be issued");
                assert_eq!(total_usd, 0.0, "a rejected turn must not be charged");
            }
            Frame::Event(ClientEvent::TurnEnded { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("model_error"));
                ended = true;
                break;
            }
            Frame::Event(ClientEvent::MessageComplete { stop_reason, .. }) => {
                assert_eq!(stop_reason.as_deref(), Some("model_error"));
            }
            other => panic!("unexpected frame before TurnEnded: {other:?}"),
        }
    }
    assert!(ended, "the turn must reach TurnEnded");

    // `TurnEnded` is the protocol's terminal marker. No retry and no second
    // terminal may follow it for this submitted turn.
    let duplicate = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
    assert!(
        duplicate.is_err(),
        "credential rejection must emit exactly one frame"
    );

    endpoint.shutdown().await;
}

/// `resolve_desktop_config` always binds the adapter gate for a transport
/// (parity with `harness_runtime::desktop::build`'s `use_noop_permission_gate: false`
/// branch), regardless of the model override.
#[tokio::test]
async fn resolved_config_uses_adapter_gate() {
    let args = boot::BridgeArgs {
        model: Some("claude-x".into()),
        ..Default::default()
    };
    let cfg = boot::resolve_desktop_config(&args);
    assert!(!cfg.use_noop_permission_gate);
    assert_eq!(cfg.default_model, "claude-x");
    // cwd is the process cwd, never the bogus default path.
    assert_ne!(cfg.cwd, PathBuf::from("/dev/null"));
}
