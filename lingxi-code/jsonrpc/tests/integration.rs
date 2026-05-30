//! End-to-end integration tests for `lingxi-jsonrpc`.
//!
//! Uses an in-process bidirectional `Bytes` pipe (two mpsc channels) to
//! exercise both ends of a `Connection` without any real I/O dependency.
//! These tests live in `tests/integration.rs` rather than `src/`, so they
//! compile as a separate binary that can only use the crate's public API —
//! the same surface every protocol-specific consumer crate (M2-02b/c/d,
//! M2-03) will reach for.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use jsonrpc::{
    Connection, InboundHandler, LineCodec, LspCodec, Notification, Request, Response, RouterError,
};
use serde_json::json;
use tokio::sync::mpsc;

/// Build a bidirectional in-process pipe over two `Bytes` mpsc channels.
/// Returns `(a_in, a_out, b_in, b_out)`: end A's stream/sink and end B's
/// stream/sink, cross-wired so anything written to `a_out` shows up on
/// `b_in` and vice versa.
#[allow(clippy::type_complexity)]
fn duplex_pipe() -> (
    impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin + 'static,
    impl futures::Sink<Bytes, Error = std::io::Error> + Send + Unpin + 'static,
    impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin + 'static,
    impl futures::Sink<Bytes, Error = std::io::Error> + Send + Unpin + 'static,
) {
    use futures::stream::unfold;
    let (a_to_b_tx, a_to_b_rx) = mpsc::unbounded_channel::<Bytes>();
    let (b_to_a_tx, b_to_a_rx) = mpsc::unbounded_channel::<Bytes>();

    let a_in = Box::pin(unfold(b_to_a_rx, |mut rx| async move {
        rx.recv().await.map(|v| (Ok(v), rx))
    }));
    let a_out = Box::pin(futures::sink::unfold(
        a_to_b_tx,
        |tx, item: Bytes| async move {
            tx.send(item)
                .map_err(|_| std::io::Error::other("sink closed"))?;
            Ok::<_, std::io::Error>(tx)
        },
    ));
    let b_in = Box::pin(unfold(a_to_b_rx, |mut rx| async move {
        rx.recv().await.map(|v| (Ok(v), rx))
    }));
    let b_out = Box::pin(futures::sink::unfold(
        b_to_a_tx,
        |tx, item: Bytes| async move {
            tx.send(item)
                .map_err(|_| std::io::Error::other("sink closed"))?;
            Ok::<_, std::io::Error>(tx)
        },
    ));

    (a_in, a_out, b_in, b_out)
}

/// Trivial echo handler — returns the request's params back as the result.
struct Echo;

#[async_trait]
impl InboundHandler for Echo {
    async fn handle(&self, req: Request) -> Response {
        Response::success(req.id, req.params.unwrap_or(json!(null)))
    }
}

#[tokio::test]
async fn end_to_end_request_response_via_line_codec() {
    let (a_in, a_out, b_in, b_out) = duplex_pipe();
    let conn_a = Connection::builder(LineCodec::default()).build(a_in, a_out);
    let conn_b = Connection::builder(LineCodec::default()).build(b_in, b_out);

    conn_b.register_handler("echo", Arc::new(Echo)).await;

    let out: serde_json::Value = conn_a.call("echo", json!({"x": 1})).await.unwrap();
    assert_eq!(out, json!({"x": 1}));
}

#[tokio::test]
async fn end_to_end_request_response_via_lsp_codec() {
    let (a_in, a_out, b_in, b_out) = duplex_pipe();
    let conn_a = Connection::builder(LspCodec::default()).build(a_in, a_out);
    let conn_b = Connection::builder(LspCodec::default()).build(b_in, b_out);

    conn_b.register_handler("echo", Arc::new(Echo)).await;

    let out: serde_json::Value = conn_a.call("echo", json!({"y": 2})).await.unwrap();
    assert_eq!(out, json!({"y": 2}));
}

#[tokio::test]
async fn timeout_surfaces_when_peer_never_answers() {
    let (a_in, a_out, _b_in, _b_out) = duplex_pipe();
    let conn_a = Connection::builder(LineCodec::default()).build(a_in, a_out);
    // There is no B at all — `_b_in`/`_b_out` are dropped, so nothing ever
    // reads A's outbound frames and no Response can come back. A's call
    // hits the configured deadline and surfaces a `Timeout`.

    let err = conn_a
        .call_with_timeout::<_, serde_json::Value>("hangs", json!({}), Duration::from_millis(120))
        .await
        .unwrap_err();
    // Expect the inner `RouterError::Timeout` wrapped in `ConnectionError`.
    match err {
        jsonrpc::ConnectionError::Router(RouterError::Timeout(d)) => {
            assert_eq!(d, Duration::from_millis(120));
        }
        other => panic!("expected Timeout, got: {other:?}"),
    }
}

