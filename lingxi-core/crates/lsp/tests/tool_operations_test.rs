//! Integration tests for `tool_operations` — the nine LSP tool operations.
//!
//! The tests spin up an in-memory peer over `Connection::new_lsp` and
//! verify the wire shape of the requests + the 1-based → 0-based position
//! conversion + the didOpen-gating + the file-size cap.

use lingxi_jsonrpc::Connection;
use lingxi_lsp::client::LspClient;
use lingxi_lsp::tool_operations::{
    document_symbol, find_references, go_to_definition, go_to_implementation, hover,
    incoming_calls, language_id_for, outgoing_calls, position_from_one_based,
    prepare_call_hierarchy, workspace_symbol, LspOperation, LspOperationError,
    MAX_LSP_FILE_SIZE_BYTES,
};
use lingxi_lsp::OpenFileTracker;
use lingxi_traits::LspServerConfig;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

const FRAME_BUFFER: usize = 64 * 1024;

async fn read_one_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
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

/// Minimal config with `.rs → rust` mapping for the test fixtures.
fn rust_config() -> LspServerConfig {
    let mut map = HashMap::new();
    map.insert(".rs".to_string(), "rust".to_string());
    LspServerConfig {
        name: "rust-analyzer".to_string(),
        command: "rust-analyzer".to_string(),
        args: vec![],
        env: HashMap::new(),
        trigger_languages: vec!["rust".to_string()],
        root_dir_markers: vec!["Cargo.toml".to_string()],
        initialization_options: None,
        extension_to_language: map,
    }
}

// ---- pure-function unit tests -------------------------------------------

#[test]
fn position_from_one_based_subtracts_one() {
    let p = position_from_one_based(5, 3).expect("ok");
    assert_eq!(p.line, 4);
    assert_eq!(p.character, 2);
}

#[test]
fn position_from_one_based_rejects_zero_line() {
    let err = position_from_one_based(0, 3).expect_err("must error");
    assert!(matches!(err, LspOperationError::InvalidPosition { .. }));
}

#[test]
fn position_from_one_based_rejects_zero_character() {
    let err = position_from_one_based(2, 0).expect_err("must error");
    assert!(matches!(err, LspOperationError::InvalidPosition { .. }));
}

#[test]
fn language_id_for_uses_extension_map() {
    let cfg = rust_config();
    assert_eq!(language_id_for(&cfg, &PathBuf::from("/tmp/x.rs")), "rust");
}

#[test]
fn language_id_for_falls_back_to_plaintext() {
    let cfg = rust_config();
    assert_eq!(
        language_id_for(&cfg, &PathBuf::from("/tmp/x.unknownext")),
        "plaintext"
    );
}

#[test]
fn language_id_for_is_case_insensitive() {
    let cfg = rust_config();
    assert_eq!(language_id_for(&cfg, &PathBuf::from("/tmp/x.RS")), "rust");
}

