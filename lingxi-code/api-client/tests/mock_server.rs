//! Shared mock-server scaffold for the api-client integration test files.
//!
//! Spawns a `127.0.0.1:0` hyper server that returns scripted responses in
//! FIFO order. Built on the same `hyper 1` + `hyper-util` pattern as
//! `lingxi-test-harness::contracts::http::spawn_echo_server`.

#![allow(dead_code)] // test-only helper

use async_trait::async_trait;
use futures::stream::Stream;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use protocol::{HttpRequest, HttpResponse, SseEvent};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use traits::{HttpError, HttpTransport};

#[derive(Clone, Debug)]
pub struct MockResp {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

pub struct Mock {
    pub base_url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    attempts: Arc<AtomicU8>,
    transport: Arc<RealTransport>,
}

impl Mock {
    #[must_use]
    pub fn attempt_count(&self) -> u8 {
        self.attempts.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn transport(&self) -> Arc<RealTransport> {
        self.transport.clone()
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn spawn_mock(responses: Vec<MockResp>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base_url = format!("http://{addr}");

    let queue: Arc<Mutex<VecDeque<MockResp>>> = Arc::new(Mutex::new(responses.into()));
    let attempts = Arc::new(AtomicU8::new(0));
    let q = queue.clone();
    let a = attempts.clone();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept = listener.accept() => {
                    let Ok((stream, _)) = accept else { break };
                    let io = TokioIo::new(stream);
                    let q2 = q.clone();
                    let a2 = a.clone();
                    tokio::spawn(async move {
                        let svc = service_fn(move |_req: Request<hyper::body::Incoming>| {
                            let q3 = q2.clone();
                            let a3 = a2.clone();
                            async move {
                                a3.fetch_add(1, Ordering::SeqCst);
                                let next = q3.lock().unwrap().pop_front();
                                let resp = match next {
                                    Some(m) => {
                                        let mut b = Response::builder().status(
                                            StatusCode::from_u16(m.status).unwrap_or(StatusCode::OK),
                                        );
                                        for (k, v) in m.headers {
                                            b = b.header(k, v);
                                        }
                                        b.body(Full::new(Bytes::from(m.body))).unwrap()
                                    }
                                    None => Response::builder()
                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                        .body(Full::new(Bytes::from_static(b"queue drained")))
                                        .unwrap(),
                                };
                                Ok::<_, std::convert::Infallible>(resp)
                            }
                        });
                        let _ = http1::Builder::new().serve_connection(io, svc).await;
                    });
                }
            }
        }
    });

    Mock {
        base_url,
        shutdown: Some(shutdown_tx),
        attempts,
        transport: Arc::new(RealTransport::new()),
    }
}

/// Thin wrapper over `reqwest::Client` so the tests use a real HTTP transport
/// (rather than `MockHttpTransport`, which is scripted-response-only and
/// doesn't speak HTTP at the wire level).
pub struct RealTransport {
    client: reqwest::Client,
}

impl RealTransport {
    fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest build"),
        }
    }
}

#[async_trait]
impl HttpTransport for RealTransport {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = match req.method {
            protocol::HttpMethod::Get => reqwest::Method::GET,
            protocol::HttpMethod::Post => reqwest::Method::POST,
            _ => return Err(HttpError::InvalidRequest("unsupported method".into())),
        };
        let mut rb = self.client.request(method, &req.url);
        for (k, v) in req.headers {
            rb = rb.header(k, v);
        }
        if let Some(body) = req.body {
            rb = rb.body(body);
        }
        let resp = rb.send().await.map_err(|e| {
            if e.is_timeout() {
                HttpError::Timeout(Duration::from_secs(5))
            } else {
                HttpError::Connection(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn stream_sse(&self, _req: HttpRequest) -> Result<traits::http::SseStream, HttpError> {
        let s: Pin<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send>> =
            Box::pin(futures::stream::empty());
        Ok(s)
    }
}
