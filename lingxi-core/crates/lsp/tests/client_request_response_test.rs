//! Integration test: `LspClient::initialize` over an in-memory duplex pipe.
//!
//! Wire-shape verification: the client must send a JSON-RPC request named
//! `"initialize"` with `process_id`, `root_uri`, `capabilities` per the LSP
//! 3.17 spec, framed with `Content-Length` headers, then follow with an
//! `"initialized"` notification once the server has responded.

use lingxi_jsonrpc::Connection;
use lingxi_lsp::client::LspClient;
use lsp_types::ServerCapabilities;
use serde_json::{json, Value};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

const FRAME_BUFFER: usize = 64 * 1024;

/// Read one `Content-Length`-framed JSON-RPC message off a reader.
async fn read_one_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    // Read until \r\n\r\n
    loop {
        reader.read_exact(&mut byte).await.expect("read header");
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = std::str::from_utf8(&header).expect("utf8 header");
    let len: usize = header_str
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .expect("content-length present")
        .trim()
        .parse()
        .expect("parse length");
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.expect("read body");
    serde_json::from_slice(&body).expect("parse json")
}

/// Write a `Content-Length`-framed JSON-RPC response.
async fn write_frame(writer: &mut (impl tokio::io::AsyncWrite + Unpin), val: &Value) {
    let body = serde_json::to_vec(val).expect("serialize");
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .await
        .expect("write header");
    writer.write_all(&body).await.expect("write body");
    writer.flush().await.expect("flush");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_sends_canonical_lsp_params_and_receives_capabilities() {
    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);

    let connection = Connection::new_lsp(client_read, client_write);
    let client = LspClient::new("rust-analyzer".to_string(), connection);

    // Spawn the peer: read initialize, respond with capabilities.
    let peer_task = tokio::spawn(async move {
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "initialize");
        let id = req["id"].clone();
        assert!(id.is_number() || id.is_string(), "id present");
        let params = &req["params"];
        // process_id may be null per LSP spec; we just verify the field exists.
        assert!(
            params.get("processId").is_some(),
            "processId field present"
        );
        assert_eq!(params["rootUri"], "file:///tmp/workspace");
        assert!(
            params["capabilities"].is_object(),
            "capabilities object present"
        );

        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "capabilities": {
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                }
            }
        });
        write_frame(&mut peer_io, &response).await;

        // Drain the follow-up `initialized` notification so the connection
        // does not hang waiting to write into a full buffer.
        let notif = read_one_frame(&mut peer_io).await;
        assert_eq!(notif["method"], "initialized");
        assert!(notif.get("id").is_none(), "notifications have no id");
    });

    let caps: ServerCapabilities = client
        .initialize("file:///tmp/workspace")
        .await
        .expect("initialize ok");
    assert!(caps.hover_provider.is_some(), "hover advertised");
    assert!(caps.definition_provider.is_some(), "definition advertised");
    assert_eq!(client.name(), "rust-analyzer");

    peer_task.await.expect("peer task ok");
}