#[tokio::test]
async fn malformed_frame_surfaces_as_codec_error_on_reader() {
    use futures::stream::unfold;
    use jsonrpc::{BrokerError, CodecError, Dispatcher, Router};

    // Build a one-way feed straight into the broker's reader half: we
    // bypass the `Connection` builder so we can grab the `BrokerHandle`
    // and await its terminal reader error.
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
    let (out_tx, _out_rx) = mpsc::unbounded_channel::<Bytes>();

    let in_stream = Box::pin(unfold(in_rx, |mut rx| async move {
        rx.recv().await.map(|v| (v, rx))
    }));
    let out_sink = Box::pin(futures::sink::unfold(
        out_tx,
        |tx, item: Bytes| async move {
            tx.send(item)
                .map_err(|_| std::io::Error::other("sink closed"))?;
            Ok::<_, std::io::Error>(tx)
        },
    ));

    let (router_tx, router_rx) = mpsc::unbounded_channel();
    let router = Router::new(router_tx);
    let dispatcher = Dispatcher::new();
    let (handle, _notif) = jsonrpc::broker::spawn(
        in_stream,
        out_sink,
        LspCodec::default(),
        router,
        dispatcher,
        router_rx,
    );

    // Send a frame whose header is unrecognized — LSP codec rejects it
    // with `MalformedHeader` because there's no `Content-Length:`.
    in_tx
        .send(Ok(Bytes::from_static(b"X-Bad-Header: 5\r\n\r\nhello")))
        .unwrap();
    // Close the inbound stream so the reader loop terminates promptly
    // after surfacing the codec error.
    drop(in_tx);

    let (reader_outcome, _writer) = handle.join().await;
    match reader_outcome {
        Err(BrokerError::Codec(CodecError::MalformedHeader(_))) => {}
        other => panic!("expected MalformedHeader, got: {other:?}"),
    }
}

#[tokio::test]
async fn one_hundred_concurrent_calls_resolve_correctly() {
    let (a_in, a_out, b_in, b_out) = duplex_pipe();
    // Wrap A's Connection in `Arc` so we can share it across spawned tasks
    // without lifetime gymnastics. All `Connection` methods take `&self`,
    // so `Arc<Connection>` lets every task drive the same handle.
    let conn_a = Arc::new(Connection::builder(LineCodec::default()).build(a_in, a_out));
    let conn_b = Connection::builder(LineCodec::default()).build(b_in, b_out);

    conn_b.register_handler("echo", Arc::new(Echo)).await;

    let mut handles = Vec::with_capacity(100);
    for i in 0..100i64 {
        let c = Arc::clone(&conn_a);
        handles.push(tokio::spawn(async move {
            let out: serde_json::Value = c.call("echo", json!({"i": i})).await.expect("call");
            out["i"].as_i64().unwrap()
        }));
    }

    let mut got: Vec<i64> = futures::future::try_join_all(handles).await.unwrap();
    got.sort_unstable();
    assert_eq!(got, (0..100i64).collect::<Vec<_>>());
}

#[tokio::test]
async fn inbound_notification_received_via_broadcast_subscription() {
    let (a_in, a_out, b_in, b_out) = duplex_pipe();
    let conn_a = Connection::builder(LineCodec::default()).build(a_in, a_out);
    let conn_b = Connection::builder(LineCodec::default()).build(b_in, b_out);

    let mut sub = conn_b.notifications();
    conn_a.notify("ping", json!({"k": 1})).unwrap();

    let n: Notification = sub.recv().await.unwrap();
    assert_eq!(n.method, "ping");
    assert_eq!(n.params, Some(json!({"k": 1})));
}

#[tokio::test]
async fn inbound_request_unknown_method_yields_method_not_found_to_caller() {
    let (a_in, a_out, b_in, b_out) = duplex_pipe();
    let conn_a = Connection::builder(LineCodec::default()).build(a_in, a_out);
    // B is alive but has no handlers — its `Dispatcher::dispatch` returns a
    // `MethodNotFound` response, which the broker writes back to A.
    let _conn_b = Connection::builder(LineCodec::default()).build(b_in, b_out);

    let err = conn_a
        .call_with_timeout::<_, serde_json::Value>("nope", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    match err {
        jsonrpc::ConnectionError::Router(RouterError::Remote(e)) => {
            assert_eq!(e.code, jsonrpc::METHOD_NOT_FOUND);
            assert!(e.message.contains("nope"));
        }
        other => panic!("expected Remote(MethodNotFound), got: {other:?}"),
    }
}
