//! Integration tests for `McpEndpoint` and `IdeBridge`:
//! - rejects WebSocket upgrades that lack `X-Claude-Code-Ide-Authorization`,
//! - rejects upgrades whose token does NOT match the lockfile authToken,
//! - happy path: `IdeBridge::start` writes the lockfile, the bound port
//!   accepts the WS upgrade with the matching token, and `shutdown` removes
//!   the lockfile via the embedded `LockfileGuard`.

use lingxi_bridge::{IdeBridge, IdeLockfile, McpEndpoint};
use std::path::PathBuf;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;

#[tokio::test]
async fn rejects_upgrade_without_auth_header() {
    let endpoint = McpEndpoint::start_on_ephemeral_port()
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token("expected-token-1234567890abcdef0123".into());

    let url = format!("http://127.0.0.1:{}/mcp", endpoint.port());
    let res = reqwest::Client::new()
        .get(&url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .expect("HTTP send");
    assert_eq!(res.status(), 401, "missing auth header must yield 401");
    let body = res.text().await.unwrap();
    assert_eq!(
        body, "unauthorized\n",
        "401 body must be literal 'unauthorized\\n'"
    );

    endpoint.shutdown().await;
}

#[tokio::test]
async fn rejects_upgrade_with_wrong_token() {
    let endpoint = McpEndpoint::start_on_ephemeral_port()
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token("the-correct-token-32chars0000000".into());

    let url = format!("http://127.0.0.1:{}/mcp", endpoint.port());
    let res = reqwest::Client::new()
        .get(&url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("x-claude-code-ide-authorization", "WRONG-TOKEN")
        .send()
        .await
        .expect("HTTP send");
    assert_eq!(res.status(), 401, "wrong token must yield 401");

    endpoint.shutdown().await;
}

#[tokio::test]
async fn bridge_writes_lockfile_then_round_trips_ws_upgrade() {
    let bridge = IdeBridge::start(vec![PathBuf::from(std::env::current_dir().unwrap())])
        .await
        .expect("start bridge");
    let path = bridge.lockfile_path().clone();
    let expected_token = bridge.auth_token().to_string();
    let port = bridge.port();

    // 1. Lockfile exists on disk with the auth token we expect.
    assert!(path.exists(), "lockfile must exist after start");
    let (body, port_from_filename) = IdeLockfile::read(&path).expect("read lockfile");
    assert_eq!(port_from_filename, port, "filename port must match bind port");
    assert_eq!(body.auth_token, expected_token);
    assert_eq!(body.transport, "ws");
    assert_eq!(body.ide_name, "LingXi");

    // 2. WebSocket upgrade with the right token succeeds. We build the
    // request via `http::Request` so we can attach the auth header
    // (`tokio_tungstenite::connect_async` accepts anything that implements
    // `IntoClientRequest`, and `http::Request<()>` is the canonical form).
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
        .header("x-claude-code-ide-authorization", expected_token.as_str())
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    // Echo of the subprotocol back from the server.
    let proto = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok());
    assert_eq!(proto, Some("mcp"));
    drop(ws);

    // 3. Bridge shutdown removes the lockfile.
    bridge.shutdown().await;
    // Give Drop a moment (LockfileGuard is sync but runs on the task that
    // drops `self`).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !path.exists(),
        "lockfile must be removed after IdeBridge::shutdown"
    );
}
