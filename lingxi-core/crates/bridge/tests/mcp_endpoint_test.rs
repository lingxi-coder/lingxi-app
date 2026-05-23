//! Integration tests for `McpEndpoint`:
//! - rejects WebSocket upgrades that lack `X-Claude-Code-Ide-Authorization`,
//! - rejects upgrades whose token does NOT match the lockfile authToken.
//!
//! The happy-path JSON-RPC roundtrip test is added in Task 12.

use lingxi_bridge::McpEndpoint;

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