// ---- end-to-end operation tests -----------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hover_sends_did_open_then_hover_with_zero_based_position() {
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    tokio::fs::write(temp.path(), b"fn main() {}\n")
        .await
        .unwrap();
    let file_path = temp.path().to_path_buf();

    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    let peer = tokio::spawn(async move {
        // First frame: didOpen notification.
        let did_open = read_one_frame(&mut peer_io).await;
        assert_eq!(did_open["method"], "textDocument/didOpen");
        assert!(did_open.get("id").is_none(), "notify has no id");
        assert_eq!(did_open["params"]["textDocument"]["languageId"], "rust");
        assert_eq!(did_open["params"]["textDocument"]["version"], 1);
        assert!(did_open["params"]["textDocument"]["text"]
            .as_str()
            .unwrap()
            .contains("fn main"));

        // Second frame: hover request.
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "textDocument/hover");
        // 1-based input (5, 3) → 0-based on wire (4, 2).
        assert_eq!(req["params"]["position"]["line"], 4);
        assert_eq!(req["params"]["position"]["character"], 2);

        write_frame(
            &mut peer_io,
            &json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {"contents": "hi"}
            }),
        )
        .await;
    });

    let result = hover(&client, &tracker, &cfg, &file_path, 5, 3)
        .await
        .expect("hover ok");
    assert_eq!(result.operation, LspOperation::Hover);
    assert!(result.raw["contents"].is_string());

    peer.await.expect("peer ok");
    // Tracker now knows the file was opened on rust-analyzer.
    assert!(
        tracker
            .is_open(
                "rust-analyzer",
                &lsp_types::Url::from_file_path(&file_path).unwrap()
            )
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hover_skips_did_open_when_already_tracked() {
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    tokio::fs::write(temp.path(), b"fn main() {}\n")
        .await
        .unwrap();
    let file_path = temp.path().to_path_buf();
    let uri = lsp_types::Url::from_file_path(&file_path).unwrap();

    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    tracker.mark_open("rust-analyzer", uri.clone()).await;
    let cfg = rust_config();

    let peer = tokio::spawn(async move {
        // Should only see the hover request, no didOpen first.
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "textDocument/hover");
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": req["id"], "result": {"contents": ""}}),
        )
        .await;
    });

    let _ = hover(&client, &tracker, &cfg, &file_path, 1, 1)
        .await
        .expect("hover ok");
    peer.await.expect("peer ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn definition_references_implementation_send_expected_method() {
    // Drive all three single-position operations through one peer.
    for (method, op_name) in [
        ("textDocument/definition", "definition"),
        ("textDocument/references", "references"),
        ("textDocument/implementation", "implementation"),
    ] {
        let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
        tokio::fs::write(temp.path(), b"// a\n").await.unwrap();
        let file_path = temp.path().to_path_buf();

        let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
        let (client_read, client_write) = tokio::io::split(client_io);
        let connection = Connection::new_lsp(client_read, client_write);
        let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
        let tracker = OpenFileTracker::new();
        let cfg = rust_config();

        let expected_method = method.to_string();
        let peer = tokio::spawn(async move {
            // didOpen.
            let _ = read_one_frame(&mut peer_io).await;
            // request.
            let req = read_one_frame(&mut peer_io).await;
            assert_eq!(
                req["method"], expected_method,
                "method should be {expected_method}"
            );
            if expected_method == "textDocument/references" {
                assert_eq!(req["params"]["context"]["includeDeclaration"], true);
            }
            write_frame(
                &mut peer_io,
                &json!({"jsonrpc": "2.0", "id": req["id"], "result": []}),
            )
            .await;
        });

        match op_name {
            "definition" => {
                go_to_definition(&client, &tracker, &cfg, &file_path, 1, 1)
                    .await
                    .unwrap();
            }
            "references" => {
                find_references(&client, &tracker, &cfg, &file_path, 1, 1, true)
                    .await
                    .unwrap();
            }
            "implementation" => {
                go_to_implementation(&client, &tracker, &cfg, &file_path, 1, 1)
                    .await
                    .unwrap();
            }
            other => panic!("unhandled op: {other}"),
        }
        peer.await.expect("peer ok");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_symbol_omits_position() {
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    tokio::fs::write(temp.path(), b"// a\n").await.unwrap();
    let file_path = temp.path().to_path_buf();

    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    let peer = tokio::spawn(async move {
        let _ = read_one_frame(&mut peer_io).await; // didOpen
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "textDocument/documentSymbol");
        assert!(req["params"]["textDocument"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("file:///"));
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": req["id"], "result": []}),
        )
        .await;
    });

    let res = document_symbol(&client, &tracker, &cfg, &file_path)
        .await
        .expect("documentSymbol ok");
    assert_eq!(res.operation, LspOperation::DocumentSymbol);
    peer.await.expect("peer ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_symbol_skips_did_open_gate() {
    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));

    let peer = tokio::spawn(async move {
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "workspace/symbol");
        assert_eq!(req["params"]["query"], "fn ");
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": req["id"], "result": []}),
        )
        .await;
    });

    let res = workspace_symbol(&client, Some("fn ".to_string()))
        .await
        .expect("workspace symbol ok");
    assert_eq!(res.operation, LspOperation::WorkspaceSymbol);
    assert!(res.file_uri.is_empty());
    peer.await.expect("peer ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_symbol_default_empty_query() {
    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));

    let peer = tokio::spawn(async move {
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["params"]["query"], "");
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": req["id"], "result": []}),
        )
        .await;
    });

    workspace_symbol(&client, None).await.expect("ok");
    peer.await.expect("peer ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_call_hierarchy_sends_position() {
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    tokio::fs::write(temp.path(), b"// a\n").await.unwrap();
    let file_path = temp.path().to_path_buf();

    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    let peer = tokio::spawn(async move {
        let _ = read_one_frame(&mut peer_io).await; // didOpen
        let req = read_one_frame(&mut peer_io).await;
        assert_eq!(req["method"], "textDocument/prepareCallHierarchy");
        assert_eq!(req["params"]["position"]["line"], 9);
        assert_eq!(req["params"]["position"]["character"], 0);
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": req["id"], "result": []}),
        )
        .await;
    });

    let res = prepare_call_hierarchy(&client, &tracker, &cfg, &file_path, 10, 1)
        .await
        .expect("ok");
    assert_eq!(res.operation, LspOperation::PrepareCallHierarchy);
    peer.await.expect("peer ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_outgoing_short_circuit_when_no_prepare_items() {
    for which in ["incoming", "outgoing"] {
        let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
        tokio::fs::write(temp.path(), b"// a\n").await.unwrap();
        let file_path = temp.path().to_path_buf();

        let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
        let (client_read, client_write) = tokio::io::split(client_io);
        let connection = Connection::new_lsp(client_read, client_write);
        let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
        let tracker = OpenFileTracker::new();
        let cfg = rust_config();

        let peer = tokio::spawn(async move {
            let _ = read_one_frame(&mut peer_io).await; // didOpen
            let prepare = read_one_frame(&mut peer_io).await;
            assert_eq!(prepare["method"], "textDocument/prepareCallHierarchy");
            // Return empty array — incoming/outgoing should not be sent.
            write_frame(
                &mut peer_io,
                &json!({"jsonrpc": "2.0", "id": prepare["id"], "result": []}),
            )
            .await;
        });

        let res = if which == "incoming" {
            incoming_calls(&client, &tracker, &cfg, &file_path, 1, 1)
                .await
                .unwrap()
        } else {
            outgoing_calls(&client, &tracker, &cfg, &file_path, 1, 1)
                .await
                .unwrap()
        };
        assert!(res.raw.is_array() && res.raw.as_array().unwrap().is_empty());
        peer.await.expect("peer ok");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_calls_two_step_round_trip() {
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    tokio::fs::write(temp.path(), b"// a\n").await.unwrap();
    let file_path = temp.path().to_path_buf();
    let uri_string = lsp_types::Url::from_file_path(&file_path)
        .unwrap()
        .to_string();

    let (client_io, mut peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    let uri_for_peer = uri_string.clone();
    let peer = tokio::spawn(async move {
        let _ = read_one_frame(&mut peer_io).await; // didOpen
        let prepare = read_one_frame(&mut peer_io).await;
        assert_eq!(prepare["method"], "textDocument/prepareCallHierarchy");
        write_frame(
            &mut peer_io,
            &json!({
                "jsonrpc": "2.0",
                "id": prepare["id"],
                "result": [{
                    "name": "foo",
                    "kind": 12, // Function
                    "uri": uri_for_peer,
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end":   {"line": 0, "character": 3}
                    },
                    "selectionRange": {
                        "start": {"line": 0, "character": 0},
                        "end":   {"line": 0, "character": 3}
                    }
                }]
            }),
        )
        .await;

        let incoming = read_one_frame(&mut peer_io).await;
        assert_eq!(incoming["method"], "callHierarchy/incomingCalls");
        assert_eq!(incoming["params"]["item"]["name"], "foo");
        write_frame(
            &mut peer_io,
            &json!({"jsonrpc": "2.0", "id": incoming["id"], "result": []}),
        )
        .await;
    });

    let res = incoming_calls(&client, &tracker, &cfg, &file_path, 1, 1)
        .await
        .expect("ok");
    assert_eq!(res.operation, LspOperation::IncomingCalls);
    peer.await.expect("peer ok");
}

#[tokio::test]
async fn file_too_large_is_rejected_before_did_open() {
    use std::os::unix::fs::FileExt;
    let temp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
    // Sparse file: seek + write a single byte > limit.
    let f = std::fs::File::options()
        .write(true)
        .open(temp.path())
        .unwrap();
    f.write_at(b"x", MAX_LSP_FILE_SIZE_BYTES + 1).unwrap();
    let file_path = temp.path().to_path_buf();

    let (client_io, _peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    let err = hover(&client, &tracker, &cfg, &file_path, 1, 1)
        .await
        .expect_err("must reject");
    match err {
        LspOperationError::FileTooLarge { size, limit } => {
            assert_eq!(limit, MAX_LSP_FILE_SIZE_BYTES);
            assert!(size > MAX_LSP_FILE_SIZE_BYTES);
        }
        other => panic!("expected FileTooLarge, got {other:?}"),
    }
    // Did NOT mark the tracker because we never sent didOpen.
    assert!(
        !tracker
            .is_open(
                "rust-analyzer",
                &lsp_types::Url::from_file_path(&file_path).unwrap()
            )
            .await
    );
}

#[tokio::test]
async fn zero_position_rejected_before_io() {
    // No need for a peer — validation happens before any network I/O.
    let (client_io, _peer_io) = duplex(FRAME_BUFFER);
    let (client_read, client_write) = tokio::io::split(client_io);
    let connection = Connection::new_lsp(client_read, client_write);
    let client = Arc::new(LspClient::new("rust-analyzer".to_string(), connection));
    let tracker = OpenFileTracker::new();
    let cfg = rust_config();

    // Use a path that does NOT exist — if validation ran AFTER stat, this
    // would surface as Io, not InvalidPosition.
    let path = PathBuf::from("/tmp/this/should/never/exist.rs");
    let err = hover(&client, &tracker, &cfg, &path, 0, 1)
        .await
        .expect_err("must reject");
    assert!(matches!(err, LspOperationError::InvalidPosition { .. }));
}
